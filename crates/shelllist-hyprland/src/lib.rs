pub mod preferences;
pub mod work_area;
#[cfg(test)]
mod work_area_tests;

use std::{
    env,
    future::Future,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use tokio::{
    fs,
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::mpsc,
    time,
};

const IPC_TIMEOUT: Duration = Duration::from_secs(2);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const COMMAND_SOCKET: &str = ".socket.sock";
const EVENT_SOCKET: &str = ".socket2.sock";
const MAX_EVENT_BYTES: u64 = 64 * 1024;
const MAX_REPLY_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Client {
    runtime_dir: PathBuf,
    signature: Option<String>,
}

impl Default for Client {
    fn default() -> Self {
        Self::from_environment()
    }
}

impl Client {
    #[must_use]
    pub fn from_environment() -> Self {
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_runtime_dir);
        Self::new(runtime_dir, env::var("HYPRLAND_INSTANCE_SIGNATURE").ok())
    }

    #[must_use]
    pub fn new(runtime_dir: PathBuf, signature: Option<String>) -> Self {
        Self {
            runtime_dir,
            signature: signature.filter(|value| valid_signature(value)),
        }
    }

    pub async fn request(&self, command: &str) -> Result<String> {
        bounded_ipc(async {
            let stream = self.connect(COMMAND_SOCKET).await?;
            // Fail over only while connecting, never replay a command that may
            // already have changed compositor state.
            request_socket(stream, command).await
        })
        .await
    }

    pub async fn work_areas(
        &self,
    ) -> Result<std::collections::BTreeMap<String, work_area::Insets>> {
        let reply = self.request("[[BATCH]]j/monitors;j/workspaces;j/workspacerules;j/clients;j/getoption general:gaps_out").await?;
        work_area::parse_work_areas(&reply)
    }

    /// Read animation preferences through the same bounded native IPC transport.
    pub async fn preferences(&self) -> Result<preferences::Preferences> {
        preferences::parse(&self.request("j/getoption animations:enabled").await?)
    }

    pub async fn event_socket(&self) -> Result<UnixStream> {
        bounded_ipc(self.connect(EVENT_SOCKET)).await
    }

    async fn connect(&self, socket: &str) -> Result<UnixStream> {
        let candidates =
            socket_candidates(&self.roots(), self.signature.as_deref(), socket).await?;
        connect_candidates(&candidates).await
    }

    fn roots(&self) -> Vec<PathBuf> {
        let runtime = self.runtime_dir.join("hypr");
        let temporary = PathBuf::from("/tmp/hypr");
        if runtime == temporary {
            vec![runtime]
        } else {
            vec![runtime, temporary]
        }
    }
}

// One budget includes discovery, connection and (for commands) the entire
// write/read exchange. Timing out drops the owned stream and pending discovery.
async fn bounded_ipc<T>(operation: impl Future<Output = Result<T>>) -> Result<T> {
    time::timeout(IPC_TIMEOUT, operation)
        .await
        .context("Hyprland IPC timed out")?
}

// Prefer the requested instance in either root, then sorted fallback instances
// in runtime-root / legacy-root order. Socket existence alone is not liveness.
async fn socket_candidates(
    roots: &[PathBuf],
    signature: Option<&str>,
    socket: &str,
) -> Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();
    if let Some(signature) = signature {
        for root in roots {
            add_socket(&mut candidates, root.join(signature).join(socket)).await;
        }
    }
    for root in roots {
        let Ok(mut entries) = fs::read_dir(root).await else {
            continue;
        };
        let mut paths = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            paths.push(entry.path().join(socket));
        }
        paths.sort();
        for path in paths {
            add_socket(&mut candidates, path).await;
        }
    }
    Ok(candidates)
}

async fn add_socket(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if !candidates.contains(&path)
        && fs::metadata(&path)
            .await
            .is_ok_and(|m| m.file_type().is_socket())
    {
        candidates.push(path);
    }
}

