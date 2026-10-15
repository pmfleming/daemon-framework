use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use serde_json::{Value, json};
use shelllist_daemon_core::{
    ClientMessage, ClientRequest, ClientRoute, DEFAULT_JSONL_REQUEST_BYTES, DaemonEndpoint,
    MAX_JSONL_REQUEST_BYTES,
};
use tokio::io::BufReader;
use tokio::sync::{Mutex, Semaphore, watch};
use tokio::task::{JoinHandle, JoinSet};

#[path = "jsonl_input.rs"]
mod input;
use input::{Frame, Frames};

use crate::{
    dbus::JsonDbusClient,
    output_actor::{
        CorrelationPolicy, OutputCommand, OutputHandle, spawn_output_actor_with_writer,
    },
    task::AbortOnDrop,
};

const OUTPUT_CAPACITY: usize = 64;
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_millis(250);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_CAPACITY: usize = 16;

struct RequestSlots {
    calls: Arc<Semaphore>,
    controls: Arc<Semaphore>,
}

impl RequestSlots {
    fn new(maximum_calls: usize) -> Self {
        Self {
            calls: Arc::new(Semaphore::new(maximum_calls)),
            controls: Arc::new(Semaphore::new(CONTROL_CAPACITY)),
        }
    }

    fn acquire(
        &self,
        request: &ClientRequest,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, tokio::sync::TryAcquireError> {
        let slots = match request {
            ClientRequest::Call { .. } | ClientRequest::Subscribe { .. } => &self.calls,
            _ => &self.controls,
        };
        Arc::clone(slots).try_acquire_owned()
    }
}

pub struct JsonlClientConfig<P> {
    pub endpoint: DaemonEndpoint,
    pub correlation: P,
    pub pending_event_limit: usize,
    pub max_in_flight_requests: usize,
    /// `None` drains every accepted request; a duration bounds graceful drain.
    pub shutdown_timeout: Option<Duration>,
}

#[derive(Clone)]
struct ReconnectingClient {
    endpoint: DaemonEndpoint,
    current: Arc<Mutex<Option<JsonDbusClient>>>,
    event_ready: watch::Sender<bool>,
}

impl ReconnectingClient {
    fn new(endpoint: DaemonEndpoint) -> Self {
        let (event_ready, _) = watch::channel(false);
        Self {
            endpoint,
            current: Arc::new(Mutex::new(None)),
            event_ready,
        }
    }

    async fn get(&self) -> Result<JsonDbusClient> {
        let mut current = self.current.lock().await;
        if let Some(client) = current.as_ref() {
            return Ok(client.clone());
        }
        let client = JsonDbusClient::session(self.endpoint).await?;
        *current = Some(client.clone());
        Ok(client)
    }

    async fn invalidate(&self) {
        self.event_ready.send_replace(false);
        self.current.lock().await.take();
    }

    async fn wait_for_event_forwarding(&self) -> Result<()> {
        self.event_ready
            .subscribe()
            .wait_for(|ready| *ready)
            .await
            .context("daemon event readiness channel closed")?;
        Ok(())
    }
}

pub async fn run_jsonl_client<P: CorrelationPolicy>(config: JsonlClientConfig<P>) -> Result<()> {
    run_jsonl_client_with_input_limit(config, DEFAULT_JSONL_REQUEST_BYTES).await
}

/// Opt in to a larger bounded record for bulk payloads. This changes neither
/// request admission, output bounds, deadlines nor direct D-Bus validation.
pub async fn run_jsonl_client_with_input_limit<P: CorrelationPolicy>(
    config: JsonlClientConfig<P>,
    maximum_request_bytes: usize,
) -> Result<()> {
    anyhow::ensure!(
        (1..=MAX_JSONL_REQUEST_BYTES).contains(&maximum_request_bytes),
        "invalid JSONL request byte limit"
    );
    let dbus = ReconnectingClient::new(config.endpoint);
    let (output, output_task) = spawn_output_actor_with_writer(
        config.correlation,
        OUTPUT_CAPACITY,
        config.pending_event_limit,
        tokio::io::stdout(),
    );
    let event_task =
        AbortOnDrop(spawn_event_forwarder(dbus.clone(), output.clone()).abort_handle());
    let owner_task = AbortOnDrop(spawn_owner_watcher(dbus.clone(), output.clone()).abort_handle());

    let mut calls = JoinSet::new();
    let request_slots = RequestSlots::new(config.max_in_flight_requests.max(1));
    let shutdown_id = request_loop(
        &dbus,
        &output,
        &mut calls,
        &request_slots,
        maximum_request_bytes,
    )
    .await?;
    drain_calls(&mut calls, config.shutdown_timeout).await;
    cancel_active(&dbus, &output, CONTROL_TIMEOUT).await;

    drop((event_task, owner_task));
    if let Some(id) = shutdown_id {
        output.send(OutputCommand::Shutdown(id)).await?;
    }
    drop(output);
    output_task
        .await
        .context("join JSONL output task")?
        .context("run JSONL output task")
}

async fn request_loop(
    dbus: &ReconnectingClient,
    output: &OutputHandle,
    calls: &mut JoinSet<()>,
    request_slots: &RequestSlots,
    maximum_request_bytes: usize,
) -> Result<Option<String>> {
    let mut frames = Frames::new(BufReader::new(tokio::io::stdin()), maximum_request_bytes);
    while let Some(frame) = frames.next().await.context("read JSONL request")? {
        let line = match frame {
            Frame::Line(line) => line,
            Frame::TooLarge => {
                output
                    .send(OutputCommand::ProtocolError(format!(
                        "JSONL request exceeds {maximum_request_bytes} bytes"
                    )))
                    .await?;
                reap_finished(calls);
                continue;
            }
        };
        let Some(request) = parse_request(&line, output).await? else {
            continue;
        };
        if let ClientRequest::Shutdown { id } = request.request {
            return Ok(Some(id));
        }
        spawn_request(calls, dbus, output, request, request_slots).await;
        reap_finished(calls);
    }
    Ok(None)
}

async fn parse_request(line: &str, output: &OutputHandle) -> Result<Option<ClientMessage>> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    let request = serde_json::from_str::<ClientMessage>(line)
        .map_err(|error| error.to_string())
        .and_then(|request| request.validate().map(|()| request).map_err(str::to_owned));
    match request {
        Ok(request) => Ok(Some(request)),
        Err(error) => {
            output.send(OutputCommand::ProtocolError(error)).await?;
            Ok(None)
        }
    }
}

