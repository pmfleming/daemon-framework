#!/usr/bin/env python3
"""Build the current local co-development graph, never historical project pins.

INVARIANT: snapshot each Git worktree once (including tracked dirty files), then
use that same graph for checks and builds. Persistent locks select third-party
inputs ONLY. Do not replace this with `flake update`, branch refs, vendoring, or
--no-write-lock-file alone: none guarantees one current framework for consumers.

No checkout, fetch, commit, live lock write, or service activation occurs here.
"""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from urllib.parse import unquote, urlsplit


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def git(root, *args):
    return run("git", "-c", f"safe.directory={root}", "-C", str(root), *args,
               stdout=subprocess.PIPE).stdout


def snapshot(root, destination):
    """Copy only tracked worktree files; never copy ignored Cargo targets/.git."""
    if Path(os.fsdecode(git(root, "rev-parse", "--show-toplevel")).strip()).resolve() != root:
        raise ValueError(f"not a repository root: {root}")
    untracked = git(root, "ls-files", "--others", "--exclude-standard", "-z")
    if untracked:
        names = os.fsdecode(untracked).replace("\0", "\n")
        raise ValueError(f"Git-add or ignore untracked files in {root}:\n{names}")
    destination.mkdir(parents=True)
    files = git(root, "ls-files", "--cached", "-z").split(b"\0")
    for entry in sorted(set(files)):
        if not entry:
            continue
        relative = Path(os.fsdecode(entry))
        source, target = root / relative, destination / relative
        if not source.exists() and not source.is_symlink():
            continue  # A tracked, uncommitted deletion is part of the worktree.
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.is_symlink():
            if not source.resolve().is_relative_to(root):
                raise ValueError(f"symlink escapes snapshot: {source}")
            target.symlink_to(os.readlink(source))
        elif source.is_file():
            before = source.stat()
            shutil.copy2(source, target)
            after = source.stat()
            if (before.st_mtime_ns, before.st_size) != (after.st_mtime_ns, after.st_size):
                raise ValueError(f"file changed while snapshotting; retry: {source}")
        else:
            raise ValueError(f"unsupported tracked directory/submodule: {source}")


def read_inputs(root):
    expression = f"(import (builtins.toPath {json.dumps(str(root / 'flake.nix'))})).inputs"
    return json.loads(run("nix", "eval", "--impure", "--json", "--expr", expression,
                          stdout=subprocess.PIPE).stdout)


def local_path(spec, root):
    url = spec.get("url", "")
    if not url.startswith("git+file:"):
        return None
    parsed = urlsplit(url.removeprefix("git+"))
    if parsed.query or parsed.fragment or parsed.netloc not in ("", "localhost"):
        raise ValueError(f"local inputs must use current worktrees, not refs/revisions: {url}")
    path = Path(unquote(parsed.path))
    return (root / path).resolve()


def merge(left, right):
    result = dict(left)
    for key, value in right.items():
        if isinstance(value, dict) and isinstance(result.get(key), dict):
            result[key] = merge(result[key], value)
        else:
            result[key] = value
    return result


def prune_lock(lock, names):
    """Drop local roots and their now-unreachable nodes; preserve remote pins."""
    nodes = lock["nodes"]
    root = lock["root"]
    for name in names:
        nodes[root]["inputs"].pop(name, None)
    reachable = set()

    def resolve(path, trail=()):
        if tuple(path) in trail:
            raise ValueError("cycle in lock follows")
        current = root
        for part in path:
            current = nodes[current]["inputs"][part]
            if isinstance(current, list):
                current = resolve(current, (*trail, tuple(path)))
        return current

    def visit(name):
        if name in reachable:
            return
        reachable.add(name)
        for child in nodes[name].get("inputs", {}).values():
            if isinstance(child, str):
                visit(child)
            elif isinstance(child, list):
                visit(resolve(child))

    visit(root)
    lock["nodes"] = {key: value for key, value in nodes.items() if key in reachable}
    return lock


def validate_policy(sources, root_inputs):
    """Fail closed if packaging slowly reintroduces a private framework."""
    for source in sources:
        if source.name not in {"app-daemon", "bar-daemon", "bt-daemon", "clip-daemon", "nm-daemon"}:
            continue
        import tomllib
        cargo = tomllib.loads((sources[source] / "Cargo.toml").read_text())
        for crate in ("shelllist-daemon-core", "shelllist-daemon-tokio"):
            dependency = cargo["dependencies"][crate]
            if dependency.get("path") != f"../daemon-framework/crates/{crate}" or any(
                key in dependency for key in ("git", "rev", "branch", "tag")
            ):
                raise ValueError(f"{source.name}: {crate} must use the shared sibling framework")
        if (sources[source] / "vendor/daemon-framework").exists():
            raise ValueError(f"{source.name}: remove the private vendored framework")
        inputs = read_inputs(sources[source])
        framework = local_path(inputs.get("daemonFramework", {}), source)
        if framework != source.parent / "daemon-framework":
            raise ValueError(f"{source.name}: daemonFramework must use the current sibling")
    if "daemon-framework" in root_inputs:
        if local_path(root_inputs["daemon-framework"], Path("/")) is None or not any(
            source.name == "daemon-framework" for source in sources
        ):
            raise ValueError("the shared root framework must be a current local checkout")
        for name in ("app-daemon", "bar-daemon", "bt-daemon", "clip-daemon", "nm-daemon"):
            if name in root_inputs and root_inputs[name].get("inputs", {}).get("daemonFramework", {}).get("follows") != "daemon-framework":
                raise ValueError(f"{name} must follow the single root daemon-framework input")
        if "shelllist" in root_inputs:
            for name in ("daemon-framework", "app-daemon", "bar-daemon", "bt-daemon", "clip-daemon", "nm-daemon"):
                if root_inputs["shelllist"].get("inputs", {}).get(name, {}).get("follows") != name:
                    raise ValueError(f"shelllist/{name} must follow the root input")


