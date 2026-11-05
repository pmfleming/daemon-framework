# Hyprland IPC gap closure: Phase 1

Validated 2026-11-05 on the coordinated `hyprland-ipc-gaps` sibling branches.

## Result

Workspace numbers are optional in compositor snapshots and both daemon adapters.
The application list retains windows on special workspaces and presents their
workspace name when a number is unavailable. The bar's numbered workspace list
excludes special workspaces; its active-window presentation remains available.
Ordinary workspace actions, handoff selection, history and close targets ignore
numberless workspaces or reject a missing required ordinary context explicitly.
The portal launcher rejects an active workspace without a number before launch.

Geometry resolves special workspaces and client occupancy by name when no
number exists, while empty specialWorkspace entries fall back to the ordinary
workspace. No matching is based on None == None. `WorkspaceRef` also preserves
addresses, names and special identity from both captured versions.

Transport replaces invalid UTF-8 in replies/events without relaxing raw byte
limits; JSON errors remain errors. Oversized command replies still cannot cause
replay on another instance. `pinned_snapshot` checks connector identifiers before
connecting, preventing semicolons from entering its batch expression.

## Checks

- Framework: `cargo fmt --check`; `cargo clippy --locked --workspace --all-targets
  -- -D warnings`; `cargo test --locked --workspace`: passed. Hyprland crate:
  15 tests passed; the full workspace has 80 passed and 2 opt-in tests ignored.
- App: fmt and locked full tests passed, including the new special-workspace
  decode test (63 unit tests, 19 integration tests; existing opt-ins ignored).
- Bar: fmt and locked full tests passed (323 pre-existing unit tests and 33
  integration tests); both added special-workspace tests passed separately after
  they were added. One exercises real service close admission and proves a
  numberless special window is excluded from ordinary close targets.
- Bar's opt-in `workspace_handoff` integration target compiles with
  `--features native-workspace-handoff` after the optional-ID API migration.
- Portal launcher: fmt and all 4 locked tests passed.
- Updated private compositor acceptance test passed on 0.56.2 and 0.57.1, including
  an open special workspace and work-area reads. Test setup follows Phase 0's
  existing-parent command. No compositor package patches were needed.
- Captured regular/special fixtures from both versions pass snapshot and work-area
  parsers. Geometry tests also cover special selectors and occupancy with two
  distinct numberless workspaces and empty monitor special-workspace entries.
- Socket tests cover invalid title bytes with successful JSON decoding, invalid
  JSON rejection, invalid event bytes followed by another event on the same
  connection, both raw size limits, timeout and no command replay.
- `git diff --check`: clean across all four changed repositories.

No QML changed in this phase. No production services were restarted; no rebuild,
activation or reboot was performed. A real non-UTF-8 application title remains
unverified (the bounded Unix-socket test reproduces the transport input directly).
Phase 2's exact event/session redesign and Lua-only command migration are deferred
until the user confirms the Phase 1S rebuild, as required by the plan.
