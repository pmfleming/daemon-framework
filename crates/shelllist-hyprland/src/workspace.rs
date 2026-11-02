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
    let mut parts = serde_json::Deserializer::from_str(&reply);
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
