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
fn rules_encode_enablement_geometry_and_explicit_mirroring() {
    for name in ["eDP-1", "Virtual_2"] {
        assert_eq!(
            command(name, Rule::Disable).unwrap(),
            format!("hl.monitor({{ output = \"{name}\", disabled = true }})")
        );
        for mode in ["1920x1200@60.00Hz", "1920x1200@60.00"] {
            assert_eq!(
                command(name, configured(Mode::Exact(mode), 1.25, Some(0), "")).unwrap(),
                format!(
                    "hl.monitor({{ output = \"{name}\", disabled = false, mode = \"1920x1200@60.00\", position = \"-1920x120\", scale = 1.25, transform = 0, mirror = \"\" }})"
                )
            );
        }
    }
    let command = command(
        "eDP-1",
        Rule::Enable {
            mode: Mode::Preferred,
            position: Position::Auto,
            scale: 1.25,
            transform: None,
            mirror: "DP-1",
        },
    )
    .unwrap();
    assert_eq!(
        command,
        "hl.monitor({ output = \"eDP-1\", disabled = false, mode = \"preferred\", position = \"auto\", scale = 1.25, mirror = \"DP-1\" })"
    );
}

#[test]
fn commands_reject_injection_invalid_geometry_and_unintended_outputs() {
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
    for mode in [
        "",
        "preferred",
        "1920x1080",
        "0x1080@60",
        "1920x0@60",
        "1920x1080@0",
        "1920x1080@NaN",
        "1920x1080@inf",
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
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
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

#[test]
fn advanced_fields_are_optional_bounded_and_not_lua_expressions() {
    use super::{Advanced, command_with_advanced};
    let rule = configured(Mode::Preferred, 1.0, None, "");
    let advanced = Advanced {
        vrr: Some(true),
        bit_depth: Some(10),
        color: Some("hdr"),
        sdr_brightness: Some(1.2),
        sdr_saturation: Some(0.9),
    };
    let lua = command_with_advanced("DP-1", rule, advanced).unwrap();
    assert!(lua.contains(
        "vrr = 1, bitdepth = 10, cm = \"hdr\", sdrbrightness = 1.2, sdrsaturation = 0.9"
    ));
    for bad in [
        Advanced {
            color: Some("hdr\"}); bad()--"),
            ..advanced
        },
        Advanced {
            bit_depth: Some(9),
            ..advanced
        },
        Advanced {
            sdr_brightness: Some(f64::NAN),
            ..advanced
        },
        Advanced {
            sdr_saturation: Some(3.0),
            ..advanced
        },
    ] {
        assert!(command_with_advanced("DP-1", rule, bad).is_err());
    }
    assert_eq!(
        command("DP-1", rule).unwrap(),
        command_with_advanced("DP-1", rule, Advanced::default()).unwrap()
    );
}
