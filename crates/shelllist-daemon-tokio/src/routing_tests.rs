use anyhow::Result;
use serde_json::{Value, json};
use shelllist_daemon_core::{ClientRoute, RouteKind};
use tokio::io::sink;

use super::{
    BasicCorrelation, CorrelationPolicy, OutputCommand, OutputState, TrackedId, emit_command,
};

fn route(consumer: &str, generation: u64, kind: RouteKind) -> ClientRoute {
    ClientRoute {
        consumer_id: consumer.into(),
        local_id: "same-local-id".into(),
        generation,
        kind,
    }
}

fn subscription(id: &str, owner: ClientRoute) -> OutputCommand {
    OutputCommand::Response {
        id: "subscribe".into(),
        result: Ok(json!({ "data": { "subscription": { "id": id } } })),
        cancelled_request_id: None,
        route: Some(owner),
    }
}

fn event(id: &str) -> OutputCommand {
    OutputCommand::Event {
        stream: "updates".into(),
        event: json!({ "subscription_id": id }),
    }
}

struct Output<P> {
    state: OutputState<P>,
    bytes: Vec<u8>,
}
impl<P: CorrelationPolicy> Output<P> {
    fn new(policy: P) -> Self {
        Self {
            state: OutputState::new(policy, 16),
            bytes: Vec::new(),
        }
    }
    async fn send(&mut self, command: OutputCommand) -> Result<()> {
        emit_command(&mut self.bytes, &mut self.state, command).await
    }
    async fn subscribe(&mut self, id: &str, owner: ClientRoute) -> Result<()> {
        self.send(subscription(id, owner)).await
    }
    fn lines(&self) -> Vec<Value> {
        std::str::from_utf8(&self.bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[tokio::test]
async fn replies_echo_routes_without_retaining_ordinary_requests() -> Result<()> {
    let mut output = Output::new(BasicCorrelation);
    for (consumer, result) in [
        ("a", Ok(json!({ "ok": true }))),
        ("b", Err("refused".into())),
    ] {
        output
            .send(OutputCommand::Response {
                id: "same-local-id".into(),
                result,
                cancelled_request_id: None,
                route: Some(route(consumer, 7, RouteKind::Call)),
            })
            .await?;
    }
    let lines = output.lines();
    assert_eq!(lines[0]["route"]["consumerId"], "a");
    assert_eq!(lines[1]["route"]["consumerId"], "b");
    assert_eq!(lines[1]["route"]["generation"], 7);
    assert_eq!(lines[1]["error"], "refused");
    assert!(output.state.active_ids.is_empty());
    Ok(())
}

#[tokio::test]
async fn early_events_follow_the_addressed_reply_and_stay_owner_scoped() -> Result<()> {
    let mut output = Output::new(BasicCorrelation);
    output.send(event("sub-a")).await?;
    assert!(output.bytes.is_empty());
    output
        .subscribe("sub-a", route("a", 1, RouteKind::BaseSubscription))
        .await?;
    output
        .subscribe("sub-b", route("b", 1, RouteKind::Subscription))
        .await?;
    output.send(event("sub-b")).await?;
    let lines = output.lines();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["kind"], "response");
    assert_eq!(lines[1]["kind"], "event");
    assert_eq!(lines[1]["route"]["consumerId"], "a");
    assert_eq!(lines[3]["route"]["consumerId"], "b");
    assert_eq!(
        output.state.owned_ids(&route("a", 1, RouteKind::Control)),
        ["sub-a"]
    );
    assert!(
        output
            .state
            .owned_ids(&route("a", 2, RouteKind::Control))
            .is_empty()
    );
    let repeated = json!({ "data": { "subscription": { "id": "sub-a" } } });
    output.state.activate(&repeated, None);
    assert_eq!(
        output.state.owned_ids(&route("a", 1, RouteKind::Control)),
        ["sub-a"]
    );
    Ok(())
}

#[tokio::test]
async fn cancellation_failure_retains_ownership_success_and_reset_remove_it() -> Result<()> {
    let mut output = Output::new(BasicCorrelation);
    output
        .subscribe("sub-a", route("a", 1, RouteKind::Subscription))
        .await?;
    output
        .send(OutputCommand::Response {
            id: "cancel".into(),
            result: Err("temporarily unavailable".into()),
            cancelled_request_id: None,
            route: Some(route("a", 1, RouteKind::Control)),
        })
        .await?;
    assert_eq!(
        output.state.owned_ids(&route("a", 1, RouteKind::Control)),
        ["sub-a"]
    );
    output
        .send(OutputCommand::Cancelled("sub-a".into()))
        .await?;
    assert!(output.state.active_ids.is_empty());
    let length = output.bytes.len();
    output.send(event("sub-a")).await?;
    assert_eq!(
        output.bytes.len(),
        length,
        "late cancelled events must be suppressed"
    );
    output
        .subscribe("sub-b", route("b", 2, RouteKind::Subscription))
        .await?;
    output.send(OutputCommand::ResetCorrelation).await?;
    assert!(output.state.active_ids.is_empty());
    assert!(output.state.pending_events.is_empty());
    Ok(())
}

struct TerminalSubscription;
impl CorrelationPolicy for TerminalSubscription {
    fn response_id(&self, response: &Value) -> Option<TrackedId> {
        BasicCorrelation.response_id(response)
    }
    fn event_id(&self, stream: &str, event: &Value) -> Option<String> {
        BasicCorrelation.event_id(stream, event)
    }
    fn is_terminal(&self, _stream: &str, _event: &Value) -> bool {
        true
    }
}

#[tokio::test]
async fn terminal_events_keep_their_route_before_retiring_ownership() -> Result<()> {
    for buffered in [false, true] {
        let mut output = Output::new(TerminalSubscription);
        if buffered {
            output.send(event("sub-a")).await?;
        }
        output
            .subscribe("sub-a", route("a", 1, RouteKind::Subscription))
            .await?;
        output.send(event("sub-a")).await?;
        let lines = output.lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["route"]["consumerId"], "a");
        assert_eq!(lines[1]["event"]["subscription_id"], "sub-a");
        assert!(output.state.active_ids.is_empty());
        assert!(output.state.pending_events.is_empty());
        assert!(output.state.suppressed_ids.contains("sub-a"));
    }
    Ok(())
}

#[tokio::test]
async fn twenty_thousand_requests_and_subscription_churn_stay_bounded() -> Result<()> {
    let mut state = OutputState::new(BasicCorrelation, 16);
    let mut writer = sink();
    for index in 0..20_000 {
        let owner = route("clipboard", index / 5, RouteKind::Call);
        emit_command(
            &mut writer,
            &mut state,
            OutputCommand::Response {
                id: format!("query-{}", index / 5),
                result: Ok(json!({ "ok": true })),
                cancelled_request_id: None,
                route: Some(owner),
            },
        )
        .await?;
        assert!(state.active_ids.is_empty());
        if index % 5 == 0 {
            let id = format!("sub-{index}");
            emit_command(
                &mut writer,
                &mut state,
                subscription(&id, route("clipboard", index, RouteKind::BaseSubscription)),
            )
            .await?;
            emit_command(&mut writer, &mut state, OutputCommand::Cancelled(id)).await?;
        }
        assert!(state.active_ids.is_empty());
        assert!(state.suppressed_ids.len() <= 16);
        assert!(state.suppressed_order.len() <= 16);
    }
    Ok(())
}
