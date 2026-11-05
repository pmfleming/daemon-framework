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

#[cfg(test)]
mod tests {
    use super::parse;

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
        assert!(crate::CompositorEvent::parse("configreloaded>>").invalidates_preferences());
        assert!(!crate::CompositorEvent::parse("configreloaded-extra>>").invalidates_preferences());
        assert!(
            !crate::CompositorEvent::parse("activewindow>>configreloaded")
                .invalidates_preferences()
        );
    }
}
