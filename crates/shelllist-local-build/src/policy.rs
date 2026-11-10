use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::{nix::Inputs, snapshot::resolve};

const DAEMONS: &[&str] = &[
    "app-daemon",
    "bar-daemon",
    "bt-daemon",
    "clip-daemon",
    "nm-daemon",
];

const FRAMEWORK_CRATES: &[&str] = &[
    "shelllist-daemon-core",
    "shelllist-daemon-tokio",
    "shelllist-hyprland",
];

pub(crate) fn local_path(spec: &Value, root: &Path) -> Result<Option<PathBuf>> {
    let Some(url) = spec.get("url").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(file) = url.strip_prefix("git+file:") else {
        return Ok(None);
    };
    ensure!(
        !file.contains(['?', '#']),
        "local inputs must use current worktrees, not refs/revisions: {url}"
    );
    let path = match file.strip_prefix("//") {
        Some(authority) => {
            let (host, path) = authority.split_at(authority.find('/').unwrap_or(authority.len()));
            ensure!(
                matches!(host, "" | "localhost"),
                "local inputs must use current worktrees, not refs/revisions: {url}"
            );
            path
        }
        None => file,
    };
    let decoded = percent_encoding::percent_decode_str(path).collect::<Vec<_>>();
    Ok(Some(resolve(&root.join(OsStr::from_bytes(&decoded)))?))
}

/// Repeated edges share captured inputs; only a nonempty overlay can copy.
pub(crate) fn overlaid(mut inputs: Rc<Inputs>, overlay: Option<&Inputs>) -> Rc<Inputs> {
    if let Some(overlay) = overlay.filter(|overlay| !overlay.is_empty()) {
        merge(Rc::make_mut(&mut inputs), overlay);
    }
    inputs
}

pub(crate) fn merge(target: &mut Inputs, overlay: &Inputs) {
    for (key, value) in overlay {
        match (
            target.get_mut(key).and_then(Value::as_object_mut),
            value.as_object(),
        ) {
            (Some(left), Some(right)) => merge(left, right),
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Follows edges reuse another input; their URL and nested overrides are not traversed.
pub(crate) fn direct_inputs(inputs: &Inputs) -> impl Iterator<Item = (&String, &Value)> {
    inputs
        .iter()
        .filter(|(_, spec)| spec.get("follows").is_none())
}

pub(crate) fn nested_inputs(spec: &Value) -> Result<Option<&Inputs>> {
    spec.get("inputs")
        .map(|value| {
            value
                .as_object()
                .context("input overrides must be an object")
        })
        .transpose()
}

pub(crate) fn reject_standalone_hyprland(inputs: &Inputs) -> Result<()> {
    for (name, spec) in inputs {
        ensure!(
            !matches!(name.as_str(), "hyprlandIpc" | "shelllist-hyprland"),
            "shelllist-hyprland belongs in daemon-framework, not a separate input"
        );
        if let Some(inputs) = nested_inputs(spec)? {
            reject_standalone_hyprland(inputs)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_root(root_inputs: &Inputs, framework_captured: bool) -> Result<()> {
    let Some(framework) = root_inputs.get("daemon-framework") else {
        return Ok(());
    };
    ensure!(
        local_path(framework, Path::new("/"))?.is_some() && framework_captured,
        "the shared root framework must be a current local checkout"
    );
    for name in DAEMONS {
        if let Some(input) = root_inputs.get(*name) {
            ensure!(
                input["inputs"]["daemonFramework"]["follows"].as_str() == Some("daemon-framework"),
                "{name} must follow the single root daemon-framework input"
            );
        }
    }
    if let Some(shelllist) = root_inputs.get("shelllist") {
        for name in std::iter::once(&"daemon-framework").chain(DAEMONS) {
            ensure!(
                shelllist["inputs"][*name]["follows"].as_str() == Some(*name),
                "shelllist/{name} must follow the root input"
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_source(source: &Path, directory: &Path, inputs: &Inputs) -> Result<()> {
    reject_standalone_hyprland(inputs)?;
    let name = source
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if !DAEMONS.contains(&name) {
        return Ok(());
    }
    validate_daemon(name, directory)?;
    let framework = local_path(
        inputs.get("daemonFramework").unwrap_or(&Value::Null),
        source,
    )?;
    ensure!(
        framework.as_deref()
            == source
                .parent()
                .map(|parent| parent.join("daemon-framework"))
                .as_deref(),
        "{name}: daemonFramework must use the current sibling"
    );
    Ok(())
}

/// Include ordinary, development and build edges in every target section.
fn dependencies(manifest: &toml::Value) -> impl Iterator<Item = (&String, &toml::Value)> {
    std::iter::once(manifest)
        .chain(
            manifest
                .get("target")
                .and_then(toml::Value::as_table)
                .into_iter()
                .flat_map(|targets| targets.values()),
        )
        .flat_map(|section| {
            ["dependencies", "dev-dependencies", "build-dependencies"]
                .into_iter()
                .filter_map(move |kind| section.get(kind).and_then(toml::Value::as_table))
        })
        .flatten()
}

/// Follow internal Cargo path dependencies without invoking Cargo (or fetching).
/// Check every framework edge, not just the first valid occurrence of a crate.
fn validate_dependencies(
    name: &str,
    root: &Path,
    directory: &Path,
    visited: &mut BTreeSet<PathBuf>,
    found: &mut BTreeSet<&'static str>,
) -> Result<()> {
    if !visited.insert(directory.to_path_buf()) {
        return Ok(());
    }
    let path = directory.join("Cargo.toml");
    let manifest: toml::Value = toml::from_str(
        &fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?,
    )?;
    for (alias, dependency) in dependencies(&manifest) {
        let package = dependency
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(alias);
        let dependency_path = dependency
            .get("path")
            .and_then(toml::Value::as_str)
            .map(|path| resolve(&directory.join(path)))
            .transpose()?;
        if let Some(&package) = FRAMEWORK_CRATES.iter().find(|&&name| name == package) {
            let expected = resolve(&root.join(format!("../daemon-framework/crates/{package}")))?;
            ensure!(
                dependency_path.as_ref() == Some(&expected)
                    && !["git", "rev", "branch", "tag"]
                        .iter()
                        .any(|key| dependency.get(*key).is_some()),
                "{name}: {package} must use the shared sibling framework ({})",
                path.display()
            );
            found.insert(package);
        } else if let Some(path) = dependency_path.filter(|path| path.starts_with(root)) {
            validate_dependencies(name, root, &path, visited, found)?;
        }
    }
    Ok(())
}

fn validate_daemon(name: &str, directory: &Path) -> Result<()> {
    let directory = resolve(directory)?;
    let mut visited = BTreeSet::new();
    let mut found = BTreeSet::new();
    validate_dependencies(name, &directory, &directory, &mut visited, &mut found)?;
    let required = if matches!(name, "app-daemon" | "bar-daemon") {
        3
    } else {
        2
    };
    for name_of_crate in &FRAMEWORK_CRATES[..required] {
        ensure!(
            found.contains(name_of_crate),
            "{name}: {name_of_crate} must use the shared sibling framework"
        );
    }
    for private in ["vendor/daemon-framework", "vendor/shelllist-hyprland"] {
        ensure!(
            !directory.join(private).exists(),
            "{name}: remove the private vendored framework crate at {private}"
        );
    }
    Ok(())
}
