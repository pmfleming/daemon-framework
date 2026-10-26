//! Correctness self-test only. Never interpret these timings as performance.
use super::*;
#[path = "../../../tests/support/qualification.rs"]
mod support;
use support::{Adapter, Step, Traffic, Workload};

struct Peer {
    socket: BufReader<UnixStream>,
    subscription: Option<String>,
    sequence: u64,
    consumer: String,
    traffic: Traffic,
}
impl Peer {
    async fn connect(path: &std::path::Path) -> Self {
        Self {
            socket: BufReader::new(UnixStream::connect(path).await.unwrap()),
            subscription: None,
            sequence: 0,
            consumer: format!(
                "qualification-{}",
                NEXT_OWNER.fetch_add(1, Ordering::Relaxed)
            ),
            traffic: Traffic::default(),
        }
    }
    fn route(&self, kind: &str) -> Value {
        json!({"consumerId":self.consumer, "localId":format!("q-{}", self.sequence),
            "generation":1, "kind":kind})
    }
    async fn request(&mut self, mut message: Value) -> Value {
        self.sequence += 1;
        let id = format!("q-{}", self.sequence);
        message["id"] = json!(id);
        let route = self.route(match message["op"].as_str().unwrap() {
            "call" => "call",
            "subscribe" => "subscription",
            "cancel" => "control",
            _ => panic!("unassigned qualification operation"),
        });
        message["route"] = route.clone();
        // Use the producer's actual schema, not a second protocol validator.
        serde_json::from_value::<shelllist_daemon_core::ClientMessage>(message.clone())
            .unwrap()
            .validate()
            .unwrap();
        self.traffic.requests += 1;
        self.traffic.request_bytes += message.to_string().len() as u64 + 1;
        send(&mut self.socket, message).await;
        let (response, bytes) = next_with_bytes(&mut self.socket).await;
        self.traffic.response_bytes += bytes as u64;
        assert_eq!(response["kind"], "response");
        assert_eq!(response["id"], id);
        assert_eq!(response["route"], route);
        assert_eq!(response["ok"], true);
        response["response"].clone()
    }
    async fn subscribe(&mut self) {
        assert!(self.subscription.is_none(), "no duplicate owned baseline");
        let response = self
            .request(json!({"op":"subscribe","streams":["test.changed"]}))
            .await;
        let id = response["data"]["subscription"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (event, bytes) = next_with_bytes(&mut self.socket).await;
        assert_eq!(event["kind"], "event");
        assert_eq!(event["stream"], "test.changed");
        assert_eq!(event["route"], self.route("subscription"));
        assert_eq!(event["event"]["subscription_id"], id);
        assert_eq!(event["event"]["data"]["first"], true);
        self.traffic.events += 1;
        self.traffic.response_bytes += bytes as u64;
        self.subscription = Some(id);
    }
    async fn cancel(&mut self) {
        let id = self
            .subscription
            .take()
            .expect("owned subscription to cancel");
        let response = self.request(json!({"op":"cancel","request_id":id})).await;
        assert_eq!(response["ok"], true);
    }
}
impl Adapter for Peer {
    async fn step(&mut self, step: Step, payload_bytes: usize) -> Result<Traffic> {
        self.traffic = Traffic::default();
        match step {
            Step::Read => {
                let payload = "x".repeat(payload_bytes);
                let response = self
                    .request(json!({"op":"call","method":"echo","params":{"payload":payload}}))
                    .await;
                assert_eq!(response["payload"], payload);
            }
            Step::Status => {
                let id = self.subscription.clone().expect("subscription status");
                let response = self
                    .request(json!({"op":"call","method":"owned-status","params":{"id":id}}))
                    .await;
                assert_eq!(response["owned"], true);
            }
            Step::Renew => {
                self.cancel().await;
                self.subscribe().await;
            }
            Step::Cancel => self.cancel().await,
            Step::Subscribe => self.subscribe().await,
        }
        Ok(self.traffic)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mixed_native_owners_with_stalled_peer_is_a_correctness_self_test() {
    let server = Running::start(JsonlSocketConfig {
        write_timeout: Duration::from_millis(50),
        output_capacity: 2,
        pending_event_limit: 2,
        ..Default::default()
    })
    .await;
    let before = support::memory_snapshot(&[std::process::id()]).unwrap();
    let mut peers = vec![
        Peer::connect(&server.path).await,
        Peer::connect(&server.path).await,
        Peer::connect(&server.path).await,
    ];
    peers[0].subscribe().await;
    peers[1].subscribe().await;
    let foreign = peers[0].subscription.clone().unwrap();
    assert_eq!(
        peers[1]
            .request(json!({"op":"call","method":"owned-status","params":{"id":foreign}}))
            .await["owned"],
        false
    );
    assert_eq!(
        peers[1]
            .request(json!({"op":"cancel","request_id":foreign}))
            .await["ok"],
        false
    );
    peers[0].step(Step::Status, 0).await.unwrap();
    peers[0].cancel().await;
    peers[1].cancel().await;
    let mut stalled = Peer::connect(&server.path).await;
    stalled.subscribe().await;
    send(
        &mut stalled.socket,
        json!({"op":"call","id":"stalled","method":"large"}),
    )
    .await;
    // Never read this peer again. The production writer must encounter real
    // kernel backpressure; healthy connections use the same listener/service.
    let workload = Workload {
        warmup_cycles: 2,
        sample_cycles: 8,
        payload_bytes: 256,
        step_deadline: Duration::from_secs(3),
        total_deadline: Duration::from_secs(20),
    };
    let measured = support::mixed(&mut peers, workload).await;
    // Observe writer-driven retirement BEFORE closing the unread socket. Do not
    // mistake ordinary caller disconnect cleanup for stalled-writer evidence.
    until(|| server.service.disconnects.load(Ordering::SeqCst) > 0).await;
    until(|| server.service.resources.lock().unwrap().is_empty()).await;
    for peer in &mut peers {
        peer.step(Step::Read, 256).await.unwrap();
    }
    // Keep cleanup reachable on an ordinary Result failure.
    drop(peers);
    drop(stalled);
    until(|| server.service.disconnects.load(Ordering::SeqCst) >= 4).await;
    let measured = measured.unwrap();
    for owner in measured["owners"].as_array().unwrap() {
        assert_eq!(owner["latency"]["read"]["n"], 16);
        for step in ["status", "renew", "cancel", "subscribe"] {
            assert_eq!(owner["latency"][step]["n"], 8);
        }
        let traffic = &owner["traffic_including_setup_warmup_cleanup"];
        assert_eq!(traffic["requests"], 72); // 10*7 + initial subscribe + final cancel
        assert_eq!(traffic["events"], 21); // 10*2 + initial baseline
        assert!(traffic["request_bytes"].as_u64().unwrap() > 5120);
        assert!(traffic["response_bytes"].as_u64().unwrap() > 5120);
    }
    let after = support::memory_snapshot(&[std::process::id()]).unwrap();
    assert!(support::memory_snapshot(&[std::process::id(), std::process::id()]).is_err());
    if let Some(path) = std::env::var_os("QUALIFICATION_SELF_TEST_REPORT") {
        // Packaged source snapshots intentionally have no .git. Identity capture
        // is opt-in in the worktree, never a prerequisite for package self-tests.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let source = support::source_identity(&root, false).unwrap();
        assert_eq!(
            support::source_identity(&root, true).is_ok(),
            source["dirty"] == false
        );
        let report = json!({"classification":"correctness-self-test-NOT-performance-evidence",
            "scope":"in-process fake service plus Rust driver, actual private Unix sockets; no QML or D-Bus path",
            "status_scope":"owned fake subscription presence, NOT operation terminal recovery",
            "driver_debug_assertions":cfg!(debug_assertions),
            "source":source,
            "binary":support::binary_identity(&std::env::current_exe().unwrap()).unwrap(),
            "memory_before":before,"memory_after":after,"workload":measured});
        fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    server.finish().await;
}

struct PausedBaseline {
    peer: Peer,
    attempts: usize,
}
impl Adapter for PausedBaseline {
    async fn step(&mut self, step: Step, payload_bytes: usize) -> Result<Traffic> {
        self.attempts += 1;
        self.peer.step(step, payload_bytes).await?;
        // An accepted real subscription exists, but the enclosing adapter step
        // does not complete. Dropping that step cannot imply resource rollback.
        std::future::pending().await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn timed_out_native_adapter_retains_owned_visibility_until_explicit_cleanup() {
    let server = Running::start(JsonlSocketConfig::default()).await;
    let mut healthy = Peer::connect(&server.path).await;
    healthy.subscribe().await;
    let mut paused = [PausedBaseline {
        peer: Peer::connect(&server.path).await,
        attempts: 0,
    }];
    let error = support::mixed(
        &mut paused,
        Workload {
            warmup_cycles: 0,
            sample_cycles: 1,
            payload_bytes: 0,
            step_deadline: Duration::from_millis(100),
            total_deadline: Duration::from_secs(3),
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("no replay"));
    assert_eq!(paused[0].attempts, 1, "no retry after accepted baseline");
    let owned = paused[0].peer.subscription.clone().unwrap();
    assert_eq!(server.service.resources.lock().unwrap().len(), 2);
    assert_eq!(
        healthy
            .request(json!({"op":"call", "method":"owned-status", "params":{"id":owned}}))
            .await["owned"],
        false
    );
    assert_eq!(
        healthy
            .request(json!({"op":"cancel", "request_id":owned}))
            .await["ok"],
        false
    );
    paused[0].peer.step(Step::Status, 0).await.unwrap();
    healthy.step(Step::Status, 0).await.unwrap();
    healthy.step(Step::Read, 32).await.unwrap();
    drop(paused);
    until(|| server.service.resources.lock().unwrap().len() == 1).await;
    healthy.step(Step::Status, 0).await.unwrap();
    healthy.cancel().await;
    drop(healthy);
    until(|| server.service.resources.lock().unwrap().is_empty()).await;
    server.finish().await;
}

struct NeverReplies {
    attempts: usize,
}
impl Adapter for NeverReplies {
    async fn step(&mut self, _: Step, _: usize) -> Result<Traffic> {
        self.attempts += 1;
        std::future::pending().await
    }
}
#[tokio::test]
async fn deadline_failure_is_not_a_sample_or_a_retry() {
    let mut peers = [NeverReplies { attempts: 0 }];
    let workload = Workload {
        warmup_cycles: 0,
        sample_cycles: 1,
        payload_bytes: 0,
        step_deadline: Duration::from_millis(10),
        total_deadline: Duration::from_secs(1),
    };
    let error = support::mixed(&mut peers, workload).await.unwrap_err();
    assert!(error.to_string().contains("no replay"));
    assert_eq!(peers[0].attempts, 1);
    peers[0].attempts = 0;
    let total_deadline = Workload {
        step_deadline: Duration::from_secs(1),
        total_deadline: Duration::from_millis(10),
        ..workload
    };
    let error = support::mixed(&mut peers, total_deadline)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("total deadline; no replay"));
    assert_eq!(peers[0].attempts, 1);
    peers[0].attempts = 0;
    assert!(
        support::mixed(
            &mut peers,
            Workload {
                sample_cycles: 0,
                ..workload
            }
        )
        .await
        .is_err()
    );
    assert_eq!(peers[0].attempts, 0, "invalid workload never dispatches");
}
