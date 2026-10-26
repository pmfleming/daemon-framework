use super::*;
use crate::{BasicCorrelation, ServiceBoundary};
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    sync::Mutex,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{Notify, oneshot},
};

struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "jsonl-socket-{}-{}",
            std::process::id(),
            NEXT_OWNER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Service {
    boundary: ServiceBoundary,
    resources: Mutex<HashMap<String, String>>,
    entered: Notify,
    release: Notify,
    completed: Notify,
    disconnects: AtomicU64,
}
impl Service {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            boundary: ServiceBoundary::with_limits(
                shelllist_daemon_core::ApiIdentity::new("test", 1),
                1,
                2,
                1024 * 1024,
                16 * 1024 * 1024,
            ),
            resources: Mutex::default(),
            entered: Notify::new(),
            release: Notify::new(),
            completed: Notify::new(),
            disconnects: AtomicU64::new(0),
        })
    }
}
fn decode(value: String) -> Result<Value> {
    Ok(serde_json::from_str(&value)?)
}
impl LocalJsonlService for Service {
    async fn call(&self, session: &SocketSession, method: &str, params: Value) -> Result<Value> {
        let admitted = match self
            .boundary
            .call(Some(session.owner()), method, &params.to_string())
        {
            Ok(p) => p,
            Err(e) => return decode(e),
        };
        if method == "hold" {
            self.entered.notify_one();
            self.release.notified().await;
            self.resources
                .lock()
                .unwrap()
                .insert("late".into(), session.owner().into());
            self.completed.notify_one();
        }
        if method == "burst-sync" {
            self.resources
                .lock()
                .unwrap()
                .insert("sync".into(), session.owner().into());
            let emitter = crate::EventEmitter::from(session.clone());
            for revision in 1..=64 {
                if emitter
                    .emit_sync("org.example.Test", "changed", json!({"revision":revision}))
                    .is_err()
                {
                    break;
                }
            }
            assert!(
                emitter.is_closed(),
                "bounded synchronous overflow must retire its owner"
            );
            self.completed.notify_one();
        }
        let value = if method == "large" {
            json!({"data": "🦀".repeat(2*1024*1024)})
        } else if method == "owned-status" {
            let owned = params["id"].as_str().is_some_and(|id| {
                self.resources
                    .lock()
                    .unwrap()
                    .get(id)
                    .is_some_and(|owner| owner == session.owner())
            });
            json!({"owned": owned})
        } else {
            params
        };
        decode(admitted.encode(&value))
    }
    async fn subscribe(&self, session: &SocketSession, streams: Vec<String>) -> Result<Value> {
        let admitted = match self.boundary.subscribe(Some(session.owner()), &streams) {
            Ok(p) => p,
            Err(e) => return decode(e),
        };
        let id = format!("s-{}", NEXT_OWNER.fetch_add(1, Ordering::Relaxed));
        self.resources
            .lock()
            .unwrap()
            .insert(id.clone(), session.owner().into());
        if streams == ["burst-preack"] {
            for revision in 1..=2 {
                session
                    .emit(
                        "test.changed",
                        json!({"subscription_id": id, "revision":revision}),
                    )
                    .await?;
            }
            self.entered.notify_one();
            self.release.notified().await;
        }
        session
            .emit(
                "test.changed",
                json!({"subscription_id": id, "data": {"first": true}}),
            )
            .await?;
        decode(admitted.encode(&json!({"data":{"subscription":{"id":id}}})))
    }
    async fn cancel(&self, session: &SocketSession, id: &str) -> Result<Value> {
        let _admitted = self
            .boundary
            .cancel(Some(session.owner()), id)
            .map_err(anyhow::Error::msg)?;
        let mut resources = self.resources.lock().unwrap();
        let owned = resources
            .get(id)
            .is_some_and(|owner| owner == session.owner());
        if owned {
            resources.remove(id);
        }
        Ok(json!({"ok":owned}))
    }
    async fn disconnected(&self, session: &SocketSession) {
        self.resources
            .lock()
            .unwrap()
            .retain(|_, owner| owner != session.owner());
        self.disconnects.fetch_add(1, Ordering::SeqCst);
    }
}
struct Running {
    directory: Directory,
    path: std::path::PathBuf,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<()>>,
    service: Arc<Service>,
}
impl Running {
    async fn start(config: JsonlSocketConfig) -> Self {
        let directory = Directory::new();
        let listener = JsonlListener::bind(&directory.0, "test").await.unwrap();
        let path = listener.path().to_owned();
        let service = Service::new();
        let (stop, stopped) = oneshot::channel();
        let s = service.clone();
        let task = tokio::spawn(listener.serve(s, BasicCorrelation, config, async {
            let _ = stopped.await;
        }));
        Self {
            directory,
            path,
            stop,
            task,
            service,
        }
    }
    async fn finish(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!self.path.exists());
        drop(self.directory);
    }
}
async fn send(stream: &mut BufReader<UnixStream>, value: Value) {
    stream
        .get_mut()
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
}
async fn next(stream: &mut BufReader<UnixStream>) -> Value {
    next_with_bytes(stream).await.0
}
async fn next_with_bytes(stream: &mut BufReader<UnixStream>) -> (Value, usize) {
    let mut line = String::new();
    let bytes = tokio::time::timeout(Duration::from_secs(3), stream.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes > 0);
    (serde_json::from_str(&line).unwrap(), bytes)
}
async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
mod qualification;

