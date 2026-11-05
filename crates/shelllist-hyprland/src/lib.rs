mod address;
mod error;
mod events;
pub use address::WindowAddress;
pub use error::{IpcError, ack};
pub use events::CompositorEvent;
pub mod monitor;
pub mod preferences;
pub mod work_area;
#[cfg(test)]
mod work_area_tests;
pub mod workspace;

use std::{
    env,
    ffi::OsString,
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

    /// A configured signature pins reads and commands to that instance.
    /// Discovery is allowed only when no signature was configured.
    pub async fn request(&self, command: &str) -> Result<String> {
        self.request_with(command, false).await
    }

    /// Requires a configured instance; commands are never replayed after connect.
    pub async fn request_exact(&self, command: &str) -> Result<String> {
        self.request_with(command, true).await
    }

    async fn request_with(&self, command: &str, exact: bool) -> Result<String> {
        let deadline = time::Instant::now() + IPC_TIMEOUT;
        let (stream, _) = self.connect_before(COMMAND_SOCKET, exact, deadline).await?;
        time::timeout_at(deadline, request_socket(stream, command))
            .await
            .map_err(|_| {
                anyhow::Error::new(IpcError::OutcomeUnknown).context("Hyprland exchange timed out")
            })?
    }

    /// Watch this client's instance, including connection identity and event loss.
    pub async fn watch_events(self, sender: mpsc::Sender<Event>) {
        tokio::select! {
            _ = sender.closed() => {},
            _ = reconnect_events(self, &sender) => {},
        }
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
        Ok(self
            .connect_before(EVENT_SOCKET, false, time::Instant::now() + IPC_TIMEOUT)
            .await?
            .0)
    }

    async fn connect_before(
        &self,
        socket: &str,
        exact: bool,
        deadline: time::Instant,
    ) -> Result<(UnixStream, String)> {
        time::timeout_at(deadline, self.connect(socket, exact))
            .await
            .map_err(|_| {
                anyhow::Error::new(IpcError::NotDelivered).context("Hyprland connect timed out")
            })?
            .map_err(|error| {
                if error.downcast_ref::<IpcError>().is_some() {
                    error
                } else {
                    anyhow::Error::new(IpcError::NotDelivered).context(format!("{error:#}"))
                }
            })
    }

    async fn connect(&self, socket: &str, exact: bool) -> Result<(UnixStream, String)> {
        let signature = self.instance_signature()?;
        anyhow::ensure!(
            !exact || signature.is_some(),
            "exact Hyprland request requires an instance signature"
        );
        let candidates = socket_candidates(&self.roots(), signature, socket).await;
        match connect_candidates(&candidates).await {
            Ok(connection) => Ok(connection),
            Err(error) => {
                // Diagnostic connections send no request and never rebind this client.
                if let Some(configured) = signature {
                    // A broken event socket alone does not prove the instance
                    // vanished while its command socket still accepts peers.
                    let counterpart = if socket == EVENT_SOCKET {
                        COMMAND_SOCKET
                    } else {
                        EVENT_SOCKET
                    };
                    let exact =
                        exact_socket_candidates(&self.roots(), configured, counterpart).await;
                    if connect_candidates(&exact).await.is_ok() {
                        return Err(error);
                    }
                    let others = socket_candidates(&self.roots(), None, socket)
                        .await
                        .into_iter()
                        .filter(|p| {
                            p.parent()
                                .and_then(|p| p.file_name())
                                .is_some_and(|v| v != configured)
                        })
                        .collect::<Vec<_>>();
                    if let Ok((_, live)) = connect_candidates(&others).await {
                        return Err(IpcError::SessionGone {
                            configured: configured.into(),
                            live,
                        }
                        .into());
                    }
                }
                Err(error)
            }
        }
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

async fn exact_socket_candidates(roots: &[PathBuf], signature: &str, socket: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let uid = rustix::process::geteuid().as_raw();
    for root in roots {
        add_socket(&mut candidates, root.join(signature).join(socket), uid).await;
    }
    candidates
}

// A configured instance is exact across both roots. Without one, discover in
// sorted runtime-root / legacy-root order. Existence alone is not liveness.
async fn socket_candidates(
    roots: &[PathBuf],
    signature: Option<&str>,
    socket: &str,
) -> Vec<PathBuf> {
    if let Some(signature) = signature {
        return exact_socket_candidates(roots, signature, socket).await;
    }
    let mut candidates = Vec::new();
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

async fn connect_candidates(candidates: &[PathBuf]) -> Result<(UnixStream, String)> {
    let mut last_error = None;
    for socket in candidates {
        let connection = UnixStream::connect(socket)
            .await
            .map_err(anyhow::Error::new)
            .and_then(|stream| validate_peer(stream, rustix::process::geteuid().as_raw()));
        match connection {
            Ok(stream) => {
                let instance = socket
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|v| v.to_str())
                    .context("invalid instance socket path")?;
                return Ok((stream, instance.to_owned()));
            }
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
    Connected { instance: String },
    Disconnected(Option<String>),
    ConnectFailed(String),
    SessionGone { configured: String, live: String },
    Lagged,
    Compositor(CompositorEvent),
}

async fn reconnect_events(
    client: Client,
    sender: &mpsc::Sender<Event>,
) -> Result<(), mpsc::error::SendError<Event>> {
    let mut last_connect_error = None;
    loop {
        match client
            .connect_before(EVENT_SOCKET, false, time::Instant::now() + IPC_TIMEOUT)
            .await
        {
            Ok((stream, instance)) => {
                last_connect_error = None;
                forward_events(stream, instance, sender).await?;
            }
            Err(error) => {
                if let Some(IpcError::SessionGone { configured, live }) =
                    error.downcast_ref::<IpcError>()
                {
                    sender
                        .send(Event::SessionGone {
                            configured: configured.clone(),
                            live: live.clone(),
                        })
                        .await?;
                    return Ok(());
                }
                let cause = format!("{error:#}");
                if last_connect_error.as_ref() != Some(&cause) {
                    sender.send(Event::ConnectFailed(cause.clone())).await?;
                    last_connect_error = Some(cause);
                }
            }
        }
        time::sleep(RECONNECT_DELAY).await;
    }
}

async fn forward_events(
    stream: UnixStream,
    instance: String,
    sender: &mpsc::Sender<Event>,
) -> Result<(), mpsc::error::SendError<Event>> {
    sender.send(Event::Connected { instance }).await?;
    let mut reader = BufReader::new(stream);
    let mut lagged = false;
    let cause = loop {
        // Keep the same read future alive while delivering pending loss. Dropping
        // read_until midway could otherwise discard an unterminated frame.
        let read = read_event(&mut reader);
        tokio::pin!(read);
        let result = if lagged {
            tokio::select! {
                result = &mut read => result,
                permit = sender.reserve() => {
                    permit.map_err(|_| mpsc::error::SendError(Event::Lagged))?.send(Event::Lagged);
                    lagged = false;
                    read.await
                }
            }
        } else {
            read.await
        };
        match result {
            Ok(Some(line)) => {
                if lagged {
                    match sender.try_send(Event::Lagged) {
                        Ok(()) => lagged = false,
                        Err(mpsc::error::TrySendError::Full(_)) => continue,
                        Err(mpsc::error::TrySendError::Closed(event)) => {
                            return Err(mpsc::error::SendError(event));
                        }
                    }
                }
                match sender.try_send(Event::Compositor(CompositorEvent::parse(&line))) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => lagged = true,
                    Err(mpsc::error::TrySendError::Closed(event)) => {
                        return Err(mpsc::error::SendError(event));
                    }
                }
            }
            Ok(None) => break None,
            Err(error) => break Some(format!("{error:#}")),
        }
    };
    if lagged {
        sender.send(Event::Lagged).await?;
    }
    sender.send(Event::Disconnected(cause)).await
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
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

async fn request_socket(mut stream: UnixStream, command: &str) -> Result<String> {
    stream
        .write_all(command.as_bytes())
        .await
        .map_err(|error| anyhow::Error::new(IpcError::OutcomeUnknown).context(error))?;
    stream
        .shutdown()
        .await
        .map_err(|error| anyhow::Error::new(IpcError::OutcomeUnknown).context(error))?;
    let mut response = Vec::new();
    stream
        .take(MAX_REPLY_BYTES + 1)
        .read_to_end(&mut response)
        .await
        .map_err(|error| anyhow::Error::new(IpcError::OutcomeUnknown).context(error))?;
    if response.len() as u64 > MAX_REPLY_BYTES {
        return Err(anyhow::Error::new(IpcError::InvalidReply)
            .context("Hyprland reply is too large; command outcome unknown"));
    }
    Ok(String::from_utf8_lossy(&response).into_owned())
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
    use super::{COMMAND_SOCKET, Client, EVENT_SOCKET, Event};
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
    async fn pinned_requests_and_watchers_diagnose_without_rebinding() {
        let (root, _, command) = socket_fixture(COMMAND_SOCKET).await;
        let event =
            tokio::net::UnixListener::bind(root.path().join("hypr/test").join(EVENT_SOCKET))
                .unwrap();
        let client = Client::new(root.path().into(), Some("missing".into()));
        let error = client.request("j/clients").await.unwrap_err();
        assert_eq!(
            error.downcast_ref::<super::IpcError>(),
            Some(&super::IpcError::SessionGone {
                configured: "missing".into(),
                live: "test".into()
            })
        );
        let (mut diagnostic, _) = command.accept().await.unwrap();
        let mut bytes = Vec::new();
        diagnostic.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
        let (sender, mut events) = mpsc::channel(1);
        let watcher = tokio::spawn(client.watch_events(sender));
        assert_eq!(
            events.recv().await,
            Some(Event::SessionGone {
                configured: "missing".into(),
                live: "test".into()
            })
        );
        watcher.await.unwrap();
        assert_eq!(events.recv().await, None);
        let (mut diagnostic, _) = event.accept().await.unwrap();
        diagnostic.read_to_end(&mut Vec::new()).await.unwrap();

        // Discovery is still supported explicitly, and Connected identifies it.
        let client = Client::new(root.path().into(), None);
        let (sender, mut events) = mpsc::channel(1);
        let watcher = tokio::spawn(client.watch_events(sender));
        let (_stream, _) = event.accept().await.unwrap();
        assert_eq!(
            events.recv().await,
            Some(Event::Connected {
                instance: "test".into()
            })
        );
        drop(events);
        watcher.await.unwrap();
    }

    #[tokio::test]
    async fn one_broken_socket_does_not_declare_a_live_session_gone() {
        let (root, client, _command) = socket_fixture(COMMAND_SOCKET).await;
        let other = root.path().join("hypr/other");
        fs::create_dir_all(&other).await.unwrap();
        let _other = tokio::net::UnixListener::bind(other.join(EVENT_SOCKET)).unwrap();
        let error = client.event_socket().await.unwrap_err();
        assert_eq!(
            error.downcast_ref::<super::IpcError>(),
            Some(&super::IpcError::NotDelivered)
        );
    }

    #[tokio::test]
    async fn full_receivers_do_not_stall_socket_intake_and_loss_survives_idle() {
        let (_root, client, listener) = socket_fixture(EVENT_SOCKET).await;
        let (sender, mut events) = mpsc::channel(1);
        let watcher = tokio::spawn(client.watch_events(sender));
        let (mut stream, _) = listener.accept().await.unwrap();
        // Connected occupies the only slot. This exceeds a Unix socket buffer;
        // completing the write proves compositor reads continued under pressure.
        let burst = b"workspacev2>>6,6\n".repeat(200_000);
        time::timeout(Duration::from_secs(5), stream.write_all(&burst))
            .await
            .unwrap()
            .unwrap();
        stream.write_all(b"windowtit").await.unwrap();
        // Let intake reach the unterminated frame before making room for Lagged.
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            events.recv().await,
            Some(Event::Connected {
                instance: "test".into()
            })
        );
        assert_eq!(
            time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap(),
            Some(Event::Lagged)
        );
        stream
            .write_all(b"lev2>>123,title,with,commas\n")
            .await
            .unwrap();
        loop {
            match time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
            {
                Some(Event::Lagged)
                | Some(Event::Compositor(super::CompositorEvent::WorkspaceChanged)) => {}
                Some(Event::Compositor(super::CompositorEvent::WindowTitleChanged)) => break,
                event => panic!("frame was lost during pending loss delivery: {event:?}"),
            }
        }
        drop(stream);
        assert_eq!(events.recv().await, Some(Event::Disconnected(None)));
        drop(events);
        watcher.await.unwrap();
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
                assert_eq!(
                    error.downcast_ref::<super::IpcError>(),
                    Some(&super::IpcError::NotDelivered)
                );
            }
        }
        while let Ok(Ok((mut stream, _))) =
            time::timeout(Duration::from_millis(50), fallback.accept()).await
        {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            assert!(bytes.is_empty(), "diagnostic connection sent a command");
        }
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
                let error = result.unwrap_err();
                assert!(error.to_string().contains("too large"));
                assert_eq!(
                    error.downcast_ref::<super::IpcError>(),
                    Some(&super::IpcError::InvalidReply)
                );
            } else {
                let reply = result.unwrap();
                assert_eq!(reply.len(), super::MAX_REPLY_BYTES as usize);
                assert!(reply.ends_with('é'));
            }
            server.await.unwrap();
        }
        let (_root, client, server) = command_server("j/clients", [0xff]).await;
        assert_eq!(client.request_exact("j/clients").await.unwrap(), "\u{fffd}");
        server.await.unwrap();
        let (_root, client, server) =
            command_server("j/clients", b"[{\"title\":\"bad \xff\"}]").await;
        let reply = client.request_exact("j/clients").await.unwrap();
        let windows: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(windows[0]["title"], "bad \u{fffd}");
        server.await.unwrap();
        let (_root, client, server) = command_server("j/clients", b"[\xff]").await;
        assert!(
            serde_json::from_str::<serde_json::Value>(
                &client.request_exact("j/clients").await.unwrap()
            )
            .is_err()
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
        ] {
            let (_root, client, listener) = socket_fixture(EVENT_SOCKET).await;
            let (sender, mut events) = mpsc::channel(8);
            let watcher = tokio::spawn(client.watch_events(sender));
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(
                events.recv().await,
                Some(Event::Connected {
                    instance: "test".into()
                })
            );
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
        let (root, client, server) = command_server(
            "dispatch test",
            vec![b'x'; super::MAX_REPLY_BYTES as usize + 1],
        )
        .await;
        let fallback = root.path().join("hypr/fallback");
        fs::create_dir_all(&fallback).await.unwrap();
        let fallback = tokio::net::UnixListener::bind(fallback.join(COMMAND_SOCKET)).unwrap();
        // An oversized reply after command execution must not trigger failover.
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
        let task = tokio::spawn(client.watch_events(sender));
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(
            events.recv().await.unwrap(),
            Event::Connected {
                instance: "test".into()
            }
        );
        stream
            .write_all(b"windowtitlev2>>123,bad \xff\nworkspace>>1\n")
            .await
            .unwrap();
        assert_eq!(
            events.recv().await.unwrap(),
            Event::Compositor(super::CompositorEvent::parse(
                "windowtitlev2>>123,bad \u{fffd}"
            ))
        );
        assert_eq!(
            events.recv().await.unwrap(),
            Event::Compositor(super::CompositorEvent::parse("workspace>>1"))
        );
        let boundary = "x".repeat(super::MAX_EVENT_BYTES as usize - 2);
        for line in ["openwindow>>123,1,app,title", "", &boundary] {
            stream
                .write_all(format!("{line}\r\n").as_bytes())
                .await
                .unwrap();
            assert_eq!(
                events.recv().await.unwrap(),
                Event::Compositor(super::CompositorEvent::parse(line))
            );
        }
        stream.write_all(b"partial").await.unwrap();
        drop(stream);
        assert_eq!(
            events.recv().await.unwrap(),
            Event::Compositor(super::CompositorEvent::parse("partial"))
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
        assert_eq!(
            events.recv().await.unwrap(),
            Event::Connected {
                instance: "test".into()
            }
        );
        drop(events);
        time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        time::pause();
        let client = Client::new("/unused".into(), Some("../invalid".into()));
        let (sender, mut events) = mpsc::channel(8);
        let task = tokio::spawn(client.watch_events(sender));
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
