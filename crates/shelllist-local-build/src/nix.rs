use std::{fs, path::Path, process::Command};

use crate::command::{output, run};
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value};

pub(crate) type Inputs = Map<String, Value>;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Override {
    pub edge: String,
    pub store_path: String,
}

/// Only Nix is substituted in tests; filesystem and Git operations stay real.
pub(crate) trait Nix {
    fn inputs(&mut self, root: &Path) -> Result<Inputs>;
    fn add_source(&mut self, source: &Path, gc_root: &Path) -> Result<String>;
    fn lock(&mut self, staged: &Path, overrides: &[Override]) -> Result<()>;
}

pub(crate) struct SystemNix;

pub(crate) fn flake_ref(path: &Path) -> Result<String> {
    Ok(format!(
        "path:{}",
        path.to_str().context("flake path is not UTF-8")?
    ))
}

impl Nix for SystemNix {
    fn inputs(&mut self, root: &Path) -> Result<Inputs> {
        let path = serde_json::to_string(&root.join("flake.nix"))?;
        // Nix double-quoted strings also interpolate ${...}; JSON escaping alone
        // must not turn a directory name into an expression.
        let path = path.replace("${", "\\${");
        let expression = format!("(import (builtins.toPath {path})).inputs");
        serde_json::from_slice(&output(Command::new("nix").args([
            "eval",
            "--impure",
            "--json",
            "--expr",
            &expression,
        ]))?)
        .context("decode flake inputs")
    }

    fn add_source(&mut self, source: &Path, gc_root: &Path) -> Result<String> {
        let bytes = output(
            Command::new("nix")
                .args(["store", "add-path", "--name", "source"])
                .arg(source),
        )?;
        let path = String::from_utf8(bytes)
            .context("decode Nix store path")?
            .trim()
            .to_owned();
        ensure!(
            path.starts_with("/nix/store/"),
            "invalid Nix store path: {path}"
        );
        fs::create_dir_all(gc_root.parent().context("GC root has no parent")?)?;
        output(
            Command::new("nix-store")
                .args(["--realise", &path, "--add-root"])
                .arg(gc_root)
                .arg("--indirect"),
        )?;
        Ok(path)
    }

    fn lock(&mut self, staged: &Path, overrides: &[Override]) -> Result<()> {
        run(&mut lock_command(staged, overrides)?)
    }
}

pub(crate) fn lock_command(staged: &Path, overrides: &[Override]) -> Result<Command> {
    let mut command = Command::new("nix");
    command.args(["flake", "lock", &flake_ref(staged)?]);
    for replacement in overrides {
        command.args([
            "--override-input",
            &replacement.edge,
            &format!("path:{}", replacement.store_path),
        ]);
    }
    command
        .arg("--output-lock-file")
        .arg(staged.join("flake.lock"));
    Ok(command)
}
