//! Same-user JSONL ingress. Domain dispatch and admission stay in the daemon.
use crate::{
    CorrelationPolicy,
    jsonl::{
        RequestSlots, cancellation_succeeded,
        input::{Frame, Frames},
        parse_request, response_command,
    },
    output_actor::{OutputCommand, OutputHandle, spawn_output_actor_with_writer},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use shelllist_daemon_core::{
    ClientMessage, ClientRequest, DEFAULT_JSONL_REQUEST_BYTES, MAX_JSONL_REQUEST_BYTES,
};
use std::{
    future::Future,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::BufReader,
    net::UnixStream,
    sync::{Semaphore, watch},
    task::JoinSet,
};

mod path;
mod writer;
pub use path::JsonlListener;

/// Implement using the SAME service boundary/coordinators as the D-Bus interface.
/// A connection is a distinct server-minted owner; never derive ownership from JSON.
/// `disconnected` must be idempotent and owner-scoped. It is also called after late
/// accepted work finishes, so a lost acknowledgement cannot leak an operation.
/// Do not use D-Bus owner monitoring for these non-D-Bus identities.
pub trait LocalJsonlService: Send + Sync + 'static {
    fn call(
        &self,
        session: &SocketSession,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value>> + Send;
    fn subscribe(
        &self,
        session: &SocketSession,
        streams: Vec<String>,
    ) -> impl Future<Output = Result<Value>> + Send;
    fn cancel(
        &self,
        session: &SocketSession,
        id: &str,
    ) -> impl Future<Output = Result<Value>> + Send;
    fn disconnected(&self, session: &SocketSession) -> impl Future<Output = ()> + Send;
}

#[derive(Clone)]
pub struct SocketSession {
    owner: Arc<str>,
    output: OutputHandle,
    closed: watch::Sender<bool>,
}
impl SocketSession {
    pub fn owner(&self) -> &str {
        &self.owner
    }
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }
    /// Register subscription workers with OwnedTaskRegistry::spawn_until and this
    /// future. It also resolves when closure happened before the worker started.
    pub async fn closed(&self) {
        let _ = self.closed.subscribe().wait_for(|closed| *closed).await;
    }
    pub async fn emit(&self, stream: impl Into<String>, event: Value) -> Result<()> {
        ensure!(!self.is_closed(), "socket session closed");
        self.output
            .send(OutputCommand::Event {
                stream: stream.into(),
                event,
            })
            .await
    }
    /// Synchronous producers must not block a runtime worker on backpressure.
    /// Fail closed rather than silently drop an event or grow an unbounded queue.
    pub fn try_emit(&self, stream: impl Into<String>, event: Value) -> Result<()> {
        ensure!(!self.is_closed(), "socket session closed");
        let result = self.output.try_event(stream.into(), event);
        if result.is_err() {
            self.close();
        }
        result
    }
    fn close(&self) {
        self.closed.send_replace(true);
    }
}

#[derive(Clone, Debug)]
pub struct JsonlSocketConfig {
    pub max_connections: usize,
    pub max_in_flight_requests: usize,
    pub maximum_request_bytes: usize,
    pub maximum_response_bytes: usize,
    pub output_capacity: usize,
    pub pending_event_limit: usize,
    pub request_timeout: Duration,
    pub write_timeout: Duration,
}
impl Default for JsonlSocketConfig {
    fn default() -> Self {
        Self {
            max_connections: 8,
            max_in_flight_requests: 64,
            maximum_request_bytes: DEFAULT_JSONL_REQUEST_BYTES,
            maximum_response_bytes: 32 * 1024 * 1024,
            output_capacity: 16,
            pending_event_limit: 32,
            request_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(5),
        }
    }
}
impl JsonlSocketConfig {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.max_connections > 0 && self.max_in_flight_requests > 0 && self.output_capacity > 0,
            "socket capacities must be positive"
        );
        ensure!(
            (1..=MAX_JSONL_REQUEST_BYTES).contains(&self.maximum_request_bytes),
            "invalid request byte bound"
        );
        ensure!(
            (1..=64 * 1024 * 1024).contains(&self.maximum_response_bytes),
            "invalid response byte bound"
        );
        ensure!(
            !self.request_timeout.is_zero() && !self.write_timeout.is_zero(),
            "socket deadlines must be positive"
        );
        Ok(())
    }
}

