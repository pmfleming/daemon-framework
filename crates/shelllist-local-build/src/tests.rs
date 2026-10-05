//! Real filesystem/Git regression tests; no Nix daemon, network or builds.
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    graph::{Source, Sources, prepare},
    lock::prune_lock,
    nix::{Inputs, Nix, Override, lock_command},
    policy::{local_path, merge, validate_policy},
    snapshot::snapshot,
};

fn inputs(value: Value) -> Inputs {
    value.as_object().unwrap().clone()
}

fn git(root: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .unwrap()
            .success()
    );
}

struct Repositories(tempfile::TempDir);
impl Repositories {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }
    fn repository(&self, name: &str, definitions: Value, files: &[(&str, &str)]) -> PathBuf {
        let root = self.path(name);
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        fs::write(root.join("flake.nix"), "{}\n").unwrap();
        fs::write(root.join("inputs.json"), definitions.to_string()).unwrap();
        for (name, text) in files {
            let target = root.join(name);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, text).unwrap();
        }
        git(&root, &["add", "."]);
        root
    }
    fn matrix(&self) -> (PathBuf, PathBuf, PathBuf) {
        let framework = self.repository("daemon-framework", json!({}), &[("source", "current")]);
        let cargo = format!(
            "[dependencies]\n{}",
            [
                "shelllist-daemon-core",
                "shelllist-daemon-tokio",
                "shelllist-hyprland"
            ]
            .map(|name| format!("{name} = {{ path = \"../daemon-framework/crates/{name}\" }}"))
            .join("\n")
        );
        let app = self.repository(
            "app-daemon",
            json!({"daemonFramework": {"url": "git+file:../daemon-framework"}}),
            &[("Cargo.toml", &cargo)],
        );
        let root = self.repository("shelllist", json!({
            "daemon-framework": {"url": "git+file:../daemon-framework"},
            "app-daemon": {"url": "git+file:../app-daemon", "inputs": {"daemonFramework": {"follows": "daemon-framework"}}}
        }), &[]);
        (root, app, framework)
    }
}

fn definitions(root: &Path) -> Inputs {
    serde_json::from_slice(&fs::read(root.join("inputs.json")).unwrap()).unwrap()
}
fn sources(paths: &[&Path]) -> Sources {
    paths
        .iter()
        .map(|path| {
            (
                path.to_path_buf(),
                Source {
                    directory: path.to_path_buf(),
                    inputs: definitions(path),
                },
            )
        })
        .collect()
}
fn error_contains<T: std::fmt::Debug>(result: Result<T>, message: &str) {
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains(message), "expected {message:?}, got {error}");
}

#[derive(Default)]
struct TestNix {
    reads: Vec<PathBuf>,
    stores: Vec<(PathBuf, PathBuf)>,
    locks: Vec<Vec<std::ffi::OsString>>,
    fail_lock: bool,
}
impl Nix for TestNix {
    fn inputs(&mut self, root: &Path) -> Result<Inputs> {
        self.reads.push(root.into());
        Ok(definitions(root))
    }
    fn add_source(&mut self, source: &Path, gc_root: &Path) -> Result<String> {
        self.stores.push((source.into(), gc_root.into()));
        Ok(format!("/nix/store/mock-source-{}", self.stores.len()))
    }
    fn lock(&mut self, staged: &Path, overrides: &[Override]) -> Result<()> {
        self.locks.push(
            lock_command(staged, overrides)?
                .get_args()
                .map(ToOwned::to_owned)
                .collect(),
        );
        anyhow::ensure!(!self.fail_lock, "injected lock failure");
        Ok(())
    }
}

