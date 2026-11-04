//! Exact-session compositor primitives. Rust validates and encodes all input;
//! the small Lua IPC guard executes atomically on the compositor event loop.
use crate::Client;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub id: i64,
    pub monitor: String,
    pub windows: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceId {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub focused: bool,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: WorkspaceId,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Window {
    pub address: String,
    pub pid: u32,
    pub workspace: WorkspaceId,
    pub mapped: bool,
    #[serde(default, rename = "class")]
    pub class: String,
    #[serde(default, rename = "initialClass")]
    pub initial_class: String,
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
    let monitor = quote(monitor)?;
    // No semicolons inside the expression: the IPC batch delimiter is ';'.
    let expression = format!(
        "local m=hl.get_monitor({monitor}) assert(m and m.active_workspace and m.active_workspace.id=={workspace},'stale monitor') local targets=hl.get_windows({{workspace={workspace}}}) assert(#targets<=512,'too many windows') __shelllist_window_refs=__shelllist_window_refs or {{}} for _,w in ipairs(targets) do local key={prefix}..'-'..w.address assert(__shelllist_window_refs[key]==nil,'duplicate token') __shelllist_window_refs[key]=w end"
    );
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
    let monitors = Deserialize::deserialize(&mut parts)?;
    let workspaces = Deserialize::deserialize(&mut parts)?;
    let windows = Deserialize::deserialize(&mut parts)?;
    parts.end()?;
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

pub async fn execute(client: &Client, expression: &str) -> Result<()> {
    let reply = client.request_exact(&format!("eval {expression}")).await?;
    ensure!(
        reply.trim() == "ok",
        "compositor rejected command: {}",
        reply.trim()
    );
    Ok(())
}

pub fn guarded_close(address: &str, pid: u32, workspace: i64) -> Result<String> {
    let guard = window_guard(address, pid, workspace)?;
    Ok(format!(
        "{guard} local r=hl.dispatch(hl.dsp.window.close({{window=w}})); assert(r.ok,r.error)"
    ))
}

pub fn focus_window(address: &str, pid: u32, workspace: i64) -> Result<String> {
    let guard = window_guard(address, pid, workspace)?;
    Ok(format!(
        "{guard} local r=hl.dispatch(hl.dsp.focus({{window=w}})); assert(r.ok,r.error)"
    ))
}

/// Follow a launch only while the invoking monitor and workspace still own focus.
pub fn follow_window(
    address: &str,
    pid: u32,
    workspace: i64,
    monitor: &str,
    origin: i64,
) -> Result<String> {
    ensure!(origin > 0, "invalid origin");
    let monitor = quote(monitor)?;
    let focus = focus_window(address, pid, workspace)?;
    Ok(format!(
        "local m=hl.get_active_monitor(); assert(m and m.name=={monitor} and m.active_workspace and m.active_workspace.id=={origin},'focus intent expired'); {focus}"
    ))
}

/// Pin a weak compositor window reference; unlike an address it cannot identify
/// a subsequently allocated window from the same process. The caller owns token
/// quotas, lifetime and journal admission. This table contains no routing policy.
pub fn pin_window(token: &str, address: &str, pid: u32, workspace: i64) -> Result<String> {
    let token = token_literal(token)?;
    let guard = window_guard(address, pid, workspace)?;
    Ok(format!(
        "{guard} __shelllist_window_refs=__shelllist_window_refs or {{}}; assert(__shelllist_window_refs[{token}]==nil,'duplicate token'); __shelllist_window_refs[{token}]=w"
    ))
}

pub fn close_pinned(token: &str, address: &str, pid: u32, workspace: i64) -> Result<String> {
    let token = token_literal(token)?;
    let guard = window_guard(address, pid, workspace)?;
    Ok(format!(
        "{guard} assert(__shelllist_window_refs and __shelllist_window_refs[{token}]==w,'expired window'); __shelllist_window_refs[{token}]=nil; local r=hl.dispatch(hl.dsp.window.close({{window=w}})); assert(r.ok,r.error)"
    ))
}

pub fn release_window(token: &str) -> Result<String> {
    let token = token_literal(token)?;
    Ok(format!(
        "if __shelllist_window_refs then __shelllist_window_refs[{token}]=nil end"
    ))
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
    let reply = client.request_exact(&format!("eval {expression}")).await?;
    if reply.trim() == "ok" {
        Ok(PinnedState::Present)
    } else if reply.contains("shelllist-handoff-gone") {
        Ok(PinnedState::Gone)
    } else if reply.contains("shelllist-handoff-moved") {
        Ok(PinnedState::Moved)
    } else if reply.contains("shelllist-handoff-lost") {
        Ok(PinnedState::Lost)
    } else {
        anyhow::bail!("cannot observe pinned window: {}", reply.trim())
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
) -> Result<String> {
    let token = token_literal(token)?;
    window_guard(previous_address, candidate.pid, destination)?;
    let movement = move_local(
        &candidate.address,
        candidate.pid,
        candidate.workspace.id,
        destination,
        monitor,
    )?;
    Ok(format!(
        "local old=hl.get_window('address:{previous_address}'); assert(__shelllist_window_refs and __shelllist_window_refs[{token}]~=nil and (not old or not old.mapped or __shelllist_window_refs[{token}]~=old),'predecessor still mapped or identity lost'); {movement}; __shelllist_window_refs[{token}]=w"
    ))
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
) -> Result<String> {
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
    Ok(format!(
        "local m=hl.get_active_monitor(); assert(m and m.name=={monitor} and m.active_workspace and m.active_workspace.id=={expected},'focus context changed'); local target=hl.get_workspace({destination}); {existence} assert(not target or target.monitor==m,'foreign destination'); {action}"
    ))
}

/// Focus/create on an explicit monitor without moving another monitor's workspace.
/// expected guards the captured local focus; background replacement restores the
/// previously focused monitor/window within the same compositor event turn.
pub fn focus_local(monitor: &str, expected: i64, destination: i64, follow: bool) -> Result<String> {
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
    Ok(format!(
        "local m=hl.get_monitor({monitor}); assert(m and m.active_workspace and m.active_workspace.id=={expected},'stale monitor'); local target=hl.get_workspace({destination}); assert(not target or target.monitor==m,'workspace belongs to another monitor'); local previous=hl.get_active_monitor(); local active=hl.get_active_window(); local r=hl.dispatch(hl.dsp.focus({{monitor=m}})); assert(r.ok,r.error); r=hl.dispatch(hl.dsp.focus({{workspace={destination}}})); assert(r.ok,r.error); {restore}"
    ))
}

pub fn guarded_move(address: &str, pid: u32, source: i64, destination: i64) -> Result<String> {
    ensure!(
        destination > 0 && destination <= i32::MAX as i64,
        "invalid destination"
    );
    let guard = window_guard(address, pid, source)?;
    Ok(format!(
        "{guard} local r=hl.dispatch(hl.dsp.window.move({{window=w,workspace={destination},follow=false}})); assert(r.ok,r.error)"
    ))
}

/// Materialize and populate a reserved destination in one compositor turn, before
/// an empty background workspace can be reclaimed. Restore prior focus afterward.
pub fn move_local(
    address: &str,
    pid: u32,
    source: i64,
    destination: i64,
    monitor: &str,
) -> Result<String> {
    let movement = guarded_move(address, pid, source, destination)?;
    let monitor = quote(monitor)?;
    Ok(format!(
        "local m=hl.get_monitor({monitor}); assert(m,'monitor disappeared'); local target=hl.get_workspace({destination}); assert(not target or target.monitor==m,'foreign destination'); local previous=hl.get_active_monitor(); local active=hl.get_active_window(); local r=hl.dispatch(hl.dsp.focus({{monitor=m}})); assert(r.ok,r.error); {movement}; if previous then r=hl.dispatch(hl.dsp.focus({{monitor=previous}})); assert(r.ok,r.error) end; if active and active~=w and active.mapped then r=hl.dispatch(hl.dsp.focus({{window=active}})); assert(r.ok,r.error) end"
    ))
}

fn window_guard(address: &str, pid: u32, workspace: i64) -> Result<String> {
    ensure!(
        address.starts_with("0x")
            && address.len() > 2
            && address.len() <= 18
            && address[2..].bytes().all(|b| b.is_ascii_hexdigit())
            && pid > 0,
        "invalid window identity"
    );
    ensure!(workspace > 0, "ordinary workspace required");
    Ok(format!(
        "local w=hl.get_window('address:{address}'); assert(w and w.mapped and w.pid=={pid} and w.workspace and w.workspace.id=={workspace}, 'stale window');"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