#[test]
fn owner_identities_have_fresh_entropy_not_just_a_pid_and_counter() {
    let a = new_owner().unwrap();
    let b = new_owner().unwrap();
    let nonce_a = a.split(':').nth(1).unwrap();
    let nonce_b = b.split(':').nth(1).unwrap();
    assert_eq!(nonce_a.len(), 32);
    assert!(u128::from_str_radix(nonce_a, 16).is_ok());
    assert_ne!(nonce_a, nonce_b);
    assert!(a.starts_with("unix:") && b.starts_with("unix:"));
}

#[tokio::test]
async fn pre_ack_overflow_closes_connection_instead_of_losing_a_baseline() {
    let running = Running::start(JsonlSocketConfig {
        pending_event_limit: 1,
        ..Default::default()
    })
    .await;
    let mut client = BufReader::new(UnixStream::connect(&running.path).await.unwrap());
    send(
        &mut client,
        json!({"id":"sub","op":"subscribe","streams":["burst-preack"]}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), running.service.entered.notified())
        .await
        .unwrap();
    until(|| running.service.disconnects.load(Ordering::SeqCst) > 0).await;
    assert_eq!(next(&mut client).await["kind"], "transport-error");
    let mut line = String::new();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), client.read_line(&mut line))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert!(running.service.resources.lock().unwrap().is_empty());
    running.service.release.notify_one();
    running.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn synchronous_event_overflow_retires_only_its_owner_without_blocking_the_executor() {
    let running = Running::start(JsonlSocketConfig::default()).await;
    let mut slow = BufReader::new(UnixStream::connect(&running.path).await.unwrap());
    let mut peer = BufReader::new(UnixStream::connect(&running.path).await.unwrap());
    send(
        &mut slow,
        json!({"id":"burst","op":"call","method":"burst-sync","params":{}}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), running.service.completed.notified())
        .await
        .unwrap();
    until(|| running.service.disconnects.load(Ordering::SeqCst) > 0).await;
    assert!(running.service.resources.lock().unwrap().is_empty());
    // A real EOF, not merely an internal closed flag or a synthetic transport.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let mut line = String::new();
            if slow.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
        }
    })
    .await
    .unwrap();
    send(
        &mut peer,
        json!({"id":"peer","op":"call","method":"echo","params":{"live":true}}),
    )
    .await;
    assert_eq!(next(&mut peer).await["response"]["live"], true);
    drop(peer);
    running.finish().await;
}