impl JsonlListener {
    /// Bind only below an existing, private XDG_RUNTIME_DIR (or isolated test
    /// equivalent). Never chmod a caller's directory or unlink a live listener.
    pub async fn bind(runtime: &Path, name: &str) -> Result<Self> {
        path::bind(runtime, name).await
    }

    pub async fn serve<S, P>(
        self,
        service: Arc<S>,
        policy: P,
        config: JsonlSocketConfig,
        shutdown: impl Future<Output = ()>,
    ) -> Result<()>
    where
        S: LocalJsonlService,
        P: CorrelationPolicy + Clone,
    {
        config.validate()?;
        let capacity = Arc::new(Semaphore::new(config.max_connections));
        let (stopping, _) = watch::channel(false);
        let mut connections = JoinSet::new();
        tokio::pin!(shutdown);
        let result = loop {
            tokio::select! {
                biased;
                () = &mut shutdown => break Ok(()),
                Some(result) = connections.join_next(), if !connections.is_empty() => {
                    if let Err(error) = result { tracing::warn!(%error, "socket connection task failed"); }
                }
                accepted = self.listener.accept() => {
                    let (stream, _) = match accepted { Ok(pair) => pair, Err(error) => break Err(error.into()) };
                    // Check credentials before reading a byte, even in a private directory.
                    if stream.peer_cred().ok().map(|c| c.uid()) != Some(self.uid) { continue; }
                    let Ok(permit) = capacity.clone().try_acquire_owned() else { continue; };
                    let (service, policy, config, stop) = (service.clone(), policy.clone(), config.clone(), stopping.subscribe());
                    connections.spawn(async move {
                        let _permit = permit;
                        if let Err(error) = connection(stream, service, policy, config, stop).await {
                            tracing::debug!(%error, "JSONL socket disconnected");
                        }
                    });
                }
            }
        };
        stopping.send_replace(true);
        while connections.join_next().await.is_some() {}
        result
    }
}

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

fn new_owner() -> Result<Arc<str>> {
    // Persistent domain claims must not match a new connection after PID reuse.
    // Kernel entropy only; unavailable entropy rejects ingress, never falls back
    // to timestamps/PIDs. NONBLOCK also protects a single-worker executor.
    let mut bytes = [0u8; 16];
    let mut filled = 0;
    while filled < bytes.len() {
        let count =
            rustix::rand::getrandom(&mut bytes[filled..], rustix::rand::GetRandomFlags::NONBLOCK)
                .context("mint socket owner identity")?;
        ensure!(count > 0, "kernel returned no socket owner entropy");
        filled += count;
    }
    Ok(format!(
        "unix:{:032x}:{}",
        u128::from_le_bytes(bytes),
        NEXT_OWNER.fetch_add(1, Ordering::Relaxed)
    )
    .into())
}

struct DisconnectGuard<S: LocalJsonlService> {
    session: Option<SocketSession>,
    service: Arc<S>,
}
impl<S: LocalJsonlService> Drop for DisconnectGuard<S> {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            session.close();
            let service = self.service.clone();
            tokio::spawn(async move {
                service.disconnected(&session).await;
            });
        }
    }
}

