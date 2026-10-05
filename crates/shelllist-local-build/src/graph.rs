use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use crate::{
    nix::{Inputs, Nix, Override, flake_ref},
    policy::{local_path, merge, nested_inputs, validate_policy},
    snapshot::{copy_tree, resolve, snapshot},
};

pub(crate) struct Source {
    pub directory: PathBuf,
    pub inputs: Rc<Inputs>,
}
pub(crate) type Sources = BTreeMap<PathBuf, Source>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Prepared {
    pub flake: String,
    pub sources: BTreeMap<PathBuf, PathBuf>,
    pub store_sources: BTreeMap<PathBuf, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_root: Option<PathBuf>,
}

pub(crate) fn prepare(
    nix: &mut impl Nix,
    root: &Path,
    destination: &Path,
    root_is_snapshot: bool,
) -> Result<Prepared> {
    prepare_with_capture(nix, root, destination, root_is_snapshot, false)
}

/// Optionally retain the exact captured root before disposable lock resolution.
/// Approval consumers must never reconstruct that identity from a live checkout.
pub(crate) fn prepare_with_capture(
    nix: &mut impl Nix,
    root: &Path,
    destination: &Path,
    root_is_snapshot: bool,
    capture_root: bool,
) -> Result<Prepared> {
    let root = resolve(root)?;
    let destination = resolve(destination)?;
    ensure!(
        !destination.exists(),
        "snapshot destination already exists: {}",
        destination.display()
    );
    ensure!(
        !root_is_snapshot || !destination.starts_with(&root),
        "snapshot destination must be outside the frozen root"
    );
    fs::create_dir_all(&destination)?;
    let mut graph = Graph {
        nix,
        root: &root,
        destination: &destination,
        root_is_snapshot,
        sources: Sources::new(),
        store_sources: BTreeMap::new(),
        overrides: Vec::new(),
    };
    graph.walk(&root, None, "", &mut BTreeSet::new())?;
    let source = graph.sources.get(&root).context("root was not captured")?;
    validate_policy(&graph.sources, &source.inputs)?;
    let original_root = if capture_root {
        let original = destination.join(".approval-root");
        ensure!(!original.exists(), "approval snapshot basename collision");
        copy_tree(&source.directory, &original)?;
        Some(original)
    } else {
        None
    };
    graph.nix.lock(&source.directory, &graph.overrides)?;
    let result = Prepared {
        original_root,
        flake: flake_ref(&source.directory)?,
        sources: graph
            .sources
            .into_iter()
            .map(|(path, source)| (path, source.directory))
            .collect(),
        store_sources: graph.store_sources,
    };
    fs::write(
        destination.join("sources.json"),
        format!("{}\n", serde_json::to_string_pretty(&result)?),
    )?;
    Ok(result)
}

struct Graph<'a, N> {
    nix: &'a mut N,
    root: &'a Path,
    destination: &'a Path,
    root_is_snapshot: bool,
    sources: Sources,
    store_sources: BTreeMap<PathBuf, String>,
    overrides: Vec<Override>,
}

impl<N: Nix> Graph<'_, N> {
    fn capture(&mut self, source: &Path) -> Result<()> {
        if self.sources.contains_key(source) {
            return Ok(());
        }
        let name = if source == self.root {
            std::ffi::OsStr::new("root")
        } else {
            source.file_name().context("repository has no basename")?
        };
        let target = self.destination.join(name);
        ensure!(
            !target.exists(),
            "local repository basename collision: {}",
            source.display()
        );
        if source == self.root && self.root_is_snapshot {
            copy_tree(source, &target)?;
        } else {
            snapshot(source, &target)?;
        }
        let inputs = self.nix.inputs(&target)?;
        self.sources.insert(
            source.to_owned(),
            Source {
                directory: target,
                inputs: Rc::new(inputs),
            },
        );
        Ok(())
    }

    fn immutable(&mut self, source: &Path) -> Result<String> {
        if let Some(path) = self.store_sources.get(source) {
            return Ok(path.clone());
        }
        let captured = self
            .sources
            .get(source)
            .context("source was not captured")?;
        let gc_root = self
            .destination
            .join("gc-roots")
            .join(source.file_name().context("repository has no basename")?);
        // Stable content-addressed paths retain standalone build caches. Keep
        // them rooted for the entire command, including interactive shells.
        let path = self.nix.add_source(&captured.directory, &gc_root)?;
        self.store_sources.insert(source.to_owned(), path.clone());
        Ok(path)
    }

    fn walk(
        &mut self,
        source: &Path,
        overlay: Option<&Inputs>,
        prefix: &str,
        ancestors: &mut BTreeSet<PathBuf>,
    ) -> Result<()> {
        ensure!(
            ancestors.insert(source.to_owned()),
            "local input cycle at {}",
            source.display()
        );
        self.capture(source)?;
        let mut inputs = self
            .sources
            .get(source)
            .context("source was not captured")?
            .inputs
            .clone();
        // Repeated edges share captured inputs; only an overlaid edge copies.
        if let Some(overlay) = overlay {
            merge(Rc::make_mut(&mut inputs), overlay);
        }
        for (name, spec) in inputs.iter() {
            if spec.get("follows").is_some() {
                continue;
            }
            let Some(child) = local_path(spec, source)? else {
                continue;
            };
            self.capture(&child)?;
            let edge = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let store_path = self.immutable(&child)?;
            self.overrides.push(Override {
                edge: edge.clone(),
                store_path,
            });
            self.walk(&child, nested_inputs(spec)?, &edge, ancestors)?;
        }
        ancestors.remove(source);
        Ok(())
    }
}
