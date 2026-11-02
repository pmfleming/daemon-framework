//! Versioned workspace lifecycle messages. Policy remains in the owning daemon.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const MAX_WORKSPACE_ID: i64 = i32::MAX as i64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchTarget {
    #[default]
    Default,
    Current,
    New,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub session: String,
    pub monitor: String,
    pub workspace_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Purpose {
    LandingPad,
    Launch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Create {
    pub request_id: String,
    pub context: Context,
    pub purpose: Purpose,
    /// None allocates a fresh ID; Some leases an existing destination.
    pub workspace_id: Option<i64>,
    pub origin: String,
    pub icon: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub session: String,
    pub request_id: String,
    pub workspace_id: i64,
    pub generation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WindowIdentity {
    pub address: String,
    pub pid: u32,
    pub workspace_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub lease: Lease,
    pub window: WindowIdentity,
    pub follow: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Close {
    pub request_id: String,
    pub context: Context,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Lookup {
    pub session: String,
    pub request_id: String,
}

/// Keys are never interpreted as paths or compositor code.
pub fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_intents_and_unknown_effect_fields_are_rejected() {
        assert!(serde_json::from_str::<LaunchTarget>("\"here\"").is_err());
        let context = Context {
            session: "s".into(),
            monitor: "DP-1".into(),
            workspace_id: 7,
        };
        let mut value = serde_json::to_value(Close {
            request_id: "close-1".into(),
            context,
        })
        .unwrap();
        value["force"] = true.into();
        assert!(serde_json::from_value::<Close>(value).is_err());
        for invalid in ["", "../other", "x;y", "x\n", "é"] {
            assert!(!valid_request_id(invalid));
        }
        assert!(valid_request_id("app-operation_123"));
    }
}
