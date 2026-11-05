use crate::{Client, ack, workspace::quote};
use anyhow::{Result, ensure};

/// Validated Lua source. The transport prefix belongs to execute, not builders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lua(pub(crate) String);
impl Lua {
    /// Compose already-validated commands in the same compositor turn.
    pub fn then(mut self, next: Self) -> Self {
        self.0.push_str("; ");
        self.0.push_str(&next.0);
        self
    }
}
impl std::ops::Deref for Lua {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Display for Lua {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl PartialEq<&str> for Lua {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

pub async fn execute(client: &Client, source: &Lua) -> Result<()> {
    client.capabilities().await?;
    ack(&client.request_exact(&format!("eval {source}")).await?)
}

fn dispatch(expression: String) -> Lua {
    Lua(format!(
        "local r=hl.dispatch({expression}); assert(r.ok,r.error)"
    ))
}
pub fn focus_workspace(id: i64, on_current_monitor: bool) -> Result<Lua> {
    ensure!(
        id > 0 && id <= i64::from(i32::MAX),
        "invalid ordinary workspace"
    );
    let current = if on_current_monitor {
        ", on_current_monitor = true"
    } else {
        ""
    };
    Ok(dispatch(format!(
        "hl.dsp.focus({{ workspace = {id}{current} }})"
    )))
}
pub fn focus_window(address: &str) -> Result<Lua> {
    let address = crate::WindowAddress::parse(address)?;
    Ok(dispatch(format!(
        "hl.dsp.focus({{ window = 'address:{address}' }})"
    )))
}
pub fn close_window(address: &str) -> Result<Lua> {
    let address = crate::WindowAddress::parse(address)?;
    Ok(dispatch(format!(
        "hl.dsp.window.close({{ window = 'address:{address}' }})"
    )))
}
pub fn move_window(address: &str, workspace: &str) -> Result<Lua> {
    let address = crate::WindowAddress::parse(address)?;
    ensure!(
        !workspace.is_empty() && workspace.len() <= 133 && !workspace.chars().any(char::is_control),
        "invalid workspace selector"
    );
    let workspace = quote(workspace)?;
    Ok(dispatch(format!(
        "hl.dsp.window.move({{ workspace = {workspace}, follow = false, window = 'address:{address}' }})"
    )))
}
pub fn configure(options: &std::collections::BTreeMap<String, serde_json::Value>) -> Result<Lua> {
    ensure!(!options.is_empty(), "empty compositor configuration");
    let mut fields = Vec::new();
    for (key, value) in options {
        ensure!(
            valid_option(key) && (value.is_boolean() || value.is_number()),
            "invalid compositor option/value"
        );
        fields.push(format!("[{}] = {value}", quote(&key.replace(':', "."))?));
    }
    Ok(Lua(format!("hl.config({{ {} }})", fields.join(", "))))
}
pub(crate) fn valid_option(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_:.-".contains(&c))
}

impl PartialEq<String> for Lua {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}
