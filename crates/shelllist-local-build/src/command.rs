//! Checked child-process execution shared by Git, Nix and CLI invocations.
use anyhow::{Context, Result, ensure};
use std::process::{Command, Stdio};

pub(crate) fn output(command: &mut Command) -> Result<Vec<u8>> {
    let result = command
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("run {command:?}"))?;
    ensure!(
        result.status.success(),
        "{command:?} failed: {}",
        result.status
    );
    Ok(result.stdout)
}

pub(crate) fn run(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("run {command:?}"))?;
    ensure!(status.success(), "{command:?} failed: {status}");
    Ok(())
}