#[test]
fn dirty_files_deletions_and_ignored_targets() {
    let repos = Repositories::new();
    let root = repos.repository(
        "project",
        json!({}),
        &[
            ("source", "old"),
            ("deleted", "old"),
            (".gitignore", "target/\n"),
            ("readonly", "immutable"),
        ],
    );
    fs::write(root.join("source"), "dirty").unwrap();
    fs::set_permissions(root.join("readonly"), fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(root.join("source"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_file(root.join("deleted")).unwrap();
    fs::create_dir(root.join("target")).unwrap();
    fs::write(root.join("target/large"), "ignored").unwrap();
    let destination = repos.path("snapshot");
    snapshot(&root, &destination).unwrap();
    fs::write(root.join("source"), "later edit").unwrap();
    assert_eq!(
        fs::read_to_string(destination.join("source")).unwrap(),
        "dirty"
    );
    assert_eq!(
        fs::metadata(destination.join("source"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    for name in ["deleted", "target", ".git"] {
        assert!(!destination.join(name).exists());
    }
}

#[test]
fn untracked_file_rejected() {
    let repos = Repositories::new();
    let root = repos.repository("project", json!({}), &[]);
    fs::write(root.join("forgotten.rs"), "new").unwrap();
    error_contains(snapshot(&root, &repos.path("snapshot")), "Git-add");
}

#[test]
fn escaping_symlink_rejected() {
    let repos = Repositories::new();
    let root = repos.repository("project", json!({}), &[]);
    symlink("../outside", root.join("outside")).unwrap();
    git(&root, &["add", "outside"]);
    error_contains(snapshot(&root, &repos.path("snapshot")), "symlink escapes");
}

#[test]
fn prepared_snapshot_preserves_internal_symlinks() {
    let repos = Repositories::new();
    let root = repos.repository("project", json!({}), &[("dir/source", "committed")]);
    let links = [
        ("source-link", "dir/source"),
        ("dir-link", "dir"),
        ("dir/relative", "../source-link"),
        ("dangling", "missing"),
    ];
    for (name, target) in links {
        symlink(target, root.join(name)).unwrap();
    }
    git(&root, &["add", "."]);
    let frozen = repos.path("frozen");
    snapshot(&root, &frozen).unwrap();
    let mut nix = TestNix::default();
    let result = prepare(&mut nix, &frozen, &repos.path("prepared"), true).unwrap();
    let prepared = &result.sources[&frozen];
    assert_eq!(result.flake, format!("path:{}", prepared.display()));
    assert_eq!(
        nix.locks,
        [vec![
            "flake".into(),
            "lock".into(),
            result.flake.into(),
            "--output-lock-file".into(),
            prepared.join("flake.lock").into_os_string()
        ]]
    );
    assert!(nix.stores.is_empty());
    for tree in [&frozen, prepared] {
        for (name, target) in links {
            assert_eq!(fs::read_link(tree.join(name)).unwrap(), Path::new(target));
        }
        for link in ["dir/relative", "dir-link/source"] {
            assert_eq!(fs::read_to_string(tree.join(link)).unwrap(), "committed");
        }
    }
}

#[test]
fn branch_and_revision_pins_rejected() {
    let repos = Repositories::new();
    for suffix in ["?ref=main", "?rev=123", "#main"] {
        assert!(
            local_path(
                &json!({"url": format!("git+file:../daemon-framework{suffix}")}),
                repos.0.path()
            )
            .is_err()
        );
    }
    assert_eq!(
        local_path(
            &json!({"url": "git+file:../daemon-framework"}),
            &repos.path("app-daemon")
        )
        .unwrap(),
        Some(repos.path("daemon-framework"))
    );
    assert!(local_path(&json!({"url": "git+file://remote/project"}), repos.0.path()).is_err());
    assert!(
        local_path(&json!({"url": "github:someone/project"}), repos.0.path())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        local_path(
            &json!({"url": "git+file:../with%20space"}),
            &repos.path("app-daemon")
        )
        .unwrap(),
        Some(repos.path("with space"))
    );
    let absolute = format!("git+file://localhost{}", repos.path("project").display());
    assert_eq!(
        local_path(&json!({"url": absolute}), repos.0.path()).unwrap(),
        Some(repos.path("project"))
    );
}

#[test]
fn remote_lock_preserved_local_graph_removed() {
    let lock = json!({"root":"root", "version":7, "nodes":{
        "root":{"inputs":{"nixpkgs":"nixpkgs", "app-daemon":"app", "daemon-framework":"framework"}},
        "app":{"inputs":{"daemonFramework":["daemon-framework"], "private":"private"}},
        "framework":{"inputs":{"nixpkgs":["nixpkgs"]}},
        "private":{"locked":{"rev":"old"}}, "nixpkgs":{"locked":{"rev":"keep-exactly"}}
    }});
    let result = prune_lock(lock, &["app-daemon".into(), "daemon-framework".into()]).unwrap();
    assert_eq!(
        result["nodes"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["nixpkgs", "root"]
    );
    assert_eq!(result["nodes"]["nixpkgs"]["locked"]["rev"], "keep-exactly");
    assert_eq!(result["version"], 7);
}

#[test]
fn one_snapshot_shared_by_all_edges_and_no_live_lock_write() {
    let repos = Repositories::new();
    let (root, app, framework) = repos.matrix();
    fs::write(framework.join("source"), "dirty framework").unwrap();
    let mut input = definitions(&root);
    input.insert(
        "alias".into(),
        json!({"url":"git+file:../daemon-framework"}),
    );
    fs::write(
        root.join("inputs.json"),
        serde_json::to_vec(&input).unwrap(),
    )
    .unwrap();
    let mut nix = TestNix::default();
    let result = prepare(&mut nix, &root, &repos.path("snapshots"), false).unwrap();
    assert_eq!(result.sources.len(), 3);
    assert_eq!(nix.reads.len(), 3);
    assert_eq!(nix.stores.len(), 2);
    assert_eq!(result.store_sources.len(), 2);
    assert_eq!(
        fs::read_to_string(result.sources[&framework].join("source")).unwrap(),
        "dirty framework"
    );
    let command = &nix.locks[0];
    assert!(command.contains(&"daemon-framework".into()));
    assert!(!command.contains(&"app-daemon/daemonFramework".into()));
    let alias = command.iter().position(|arg| arg == "alias").unwrap();
    let primary = command
        .iter()
        .position(|arg| arg == "daemon-framework")
        .unwrap();
    assert_eq!(command[alias + 1], command[primary + 1]);
    for source in [&root, &app, &framework] {
        assert!(!source.join("flake.lock").exists());
    }
    let saved: Value =
        serde_json::from_slice(&fs::read(repos.path("snapshots/sources.json")).unwrap()).unwrap();
    assert_eq!(saved, serde_json::to_value(result).unwrap());
}

#[test]
fn shared_root_cannot_be_remote_pinned() {
    let repos = Repositories::new();
    let (root, app, framework) = repos.matrix();
    let mut inputs = definitions(&root);
    inputs["daemon-framework"]["url"] = json!("github:pmfleming/daemon-framework/old");
    error_contains(
        validate_policy(&sources(&[&app, &framework]), &inputs),
        "current local",
    );
}

#[test]
fn private_or_vendored_framework_rejected() {
    let repos = Repositories::new();
    let (root, app, framework) = repos.matrix();
    let sources = sources(&[&app, &framework]);
    validate_policy(&sources, &definitions(&root)).unwrap();
    fs::create_dir_all(app.join("vendor/daemon-framework")).unwrap();
    error_contains(validate_policy(&sources, &definitions(&root)), "vendored");
}

#[test]
fn hyprland_consumers_require_the_framework_crate() {
    let repos = Repositories::new();
    let (root, app, framework) = repos.matrix();
    let manifest = fs::read_to_string(app.join("Cargo.toml")).unwrap();
    let bar = repos.repository(
        "bar-daemon",
        Value::Object(definitions(&app)),
        &[("Cargo.toml", &manifest)],
    );
    let sources = sources(&[&app, &bar, &framework]);
    let root_inputs = definitions(&root);
    validate_policy(&sources, &root_inputs).unwrap();
    let valid = "shelllist-hyprland = { path = \"../daemon-framework/crates/shelllist-hyprland\" }";
    for consumer in [&app, &bar] {
        for invalid in [
            "shelllist-hyprland = { path = \"../shelllist-hyprland\" }",
            "shelllist-hyprland = { path = \"vendor/shelllist-hyprland\" }",
            "shelllist-hyprland = { path = \"../daemon-framework/crates/shelllist-hyprland\", git = \"https://example.org/private\" }",
            "shelllist-hyprland = \"0.1\"",
            "",
        ] {
            fs::write(
                consumer.join("Cargo.toml"),
                manifest.replace(valid, invalid),
            )
            .unwrap();
            error_contains(
                validate_policy(&sources, &root_inputs),
                "shelllist-hyprland must use the shared sibling",
            );
        }
        fs::write(consumer.join("Cargo.toml"), &manifest).unwrap();
        fs::create_dir_all(consumer.join("vendor/shelllist-hyprland")).unwrap();
        error_contains(validate_policy(&sources, &root_inputs), "vendored");
        fs::remove_dir(consumer.join("vendor/shelllist-hyprland")).unwrap();
    }
}

#[test]
fn non_hyprland_daemon_does_not_require_the_crate() {
    let repos = Repositories::new();
    let (root, app, framework) = repos.matrix();
    let manifest = fs::read_to_string(app.join("Cargo.toml"))
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("shelllist-hyprland"))
        .collect::<Vec<_>>()
        .join("\n");
    let bt = repos.repository(
        "bt-daemon",
        Value::Object(definitions(&app)),
        &[("Cargo.toml", &manifest)],
    );
    validate_policy(&sources(&[&bt, &framework]), &definitions(&root)).unwrap();
}

#[test]
fn standalone_hyprland_inputs_rejected() {
    let repos = Repositories::new();
    let (root, app, framework) = repos.matrix();
    let mut sources = sources(&[&app, &framework]);
    for extra in [
        json!({"shelllist-hyprland":{"url":"git+file:../shelllist-hyprland"}}),
        json!({"bar-daemon":{"inputs":{"hyprlandIpc":{"follows":"shelllist-hyprland"}}}}),
        json!({"shelllist":{"inputs":{"shelllist-hyprland":{"follows":"shelllist-hyprland"}}}}),
    ] {
        let mut root_inputs = definitions(&root);
        merge(&mut root_inputs, &inputs(extra));
        error_contains(
            validate_policy(&sources, &root_inputs),
            "not a separate input",
        );
    }
    sources.get_mut(&app).unwrap().inputs.insert(
        "hyprlandIpc".into(),
        json!({"url":"git+file:../shelllist-hyprland"}),
    );
    error_contains(
        validate_policy(&sources, &definitions(&root)),
        "not a separate input",
    );
}

#[test]
fn missing_root_follows_rejected() {
    let repos = Repositories::new();
    let (root, app, framework) = repos.matrix();
    let mut inputs = definitions(&root);
    inputs["app-daemon"]
        .as_object_mut()
        .unwrap()
        .remove("inputs");
    error_contains(
        validate_policy(&sources(&[&app, &framework]), &inputs),
        "single root",
    );
}

#[test]
fn lock_follows_resolve_from_root_and_cycles_are_rejected() {
    let lock = json!({"root":"root", "nodes": {
        "root":{"inputs":{"a":"a", "alias":["a", "dep"]}},
        "a":{"inputs":{"dep":"remote"}}, "remote":{"locked":{"rev":"keep"}}, "unused":{}
    }});
    let result = prune_lock(lock, &[]).unwrap();
    assert!(result["nodes"].get("unused").is_none());
    assert_eq!(result["nodes"]["remote"]["locked"]["rev"], "keep");
    error_contains(
        prune_lock(
            json!({"root":"root", "nodes":{"root":{"inputs":{"a":["b"], "b":["a"]}}}}),
            &[],
        ),
        "cycle in lock follows",
    );
    assert!(
        prune_lock(
            json!({"root":"root", "nodes":{"root":{"inputs":{"a":["missing"]}}}}),
            &[]
        )
        .is_err()
    );
}

#[test]
fn graph_cycles_collisions_and_existing_destinations_fail_closed() {
    let repos = Repositories::new();
    let a = repos.repository("a", json!({"b":{"url":"git+file:../b"}}), &[]);
    repos.repository("b", json!({"a":{"url":"git+file:../a"}}), &[]);
    let mut nix = TestNix::default();
    error_contains(
        prepare(&mut nix, &a, &repos.path("cycle"), false),
        "local input cycle",
    );
    assert!(nix.locks.is_empty());
    error_contains(
        prepare(&mut nix, &a, &repos.path("cycle"), false),
        "already exists",
    );
    let root = repos.repository(
        "consumer",
        json!({"x":{"url":"git+file:../x/shared"}, "y":{"url":"git+file:../y/shared"}}),
        &[],
    );
    repos.repository("x/shared", json!({}), &[]);
    repos.repository("y/shared", json!({}), &[]);
    error_contains(
        prepare(&mut nix, &root, &repos.path("collision"), false),
        "basename collision",
    );
    assert!(nix.locks.is_empty());
}

#[test]
fn failed_lock_never_writes_live_locks_or_success_manifest() {
    let repos = Repositories::new();
    let root = repos.repository("project", json!({}), &[("flake.lock", "original lock")]);
    let mut nix = TestNix {
        fail_lock: true,
        ..TestNix::default()
    };
    error_contains(
        prepare(&mut nix, &root, &repos.path("failed"), false),
        "injected lock failure",
    );
    assert_eq!(
        fs::read_to_string(root.join("flake.lock")).unwrap(),
        "original lock"
    );
    assert!(!repos.path("failed/sources.json").exists());
}
