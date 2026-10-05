use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value};

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
    let mut pending = vec![root.clone()];
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let node = nodes
            .get(&name)
            .with_context(|| format!("missing lock node {name}"))?;
        if let Some(inputs) = node.get("inputs").and_then(Value::as_object) {
            for child in inputs.values() {
                pending.push(resolve(nodes, &root, child, &mut Vec::new())?);
            }
        }
    }
    nodes.retain(|name, _| reachable.contains(name));
    Ok(lock)
}

fn resolve(
    nodes: &Map<String, Value>,
    root: &str,
    link: &Value,
    trail: &mut Vec<Value>,
) -> Result<String> {
    if let Some(name) = link.as_str() {
        return Ok(name.to_owned());
    }
    let path = link.as_array().context("invalid lock input reference")?;
    ensure!(!trail.contains(link), "cycle in lock follows");
    trail.push(link.clone());
    let mut current = root.to_owned();
    for part in path {
        let part = part.as_str().context("invalid lock follows path")?;
        let next = nodes
            .get(&current)
            .and_then(|node| node.get("inputs"))
            .and_then(|inputs| inputs.get(part))
            .with_context(|| format!("missing lock follows edge {current}/{part}"))?;
        current = resolve(nodes, root, next, trail)?;
    }
    trail.pop();
    Ok(current)
}