async fn connection<S: LocalJsonlService, P: CorrelationPolicy>(
    stream: UnixStream,
    service: Arc<S>,
    policy: P,
    config: JsonlSocketConfig,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let writer =
        writer::DeadlineWriter::new(writer, config.maximum_response_bytes, config.write_timeout);
    let (output, mut output_task) = spawn_output_actor_with_writer(
        policy,
        config.output_capacity,
        config.pending_event_limit,
        writer,
    );
    let _writer_guard = crate::AbortOnDrop(output_task.abort_handle());
    let (closed, _) = watch::channel(false);
    let session = SocketSession {
        owner: new_owner()?,
        output,
        closed,
    };
    let mut guard = DisconnectGuard {
        session: Some(session.clone()),
        service: service.clone(),
    };
    let mut calls = JoinSet::new();
    let slots = RequestSlots::new(config.max_in_flight_requests);
    let mut frames = Frames::new(BufReader::new(reader), config.maximum_request_bytes);
    let reading = async {
        while let Some(frame) = frames.next().await? {
            match frame {
                Frame::TooLarge => {
                    session
                        .output
                        .send(OutputCommand::ProtocolError(format!(
                            "JSONL request exceeds {} bytes",
                            config.maximum_request_bytes
                        )))
                        .await?
                }
                Frame::Line(line) => {
                    if let Some(message) = parse_request(&line, &session.output).await? {
                        if let ClientRequest::Shutdown { id } = message.request {
                            while calls.join_next().await.is_some() {}
                            session.output.send(OutputCommand::Shutdown(id)).await?;
                            let _ = session.output.active_ids().await; // ordered writer barrier
                            return Ok::<_, anyhow::Error>(());
                        }
                        let id = request_id(&message.request).to_owned();
                        match slots.acquire(&message.request) {
                        Ok(permit) => {
                            let (service, session, budget) = (service.clone(), session.clone(), config.request_timeout);
                            calls.spawn(async move {
                                let ClientMessage { request, route } = message;
                                let control = !matches!(request, ClientRequest::Call { .. });
                                let work_service = service.clone(); let work_session = session.clone();
                                let work_route = route.clone();
                                // Keep accepted work AND its permit alive on lost reply/deadline.
                                // Dropping a JoinHandle detaches; it does not claim effects rolled back.
                                let mut work = tokio::spawn(async move {
                                    let _permit = permit;
                                    let result = crate::catch_task("socket request", dispatch(work_service.as_ref(), &work_session, request, work_route)).await;
                                    if work_session.is_closed() { work_service.disconnected(&work_session).await; }
                                    result
                                });
                                let command = match tokio::time::timeout(budget, &mut work).await {
                                    Ok(Ok(Ok((value, cancelled)))) => response_command(id, Ok(value), cancelled, route),
                                    _ => {
                                        if control { session.close(); }
                                        OutputCommand::RequestError { id, route, outcome: "outcome-unknown", message: "socket request reply unavailable; reconcile state, never replay effects".into() }
                                    },
                                };
                                let _ = session.output.send(command).await;
                            });
                        }
                        Err(_) => session.output.send(OutputCommand::RequestError { id, route: message.route, outcome: "rejected-before-dispatch", message: "socket request capacity exceeded; request was not dispatched".into() }).await?,
                    }
                    }
                }
            }
            while calls.try_join_next().is_some() {}
        }
        Ok(())
    };
    let result = tokio::select! {
        result = reading => result,
        result = &mut output_task => result.context("join socket writer").and_then(|result| result),
        _ = stop.wait_for(|stop| *stop) => Ok(()),
        () = session.closed() => Ok(()),
    };
    // Close the lifetime before cleanup. Abort reply waiters, not accepted work.
    session.close();
    calls.shutdown().await;
    service.disconnected(&session).await;
    guard.session = None;
    output_task.abort();
    if !output_task.is_finished() {
        let _ = output_task.await;
    }
    result
}

fn request_id(request: &ClientRequest) -> &str {
    match request {
        ClientRequest::Call { id, .. }
        | ClientRequest::Subscribe { id, .. }
        | ClientRequest::Cancel { id, .. }
        | ClientRequest::Release { id }
        | ClientRequest::Shutdown { id } => id,
    }
}
async fn dispatch<S: LocalJsonlService>(
    service: &S,
    session: &SocketSession,
    request: ClientRequest,
    route: Option<shelllist_daemon_core::ClientRoute>,
) -> Result<(Value, Option<String>)> {
    ensure!(!session.is_closed(), "connection closed before dispatch");
    let value = match request {
        ClientRequest::Call { method, params, .. } => {
            service.call(session, &method, params).await?
        }
        ClientRequest::Subscribe { streams, .. } => service.subscribe(session, streams).await?,
        ClientRequest::Cancel { request_id, .. } => {
            let response = service.cancel(session, &request_id).await?;
            let cancelled = cancellation_succeeded(&response).then_some(request_id);
            return Ok((response, cancelled));
        }
        ClientRequest::Release { .. } => {
            let route = route.context("release requires routed dispatch")?;
            let ids = session.output.owned_ids(&route).await;
            let released = ids.len();
            for id in ids {
                let response = service.cancel(session, &id).await?;
                ensure!(
                    cancellation_succeeded(&response),
                    "daemon rejected cancellation"
                );
                session.output.send(OutputCommand::Cancelled(id)).await?;
            }
            json!({ "released": released })
        }
        ClientRequest::Shutdown { .. } => json!({}),
    };
    Ok((value, None))
}

#[cfg(test)]
mod tests;