def prepare(root, destination, root_is_snapshot=False):
    root, destination = root.resolve(), destination.resolve()
    if destination.exists():
        raise ValueError(f"snapshot destination already exists: {destination}")
    destination.mkdir(parents=True)
    sources = {}
    store_sources = {}
    definitions = {}
    overrides = []

    def immutable(source):
        if source not in store_sources:
            # Stable content-addressed paths keep unchanged standalone builds
            # cached; random /tmp paths in a root Cargo src's lock would force
            # recompilation on every invocation. Root these until we finish.
            path = run("nix", "store", "add-path", "--name", "source", str(sources[source]),
                       stdout=subprocess.PIPE, text=True).stdout.strip()
            gc_root = destination / "gc-roots" / source.name
            gc_root.parent.mkdir(exist_ok=True)
            run("nix-store", "--realise", path, "--add-root", str(gc_root), "--indirect",
                stdout=subprocess.DEVNULL)
            store_sources[source] = path
        return store_sources[source]

    def capture(source):
        if source not in sources:
            target = destination / ("root" if source == root else source.name)
            if target.exists():
                raise ValueError(f"local repository basename collision: {source}")
            if source == root and root_is_snapshot:
                shutil.copytree(source, target)
            else:
                snapshot(source, target)
            sources[source] = target
            definitions[source] = read_inputs(target)
        return sources[source]

    def walk(source, overlay, prefix, ancestors):
        if source in ancestors:
            raise ValueError(f"local input cycle at {source}")
        target = capture(source)
        for name, spec in merge(definitions[source], overlay).items():
            if "follows" in spec:
                continue
            child = local_path(spec, source)
            if child is None:
                continue
            capture(child)
            edge = f"{prefix}/{name}" if prefix else name
            overrides.extend(["--override-input", edge, f"path:{immutable(child)}"])
            walk(child, spec.get("inputs", {}), edge, ancestors | {source})
        return target

    staged = walk(root, {}, "", set())
    validate_policy(sources, definitions[root])
    # Old local lock entries must never win, even when dependency topology changes.
    # Only this disposable lock is resolved; source repositories stay untouched.
    run("nix", "flake", "lock", f"path:{staged}", *overrides,
        "--output-lock-file", str(staged / "flake.lock"))
    result = {"flake": f"path:{staged}", "sources": {str(k): str(v) for k, v in sources.items()},
              "storeSources": {str(k): v for k, v in store_sources.items()}}
    (destination / "sources.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prepare_parser = sub.add_parser("prepare")
    prepare_parser.add_argument("root", type=Path)
    prepare_parser.add_argument("destination", type=Path)
    prepare_parser.add_argument("--root-is-snapshot", action="store_true")
    prune_parser = sub.add_parser("prune-lock")
    prune_parser.add_argument("root", type=Path)
    prune_parser.add_argument("lock", type=Path)
    for command in ("check", "build", "develop", "run"):
        child = sub.add_parser(command)
        if command != "check":
            child.add_argument("--attr", help="package/app/devShell attribute (before root)")
        child.add_argument("root", type=Path)
        child.add_argument("args", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command == "prepare":
        print(json.dumps(prepare(args.root, args.destination, args.root_is_snapshot)))
    elif args.command == "prune-lock":
        inputs = read_inputs(args.root.resolve())
        names = [name for name, spec in inputs.items() if local_path(spec, args.root)]
        lock = prune_lock(json.loads(args.lock.read_text()), names)
        args.lock.write_text(json.dumps(lock, indent=2) + "\n")
    else:
        extra = args.args
        if extra[:1] == ["--"]:
            extra = extra[1:]
        if any(arg.split("=")[0] in {"--override-input", "--update-input", "--recreate-lock-file", "--override-flake", "--flake"} for arg in extra):
            raise ValueError("source overrides would break the single-snapshot guarantee")
        with tempfile.TemporaryDirectory(prefix="local-build-") as temporary:
            result = prepare(args.root, Path(temporary) / "sources")
            print(json.dumps(result, indent=2), file=sys.stderr)
            command = ["nix", "flake", "check"] if args.command == "check" else ["nix", args.command]
            installable = result["flake"]
            if getattr(args, "attr", None):
                installable += "#" + args.attr
            if args.command == "run":
                extra = ["--", *extra]
            run(*command, installable, "--no-update-lock-file", *extra)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
