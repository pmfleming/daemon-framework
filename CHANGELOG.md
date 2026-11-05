# Changelog

## Unreleased

- Accept Hyprland 0.57 special workspaces without numeric IDs, preserving their
  names/addresses and resolving geometry by identity without treating absence as
  workspace zero. Decode bounded IPC text lossily while retaining JSON errors;
  reject monitor identifiers containing batch delimiters before pinning windows.
  Captured 0.56.2/0.57.1 fixtures and nested acceptance evidence cover the upgrade.

- Include current and initial application classes in workspace window snapshots
  for presentation; missing class fields remain compatible with older snapshots.

- Add guarded local workspace selection with optional focused-window movement;
  recheck destination existence and focus context atomically before dispatch.

- Add guarded observation and replacement of pinned Hyprland windows, distinguishing address reuse and preserving focus. Destination ownership, process provenance, and handoff deadlines remain daemon policy.

- Add `shelllist-hyprland::monitor` for typed, syntax-validated Lua monitor commands. Explicit enable/disable and mirror-reset semantics are shared by bar-daemon's manual layout and automatic docking paths; policy, persistence, safety checks and observed confirmation remain daemon-owned.

- Replace the Python local-build utility and its tests with the `shelllist-local-build` Rust workspace crate. The native CLI preserves source-policy and JSON/command contracts, with 21 Rust regression tests (including all 13 former Python scenarios). Use `tools/local-build` or the rebuilt installed `local-build`; the checkout launcher requires Cargo, while the packaged binary only needs Git/Nix. Nix checks, CI, and active sibling entrypoints now use the native tool.

- Share operation/subscription response correlation through `CorrelationPolicy::operation_id`, and add `OwnedOperations::drain` for clone-free bulk claims.
- Remove unused legacy `OwnedTaskRegistry::cancel_all`/`cancel_owner` and `wait_for_owner_loss`/`wait_for_owner_name_loss` APIs after auditing all local consumers. Use managed owner lifetimes, `cancel_owned`, `shutdown`, or an explicit connection-scoped `OwnerLossMonitor::wait`. This is a source API removal; deployed D-Bus/JSONL contracts are unchanged.

- Consolidated `shelllist-hyprland` into a separate workspace crate, preserving its API and tests. App/bar daemons share it through the framework; no standalone checkout or vendored copy is needed.

- Reused established fuzzy matching and edit-distance implementations in the search service.
- Reduced JSONL and output actor branching while preserving ordered output behavior.
- Consolidated related core identity and envelope modules to improve locality.
- Removed output cloning, repeated D-Bus watch logic, wildcard imports, and test panic paths.
- Added a reproducible Rust Quality Lens configuration.

## 0.1.0

- Added the `shelllist-daemon-core` runtime-independent crate.
- Added the `shelllist-daemon-tokio` D-Bus, JSONL, ownership, shutdown, and subscription crate.
- Moved `shelllist-search` into the workspace without changing its binary or JSONL contract.
- Added versioned API/event envelope builders, protocol fixture helpers, monotonic IDs, and secure atomic JSON state helpers.
- Added a shared ordered JSONL client runner with configurable correlation, cancellation, and call-failure policies.
