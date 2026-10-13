use super::*;
use crate::{BasicCorrelation, OwnedTaskRegistry, transport_tests::Bus};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncBufReadExt, duplex};
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
fn no_mapper(_: &str, _: &anyhow::Error) -> CallFailure {
    panic!("uncertainty must bypass domain mapper")
}

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
    let (writer, reader) = duplex(8192);
    let (output, task) = spawn_output_actor_with_writer(BasicCorrelation, 8, 8, writer);
    let mut lines = BufReader::new(reader).lines();
    let (slow, healthy) = tokio::join!(
        ordinary_call(
            "slow".into(),
            None,
            Duration::from_millis(30),
            "slow",
            no_mapper,
            client.call("slow", json!({}))
        ),
        ordinary_call(
            "healthy".into(),
            None,
            Duration::from_secs(2),
            "healthy",
            no_mapper,
            client.call("healthy", json!({}))
        ),
    );
    output.send(slow).await?;
    output.send(healthy).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    output
        .send(
            ordinary_call(
                "after".into(),
                None,
                Duration::from_secs(2),
                "failure",
                no_mapper,
                client.call("failure", json!({})),
            )
            .await,
        )
        .await?;
    drop(output);
    task.await??;
    let slow: Value = serde_json::from_str(&lines.next_line().await?.unwrap())?;
    assert_eq!(slow["outcome"], "outcome-unknown");
    assert_eq!(slow["kind"], "response");
    let healthy: Value = serde_json::from_str(&lines.next_line().await?.unwrap())?;
    assert_eq!(healthy["response"]["ok"], true);
    let failure: Value = serde_json::from_str(&lines.next_line().await?.unwrap())?;
    assert_eq!(failure["ok"], true); // domain failure, not a transport failure
    assert_eq!(failure["response"]["ok"], false);
    assert!(lines.next_line().await?.is_none());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    Ok(())
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
    let (writer, reader) = duplex(4096);
    let (output, task) = spawn_output_actor_with_writer(BasicCorrelation, 8, 8, writer);
    let mut lines = BufReader::new(reader).lines();
    assert!(
        with_transport_timeout(
            &output,
            Duration::from_millis(30),
            client.subscribe(vec!["state".into()])
        )
        .await
        .is_err()
    );
    let error: Value = serde_json::from_str(&lines.next_line().await?.unwrap())?;
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
    drop(output);
    task.await??;
    Ok(())
}

#[tokio::test]
async fn readiness_expiry_is_rejected_before_dispatch_not_generation_failure() -> Result<()> {
    let bus = Bus::start();
    let dbus = ReconnectingClient::new(ENDPOINT);
    *dbus.current.lock().await = Some(JsonDbusClient::new(bus.connection().await, ENDPOINT));
    let (writer, reader) = duplex(4096);
    let (output, task) = spawn_output_actor_with_writer(BasicCorrelation, 8, 8, writer);
    let command = execute_request(
        dbus,
        &output,
        ClientMessage {
            request: ClientRequest::Subscribe {
                id: "waiting".into(),
                streams: vec![],
            },
            route: None,
        },
        CancelMode::Json,
        no_mapper,
    )
    .await;
    output.send(command).await?;
    drop(output);
    task.await??;
    let mut lines = BufReader::new(reader).lines();
    let response: Value = serde_json::from_str(&lines.next_line().await?.unwrap())?;
    assert_eq!(response["outcome"], "rejected-before-dispatch");
    assert!(lines.next_line().await?.is_none());
    Ok(())
}

#[tokio::test]
#[ignore = "manual cost sample, no timing assertion"]
async fn measure_proxy_and_json_cost() -> Result<()> {
    let bus = Bus::start();
    let connection = bus.connection().await;
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        let _ = zbus::Proxy::new(
            &connection,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await?;
    }
    eprintln!("1000 owned proxy constructions: {:?}", start.elapsed());
    let payload = json!({"data": {"rows": vec!["sample"; 1000]}});
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(serde_json::to_string(&payload)?);
    }
    eprintln!("1000 ~9KB JSON serializations: {:?}", start.elapsed());
    Ok(())
}
