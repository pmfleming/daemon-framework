use super::{ReconnectingClient, execute_request, ordinary_call, with_transport_timeout};
use crate::{
    dbus::JsonDbusClient, output_actor::test_support::Output, subscription::OwnedTaskRegistry,
    transport_tests::Bus,
};
use anyhow::Result;
use serde_json::json;
use shelllist_daemon_core::{ClientMessage, ClientRequest, DaemonEndpoint};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use zbus::message::Header;

struct Service {
    effects: Arc<AtomicUsize>,
    subscriptions: Arc<OwnedTaskRegistry>,
    held: Arc<()>,
}
#[zbus::interface(name = "org.shelllist.Deadlines")]
impl Service {
    async fn call(&self, method: &str, _params: &str) -> String {
        if method == "slow" {
            self.effects.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        json!({"ok": method != "failure"}).to_string()
    }
    async fn subscribe(
        &self,
        _streams: Vec<String>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> String {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let held = self.held.clone();
        self.subscriptions
            .spawn_for_owner(
                "late-sub".into(),
                header.sender().map(ToString::to_string),
                connection,
                async move {
                    let _held = held;
                    std::future::pending::<()>().await;
                },
            )
            .unwrap();
        json!({"data":{"subscription":{"id":"late-sub"}}}).to_string()
    }
}
const ENDPOINT: DaemonEndpoint = DaemonEndpoint::new(
    "test",
    "org.shelllist.Deadlines",
    "/test",
    "org.shelllist.Deadlines",
);
#[tokio::test]
async fn applied_slow_call_does_not_fail_healthy_call_or_replay() -> Result<()> {
    let bus = Bus::start();
    let server = bus.connection().await;
    let effects = Arc::new(AtomicUsize::new(0));
    server
        .object_server()
        .at(
            "/test",
            Service {
                effects: effects.clone(),
                subscriptions: Arc::default(),
                held: Arc::new(()),
            },
        )
        .await?;
    server.request_name(ENDPOINT.bus_name).await?;
    let connection = bus.connection().await;
    let client = JsonDbusClient::new(connection, ENDPOINT);
    let mut output = Output::new();
    let (slow, healthy) = tokio::join!(
        ordinary_call(
            "slow".into(),
            None,
            Duration::from_millis(30),
            client.call("slow", json!({}))
        ),
        ordinary_call(
            "healthy".into(),
            None,
            Duration::from_secs(2),
            client.call("healthy", json!({}))
        ),
    );
    output.handle.send(slow).await?;
    output.handle.send(healthy).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    output
        .handle
        .send(
            ordinary_call(
                "after".into(),
                None,
                Duration::from_secs(2),
                client.call("failure", json!({})),
            )
            .await,
        )
        .await?;
    let slow = output.next().await?;
    assert_eq!(slow["outcome"], "outcome-unknown");
    assert_eq!(slow["kind"], "response");
    let healthy = output.next().await?;
    assert_eq!(healthy["response"]["ok"], true);
    let failure = output.next().await?;
    assert_eq!(failure["ok"], true); // domain failure, not a transport failure
    assert_eq!(failure["response"]["ok"], false);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    output.finish().await
}

#[tokio::test]
async fn subscription_expiry_recovers_owner_even_when_server_creates_it_late() -> Result<()> {
    let bus = Bus::start();
    let server = bus.connection().await;
    let held = Arc::new(());
    server
        .object_server()
        .at(
            "/test",
            Service {
                effects: Arc::default(),
                subscriptions: Arc::default(),
                held: held.clone(),
            },
        )
        .await?;
    server.request_name(ENDPOINT.bus_name).await?;
    let connection = bus.connection().await;
    let client = JsonDbusClient::new(connection.clone(), ENDPOINT);
    let mut output = Output::new();
    assert!(
        with_transport_timeout(
            &output.handle,
            Duration::from_millis(30),
            client.subscribe(vec!["state".into()])
        )
        .await
        .is_err()
    );
    let error = output.next().await?;
    assert_eq!(error["kind"], "transport-error");
    // The real QML transport-error path terminates/restarts the whole bridge.
    // Close the old owner here, not just the timed-out subscription future.
    connection.close().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        Arc::strong_count(&held),
        2,
        "only test and service retain marker; late task was cleaned up"
    );
    output.finish().await
}

#[tokio::test]
async fn readiness_expiry_is_rejected_before_dispatch_not_generation_failure() -> Result<()> {
    let bus = Bus::start();
    let dbus = ReconnectingClient::new(ENDPOINT);
    *dbus.current.lock().await = Some(JsonDbusClient::new(bus.connection().await, ENDPOINT));
    let mut output = Output::new();
    let command = execute_request(
        dbus,
        &output.handle,
        ClientMessage {
            request: ClientRequest::Subscribe {
                id: "waiting".into(),
                streams: vec![],
            },
            route: None,
        },
    )
    .await;
    output.handle.send(command).await?;
    let response = output.next().await?;
    assert_eq!(response["outcome"], "rejected-before-dispatch");
    output.finish().await
}
