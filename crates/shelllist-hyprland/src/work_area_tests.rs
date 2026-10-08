use super::work_area::{geometry_event, parse_work_areas};
use serde_json::{Value, json};

fn fixture() -> [Value; 5] {
    [
        json!([
            {"id":0,"name":"eDP-1","description":"Laptop panel","focused":true,"x":0,"y":0,"width":1920,"height":1200,"scale":1.25,"activeWorkspace":{"id":1},"specialWorkspace":{"id":0},"reserved":[0,51,0,0]},
            {"id":1,"name":"DP-1","description":"External display","make":"Acme","model":"Panel","serial":"123","x":1536,"y":0,"width":3840,"height":2160,"scale":2,"activeWorkspace":{"id":3},"reserved":[24,80,10,20]}
        ]),
        json!([{"id":1,"name":"1","windows":2},{"id":3,"name":"3","windows":0}]),
        json!([]),
        json!([{"workspace":{"id":1},"mapped":true,"floating":false,"grouped":["a","b"]},{"workspace":{"id":1},"mapped":true,"floating":true}]),
        json!({"css":"2"}),
    ]
}
fn text(parts: &[Value; 5]) -> String {
    parts
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n\n")
}
fn margins(parts: &[Value; 5], monitor: &str) -> Value {
    serde_json::to_value(parse_work_areas(&text(parts)).unwrap().get(monitor)).unwrap()
}
fn assert_margins(parts: &[Value; 5], monitor: &str, [left, top, right, bottom]: [f64; 4]) {
    assert_eq!(
        margins(parts, monitor),
        json!({"left": left, "top": top, "right": right, "bottom": bottom})
    );
}
fn assert_rule(parts: &mut [Value; 5], selector: &str, monitor: &str, top: f64) {
    parts[2] = json!([{"workspaceString":selector,"gapsOut":[0]}]);
    assert_eq!(margins(parts, monitor)["top"], top, "{selector}");
}
#[test]
fn logical_reservations_css_gaps_and_ordered_rules() {
    let mut parts = fixture();
    assert_margins(&parts, "eDP-1", [2.0, 53.0, 2.0, 2.0]);
    parts[2] = json!([{"workspaceString":"r[1-4]","gapsOut":[9,10,11,12]},{"workspaceString":"1","gapsOut":[3,4,5,6]},{"workspaceString":"1","borderSize":10},{"workspaceString":"1","gapsOut":"invalid"},{"workspaceString":"3","gapsOut":[7]}]);
    assert_margins(&parts, "eDP-1", [6.0, 54.0, 4.0, 5.0]);
    assert_margins(&parts, "DP-1", [31.0, 87.0, 17.0, 27.0]);
    parts[2] = json!([]);
    parts[4]["css"] = json!("4 8 12 16");
    parts[0][0]["reserved"] = json!([7, 92, 13, 28]);
    assert_margins(&parts, "eDP-1", [23.0, 96.0, 21.0, 40.0]);
    assert!(margins(&parts, "missing").is_null());
}
#[test]
fn css_shorthand_accepts_one_to_four_finite_numbers_only() {
    let mut parts = fixture();
    for (text, array, expected) in [
        ("1", json!([1]), [1.0, 52.0, 1.0, 1.0]),
        ("1 2", json!([1, 2]), [2.0, 52.0, 2.0, 1.0]),
        ("1 2 3", json!([1, 2, 3]), [2.0, 52.0, 2.0, 3.0]),
        ("1 2 3 4", json!([1, 2, 3, 4]), [4.0, 52.0, 2.0, 3.0]),
    ] {
        for css in [json!(text), array] {
            parts[4]["css"] = css;
            assert_margins(&parts, "eDP-1", expected);
        }
    }
    for invalid in [
        json!(""),
        json!([]),
        json!("1 bad"),
        json!([1, null]),
        json!("NaN"),
        json!("1 inf"),
        json!("1 2 3 -inf"),
        json!("1 2 3 4 5"),
        json!([1, 2, 3, 4, 5]),
        json!(true),
    ] {
        parts[4]["css"] = invalid;
        assert!(parse_work_areas(&text(&parts)).is_err());
    }
}

