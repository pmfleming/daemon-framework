//! Canonical payload-oriented envelopes. Domain fields can never overwrite identity.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ApiError, ApiIdentity, Correlation};

#[derive(Debug, Serialize, JsonSchema)]
pub struct Event<T> {
    protocol: &'static str,
    version: u32,
    stream: String,
    event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    subscription_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ApiError>,
}

impl<T> Event<T> {
    #[must_use]
    pub fn new(
        api: ApiIdentity,
        stream: &str,
        event: &str,
        correlation: Correlation<'_>,
        data: T,
    ) -> Self {
        let (subscription_id, request_id) = match correlation {
            Correlation::None => (None, None),
            Correlation::Subscription(id) => (Some(id.into()), None),
            Correlation::Request(id) => (None, Some(id.into())),
            Correlation::Both {
                subscription_id,
                request_id,
            } => (Some(subscription_id.into()), Some(request_id.into())),
        };
        Self {
            protocol: api.protocol,
            version: api.version,
            stream: stream.into(),
            event: event.into(),
            subscription_id,
            request_id,
            data,
            error: None,
        }
    }
    #[must_use]
    pub fn with_error(mut self, error: ApiError) -> Self {
        self.error = Some(error);
        self
    }
}

impl<T: Serialize> Event<T> {
    pub fn into_value(self) -> serde_json::Result<Value> {
        serde_json::to_value(self)
    }
}

/// Explicit event body for sources which can publish either state or an error.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventBody {
    pub data: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}
impl EventBody {
    #[must_use]
    pub fn data(data: Value) -> Self {
        Self { data, error: None }
    }
    #[must_use]
    pub fn error(error: ApiError) -> Self {
        Self {
            data: serde_json::json!({}),
            error: Some(error),
        }
    }
    #[must_use]
    pub fn envelope(
        self,
        api: ApiIdentity,
        stream: &str,
        event: &str,
        correlation: Correlation<'_>,
    ) -> Event<Value> {
        let mut envelope = Event::new(api, stream, event, correlation, self.data);
        envelope.error = self.error;
        envelope
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Gap {
    pub reason: GapReason,
    /// Decimal string: loss counters must survive JavaScript without rounding.
    pub dropped_events: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum GapReason {
    SubscriberLag,
    SourceReset,
}

impl Gap {
    #[must_use]
    pub fn lagged(dropped_events: u64) -> Self {
        Self {
            reason: GapReason::SubscriberLag,
            dropped_events: dropped_events.to_string(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Success<T> {
    protocol: &'static str,
    version: u32,
    ok: bool,
    data: T,
}
impl<T> Success<T> {
    #[must_use]
    pub const fn new(api: ApiIdentity, data: T) -> Self {
        Self {
            protocol: api.protocol,
            version: api.version,
            ok: true,
            data,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Failure {
    protocol: &'static str,
    version: u32,
    ok: bool,
    error: ApiError,
}
impl Failure {
    #[must_use]
    pub const fn new(api: ApiIdentity, error: ApiError) -> Self {
        Self {
            protocol: api.protocol,
            version: api.version,
            ok: false,
            error,
        }
    }
}
