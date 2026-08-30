use std::collections::{HashSet, VecDeque};

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use shelllist_daemon_core::{
    event_message, protocol_error_message, response_error_message, response_message,
    shutdown_message, transport_error_message,
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
    fn response_id(&self, response: &Value) -> Option<TrackedId>;
    fn event_id(&self, stream: &str, event: &Value) -> Option<String>;
    fn is_terminal(&self, stream: &str, event: &Value) -> bool;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BasicCorrelation;

impl CorrelationPolicy for BasicCorrelation {
    fn response_id(&self, response: &Value) -> Option<TrackedId> {
        response
            .pointer("/data/subscription/id")
            .and_then(Value::as_str)
            .map(|id| TrackedId {
                id: id.to_string(),
                kind: TrackedKind::Subscription,
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

pub enum OutputCommand {
    Response {
        id: String,
        result: std::result::Result<Value, String>,
        cancelled_request_id: Option<String>,
    },
    Event {
        stream: String,
        event: Value,
    },
    ProtocolError(String),
    TransportError(String),
    ActiveIds(oneshot::Sender<Vec<String>>),
    Shutdown(String),
}

#[derive(Clone)]
pub struct OutputHandle {
    sender: mpsc::Sender<OutputCommand>,
}

impl OutputHandle {
    pub async fn send(&self, command: OutputCommand) -> Result<()> {
        self.sender
            .send(command)
            .await
            .context("send daemon output command")
    }

    pub async fn active_ids(&self) -> Vec<String> {
        let (reply, response) = oneshot::channel();
        if self.send(OutputCommand::ActiveIds(reply)).await.is_err() {
            return Vec::new();
        }
        response.await.unwrap_or_default()
    }
}

enum EventDisposition {
    Emit,
    Buffer(String),
    Drop,
}

struct OutputState<P> {
    policy: P,
    active_ids: HashSet<String>,
    pending_events: VecDeque<(String, String, Value)>,
    suppressed_ids: HashSet<String>,
    suppressed_order: VecDeque<String>,
    pending_limit: usize,
}

impl<P: CorrelationPolicy> OutputState<P> {
    fn new(policy: P, pending_limit: usize) -> Self {
        Self {
            policy,
            active_ids: HashSet::new(),
            pending_events: VecDeque::new(),
            suppressed_ids: HashSet::new(),
            suppressed_order: VecDeque::new(),
            pending_limit,
        }
    }

    fn activate(&mut self, response: &Value) -> Vec<(String, Value)> {
        let Some(tracked) = self.policy.response_id(response) else {
            return Vec::new();
        };
        if self.suppressed_ids.contains(&tracked.id) {
            return Vec::new();
        }
        self.active_ids.insert(tracked.id.clone());
        self.take_pending(&tracked.id)
    }

    fn event_disposition(&self, stream: &str, event: &Value) -> EventDisposition {
        let Some(id) = self.policy.event_id(stream, event) else {
            return EventDisposition::Emit;
        };
        if self.suppressed_ids.contains(&id) {
            EventDisposition::Drop
        } else if self.active_ids.contains(&id) {
            EventDisposition::Emit
        } else {
            EventDisposition::Buffer(id)
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
        let mut matching = Vec::new();
        let mut retained = VecDeque::new();
        while let Some((event_id, stream, event)) = self.pending_events.pop_front() {
            if event_id == id {
                matching.push((stream, event));
            } else {
                retained.push_back((event_id, stream, event));
            }
        }
        self.pending_events = retained;
        matching
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

    fn cancelled(&mut self, id: &str) {
        self.active_ids.remove(id);
        self.take_pending(id);
        self.suppress(id.to_owned());
    }

    fn emitted(&mut self, stream: &str, event: &Value) {
        if self.policy.is_terminal(stream, event)
            && let Some(id) = self.policy.event_id(stream, event)
        {
            self.active_ids.remove(&id);
            self.take_pending(&id);
            self.suppress(id);
        }
    }

    fn active_ids(&self) -> Vec<String> {
        let mut ids = self.active_ids.iter().cloned().collect::<Vec<_>>();
        ids.sort();
        ids
    }
}

#[must_use]
pub fn spawn_output_actor<P: CorrelationPolicy>(
    policy: P,
    capacity: usize,
    pending_limit: usize,
) -> (OutputHandle, JoinHandle<Result<()>>) {
    spawn_output_actor_with_writer(policy, capacity, pending_limit, tokio::io::stdout())
}

#[must_use]
pub fn spawn_output_actor_with_writer<P, W>(
    policy: P,
    capacity: usize,
    pending_limit: usize,
    writer: W,
) -> (OutputHandle, JoinHandle<Result<()>>)
where
    P: CorrelationPolicy,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (sender, receiver) = mpsc::channel(capacity);
    let handle = OutputHandle { sender };
    let task = tokio::spawn(run_output_actor(
        receiver,
        writer,
        OutputState::new(policy, pending_limit),
    ));
    (handle, task)
}

async fn run_output_actor<P, W>(
    mut commands: mpsc::Receiver<OutputCommand>,
    mut writer: W,
    mut state: OutputState<P>,
) -> Result<()>
where
    P: CorrelationPolicy,
    W: AsyncWrite + Unpin,
{
    while let Some(command) = commands.recv().await {
        match command {
            OutputCommand::Response {
                id,
                result,
                cancelled_request_id,
            } => {
                let line = match &result {
                    Ok(response) => response_message(&id, response.clone()),
                    Err(error) => response_error_message(&id, error.clone()),
                };
                emit_line(&mut writer, &line).await?;
                if let Some(cancelled) = cancelled_request_id {
                    state.cancelled(&cancelled);
                }
                if let Ok(response) = result {
                    for (stream, event) in state.activate(&response) {
                        emit_line(&mut writer, &event_message(&stream, event.clone())).await?;
                        state.emitted(&stream, &event);
                    }
                }
            }
            OutputCommand::Event { stream, event } => {
                match state.event_disposition(&stream, &event) {
                    EventDisposition::Emit => {
                        emit_line(&mut writer, &event_message(&stream, event.clone())).await?;
                        state.emitted(&stream, &event);
                    }
                    EventDisposition::Buffer(id) => state.buffer(id, stream, event),
                    EventDisposition::Drop => {}
                }
            }
            OutputCommand::ProtocolError(error) => {
                emit_line(&mut writer, &protocol_error_message(error)).await?;
            }
            OutputCommand::TransportError(error) => {
                emit_line(&mut writer, &transport_error_message(error)).await?;
            }
            OutputCommand::ActiveIds(reply) => {
                let _ = reply.send(state.active_ids());
            }
            OutputCommand::Shutdown(id) => {
                emit_line(&mut writer, &shutdown_message(&id)).await?;
            }
        }
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
mod tests {
    use tokio::io::{AsyncReadExt, duplex};

    use super::*;

    #[test]
    fn pending_limit_counts_events_instead_of_correlation_ids() {
        let mut state = OutputState::new(BasicCorrelation, 2);
        for sequence in 1..=3 {
            state.buffer(
                "sub-1".into(),
                "things.changed".into(),
                serde_json::json!({ "sequence": sequence }),
            );
        }
        assert_eq!(state.pending_events.len(), 2);
        assert_eq!(state.pending_events[0].2["sequence"], 2);
    }

    #[tokio::test]
    async fn buffers_subscription_events_until_the_response_is_written() {
        let (writer, mut reader) = duplex(4096);
        let (output, task) = spawn_output_actor_with_writer(BasicCorrelation, 8, 4, writer);
        output
            .send(OutputCommand::Event {
                stream: "things.changed".into(),
                event: serde_json::json!({
                    "event": "subscribed", "subscription_id": "sub-1"
                }),
            })
            .await
            .unwrap();
        output
            .send(OutputCommand::Response {
                id: "subscribe".into(),
                result: Ok(serde_json::json!({
                    "data": { "subscription": { "id": "sub-1" } }
                })),
                cancelled_request_id: None,
            })
            .await
            .unwrap();
        drop(output);
        task.await.unwrap().unwrap();
        let mut text = String::new();
        reader.read_to_string(&mut text).await.unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            serde_json::from_str::<Value>(lines[0]).unwrap()["kind"],
            "response"
        );
        assert_eq!(
            serde_json::from_str::<Value>(lines[1]).unwrap()["kind"],
            "event"
        );
    }

    #[tokio::test]
    async fn drops_late_events_after_cancellation() {
        let (writer, mut reader) = duplex(4096);
        let (output, task) = spawn_output_actor_with_writer(BasicCorrelation, 8, 4, writer);
        output
            .send(OutputCommand::Response {
                id: "subscribe".into(),
                result: Ok(serde_json::json!({
                    "data": { "subscription": { "id": "sub-1" } }
                })),
                cancelled_request_id: None,
            })
            .await
            .unwrap();
        output
            .send(OutputCommand::Response {
                id: "cancel".into(),
                result: Ok(serde_json::json!({ "cancelled": "sub-1" })),
                cancelled_request_id: Some("sub-1".into()),
            })
            .await
            .unwrap();
        output
            .send(OutputCommand::Event {
                stream: "things.changed".into(),
                event: serde_json::json!({ "subscription_id": "sub-1" }),
            })
            .await
            .unwrap();
        drop(output);
        task.await.unwrap().unwrap();

        let mut text = String::new();
        reader.read_to_string(&mut text).await.unwrap();
        assert_eq!(text.lines().count(), 2);
    }
}
