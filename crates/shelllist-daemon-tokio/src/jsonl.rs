use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;
use shelllist_daemon_core::{ClientRequest, DaemonEndpoint};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, oneshot};
use tokio::task::{JoinHandle, JoinSet};

use crate::{CorrelationPolicy, JsonDbusClient, OutputCommand, OutputHandle, spawn_output_actor};

const OUTPUT_CAPACITY: usize = 64;
const PENDING_EVENT_LIMIT: usize = 32;
const MAX_IN_FLIGHT_REQUESTS: usize = 64;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_millis(250);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelMode {
    Json,
    Unit,
}

pub enum CallFailure {
    Api(Value),
    Transport(String),
}

pub type CallFailureMapper = fn(&str, &anyhow::Error) -> CallFailure;

pub struct JsonlClientConfig<P> {
    pub endpoint: DaemonEndpoint,
    pub correlation: P,
    pub cancel_mode: CancelMode,
    pub call_failure: CallFailureMapper,
}

#[derive(Clone)]
struct ReconnectingClient {
    endpoint: DaemonEndpoint,
    current: Arc<Mutex<Option<JsonDbusClient>>>,
}

impl ReconnectingClient {
    fn new(endpoint: DaemonEndpoint) -> Self {
        Self {
            endpoint,
            current: Arc::new(Mutex::new(None)),
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
        self.current.lock().await.take();
    }
}

pub async fn run_jsonl_client<P: CorrelationPolicy>(config: JsonlClientConfig<P>) -> Result<()> {
    let dbus = ReconnectingClient::new(config.endpoint);
    let (output, output_task) =
        spawn_output_actor(config.correlation, OUTPUT_CAPACITY, PENDING_EVENT_LIMIT);
    let (event_ready, ready) = oneshot::channel();
    let event_task = spawn_event_forwarder(dbus.clone(), output.clone(), event_ready);
    let owner_task = spawn_owner_watcher(dbus.clone(), output.clone());
    ready.await.context("establish daemon event forwarding")?;

    let mut calls = JoinSet::new();
    let shutdown_id = request_loop(
        &dbus,
        &output,
        &mut calls,
        config.cancel_mode,
        config.call_failure,
    )
    .await?;
    drain_calls(&mut calls).await;
    cancel_active(&dbus, &output, config.cancel_mode).await;

    event_task.abort();
    owner_task.abort();
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
    cancel_mode: CancelMode,
    call_failure: CallFailureMapper,
) -> Result<Option<String>> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await.context("read JSONL request")? {
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<ClientRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                output
                    .send(OutputCommand::ProtocolError(error.to_string()))
                    .await?;
                continue;
            }
        };
        if let ClientRequest::Shutdown { id } = request {
            return Ok(Some(id));
        }
        wait_for_request_slot(calls).await;
        spawn_request(
            calls,
            dbus.clone(),
            output.clone(),
            request,
            cancel_mode,
            call_failure,
        );
        reap_finished(calls);
    }
    Ok(None)
}

fn spawn_request(
    calls: &mut JoinSet<()>,
    dbus: ReconnectingClient,
    output: OutputHandle,
    request: ClientRequest,
    cancel_mode: CancelMode,
    call_failure: CallFailureMapper,
) {
    calls.spawn(async move {
        let command = execute_request(dbus, request, cancel_mode, call_failure).await;
        let _ = output.send(command).await;
    });
}

async fn execute_request(
    dbus: ReconnectingClient,
    request: ClientRequest,
    cancel_mode: CancelMode,
    call_failure: CallFailureMapper,
) -> OutputCommand {
    match request {
        ClientRequest::Call { id, method, params } => {
            let result = async { dbus.get().await?.call(&method, params).await }
                .await
                .map_err(|error| call_failure(&method, &error))
                .or_else(|failure| match failure {
                    CallFailure::Api(response) => Ok(response),
                    CallFailure::Transport(error) => Err(error),
                });
            response_command(id, result, None)
        }
        ClientRequest::Subscribe { id, streams } => {
            let result = async { dbus.get().await?.subscribe(streams).await }
                .await
                .map_err(|error| error.to_string());
            response_command(id, result, None)
        }
        ClientRequest::Cancel { id, request_id } => {
            let result = async {
                let client = dbus.get().await?;
                cancel(&client, &request_id, cancel_mode).await
            }
            .await
            .map_err(|error| error.to_string());
            let cancelled = result.as_ref().ok().map(|_| request_id);
            response_command(id, result, cancelled)
        }
        ClientRequest::Shutdown { id } => OutputCommand::Shutdown(id),
    }
}

fn response_command(
    id: String,
    result: std::result::Result<Value, String>,
    cancelled_request_id: Option<String>,
) -> OutputCommand {
    OutputCommand::Response {
        id,
        result,
        cancelled_request_id,
    }
}

async fn cancel(dbus: &JsonDbusClient, request_id: &str, mode: CancelMode) -> Result<Value> {
    match mode {
        CancelMode::Json => dbus.cancel_json(request_id).await,
        CancelMode::Unit => dbus.cancel_unit(request_id).await,
    }
}

fn spawn_event_forwarder(
    dbus: ReconnectingClient,
    output: OutputHandle,
    ready: oneshot::Sender<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut delay = INITIAL_RECONNECT_DELAY;
        let mut last_error = None;
        let mut ready = Some(ready);
        loop {
            let message = event_forwarding_error(&dbus, &output, &mut ready).await;
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

async fn event_forwarding_error(
    dbus: &ReconnectingClient,
    output: &OutputHandle,
    ready: &mut Option<oneshot::Sender<()>>,
) -> String {
    let result = async { dbus.get().await?.forward_events(output, ready).await }.await;
    result.map_or_else(
        |error| error.to_string(),
        |()| "daemon event forwarding stopped".to_owned(),
    )
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
    tokio::spawn(async move {
        loop {
            let result = async { dbus.get().await?.watch_replacement().await }.await;
            match result {
                Ok(()) => {
                    if output.send(OutputCommand::ResetCorrelation).await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "daemon owner watcher stopped");
                    tokio::time::sleep(INITIAL_RECONNECT_DELAY).await;
                }
            }
        }
    })
}

async fn wait_for_request_slot(calls: &mut JoinSet<()>) {
    while calls.len() >= MAX_IN_FLIGHT_REQUESTS {
        if let Some(result) = calls.join_next().await {
            log_join_result(result);
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

async fn drain_calls(calls: &mut JoinSet<()>) {
    if tokio::time::timeout(SHUTDOWN_TIMEOUT, async {
        while let Some(result) = calls.join_next().await {
            log_join_result(result);
        }
    })
    .await
    .is_err()
    {
        calls.abort_all();
        while calls.join_next().await.is_some() {}
    }
}

async fn cancel_active(dbus: &ReconnectingClient, output: &OutputHandle, mode: CancelMode) {
    let Ok(client) = dbus.get().await else {
        return;
    };
    for id in output.active_ids().await {
        let _ = cancel(&client, &id, mode).await;
    }
}
