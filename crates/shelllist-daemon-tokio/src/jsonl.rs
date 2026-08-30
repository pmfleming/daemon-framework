use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;
use shelllist_daemon_core::{ClientRequest, DaemonEndpoint};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
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
    let event_task = spawn_event_forwarder(dbus.clone(), output.clone());

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
        let (id, result, cancelled_request_id) = match request {
            ClientRequest::Call { id, method, params } => {
                let result = match dbus.get().await {
                    Ok(client) => client.call(&method, params).await,
                    Err(error) => Err(error),
                };
                let result = result
                    .map_err(|error| call_failure(&method, &error))
                    .or_else(|failure| match failure {
                        CallFailure::Api(response) => Ok(response),
                        CallFailure::Transport(error) => Err(error),
                    });
                (id, result, None)
            }
            ClientRequest::Subscribe { id, streams } => {
                let result = match dbus.get().await {
                    Ok(client) => client.subscribe(streams).await,
                    Err(error) => Err(error),
                };
                (id, result.map_err(|error| error.to_string()), None)
            }
            ClientRequest::Cancel { id, request_id } => {
                let result = match dbus.get().await {
                    Ok(client) => cancel(&client, &request_id, cancel_mode).await,
                    Err(error) => Err(error),
                };
                let result = result.map_err(|error| error.to_string());
                let cancelled = result.as_ref().ok().map(|_| request_id);
                (id, result, cancelled)
            }
            ClientRequest::Shutdown { .. } => unreachable!("shutdown is handled by request loop"),
        };
        let _ = output
            .send(OutputCommand::Response {
                id,
                result,
                cancelled_request_id,
            })
            .await;
    });
}

async fn cancel(dbus: &JsonDbusClient, request_id: &str, mode: CancelMode) -> Result<Value> {
    match mode {
        CancelMode::Json => dbus.cancel_json(request_id).await,
        CancelMode::Unit => dbus.cancel_unit(request_id).await,
    }
}

fn spawn_event_forwarder(dbus: ReconnectingClient, output: OutputHandle) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut delay = INITIAL_RECONNECT_DELAY;
        let mut last_error = None;
        loop {
            let result = match dbus.get().await {
                Ok(client) => client.forward_events(&output).await,
                Err(error) => Err(error),
            };
            let message = match result {
                Ok(()) => "daemon event forwarding stopped".to_owned(),
                Err(error) => error.to_string(),
            };
            if last_error.as_deref() != Some(message.as_str()) {
                if output
                    .send(OutputCommand::TransportError(message.clone()))
                    .await
                    .is_err()
                {
                    return;
                }
                last_error = Some(message);
            }
            dbus.invalidate().await;
            tokio::time::sleep(delay).await;
            delay = delay.saturating_mul(2).min(MAX_RECONNECT_DELAY);
        }
    })
}

async fn wait_for_request_slot(calls: &mut JoinSet<()>) {
    while calls.len() >= MAX_IN_FLIGHT_REQUESTS {
        if let Some(result) = calls.join_next().await
            && let Err(error) = result
        {
            tracing_log_join(error);
        }
    }
}

fn reap_finished(calls: &mut JoinSet<()>) {
    while let Some(result) = calls.try_join_next() {
        if let Err(error) = result {
            tracing_log_join(error);
        }
    }
}

fn tracing_log_join(error: tokio::task::JoinError) {
    tracing::warn!(%error, "daemon JSONL call task failed");
}

async fn drain_calls(calls: &mut JoinSet<()>) {
    if tokio::time::timeout(SHUTDOWN_TIMEOUT, async {
        while let Some(result) = calls.join_next().await {
            if let Err(error) = result {
                tracing_log_join(error);
            }
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
