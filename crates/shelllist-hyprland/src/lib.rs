pub mod monitor;
pub mod preferences;
pub mod work_area;
#[cfg(test)]
mod work_area_tests;

use std::{
    env,
    ffi::OsString,
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
    signature: Option<OsString>,
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
        Self {
            runtime_dir,
            signature: env::var_os("HYPRLAND_INSTANCE_SIGNATURE"),
        }
    }

    /// Invalid signatures are retained and reported by IPC operations, not
    /// silently treated as absent. Use [`Self::instance_signature`] to validate early.
    #[must_use]
    pub fn new(runtime_dir: PathBuf, signature: Option<String>) -> Self {
        Self {
            runtime_dir,
            signature: signature.map(OsString::from),
        }
    }

    /// The configured signature, distinguishing absent from invalid configuration.
    pub fn instance_signature(&self) -> Result<Option<&str>> {
        self.signature
            .as_deref()
            .map(|value| {
                let value = value
                    .to_str()
                    .context("HYPRLAND_INSTANCE_SIGNATURE is not valid Unicode")?;
                anyhow::ensure!(
                    valid_signature(value),
                    "invalid HYPRLAND_INSTANCE_SIGNATURE"
                );
                Ok(value)
            })
            .transpose()
    }

    /// Request with discovery fallback. Prefer [`Self::request_exact`] for mutations.
    pub async fn request(&self, command: &str) -> Result<String> {
        bounded_ipc(async {
            let stream = self.connect(COMMAND_SOCKET).await?;
            // Fail over only while connecting, never replay a command that may
            // already have changed compositor state.
            request_socket(stream, command).await
        })
        .await
    }

    /// Send only to the configured instance, never to a different signature.
    ///
    /// Requires a valid signature. Both runtime and legacy locations are tried,
    /// but commands are never replayed after a connection succeeds.
    pub async fn request_exact(&self, command: &str) -> Result<String> {
        bounded_ipc(async {
            let signature = self
                .instance_signature()?
                .context("exact Hyprland request requires an instance signature")?;
            let candidates =
                exact_socket_candidates(&self.roots(), signature, COMMAND_SOCKET).await;
            let stream = connect_candidates(&candidates).await?;
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
        let candidates = socket_candidates(&self.roots(), self.instance_signature()?, socket).await;
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

async fn exact_socket_candidates(roots: &[PathBuf], signature: &str, socket: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let uid = rustix::process::geteuid().as_raw();
    for root in roots {
        add_socket(&mut candidates, root.join(signature).join(socket), uid).await;
    }
    candidates
}

// Prefer the requested instance in either root, then sorted fallback instances
// in runtime-root / legacy-root order. Socket existence alone is not liveness.
async fn socket_candidates(
    roots: &[PathBuf],
    signature: Option<&str>,
    socket: &str,
) -> Vec<PathBuf> {
    let mut candidates = match signature {
        Some(signature) => exact_socket_candidates(roots, signature, socket).await,
        None => Vec::new(),
    };
    let uid = rustix::process::geteuid().as_raw();
    for root in roots {
        let Ok(mut entries) = fs::read_dir(root).await else {
            continue;
        };
        let mut paths = Vec::new();
        // A root can disappear or become unreadable mid-scan. Keep preferred
        // and already collected candidates, and continue with the other roots.
        while let Ok(Some(entry)) = entries.next_entry().await {
            paths.push(entry.path().join(socket));
        }
        paths.sort();
        for path in paths {
            add_socket(&mut candidates, path, uid).await;
        }
    }
    candidates
}

async fn add_socket(candidates: &mut Vec<PathBuf>, path: PathBuf, uid: u32) {
    let Some(parent) = path.parent() else {
        return;
    };
    // Reject symlink sockets/directories and instance directories that another
    // user can modify. Peer credentials below also protect the check/connect gap.
    if !candidates.contains(&path)
        && fs::symlink_metadata(parent)
            .await
            .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o022 == 0)
        && fs::symlink_metadata(&path)
            .await
            .is_ok_and(|m| m.file_type().is_socket() && m.uid() == uid)
    {
        candidates.push(path);
    }
}

fn validate_peer(stream: UnixStream, uid: u32) -> Result<UnixStream> {
    anyhow::ensure!(
        stream
            .peer_cred()
            .context("read Hyprland peer credentials")?
            .uid()
            == uid,
        "Hyprland socket peer belongs to a different user"
    );
    Ok(stream)
}

async fn connect_candidates(candidates: &[PathBuf]) -> Result<UnixStream> {
    let mut last_error = None;
    for socket in candidates {
        let connection = UnixStream::connect(socket)
            .await
            .map_err(anyhow::Error::new)
            .and_then(|stream| validate_peer(stream, rustix::process::geteuid().as_raw()));
        match connection {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                last_error =
                    Some(error.context(format!("connect to Hyprland socket {}", socket.display())))
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
    /// An established connection ended: None for EOF, Some for a read/protocol error.
    Disconnected(Option<String>),
    /// Connection failed, including at startup. Identical consecutive failures
    /// are suppressed until the cause changes or a connection succeeds.
    ConnectFailed(String),
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
    // Receiver closure cancels connection attempts, reads, sends and backoff.
    tokio::select! {
        _ = sender.closed() => {},
        _ = reconnect_events(client, &sender, &map) => {},
    }
}

async fn reconnect_events<T>(
    client: Client,
    sender: &mpsc::Sender<T>,
    map: &impl Fn(Event) -> T,
) -> Result<(), mpsc::error::SendError<T>> {
    let mut last_connect_error = None;
    loop {
        match client.event_socket().await {
            Ok(stream) => {
                last_connect_error = None;
                forward_events(stream, sender, map).await?;
            }
            Err(error) => {
                let cause = format!("{error:#}");
                if last_connect_error.as_ref() != Some(&cause) {
                    sender
                        .send(map(Event::ConnectFailed(cause.clone())))
                        .await?;
                    last_connect_error = Some(cause);
                }
            }
        }
        // Back off even when a socket accepts and immediately closes.
        time::sleep(RECONNECT_DELAY).await;
    }
}

async fn forward_events<T>(
    stream: UnixStream,
    sender: &mpsc::Sender<T>,
    map: &impl Fn(Event) -> T,
) -> Result<(), mpsc::error::SendError<T>> {
    sender.send(map(Event::Connected)).await?;
    let mut reader = BufReader::new(stream);
    let cause = loop {
        match read_event(&mut reader).await {
            Ok(Some(line)) => sender.send(map(Event::Message(line))).await?,
            Ok(None) => break None,
            Err(error) => break Some(format!("{error:#}")),
        }
    };
    sender.send(map(Event::Disconnected(cause))).await
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
    let mut response = Vec::new();
    stream
        .take(MAX_REPLY_BYTES + 1)
        .read_to_end(&mut response)
        .await?;
    anyhow::ensure!(
        response.len() as u64 <= MAX_REPLY_BYTES,
        "Hyprland reply is too large"
    );
    String::from_utf8(response).context("Hyprland reply is not valid UTF-8")
}

fn default_runtime_dir() -> PathBuf {
    let uid = rustix::process::geteuid().as_raw();
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
    use super::{COMMAND_SOCKET, Client, EVENT_SOCKET, Event, watch_events_with};
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
    async fn invalid_and_exact_targets_never_use_another_instance() {
        use std::os::unix::ffi::OsStringExt;
        let (root, _client, fallback) = socket_fixture(COMMAND_SOCKET).await;
        let signature = root
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let missing = format!("{signature}-missing");
        let instance = root.path().join("hypr").join(&signature);
        fs::create_dir_all(&instance).await.unwrap();
        drop(tokio::net::UnixListener::bind(instance.join(COMMAND_SOCKET)).unwrap());
        for signature in [
            None,
            Some(missing),
            Some(signature),
            Some("../invalid".into()),
        ] {
            let client = Client::new(root.path().into(), signature);
            assert!(client.request_exact("dispatch test").await.is_err());
        }
        let mut invalid = vec![std::ffi::OsString::from_vec(vec![0xff])];
        invalid.extend(["", ".", "..", "../other", "nested/instance"].map(Into::into));
        for signature in invalid {
            let client = Client {
                runtime_dir: root.path().into(),
                signature: Some(signature),
            };
            for error in [
                client.request("j/clients").await.unwrap_err(),
                client.request_exact("dispatch test").await.unwrap_err(),
                client.event_socket().await.unwrap_err(),
            ] {
                assert!(error.to_string().contains("HYPRLAND_INSTANCE_SIGNATURE"));
            }
        }
        assert!(
            time::timeout(Duration::from_millis(50), fallback.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn requests_bound_reply_size_encoding_and_wait_time() {
        for (prefix, oversized) in [
            (super::MAX_REPLY_BYTES as usize - 2, false),
            (super::MAX_REPLY_BYTES as usize, true),
        ] {
            let reply = format!("{}é", "x".repeat(prefix));
            let (_root, client, server) = command_server("j/clients", reply).await;
            let result = client.request_exact("j/clients").await;
            if oversized {
                // take(MAX + 1) retains only the first byte of é.
                assert!(result.unwrap_err().to_string().contains("too large"));
            } else {
                let reply = result.unwrap();
                assert_eq!(reply.len(), super::MAX_REPLY_BYTES as usize);
                assert!(reply.ends_with('é'));
            }
            server.await.unwrap();
        }
        let (_root, client, server) = command_server("j/clients", [0xff]).await;
        assert!(
            client
                .request_exact("j/clients")
                .await
                .unwrap_err()
                .to_string()
                .contains("UTF-8")
        );
        server.await.unwrap();
        let (_root, client, listener) = socket_fixture(COMMAND_SOCKET).await;
        let request = tokio::spawn(async move { client.request("j/clients").await });
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.read_to_end(&mut Vec::new()).await.unwrap();
        time::pause(); // The connected peer deliberately never replies.
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
    async fn event_limits_and_receiver_closure_cancel_reads_and_backoff() {
        for (bytes, failure) in [
            (b"partial".to_vec(), None),
            (
                vec![b'x'; super::MAX_EVENT_BYTES as usize + 1],
                Some("too large"),
            ),
            (b"\xff\n".to_vec(), Some("utf-8")),
        ] {
            let (_root, client, listener) = socket_fixture(EVENT_SOCKET).await;
            let (sender, mut events) = mpsc::channel(8);
            let watcher = tokio::spawn(watch_events_with(client, sender, |event| event));
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(events.recv().await, Some(Event::Connected));
            stream.write_all(&bytes).await.unwrap();
            if let Some(expected) = failure {
                let Some(Event::Disconnected(Some(cause))) =
                    time::timeout(Duration::from_secs(1), events.recv())
                        .await
                        .unwrap()
                else {
                    panic!("expected a protocol failure")
                };
                assert!(cause.contains(expected), "{cause}");
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
    async fn event_stream_reports_reconnects_failures_and_cancels_backoff() {
        let (_root, client, listener) = socket_fixture(EVENT_SOCKET).await;
        let (sender, mut events) = mpsc::channel(8);
        let task = tokio::spawn(watch_events_with(client, sender, |event| event));
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(events.recv().await.unwrap(), Event::Connected);
        let boundary = "x".repeat(super::MAX_EVENT_BYTES as usize - 2);
        for line in ["openwindow>>123,1,app,title", "", &boundary] {
            stream
                .write_all(format!("{line}\r\n").as_bytes())
                .await
                .unwrap();
            assert_eq!(events.recv().await.unwrap(), Event::Message(line.into()));
        }
        stream.write_all(b"partial").await.unwrap();
        drop(stream);
        assert_eq!(
            events.recv().await.unwrap(),
            Event::Message("partial".into())
        );
        assert_eq!(events.recv().await.unwrap(), Event::Disconnected(None));
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
        time::pause();
        let client = Client::new("/unused".into(), Some("../invalid".into()));
        let (sender, mut events) = mpsc::channel(8);
        let task = tokio::spawn(watch_events_with(client, sender, |event| event));
        let Some(Event::ConnectFailed(_)) = events.recv().await else {
            panic!("expected initial connection failure");
        };
        assert!(
            time::timeout(Duration::from_secs(3), events.recv())
                .await
                .is_err()
        );
        drop(events);
        time::timeout(Duration::from_millis(100), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn discovery_requires_owned_protected_instance_and_socket() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let (root, _client, _listener) = socket_fixture(COMMAND_SOCKET).await;
        let instance = root.path().join("hypr/test");
        let socket = instance.join(COMMAND_SOCKET);
        let uid = rustix::process::geteuid().as_raw();
        let mut candidates = Vec::new();
        super::add_socket(&mut candidates, socket.clone(), uid.wrapping_add(1)).await;
        assert!(candidates.is_empty());
        fs::set_permissions(&instance, std::fs::Permissions::from_mode(0o777))
            .await
            .unwrap();
        super::add_socket(&mut candidates, socket.clone(), uid).await;
        assert!(candidates.is_empty());
        fs::set_permissions(&instance, std::fs::Permissions::from_mode(0o700))
            .await
            .unwrap();
        let alias = instance.join("alias.sock");
        symlink(&socket, &alias).unwrap();
        super::add_socket(&mut candidates, alias, uid).await;
        let directory_alias = root.path().join("alias");
        symlink(&instance, &directory_alias).unwrap();
        super::add_socket(&mut candidates, directory_alias.join(COMMAND_SOCKET), uid).await;
        assert!(candidates.is_empty());
        super::add_socket(&mut candidates, socket.clone(), uid).await;
        assert_eq!(candidates, [socket]);
        let (stream, _peer) = tokio::net::UnixStream::pair().unwrap();
        assert!(super::validate_peer(stream, uid).is_ok());
        let (stream, _peer) = tokio::net::UnixStream::pair().unwrap();
        assert!(super::validate_peer(stream, uid.wrapping_add(1)).is_err());
    }
}