#[tokio::test]
async fn filesystem_lock_stale_socket_modes_and_inode_cleanup() {
    let directory = Directory::new();
    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(JsonlListener::bind(&directory.0, "test").await.is_err());
    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(JsonlListener::bind(&directory.0, "../bad").await.is_err());
    fs::write(directory.0.join("test.sock"), "keep").unwrap();
    assert!(JsonlListener::bind(&directory.0, "test").await.is_err());
    fs::remove_file(directory.0.join("test.sock")).unwrap();
    symlink("absent", directory.0.join("test.sock")).unwrap();
    assert!(JsonlListener::bind(&directory.0, "test").await.is_err());
    fs::remove_file(directory.0.join("test.sock")).unwrap();
    let stale = std::os::unix::net::UnixListener::bind(directory.0.join("test.sock")).unwrap();
    drop(stale);
    let listener = JsonlListener::bind(&directory.0, "test").await.unwrap();
    assert_eq!(
        fs::metadata(listener.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(JsonlListener::bind(&directory.0, "test").await.is_err());
    fs::remove_file(listener.path()).unwrap();
    fs::write(listener.path(), "replacement").unwrap();
    drop(listener);
    assert_eq!(
        fs::read_to_string(directory.0.join("test.sock")).unwrap(),
        "replacement"
    );
}
#[tokio::test]
async fn routes_early_events_release_and_connection_ownership() {
    let server = Running::start(JsonlSocketConfig::default()).await;
    let mut a = BufReader::new(UnixStream::connect(&server.path).await.unwrap());
    let mut b = BufReader::new(UnixStream::connect(&server.path).await.unwrap());
    let route = json!({"consumerId":"view","localId":"sub","generation":2,"kind":"subscription"});
    send(
        &mut a,
        json!({"op":"subscribe","id":"sub","streams":["test.changed"],"route":route}),
    )
    .await;
    let ack = next(&mut a).await;
    assert_eq!(ack["route"], route);
    let id = ack["response"]["data"]["subscription"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let event = next(&mut a).await;
    assert_eq!(event["route"], route);
    send(
        &mut b,
        json!({"op":"cancel","id":"foreign","request_id":id}),
    )
    .await;
    assert_eq!(next(&mut b).await["response"]["ok"], false);
    send(&mut a,json!({"op":"release","id":"release","route":{"consumerId":"view","localId":"release","generation":2,"kind":"control"}})).await;
    assert_eq!(next(&mut a).await["response"]["released"], 1);
    assert!(server.service.resources.lock().unwrap().is_empty());
    send(
        &mut a,
        json!({"op":"subscribe","id":"again","streams":["test.changed"]}),
    )
    .await;
    next(&mut a).await;
    next(&mut a).await;
    drop(a);
    until(|| server.service.resources.lock().unwrap().is_empty()).await;
    drop(b);
    server.finish().await;
}
#[tokio::test]
async fn deadlines_do_not_abort_effects_or_free_shared_admission_and_late_work_is_cleaned() {
    let server = Running::start(JsonlSocketConfig {
        request_timeout: Duration::from_millis(30),
        ..Default::default()
    })
    .await;
    let mut a = BufReader::new(UnixStream::connect(&server.path).await.unwrap());
    send(&mut a, json!({"op":"call","id":"effect","method":"hold"})).await;
    server.service.entered.notified().await;
    assert_eq!(next(&mut a).await["outcome"], "outcome-unknown");
    assert!(
        server
            .service
            .boundary
            .call(Some(":dbus-owner"), "another", "{}")
            .is_err()
    );
    send(
        &mut a,
        json!({"op":"cancel","id":"control","request_id":"none"}),
    )
    .await;
    assert_eq!(next(&mut a).await["id"], "control");
    drop(a);
    until(|| server.service.disconnects.load(Ordering::SeqCst) > 0).await;
    server.service.release.notify_one();
    server.service.completed.notified().await;
    until(|| {
        server.service.disconnects.load(Ordering::SeqCst) > 1
            && server.service.resources.lock().unwrap().is_empty()
    })
    .await;
    assert!(
        server
            .service
            .boundary
            .call(Some(":dbus-owner"), "another", "{}")
            .is_ok()
    );
    server.finish().await;
}
#[tokio::test]
async fn fragmented_utf8_oversized_frame_resynchronization_and_long_response() {
    let server = Running::start(JsonlSocketConfig {
        maximum_request_bytes: 256,
        ..Default::default()
    })
    .await;
    let mut a = BufReader::new(UnixStream::connect(&server.path).await.unwrap());
    let bytes = format!(
        "{}\n",
        json!({"op":"call","id":"unicode","method":"echo","params":{"text":"你好 🦀"}})
    )
    .into_bytes();
    for byte in bytes {
        a.get_mut().write_all(&[byte]).await.unwrap();
    }
    assert_eq!(next(&mut a).await["response"]["text"], "你好 🦀");
    a.get_mut()
        .write_all(format!("{}\n", "x".repeat(257)).as_bytes())
        .await
        .unwrap();
    assert_eq!(next(&mut a).await["kind"], "protocol-error");
    send(&mut a, json!({"op":"call","id":"large","method":"large"})).await;
    assert_eq!(
        next(&mut a).await["response"]["data"]
            .as_str()
            .unwrap()
            .len(),
        8 * 1024 * 1024
    );
    send(&mut a, json!({"op":"shutdown","id":"done"})).await;
    assert_eq!(next(&mut a).await["response"]["shutdown"], true);
    drop(a);
    server.finish().await;
}
#[tokio::test]
async fn stalled_reader_and_response_limits_close_owner_without_blocking_others() {
    let server = Running::start(JsonlSocketConfig {
        write_timeout: Duration::from_millis(50),
        ..Default::default()
    })
    .await;
    let mut a = BufReader::new(UnixStream::connect(&server.path).await.unwrap());
    send(&mut a, json!({"op":"call","id":"large","method":"large"})).await;
    until(|| server.service.disconnects.load(Ordering::SeqCst) > 0).await;
    let mut b = BufReader::new(UnixStream::connect(&server.path).await.unwrap());
    send(&mut b, json!({"op":"call","id":"ok","method":"echo"})).await;
    assert_eq!(next(&mut b).await["id"], "ok");
    drop(a);
    drop(b);
    server.finish().await;
    let (writer, _reader) = tokio::io::duplex(1);
    let mut writer = writer::DeadlineWriter::new(writer, 3, Duration::from_millis(10));
    assert!(writer.write_all(b"oversize").await.is_err());
}
