//! Hyprland workspace-rule interpretation. Output is logical insets, never pixels.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Insets {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WorkspaceRef {
    id: i64,
}
#[derive(Debug, Deserialize)]
struct Monitor {
    id: i64,
    name: String,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    disabled: bool,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
    #[serde(default)]
    transform: u8,
    #[serde(default)]
    description: String,
    #[serde(default)]
    make: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    serial: String,
    #[serde(default)]
    reserved: Option<[f64; 4]>,
    #[serde(default, rename = "activeWorkspace")]
    active: WorkspaceRef,
    #[serde(default, rename = "specialWorkspace")]
    special: WorkspaceRef,
}
#[derive(Debug, Deserialize)]
struct Workspace {
    id: i64,
    name: String,
    #[serde(default)]
    windows: usize,
    #[serde(default)]
    hasfullscreen: bool,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Window {
    workspace: Option<WorkspaceRef>,
    mapped: bool,
    floating: bool,
    pinned: bool,
    hidden: bool,
    visible: Option<bool>,
    grouped: Vec<String>,
    fullscreen: i64,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rule {
    #[serde(rename = "workspaceString")]
    selector: String,
    #[serde(rename = "gapsOut")]
    gaps: Option<Value>,
}
struct Snapshot {
    monitors: Vec<Monitor>,
    workspaces: Vec<Workspace>,
    rules: Vec<Rule>,
    clients: Vec<Window>,
    gaps: [f64; 4],
}

fn css(value: &Value) -> Option<[f64; 4]> {
    let parts: Vec<f64> = if let Some(text) = value.as_str() {
        text.split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?
    } else {
        value
            .as_array()?
            .iter()
            .map(Value::as_f64)
            .collect::<Option<_>>()?
    };
    if !(1..=4).contains(&parts.len()) || parts.iter().any(|value| !value.is_finite()) {
        return None;
    }
    Some([
        parts[0],
        *parts.get(1).unwrap_or(&parts[0]),
        *parts.get(2).unwrap_or(&parts[0]),
        *parts.get(3).or(parts.get(1)).unwrap_or(&parts[0]),
    ])
}
fn parse(text: &str) -> Result<Snapshot> {
    ensure!(
        text.len() <= 16 * 1024 * 1024,
        "Hyprland geometry reply too large"
    );
    let mut docs = serde_json::Deserializer::from_str(text)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()?;
    ensure!(docs.len() == 5, "Incomplete Hyprland geometry snapshot");
    let gaps =
        css(&docs[4]["css"]).ok_or_else(|| anyhow::anyhow!("Missing Hyprland outer gaps"))?;
    let clients = serde_json::from_value(docs[3].take())?;
    let rules = serde_json::from_value(docs[2].take())?;
    let workspaces = serde_json::from_value(docs[1].take())?;
    let monitors: Vec<Monitor> = serde_json::from_value(docs[0].take())?;
    ensure!(
        monitors.iter().all(|m| m.scale.is_finite()
            && m.scale > 0.0
            && [m.x, m.y, m.width, m.height].iter().all(|v| v.is_finite())),
        "Invalid Hyprland monitor geometry"
    );
    Ok(Snapshot {
        monitors,
        workspaces,
        rules,
        clients,
        gaps,
    })
}
fn monitor_box(m: &Monitor) -> [f64; 4] {
    let (width, height) = if m.transform % 2 == 0 {
        (m.width, m.height)
    } else {
        (m.height, m.width)
    };
    [
        m.x,
        m.y,
        (width / m.scale).round(),
        (height / m.scale).round(),
    ]
}
fn directional<'a>(direction: &str, monitors: &'a [Monitor]) -> Option<&'a Monitor> {
    let focused = monitors.iter().find(|m| m.focused)?;
    let [x, y, w, h] = monitor_box(focused);
    monitors
        .iter()
        .filter(|m| m.name != focused.name && !m.disabled)
        .filter_map(|monitor| {
            let [tx, ty, tw, th] = monitor_box(monitor);
            let distance = match direction {
                "l" => x - tx - tw,
                "r" => x + w - tx,
                "u" | "t" => y - ty - th,
                _ => y + h - ty,
            };
            if distance.abs() >= 2.0 {
                return None;
            }
            let overlap = if matches!(direction, "l" | "r") {
                (y + h).min(ty + th) - y.max(ty)
            } else {
                (x + w).min(tx + tw) - x.max(tx)
            }
            .max(0.0);
            Some((monitor, overlap))
        })
        // min_by retains the first equal candidate, unlike max_by.
        .min_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(monitor, _)| monitor)
}
fn monitor_matches(selector: &str, monitor: &Monitor, monitors: &[Monitor]) -> bool {
    if selector == "current" {
        return monitor.focused;
    }
    if matches!(selector, "l" | "r" | "u" | "d" | "t" | "b") {
        return directional(selector, monitors).is_some_and(|m| m.name == monitor.name);
    }
    if selector.starts_with(['+', '-']) {
        let Some(index) = monitors.iter().position(|m| m.focused) else {
            return false;
        };
        let Ok(delta) = selector.parse::<i64>() else {
            return false;
        };
        let length = monitors.len() as i64;
        return monitors[((index as i64 + delta % length + length) % length) as usize].name
            == monitor.name;
    }
    if let Ok(id) = selector.parse::<u64>() {
        return monitor.id >= 0 && monitor.id as u64 == id;
    }
    if let Some(description) = selector.strip_prefix("desc:") {
        let short = format!("{} {} {}", monitor.make, monitor.model, monitor.serial);
        return monitor.description.starts_with(description.trim())
            || short.trim().starts_with(description.trim());
    }
    monitor.name == selector
}
impl Window {
    fn on(&self, workspace: &Workspace) -> bool {
        self.workspace
            .as_ref()
            .is_some_and(|w| w.id == workspace.id)
    }

