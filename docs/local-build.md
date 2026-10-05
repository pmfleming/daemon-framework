# Native local-build

`crates/shelllist-local-build` replaces `tools/local-build.py` and its Python
unit tests. It is workspace development tooling, not a dependency of any daemon.
There is no Python compatibility wrapper or Python test runner.

## Running it

From this checkout, with Cargo/Rust, Git and Nix installed:

```sh
tools/local-build check ../shelllist --keep-going
tools/local-build build --attr localBuild . --no-link --print-out-paths
tools/local-build develop ../app-daemon --command cargo test --locked
tools/local-build run ../bt-daemon -- probe-bluez
```

`tools/local-build` is a small POSIX launcher for `cargo run --locked`; it uses
this checkout's manifest and preserves the caller's working directory and
arguments. The first invocation needs the crate dependencies in Cargo's cache
or network access. Subsequent invocations reuse Cargo's target directory. It
does not select an older installed binary or bootstrap via a separately pinned
framework flake.

The Nix `packages.<system>.localBuild` package installs a native `local-build`
binary with Git and Nix in its runtime PATH. The packaged command needs neither
Python nor Cargo. `apps.<system>.localBuild` exposes the same executable. Rebuild
the installed package to replace the previous Python-backed desktop command.

## Compatible CLI and guarantees

- `preflight ROOT` reports every discoverable worktree problem together before
  authentication, or prints JSON with `repositories`. It respects follows and
  nested overlays, without snapshotting, locking or changing Git tracking.
  Preparation repeats worktree validation rather than trusting this early check.
- `prepare ROOT DESTINATION [--root-is-snapshot] [--capture-root]` leaves a persistent prepared
  graph, prints its JSON description, and writes `DESTINATION/sources.json`.
  The keys remain `flake`, `sources`, and `storeSources`. The destination must
  not already exist. A frozen root is copied without dereferencing symlinks;
  its destination must be outside that root to prevent recursive copying.
  `--capture-root` additionally retains `DESTINATION/.approval-root` before lock
  resolution and returns its path as `originalRoot`. Rebuild approval uses this
  exact captured source and original lock, never a second live checkout copy.
- `prune-lock ROOT LOCK` rewrites only the explicitly supplied lock. It removes
  local input roots and unreachable nodes while retaining remote pins and other
  lock metadata. Follows cycles and missing references fail instead of hanging.
- `check`, `build`, `develop`, and `run` capture a disposable graph and keep it
  alive until the child exits. Normal success and command failure both clean up
  that graph and its temporary GC-root links.
- `--attr NAME` belongs **before** ROOT for build/develop/run. Everything after
  ROOT is forwarded, including child `--help`; an optional leading `--` is
  stripped. `run` inserts the separator needed by `nix run`.
- Source-override flags remain forbidden, and execution uses
  `--no-update-lock-file` after preparing the graph.
- Tracked dirty files, deletions, file permissions and internal symlink targets
  are preserved. Untracked, non-ignored files must be Git-added first. Ignored
  targets and `.git` are excluded. Changes during file copying fail the run.
- Each repository is captured once and each dependency is added to the store
  once, even when referenced by several edges. Local refs/revisions, dependency
  cycles, basename collisions, private/vendored frameworks and broken root
  follows relationships fail closed. Live repository locks are never resolved
  or updated by prepare/check/build/develop/run.

The tool still evaluates trusted flake expressions and runs Git/Nix subprocesses;
porting it to Rust does not make untrusted build inputs safe.

## Tests and migration

```sh
cargo test -p shelllist-local-build --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

All 13 original policy scenarios are Rust tests using temporary real Git
repositories. Additional unit and binary integration tests exercise aliases,
follows cycles, graph cycles/collisions, lock failure, read-only/executable files,
dangling links, argument forwarding, JSON output and temporary cleanup. Nix is
substituted at a narrow interface in unit tests; binary tests use a per-child
mock executable. Coverage also includes aggregated preflight failures, distinct
nested overlays and preserving the approval lock before disposable resolution.
Tests do not modify the process environment globally or require
a Nix daemon/network (after Cargo dependencies are available).

Existing invocations must change from `python3 .../tools/local-build.py` to
`.../tools/local-build` (or the rebuilt installed `local-build`). Framework Nix
checks/CI and active sibling CI/launcher entrypoints are migrated together.
CI using the checkout launcher installs Rust before invoking it. Historical
review documents retain the commands used to produce their original evidence.

## Migration validation

- Formatting and strict workspace/all-target Clippy passed on Rust 1.95.0.
- `cargo test --workspace --locked`: 78 passed, one existing live Hyprland probe
  ignored; 21 tests belong to local-build (18 unit, three binary integration).
- `tools/local-build check . --print-build-logs`: all x86_64-linux Nix checks passed,
  including the native package and workspace tests. aarch64 was not built.
- Native package built through `tools/local-build build --attr localBuild .`;
  its ELF executable runs with no Python/Cargo on the caller's PATH.
- Live `prepare ../shelllist ...` captured seven current repositories, added six
  dependency snapshots to the store, and resolved only a disposable root lock.
  This validates graph preparation, not a fresh build of the entire daemon/UI
  family. No desktop services or installed profiles were changed.
