# Changelog

## Unreleased

- Parse valid Hyprland prerelease/build suffixes without dropping the numeric
  minimum, malformed-version checks or runtime API validation.

- Keep the special-workspace toggle helper and its API check in private
  acceptance; unused test APIs no longer gate production mutations.

- Preserve numberless named workspace identities in events, workspace/monitor
  reads and literal destination selectors. Capture real private 0.57.1 named
  JSON/events and verify numbered pin guards still reject named destinations.

- Document the complete Hyprland IPC contract and add opt-in Nix-wired private
  acceptance. Compile every Lua builder without effects, construct dispatchers
  without dispatch, and exercise live pinned replacement/state/prefix guards
  and special-workspace event decoding. Remove unused `guarded_close`.

- Make the shared connect/exchange deadline configurable and back off reconnects
  from one to eight seconds, resetting after a healthy stream. Add prefix-scoped
  reference release for bar journal recovery after pin-before-admission crashes.

- Share window/workspace display fields and a single-turn status batch across
  app/bar and portal consumers. Preserve optional special-workspace identity,
  required snapshot identity, default unfocused rank, per-window address
  filtering and explicit absence of an active window.

- Use typed Lua builders and one checked execution path for all Rust compositor
  mutations. Require Lua configuration, Hyprland 0.57.1 and the builder API set;
  cache only successful checks for the exact client session. Remove legacy
  command retries from consumers and Lua/legacy branches from Shelllist.

- Replace raw Hyprland event strings with typed, exact-name notifications and
  explicit loss delivery. Custom clients keep their session identity; diagnostic
  discovery never rebinds reads or commands. App/bar daemons drain normal
  shutdown and return an error when their configured compositor is replaced.

- Add downcastable IPC delivery errors, exact single-command acknowledgements,
  validated window addresses and client-owned event watching. Connection and
  exchange share one deadline; configured signatures never redirect requests.

- Record the successfully deployed Hyprland 0.57.1 upgrade after the Mesa/libc
  repair, and validate snapshot/work-area decoding against a redacted capture
  of the actual upgraded desktop. Physical recovery validation remains pending.

- Validate the coordinated Phase 1S upgrade preparation against Hyprland 0.57.1
  and Quickshell 0.3.2 on the approved Qt. Record local commits, checks, display
  stack versions and deployment blockers; activation waits for user approval.

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
