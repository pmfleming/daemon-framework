use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use shelllist_daemon_core::{ClientRequest, DaemonEndpoint};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::task::{JoinHandle, JoinSet};

use crate::{CorrelationPolicy, JsonDbusClient, OutputCommand, OutputHandle, spawn_output_actor};

const OUTPUT_CAPACITY: usize = 64;
const PENDING_EVENT_LIMIT: usize = 32;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelMode {
    Json,
    Unit,
}

pub type CallFailureMapper = fn(&str, &anyhow::Error) -> Value;

pub struct JsonlClientConfig<P> {
    pub endpoint: DaemonEndpoint,
    pub correlation: P,
    pub cancel_mode: CancelMode,
    pub call_failure: CallFailureMapper,
}

pub async fn run_jsonl_client<P: CorrelationPolicy>(config: JsonlClientConfig<P>) -> Result<()> {
    let dbus = JsonDbusClient::session(config.endpoint).await.ok();
    let (output, output_task) =
        spawn_output_actor(config.correlation, OUTPUT_CAPACITY, PENDING_EVENT_LIMIT);
    let event_task = dbus
        .as_ref()
        .map(|client| spawn_event_forwarder(client.clone(), output.clone()));
    let owner_task = dbus
        .as_ref()
        .map(|client| spawn_owner_watcher(client.clone(), output.clone()));

    let mut calls = JoinSet::new();
    let shutdown_id = request_loop(
        dbus.as_ref(),
        &output,
        &mut calls,
        config.cancel_mode,
        config.call_failure,
    )
    .await?;
    drain_calls(&mut calls).await;
    cancel_active(dbus.as_ref(), &output, config.cancel_mode).await;

    if let Some(task) = event_task {
        task.abort();
    }
    if let Some(task) = owner_task {
        task.abort();
    }
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
    dbus: Option<&JsonDbusClient>,
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
        spawn_request(
            calls,
            dbus.cloned(),
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
    dbus: Option<JsonDbusClient>,
    output: OutputHandle,
    request: ClientRequest,
    cancel_mode: CancelMode,
    call_failure: CallFailureMapper,
) {
    calls.spawn(async move {
        let (id, result, cancelled_request_id) = match request {
            ClientRequest::Call { id, method, params } => {
                let response = match dbus.as_ref() {
                    Some(dbus) => dbus.call(&method, params).await,
                    None => Err(anyhow!("session D-Bus unavailable")),
                }
                .unwrap_or_else(|error| call_failure(&method, &error));
                (id, Ok(response), None)
            }
            ClientRequest::Subscribe { id, streams } => {
                let result = match dbus.as_ref() {
                    Some(dbus) => dbus.subscribe(streams).await,
                    None => Err(anyhow!("session D-Bus unavailable")),
                }
                .map_err(|error| error.to_string());
                (id, result, None)
            }
            ClientRequest::Cancel { id, request_id } => {
                let result = match dbus.as_ref() {
                    Some(dbus) => cancel(dbus, &request_id, cancel_mode).await,
                    None => Err(anyhow!("session D-Bus unavailable")),
                }
                .map_err(|error| error.to_string());
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

fn spawn_event_forwarder(dbus: JsonDbusClient, output: OutputHandle) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(error) = dbus.forward_events(&output).await {
            let _ = output
                .send(OutputCommand::TransportError(error.to_string()))
                .await;
        }
    })
}

fn spawn_owner_watcher(dbus: JsonDbusClient, output: OutputHandle) -> JoinHandle<()> {
    tokio::spawn(async move {
        match dbus.watch_replacement().await {
            Ok(()) => {
                let _ = output
                    .send(OutputCommand::TransportError(
                        "daemon restarted; reconnecting".into(),
                    ))
                    .await;
            }
            Err(error) => {
                let _ = output
                    .send(OutputCommand::TransportError(error.to_string()))
                    .await;
            }
        }
    })
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

async fn cancel_active(dbus: Option<&JsonDbusClient>, output: &OutputHandle, mode: CancelMode) {
    let Some(dbus) = dbus else {
        return;
    };
    for id in output.active_ids().await {
        let _ = cancel(dbus, &id, mode).await;
    }
}
