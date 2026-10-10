use super::{Mode, Position, Rule, command};

fn configured<'a>(mode: Mode<'a>, scale: f64, transform: Option<u8>, mirror: &'a str) -> Rule<'a> {
    Rule::Enable {
        mode,
        position: Position::Exact { x: -1920, y: 120 },
        scale,
        transform,
        mirror,
    }
}

#[test]
fn exact_rules_explicitly_enable_and_clear_previous_mirroring() {
    for name in ["eDP-1", "DP-1", "HDMI-A-1", "HEADLESS-1", "Virtual_2"] {
        assert_eq!(
            command(name, Rule::Disable).unwrap(),
            format!("eval hl.monitor({{ output = \"{name}\", disabled = true }})")
        );
        for mode in ["1920x1200@60.00Hz", "1920x1200@60.00"] {
            assert_eq!(
                command(name, configured(Mode::Exact(mode), 1.25, Some(0), "")).unwrap(),
                format!(
                    "eval hl.monitor({{ output = \"{name}\", disabled = false, mode = \"1920x1200@60.00\", position = \"-1920x120\", scale = 1.25, transform = 0, mirror = \"\" }})"
                )
            );
        }
    }
}

#[test]
fn preferred_auto_restore_clears_disabled_but_preserves_unspecified_transform() {
    let command = command(
        "eDP-1",
        Rule::Enable {
            mode: Mode::Preferred,
            position: Position::Auto,
            scale: 1.25,
            transform: None,
            mirror: "",
        },
    )
    .unwrap();
    assert_eq!(
        command,
        "eval hl.monitor({ output = \"eDP-1\", disabled = false, mode = \"preferred\", position = \"auto\", scale = 1.25, mirror = \"\" })"
    );
}

#[test]
fn mirrored_rules_and_geometry_do_not_import_daemon_policy_limits() {
    let encoded = command(
        "HEADLESS-1",
        Rule::Enable {
            mode: Mode::Exact("20000x18000@2000Hz"),
            position: Position::Exact {
                x: i32::MIN,
                y: i32::MAX,
            },
            scale: 0.125,
            transform: Some(7),
            mirror: "DP-1",
        },
    )
    .unwrap();
    assert!(encoded.contains("disabled = false"));
    assert!(encoded.contains("mode = \"20000x18000@2000\""));
    assert!(encoded.contains("position = \"-2147483648x2147483647\""));
    assert!(encoded.contains("scale = 0.125, transform = 7, mirror = \"DP-1\""));
}

#[test]
fn identifiers_cannot_inject_lua_or_select_unintended_outputs() {
    for name in [
        "",
        "desc:Some Display",
        "*",
        " DP-1",
        "DP-1\n",
        "DP-1\0",
        "DP\\1",
        "éDP-1",
        "eDP-1\"});os.execute('bad')--",
    ] {
        assert!(command(name, Rule::Disable).is_err(), "{name:?}");
        assert!(
            command(name, configured(Mode::Preferred, 1.0, None, "")).is_err(),
            "{name:?}"
        );
        if !name.is_empty() {
            assert!(
                command("eDP-1", configured(Mode::Preferred, 1.0, None, name)).is_err(),
                "{name:?}"
            );
        }
    }
}

#[test]
fn mode_scale_and_transform_validation_rejects_invalid_commands() {
    for mode in [
        "",
        "preferred",
        "1920x1080",
        "0x1080@60",
        "1920x0@60",
        "1920x1080@0",
        "1920x1080@NaN",
        "1920x1080@inf",
        "1920x1080@1e3",
        "1920x1080@-60",
        "1920x1080@60HzHz",
        "1920x1080@60\n",
        "1920x1080@60\"});bad()--",
        "4294967296x1080@60",
    ] {
        assert!(
            command("DP-1", configured(Mode::Exact(mode), 1.0, Some(0), "")).is_err(),
            "{mode:?}"
        );
    }
    for scale in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            command("DP-1", configured(Mode::Preferred, scale, None, "")).is_err(),
            "{scale}"
        );
    }
    for transform in [8, u8::MAX] {
        assert!(
            command(
                "DP-1",
                configured(Mode::Preferred, 1.0, Some(transform), "")
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn encoded_monitor_rule_uses_existing_native_transport() {
    let (_root, client, server) = crate::tests::command_server(
        "eval hl.monitor({ output = \"DP-1\", disabled = false, mode = \"1920x1200@60.00\", position = \"-1920x120\", scale = 1.25, transform = 0, mirror = \"\" })",
        "ok",
    ).await;
    let command = command(
        "DP-1",
        configured(Mode::Exact("1920x1200@60.00Hz"), 1.25, Some(0), ""),
    )
    .unwrap();
    assert_eq!(client.request(&command).await.unwrap(), "ok");
    server.await.unwrap();
}
