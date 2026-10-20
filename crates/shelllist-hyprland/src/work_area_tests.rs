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
#[test]
fn logical_reservations_css_gaps_and_ordered_rules() {
    let mut parts = fixture();
    assert_margins(&parts, "eDP-1", [2.0, 53.0, 2.0, 2.0]);
    parts[2] = json!([{"workspaceString":"r[1-4]","gapsOut":[9,10,11,12]},{"workspaceString":"1","gapsOut":[3,4,5,6]},{"workspaceString":"1","borderSize":10},{"workspaceString":"1","gapsOut":"invalid"},{"workspaceString":"3","gapsOut":[7]}]);
    assert_margins(&parts, "eDP-1", [6.0, 54.0, 4.0, 5.0]);
    assert_margins(&parts, "DP-1", [31.0, 87.0, 17.0, 27.0]);
    for invalid in [
        Value::Null,
        json!(false),
        json!({"top": 1}),
        json!([]),
        json!([1, null]),
        json!([1, 2, 3, 4, 5]),
        json!("bad"),
        json!("NaN"),
    ] {
        parts[2] = json!([
            {"workspaceString":"1", "gapsOut":"3 4 5 6"},
            {"workspaceString":"1", "gapsOut":invalid}
        ]);
        assert_margins(&parts, "eDP-1", [6.0, 54.0, 4.0, 5.0]);
        assert_margins(&parts, "DP-1", [26.0, 82.0, 12.0, 22.0]);
    }
    parts[2] = json!([]);
    parts[4]["css"] = json!("4 8 12 16");
    parts[0][0]["reserved"] = json!([7, 92, 13, 28]);
    assert_margins(&parts, "eDP-1", [23.0, 96.0, 21.0, 40.0]);
    assert!(margins(&parts, "missing").is_null());
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
    for invalid in ["[] [] [] [] {}", "[] [] [] [] {\"css\":\"2\"} {}"] {
        assert!(parse_work_areas(invalid).is_err());
    }
    let mut parts = fixture();
    parts[0][0]["scale"] = json!(0);
    assert!(parse_work_areas(&text(&parts)).is_err());
    assert!(geometry_event("configreloaded>>"));
    assert!(geometry_event("togglegroup>>1"));
    assert!(!geometry_event("activewindow>>title"));
}
