use super::{API, ServiceBoundary, code, rejected};
use serde_json::{Value, json};
use shelllist_daemon_core::{ExecutionMode, MethodPolicy, Sensitivity};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
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
async fn drained(boundary: &ServiceBoundary) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            // Probe restored method and owner capacity instead of inspecting
            // the representation of the boundary's accounting maps.
            if ["read", "mutate"].into_iter().all(|method| {
                let first =
                    boundary.call_policy(Some(":1.1"), method, "{}", policy(ExecutionMode::Read));
                let second =
                    boundary.call_policy(Some(":1.2"), method, "{}", policy(ExecutionMode::Read));
                first.is_ok() && second.is_ok()
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
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
    let undeclared = boundary.call(Some(":1.1"), "read", "{}").unwrap();
    assert_eq!(
        code(
            &undeclared
                .run::<Value, _>(async { panic!("undeclared work ran") })
                .await
        ),
        "rejected-before-dispatch"
    );
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
        rejected(boundary.call_policy(None, "read", "{}", policy)),
        "owner-required"
    );
    assert_eq!(
        rejected(boundary.call_policy(Some(":1.1"), "read", "123456789", policy)),
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
    assert_eq!(code(&first.encoded("x".repeat(129))), "outcome-unknown");
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