    fn matches_flags(&self, flags: &str) -> bool {
        flags.chars().all(|flag| match flag {
            't' => !self.floating,
            'f' => self.floating,
            'p' => self.pinned,
            'v' => !self.hidden && self.visible != Some(false),
            _ => true, // Group deduplication is handled by window_count.
        })
    }
}

fn window_count(flags: &str, workspace: &Workspace, clients: &[Window]) -> usize {
    if flags.is_empty() {
        return workspace.windows;
    }
    let mut groups = HashSet::new();
    clients
        .iter()
        .filter(|c| c.on(workspace) && c.mapped && c.matches_flags(flags))
        .filter(|c| {
            if !flags.contains('g') {
                return true;
            }
            if c.grouped.is_empty() {
                return false;
            }
            let mut group: Vec<_> = c.grouped.iter().collect();
            group.sort();
            groups.insert(group)
        })
        .count()
}
fn range(value: &str) -> Option<std::ops::RangeInclusive<i64>> {
    if let Some((low, high)) = value.split_once('-') {
        let (low, high) = (low.parse::<i64>().ok()?, high.parse::<i64>().ok()?);
        return (low >= 1 && high >= low).then_some(low..=high);
    }
    let count = value.parse::<i64>().ok()?;
    Some(count..=count)
}

// Hyprland consumes each window flag once; t/f are mutually exclusive. Leaving
// an invalid flag in the numeric suffix makes the whole selector fail.
fn window_selector(value: &str) -> (&str, &str) {
    let mut seen = 0_u8;
    let split = value
        .find(|flag| {
            let bit = match flag {
                't' | 'f' => 1,
                'p' => 2,
                'g' => 4,
                'v' => 8,
                _ => return true,
            };
            let duplicate = seen & bit != 0;
            seen |= bit;
            duplicate
        })
        .unwrap_or(value.len());
    value.split_at(split)
}