async fn connect_candidates(candidates: &[PathBuf]) -> Result<UnixStream> {
    let mut last_error = None;
    for socket in candidates {
        match UnixStream::connect(socket).await {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                last_error = Some(
                    anyhow::Error::new(error)
                        .context(format!("connect to Hyprland socket {}", socket.display())),
                )
            }
        }
    }
    match last_error {
        Some(error) => Err(error.context("no reachable Hyprland IPC instance is available")),
        None => bail!("no active Hyprland IPC instance is available"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Connected,
    Disconnected,
    Message(String),
}

/// Compatibility stream for clients that reconcile on every compositor event.
pub async fn watch_events(sender: mpsc::Sender<()>) {
    watch_events_with(Client::default(), sender, |_| ()).await;
}

/// Includes connection transitions so consumers can reduce healthy polling,
/// filter unrelated events, and refresh immediately when delivery recovers.
pub async fn watch_events_detailed(sender: mpsc::Sender<Event>) {
    watch_events_with(Client::default(), sender, |event| event).await;
}

async fn watch_events_with<T, F>(client: Client, sender: mpsc::Sender<T>, map: F)
where
    F: Fn(Event) -> T,
{
    let watch = async {
        loop {
            if let Ok(stream) = client.event_socket().await
                && forward_events(stream, &sender, &map).await.is_err()
            {
                return;
            }
            // Back off even when a socket accepts and immediately closes.
            time::sleep(RECONNECT_DELAY).await;
        }
    };
    // Receiver closure cancels connection attempts, reads, sends and backoff.
    tokio::select! { _ = sender.closed() => {}, _ = watch => {} }
}

async fn forward_events<T>(
    stream: UnixStream,
    sender: &mpsc::Sender<T>,
    map: &impl Fn(Event) -> T,
) -> Result<(), mpsc::error::SendError<T>> {
    sender.send(map(Event::Connected)).await?;
    let mut reader = BufReader::new(stream);
    while let Ok(Some(line)) = read_event(&mut reader).await {
        sender.send(map(Event::Message(line))).await?;
    }
    sender.send(map(Event::Disconnected)).await
}

// Bound an unterminated line too: read_line/lines would otherwise grow forever.
// No idle timeout: a healthy compositor is allowed to emit no events.
async fn read_event(reader: &mut (impl AsyncBufRead + Unpin)) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_EVENT_BYTES + 1)
        .read_until(b'\n', &mut bytes)
        .await?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_EVENT_BYTES,
        "Hyprland event is too large"
    );
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    Ok(Some(String::from_utf8(bytes)?))
}

async fn request_socket(mut stream: UnixStream, command: &str) -> Result<String> {
    stream.write_all(command.as_bytes()).await?;
    stream.shutdown().await?;
    let mut response = String::new();
    stream
        .take(MAX_REPLY_BYTES + 1)
        .read_to_string(&mut response)
        .await?;
    anyhow::ensure!(
        response.len() as u64 <= MAX_REPLY_BYTES,
        "Hyprland reply is too large"
    );
    Ok(response)
}

fn default_runtime_dir() -> PathBuf {
    let uid = std::fs::metadata("/proc/self").map_or(0, |process| process.uid());
    PathBuf::from(format!("/run/user/{uid}"))
}

fn valid_signature(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_-.".contains(character))
}

#[cfg(test)]
mod tests {
    use super::{
        COMMAND_SOCKET, Client, EVENT_SOCKET, Event, connect_candidates, socket_candidates,
        valid_signature, watch_events_with,
    };
    use std::time::Duration;
    use tokio::{
        fs,
        io::{AsyncReadExt, AsyncWriteExt},
        sync::mpsc,
        time,
    };

    async fn socket_fixture(socket: &str) -> (tempfile::TempDir, Client, tokio::net::UnixListener) {
        let root = tempfile::tempdir().unwrap();
        let instance = root.path().join("hypr/test");
        fs::create_dir_all(&instance).await.unwrap();
        let listener = tokio::net::UnixListener::bind(instance.join(socket)).unwrap();
        let client = Client::new(root.path().into(), Some("test".into()));
        (root, client, listener)
    }

