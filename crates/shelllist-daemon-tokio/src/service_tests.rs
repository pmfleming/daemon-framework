use super::{Admitted, ServiceBoundary};
use crate::transport_tests::{Bus, within_seconds};
use shelllist_daemon_core::ApiIdentity;
use tokio::sync::Semaphore;

#[path = "service_policy_tests.rs"]
mod policy;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zbus::message::Header;

const API: ApiIdentity = ApiIdentity::new("test-api", 2);
fn code(response: &str) -> String {
    serde_json::from_str::<Value>(response).unwrap()["error"]["code"]
        .as_str()
        .unwrap()
        .into()
}

fn rejected(result: Result<Admitted, String>) -> String {
    code(&result.err().expect("request must be rejected"))
}

struct Probe {
    boundary: ServiceBoundary,
    started: Arc<Semaphore>,
    finish: Arc<Semaphore>,
    executed: Arc<AtomicUsize>,
}
#[zbus::interface(name = "org.shelllist.Admission2")]
impl Probe {
    async fn call(&self, method: &str, params: &str, #[zbus(header)] header: Header<'_>) -> String {
        let admitted = match self
            .boundary
            .call(header.sender().map(|s| s.as_str()), method, params)
        {
            Ok(admitted) => admitted,
            Err(response) => return response,
        };
        self.executed.fetch_add(1, Ordering::SeqCst);
        self.started.add_permits(1);
        self.finish.acquire().await.unwrap().forget();
        admitted.encode(&shelllist_daemon_core::success(API, json!({})))
    }
    async fn subscribe(&self, streams: Vec<String>, #[zbus(header)] header: Header<'_>) -> String {
        let admitted = match self
            .boundary
            .subscribe(header.sender().map(|s| s.as_str()), &streams)
        {
            Ok(admitted) => admitted,
            Err(response) => return response,
        };
        admitted.encode(&shelllist_daemon_core::success(API, json!({})))
    }
    async fn cancel(&self, id: &str, #[zbus(header)] header: Header<'_>) -> String {
        let admitted = match self
            .boundary
            .cancel(header.sender().map(|s| s.as_str()), id)
        {
            Ok(admitted) => admitted,
            Err(response) => return response,
        };
        admitted.encode(&shelllist_daemon_core::success(
            API,
            json!({"cancellation_requested": true}),
        ))
    }
}

#[tokio::test]
async fn direct_bus_saturation_keeps_cleanup_available_without_dispatching_rejected_calls() {
    within_seconds(5, async {
        let bus = Bus::start();
        let server = bus.connection().await;
        let caller = bus.connection().await;
        let started = Arc::new(Semaphore::new(0));
        let finish = Arc::new(Semaphore::new(0));
        let executed = Arc::new(AtomicUsize::new(0));
        server
            .object_server()
            .at(
                "/test",
                Probe {
                    boundary: ServiceBoundary::with_limits(API, 1, 1, 16, 1024),
                    started: started.clone(),
                    finish: finish.clone(),
                    executed: executed.clone(),
                },
            )
            .await
            .unwrap();
        let proxy = zbus::Proxy::new(
            &caller,
            server.unique_name().unwrap().to_owned(),
            "/test",
            "org.shelllist.Admission2",
        )
        .await
        .unwrap();
        let held = proxy.clone();
        let pending = tokio::spawn(async move {
            held.call::<_, _, String>("Call", &("slow", "{}"))
                .await
                .unwrap()
        });
        started.acquire().await.unwrap().forget();
        let response: String = proxy.call("Call", &("mutation", "{}")).await.unwrap();
        assert_eq!(code(&response), "rejected-before-dispatch");
        let response: String = proxy.call("Subscribe", &(vec!["events"],)).await.unwrap();
        assert_eq!(code(&response), "rejected-before-dispatch");
        let response: String = proxy
            .call("Call", &("mutation", "x".repeat(17)))
            .await
            .unwrap();
        assert_eq!(code(&response), "request-too-large");
        let response: String = proxy
            .call("Subscribe", &(vec!["events"; 65],))
            .await
            .unwrap();
        assert_eq!(code(&response), "request-too-large");
        let response: String = proxy.call("Cancel", &("operation",)).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap()["ok"],
            true
        );
        assert_eq!(executed.load(Ordering::SeqCst), 1);
        finish.add_permits(1);
        assert_eq!(
            serde_json::from_str::<Value>(&pending.await.unwrap()).unwrap()["ok"],
            true
        );
        let response: String = proxy.call("Subscribe", &(vec!["events"],)).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap()["ok"],
            true
        );
    })
    .await;
}
