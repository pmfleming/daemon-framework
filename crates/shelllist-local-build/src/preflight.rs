//! Diagnostics only. Snapshotting repeats validation after authentication.
use crate::{
    nix::{Inputs, Nix},
    policy::{local_path, merge, nested_inputs},
    snapshot::{resolve, validate_worktree},
};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub repositories: Vec<PathBuf>,
}

pub(crate) fn preflight(nix: &mut impl Nix, root: &Path) -> Result<Report> {
    let mut scan = Scan {
        nix,
        definitions: BTreeMap::new(),
        walked: BTreeSet::new(),
        problems: Vec::new(),
    };
    scan.walk(&resolve(root)?, None, &mut BTreeSet::new());
    ensure!(
        scan.problems.is_empty(),
        "Local worktree preflight failed:\n\n{}\n\nAdd or ignore untracked files explicitly; rebuild never changes Git tracking.",
        scan.problems.join("\n\n")
    );
    Ok(Report {
        repositories: scan.definitions.into_keys().collect(),
    })
}

struct Scan<'a, N> {
    nix: &'a mut N,
    definitions: BTreeMap<PathBuf, Option<Inputs>>,
    walked: BTreeSet<(PathBuf, String)>,
    problems: Vec<String>,
}
impl<N: Nix> Scan<'_, N> {
    fn walk(&mut self, source: &Path, overlay: Option<&Inputs>, ancestors: &mut BTreeSet<PathBuf>) {
        if ancestors.contains(source) {
            self.problems
                .push(format!("Local input cycle at {}", source.display()));
            return;
        }
        // Each repository is inspected once, but distinct follows overlays must
        // still be walked so no nested local worktree escapes diagnostics.
        let key = (
            source.to_owned(),
            serde_json::to_string(&overlay).expect("JSON inputs"),
        );
        if !self.walked.insert(key) {
            return;
        }
        if let std::collections::btree_map::Entry::Vacant(entry) =
            self.definitions.entry(source.to_owned())
        {
            if let Err(error) = validate_worktree(source) {
                self.problems
                    .push(format!("Cannot inspect {}: {error:#}", source.display()));
            }
            let inputs = match self.nix.inputs(source) {
                Ok(inputs) => Some(inputs),
                Err(error) => {
                    self.problems
                        .push(format!("Cannot inspect {}: {error:#}", source.display()));
                    None
                }
            };
            entry.insert(inputs);
        }
        let Some(mut inputs) = self.definitions.get(source).cloned().flatten() else {
            return;
        };
        if let Some(overlay) = overlay {
            merge(&mut inputs, overlay);
        }
        ancestors.insert(source.to_owned());
        for (name, spec) in inputs {
            if spec.get("follows").is_some() {
                continue;
            }
            let edge =
                (|| -> Result<_> { Ok((local_path(&spec, source)?, nested_inputs(&spec)?)) })();
            match edge {
                Ok((Some(child), overlay)) => self.walk(&child, overlay, ancestors),
                Ok((None, _)) => {}
                Err(error) => self.problems.push(format!(
                    "Cannot inspect {}/{name}: {error:#}",
                    source.display()
                )),
            }
        }
        ancestors.remove(source);
    }
}
