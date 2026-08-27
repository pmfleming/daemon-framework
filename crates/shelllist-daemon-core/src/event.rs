use serde_json::{Map, Value, json};

use crate::ApiIdentity;

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

/// Builds the common event fields and merges daemon-owned payload fields.
#[must_use]
pub fn event_envelope(
    api: ApiIdentity,
    stream: &str,
    event: &str,
    correlation: Correlation<'_>,
    fields: Value,
) -> Value {
    let mut envelope = Map::from_iter([
        ("protocol".into(), json!(api.protocol)),
        ("version".into(), json!(api.version)),
        ("stream".into(), json!(stream)),
        ("event".into(), json!(event)),
    ]);
    match correlation {
        Correlation::None => {}
        Correlation::Subscription(id) => {
            envelope.insert("subscription_id".into(), json!(id));
        }
        Correlation::Request(id) => {
            envelope.insert("request_id".into(), json!(id));
        }
        Correlation::Both {
            subscription_id,
            request_id,
        } => {
            envelope.insert("subscription_id".into(), json!(subscription_id));
            envelope.insert("request_id".into(), json!(request_id));
        }
    }
    if let Value::Object(fields) = fields {
        envelope.extend(fields);
    }
    Value::Object(envelope)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn correlation_and_domain_fields_are_preserved() {
        let event = event_envelope(
            ApiIdentity::new("test-api", 1),
            "things.changed",
            "changed",
            Correlation::Subscription("sub-1"),
            json!({ "data": { "revision": 2 } }),
        );
        assert_eq!(event["subscription_id"], "sub-1");
        assert_eq!(event["data"]["revision"], 2);
    }
}