#[test]
fn smart_gaps_and_group_visibility_filters() {
    let mut parts = fixture();
    for selector in [
        "m[eDP-1] w[t1]",
        "w[tg1]",
        "w[f1]",
        "r[1-4] m[current] w[2] f[-1]",
        "  r[1-4]m[current]\t w[2] f[-1]  ",
    ] {
        assert_rule(&mut parts, selector, "eDP-1", 51.0);
    }
    parts[2] = json!([{"workspaceString":"w[t1]","gapsOut":[0,0,0,0]}]);
    parts[3][1]["floating"] = json!(false);
    assert_eq!(margins(&parts, "eDP-1")["top"], 53.0);
    parts[3][1]["grouped"] = json!(["b", "a"]);
    parts[2] = json!([{"workspaceString":"w[tg1]","gapsOut":[0,0,0,0]}]);
    assert_eq!(margins(&parts, "eDP-1")["top"], 51.0);
    parts[3][1]["floating"] = json!(true);
    parts[3][1]["pinned"] = json!(true);
    parts[3][1]["visible"] = json!(false);
    for selector in ["w[p1]", "w[fv0]"] {
        assert_rule(&mut parts, selector, "eDP-1", 51.0);
    }
    parts[3][0]["fullscreen"] = json!(2);
    parts[1][0]["hasfullscreen"] = json!(true);
    for (selector, top) in [("f[0]", 51.0), ("f[1]", 53.0), ("f[other]", 53.0)] {
        assert_rule(&mut parts, selector, "eDP-1", top);
    }
    parts[3][0]["workspace"]["id"] = json!(3);
    assert_rule(&mut parts, "f[0]", "eDP-1", 53.0);
}
#[test]
fn selector_validation_matches_hyprland_workspace_cpp() {
    let mut parts = fixture();
    // Native parsing rejects duplicate/conflicting flags and zero-based ranges,
    // but a single zero window count is valid. Malformed fullscreen isn't f[2].
    for selector in [
        "w[tf0]",
        "w[ft0]",
        "w[tt1]",
        "w[gg1]",
        "w[pp0]",
        "w[vv2]",
        "r[0-2]",
        "r[2-1]",
        "w[0-2]",
        "f[other]",
        "f[]",
        "f[2147483648]",
    ] {
        assert_rule(&mut parts, selector, "eDP-1", 53.0);
    }
    for selector in [
        "w[0]", "w[t0]", "f[2]", "s[0]", "s[false]", "n[0]", "n[bogus]",
    ] {
        assert_rule(&mut parts, selector, "DP-1", 80.0);
    }
    for selector in ["s[2]", "n[2]"] {
        assert_rule(&mut parts, selector, "DP-1", 82.0);
    }
    parts[3][0]["fullscreen"] = json!(2);
    assert_rule(&mut parts, "f[0]", "eDP-1", 53.0); // workspace has no fullscreen
    parts[1][0]["hasfullscreen"] = json!(true);
    for selector in ["f[0]", "f[+0]", "f[0suffix]"] {
        assert_rule(&mut parts, selector, "eDP-1", 51.0);
    }
    assert_rule(&mut parts, "f[-1]", "eDP-1", 53.0);
}

