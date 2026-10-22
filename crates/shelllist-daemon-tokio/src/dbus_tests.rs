use super::JsonDbusClient;
use crate::transport_tests::Bus;
use futures::StreamExt;
use serde_json::{Value, json};
use shelllist_daemon_core::DaemonEndpoint;
use std::time::{Duration, Instant};

const ENDPOINT: DaemonEndpoint = DaemonEndpoint::new(
    "probe",
    "org.shelllist.ProxyProbe",
    "/probe",
    "org.shelllist.ProxyProbe",
);
struct Probe(u8);
#[zbus::interface(name = "org.shelllist.ProxyProbe")]
impl Probe {
    fn call(&self, _method: &str, _params: &str) -> String {
        json!({"owner":self.0}).to_string()
    }
}

async fn server(bus: &Bus, owner: u8) -> zbus::Connection {
    let server = bus.connection().await;
    server
        .object_server()
        .at("/probe", Probe(owner))
        .await
        .unwrap();
    server.request_name(ENDPOINT.bus_name).await.unwrap();
    server
}

#[tokio::test]
async fn method_only_proxies_add_no_match_round_trips_and_follow_owner_replacement() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let bus = Bus::start();
        let first = server(&bus, 1).await;
        let monitor = bus.connection().await;
        let monitoring = zbus::Proxy::new(
            &monitor,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.Monitoring",
        )
        .await
        .unwrap();
        let mut messages = zbus::MessageStream::from(&monitor);
        monitoring
            .call::<_, _, ()>("BecomeMonitor", &(Vec::<String>::new(), 0u32))
            .await
            .unwrap();
        let caller = bus.connection().await;
        let sender = caller.unique_name().unwrap().to_owned();
        let client = JsonDbusClient::new(caller, ENDPOINT);
        for _ in 0..10 {
            assert_eq!(client.call("probe", json!({})).await.unwrap()["owner"], 1);
        }
        let (mut calls, mut matches, mut lookups) = (0, 0, 0);
        while calls < 10 {
            let message = messages.next().await.unwrap().unwrap();
            let header = message.header();
            if header.sender() != Some(&sender)
                || header.message_type() != zbus::message::Type::MethodCall
            {
                continue;
            }
            match header.member().map(|m| m.as_str()) {
                Some("Call") => calls += 1,
                Some("AddMatch" | "RemoveMatch") => matches += 1,
                Some("GetNameOwner" | "GetAll") => lookups += 1,
                _ => {}
            }
        }
        assert_eq!((calls, matches, lookups), (10, 0, 0));
        first.release_name(ENDPOINT.bus_name).await.unwrap();
        let _second = server(&bus, 2).await;
        assert_eq!(client.call("probe", json!({})).await.unwrap()["owner"], 2);
    })
    .await
    .expect("private-bus monitoring must terminate");
}

#[tokio::test]
#[ignore = "release-only private-bus proxy-cost investigation"]
async fn profile_fresh_and_reused_method_proxies() {
    let bus = Bus::start();
    let _server = server(&bus, 1).await;
    let connection = bus.connection().await;
    let client = JsonDbusClient::new(connection.clone(), ENDPOINT);
    let proxy = client.proxy().await.unwrap();
    for run in 1..=3 {
        for reuse in [false, true] {
            let start = Instant::now();
            for _ in 0..1000 {
                let value = if reuse {
                    let params = serde_json::to_string(&json!({})).unwrap();
                    let response: String = proxy
                        .call("Call", &("probe", params.as_str()))
                        .await
                        .unwrap();
                    serde_json::from_str::<Value>(&response).unwrap()
                } else {
                    client.call("probe", json!({})).await.unwrap()
                };
                assert_eq!(value["owner"], 1);
            }
            eprintln!(
                "run={run} reused={reuse} calls=1000 elapsed_ms={:.3}",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}