async fn spawn_request(
    calls: &mut JoinSet<()>,
    dbus: &ReconnectingClient,
    output: &OutputHandle,
    message: ClientMessage,
    request_slots: &RequestSlots,
) {
    // Acquire before spawning: a semaphore inside the task still permits an
    // unbounded backlog of waiting tasks. Reject overload without replaying it.
    let permit = match request_slots.acquire(&message.request) {
        Ok(permit) => permit,
        Err(_) => {
            let id = match message.request {
                ClientRequest::Call { id, .. }
                | ClientRequest::Subscribe { id, .. }
                | ClientRequest::Cancel { id, .. }
                | ClientRequest::Release { id }
                | ClientRequest::Shutdown { id } => id,
            };
            let _ = output
                .send(OutputCommand::RequestError {
                    id,
                    outcome: "rejected-before-dispatch",
                    message: "bridge request capacity exceeded; request was not sent".into(),
                    route: message.route,
                })
                .await;
            return;
        }
    };
    let dbus = dbus.clone();
    let output = output.clone();
    calls.spawn(async move {
        let _permit = permit;
        let command = execute_request(dbus, &output, message).await;
        let _ = output.send(command).await;
    });
}

async fn execute_request(
    dbus: ReconnectingClient,
    output: &OutputHandle,
    message: ClientMessage,
) -> OutputCommand {
    let ClientMessage { request, route } = message;
    match request {
        ClientRequest::Call { id, method, params } => {
            let Ok(Ok(client)) = tokio::time::timeout(CONTROL_TIMEOUT, dbus.get()).await else {
                return OutputCommand::RequestError {
                    id,
                    route,
                    outcome: "rejected-before-dispatch",
                    message: "daemon connection unavailable; request was not sent".into(),
                };
            };
            ordinary_call(id, route, REQUEST_TIMEOUT, client.call(&method, params)).await
        }
        ClientRequest::Subscribe { id, streams } => {
            // Readiness has not admitted a server subscription or effect.
            if !matches!(
                tokio::time::timeout(CONTROL_TIMEOUT, dbus.wait_for_event_forwarding()).await,
                Ok(Ok(()))
            ) {
                return OutputCommand::RequestError {
                    id,
                    route,
                    outcome: "rejected-before-dispatch",
                    message: "daemon event forwarding is not ready; subscription was not sent"
                        .into(),
                };
            }
            let result = with_transport_timeout(output, REQUEST_TIMEOUT, async {
                dbus.get().await?.subscribe(streams).await
            })
            .await;
            response_command(id, result, None, route)
        }
        ClientRequest::Cancel { id, request_id } => {
            let result = with_transport_timeout(output, CONTROL_TIMEOUT, async {
                let client = dbus.get().await?;
                client.cancel_json(&request_id).await
            })
            .await;
            let cancelled = result
                .as_ref()
                .ok()
                .filter(|response| cancellation_succeeded(response))
                .map(|_| request_id);
            response_command(id, result, cancelled, route)
        }
        ClientRequest::Release { id } => {
            let result = release_consumer(&dbus, output, route.as_ref()).await;
            response_command(id, result, None, route)
        }
        ClientRequest::Shutdown { id } => OutputCommand::Shutdown(id),
    }
}

