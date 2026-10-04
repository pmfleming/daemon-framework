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
    let mut best = None;
    let mut intersection = -1.0_f64;
    for monitor in monitors
        .iter()
        .filter(|m| m.name != focused.name && !m.disabled)
    {
        let [tx, ty, tw, th] = monitor_box(monitor);
        let distance = match direction {
            "l" => x - tx - tw,
            "r" => x + w - tx,
            "u" | "t" => y - ty - th,
            _ => y + h - ty,
        };
        if distance.abs() >= 2.0 {
            continue;
        }
        let overlap = if matches!(direction, "l" | "r") {
            (y + h).min(ty + th) - y.max(ty)
        } else {
            (x + w).min(tx + tw) - x.max(tx)
        }
        .max(0.0);
        if overlap > intersection {
            best = Some(monitor);
            intersection = overlap;
        }
    }
    best
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
fn range(value: &str) -> Option<(i64, i64)> {
    let (low, high) = value.split_once('-').map_or((value, value), |pair| pair);
    let low = low.parse::<u32>().ok()? as i64;
    let high = high.parse::<u32>().ok()? as i64;
    Some((low, high))
}
fn term(
    kind: char,
    value: &str,
    workspace: &Workspace,
    monitor: &Monitor,
    snapshot: &Snapshot,
) -> bool {
    let boolean = matches!(value, "true" | "1" | "yes" | "on");
    match kind {
        'r' => {
            value.contains('-')
                && range(value).is_some_and(|(low, high)| (low..=high).contains(&workspace.id))
        }
        's' => (workspace.id < -1 && workspace.id > -1337) == boolean,
        'n' => match value.split_once(':') {
            Some(("s", prefix)) => workspace.name.starts_with(prefix),
            Some(("e", suffix)) => workspace.name.ends_with(suffix),
            _ => (workspace.id <= -1337) == boolean,
        },
        'm' => monitor_matches(value, monitor, &snapshot.monitors),
        'w' => {
            let split = value
                .find(|c: char| !"tfpgv".contains(c))
                .unwrap_or(value.len());
            range(&value[split..]).is_some_and(|(low, high)| {
                (low..=high)
                    .contains(&(window_count(&value[..split], workspace, &snapshot.clients) as i64))
            })
        }
        'f' => fullscreen_matches(value, workspace, &snapshot.clients),
        _ => false,
    }
}
fn fullscreen_matches(value: &str, workspace: &Workspace, clients: &[Window]) -> bool {
    let mode = match value {
        "-1" => return !workspace.hasfullscreen,
        "0" => 2,
        "1" => 1,
        _ => return true,
    };
    clients
        .iter()
        .any(|c| c.on(workspace) && c.fullscreen == mode)
}
fn matches(selector: &str, workspace: &Workspace, monitor: &Monitor, snapshot: &Snapshot) -> bool {
    let mut rest = selector.trim();
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
    while !rest.is_empty() {
        let Some(kind) = rest.chars().next() else {
            return false;
        };
        if !"rsnmwf".contains(kind) {
            return false;
        }
        let Some(body) = rest.get(1..).and_then(|s| s.strip_prefix('[')) else {
            return false;
        };
        let Some(end) = body.find(']') else {
            return false;
        };
        if !term(kind, &body[..end], workspace, monitor, snapshot) {
            return false;
        }
        rest = body[end + 1..].trim_start();
    }
    true
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
            let mut gaps = snapshot.gaps;
            for rule in &snapshot.rules {
                if let Some(value) = &rule.gaps
                    && matches(&rule.selector, workspace, monitor, snapshot)
                {
                    gaps = css(value).unwrap_or(gaps);
                }
            }
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
