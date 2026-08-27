use serde_json::{Map, Value, json};

use crate::ApiIdentity;

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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
}
