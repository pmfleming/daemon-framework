use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use shelllist_daemon_core::{
    ClientRoute, addressed_message, event_message, protocol_error_message, response_error_message,
    response_message, shutdown_message, transport_error_message,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackedKind {
    Operation,
    Subscription,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedId {
    pub id: String,
    pub kind: TrackedKind,
}

pub trait CorrelationPolicy: Send + Sync + 'static {
    /// Return only operation IDs that domain policy considers trackable.
    /// Borrowing keeps allocation in the common response path, after selection.
    fn operation_id<'a>(&self, _response: &'a Value) -> Option<&'a str> {
        None
    }

    /// Operations take precedence over the standard subscription envelope.
    /// Override this for protocols with a different subscription shape.
    fn response_id(&self, response: &Value) -> Option<TrackedId> {
        let (id, kind) = match self.operation_id(response) {
            Some(id) => (id, TrackedKind::Operation),
            None => (
                response.pointer("/data/subscription/id")?.as_str()?,
                TrackedKind::Subscription,
            ),
        };
        Some(TrackedId {
            id: id.to_owned(),
            kind,
        })
    }

    fn event_id(&self, _stream: &str, event: &Value) -> Option<String> {
        event
            .get("subscription_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    fn is_terminal(&self, _stream: &str, _event: &Value) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BasicCorrelation;

impl CorrelationPolicy for BasicCorrelation {}

pub(crate) enum OutputCommand {
    Response {
        id: String,
        result: std::result::Result<Value, String>,
        cancelled_request_id: Option<String>,
        route: Option<ClientRoute>,
    },
    ActiveIds {
        owner: Option<(String, u64)>,
        reply: oneshot::Sender<Vec<String>>,
    },
    Cancelled(String),
    Event {
        stream: String,
        event: Value,
    },
    ProtocolError(String),
    TransportError(String),
    ResetCorrelation,
    Shutdown(String),
}

#[derive(Clone)]
pub(crate) struct OutputHandle {
    priority_sender: mpsc::Sender<OutputCommand>,
    event_sender: mpsc::Sender<OutputCommand>,
}

impl OutputHandle {
    pub(crate) async fn send(&self, command: OutputCommand) -> Result<()> {
        let sender = if matches!(command, OutputCommand::Event { .. }) {
            &self.event_sender
        } else {
            &self.priority_sender
        };
        sender
            .send(command)
            .await
            .context("send daemon output command")
    }

    pub(crate) async fn owned_ids(&self, route: &ClientRoute) -> Vec<String> {
        self.query_ids(Some((route.consumer_id.clone(), route.generation)))
            .await
    }

    pub(crate) async fn active_ids(&self) -> Vec<String> {
        self.query_ids(None).await
    }

    async fn query_ids(&self, owner: Option<(String, u64)>) -> Vec<String> {
        let (reply, response) = oneshot::channel();
        if self
            .send(OutputCommand::ActiveIds { owner, reply })
            .await
            .is_err()
        {
            return Vec::new();
        }
        response.await.unwrap_or_default()
    }
}

struct OutputState<P> {
    policy: P,
    // One entry per live operation/subscription; only subscriptions retain a route.
    // Ordinary request addresses live with their task/response, not this map.
    active_ids: HashMap<String, Option<ClientRoute>>,
    pending_events: VecDeque<(String, String, Value)>,
    suppressed_ids: HashSet<String>,
    suppressed_order: VecDeque<String>,
    pending_limit: usize,
}

impl<P: CorrelationPolicy> OutputState<P> {
    fn new(policy: P, pending_limit: usize) -> Self {
        Self {
            policy,
            active_ids: HashMap::new(),
            pending_events: VecDeque::new(),
            suppressed_ids: HashSet::new(),
            suppressed_order: VecDeque::new(),
            pending_limit,
        }
    }

    fn activate(
        &mut self,
        tracked: Option<TrackedId>,
        route: Option<ClientRoute>,
    ) -> Vec<(String, Value)> {
        let Some(tracked) = tracked else {
            return Vec::new();
        };
        if self.suppressed_ids.contains(&tracked.id) {
            return Vec::new();
        }
        let pending = self.take_pending(&tracked.id);
        let active_route = self.active_ids.entry(tracked.id).or_default();
        // A repeated response must not discard an already acknowledged route.
        if let Some(route) = route.filter(|_| tracked.kind == TrackedKind::Subscription) {
            *active_route = Some(route);
        }
        pending
    }

    fn accept_event(&mut self, stream: String, event: Value) -> Option<Value> {
        match self.policy.event_id(&stream, &event) {
            Some(id) if self.suppressed_ids.contains(&id) => None,
            Some(id) if !self.active_ids.contains_key(&id) => {
                self.buffer(id, stream, event);
                None
            }
            _ => Some(self.event_message(&stream, event)),
        }
    }

    fn buffer(&mut self, id: String, stream: String, event: Value) {
        if self.pending_limit == 0 {
            return;
        }
        if self.pending_events.len() >= self.pending_limit {
            self.pending_events.pop_front();
        }
        self.pending_events.push_back((id, stream, event));
    }

    fn take_pending(&mut self, id: &str) -> Vec<(String, Value)> {
        let (matching, retained) = std::mem::take(&mut self.pending_events)
            .into_iter()
            .partition(|(event_id, _, _)| event_id == id);
        self.pending_events = retained;
        matching
            .into_iter()
            .map(|(_, stream, event)| (stream, event))
            .collect()
    }

    fn suppress(&mut self, id: String) {
        if self.pending_limit == 0 || !self.suppressed_ids.insert(id.clone()) {
            return;
        }
        if self.suppressed_order.len() >= self.pending_limit
            && let Some(oldest) = self.suppressed_order.pop_front()
        {
            self.suppressed_ids.remove(&oldest);
        }
        self.suppressed_order.push_back(id);
    }

    fn cancelled(&mut self, id: String) {
        self.active_ids.remove(&id);
        self.pending_events
            .retain(|(event_id, _, _)| event_id != &id);
        self.suppress(id);
    }

    fn active_ids(&self) -> Vec<String> {
        let mut ids = self.active_ids.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        ids
    }

    fn owned_ids(&self, consumer_id: &str, generation: u64) -> Vec<String> {
        self.active_ids
            .iter()
            .filter(|(_, route)| {
                route.as_ref().is_some_and(|route| {
                    route.consumer_id == consumer_id && route.generation == generation
                })
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn event_message(&mut self, stream: &str, event: Value) -> Value {
        let terminal = self
            .policy
            .is_terminal(stream, &event)
            .then(|| self.policy.event_id(stream, &event))
            .flatten();
        // Subscription ownership is independent of domain operation correlation.
        let route = event
            .get("subscription_id")
            .and_then(Value::as_str)
            .and_then(|id| self.active_ids.get(id)?.as_ref());
        let message = addressed_message(event_message(stream, event), route);
        if let Some(id) = terminal {
            self.cancelled(id);
        }
        message
    }

    fn reset_correlation(&mut self) {
        self.active_ids.clear();
        self.pending_events.clear();
        self.suppressed_ids.clear();
        self.suppressed_order.clear();
    }
}

#[must_use]
pub(crate) fn spawn_output_actor_with_writer<P, W>(
    policy: P,
    capacity: usize,
    pending_limit: usize,
    writer: W,
) -> (OutputHandle, JoinHandle<Result<()>>)
where
    P: CorrelationPolicy,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (priority_sender, priority_receiver) = mpsc::channel(capacity);
    let (event_sender, event_receiver) = mpsc::channel(capacity);
    let handle = OutputHandle {
        priority_sender,
        event_sender,
    };
    let task = tokio::spawn(run_output_actor(
        priority_receiver,
        event_receiver,
        writer,
        OutputState::new(policy, pending_limit),
    ));
    (handle, task)
}

async fn run_output_actor<P, W>(
    mut priority_commands: mpsc::Receiver<OutputCommand>,
    mut events: mpsc::Receiver<OutputCommand>,
    mut writer: W,
    mut state: OutputState<P>,
) -> Result<()>
where
    P: CorrelationPolicy,
    W: AsyncWrite + Unpin,
{
    loop {
        let command = tokio::select! {
            biased;
            Some(command) = priority_commands.recv() => command,
            Some(command) = events.recv() => command,
            else => return Ok(()),
        };
        emit_command(&mut writer, &mut state, command).await?;
    }
}

async fn emit_command<P, W>(
    writer: &mut W,
    state: &mut OutputState<P>,
    command: OutputCommand,
) -> Result<()>
where
    P: CorrelationPolicy,
    W: AsyncWrite + Unpin,
{
    let message = match command {
        OutputCommand::Response {
            id,
            result,
            cancelled_request_id,
            route,
        } => return emit_response(writer, state, id, result, cancelled_request_id, route).await,
        OutputCommand::ActiveIds { owner, reply } => {
            let ids = owner.as_ref().map_or_else(
                || state.active_ids(),
                |(consumer, generation)| state.owned_ids(consumer, *generation),
            );
            let _ = reply.send(ids);
            None
        }
        OutputCommand::Cancelled(id) => {
            state.cancelled(id);
            None
        }
        OutputCommand::Event { stream, event } => state.accept_event(stream, event),
        OutputCommand::ProtocolError(error) => Some(protocol_error_message(error)),
        OutputCommand::TransportError(error) => Some(transport_error_message(error)),
        OutputCommand::ResetCorrelation => {
            state.reset_correlation();
            None
        }
        OutputCommand::Shutdown(id) => Some(shutdown_message(&id)),
    };
    if let Some(message) = message {
        emit_line(writer, &message).await?;
    }
    Ok(())
}

async fn emit_response<P, W>(
    writer: &mut W,
    state: &mut OutputState<P>,
    id: String,
    result: std::result::Result<Value, String>,
    cancelled_request_id: Option<String>,
    route: Option<ClientRoute>,
) -> Result<()>
where
    P: CorrelationPolicy,
    W: AsyncWrite + Unpin,
{
    if let Some(cancelled) = cancelled_request_id {
        state.cancelled(cancelled);
    }
    let (line, tracked) = match result {
        Ok(response) => {
            let tracked = state.policy.response_id(&response);
            (response_message(&id, response), tracked)
        }
        Err(error) => (response_error_message(&id, error), None),
    };
    let line = addressed_message(line, route.as_ref());
    let pending = state.activate(tracked, route);
    emit_line(writer, &line).await?;
    for message in pending
        .into_iter()
        .filter_map(|(stream, event)| state.accept_event(stream, event))
    {
        emit_line(writer, &message).await?;
    }
    Ok(())
}

async fn emit_line<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value).context("serialize daemon JSON line")?;
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .await
        .context("write daemon JSON line")?;
    writer.flush().await.context("flush daemon JSON line")
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod routing_tests;

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, duplex};
    use tokio::sync::mpsc;

    use super::{
        BasicCorrelation, CorrelationPolicy, OutputCommand, OutputState, TrackedKind,
        run_output_actor, spawn_output_actor_with_writer,
    };

    struct Operations;
    impl CorrelationPolicy for Operations {
        fn operation_id<'a>(&self, response: &'a Value) -> Option<&'a str> {
            response.get("operation")?.as_str()
        }
    }

    #[test]
    fn response_defaults_prioritize_operations_and_fall_back_on_invalid_ids() {
        let mut response = json!({"operation": "op", "data": {"subscription": {"id": "sub"}}});
        let operation = Operations.response_id(&response).unwrap();
        assert_eq!(
            (operation.id.as_str(), operation.kind),
            ("op", TrackedKind::Operation)
        );
        for invalid in [Value::Null, json!(42), json!({})] {
            response["operation"] = invalid;
            assert_eq!(
                Operations.response_id(&response),
                BasicCorrelation.response_id(&response)
            );
            assert_eq!(Operations.response_id(&response).unwrap().id, "sub");
        }
        assert!(Operations.response_id(&json!({})).is_none());
        assert!(
            BasicCorrelation
                .response_id(&json!({"data": {"subscription": {"id": 42}}}))
                .is_none()
        );
    }

    async fn render(commands: Vec<OutputCommand>) -> Result<Vec<Value>> {
        let (writer, mut reader) = duplex(4096);
        let (output, task) = spawn_output_actor_with_writer(BasicCorrelation, 8, 4, writer);
        for command in commands {
            output.send(command).await?;
        }
        drop(output);
        task.await??;

        let mut text = String::new();
        reader.read_to_string(&mut text).await?;
        text.lines()
            .map(serde_json::from_str)
            .collect::<serde_json::Result<_>>()
            .map_err(Into::into)
    }

    #[tokio::test]
    async fn responses_overtake_queued_events() -> Result<()> {
        let (priority_sender, priority_receiver) = mpsc::channel(4);
        let (event_sender, event_receiver) = mpsc::channel(4);
        event_sender
            .send(OutputCommand::Event {
                stream: "things.changed".into(),
                event: json!({ "sequence": 1 }),
            })
            .await?;
        priority_sender
            .send(OutputCommand::Response {
                id: "status".into(),
                result: Ok(json!({ "ok": true })),
                cancelled_request_id: None,
                route: None,
            })
            .await?;
        drop(priority_sender);
        drop(event_sender);

        let (writer, mut reader) = duplex(4096);
        run_output_actor(
            priority_receiver,
            event_receiver,
            writer,
            OutputState::new(BasicCorrelation, 4),
        )
        .await?;
        let mut text = String::new();
        reader.read_to_string(&mut text).await?;
        let lines = text
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<serde_json::Result<Vec<_>>>()?;

        assert_eq!(lines[0]["kind"], "response");
        assert_eq!(lines[1]["kind"], "event");
        Ok(())
    }

    #[tokio::test]
    async fn drops_late_events_after_cancellation() -> Result<()> {
        let lines = render(vec![
            OutputCommand::Response {
                id: "subscribe".into(),
                result: Ok(json!({ "data": { "subscription": { "id": "sub-1" } } })),
                cancelled_request_id: None,
                route: None,
            },
            OutputCommand::Response {
                id: "cancel".into(),
                result: Ok(json!({ "cancelled": "sub-1" })),
                cancelled_request_id: Some("sub-1".into()),
                route: None,
            },
            OutputCommand::Event {
                stream: "things.changed".into(),
                event: json!({ "subscription_id": "sub-1" }),
            },
        ])
        .await?;

        assert_eq!(lines.len(), 2);
        Ok(())
    }
}
