//! Exercise the compiled CLI and subprocess adapter, without running real Nix.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    log: PathBuf,
    path: std::ffi::OsString,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repository with spaces");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("flake.nix"), "{ inputs = {}; }").unwrap();
        for args in [vec!["init", "-q"], vec!["add", "."]] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let bin = temporary.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let nix = bin.join("nix");
        fs::write(
            &nix,
            r#"#!/bin/sh
set -eu
printf '%s\n' "$@" >> "$LOCAL_BUILD_TEST_LOG"
case "$1 $2" in
  'eval --impure') printf '{}\n' ;;
  'flake lock')
    for arg do destination=$arg; done
    printf '{}\n' > "$destination"
    ;;
  *)
    for arg do
      case "$arg" in
        path:*) tree=${arg#path:}; tree=${tree%%#*}; test -f "$tree/flake.lock" ;;
      esac
    done
    exit "${LOCAL_BUILD_TEST_EXIT:-0}"
    ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(nix, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let log = temporary.path().join("commands");
        Self {
            _temporary: temporary,
            root,
            log,
            path,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_local-build"));
        command
            .env("PATH", &self.path)
            .env("LOCAL_BUILD_TEST_LOG", &self.log);
        command
    }

    fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap()
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn approval_capture_preserves_original_lock_before_resolution() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("flake.lock"), "{\"original\": true}\n").unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&fixture.root)
            .args(["add", "."])
            .status()
            .unwrap()
            .success()
    );
    let destination = fixture.root.parent().unwrap().join("captured");
    let output = fixture
        .command()
        .arg("prepare")
        .arg(&fixture.root)
        .arg(&destination)
        .arg("--capture-root")
        .output()
        .unwrap();
    assert_success(&output);
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["originalRoot"],
        destination.join(".approval-root").to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(destination.join(".approval-root/flake.lock")).unwrap(),
        "{\"original\": true}\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("root/flake.lock")).unwrap(),
        "{}\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("flake.lock")).unwrap(),
        "{\"original\": true}\n"
    );
}

#[test]
fn invocation_uses_frozen_lock_forwards_arguments_and_cleans_up_on_success_or_failure() {
    let fixture = Fixture::new();
    for (action, status) in [("check", 0), ("build", 0), ("develop", 17), ("run", 0)] {
        fs::write(&fixture.log, "").unwrap();
        let output = fixture
            .command()
            .env("LOCAL_BUILD_TEST_EXIT", status.to_string())
            .arg(action)
            .arg(&fixture.root)
            .args(["--", "--test-option", "two words"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            status == 0,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let log = fixture.log();
        assert!(log.contains("--no-update-lock-file\n"));
        assert!(log.ends_with("--test-option\ntwo words\n"));
        assert!(log.contains("--\n") == (action == "run"));
        for flake in log.lines().filter_map(|line| line.strip_prefix("path:")) {
            assert!(
                !Path::new(flake).exists(),
                "temporary graph survived {action}"
            );
        }
        assert!(!fixture.root.join("flake.lock").exists());
    }
}

#[test]
fn prepare_and_prune_lock_keep_machine_readable_contracts() {
    let fixture = Fixture::new();
    let destination = fixture._temporary.path().join("prepared");
    let output = fixture
        .command()
        .arg("prepare")
        .arg(&fixture.root)
        .arg(&destination)
        .output()
        .unwrap();
    assert_success(&output);
    let prepared: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join("sources.json")).unwrap()).unwrap();
    assert_eq!(saved, prepared);
    assert!(prepared["storeSources"].as_object().unwrap().is_empty());
    assert!(destination.join("root/flake.lock").is_file());
    assert!(!fixture.root.join("flake.lock").exists());
    // No local inputs: only unreachable nodes should be removed.
    let lock = fixture._temporary.path().join("explicit.lock");
    fs::write(&lock, r#"{"version":7,"root":"root","nodes":{"root":{"inputs":{"dep":"dep"}},"dep":{"locked":{"rev":"keep"}},"unused":{}}}"#).unwrap();
    assert_success(
        &fixture
            .command()
            .arg("prune-lock")
            .arg(&fixture.root)
            .arg(&lock)
            .output()
            .unwrap(),
    );
    let pruned: serde_json::Value = serde_json::from_slice(&fs::read(lock).unwrap()).unwrap();
    assert_eq!(pruned["nodes"]["dep"]["locked"]["rev"], "keep");
    assert!(pruned["nodes"].get("unused").is_none());
}

#[test]
fn override_rejection_precedes_any_nix_process() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .arg("check")
        .arg(&fixture.root)
        .arg("--override-input=private")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("single-snapshot guarantee"));
    assert!(!fixture.log.exists());
}
