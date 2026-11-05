//! Exact-session compositor primitives. Rust validates and encodes all input;
//! the small Lua IPC guard executes atomically on the compositor event loop.
pub use crate::execute;
use crate::{Client, Lua};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, Hash)]
pub struct Workspace {
    pub id: Option<i64>,
    pub monitor: String,
    pub windows: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "hasfullscreen")]
    pub has_fullscreen: bool,
    #[serde(default, rename = "lastwindowtitle")]
    pub last_window_title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Hash)]
pub struct WorkspaceId {
    pub id: Option<i64>,
}

/// Workspace identity across numbered and special workspaces. Hyprland 0.57
/// omits `id` for special workspaces; never interpret absence as workspace zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Hash)]
#[serde(from = "WorkspaceRefWire")]
pub struct WorkspaceRef {
    pub id: Option<i64>,
    pub address: Option<String>,
    pub name: String,
    pub special: bool,
}

#[derive(Deserialize)]
struct WorkspaceRefWire {
    id: Option<i64>,
    address: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
}

impl From<WorkspaceRefWire> for WorkspaceRef {
    fn from(value: WorkspaceRefWire) -> Self {
        let special = value.kind.as_deref() == Some("special")
            || value.name.starts_with("special:")
            || value
                .address
                .as_deref()
                .is_some_and(|s| s.starts_with("special:"))
            || value.id.is_some_and(|id| (-1336..-1).contains(&id));
        Self {
            id: value.id,
            address: value.address,
            name: value.name,
            special,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub focused: bool,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: WorkspaceId,
}

#[derive(Debug, Clone, Deserialize, Hash)]
pub struct Window {
    pub address: String,
    pub pid: u32,
    pub workspace: WorkspaceRef,
    #[serde(default = "mapped")]
    pub mapped: bool,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "focusHistoryID", default = "unfocused")]
    pub focus_rank: i64,
    #[serde(default)]
    pub floating: bool,
    #[serde(default)]
    pub fullscreen: i8,
    #[serde(default, rename = "class")]
    pub class: String,
    #[serde(default, rename = "initialClass")]
    pub initial_class: String,
}
impl Default for Window {
    fn default() -> Self {
        Self {
            address: String::new(),
            pid: 0,
            workspace: WorkspaceRef::default(),
            mapped: true,
            title: String::new(),
            focus_rank: i64::MAX,
            floating: false,
            fullscreen: 0,
            class: String::new(),
            initial_class: String::new(),
        }
    }
}

const fn mapped() -> bool {
    true
}
const fn unfocused() -> i64 {
    i64::MAX
}

/// One compositor turn, without attaching daemon policy or session journals.
#[derive(Debug, Clone)]
pub struct Status {
    pub workspaces: Vec<Workspace>,
    pub monitors: Vec<Monitor>,
    pub active_window: Option<Window>,
}