async fn release_consumer(
    dbus: &ReconnectingClient,
    output: &OutputHandle,
    route: Option<&ClientRoute>,
) -> Result<Value> {
    let route = route.context("release requires a consumer route")?;
    let ids = output.owned_ids(route).await;
    with_transport_timeout(output, CONTROL_TIMEOUT, async {
        let released = ids.len();
        for id in ids {
            let response = dbus.get().await?.cancel_json(&id).await?;
            anyhow::ensure!(
                cancellation_succeeded(&response),
                "daemon rejected cancellation of {id}"
            );
            output.send(OutputCommand::Cancelled(id)).await?;
        }
        Ok(json!({ "released": released }))
    })
    .await
}

// Ordinary expiry never requests generation recovery and bypasses domain mappers
// that would otherwise erase uncertainty as `daemon-unavailable`. Never replay.
async fn ordinary_call(
    id: String,
    route: Option<ClientRoute>,
    timeout: Duration,
    future: impl Future<Output = Result<Value>>,
) -> OutputCommand {
    let message = match tokio::time::timeout(timeout, future).await {
        Ok(Ok(response)) => return response_command(id, Ok(response), None, route),
        Err(_) => format!(
            "daemon request timed out after {}ms; outcome unknown; reconcile state, do not retry automatically",
            timeout.as_millis()
        ),
        // A D-Bus/decoding error after entering Call does not prove that the
        // effect was rejected. Domain failures arrive as valid API envelopes.
        Ok(Err(error)) => format!(
            "daemon call reply unavailable; outcome unknown; do not retry automatically: {error:#}"
        ),
    };
    OutputCommand::RequestError {
        id,
        route,
        outcome: "outcome-unknown",
        message,
    }
}

async fn with_transport_timeout<T>(
    output: &OutputHandle,
    timeout: Duration,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result,
        Err(_) => {
            let message = format!("daemon request timed out after {}ms", timeout.as_millis());
            // The frontend restarts the bridge on transport-error. Keep this
            // generation intact until then: replacing just the cached client
            // strands the event reader on its old connection and leaves new
            // subscriptions waiting for readiness that can never arrive.
            output
                .send(OutputCommand::TransportError(message.clone()))
                .await?;
            anyhow::bail!(message)
        }
    }
}

fn response_command(
    id: String,
    result: Result<Value>,
    cancelled_request_id: Option<String>,
    route: Option<ClientRoute>,
) -> OutputCommand {
    OutputCommand::Response {
        id,
        result: result.map_err(|error| error.to_string()),
        cancelled_request_id,
        route,
    }
}

fn cancellation_succeeded(response: &Value) -> bool {
    response.get("ok").and_then(Value::as_bool) != Some(false)
}

fn spawn_event_forwarder(dbus: ReconnectingClient, output: OutputHandle) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut delay = INITIAL_RECONNECT_DELAY;
        let mut last_error = None;
        loop {
            dbus.event_ready.send_replace(false);
            let Err(error) = forward_events(&dbus, &output).await else {
                return;
            };
            let message = error.to_string();
            if output.send(OutputCommand::ResetCorrelation).await.is_err()
                || !report_transport_error(&output, &mut last_error, message).await
            {
                return;
            }
            dbus.invalidate().await;
            tokio::time::sleep(delay).await;
            delay = delay.saturating_mul(2).min(MAX_RECONNECT_DELAY);
        }
    })
}

async fn forward_events(dbus: &ReconnectingClient, output: &OutputHandle) -> Result<()> {
    let mut events = dbus.get().await?.events().await?;
    // Retain readiness even when no subscription is currently waiting.
    dbus.event_ready.send_replace(true);
    while let Some(message) = events.next().await {
        let body = message.body();
        let (stream, event_json): (String, &str) =
            body.deserialize().context("decode daemon event signal")?;
        let event = serde_json::from_str::<Value>(event_json)
            .unwrap_or_else(|_| json!({ "raw": event_json }));
        output.send(OutputCommand::Event { stream, event }).await?;
    }
    anyhow::bail!("daemon event stream ended")
}

async fn report_transport_error(
    output: &OutputHandle,
    last_error: &mut Option<String>,
    message: String,
) -> bool {
    if last_error.as_deref() == Some(message.as_str()) {
        return true;
    }
    let sent = output
        .send(OutputCommand::TransportError(message.clone()))
        .await
        .is_ok();
    *last_error = Some(message);
    sent
}

