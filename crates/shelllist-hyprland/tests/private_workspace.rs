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
    compile_builders_without_effects(&client).await;
    let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel(1024);
    let event_task = tokio::spawn(client.clone().watch_events(event_sender));
    assert!(
        matches!(event_receiver.recv().await, Some(shelllist_hyprland::Event::Connected { instance }) if Some(instance.as_str()) == client.instance_signature().unwrap())
    );
    let initial = workspace::snapshot(&client).await.unwrap();
    assert!(!initial.monitors.is_empty());
    let monitor = &initial.monitors[0];
    workspace::execute(
        &client,
        &workspace::focus_local(&monitor.name, monitor.active_workspace.id.unwrap(), 6, true)
            .unwrap(),
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
    assert_eq!(target.workspace.id, Some(6));
    let pinned = workspace::pinned_snapshot(&client, &monitor.name, 6, "probe-1")
        .await
        .unwrap();
    assert!(
        pinned
            .windows
            .iter()
            .any(|window| window.address == target.address)
    );
    let token = format!("probe-1-{}", target.address);
    workspace::execute(
        &client,
        &workspace::move_local(&target.address, target.pid, 6, 7, &monitor.name).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        workspace::snapshot(&client)
            .await
            .unwrap()
            .monitors
            .iter()
            .find(|m| m.focused)
            .unwrap()
            .active_workspace
            .id,
        Some(6),
        "silent placement must not follow the moved active window"
    );
    assert!(
        workspace::execute(
            &client,
            &workspace::close_pinned(&token, &target.address, target.pid, 6).unwrap()
        )
        .await
        .is_err()
    );
    assert!(app.0.try_wait().unwrap().is_none());
    for token in ["retained-ref", "acceptance-one-a", "acceptance-two-a"] {
        workspace::execute(
            &client,
            &workspace::pin_window(token, &target.address, target.pid, 7).unwrap(),
        )
        .await
        .unwrap();
    }
    workspace::execute(
        &client,
        &workspace::release_prefix("acceptance-one-").unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        workspace::pinned_state(&client, "acceptance-one-a", &target.address, target.pid, 7)
            .await
            .unwrap(),
        workspace::PinnedState::Lost
    );
    assert_eq!(
        workspace::pinned_state(&client, "acceptance-two-a", &target.address, target.pid, 7)
            .await
            .unwrap(),
        workspace::PinnedState::Present
    );
    assert_eq!(
        workspace::pinned_state(&client, "retained-ref", &target.address, target.pid, 6)
            .await
            .unwrap(),
        workspace::PinnedState::Moved
    );
    workspace::execute(
        &client,
        &workspace::release_window("acceptance-two-a").unwrap(),
    )
    .await
    .unwrap();
    workspace::execute(
        &client,
        &workspace::close_pinned(&token, &target.address, target.pid, 7).unwrap(),
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
            &workspace::close_pinned(&token, &target.address, target.pid, 7).unwrap()
        )
        .await
        .is_err()
    );
    assert_eq!(
        workspace::pinned_state(&client, "retained-ref", &target.address, target.pid, 7)
            .await
            .unwrap(),
        workspace::PinnedState::Gone
    );
    let mut replacement = spawn(
        &weston.join("bin/weston-flower"),
        &[],
        &root,
        &display,
        "replacement.log",
    );
    let mut candidate = None;
    for _ in 0..100 {
        candidate = workspace::snapshot(&client)
            .await
            .unwrap()
            .windows
            .into_iter()
            .find(|w| w.pid == replacement.0.id() && w.mapped);
        if candidate.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let candidate = candidate.expect("replacement did not map");
    workspace::execute(
        &client,
        &workspace::replace_pinned(
            "retained-ref",
            &target.address,
            &candidate,
            7,
            &monitor.name,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        workspace::pinned_state(
            &client,
            "retained-ref",
            &candidate.address,
            candidate.pid,
            7
        )
        .await
        .unwrap(),
        workspace::PinnedState::Present
    );
    workspace::execute(
        &client,
        &workspace::follow_window(&candidate.address, candidate.pid, 7, &monitor.name, 6).unwrap(),
    )
    .await
    .unwrap();
    assert!(
        workspace::execute(
            &client,
            &workspace::follow_window(&candidate.address, candidate.pid, 7, &monitor.name, 6)
                .unwrap()
        )
        .await
        .is_err(),
        "stale follow intent must fail"
    );
    workspace::execute(
        &client,
        &workspace::close_pinned("retained-ref", &candidate.address, candidate.pid, 7).unwrap(),
    )
    .await
    .unwrap();
    for _ in 0..100 {
        if replacement.0.try_wait().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(replacement.0.try_wait().unwrap().is_some());
    workspace::execute(
        &client,
        &shelllist_hyprland::lua::focus_workspace(6, false).unwrap(),
    )
    .await
    .unwrap();
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
                other.active_workspace.id.unwrap(),
                source.active_workspace.id.unwrap(),
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
        &workspace::focus_local(&other.name, other.active_workspace.id.unwrap(), 8, false).unwrap(),
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
        Some(8)
    );
    // Real special workspaces must remain readable on 0.57, where id is absent.
    toggle_special(&client, "ipc-probe").await;
    let special = workspace::snapshot(&client).await.unwrap();
    assert!(
        special
            .workspaces
            .iter()
            .any(|w| w.id.is_none_or(|id| id < 0))
    );
    assert!(!client.work_areas().await.unwrap().is_empty());
    let special_app = spawn(
        &weston.join("bin/weston-flower"),
        &[],
        &root,
        &display,
        "special.log",
    );
    let mut window = None;
    for _ in 0..100 {
        window = workspace::snapshot(&client)
            .await
            .unwrap()
            .windows
            .into_iter()
            .find(|w| w.pid == special_app.0.id() && w.mapped);
        if window.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let window = window.expect("special application did not map");
    workspace::execute(
        &client,
        &shelllist_hyprland::lua::move_window(&window.address, "6").unwrap(),
    )
    .await
    .unwrap();
    workspace::execute(
        &client,
        &shelllist_hyprland::lua::move_window(&window.address, "special:ipc-probe").unwrap(),
    )
    .await
    .unwrap();
    let observed = workspace::snapshot(&client).await.unwrap();
    let moved = observed
        .windows
        .iter()
        .find(|w| w.pid == special_app.0.id())
        .unwrap();
    assert!(moved.workspace.special);
    assert_eq!(moved.workspace.id, None);
    let status = client.status().await.unwrap();
    assert!(status.workspaces.iter().any(|w| w.id.is_none()));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match event_receiver.recv().await.unwrap() {
                shelllist_hyprland::Event::Compositor(
                    shelllist_hyprland::CompositorEvent::WindowMoved { workspace, .. },
                ) if workspace.special => break,
                shelllist_hyprland::Event::Compositor(
                    shelllist_hyprland::CompositorEvent::Malformed { name, data },
                ) => panic!("malformed live event {name}>>{data}"),
                shelllist_hyprland::Event::Lagged => panic!("acceptance event capture overflowed"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    // Capture a real named regular workspace: 0.57 gives it an address but no id.
    toggle_special(&client, "ipc-probe").await;
    let raw_events = tokio::net::UnixStream::connect(
        root.join("hypr")
            .join(client.instance_signature().unwrap().unwrap())
            .join(".socket2.sock"),
    )
    .await
    .unwrap();
    workspace::execute(
        &client,
        &shelllist_hyprland::lua::move_window(&window.address, "name:ipc-review").unwrap(),
    )
    .await
    .unwrap();
    workspace::execute(
        &client,
        &shelllist_hyprland::lua::focus_window(&window.address).unwrap(),
    )
    .await
    .unwrap();
    let named = client.active_workspace().await.unwrap();
    assert_eq!(named.id, None);
    assert!(!named.special);
    assert_eq!(named.address.as_deref(), Some("ipc-review"));
    assert_eq!(named.selector().unwrap(), "name:ipc-review");
    let observed = workspace::snapshot(&client).await.unwrap();
    assert!(
        observed
            .workspaces
            .iter()
            .any(|w| w.address == named.address && w.id.is_none() && !w.special)
    );
    assert!(
        observed
            .monitors
            .iter()
            .any(|m| m.active_workspace == named)
    );
    assert!(!client.work_areas().await.unwrap().is_empty());
    assert!(
        workspace::execute(
            &client,
            &workspace::pin_window("named-must-not-pin", &window.address, window.pid, 6).unwrap()
        )
        .await
        .is_err()
    );
    let mut capture = serde_json::Map::new();
    for key in [
        "monitors",
        "workspaces",
        "clients",
        "activeworkspace",
        "activewindow",
        "workspacerules",
    ] {
        capture.insert(
            key.into(),
            serde_json::from_str::<serde_json::Value>(
                &client.request_exact(&format!("j/{key}")).await.unwrap(),
            )
            .unwrap(),
        );
    }
    capture.insert(
        "gaps".into(),
        serde_json::from_str(
            &client
                .request_exact("j/getoption general:gaps_out")
                .await
                .unwrap(),
        )
        .unwrap(),
    );
    fs::write(
        root.join("named.json"),
        serde_json::to_string_pretty(&capture).unwrap(),
    )
    .unwrap();
    use tokio::io::AsyncBufReadExt;
    let mut raw_events = tokio::io::BufReader::new(raw_events).lines();
    let mut moves = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        while moves.len() < 2 {
            let line = raw_events.next_line().await.unwrap().unwrap();
            if line.starts_with("movewindow>>") || line.starts_with("movewindowv2>>") {
                let shelllist_hyprland::CompositorEvent::WindowMoved { workspace, .. } =
                    shelllist_hyprland::CompositorEvent::parse(&line)
                else {
                    panic!("malformed live named move: {line}")
                };
                assert_eq!(workspace.id, None);
                assert!(!workspace.special);
                assert_eq!(workspace.address, named.address);
                moves.push(line);
            }
        }
    })
    .await
    .unwrap();
    fs::write(root.join("named-events.txt"), moves.join("\n") + "\n").unwrap();
    drop(event_receiver);
    event_task.await.unwrap();
}

// Acceptance-only API: a rename must fail this gate, not every production mutation.
async fn toggle_special(client: &Client, name: &str) {
    let name = workspace::quote(name).unwrap();
    shelllist_hyprland::ack(&client.request_exact(&format!(
        "eval assert(type(hl.dsp.workspace.toggle_special)=='function'); local r=hl.dispatch(hl.dsp.workspace.toggle_special({name})); assert(r.ok,r.error)"
    )).await.unwrap()).unwrap();
}

async fn compile_builders_without_effects(client: &Client) {
    use shelllist_hyprland::{
        lua,
        monitor::{self, Mode, Position, Rule},
    };
    assert_eq!(client.capabilities().await.unwrap().version, "0.57.1");
    let before = client
        .request_exact("[[BATCH]]j/workspaces;j/clients")
        .await
        .unwrap();
    let window = workspace::Window {
        address: "0x456".into(),
        pid: 42,
        workspace: workspace::WorkspaceRef {
            id: Some(6),
            ..Default::default()
        },
        ..Default::default()
    };
    let sources = [
        lua::focus_workspace(6, true).unwrap(),
        lua::focus_window("0x123").unwrap(),
        lua::close_window("0x123").unwrap(),
        lua::move_window("0x123", "6").unwrap(),
        lua::configure(&std::collections::BTreeMap::from([(
            "cursor:no_warps".into(),
            serde_json::json!(true),
        )]))
        .unwrap(),
        monitor::command("TEST-1", Rule::Disable).unwrap(),
        monitor::command(
            "TEST-1",
            Rule::Enable {
                mode: Mode::Exact("1920x1080@60"),
                position: Position::Exact { x: 0, y: 0 },
                scale: 1.0,
                transform: Some(0),
                mirror: "",
            },
        )
        .unwrap(),
        workspace::focus_window("0x123", 42, 6).unwrap(),
        workspace::follow_window("0x123", 42, 6, "TEST-1", 1).unwrap(),
        workspace::pin_window("compile-only", "0x123", 42, 6).unwrap(),
        workspace::close_pinned("compile-only", "0x123", 42, 6).unwrap(),
        workspace::release_window("compile-only").unwrap(),
        workspace::release_prefix("compile-").unwrap(),
        workspace::replace_pinned("compile-only", "0x123", &window, 7, "TEST-1").unwrap(),
        workspace::select_local("TEST-1", 1, 6, true, false).unwrap(),
        workspace::select_local("TEST-1", 1, 6, false, true).unwrap(),
        workspace::focus_local("TEST-1", 1, 6, true).unwrap(),
        workspace::guarded_move("0x123", 42, 6, 7).unwrap(),
        workspace::move_local("0x123", 42, 6, 7, "TEST-1").unwrap(),
    ];
    for source in sources {
        let quoted = serde_json::to_string(&source.to_string()).unwrap();
        shelllist_hyprland::ack(
            &client
                .request_exact(&format!("eval assert(load({quoted}))"))
                .await
                .unwrap(),
        )
        .unwrap();
    }
    for dispatcher in [
        "hl.dsp.focus({workspace=6})",
        "hl.dsp.window.move({workspace=6,window='address:0x123'})",
        "hl.dsp.window.close({window='address:0x123'})",
        "hl.dsp.workspace.toggle_special('compile-only')",
    ] {
        shelllist_hyprland::ack(
            &client
                .request_exact(&format!("eval return {dispatcher}"))
                .await
                .unwrap(),
        )
        .unwrap();
    }
    assert_eq!(
        client
            .request_exact("[[BATCH]]j/workspaces;j/clients")
            .await
            .unwrap(),
        before,
        "compile/constructor checks dispatched effects"
    );
}
