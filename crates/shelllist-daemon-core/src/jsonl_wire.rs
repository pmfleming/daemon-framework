use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Default UTF-8 bytes in one bridge input record, excluding LF or CRLF.
/// Enforced before JSON parsing; this does not bound direct D-Bus calls/results.
pub const DEFAULT_JSONL_REQUEST_BYTES: usize = 1024 * 1024;
/// Hard ceiling for explicitly configured bulk clients (notably clipboard text).
/// Matches the D-Bus message ceiling; actual D-Bus payloads must also fit headers.
pub const MAX_JSONL_REQUEST_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum ClientRequest {
    Call {
        id: String,
        method: String,
        #[serde(default)]
        params: Value,
    },
    Subscribe {
        id: String,
        #[serde(default)]
        streams: Vec<String>,
    },
    Cancel {
        id: String,
        request_id: String,
    },
    /// Release subscriptions belonging to the routed consumer. In-flight
    /// replies retain their route so a detached frontend can cancel late IDs.
    Release {
        id: String,
    },
    Shutdown {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RouteKind {
    Call,
    Subscription,
    BaseSubscription,
    Control,
}

/// Bridge-local addressing, never forwarded into the domain D-Bus API.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientRoute {
    pub consumer_id: String,
    pub local_id: String,
    pub generation: u64,
    pub kind: RouteKind,
}

/// Unrouted legacy clients keep their existing wire format.
#[derive(Debug, Deserialize, PartialEq)]
pub struct ClientMessage {
    #[serde(flatten)]
    pub request: ClientRequest,
    #[serde(default)]
    pub route: Option<ClientRoute>,
}

impl ClientMessage {
    pub fn validate(&self) -> Result<(), &'static str> {
        let Some(route) = &self.route else {
            return if matches!(self.request, ClientRequest::Release { .. }) {
                Err("release requires a consumer route")
            } else {
                Ok(())
            };
        };
        if !(1..=256).contains(&route.consumer_id.len())
            || !(1..=1024).contains(&route.local_id.len())
        {
            return Err("invalid bridge route identifier length");
        }
        let valid = matches!(
            (&self.request, route.kind),
            (ClientRequest::Call { .. }, RouteKind::Call)
                | (
                    ClientRequest::Subscribe { .. },
                    RouteKind::Subscription | RouteKind::BaseSubscription
                )
                | (
                    ClientRequest::Cancel { .. } | ClientRequest::Release { .. },
                    RouteKind::Control
                )
        );
        if valid {
            Ok(())
        } else {
            Err("bridge route kind does not match request")
        }
    }
}

#[must_use]
pub fn addressed_message(mut message: Value, route: Option<&ClientRoute>) -> Value {
    if let Some(route) = route {
        message["route"] = json!(route);
    }
    message
}

#[must_use]
pub fn response_message(id: &str, response: Value) -> Value {
    let mut message = json!({ "kind": "response", "id": id, "ok": true });
    message["response"] = response;
    message
}

#[must_use]
pub fn response_error_message(id: &str, error: impl Into<String>) -> Value {
    json!({ "kind": "response", "id": id, "ok": false, "error": error.into() })
}

#[must_use]
pub fn event_message(stream: &str, event: Value) -> Value {
    let mut message = json!({ "kind": "event", "stream": stream });
    message["event"] = event;
    message
}

#[must_use]
pub fn protocol_error_message(error: impl Into<String>) -> Value {
    json!({ "kind": "protocol-error", "error": error.into() })
}

#[must_use]
pub fn transport_error_message(error: impl Into<String>) -> Value {
    json!({ "kind": "transport-error", "outcome": "transport-generation-failure", "error": error.into() })
}

#[must_use]
pub fn shutdown_message(id: &str) -> Value {
    response_message(id, json!({ "shutdown": true }))
}

#[cfg(test)]
mod tests {
    use super::ClientMessage;
    use serde_json::json;

    #[test]
    fn validates_route_kinds_and_identifier_bounds() -> serde_json::Result<()> {
        for (op, kind, consumer, valid) in [
            ("call", "call", "view".into(), true),
            ("subscribe", "subscription", "view".into(), true),
            ("release", "control", "view".into(), true),
            ("call", "control", "view".into(), false),
            ("subscribe", "call", "view".into(), false),
            ("release", "call", "view".into(), false),
            ("call", "call", "".into(), false),
            ("call", "call", "v".repeat(257), false),
        ] {
            let message: ClientMessage = serde_json::from_value(json!({
                "op": op, "id": "1", "method": "things.get",
                "route": { "consumerId": consumer, "localId": "1", "generation": 0, "kind": kind }
            }))?;
            assert_eq!(message.validate().is_ok(), valid, "{op}/{kind}");
        }
        let release: ClientMessage = serde_json::from_value(json!({ "op": "release", "id": "1" }))?;
        assert!(release.validate().is_err());
        Ok(())
    }
}
