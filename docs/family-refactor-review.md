# Daemon family refactor (2026-10-04)

## Scope and stages

Review the current sibling worktrees: daemon-framework, app-daemon, bar-daemon,
bt-daemon, clip-daemon, nm-daemon, and the standalone privileged update-daemon.
All began clean. No deployment, hardware mutation or dependency update is intended.

1. Baseline/review (this checkpoint).
2. Framework: simplify Hyprland rule interpretation; centralize the common
   operation/subscription response-correlation mechanism without moving daemon
   stream names, terminal states or acceptance policy into the framework.
3. Consumers: adopt that mechanism; reduce unnecessary snapshot cloning and
   repeated local workflows. Preserve wire formats and ownership semantics.
4. Remeasure, validate and record limitations and deferred work.

Each implementation stage is committed in its affected repositories. Framework
API additions precede consumer adoption; sibling paths remain the source policy.

## Evidence

Instrument: `../rust-quality-lens/target/debug/rqlens`, version 0.1.0; Rust 1.95.
Focused producers: hotspots, clones, escape-hatches, locality, leverage.
Baseline/final artifacts: each repository's `target/family-review-before/` and
`target/family-review-after/` (ignored build artifacts). Configurations preserve
repository settings, with absolute project roots and separate output directories.
Measurements include tests; production claims must distinguish those explicitly.
Some syntax/identity evidence is partial (macros, block-local types, unresolved
helper graph); no calibrated engineering-effort or runtime-performance claim is
possible. Architecture scores are heuristic, not correctness gates.

Initial hotspot review: framework `term` cyclomatic/cognitive 17/15,
`window_count` 15/15; bar display-policy `monitor` 18/18 and `reconcile` 18/13.
The handwritten response-correlation fallbacks repeat across four clients.
Bluetooth, Clipboard and NetworkManager already have zero reported syntax
escape-hatch records. Structurally similar but unrelated domain matches are not
candidates for artificial unification.

Framework baseline `cargo test --workspace --locked` passes. Validation and final
measurements will be recorded after implementation. Existing hardware-dependent
and ignored tests will not be represented as exercised hardware validation.
