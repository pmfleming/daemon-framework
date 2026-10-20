//! Forwarding mechanics; payloads and lag recovery remain daemon policy.
use shelllist_daemon_core::{ApiIdentity, Correlation};
use std::future::Future;
use tokio::sync::{broadcast, watch};
use zbus::object_server::SignalEmitter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchPhase {
    Initial,
    Changed,
}

/// Marks exactly the cloned value observed and releases the watch lock before I/O.
pub async fn forward_watch<T: Clone, F: Future<Output = ()>>(
    mut receiver: watch::Receiver<T>,
    mut emit: impl FnMut(T, WatchPhase) -> F,
) {
    let initial = receiver.borrow_and_update().clone();
    emit(initial, WatchPhase::Initial).await;
    while receiver.changed().await.is_ok() {
        let value = receiver.borrow_and_update().clone();
        emit(value, WatchPhase::Changed).await;
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BroadcastEvent<T> {
    Item(T),
    Lagged(u64),
}

/// Source closure ends forwarding. Lag is delivered explicitly, never silently skipped.
pub async fn forward_broadcast<T: Clone, F: Future<Output = ()>>(
    mut receiver: broadcast::Receiver<T>,
    mut emit: impl FnMut(BroadcastEvent<T>) -> F,
) {
    loop {
        let event = match receiver.recv().await {
            Ok(item) => BroadcastEvent::Item(item),
            Err(broadcast::error::RecvError::Lagged(count)) => BroadcastEvent::Lagged(count),
            Err(broadcast::error::RecvError::Closed) => return,
        };
        emit(event).await;
    }
}

/// Canonical emission takes an explicit body, never arbitrary envelope fields.
pub async fn emit_event_body(
    emitter: &SignalEmitter<'_>,
    interface: &str,
    api: ApiIdentity,
    stream: &str,
    event: &str,
    correlation: Correlation<'_>,
    body: shelllist_daemon_core::wire::EventBody,
) -> zbus::Result<()> {
    let message = serde_json::to_string(&body.envelope(api, stream, event, correlation))
        .map_err(|error| zbus::Error::Failure(error.to_string()))?;
    emitter.emit(interface, "Event", &(stream, message)).await
}
