use std::{
    ffi::OsStr,
    fs,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::{graph::Sources, nix::Inputs, snapshot::resolve};

const DAEMONS: &[&str] = &[
    "app-daemon",
    "bar-daemon",
    "bt-daemon",
    "clip-daemon",
    "nm-daemon",
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
            let (host, path) = authority
                .split_once('/')
                .map_or((authority, String::new()), |(host, path)| {
                    (host, format!("/{path}"))
                });
            ensure!(
                matches!(host, "" | "localhost"),
                "local inputs must use current worktrees, not refs/revisions: {url}"
            );
            path
        }
        None => file.to_owned(),
    };
    let decoded = percent_encoding::percent_decode_str(&path).collect::<Vec<_>>();
    Ok(Some(resolve(&root.join(OsStr::from_bytes(&decoded)))?))
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

pub(crate) fn nested_inputs(spec: &Value) -> Result<Inputs> {
    match spec.get("inputs") {
        None => Ok(Inputs::new()),
        Some(value) => value
            .as_object()
            .cloned()
            .context("input overrides must be an object"),
    }
}

fn reject_standalone_hyprland(inputs: &Inputs) -> Result<()> {
    for (name, spec) in inputs {
        ensure!(
            !matches!(name.as_str(), "hyprlandIpc" | "shelllist-hyprland"),
            "shelllist-hyprland belongs in daemon-framework, not a separate input"
        );
        reject_standalone_hyprland(&nested_inputs(spec)?)?;
    }
    Ok(())
}

pub(crate) fn validate_policy(sources: &Sources, root_inputs: &Inputs) -> Result<()> {
    reject_standalone_hyprland(root_inputs)?;
    for (source, captured) in sources {
        reject_standalone_hyprland(&captured.inputs)?;
        let name = source
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default();
        if !DAEMONS.contains(&name) {
            continue;
        }
        validate_daemon(name, &captured.directory)?;
        let framework = local_path(
            captured
                .inputs
                .get("daemonFramework")
                .unwrap_or(&Value::Null),
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
    }
    let Some(framework) = root_inputs.get("daemon-framework") else {
        return Ok(());
    };
    ensure!(
        local_path(framework, Path::new("/"))?.is_some()
            && sources
                .keys()
                .any(|source| source.file_name() == Some(OsStr::new("daemon-framework"))),
        "the shared root framework must be a current local checkout"
    );
    for name in DAEMONS {
        if let Some(input) = root_inputs.get(*name) {
            ensure!(
                input
                    .pointer("/inputs/daemonFramework/follows")
                    .and_then(Value::as_str)
                    == Some("daemon-framework"),
                "{name} must follow the single root daemon-framework input"
            );
        }
    }
    if let Some(shelllist) = root_inputs.get("shelllist") {
        for name in std::iter::once(&"daemon-framework").chain(DAEMONS) {
            ensure!(
                shelllist
                    .pointer(&format!("/inputs/{name}/follows"))
                    .and_then(Value::as_str)
                    == Some(*name),
                "shelllist/{name} must follow the root input"
            );
        }
    }
    Ok(())
}

fn validate_daemon(name: &str, directory: &Path) -> Result<()> {
    let manifest: toml::Value = toml::from_str(&fs::read_to_string(directory.join("Cargo.toml"))?)?;
    let dependencies = manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .context("missing Cargo dependencies")?;
    let hyprland = matches!(name, "app-daemon" | "bar-daemon")
        || dependencies.contains_key("shelllist-hyprland");
    let crates = ["shelllist-daemon-core", "shelllist-daemon-tokio"]
        .into_iter()
        .chain(hyprland.then_some("shelllist-hyprland"));
    for name_of_crate in crates {
        let dependency = dependencies
            .get(name_of_crate)
            .and_then(toml::Value::as_table);
        ensure!(
            dependency.is_some_and(|dependency| {
                dependency.get("path").and_then(toml::Value::as_str)
                    == Some(format!("../daemon-framework/crates/{name_of_crate}").as_str())
                    && !["git", "rev", "branch", "tag"]
                        .iter()
                        .any(|key| dependency.contains_key(*key))
            }),
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
