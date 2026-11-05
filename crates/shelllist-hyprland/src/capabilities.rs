use crate::{Client, IpcError, ack};
use anyhow::{Context, Result, ensure};

/// Successful checks are cached only for this immutable exact-session client.
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub version: String,
}

pub const LUA_APIS: &[&str] = &[
    "hl.dispatch",
    "hl.monitor",
    "hl.get_monitor",
    "hl.get_active_monitor",
    "hl.get_window",
    "hl.get_windows",
    "hl.get_workspace",
    "hl.get_active_window",
    "hl.dsp.focus",
    "hl.dsp.window.move",
    "hl.dsp.window.close",
    "hl.config",
    "hl.dsp.workspace.toggle_special",
];

pub fn probe() -> String {
    LUA_APIS
        .iter()
        .map(|api| format!("assert(type({api})=='function','missing {api}')"))
        .collect::<Vec<_>>()
        .join("; ")
}

pub(crate) async fn load(client: &Client) -> Result<Capabilities> {
    let status: serde_json::Value = decode(&client.request_exact("j/status").await?)?;
    ensure!(
        status["configProvider"] == "lua",
        IpcError::Rejected("Lua configuration is required".into())
    );
    let version: serde_json::Value = decode(&client.request_exact("j/version").await?)?;
    let version = version["version"]
        .as_str()
        .context(IpcError::InvalidReply)?;
    let numbers = version
        .trim_start_matches('v')
        .split('.')
        .take(3)
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context(IpcError::InvalidReply)?;
    ensure!(
        numbers.len() == 3 && numbers.as_slice() >= [0, 57, 1].as_slice(),
        IpcError::Rejected("Hyprland 0.57.1 or newer is required".into())
    );
    ack(&client.request_exact(&format!("eval {}", probe())).await?)?;
    Ok(Capabilities {
        version: version.into(),
    })
}

fn decode(reply: &str) -> Result<serde_json::Value> {
    serde_json::from_str(reply).context(IpcError::InvalidReply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    #[test]
    fn every_builder_api_is_probed() {
        for source in [
            include_str!("lua.rs"),
            include_str!("monitor.rs"),
            include_str!("workspace.rs"),
        ] {
            for tail in source.split("hl.").skip(1) {
                let token = tail
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.'))
                    .collect::<String>();
                let api = format!("hl.{}", token.trim_end_matches('.'));
                assert!(
                    LUA_APIS.contains(&api.as_str()),
                    "builder uses unprobed API {api}"
                );
            }
        }
    }

    #[tokio::test]
    async fn failures_are_not_cached_and_success_is_bound_to_this_client() {
        let root = tempfile::tempdir().unwrap();
        let instance = root.path().join("hypr/test");
        std::fs::create_dir_all(&instance).unwrap();
        let listener = UnixListener::bind(instance.join(".socket.sock")).unwrap();
        let client = Client::new(root.path().into(), Some("test".into()));
        let server = tokio::spawn(async move {
            for (expected, reply) in [
                ("j/status".to_owned(), r#"{"configProvider":"hyprlang"}"#),
                ("j/status".to_owned(), r#"{"configProvider":"lua"}"#),
                ("j/version".to_owned(), r#"{"version":"0.56.2"}"#),
                ("j/status".to_owned(), r#"{"configProvider":"lua"}"#),
                ("j/version".to_owned(), r#"{"version":"0.57.1"}"#),
                (format!("eval {}", probe()), "missing API"),
                ("j/status".to_owned(), r#"{"configProvider":"lua"}"#),
                ("j/version".to_owned(), r#"{"version":"0.57.1"}"#),
                (format!("eval {}", probe()), "ok"),
                (
                    format!("eval {}", crate::lua::focus_workspace(6, false).unwrap()),
                    "ok",
                ),
                (
                    format!("eval {}", crate::lua::focus_workspace(6, false).unwrap()),
                    "ok",
                ),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut command = String::new();
                stream.read_to_string(&mut command).await.unwrap();
                assert_eq!(command, expected);
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        for _ in 0..3 {
            assert!(
                crate::execute(&client, &crate::lua::focus_workspace(6, false).unwrap())
                    .await
                    .is_err()
            );
        }
        assert_eq!(client.capabilities().await.unwrap().version, "0.57.1");
        for _ in 0..2 {
            crate::execute(&client, &crate::lua::focus_workspace(6, false).unwrap())
                .await
                .unwrap();
        }
        server.await.unwrap();
        // The server is gone; a cached success remains available on this client.
        assert_eq!(
            client.clone().capabilities().await.unwrap().version,
            "0.57.1"
        );
    }
}
