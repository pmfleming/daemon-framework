//! Snapshot each current Git worktree once, including tracked dirty files.
//! Persistent locks select third-party inputs, never historical local projects.
//! No checkout, fetch, commit, live lock resolution or service activation occurs.

mod cli;
mod command;
mod graph;
mod lock;
mod nix;
mod policy;
mod preflight;
mod snapshot;

#[cfg(test)]
mod tests;

use clap::Parser;

fn main() -> std::process::ExitCode {
    match cli::Cli::parse().run(&mut nix::SystemNix) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("local-build: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
