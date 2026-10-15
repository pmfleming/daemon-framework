#!/usr/bin/env python3
"""Safety/retention tests; only temporary fixture directories are modified."""
import contextlib
import fcntl
import importlib.util
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SCRIPT = Path(__file__).with_name("trim-build-cache.py")
spec = importlib.util.spec_from_file_location("cache", SCRIPT)
cache = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cache)


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve() / "repo"
        self.root.mkdir()
        self.target = self.root / "target"
        self.target.mkdir()
        self.tracked = set()

    def file(self, path, size=16384):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x" * size)
        return path

    def profile(self, base=None, name="debug"):
        profile = (base or self.target) / name
        self.file(profile / ".cargo-lock", 0)
        self.file(profile / ".fingerprint" / "hash")
        self.file(profile / "deps" / "binary")
        self.file(profile / "incremental" / "object")
        return profile

    def trim(self, limit=1, dry_run=False, target=None):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cache.trim_target(self.root, target or self.target, limit, dry_run, self.tracked)
        return output.getvalue()

    def test_noop_and_dry_run_do_not_change_files(self):
        self.profile()
        before = {p: p.stat().st_mtime_ns for p in self.target.rglob("*")}
        self.assertEqual(self.trim(limit=cache.GIB), "")
        self.assertEqual(before, {p: p.stat().st_mtime_ns for p in self.target.rglob("*")})
        self.assertIn("would remove", self.trim(dry_run=True))
        self.assertEqual(before, {p: p.stat().st_mtime_ns for p in self.target.rglob("*")})

    def test_incremental_first_and_stop_at_limit(self):
        profile = self.profile()
        amount = cache.disk_bytes(profile / "incremental")
        self.trim(limit=cache.disk_bytes(self.target) - amount + 4096)
        self.assertFalse((profile / "incremental").exists())
        self.assertTrue((profile / "deps/binary").exists())
        self.assertTrue((profile / ".cargo-lock").exists())

    def test_coverage_before_normal_artifacts(self):
        normal = self.profile()
        coverage = self.profile(self.target / "llvm-cov-target")
        incremental = sum(cache.disk_bytes(p / "incremental") for p in (normal, coverage))
        self.trim(limit=cache.disk_bytes(self.target) - incremental - 4096)
        self.assertFalse((coverage / "deps").exists())
        self.assertTrue((coverage / ".cargo-lock").exists())
        self.assertTrue((normal / "deps/binary").exists())

    def test_profiles_cleared_without_replacing_lock_files(self):
        for base, name in [(self.target, "release"),
                           (self.target / "x86_64-unknown-linux-gnu", "debug")]:
            with self.subTest(base=base, name=name):
                profile = self.profile(base, name)
                inode = (profile / ".cargo-lock").stat().st_ino
                self.trim()
                self.assertEqual(list(profile.iterdir()), [profile / ".cargo-lock"])
                self.assertEqual((profile / ".cargo-lock").stat().st_ino, inode)

    def test_dry_run_accounts_for_retained_hardlinks_and_literal_paths(self):
        profile = self.profile()
        binary = self.file(profile / "deps/binary", 8 * 1024 * 1024)
        os.link(binary, self.target / "report-binary")
        artifact = self.file(self.target / "cache[one]*?.bin", 1024 * 1024)
        retained = self.file(self.target / "cacheoXX.bin")
        projected = cache.disk_bytes(self.target, [artifact])
        artifact.unlink()
        self.assertEqual(projected, cache.disk_bytes(self.target))
        output = self.trim(limit=1024 * 1024, dry_run=True)
        self.assertIn("manual review", output)
        self.assertNotIn("-0.", output)
        self.assertGreaterEqual(cache.disk_bytes(self.target, [profile]), 8 * 1024 * 1024)
        self.assertTrue(binary.exists())
        self.assertTrue(retained.exists())

    def test_active_cargo_lock_skips_whole_target(self):
        self.profile()
        release = self.profile(name="release")
        with (release / ".cargo-lock").open("rb") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.assertIn("active build", self.trim())
        self.assertTrue((self.target / "debug/incremental/object").exists())

    def test_reports_and_review_checkouts_preserved(self):
        self.profile()
        report = self.file(self.target / "analysis/report.json")
        checkout = self.file(self.target / "review-current/frozen/repo/.git", 0)
        source = self.file(checkout.parent / "src/modified.rs")
        review_profile = self.profile(self.target / "review-custom-build")
        self.assertIn("manual review", self.trim())
        for path in (report, checkout, source, review_profile / "deps/binary"):
            self.assertTrue(path.exists(), str(path))

    def test_source_markers_preserve_entire_profile(self):
        profile = self.profile()
        for marker in [".git", "Cargo.toml"]:
            with self.subTest(marker=marker):
                path = self.file(profile / "deps/source" / marker)
                self.assertIn("preserving source", self.trim())
                self.assertTrue((profile / "incremental/object").exists())
                self.assertTrue((profile / "deps/binary").exists())
                path.unlink()

    def test_tracked_artifact_preserves_entire_profile(self):
        profile = self.profile()
        self.tracked.add(profile / "deps/binary")
        self.trim()
        self.assertTrue((profile / "incremental/object").exists())

    def test_profile_and_artifact_symlinks_are_not_followed(self):
        outside = self.profile(self.root / "elsewhere")
        (self.target / "release").symlink_to(outside, target_is_directory=True)
        profile = self.profile()
        source = self.file(self.root / "source.txt")
        (profile / "source-link").symlink_to(source)
        self.trim()
        self.assertTrue((outside / "deps/binary").exists())
        self.assertTrue(source.exists())
        self.assertTrue((profile / "source-link").is_symlink())

    def test_external_symlinked_and_nested_repository_targets_are_refused(self):
        profile = self.profile()
        link = self.root / "linked-target"
        link.symlink_to(self.target, target_is_directory=True)
        external = self.root.parent / "external"
        external_profile = self.profile(external)
        nested = self.root / "nested"
        self.file(nested / ".git", 0)
        nested_profile = self.profile(nested / "target")
        for target, kept in [(link, profile), (external, external_profile),
                             (nested / "target", nested_profile)]:
            with self.subTest(target=target):
                self.assertIn("refusing", self.trim(target=target))
                self.assertTrue((kept / "deps/binary").exists())

    def test_unrecognized_debug_directory_preserved(self):
        source = self.file(self.target / "debug/source.txt")
        self.trim()
        self.assertTrue(source.exists())

    def test_target_selection_uses_manifests_or_repository_relative_override(self):
        tracked = {self.root / "Cargo.toml", self.root / "rust/search/Cargo.toml"}
        for environment, expected in [
            ({}, {self.target, self.root / "rust/search/target"}),
            ({"CARGO_TARGET_DIR": "build-cache"}, {self.root / "build-cache"}),
        ]:
            with self.subTest(environment=environment):
                with patch.dict(os.environ, environment, clear=True):
                    self.assertEqual(set(cache.target_paths(self.root, tracked)), expected)

    def test_cli_validation_and_disabled_mode(self):
        with patch.dict(os.environ, {}, clear=True):
            for value in ("-1", "abc", "1.5", ""):
                result = subprocess.run([sys.executable, str(SCRIPT), "--max-gib", value],
                                        capture_output=True)
                self.assertEqual(result.returncode, 2)
            result = subprocess.run([sys.executable, str(SCRIPT), "--max-gib", "0"],
                                    capture_output=True)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout, b"")

    def test_git_inventory_disables_checkout_hooks_and_fsmonitor(self):
        with patch.object(cache.subprocess, "check_output", side_effect=[
            str(self.root) + "\n", b"Cargo.toml\0"
        ]) as command:
            self.assertEqual(cache.git_files(self.root), {self.root / "Cargo.toml"})
        for call in command.call_args_list:
            self.assertIn("core.fsmonitor=false", call.args[0])
            self.assertIn("core.hooksPath=/dev/null", call.args[0])

    def test_git_index_and_cli_from_other_working_directory(self):
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        tracked = self.file(self.root / "Cargo.toml")
        subprocess.run(["git", "-C", str(self.root), "add", "Cargo.toml"], check=True)
        self.assertEqual(cache.git_files(self.root), {tracked})
        destination = self.root / "scripts/trim-build-cache.py"
        destination.parent.mkdir()
        shutil.copyfile(SCRIPT, destination)
        profile = self.profile()
        result = subprocess.run(
            [sys.executable, str(destination), "--dry-run"], cwd="/",
            env={**os.environ, "CARGO_TARGET_MAX_GIB": "4", "CARGO_TARGET_DIR": ""},
            capture_output=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((profile / "deps/binary").exists())


if __name__ == "__main__":
    unittest.main()
