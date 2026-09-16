#!/usr/bin/env python3
"""Regression tests for live-source selection. No Nix/network/builds required."""
import importlib.util
import json
import sys
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("local_build", Path(__file__).with_name("local-build.py"))
local = importlib.util.module_from_spec(spec)
spec.loader.exec_module(local)


class LocalBuildTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)

    def repository(self, name, inputs=None, files=None):
        root = self.base / name
        root.mkdir()
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        (root / "flake.nix").write_text("{}\n")
        (root / "inputs.json").write_text(json.dumps(inputs or {}))
        for name, text in (files or {}).items():
            target = root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text)
        subprocess.run(["git", "-C", str(root), "add", "."], check=True)
        return root

    def test_dirty_files_deletions_and_ignored_targets(self):
        root = self.repository("project", files={"source": "old", "deleted": "old", ".gitignore": "target/\n"})
        (root / "source").write_text("dirty")
        (root / "deleted").unlink()
        (root / "target").mkdir()
        (root / "target/large").write_text("ignored")
        destination = self.base / "snapshot"
        local.snapshot(root, destination)
        (root / "source").write_text("later edit")
        self.assertEqual((destination / "source").read_text(), "dirty")
        for name in ("deleted", "target", ".git"):
            self.assertFalse((destination / name).exists())

    def test_untracked_file_rejected(self):
        root = self.repository("project")
        (root / "forgotten.rs").write_text("new")
        with self.assertRaisesRegex(ValueError, "Git-add"):
            local.snapshot(root, self.base / "snapshot")

    def test_escaping_symlink_rejected(self):
        root = self.repository("project")
        (root / "outside").symlink_to("../outside")
        subprocess.run(["git", "-C", str(root), "add", "outside"], check=True)
        with self.assertRaisesRegex(ValueError, "symlink escapes"):
            local.snapshot(root, self.base / "snapshot")

    def test_branch_and_revision_pins_rejected(self):
        for suffix in ("?ref=main", "?rev=123", "#main"):
            with self.assertRaises(ValueError):
                local.local_path({"url": "git+file:../daemon-framework" + suffix}, self.base)
        self.assertEqual(local.local_path({"url": "git+file:../daemon-framework"}, self.base / "app-daemon"), self.base / "daemon-framework")

    def test_remote_lock_preserved_local_graph_removed(self):
        lock = {"root": "root", "version": 7, "nodes": {
            "root": {"inputs": {"nixpkgs": "nixpkgs", "app-daemon": "app", "daemon-framework": "framework"}},
            "app": {"inputs": {"daemonFramework": ["daemon-framework"], "private": "private"}},
            "framework": {"inputs": {"nixpkgs": ["nixpkgs"]}},
            "private": {"locked": {"rev": "old"}},
            "nixpkgs": {"locked": {"rev": "keep-exactly"}},
        }}
        result = local.prune_lock(lock, ["app-daemon", "daemon-framework"])
        self.assertEqual(set(result["nodes"]), {"root", "nixpkgs"})
        self.assertEqual(result["nodes"]["nixpkgs"]["locked"]["rev"], "keep-exactly")

    def matrix(self):
        framework = self.repository("daemon-framework", files={"source": "current"})
        cargo = '[dependencies]\n' + "\n".join(
            f'{crate} = {{ path = "../daemon-framework/crates/{crate}" }}'
            for crate in ("shelllist-daemon-core", "shelllist-daemon-tokio")
        )
        app = self.repository("app-daemon", {"daemonFramework": {"url": "git+file:../daemon-framework"}}, {"Cargo.toml": cargo})
        root = self.repository("shelllist", {
            "daemon-framework": {"url": "git+file:../daemon-framework"},
            "app-daemon": {"url": "git+file:../app-daemon", "inputs": {"daemonFramework": {"follows": "daemon-framework"}}},
        })
        return root, app, framework

    @staticmethod
    def inputs(root):
        return json.loads((root / "inputs.json").read_text())

    def test_one_snapshot_shared_by_all_edges_and_no_live_lock_write(self):
        root, app, framework = self.matrix()
        (framework / "source").write_text("dirty framework")
        with patch.object(local, "read_inputs", self.inputs), patch.object(local, "run") as run:
            # Keep real Git operations while mocking only Nix lock resolution.
            def command(*args, **kwargs):
                if args[0] == "git":
                    return subprocess.run(args, check=True, **kwargs)
                return subprocess.CompletedProcess(args, 0, stdout="/nix/store/mock-source\n")
            run.side_effect = command
            result = local.prepare(root, self.base / "snapshots")
        self.assertEqual(len(result["sources"]), 3)
        self.assertEqual((Path(result["sources"][str(framework)]) / "source").read_text(), "dirty framework")
        lock_command = [call.args for call in run.call_args_list if call.args[:3] == ("nix", "flake", "lock")][0]
        self.assertIn("daemon-framework", lock_command)
        self.assertNotIn("app-daemon/daemonFramework", lock_command)  # follows stays intact
        self.assertFalse((root / "flake.lock").exists())
        self.assertFalse((app / "flake.lock").exists())

    def test_shared_root_cannot_be_remote_pinned(self):
        root, app, framework = self.matrix()
        inputs = self.inputs(root)
        inputs["daemon-framework"]["url"] = "github:pmfleming/daemon-framework/old"
        with patch.object(local, "read_inputs", self.inputs), self.assertRaisesRegex(ValueError, "current local"):
            local.validate_policy({app: app, framework: framework}, inputs)

    def test_private_or_vendored_framework_rejected(self):
        root, app, framework = self.matrix()
        with patch.object(local, "read_inputs", self.inputs):
            local.validate_policy({app: app, framework: framework}, self.inputs(root))
            (app / "vendor/daemon-framework").mkdir(parents=True)
            with self.assertRaisesRegex(ValueError, "vendored"):
                local.validate_policy({app: app}, self.inputs(root))

    def test_missing_root_follows_rejected(self):
        root, app, framework = self.matrix()
        inputs = self.inputs(root)
        del inputs["app-daemon"]["inputs"]
        with patch.object(local, "read_inputs", self.inputs), self.assertRaisesRegex(ValueError, "single root"):
            local.validate_policy({app: app, framework: framework}, inputs)


if __name__ == "__main__":
    unittest.main()
