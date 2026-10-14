use super::*;
use serde_json::{Value, json};
use shelllist_daemon_core::Sensitivity;

const API: ApiIdentity = ApiIdentity::new("policy-test", 2);
fn policy(mode: ExecutionMode) -> MethodPolicy {
    MethodPolicy {
        ack_budget_ms: 10,
        request_bytes: 8,
        result_bytes: 128,
        max_in_flight: 2,
        max_owner_in_flight: 1,
        ..MethodPolicy::new(mode, Sensitivity::Effect, "test")
    }
}
fn code(response: &str) -> String {
    serde_json::from_str::<Value>(response).unwrap()["error"]["code"]
        .as_str()
        .unwrap()
        .into()
}
async fn drained(boundary: &ServiceBoundary) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if boundary.usage.lock().unwrap().owners.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(boundary.usage.lock().unwrap().methods.is_empty());
}

#[tokio::test]
async fn effects_retain_capacity_after_ack_timeout_and_waiter_loss() {
    let boundary = ServiceBoundary::with_limits(API, 2, 1, 1024, 1024);
    let finish = Arc::new(Semaphore::new(0));
    let run = |owner| {
        let admitted = boundary
            .call_policy(Some(owner), "mutate", "{}", policy(ExecutionMode::Effect))
            .unwrap();
        let finish = finish.clone();
        admitted.run(async move {
            finish.acquire().await.unwrap().forget();
            json!({"ok":true})
        })
    };
    assert_eq!(code(&run(":1.1").await), "outcome-unknown");
    assert!(
        boundary
            .call_policy(Some(":1.1"), "other", "{}", policy(ExecutionMode::Effect))
            .is_err()
    );
    let waiter = tokio::spawn(run(":1.2"));
    tokio::task::yield_now().await;
    waiter.abort();
    let _ = waiter.await;
    assert!(
        boundary
            .call_policy(Some(":1.3"), "mutate", "{}", policy(ExecutionMode::Effect))
            .is_err()
    );
    // Cleanup lane is independent of owner and method quotas.
    assert!(boundary.cancel(Some(":1.1"), "owned-operation").is_ok());
    finish.add_permits(2);
    drained(&boundary).await;
    assert!(
        boundary
            .call_policy(Some(":1.3"), "mutate", "{}", policy(ExecutionMode::Effect))
            .is_ok()
    );
}

#[tokio::test]
async fn read_timeout_and_panicking_handler_release_all_accounting() {
    let boundary = ServiceBoundary::new(API);
    let admitted = boundary
        .call_policy(Some(":1.1"), "read", "{}", policy(ExecutionMode::Read))
        .unwrap();
    assert_eq!(
        code(&admitted.run(std::future::pending::<Value>()).await),
        "outcome-unknown"
    );
    drained(&boundary).await;
    let admitted = boundary
        .call_policy(Some(":1.1"), "read", "{}", policy(ExecutionMode::Read))
        .unwrap();
    assert_eq!(
        code(
            &admitted
                .run::<Value, _>(async { panic!("injected handler failure") })
                .await
        ),
        "outcome-unknown"
    );
    drained(&boundary).await;
}

#[tokio::test]
async fn policy_bytes_and_two_owner_quotas_apply_before_execution() {
    let boundary = ServiceBoundary::new(API);
    let policy = policy(ExecutionMode::Read);
    assert_eq!(
        code(
            &boundary
                .call_policy(Some(":1.1"), "read", "123456789", policy)
                .err()
                .unwrap()
        ),
        "request-too-large"
    );
    let first = boundary
        .call_policy(Some(":1.1"), "read", "{}", policy)
        .unwrap();
    assert!(
        boundary
            .call_policy(Some(":1.1"), "another-read", "{}", policy)
            .is_err()
    );
    let second = boundary
        .call_policy(Some(":1.2"), "read", "{}", policy)
        .unwrap();
    assert!(
        boundary
            .call_policy(Some(":1.3"), "read", "{}", policy)
            .is_err()
    );
    assert_eq!(
        code(&second.run(async { json!({"large":"x".repeat(129)}) }).await),
        "outcome-unknown"
    );
    drop(first);
    drained(&boundary).await;
}