#[test]
fn named_special_monitor_and_malformed_selectors() {
    let mut parts = fixture();
    for selector in [
        "m[+1]",
        "m[-1]",
        "m[+3]",
        "m[r]",
        "m[1]",
        "m[desc:Acme Panel]",
    ] {
        assert_rule(&mut parts, selector, "DP-1", 80.0);
    }
    parts[2] = json!([{"workspaceString":"m[r]","gapsOut":[0,0,0,0]}]);
    parts[0][1]["x"] = json!(1538);
    assert_eq!(margins(&parts, "DP-1")["top"], 82.0);
    parts[0][1]["x"] = json!(1537);
    assert_eq!(margins(&parts, "DP-1")["top"], 80.0);
    for selector in [
        "r[1-4] junk",
        "w[bad]",
        "n[true]",
        "m[right]",
        "m[current]]",
        "m[current]w[2",
        "mm[current]",
    ] {
        assert_rule(&mut parts, selector, "eDP-1", 53.0);
    }
    parts[1]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":-1337,"name":"project:web","windows":0}));
    parts[0][0]["activeWorkspace"] = json!({"id":-1337});
    parts[2] = json!([{"workspaceString":"n[true] n[s:project:] n[e:web]","gapsOut":[1,2,3,4]},{"workspaceString":"name:project:web","gapsOut":[5,6,7,8]}]);
    assert_eq!(margins(&parts, "eDP-1")["top"], 56.0);
    parts[1]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":-99,"name":"special:notes"}));
    parts[0][0]["specialWorkspace"] = json!({"id":-99});
    assert_rule(&mut parts, "s[true]", "eDP-1", 51.0);
}
#[test]
fn directional_selection_keeps_first_tie_and_skips_disabled_monitors() {
    let mut parts = fixture();
    let mut other = parts[0][1].clone();
    other["name"] = json!("DP-2");
    other["id"] = json!(2);
    parts[0].as_array_mut().unwrap().push(other);
    parts[2] = json!([{"workspaceString":"m[r]","gapsOut":[0]}]);
    assert_eq!(margins(&parts, "DP-1")["top"], 80.0);
    assert_eq!(margins(&parts, "DP-2")["top"], 82.0);
    parts[0][1]["disabled"] = json!(true);
    assert_eq!(margins(&parts, "DP-2")["top"], 80.0);
    parts[0][0]["focused"] = json!(false);
    assert_eq!(margins(&parts, "DP-2")["top"], 82.0);
}

#[test]
fn directional_selection_normalizes_axes_after_rotation_and_scaling() {
    for (direction, x, y, transform) in [
        ("l", -1920, 0, 0),
        ("r", 1536, 0, 0),
        ("u", 0, -1080, 0),
        ("t", 0, -1080, 0),
        ("d", 0, 960, 0),
        ("b", 0, 960, 0),
        ("r", 960, 0, 1),
        ("u", 0, -1920, 1),
    ] {
        let mut parts = fixture();
        parts[0][0]["transform"] = json!(transform);
        parts[0][1]["transform"] = json!(transform);
        parts[0][1]["x"] = json!(x);
        parts[0][1]["y"] = json!(y);
        assert_rule(&mut parts, &format!("m[{direction}]"), "DP-1", 80.0);
    }
}

#[tokio::test]
#[ignore = "requires a running Hyprland session; read-only probe"]
async fn live_work_areas() {
    let areas = crate::Client::default().work_areas().await.unwrap();
    assert!(!areas.is_empty());
    assert!(areas.values().all(|value| {
        [value.left, value.top, value.right, value.bottom]
            .iter()
            .all(|v| v.is_finite())
    }));
}

#[tokio::test]
async fn native_batch_request_returns_normalized_work_areas() {
    let (_root, client, server) = crate::tests::command_server(
        "[[BATCH]]j/monitors;j/workspaces;j/workspacerules;j/clients;j/getoption general:gaps_out",
        text(&fixture()),
    )
    .await;
    assert_eq!(client.work_areas().await.unwrap()["eDP-1"].top, 53.0);
    server.await.unwrap();
}

#[test]
fn parses_adjacent_json_and_rejects_missing_or_invalid_documents() {
    let mut parts = fixture();
    parts[0][0]["description"] = json!("Display with } [ \" \\ and\nnewline");
    assert!(parse_work_areas(&text(&parts)).is_ok());
    for invalid in [
        "",
        "not running",
        "[] [] [] [] {}",
        "[null] [] [] [] {\"css\":\"2\"}",
    ] {
        assert!(parse_work_areas(invalid).is_err());
    }
    parts[0][0]["scale"] = json!(0);
    assert!(parse_work_areas(&text(&parts)).is_err());
    assert!(geometry_event("configreloaded>>"));
    assert!(geometry_event("togglegroup>>1"));
    assert!(!geometry_event("activewindow>>title"));
}
