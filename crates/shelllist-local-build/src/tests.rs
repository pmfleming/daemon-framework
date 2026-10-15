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
    graph::{Source, Sources, prepare, validate_policy},
    lock::prune_lock,
    nix::{Inputs, Nix, Override, lock_command},
    policy::{local_path, merge},
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
                    inputs: definitions(path).into(),
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
        Ok(serde_json::from_slice(&fs::read(
            root.join("inputs.json"),
        )?)?)
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
fn snapshots_capture_dirty_files_preserve_metadata_and_confine_symlinks() {
    let repos = Repositories::new();
    let root = repos.repository(
        "project",
        json!({}),
        &[
            ("dir/source", "old"),
            ("deleted", "old"),
            (".gitignore", "target/\n"),
            ("readonly", "immutable"),
        ],
    );
    fs::write(root.join("forgotten.rs"), "new").unwrap();
    error_contains(snapshot(&root, &repos.path("untracked")), "Git-add");
    fs::remove_file(root.join("forgotten.rs")).unwrap();
    symlink("../outside", root.join("outside")).unwrap();
    git(&root, &["add", "outside"]);
    error_contains(snapshot(&root, &repos.path("escaped")), "symlink escapes");
    fs::remove_file(root.join("outside")).unwrap();
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
    fs::write(root.join("dir/source"), "dirty").unwrap();
    fs::remove_file(root.join("deleted")).unwrap();
    fs::create_dir(root.join("target")).unwrap();
    fs::write(root.join("target/large"), "ignored").unwrap();
    fs::set_permissions(root.join("readonly"), fs::Permissions::from_mode(0o444)).unwrap();
    let frozen = repos.path("frozen");
    fs::set_permissions(root.join("dir/source"), fs::Permissions::from_mode(0o755)).unwrap();
    let original = fs::metadata(root.join("dir/source")).unwrap();
    snapshot(&root, &frozen).unwrap();
    fs::write(root.join("dir/source"), "later edit").unwrap();
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
        for excluded in ["deleted", "target", ".git", "outside"] {
            assert!(!tree.join(excluded).exists());
        }
        let copied = fs::metadata(tree.join("dir/source")).unwrap();
        assert_eq!(copied.modified().unwrap(), original.modified().unwrap());
        assert_eq!(copied.permissions().mode(), original.permissions().mode());
        for (name, target) in links {
            assert_eq!(fs::read_link(tree.join(name)).unwrap(), Path::new(target));
        }
        for link in ["dir/relative", "dir-link/source"] {
            assert_eq!(fs::read_to_string(tree.join(link)).unwrap(), "dirty");
        }
    }
}

#[test]
fn snapshots_share_sources_and_preserve_live_locks_on_success_or_failure() {
    let repos = Repositories::new();
    let (root, app, framework) = repos.matrix();
    fs::write(root.join("flake.lock"), "original lock").unwrap();
    git(&root, &["add", "flake.lock"]);
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
    for source in [&app, &framework] {
        assert!(!source.join("flake.lock").exists());
    }
    let saved: Value =
        serde_json::from_slice(&fs::read(repos.path("snapshots/sources.json")).unwrap()).unwrap();
    assert_eq!(saved, serde_json::to_value(result).unwrap());
    assert_eq!(
        fs::read_to_string(root.join("flake.lock")).unwrap(),
        "original lock"
    );
    nix.fail_lock = true;
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

#[test]
fn source_policy_rejects_pins_divergent_edges_and_private_frameworks() {
    let repos = Repositories::new();
    let (root, app, framework) = repos.matrix();
    let captured = sources(&[&app, &framework]);
    for url in [
        "git+file:../daemon-framework?ref=main",
        "git+file:../daemon-framework?rev=123",
        "git+file:../daemon-framework#main",
        "git+file://remote/project",
    ] {
        let mut root_inputs = definitions(&root);
        root_inputs.insert("daemon-framework".into(), json!({"url":url}));
        error_contains(
            validate_policy(&captured, &root_inputs),
            "current worktrees",
        );
    }
    for (url, expected) in [
        ("git+file:../with%20space".into(), repos.path("with space")),
        (
            format!("git+file://localhost{}", repos.path("project").display()),
            repos.path("project"),
        ),
    ] {
        assert_eq!(
            local_path(&json!({"url":url}), &app).unwrap(),
            Some(expected)
        );
    }
    for (overlay, reason) in [
        (
            json!({"daemon-framework":{"url":"github:pmfleming/daemon-framework/old"}}),
            "current local",
        ),
        (
            json!({"app-daemon":{"inputs":{"daemonFramework":{"follows":"wrong"}}}}),
            "single root",
        ),
        (
            json!({"shelllist-hyprland":{"url":"git+file:../shelllist-hyprland"}}),
            "not a separate input",
        ),
        (
            json!({"bar-daemon":{"inputs":{"hyprlandIpc":{"follows":"shelllist-hyprland"}}}}),
            "not a separate input",
        ),
    ] {
        let mut root_inputs = definitions(&root);
        merge(&mut root_inputs, &inputs(overlay));
        error_contains(validate_policy(&captured, &root_inputs), reason);
    }
    let manifest = fs::read_to_string(app.join("Cargo.toml")).unwrap();
    let bar = repos.repository(
        "bar-daemon",
        Value::Object(definitions(&app)),
        &[("Cargo.toml", &manifest)],
    );
    let valid = "shelllist-hyprland = { path = \"../daemon-framework/crates/shelllist-hyprland\" }";
    let bt = repos.repository(
        "bt-daemon",
        Value::Object(definitions(&app)),
        &[("Cargo.toml", &manifest.replace(valid, ""))],
    );
    let mut sources = sources(&[&app, &bar, &bt, &framework]);
    let root_inputs = definitions(&root);
    validate_policy(&sources, &root_inputs).unwrap();
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
        for private in ["vendor/daemon-framework", "vendor/shelllist-hyprland"] {
            fs::create_dir_all(consumer.join(private)).unwrap();
            error_contains(validate_policy(&sources, &root_inputs), "vendored");
            fs::remove_dir(consumer.join(private)).unwrap();
        }
    }
    std::rc::Rc::make_mut(&mut sources.get_mut(&app).unwrap().inputs).insert(
        "hyprlandIpc".into(),
        json!({"url":"git+file:../shelllist-hyprland"}),
    );
    error_contains(
        validate_policy(&sources, &root_inputs),
        "not a separate input",
    );
}

