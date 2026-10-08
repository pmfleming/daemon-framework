//! Diagnostics only. Snapshotting repeats validation after authentication.
use crate::{
    nix::{Inputs, Nix},
    policy::{local_path, nested_inputs, overlaid},
    snapshot::{resolve, validate_worktree},
};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::Rc,
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
    scan.walk(&resolve(root)?, None, &mut BTreeSet::new())?;
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
    definitions: BTreeMap<PathBuf, Option<Rc<Inputs>>>,
    walked: BTreeSet<(PathBuf, String)>,
    problems: Vec<String>,
}
impl<N: Nix> Scan<'_, N> {
    fn inputs(&mut self, source: &Path) -> Option<Rc<Inputs>> {
        let problems = &mut self.problems;
        self.definitions
            .entry(source.to_owned())
            .or_insert_with(|| {
                inspect(problems, source.display(), validate_worktree(source));
                inspect(problems, source.display(), self.nix.inputs(source)).map(Rc::new)
            })
            .clone()
    }

    fn walk(
        &mut self,
        source: &Path,
        overlay: Option<&Inputs>,
        ancestors: &mut BTreeSet<PathBuf>,
    ) -> Result<()> {
        if ancestors.contains(source) {
            self.problems
                .push(format!("Local input cycle at {}", source.display()));
            return Ok(());
        }
        // Each repository is inspected once, but distinct follows overlays must
        // still be walked so no nested local worktree escapes diagnostics.
        let key = (source.to_owned(), serde_json::to_string(&overlay)?);
        if !self.walked.insert(key) {
            return Ok(());
        }
        let Some(inputs) = self.inputs(source) else {
            return Ok(());
        };
        let inputs = overlaid(inputs, overlay);
        ancestors.insert(source.to_owned());
        for (name, spec) in inputs.iter() {
            if spec.get("follows").is_some() {
                continue;
            }
            let edge = local_path(spec, source).and_then(|child| Ok((child, nested_inputs(spec)?)));
            if let Some((Some(child), overlay)) = inspect(
                &mut self.problems,
                format_args!("{}/{name}", source.display()),
                edge,
            ) {
                self.walk(&child, overlay, ancestors)?;
            }
        }
        ancestors.remove(source);
        Ok(())
    }
}

fn inspect<T>(
    problems: &mut Vec<String>,
    source: impl std::fmt::Display,
    result: Result<T>,
) -> Option<T> {
    result
        .inspect_err(|error| problems.push(format!("Cannot inspect {source}: {error:#}")))
        .ok()
}
