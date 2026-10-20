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

/// Common API error fields. Absent optional fields are omitted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
        value.extend(
            self.retryable
                .map(|flag| ("retryable".into(), Value::Bool(flag))),
        );
        value.extend(self.details.map(|details| ("details".into(), details)));
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
    let mut response = json!({ "protocol": api.protocol, "version": api.version, "ok": true });
    // json! serializes Value by reference; insert it directly to retain its storage.
    response["data"] = data;
    response
}

#[must_use]
pub fn error(api: ApiIdentity, error: ApiError) -> Value {
    let mut response = json!({ "protocol": api.protocol, "version": api.version, "ok": false });
    response["error"] = error.into_value();
    response
}