impl Status {
    pub fn parse(reply: &str) -> Result<Self> {
        let mut parts = serde_json::Deserializer::from_str(reply);
        let workspaces =
            Deserialize::deserialize(&mut parts).context(crate::IpcError::InvalidReply)?;
        let monitors =
            Deserialize::deserialize(&mut parts).context(crate::IpcError::InvalidReply)?;
        let active: serde_json::Value =
            Deserialize::deserialize(&mut parts).context(crate::IpcError::InvalidReply)?;
        parts.end().context(crate::IpcError::InvalidReply)?;
        let active_window = if active.is_null() || active.as_object().is_some_and(|v| v.is_empty())
        {
            None
        } else {
            Some(serde_json::from_value(active).context(crate::IpcError::InvalidReply)?)
        };
        Ok(Status {
            workspaces,
            monitors,
            active_window,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub session: String,
    pub monitors: Vec<Monitor>,
    pub workspaces: Vec<Workspace>,
    pub windows: Vec<Window>,
}

pub async fn snapshot(client: &Client) -> Result<Snapshot> {
    let session = client
        .instance_signature()?
        .context("compositor session is required")?
        .to_owned();
    // A batch is handled synchronously by the compositor; unavailable is an error.
    let reply = client
        .request_exact("[[BATCH]]j/monitors;j/workspaces;j/clients")
        .await?;
    parse_snapshot(session, &reply)
}

/// Capture weak window identities in the same compositor turn as their JSON
/// snapshot. An address reused after this turn cannot satisfy close_pinned.
pub async fn pinned_snapshot(
    client: &Client,
    monitor: &str,
    workspace: i64,
    prefix: &str,
) -> Result<Snapshot> {
    ensure!(workspace > 0, "ordinary workspace required");
    let session = client
        .instance_signature()?
        .context("compositor session required")?
        .to_owned();
    let prefix = token_literal(prefix)?;
    ensure!(
        crate::monitor::identifier(monitor),
        "invalid monitor identifier"
    );
    let monitor = quote(monitor)?;
    // No semicolons inside the expression: the IPC batch delimiter is ';'.
    let expression = format!(
        "local m=hl.get_monitor({monitor}) assert(m and m.active_workspace and m.active_workspace.id=={workspace},'stale monitor') local targets=hl.get_windows({{workspace={workspace}}}) assert(#targets<=512,'too many windows') __shelllist_window_refs=__shelllist_window_refs or {{}} for _,w in ipairs(targets) do local key={prefix}..'-'..w.address assert(__shelllist_window_refs[key]==nil,'duplicate token') __shelllist_window_refs[key]=w end"
    );
    client.capabilities().await?;
    let reply = client
        .request_exact(&format!(
            "[[BATCH]]eval {expression};j/monitors;j/workspaces;j/clients"
        ))
        .await?;
    let json = reply
        .strip_prefix("ok")
        .context("compositor did not pin close snapshot")?;
    parse_snapshot(session, json)
}

fn parse_snapshot(session: String, reply: &str) -> Result<Snapshot> {
    let mut parts = serde_json::Deserializer::from_str(reply);
    let monitors = Deserialize::deserialize(&mut parts).context(crate::IpcError::InvalidReply)?;
    let workspaces = Deserialize::deserialize(&mut parts).context(crate::IpcError::InvalidReply)?;
    let windows = Deserialize::deserialize(&mut parts).context(crate::IpcError::InvalidReply)?;
    parts.end().context(crate::IpcError::InvalidReply)?;
    Ok(Snapshot {
        session,
        monitors,
        workspaces,
        windows,
    })
}

pub fn quote(value: &str) -> Result<String> {
    ensure!(
        value.len() <= 256 && !value.chars().any(char::is_control),
        "invalid compositor identifier"
    );
    // JSON ASCII strings have the same escapes used here by Lua.
    Ok(serde_json::to_string(value)?)
}

pub fn guarded_close(address: &str, pid: u32, workspace: i64) -> Result<Lua> {
    let guard = window_guard(address, pid, workspace)?;
    Ok(Lua(format!(
        "{guard} local r=hl.dispatch(hl.dsp.window.close({{window=w}})); assert(r.ok,r.error)"
    )))
}

pub fn focus_window(address: &str, pid: u32, workspace: i64) -> Result<Lua> {
    let guard = window_guard(address, pid, workspace)?;
    Ok(Lua(format!(
        "{guard} local r=hl.dispatch(hl.dsp.focus({{window=w}})); assert(r.ok,r.error)"
    )))
}

/// Follow a launch only while the invoking monitor and workspace still own focus.
pub fn follow_window(
    address: &str,
    pid: u32,
    workspace: i64,
    monitor: &str,
    origin: i64,
) -> Result<Lua> {
    ensure!(origin > 0, "invalid origin");
    let monitor = quote(monitor)?;
    let focus = focus_window(address, pid, workspace)?;
    Ok(Lua(format!(
        "local m=hl.get_active_monitor(); assert(m and m.name=={monitor} and m.active_workspace and m.active_workspace.id=={origin},'focus intent expired'); {focus}"
    )))
}

/// Pin a weak compositor window reference; unlike an address it cannot identify
/// a subsequently allocated window from the same process. The caller owns token
/// quotas, lifetime and journal admission. This table contains no routing policy.
pub fn pin_window(token: &str, address: &str, pid: u32, workspace: i64) -> Result<Lua> {
    let token = token_literal(token)?;
    let guard = window_guard(address, pid, workspace)?;
    Ok(Lua(format!(
        "{guard} __shelllist_window_refs=__shelllist_window_refs or {{}}; assert(__shelllist_window_refs[{token}]==nil,'duplicate token'); __shelllist_window_refs[{token}]=w"
    )))
}

pub fn close_pinned(token: &str, address: &str, pid: u32, workspace: i64) -> Result<Lua> {
    let token = token_literal(token)?;
    let guard = window_guard(address, pid, workspace)?;
    Ok(Lua(format!(
        "{guard} assert(__shelllist_window_refs and __shelllist_window_refs[{token}]==w,'expired window'); __shelllist_window_refs[{token}]=nil; local r=hl.dispatch(hl.dsp.window.close({{window=w}})); assert(r.ok,r.error)"
    )))
}

pub fn release_window(token: &str) -> Result<Lua> {
    let token = token_literal(token)?;
    Ok(Lua(format!(
        "if __shelllist_window_refs then __shelllist_window_refs[{token}]=nil end"
    )))
}

/// Release only tokens in the caller-owned namespace. No window is moved or
/// closed; callers must fence old journal observers before admitting new pins.
pub fn release_prefix(prefix: &str) -> Result<Lua> {
    let prefix = token_literal(prefix)?;
    Ok(Lua(format!(
        "if __shelllist_window_refs then for key in pairs(__shelllist_window_refs) do if type(key)=='string' and key:sub(1,#{prefix})=={prefix} then __shelllist_window_refs[key]=nil end end end"
    )))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedState {
    Present,
    Gone,
    Moved,
    Lost,
}

/// Observe the exact pinned window, including when its address has been reused.
/// Unknown IPC failures remain errors, never evidence that a window disappeared.
pub async fn pinned_state(
    client: &Client,
    token: &str,
    address: &str,
    pid: u32,
    workspace: i64,
) -> Result<PinnedState> {
    let token = token_literal(token)?;
    window_guard(address, pid, workspace)?;
    let expression = format!(
        "assert(__shelllist_window_refs and __shelllist_window_refs[{token}]~=nil,'shelllist-handoff-lost'); local w=hl.get_window('address:{}'); assert(w and w.mapped and __shelllist_window_refs[{token}]==w,'shelllist-handoff-gone'); assert(w.pid=={} and w.workspace and w.workspace.id=={},'shelllist-handoff-moved')",
        address, pid, workspace
    );
    client.capabilities().await?;
    let reply = client.request_exact(&format!("eval {expression}")).await?;
    match crate::ack(&reply) {
        Ok(()) => Ok(PinnedState::Present),
        Err(error) => match error.downcast_ref::<crate::IpcError>() {
            Some(crate::IpcError::Rejected(reply)) if reply.contains("shelllist-handoff-gone") => {
                Ok(PinnedState::Gone)
            }
            Some(crate::IpcError::Rejected(reply)) if reply.contains("shelllist-handoff-moved") => {
                Ok(PinnedState::Moved)
            }
            Some(crate::IpcError::Rejected(reply)) if reply.contains("shelllist-handoff-lost") => {
                Ok(PinnedState::Lost)
            }
            _ => Err(error.context("cannot observe pinned window")),
        },
    }
}

/// A successor may move only after the pinned predecessor has disappeared.
/// Refresh the weak reference atomically with the move; never follow focus.
pub fn replace_pinned(
    token: &str,
    previous_address: &str,
    candidate: &Window,
    destination: i64,
    monitor: &str,
) -> Result<Lua> {
    let token = token_literal(token)?;
    window_guard(previous_address, candidate.pid, destination)?;
    let movement = move_local(
        &candidate.address,
        candidate.pid,
        candidate
            .workspace
            .id
            .context("ordinary workspace required")?,
        destination,
        monitor,
    )?;
    Ok(Lua(format!(
        "local old=hl.get_window('address:{previous_address}'); assert(__shelllist_window_refs and __shelllist_window_refs[{token}]~=nil and (not old or not old.mapped or __shelllist_window_refs[{token}]~=old),'predecessor still mapped or identity lost'); {movement}; __shelllist_window_refs[{token}]=w"
    )))
}

fn token_literal(token: &str) -> Result<String> {
    ensure!(
        !token.is_empty()
            && token.len() <= 128
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
        "invalid window token"
    );
    quote(token)
}

/// Select a destination, optionally moving the currently focused window with it.
/// The owner decides whether an absent compositor object may be materialized.
/// Check existence again in the effect turn, so snapshot races cannot create it.
pub fn select_local(
    monitor: &str,
    expected: i64,
    destination: i64,
    move_window: bool,
    materialize: bool,
) -> Result<Lua> {
    ensure!(
        expected > 0 && destination > 0 && destination <= i32::MAX as i64,
        "invalid workspace"
    );
    let monitor = quote(monitor)?;
    let existence = if materialize {
        ""
    } else {
        "assert(target,'destination disappeared');"
    };
    let action = if move_window {
        format!(
            "local w=hl.get_active_window(); assert(w and w.mapped and w.workspace and w.workspace.id=={expected},'focused window changed'); local r=hl.dispatch(hl.dsp.window.move({{window=w,workspace={destination},follow=true}})); assert(r.ok,r.error)"
        )
    } else {
        format!(
            "local r=hl.dispatch(hl.dsp.focus({{workspace={destination}}})); assert(r.ok,r.error)"
        )
    };
    Ok(Lua(format!(
        "local m=hl.get_active_monitor(); assert(m and m.name=={monitor} and m.active_workspace and m.active_workspace.id=={expected},'focus context changed'); local target=hl.get_workspace({destination}); {existence} assert(not target or target.monitor==m,'foreign destination'); {action}"
    )))
}

/// Focus/create on an explicit monitor without moving another monitor's workspace.
/// expected guards the captured local focus; background replacement restores the
/// previously focused monitor/window within the same compositor event turn.
pub fn focus_local(monitor: &str, expected: i64, destination: i64, follow: bool) -> Result<Lua> {
    ensure!(
        expected > 0 && destination > 0 && destination <= i32::MAX as i64,
        "invalid workspace"
    );
    let monitor = quote(monitor)?;
    let restore = if follow {
        ""
    } else {
        "if previous then local r=hl.dispatch(hl.dsp.focus({monitor=previous})); assert(r.ok,r.error) end; if active and active.mapped then local r=hl.dispatch(hl.dsp.focus({window=active})); assert(r.ok,r.error) end"
    };
    Ok(Lua(format!(
        "local m=hl.get_monitor({monitor}); assert(m and m.active_workspace and m.active_workspace.id=={expected},'stale monitor'); local target=hl.get_workspace({destination}); assert(not target or target.monitor==m,'workspace belongs to another monitor'); local previous=hl.get_active_monitor(); local active=hl.get_active_window(); local r=hl.dispatch(hl.dsp.focus({{monitor=m}})); assert(r.ok,r.error); r=hl.dispatch(hl.dsp.focus({{workspace={destination}}})); assert(r.ok,r.error); {restore}"
    )))
}

pub fn guarded_move(address: &str, pid: u32, source: i64, destination: i64) -> Result<Lua> {
    ensure!(
        destination > 0 && destination <= i32::MAX as i64,
        "invalid destination"
    );
    let guard = window_guard(address, pid, source)?;
    Ok(Lua(format!(
        "{guard} local r=hl.dispatch(hl.dsp.window.move({{window=w,workspace={destination},follow=false}})); assert(r.ok,r.error)"
    )))
}

/// Materialize and populate a reserved destination in one compositor turn, before
/// an empty background workspace can be reclaimed. Restore prior focus afterward.
pub fn move_local(
    address: &str,
    pid: u32,
    source: i64,
    destination: i64,
    monitor: &str,
) -> Result<Lua> {
    let movement = guarded_move(address, pid, source, destination)?;
    let monitor = quote(monitor)?;
    Ok(Lua(format!(
        "local m=hl.get_monitor({monitor}); assert(m,'monitor disappeared'); local target=hl.get_workspace({destination}); assert(not target or target.monitor==m,'foreign destination'); local previous=hl.get_active_monitor(); local active=hl.get_active_window(); local r=hl.dispatch(hl.dsp.focus({{monitor=m}})); assert(r.ok,r.error); {movement}; if previous then r=hl.dispatch(hl.dsp.focus({{monitor=previous}})); assert(r.ok,r.error) end; if active and active~=w and active.mapped then r=hl.dispatch(hl.dsp.focus({{window=active}})); assert(r.ok,r.error) end"
    )))
}

fn window_guard(address: &str, pid: u32, workspace: i64) -> Result<String> {
    let address = crate::WindowAddress::parse(address)?;
    ensure!(pid > 0, "invalid window identity");
    ensure!(workspace > 0, "ordinary workspace required");
    Ok(format!(
        "local w=hl.get_window('address:{address}'); assert(w and w.mapped and w.pid=={pid} and w.workspace and w.workspace.id=={workspace}, 'stale window');"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_is_one_batch_and_empty_active_window_is_absent() {
        let reply = r#"[{"id":3,"monitor":"DP-1","windows":1,"name":"three","hasfullscreen":true,"lastwindowtitle":"title"}]
            [{"id":0,"name":"DP-1","focused":true,"activeWorkspace":{"id":3}}]
            {}"#;
        let (_root, client, server) =
            crate::tests::command_server("[[BATCH]]j/workspaces;j/monitors;j/activewindow", reply)
                .await;
        let status = client.status().await.unwrap();
        assert!(status.active_window.is_none());
        assert_eq!(status.workspaces[0].name, "three");
        assert!(status.workspaces[0].has_fullscreen);
        assert_eq!(status.workspaces[0].last_window_title, "title");
        server.await.unwrap();
        let window: Window = serde_json::from_str(
            r#"{"address":"not-a-selector","pid":42,"workspace":{"id":3,"name":"3"}}"#,
        )
        .unwrap();
        assert_eq!(window.address, "not-a-selector"); // consumer filters, not snapshot-wide failure
        assert_eq!(window.focus_rank, i64::MAX);
        assert_eq!(Window::default().focus_rank, i64::MAX);
        assert!(window.mapped);
        assert!(
            serde_json::from_str::<Window>(r#"{"address":"0x123","workspace":{"id":3}}"#).is_err()
        );
        assert!(Status::parse("[] [] {\"address\":\"0x123\"}").is_err());
        assert!(Status::parse("[] [] {} trailing").is_err());
    }

    #[tokio::test]
    async fn pin_snapshot_rejects_batch_delimiters_before_connecting() {
        let client = Client::new("/unused".into(), Some("test".into()));
        let error = pinned_snapshot(&client, "DP;j/x", 6, "probe")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid monitor identifier"));
    }

    #[test]
    fn captured_snapshots_and_work_areas_accept_both_workspace_shapes() {
        for (version, special, text) in [
            (
                "0.57.1",
                false,
                include_str!("../tests/fixtures/0.57.1/regular.json"),
            ),
            (
                "0.57.1",
                true,
                include_str!("../tests/fixtures/0.57.1/special.json"),
            ),
            (
                "0.57.1",
                false,
                include_str!("../tests/fixtures/0.57.1/deployed.json"),
            ),
        ] {
            let fixture: serde_json::Value = serde_json::from_str(text).unwrap();
            let batch = |keys: &[&str]| {
                keys.iter()
                    .map(|key| fixture[key].to_string())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            let snapshot = parse_snapshot(
                "fixture".into(),
                &batch(&["monitors", "workspaces", "clients"]),
            )
            .unwrap();
            let window = &snapshot.windows[0];
            assert_eq!(window.workspace.special, special);
            if special && version == "0.57.1" {
                assert_eq!(window.workspace.id, None);
                assert_eq!(
                    window.workspace.address.as_deref(),
                    Some("special:ipc-probe")
                );
            } else {
                assert!(window.workspace.id.is_some());
            }
            let areas = crate::work_area::parse_work_areas(&batch(&[
                "monitors",
                "workspaces",
                "workspacerules",
                "clients",
                "gaps",
            ]))
            .unwrap();
            assert_eq!(areas.len(), snapshot.monitors.len());
        }
    }

    #[test]
    fn selection_checks_existence_before_dispatch_and_guards_focus() {
        for moving in [false, true] {
            let guarded = select_local("DP-1", 1, 6, moving, false).unwrap();
            assert!(guarded.find("assert(target,").unwrap() < guarded.find("hl.dispatch").unwrap());
            assert!(guarded.contains("focus context changed"));
            assert!(guarded.contains("foreign destination"));
            assert!(
                !select_local("DP-1", 1, 6, moving, true)
                    .unwrap()
                    .contains("assert(target,")
            );
        }
        assert!(select_local("DP-1", 0, 6, true, false).is_err());
    }
    #[test]
    fn close_never_falls_back_to_active_window_or_accepts_code() {
        for address in ["", "active", "0x", "0x1');evil()", "0xé"] {
            assert!(guarded_close(address, 1, 6).is_err());
        }
        assert!(guarded_close("0xabc", 0, 6).is_err());
        assert!(guarded_close("0xabc", 1, -1).is_err());
        let command = guarded_close("0xabc", 42, 6).unwrap();
        assert!(command.contains("w.pid==42 and w.workspace and w.workspace.id==6"));
        assert!(command.ends_with("assert(r.ok,r.error)"));
        assert!(guarded_move("0xabc", 42, 6, 0).is_err());
    }
}
