use std::{collections::BTreeSet, fs, path::Path};

use crate::{nix::Nix, policy::local_path, snapshot::resolve as resolve_path};
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value};

pub(crate) fn prune_file(nix: &mut impl Nix, root: &Path, lock: &Path) -> Result<()> {
    let root = resolve_path(root)?;
    let mut names = Vec::new();
    for (name, spec) in nix.inputs(&root)? {
        if local_path(&spec, &root)?.is_some() {
            names.push(name);
        }
    }
    let result = prune_lock(serde_json::from_slice(&fs::read(lock)?)?, &names)?;
    fs::write(
        lock,
        format!("{}\n", serde_json::to_string_pretty(&result)?),
    )?;
    Ok(())
}

/// Drop local roots and their now-unreachable nodes; preserve remote pins and
/// unknown lock metadata. Follows paths always resolve from the lock's root.
pub(crate) fn prune_lock(mut lock: Value, names: &[String]) -> Result<Value> {
    let root = lock
        .get("root")
        .and_then(Value::as_str)
        .context("lock has no root")?
        .to_owned();
    let nodes = lock
        .get_mut("nodes")
        .and_then(Value::as_object_mut)
        .context("lock has no nodes")?;
    let inputs = nodes
        .get_mut(&root)
        .and_then(|node| node.get_mut("inputs"))
        .and_then(Value::as_object_mut)
        .context("lock root has no inputs")?;
    for name in names {
        inputs.remove(name);
    }
    let mut reachable = BTreeSet::new();
    let mut pending = vec![root.as_str()];
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.to_owned()) {
            continue;
        }
        let node = nodes
            .get(name)
            .with_context(|| format!("missing lock node {name}"))?;
        for child in node
            .get("inputs")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(Map::values)
        {
            pending.push(resolve(nodes, &root, child, &mut Vec::new())?);
        }
    }
    nodes.retain(|name, _| reachable.contains(name));
    Ok(lock)
}

fn resolve<'a>(
    nodes: &'a Map<String, Value>,
    root: &'a str,
    link: &'a Value,
    trail: &mut Vec<&'a Value>,
) -> Result<&'a str> {
    if let Some(name) = link.as_str() {
        return Ok(name);
    }
    let path = link.as_array().context("invalid lock input reference")?;
    ensure!(!trail.contains(&link), "cycle in lock follows");
    trail.push(link);
    let mut current = root;
    for part in path {
        let part = part.as_str().context("invalid lock follows path")?;
        let next = nodes
            .get(current)
            .and_then(|node| node.get("inputs"))
            .and_then(|inputs| inputs.get(part))
            .with_context(|| format!("missing lock follows edge {current}/{part}"))?;
        current = resolve(nodes, root, next, trail)?;
    }
    trail.pop();
    Ok(current)
}
