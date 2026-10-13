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
fn prepare_capture_and_prune_preserve_locks_and_machine_readable_contracts() {
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
    let destination = fixture._temporary.path().join("captured");
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
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join("sources.json")).unwrap()).unwrap();
    assert_eq!(saved, result);
    assert!(result["storeSources"].as_object().unwrap().is_empty());
    assert_eq!(
        result["originalRoot"],
        destination.join(".approval-root").to_str().unwrap()
    );
    for path in [
        fixture.root.join("flake.lock"),
        destination.join(".approval-root/flake.lock"),
    ] {
        assert_eq!(fs::read_to_string(path).unwrap(), "{\"original\": true}\n");
    }
    assert_eq!(
        fs::read_to_string(destination.join("root/flake.lock")).unwrap(),
        "{}\n"
    );
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
fn invocation_uses_frozen_lock_forwards_arguments_and_cleans_up_on_success_or_failure() {
    let fixture = Fixture::new();
    for (action, status) in [("check", 0), ("build", 0), ("develop", 17), ("run", 0)] {
        fs::write(&fixture.log, "").unwrap();
        let mut command = fixture.command();
        command
            .env("LOCAL_BUILD_TEST_EXIT", status.to_string())
            .arg(action);
        if action != "check" {
            command.args(["--attr", "chosen"]);
        }
        command
            .arg(&fixture.root)
            .args(["--", "--keep-going", "two words"]);
        if action == "run" {
            command.args(["--help", "--attr", "child-option"]);
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.success(),
            status == 0,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let log = fixture.log();
        let prefix = if action == "check" {
            "flake\ncheck"
        } else {
            action
        };
        assert!(log.contains(&format!("{prefix}\npath:")));
        assert!(log.contains("--no-update-lock-file\n"));
        assert!(log.contains("--keep-going\ntwo words\n"));
        assert_eq!(log.contains("#chosen\n"), action != "check");
        assert_eq!(log.contains("--\n"), action == "run");
        if action == "run" {
            assert!(log.ends_with("--help\n--attr\nchild-option\n"));
        }
        for flake in log.lines().filter_map(|line| line.strip_prefix("path:")) {
            let path = flake.split_once('#').map_or(flake, |(path, _)| path);
            assert!(
                !Path::new(path).exists(),
                "temporary graph survived {action}"
            );
        }
        assert!(!fixture.root.join("flake.lock").exists());
    }
}

#[test]
fn override_rejection_precedes_any_nix_process() {
    let fixture = Fixture::new();
    for flag in [
        "--override-input",
        "--update-input",
        "--recreate-lock-file",
        "--override-flake",
        "--flake",
    ] {
        for argument in [flag.to_owned(), format!("{flag}=elsewhere")] {
            let output = fixture
                .command()
                .arg("check")
                .arg(&fixture.root)
                .arg(&argument)
                .output()
                .unwrap();
            assert!(!output.status.success(), "{argument}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("single-snapshot guarantee"),
                "{argument}"
            );
            assert!(!fixture.log.exists());
        }
    }
}