#[test]
fn lock_pruning_preserves_remote_pins_resolves_follows_and_rejects_cycles() {
    let lock = json!({"root":"root", "version":7, "nodes": {
        "root":{"inputs":{"local":"local", "a":"a", "alias":["a", "dep"]}},
        "local":{"inputs":{"private":"private"}}, "private":{"locked":{"rev":"old"}},
        "a":{"inputs":{"dep":"remote"}}, "remote":{"locked":{"rev":"keep"}}, "unused":{}
    }});
    let result = prune_lock(lock, &["local".into()]).unwrap();
    assert_eq!(
        result["nodes"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["a", "remote", "root"]
    );
    assert!(result["nodes"]["root"]["inputs"].get("local").is_none());
    assert_eq!(result["nodes"]["remote"]["locked"]["rev"], "keep");
    assert_eq!(result["version"], 7);
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
fn preflight_aggregates_worktrees_ignores_follows_and_does_not_write() {
    let repos = Repositories::new();
    let root = repos.repository(
        "root",
        json!({
            "sibling": {"url": "git+file:../sibling"},
            "again": {"url": "git+file:../sibling", "inputs": {} },
            "followed": {"follows": "sibling", "url": "git+file:../missing"}
        }),
        &[],
    );
    let sibling = repos.repository("sibling", json!({}), &[(".gitignore", "ignored/\n")]);
    fs::create_dir(sibling.join("ignored")).unwrap();
    fs::write(sibling.join("ignored/output"), "ignored").unwrap();
    fs::write(sibling.join("image.png"), "untracked").unwrap();
    fs::write(root.join("forgotten.nix"), "untracked").unwrap();
    let mut nix = TestNix::default();
    let error = format!(
        "{:#}",
        crate::preflight::preflight(&mut nix, &root).unwrap_err()
    );
    assert!(
        error.contains("forgotten.nix") && error.contains("image.png"),
        "{error}"
    );
    assert!(
        !error.contains("ignored/output") && !error.contains("missing"),
        "{error}"
    );
    assert_eq!(nix.reads.len(), 2);
    assert!(nix.locks.is_empty() && nix.stores.is_empty());
    assert!(root.join("forgotten.nix").exists() && sibling.join("image.png").exists());
    fs::remove_file(root.join("forgotten.nix")).unwrap();
    fs::remove_file(sibling.join("image.png")).unwrap();
    let report = crate::preflight::preflight(&mut nix, &root).unwrap();
    assert_eq!(report.repositories.len(), 2);
    // Failed evaluations are cached too, even for distinct edge overlays.
    nix.reads.clear();
    fs::write(sibling.join("inputs.json"), "invalid JSON").unwrap();
    let error = crate::preflight::preflight(&mut nix, &root)
        .unwrap_err()
        .to_string();
    assert_eq!(error.matches("Cannot inspect").count(), 1, "{error}");
    assert!(error.contains(sibling.to_str().unwrap()), "{error}");
    assert_eq!(nix.reads.len(), 2);
}

#[test]
fn preflight_walks_distinct_overlays_and_rejects_cycles() {
    let repos = Repositories::new();
    let root = repos.repository(
        "root",
        json!({
            "a": {"url": "git+file:../sibling", "inputs": {"child": {"follows": "a"}}},
            "b": {"url": "git+file:../sibling"}
        }),
        &[],
    );
    repos.repository(
        "sibling",
        json!({"child": {"url": "git+file:../child"}}),
        &[],
    );
    let child = repos.repository("child", json!({}), &[]);
    fs::write(child.join("untracked"), "new").unwrap();
    let mut nix = TestNix::default();
    error_contains(crate::preflight::preflight(&mut nix, &root), "untracked");
    assert_eq!(nix.reads.len(), 3);
    fs::remove_file(child.join("untracked")).unwrap();
    fs::write(
        child.join("inputs.json"),
        json!({"loop": {"url": "git+file:../root"}}).to_string(),
    )
    .unwrap();
    error_contains(crate::preflight::preflight(&mut nix, &root), "cycle");
    for invalid in [Value::Null, json!([]), json!("bad")] {
        fs::write(
            root.join("inputs.json"),
            json!({"sibling": {"url":"git+file:../sibling", "inputs":invalid}}).to_string(),
        )
        .unwrap();
        error_contains(
            crate::preflight::preflight(&mut nix, &root),
            "input overrides must be an object",
        );
    }
}
