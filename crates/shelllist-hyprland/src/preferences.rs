//! Typed, read-only compositor preferences. Cache lifetime and UI decisions
//! belong to consumers, not to this platform adapter.
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

/// Observed compositor preferences, never guessed from missing options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preferences {
    pub animations_enabled: bool,
}

#[derive(Deserialize)]
struct AnimationOption {
    option: String,
    #[serde(rename = "bool")]
    boolean: Option<bool>,
    int: Option<i64>,
}

pub(super) fn parse(reply: &str) -> Result<Preferences> {
    let option: AnimationOption =
        serde_json::from_str(reply).context("decode Hyprland animation preference")?;
    ensure!(
        option.option == "animations:enabled",
        "Unexpected Hyprland animation option"
    );
    let animations_enabled = match (option.boolean, option.int) {
        (Some(value), None) => value,
        (None, Some(0)) => false,
        (None, Some(1)) => true,
        (Some(value), Some(integer)) if integer == i64::from(value) => value,
        _ => bail!("Missing or invalid Hyprland animation preference"),
    };
    Ok(Preferences { animations_enabled })
}

/// Config reloads invalidate preferences. Socket connection changes also need
/// a refresh; those are reported separately by the event transport.
#[must_use]
pub fn preference_event(event: &str) -> bool {
    event
        .split_once(">>")
        .is_some_and(|(name, _)| name == "configreloaded")
}

#[cfg(test)]
mod tests {
    use super::{parse, preference_event};
    use crate::tests::command_server;

    #[test]
    fn parses_boolean_and_legacy_integer_options_without_guessing() {
        for (value, enabled) in [
            ("\"bool\":true", true),
            ("\"bool\":false", false),
            ("\"int\":0", false),
            ("\"int\":1", true),
            ("\"int\":0,\"bool\":false", false),
        ] {
            let reply = format!("{{\"option\":\"animations:enabled\",{value}}}");
            assert_eq!(parse(&reply).unwrap().animations_enabled, enabled);
        }
        for value in [
            "\"int\":2",
            "\"int\":-1",
            "\"int\":0.5",
            "\"int\":\"0\"",
            "\"bool\":0",
            "\"bool\":null",
            "\"int\":1,\"bool\":false",
            "\"set\":true",
        ] {
            let reply = format!("{{\"option\":\"animations:enabled\",{value}}}");
            assert!(parse(&reply).is_err(), "{reply}");
        }
        for reply in ["{}", "[]", "broken", r#"{"option":"other","int":0}"#] {
            assert!(parse(reply).is_err());
        }
        assert!(preference_event("configreloaded>>"));
        assert!(!preference_event("configreloaded-extra>>"));
        assert!(!preference_event("activewindow>>configreloaded"));
    }

    #[tokio::test]
    async fn preference_read_uses_native_bounded_command_transport() {
        let (_root, client, server) = command_server(
            "j/getoption animations:enabled",
            r#"{"option":"animations:enabled","bool":false}"#,
        )
        .await;
        assert!(!client.preferences().await.unwrap().animations_enabled);
        server.await.unwrap();
    }
}
