use super::{Admitted, ServiceBoundary};
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

#[tokio::test]
async fn admission_limits_are_pre_dispatch_and_permits_are_released() {
    let boundary = ServiceBoundary::with_limits(API, 1, 1, 16, 128);
    assert_eq!(
        rejected(boundary.call(None, "test", "{}")),
        "owner-required"
    );
    assert_eq!(
        rejected(boundary.call(Some(":1.1"), "test", &"x".repeat(17))),
        "request-too-large"
    );
    assert_eq!(
        rejected(boundary.subscribe(Some(":1.1"), &vec!["s".into(); 65])),
        "request-too-large"
    );
    let admitted = boundary.call(Some(":1.1"), "test", "{}").unwrap();
    assert_eq!(
        rejected(boundary.subscribe(Some(":1.2"), &["s".into()])),
        "rejected-before-dispatch"
    );
    let control = boundary.cancel(Some(":1.1"), "id").unwrap();
    assert!(boundary.cancel(Some(":1.2"), "other").is_err());
    assert_eq!(
        code(&admitted.encode(&json!({"large": "x".repeat(129)}))),
        "outcome-unknown"
    );
    assert_eq!(code(&admitted.encoded("x".repeat(129))), "outcome-unknown");
    drop(control);
    assert_eq!(
        code(
            &admitted
                .run::<Value, _>(async { panic!("undeclared work must not run") })
                .await
        ),
        "rejected-before-dispatch"
    );
    assert!(boundary.call(Some(":1.1"), "test", "{}").is_ok());
    assert!(boundary.cancel(Some(":1.1"), "id").is_ok());
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
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let bus = crate::transport_tests::Bus::start();
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
        let response: String = proxy.call("Cancel", &("operation",)).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap()["ok"],
            true
        );
        assert_eq!(executed.load(Ordering::SeqCst), 1);
        // Reject old interfaces rather than supporting a second cancellation signature.
        let old = zbus::Proxy::new(
            &caller,
            server.unique_name().unwrap().as_str(),
            "/test",
            "org.shelllist.Admission1",
        )
        .await
        .unwrap();
        assert!(
            old.call::<_, _, String>("Cancel", &("operation",))
                .await
                .is_err()
        );
        finish.add_permits(1);
        assert_eq!(
            serde_json::from_str::<Value>(&pending.await.unwrap()).unwrap()["ok"],
            true
        );
    })
    .await
    .expect("direct-bus admission must not deadlock");
}
