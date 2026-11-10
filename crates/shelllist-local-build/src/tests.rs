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
    // A real conflicted index lists one worktree path at three merge stages.
    // Snapshot it once, using the dirty worktree contents rather than any blob.
    let staged = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["ls-files", "--stage", "dir/source", "source-link"])
        .output()
        .unwrap();
    assert!(staged.status.success());
    let staged = String::from_utf8(staged.stdout).unwrap();
    let mut index = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["update-index", "--index-info"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let mut input = index.stdin.take().unwrap();
        for line in staged.lines() {
            let (fields, path) = line.split_once('\t').unwrap();
            let mut fields = fields.split_whitespace();
            let mode = fields.next().unwrap();
            let hash = fields.next().unwrap();
            writeln!(input, "0 {hash}\t{path}").unwrap();
            for stage in 1..=3 {
                writeln!(input, "{mode} {hash} {stage}\t{path}").unwrap();
            }
        }
    }
    assert!(index.wait().unwrap().success());
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
    let result = prepare(&mut nix, &frozen, &repos.path("prepared"), true, false).unwrap();
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
fn source_policy_follows_internal_crates_and_checks_every_framework_edge() {
    let repos = Repositories::new();
    let (root, app, framework) = repos.matrix();
    let nested = fs::read_to_string(app.join("Cargo.toml"))
        .unwrap()
        .replace("../daemon-framework/", "../../../daemon-framework/");
    let root_manifest = "[workspace]\nmembers = [\".\", \"crates/*\"]\n\
        [dependencies]\ninternal = { path = \"crates/internal\" }\n";
    let bar = repos.repository(
        "bar-daemon",
        Value::Object(definitions(&app)),
        &[
            ("Cargo.toml", root_manifest),
            ("crates/internal/Cargo.toml", &nested),
        ],
    );
    let captured = sources(&[&bar, &framework]);
    let root_inputs = definitions(&root);
    validate_policy(&captured, &root_inputs).unwrap();

    // Cyclic internal edges must not recurse forever.
    let cyclic = format!("{nested}\nparent = {{ path = \"../..\" }}\n");
    fs::write(bar.join("crates/internal/Cargo.toml"), &cyclic).unwrap();
    validate_policy(&captured, &root_inputs).unwrap();

    for invalid in [
        nested.replace("../../../daemon-framework/", "../../vendor/daemon-framework/"),
        nested.replace(
            "shelllist-daemon-core = { path",
            "shelllist-daemon-core = { git = \"https://example.org/private\", path",
        ),
        nested.replace(
            "shelllist-daemon-core = { path = \"../../../daemon-framework/crates/shelllist-daemon-core\" }",
            "shelllist-daemon-core = \"0.1\"",
        ),
        nested.replace(
            "shelllist-daemon-core = { path = \"../../../daemon-framework/crates/shelllist-daemon-core\" }",
            "",
        ),
    ] {
        fs::write(bar.join("crates/internal/Cargo.toml"), invalid).unwrap();
        error_contains(
            validate_policy(&captured, &root_inputs),
            "shelllist-daemon-core must use the shared sibling",
        );
    }
    fs::write(bar.join("crates/internal/Cargo.toml"), &nested).unwrap();

    // A valid internal edge must not hide an invalid renamed, dev or target edge.
    for section in [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "target.'cfg(unix)'.dependencies",
    ] {
        let separator = if section == "dependencies" {
            String::new()
        } else {
            format!("[{section}]\n")
        };
        fs::write(
            bar.join("Cargo.toml"),
            format!("{root_manifest}{separator}renamed = {{ package = \"shelllist-daemon-core\", path = \"vendor/core\" }}\n"),
        ).unwrap();
        error_contains(
            validate_policy(&captured, &root_inputs),
            "shelllist-daemon-core must use the shared sibling",
        );
    }
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
        prepare(&mut nix, &a, &repos.path("cycle"), false, false),
        "local input cycle",
    );
    assert!(nix.locks.is_empty());
    error_contains(
        prepare(&mut nix, &a, &repos.path("cycle"), false, false),
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
        prepare(&mut nix, &root, &repos.path("collision"), false, false),
        "basename collision",
    );
    assert!(nix.locks.is_empty());
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
