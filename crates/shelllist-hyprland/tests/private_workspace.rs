//! Opt-in installed-compositor acceptance in an isolated nested Wayland session.
use shelllist_hyprland::{Client, workspace};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn(binary: &Path, args: &[&str], runtime: &Path, display: &str, log: &str) -> Process {
    let file = fs::File::create(runtime.join(log)).unwrap();
    Process(
        Command::new(binary)
            .args(args)
            .env("XDG_RUNTIME_DIR", runtime)
            .env("XDG_CONFIG_HOME", runtime)
            .env("XDG_CACHE_HOME", runtime.join("cache"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/no-bus", runtime.display()),
            )
            .env("WAYLAND_DISPLAY", display)
            .env_remove("DISPLAY")
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .env_remove("NOTIFY_SOCKET")
            .env("AQ_DRM_DEVICES", "/dev/null")
            .env("LIBSEAT_BACKEND", "noop")
            .env("WLR_BACKENDS", "headless")
            .env("WLR_RENDERER", "gles2")
            .env("HYPRLAND_NO_SD_NOTIFY", "1")
            .env("HYPRLAND_NO_SD_VARS", "1")
            .stdin(Stdio::null())
            .stdout(file.try_clone().unwrap())
            .stderr(file)
            .spawn()
            .unwrap(),
    )
}

#[tokio::test]
#[ignore = "requires WORKSPACE_TEST_WESTON, WORKSPACE_TEST_HYPRLAND and a nested renderer"]
async fn installed_compositor_rejects_moved_targets_and_normally_closes_matching_window() {
    let weston = PathBuf::from(std::env::var("WORKSPACE_TEST_WESTON").unwrap());
    let hyprland = PathBuf::from(std::env::var("WORKSPACE_TEST_HYPRLAND").unwrap());
    let root = tempfile::Builder::new()
        .prefix("ws-")
        .tempdir_in("/tmp")
        .unwrap()
        .keep();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    eprintln!("private compositor evidence: {}", root.display());
    let sway = std::env::var_os("WORKSPACE_TEST_SWAY").map(PathBuf::from);
    let parent_config = root.join("parent.conf");
    fs::write(&parent_config, "output HEADLESS-1 mode 1280x720\n").unwrap();
    let existing_parent = std::env::var("WORKSPACE_TEST_PARENT_DISPLAY").ok();
    let mut parent = if existing_parent.is_some() {
        None
    } else if let Some(sway) = sway {
        Some(spawn(
            &sway,
            &[
                "--config",
                parent_config.to_str().unwrap(),
                "--unsupported-gpu",
            ],
            &root,
            "unavailable",
            "parent.log",
        ))
    } else {
        Some(spawn(
            &weston.join("bin/weston"),
            &[
                "--backend=headless",
                "--renderer=gl",
                "--socket=workspace-parent",
                "--no-config",
                "--shell=kiosk-shell.so",
                "--idle-time=0",
            ],
            &root,
            "unavailable",
            "parent.log",
        ))
    };
    let mut parent_display = existing_parent;
    for _ in 0..100 {
        if parent_display.is_some() {
            break;
        }
        assert!(
            parent.as_mut().unwrap().0.try_wait().unwrap().is_none(),
            "parent exited; see parent.log"
        );
        parent_display = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|n| {
                (n.starts_with("wayland-") || n == "workspace-parent") && !n.ends_with(".lock")
            });
        if parent_display.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let parent_display = parent_display.expect("parent socket");
    let config = root.join("hyprland.lua");
    fs::write(
        &config,
        "-- Private workspace acceptance. No user includes or startup commands.\n",
    )
    .unwrap();
    let mut compositor = spawn(
        &hyprland,
        &["--config", config.to_str().unwrap()],
        &root,
        &parent_display,
        "hyprland.log",
    );
    let mut client = None;
    for _ in 0..150 {
        assert!(
            compositor.0.try_wait().unwrap().is_none(),
            "Hyprland exited; see hyprland.log"
        );
        if let Ok(entries) = fs::read_dir(root.join("hypr")) {
            for entry in entries.flatten() {
                let probe = Client::new(
                    root.clone(),
                    Some(entry.file_name().to_string_lossy().into_owned()),
                );
                if probe.request_exact("j/monitors").await.is_ok() {
                    client = Some(probe);
                    break;
                }
            }
        }
        if client.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let client = client.expect("private Hyprland IPC did not start");
    let initial = workspace::snapshot(&client).await.unwrap();
    assert!(!initial.monitors.is_empty());
    let monitor = &initial.monitors[0];
    workspace::execute(
        &client,
        &workspace::focus_local(&monitor.name, monitor.active_workspace.id, 6, true).unwrap(),
    )
    .await
    .unwrap();
    let display = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with("wayland-") && !n.ends_with(".lock") && n != &parent_display)
        .expect("private Wayland socket");
    let mut app = spawn(
        &weston.join("bin/weston-flower"),
        &[],
        &root,
        &display,
        "window.log",
    );
    let mut target = None;
    for _ in 0..100 {
        let snapshot = workspace::snapshot(&client).await.unwrap();
        target = snapshot
            .windows
            .into_iter()
            .find(|w| w.pid == app.0.id() && w.mapped);
        if target.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let target = target.expect("test window did not map");
    assert_eq!(target.workspace.id, 6);
    workspace::execute(
        &client,
        &workspace::pin_window("probe-1", &target.address, target.pid, 6).unwrap(),
    )
    .await
    .unwrap();
    workspace::execute(
        &client,
        &workspace::move_local(&target.address, target.pid, 6, 7, &monitor.name).unwrap(),
    )
    .await
    .unwrap();
    assert!(
        workspace::execute(
            &client,
            &workspace::close_pinned("probe-1", &target.address, target.pid, 6).unwrap()
        )
        .await
        .is_err()
    );
    assert!(app.0.try_wait().unwrap().is_none());
    workspace::execute(
        &client,
        &workspace::close_pinned("probe-1", &target.address, target.pid, 7).unwrap(),
    )
    .await
    .unwrap();
    let mut closed = false;
    for _ in 0..100 {
        if app.0.try_wait().unwrap().is_some() {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(closed, "normal close was not observed");
    assert!(
        workspace::execute(
            &client,
            &workspace::close_pinned("probe-1", &target.address, target.pid, 7).unwrap()
        )
        .await
        .is_err()
    );
    assert_eq!(
        client
            .request_exact("output create headless")
            .await
            .unwrap()
            .trim(),
        "ok"
    );
    let mut snapshot = workspace::snapshot(&client).await.unwrap();
    for _ in 0..50 {
        if snapshot.monitors.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        snapshot = workspace::snapshot(&client).await.unwrap();
    }
    let other = snapshot
        .monitors
        .iter()
        .find(|m| m.name != monitor.name)
        .unwrap();
    let source = snapshot
        .monitors
        .iter()
        .find(|m| m.name == monitor.name)
        .unwrap();
    assert!(
        workspace::execute(
            &client,
            &workspace::focus_local(
                &other.name,
                other.active_workspace.id,
                source.active_workspace.id,
                true
            )
            .unwrap()
        )
        .await
        .is_err()
    );
    let before_focus = snapshot
        .monitors
        .iter()
        .find(|m| m.focused)
        .unwrap()
        .name
        .clone();
    workspace::execute(
        &client,
        &workspace::focus_local(&other.name, other.active_workspace.id, 8, false).unwrap(),
    )
    .await
    .unwrap();
    let after = workspace::snapshot(&client).await.unwrap();
    assert_eq!(
        after.monitors.iter().find(|m| m.focused).unwrap().name,
        before_focus
    );
    assert_eq!(
        after
            .monitors
            .iter()
            .find(|m| m.name == other.name)
            .unwrap()
            .active_workspace
            .id,
        8
    );
}
