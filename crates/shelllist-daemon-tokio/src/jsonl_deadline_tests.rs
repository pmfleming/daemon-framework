use super::{
    ReconnectingClient, cancel_active, execute_request, ordinary_call, with_transport_timeout,
};
use crate::{
    dbus::JsonDbusClient,
    output_actor::{OutputCommand, test_support::Output},
    subscription::OwnedTaskRegistry,
    transport_tests::{Bus, within_seconds},
};
use anyhow::Result;
use serde_json::json;
use shelllist_daemon_core::{ClientMessage, ClientRequest, DaemonEndpoint};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;
use zbus::message::Header;

struct Service {
    effects: Arc<AtomicUsize>,
    subscriptions: Arc<OwnedTaskRegistry>,
    subscription_lifetime: Arc<Mutex<Option<oneshot::Sender<()>>>>,
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
    async fn cancel(&self, id: &str) -> String {
        if id == "blocked" {
            self.effects.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
        }
        json!({"ok": id != "refused"}).to_string()
    }
    async fn subscribe(
        &self,
        _streams: Vec<String>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> String {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let lifetime = self.subscription_lifetime.lock().unwrap().take();
        self.subscriptions
            .spawn_for_owner(
                "late-sub".into(),
                header.sender().map(ToString::to_string),
                connection,
                async move {
                    let _lifetime = lifetime;
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
async fn call_and_cancel_deadlines_preserve_outcomes_ownership_and_no_replay() -> Result<()> {
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
                subscription_lifetime: Arc::default(),
            },
        )
        .await?;
    server.request_name(ENDPOINT.bus_name).await?;
    let client = JsonDbusClient::new(bus.connection().await, ENDPOINT);
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
    assert_eq!(output.next().await?["response"]["ok"], true);
    let failure = output.next().await?;
    assert_eq!(failure["ok"], true); // Domain failure, not a transport failure.
    assert_eq!(failure["response"]["ok"], false);
    assert_eq!(effects.load(Ordering::SeqCst), 1);

    let dbus = ReconnectingClient::new(ENDPOINT);
    *dbus.current.lock().await = Some(client);
    for id in ["refused", "active", "blocked"] {
        output
            .handle
            .send(OutputCommand::Response {
                id: format!("subscribe-{id}"),
                result: Ok(json!({"data":{"subscription":{"id":id}}})),
                cancelled_request_id: None,
                route: None,
            })
            .await?;
        output.next().await?;
    }
    for (id, succeeded, retained) in [
        ("refused", false, vec!["active", "blocked", "refused"]),
        ("active", true, vec!["blocked", "refused"]),
    ] {
        let command = execute_request(
            dbus.clone(),
            &output.handle,
            ClientMessage {
                request: ClientRequest::Cancel {
                    id: format!("cancel-{id}"),
                    request_id: id.into(),
                },
                route: None,
            },
        )
        .await;
        output.handle.send(command).await?;
        assert_eq!(output.next().await?["response"]["ok"], succeeded);
        let mut active = output.handle.active_ids().await;
        active.sort();
        assert_eq!(active, retained);
    }
    // A real stalled Cancel reply must not hang shutdown or be replayed.
    within_seconds(
        2,
        cancel_active(&dbus, &output.handle, Duration::from_secs(1)),
    )
    .await;
    assert_eq!(effects.load(Ordering::SeqCst), 2);
    output.finish().await
}

#[tokio::test]
async fn subscription_readiness_and_late_expiry_preserve_owner_cleanup() -> Result<()> {
    let bus = Bus::start();
    let server = bus.connection().await;
    let (lifetime, released) = oneshot::channel();
    server
        .object_server()
        .at(
            "/test",
            Service {
                effects: Arc::default(),
                subscriptions: Arc::default(),
                subscription_lifetime: Arc::new(Mutex::new(Some(lifetime))),
            },
        )
        .await?;
    server.request_name(ENDPOINT.bus_name).await?;
    let connection = bus.connection().await;
    let client = JsonDbusClient::new(connection.clone(), ENDPOINT);
    let mut output = Output::new();
    let dbus = ReconnectingClient::new(ENDPOINT);
    *dbus.current.lock().await = Some(client.clone());
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
    assert_eq!(output.next().await?["outcome"], "rejected-before-dispatch");
    assert!(
        with_transport_timeout(
            &output.handle,
            Duration::from_millis(30),
            client.subscribe(vec!["state".into()])
        )
        .await
        .is_err()
    );
    assert_eq!(output.next().await?["kind"], "transport-error");
    // The frontend closes the bridge owner on transport error. Even a server
    // subscription created after that close must release its owned worker.
    connection.close().await?;
    assert!(within_seconds(2, released).await.is_err());
    output.finish().await
}