fn selector_integer(value: &str) -> Option<i64> {
    for (prefixes, number) in [(["true", "on", "yes"], 1), (["false", "off", "no"], 0)] {
        if prefixes.iter().any(|prefix| value.starts_with(prefix)) {
            return Some(number);
        }
    }
    match value.strip_prefix("0x") {
        Some(hex) => i64::from_str_radix(hex, 16).ok(),
        None => value.parse().ok(),
    }
}
fn term(
    kind: char,
    value: &str,
    workspace: &Workspace,
    monitor: &Monitor,
    snapshot: &Snapshot,
) -> bool {
    let integer = selector_integer(value);
    match kind {
        'r' => {
            value.contains('-') && range(value).is_some_and(|range| range.contains(&workspace.id))
        }
        's' => {
            integer.is_none_or(|value| (workspace.id < -1 && workspace.id > -1337) == (value != 0))
        }
        'n' => match value.split_once(':') {
            Some(("s", prefix)) => workspace.name.starts_with(prefix),
            Some(("e", suffix)) => workspace.name.ends_with(suffix),
            _ => integer.is_none_or(|value| i64::from(workspace.id <= -1337) == value),
        },
        'm' => monitor_matches(value, monitor, &snapshot.monitors),
        'w' => {
            let (flags, count) = window_selector(value);
            range(count).is_some_and(|range| {
                range.contains(&(window_count(flags, workspace, &snapshot.clients) as i64))
            })
        }
        'f' => fullscreen_matches(value, workspace, &snapshot.clients),
        _ => false,
    }
}
fn fullscreen_matches(value: &str, workspace: &Workspace, clients: &[Window]) -> bool {
    // C++ stoi accepts a signed decimal prefix but rejects missing/overflowing
    // numbers. Unknown *numeric* modes are deliberately unconstrained upstream.
    let value = value.trim_start();
    let end = value
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *i == 0 && matches!(c, '+' | '-')))
        .map_or(value.len(), |(i, _)| i);
    let mode = match value[..end].parse::<i32>() {
        Ok(-1) => return !workspace.hasfullscreen,
        Ok(0) => 2,
        Ok(1) => 1,
        Ok(_) => return true,
        Err(_) => return false,
    };
    workspace.hasfullscreen
        && clients
            .iter()
            .any(|c| c.on(workspace) && c.fullscreen == mode)
}
fn matches(selector: &str, workspace: &Workspace, monitor: &Monitor, snapshot: &Snapshot) -> bool {
    let rest = selector.trim();
    if rest.is_empty() {
        return true;
    }
    if let Ok(id) = rest.parse::<i64>() {
        return workspace.id == id;
    }
    if let Some(name) = rest.strip_prefix("name:") {
        return workspace.name == name;
    }
    if rest.starts_with("special") {
        return workspace.name == rest;
    }
    let Some(clauses) = rest.strip_suffix(']') else {
        return false;
    };
    clauses.split(']').all(|clause| {
        let Some((kind, value)) = clause.trim_start().split_once('[') else {
            return false;
        };
        let [kind] = kind.as_bytes() else {
            return false;
        };
        term(char::from(*kind), value, workspace, monitor, snapshot)
    })
}
fn insets(snapshot: &Snapshot) -> BTreeMap<String, Insets> {
    snapshot
        .monitors
        .iter()
        .filter(|m| !m.disabled)
        .filter_map(|monitor| {
            let reserved = monitor.reserved?;
            if reserved.iter().any(|v| !v.is_finite()) {
                return None;
            }
            let active = if monitor.special.id != 0 {
                monitor.special.id
            } else {
                monitor.active.id
            };
            let workspace = snapshot.workspaces.iter().find(|w| w.id == active)?;
            let gaps = snapshot
                .rules
                .iter()
                .rev()
                .find_map(|rule| {
                    let gaps = css(rule.gaps.as_ref()?)?;
                    matches(&rule.selector, workspace, monitor, snapshot).then_some(gaps)
                })
                .unwrap_or(snapshot.gaps);
            Some((
                monitor.name.clone(),
                Insets {
                    left: reserved[0] + gaps[3],
                    top: reserved[1] + gaps[0],
                    right: reserved[2] + gaps[1],
                    bottom: reserved[3] + gaps[2],
                },
            ))
        })
        .collect()
}
pub fn parse_work_areas(text: &str) -> Result<BTreeMap<String, Insets>> {
    Ok(insets(&parse(text)?))
}
pub fn geometry_event(event: &str) -> bool {
    [
        "workspace",
        "focusedmon",
        "monitor",
        "configreloaded",
        "openlayer",
        "closelayer",
        "openwindow",
        "closewindow",
        "movewindow",
        "changefloatingmode",
        "fullscreen",
        "activespecial",
        "moveworkspace",
        "renameworkspace",
        "createworkspace",
        "destroyworkspace",
        "togglegroup",
        "moveintogroup",
        "moveoutofgroup",
        "pin",
    ]
    .iter()
    .any(|prefix| event.starts_with(prefix))
}