fn spawn_owner_watcher(dbus: ReconnectingClient, output: OutputHandle) -> JoinHandle<()> {
    tokio::spawn(async move { while watch_owner_once(&dbus, &output).await {} })
}

async fn watch_owner_once(dbus: &ReconnectingClient, output: &OutputHandle) -> bool {
    let result = async { dbus.get().await?.watch_replacement().await }.await;
    match result {
        Ok(()) => {
            dbus.invalidate().await;
            if output.send(OutputCommand::ResetCorrelation).await.is_err() {
                return false;
            }
            output
                .send(OutputCommand::TransportError(format!(
                    "{} D-Bus owner changed",
                    dbus.endpoint.executable
                )))
                .await
                .is_ok()
        }
        Err(error) => {
            tracing::warn!(%error, "daemon owner watcher stopped");
            tokio::time::sleep(INITIAL_RECONNECT_DELAY).await;
            true
        }
    }
}

fn reap_finished(calls: &mut JoinSet<()>) {
    while let Some(result) = calls.try_join_next() {
        log_join_result(result);
    }
}

fn log_join_result(result: std::result::Result<(), tokio::task::JoinError>) {
    if let Err(error) = result {
        tracing::warn!(%error, "daemon JSONL call task failed");
    }
}

async fn drain_calls(calls: &mut JoinSet<()>, timeout: Option<Duration>) {
    let Some(timeout) = timeout else {
        drain_all(calls).await;
        return;
    };
    if tokio::time::timeout(timeout, drain_all(calls))
        .await
        .is_ok()
    {
        return;
    }
    calls.shutdown().await;
}

async fn drain_all(calls: &mut JoinSet<()>) {
    while let Some(result) = calls.join_next().await {
        log_join_result(result);
    }
}

// One budget covers connection acquisition, the ID query and all cancellations.
async fn cancel_active(dbus: &ReconnectingClient, output: &OutputHandle, timeout: Duration) {
    let cleanup = async {
        let Ok(client) = dbus.get().await else {
            return;
        };
        for id in output.active_ids().await {
            let _ = client.cancel_json(&id).await;
        }
    };
    if tokio::time::timeout(timeout, cleanup).await.is_err() {
        tracing::warn!("daemon cancellation cleanup timed out");
    }
}

#[cfg(test)]
#[path = "jsonl_deadline_tests.rs"]
mod deadline_tests;

#[cfg(test)]
mod tests {
    use crate::output_actor::test_support::Output;
    use anyhow::Result;
    use serde_json::json;

    use super::{ReconnectingClient, RequestSlots, spawn_request};
    use shelllist_daemon_core::{ClientMessage, ClientRequest, DaemonEndpoint};

    use tokio::task::JoinSet;

    #[tokio::test]
    async fn invalid_limits_and_overload_are_rejected_before_dispatch() -> Result<()> {
        let endpoint = DaemonEndpoint::new("test", "org.test.Daemon", "/test", "org.test.Daemon");
        for maximum in [0, shelllist_daemon_core::MAX_JSONL_REQUEST_BYTES + 1] {
            let config = super::JsonlClientConfig {
                endpoint,
                correlation: crate::output_actor::BasicCorrelation,
                pending_event_limit: 1,
                max_in_flight_requests: 1,
                shutdown_timeout: None,
            };
            assert!(
                super::run_jsonl_client_with_input_limit(config, maximum)
                    .await
                    .is_err()
            );
        }
        let mut output = Output::new();
        let slots = RequestSlots::new(0);
        let message: ClientMessage = serde_json::from_value(json!({
            "op": "call", "id": "view::mutation", "method": "must.not.execute",
            "route": { "consumerId": "view", "localId": "mutation", "generation": 2, "kind": "call" }
        }))?;
        let expected_route = serde_json::to_value(message.route.as_ref().unwrap())?;
        let mut calls = JoinSet::new();
        // No D-Bus connection exists; overload must be rejected before execution.
        let dbus = ReconnectingClient::new(endpoint);
        spawn_request(&mut calls, &dbus, &output.handle, message, &slots).await;
        assert!(calls.is_empty());
        let response = output.next().await?;
        assert_eq!(response["ok"], false);
        assert_eq!(response["route"], expected_route);
        assert!(response["error"].as_str().unwrap().contains("not sent"));
        // A separate, bounded control lane remains usable under call saturation.
        let permits = (0..super::CONTROL_CAPACITY)
            .map(|_| {
                slots
                    .acquire(&ClientRequest::Release {
                        id: "release".into(),
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(
            slots
                .acquire(&ClientRequest::Release {
                    id: "overflow".into()
                })
                .is_err()
        );
        drop(permits);
        output.finish().await
    }
}
