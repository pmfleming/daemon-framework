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
    ensure!(
        supported_version(version)?,
        IpcError::Rejected("Hyprland 0.57.1 or newer is required".into())
    );
    ack(&client.request_exact(&format!("eval {}", probe())).await?)?;
    Ok(Capabilities {
        version: version.into(),
    })
}

fn supported_version(version: &str) -> Result<bool> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let (version, build) = version
        .split_once('+')
        .map_or((version, None), |(v, b)| (v, Some(b)));
    let (core, prerelease) = version
        .split_once('-')
        .map_or((version, None), |(v, p)| (v, Some(p)));
    for suffix in [build, prerelease].into_iter().flatten() {
        ensure!(
            suffix.split('.').all(|part| !part.is_empty()
                && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')),
            IpcError::InvalidReply
        );
    }
    let components: Vec<_> = core.split('.').collect();
    ensure!(
        components.len() == 3
            && components
                .iter()
                .all(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit())),
        IpcError::InvalidReply
    );
    let mut numbers = [0u32; 3];
    for (number, component) in numbers.iter_mut().zip(components) {
        *number = component.parse().context(IpcError::InvalidReply)?;
    }
    // A prerelease of the floor itself is still older than the required release.
    Ok(numbers > [0, 57, 1] || (numbers == [0, 57, 1] && prerelease.is_none()))
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
    fn version_suffixes_preserve_the_release_floor_and_reject_malformed_cores() {
        for version in [
            "0.57.1",
            "v0.57.1",
            "0.57.1+build.123",
            "0.58.0-rc1",
            "v0.58.0-dev.2+git.abc",
            "1.0.0",
        ] {
            assert!(supported_version(version).unwrap(), "{version}");
        }
        for version in ["0.56.2", "0.57.0+build", "0.57.1-rc1"] {
            assert!(!supported_version(version).unwrap(), "{version}");
        }
        for version in [
            "",
            "vv0.57.1",
            "0.57",
            "0.57.1.2",
            "0.58.0oops",
            "0.58.0-",
            "0.58.0+",
            "0.58.0-rc..1",
            "0.58.0+git+extra",
            "0.58.0-rc 1",
            "0.58.4294967296",
        ] {
            assert!(supported_version(version).is_err(), "{version}");
        }
    }

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
                ("j/version".to_owned(), r#"{"version":"0.58.0-rc1"}"#),
                (format!("eval {}", probe()), "missing API"),
                ("j/status".to_owned(), r#"{"configProvider":"lua"}"#),
                ("j/version".to_owned(), r#"{"version":"0.58.0-rc1"}"#),
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
        assert_eq!(client.capabilities().await.unwrap().version, "0.58.0-rc1");
        for _ in 0..2 {
            crate::execute(&client, &crate::lua::focus_workspace(6, false).unwrap())
                .await
                .unwrap();
        }
        server.await.unwrap();
        // The server is gone; a cached success remains available on this client.
        assert_eq!(
            client.clone().capabilities().await.unwrap().version,
            "0.58.0-rc1"
        );
    }
}
