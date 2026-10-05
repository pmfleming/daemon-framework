use serde_json::{Map, Value, json};

/// Identity embedded in every versioned API response and event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiIdentity {
    pub protocol: &'static str,
    pub version: u32,
}

impl ApiIdentity {
    #[must_use]
    pub const fn new(protocol: &'static str, version: u32) -> Self {
        Self { protocol, version }
    }
}

/// Configurable API error fields. Optional fields are omitted to preserve each
/// daemon's existing wire contract.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub retryable: Option<bool>,
    pub details: Option<Value>,
}

impl ApiError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: None,
            details: None,
        }
    }

    #[must_use]
    pub const fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);
        self
    }

    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    #[must_use]
    pub fn into_value(self) -> Value {
        let mut value = Map::from_iter([
            ("code".into(), Value::String(self.code)),
            ("message".into(), Value::String(self.message)),
        ]);
        if let Some(retryable) = self.retryable {
            value.insert("retryable".into(), Value::Bool(retryable));
        }
        if let Some(details) = self.details {
            value.insert("details".into(), details);
        }
        Value::Object(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Correlation<'a> {
    None,
    Subscription(&'a str),
    Request(&'a str),
    Both {
        subscription_id: &'a str,
        request_id: &'a str,
    },
}

#[must_use]
pub fn success(api: ApiIdentity, data: Value) -> Value {
    json!({
        "protocol": api.protocol,
        "version": api.version,
        "ok": true,
        "data": data,
    })
}

#[must_use]
pub fn error(api: ApiIdentity, error: ApiError) -> Value {
    json!({
        "protocol": api.protocol,
        "version": api.version,
        "ok": false,
        "error": error.into_value(),
    })
}

/// Builds the common event fields and merges daemon-owned payload fields.
#[must_use]
pub fn event_envelope(
    api: ApiIdentity,
    stream: &str,
    event: &str,
    correlation: Correlation<'_>,
    fields: Value,
) -> Value {
    let mut envelope = match fields {
        Value::Object(fields) => fields,
        _ => Map::new(),
    };
    envelope.remove("subscription_id");
    envelope.remove("request_id");
    envelope.extend([
        ("protocol".into(), json!(api.protocol)),
        ("version".into(), json!(api.version)),
        ("stream".into(), json!(stream)),
        ("event".into(), json!(event)),
    ]);
    if let Correlation::Subscription(id)
    | Correlation::Both {
        subscription_id: id,
        ..
    } = correlation
    {
        envelope.insert("subscription_id".into(), json!(id));
    }
    if let Correlation::Request(id) | Correlation::Both { request_id: id, .. } = correlation {
        envelope.insert("request_id".into(), json!(id));
    }
    Value::Object(envelope)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ApiError, ApiIdentity, Correlation, error, event_envelope};

    const API: ApiIdentity = ApiIdentity::new("test-api", 1);

    #[test]
    fn optional_error_fields_are_absent_until_requested() {
        let basic = error(API, ApiError::new("failed", "no"));
        assert_eq!(
            basic,
            json!({
                "protocol": "test-api", "version": 1, "ok": false,
                "error": { "code": "failed", "message": "no" }
            })
        );

        let detailed = error(
            API,
            ApiError::new("failed", "no")
                .with_retryable(false)
                .with_details(json!({ "operation": "test" })),
        );
        assert_eq!(detailed["error"]["retryable"], false);
        assert_eq!(detailed["error"]["details"]["operation"], "test");
    }

    #[test]
    fn domain_fields_cannot_replace_envelope_identity() {
        for (correlation, subscription, request) in [
            (Correlation::None, None, None),
            (Correlation::Subscription("sub-1"), Some("sub-1"), None),
            (Correlation::Request("req-1"), None, Some("req-1")),
            (
                Correlation::Both {
                    subscription_id: "sub-1",
                    request_id: "req-1",
                },
                Some("sub-1"),
                Some("req-1"),
            ),
        ] {
            let event = event_envelope(
                API,
                "things.changed",
                "changed",
                correlation,
                json!({
                    "protocol": "spoofed",
                    "version": 99,
                    "stream": "spoofed",
                    "event": "spoofed",
                    "subscription_id": "spoofed",
                    "request_id": "spoofed",
                    "data": { "revision": 2 }
                }),
            );
            assert_eq!(event["protocol"], "test-api");
            assert_eq!(event["version"], 1);
            assert_eq!(event["stream"], "things.changed");
            assert_eq!(event["event"], "changed");
            assert_eq!(
                event
                    .get("subscription_id")
                    .and_then(serde_json::Value::as_str),
                subscription
            );
            assert_eq!(
                event.get("request_id").and_then(serde_json::Value::as_str),
                request
            );
            assert_eq!(event["data"]["revision"], 2);
        }
        assert_eq!(
            event_envelope(API, "s", "e", Correlation::None, json!(null)),
            event_envelope(API, "s", "e", Correlation::None, json!({})),
        );
    }
}
