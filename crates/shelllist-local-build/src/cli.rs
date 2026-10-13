use std::{ffi::OsString, os::unix::ffi::OsStrExt, path::PathBuf, process::Command};

use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand};

use crate::{
    command::run,
    graph::{prepare, prepare_with_capture},
    lock::prune_file,
    nix::Nix,
    preflight::preflight,
};

#[derive(Debug, Parser)]
#[command(
    name = "local-build",
    version,
    about = "Build one snapshot of the current local co-development graph, never historical project pins"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Snapshot the graph and resolve only its disposable lock.
    Prepare {
        root: PathBuf,
        destination: PathBuf,
        #[arg(long)]
        root_is_snapshot: bool,
        /// Retain the captured root before lock resolution for baseline approval.
        #[arg(long)]
        capture_root: bool,
    },
    /// List every discoverable worktree problem without snapshotting or building.
    Preflight {
        root: PathBuf,
    },
    /// Remove local inputs from an explicitly supplied lock, retaining remote pins.
    PruneLock {
        root: PathBuf,
        lock: PathBuf,
    },
    Check(Invocation),
    Build(Target),
    Develop(Target),
    Run(Target),
}

#[derive(Debug, Args)]
struct Target {
    /// Package/app/devShell attribute (before root).
    #[arg(long)]
    attr: Option<String>,
    #[command(flatten)]
    invocation: Invocation,
}

#[derive(Debug, Args)]
struct Invocation {
    /// Repository root, followed by arguments passed verbatim to Nix/the app.
    #[arg(required = true, num_args = 1.., trailing_var_arg = true, value_name = "ROOT_AND_ARGS")]
    arguments: Vec<OsString>,
}

impl Cli {
    pub(crate) fn run(self, nix: &mut impl Nix) -> Result<()> {
        match self.command {
            Action::Prepare {
                root,
                destination,
                root_is_snapshot,
                capture_root,
            } => print_report(prepare_with_capture(
                nix,
                &root,
                &destination,
                root_is_snapshot,
                capture_root,
            )?),
            Action::Preflight { root } => print_report(preflight(nix, &root)?),
            Action::PruneLock { root, lock } => prune_file(nix, &root, &lock),
            Action::Check(invocation) => invocation.run(nix, "check", None),
            Action::Build(target) => target.invocation.run(nix, "build", target.attr),
            Action::Develop(target) => target.invocation.run(nix, "develop", target.attr),
            Action::Run(target) => target.invocation.run(nix, "run", target.attr),
        }
    }
}

fn print_report(report: impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

fn validate_args(args: &[OsString]) -> Result<()> {
    const FORBIDDEN: &[&str] = &[
        "--override-input",
        "--update-input",
        "--recreate-lock-file",
        "--override-flake",
        "--flake",
    ];
    ensure!(
        !args.iter().any(|arg| {
            let flag = arg
                .as_bytes()
                .split(|byte| *byte == b'=')
                .next()
                .unwrap_or_default();
            FORBIDDEN
                .iter()
                .any(|forbidden| flag == forbidden.as_bytes())
        }),
        "source overrides would break the single-snapshot guarantee"
    );
    Ok(())
}

impl Invocation {
    fn parts(&self) -> Result<(&std::path::Path, &[OsString])> {
        let (root, args) = self
            .arguments
            .split_first()
            .context("missing repository root")?;
        let args = if args.first().is_some_and(|arg| arg == "--") {
            &args[1..]
        } else {
            args
        };
        Ok((std::path::Path::new(root), args))
    }

    fn run(self, nix: &mut impl Nix, action: &str, attr: Option<String>) -> Result<()> {
        let (root, args) = self.parts()?;
        validate_args(args)?;
        let temporary = tempfile::Builder::new().prefix("local-build-").tempdir()?;
        let prepared = prepare(nix, root, &temporary.path().join("sources"), false)?;
        eprintln!("{}", serde_json::to_string_pretty(&prepared)?);
        let installable = match attr {
            Some(attr) => format!("{}#{attr}", prepared.flake),
            None => prepared.flake,
        };
        run(&mut invocation_command(action, &installable, args))
    }
}

fn invocation_command(action: &str, installable: &str, args: &[OsString]) -> Command {
    let mut command = Command::new("nix");
    if action == "check" {
        command.arg("flake");
    }
    command.args([action, installable, "--no-update-lock-file"]);
    if action == "run" {
        command.arg("--");
    }
    command.args(args);
    command
}