    pub(crate) async fn command_server(
        expected: &'static str,
        reply: impl AsRef<[u8]> + Send + 'static,
    ) -> (tempfile::TempDir, Client, tokio::task::JoinHandle<()>) {
        let (root, client, listener) = socket_fixture(COMMAND_SOCKET).await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut command = String::new();
            stream.read_to_string(&mut command).await.unwrap();
            assert_eq!(command, expected);
            stream.write_all(reply.as_ref()).await.unwrap();
        });
        (root, client, server)
    }

    #[tokio::test]
    async fn request_uses_bounded_socket_protocol_without_a_process() {
        let (root, client, server) = command_server("j/clients", "[]").await;
        assert_eq!(client.request("j/clients").await.unwrap(), "[]");
        server.await.unwrap();
        let root = root.path().join("hypr");
        for signature in [None, Some("missing")] {
            assert_eq!(
                socket_candidates(std::slice::from_ref(&root), signature, COMMAND_SOCKET)
                    .await
                    .unwrap(),
                [root.join("test").join(COMMAND_SOCKET)]
            );
            assert!(
                socket_candidates(std::slice::from_ref(&root), signature, EVENT_SOCKET)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn ipc_deadline_cancels_pending_operations() {
        let marker = std::sync::Arc::new(());
        let held = marker.clone();
        let started = time::Instant::now();
        let error = super::bounded_ipc(async move {
            let _held = held;
            std::future::pending::<anyhow::Result<()>>().await
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert_eq!(time::Instant::now() - started, super::IPC_TIMEOUT);
        assert_eq!(std::sync::Arc::strong_count(&marker), 1);
    }

    #[tokio::test]
    async fn request_deadline_includes_waiting_for_a_reply() {
        let (_root, client, listener) = socket_fixture(COMMAND_SOCKET).await;
        let request = tokio::spawn(async move { client.request("j/clients").await });
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut command = String::new();
        stream.read_to_string(&mut command).await.unwrap();
        assert_eq!(command, "j/clients");
        time::pause(); // discovery/connection finished; peer deliberately never replies
        assert!(
            request
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
    }

    #[tokio::test]
    async fn replies_and_event_lines_have_explicit_size_and_encoding_bounds() {
        let (_root, client, server) =
            command_server("j/clients", "x".repeat(super::MAX_REPLY_BYTES as usize + 1)).await;
        assert!(
            client
                .request("j/clients")
                .await
                .unwrap_err()
                .to_string()
                .contains("too large")
        );
        server.await.unwrap();
        for (mut input, expected) in [
            (b"event>>1\n".as_slice(), Some("event>>1")),
            (b"event>>2\r\n", Some("event>>2")),
            (b"partial", Some("partial")),
            (b"\n", Some("")),
            (b"", None),
        ] {
            assert_eq!(
                super::read_event(&mut input).await.unwrap().as_deref(),
                expected
            );
        }
        assert!(super::read_event(&mut b"\xff\n".as_slice()).await.is_err());
        let mut boundary = vec![b'x'; super::MAX_EVENT_BYTES as usize];
        *boundary.last_mut().unwrap() = b'\n';
        assert!(
            super::read_event(&mut boundary.as_slice())
                .await
                .unwrap()
                .is_some()
        );
        for terminated in [false, true] {
            let mut oversized = vec![b'x'; super::MAX_EVENT_BYTES as usize + 1];
            if terminated {
                *oversized.last_mut().unwrap() = b'\n';
            }
            assert!(
                super::read_event(&mut oversized.as_slice())
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("too large")
            );
        }
    }

    #[tokio::test]
    async fn event_limits_and_receiver_closure_cancel_reads_and_backoff() {
        for length in [12, super::MAX_EVENT_BYTES as usize + 1] {
            let (_root, client, listener) = socket_fixture(EVENT_SOCKET).await;
            let (sender, mut events) = mpsc::channel(8);
            let watcher = tokio::spawn(watch_events_with(client, sender, |event| event));
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(events.recv().await, Some(Event::Connected));
            stream.write_all(&vec![b'x'; length]).await.unwrap();
            if length > super::MAX_EVENT_BYTES as usize {
                assert_eq!(
                    time::timeout(Duration::from_secs(1), events.recv())
                        .await
                        .unwrap(),
                    Some(Event::Disconnected)
                );
            }
            // Cancel either a partial read or the oversized-frame backoff.
            drop(events);
            time::timeout(Duration::from_millis(100), watcher)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn discovery_is_ordered_and_connect_skips_stale_sockets() {
        let root = tempfile::tempdir().unwrap();
        let roots = [root.path().join("runtime"), root.path().join("legacy")];
        for socket in [COMMAND_SOCKET, EVENT_SOCKET] {
            let mut listeners = Vec::new();
            for (base, name) in [(0, "z"), (0, "b"), (0, "preferred"), (1, "preferred")] {
                let directory = roots[base].join(name);
                fs::create_dir_all(&directory).await.unwrap();
                listeners.push(tokio::net::UnixListener::bind(directory.join(socket)).unwrap());
            }
            let candidates = socket_candidates(&roots, Some("preferred"), socket)
                .await
                .unwrap();
            assert_eq!(
                candidates,
                [
                    roots[0].join("preferred").join(socket),
                    roots[1].join("preferred").join(socket),
                    roots[0].join("b").join(socket),
                    roots[0].join("z").join(socket)
                ]
            );
            drop(listeners.pop()); // stale preferred socket in legacy root
            drop(listeners.pop()); // stale preferred socket in runtime root
            let stream = connect_candidates(&candidates).await.unwrap();
            let (_peer, _) = listeners[1].accept().await.unwrap(); // sorted fallback b, not z
            drop(stream);
            fs::create_dir_all(roots[0].join("file")).await.unwrap();
            fs::write(roots[0].join("file").join(socket), b"not a socket")
                .await
                .unwrap();
            assert_eq!(
                socket_candidates(&roots, Some("preferred"), socket)
                    .await
                    .unwrap(),
                candidates
            );
        }
    }

    #[tokio::test]
    async fn command_errors_after_connect_do_not_replay_on_another_instance() {
        let (root, client, server) = command_server("dispatch test", [0xff]).await;
        let fallback = root.path().join("hypr/fallback");
        fs::create_dir_all(&fallback).await.unwrap();
        let fallback = tokio::net::UnixListener::bind(fallback.join(COMMAND_SOCKET)).unwrap();
        // Invalid UTF-8 reply after command execution must not trigger failover.
        assert!(client.request("dispatch test").await.is_err());
        server.await.unwrap();
        assert!(
            time::timeout(Duration::from_millis(50), fallback.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn event_stream_reports_reconnects_and_backs_off_after_eof() {
        let (_root, client, listener) = socket_fixture(EVENT_SOCKET).await;
        let (sender, mut events) = mpsc::channel(8);
        let task = tokio::spawn(watch_events_with(client, sender, |event| event));
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(events.recv().await.unwrap(), Event::Connected);
        stream
            .write_all(b"openwindow>>123,1,app,title\n")
            .await
            .unwrap();
        assert_eq!(
            events.recv().await.unwrap(),
            Event::Message("openwindow>>123,1,app,title".into())
        );
        drop(stream);
        assert_eq!(events.recv().await.unwrap(), Event::Disconnected);
        assert!(
            time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        let (_stream, _) = time::timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(events.recv().await.unwrap(), Event::Connected);
        drop(events);
        time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn rejects_signatures_that_can_escape_the_runtime_root() {
        assert!(valid_signature("instance_123.456"));
        for invalid in ["", ".", "..", "../other", "nested/instance"] {
            assert!(!valid_signature(invalid), "{invalid}");
        }
    }
}
