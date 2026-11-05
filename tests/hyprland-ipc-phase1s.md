# Phase 1S upgrade preparation

Completed 2026-11-05 on local `hyprland-ipc-gaps` branches. The implementation
stops at the plan's D8 approval gate. No branches were pushed or merged, and no
system rebuild, activation, production service restart or reboot was performed.

Coordinated branch tips before this framework evidence commit:

| Repository | Tip | Included phases |
| --- | --- | --- |
| daemon-framework | 27927b7 | baseline ad2547a, compatibility 27927b7 |
| app-daemon | ad1d0ce | Phase 1 |
| bar-daemon | 2732ed1 | Phase 1 |
| shelllist | c70304c8 | Phase 0, Phase 1, Phase 1S |
| NixOS configuration | 01028ee | Phase 1S |
| update-daemon | 7c4b393 | unchanged sibling dependency |

Full handoff evidence, upstream review, version table, checks, all unrelated
uncommitted local input files and required deployment steps are recorded in
the paired Shelllist commit `c70304c8`, at
`tests/hyprland-upgrade-phase1s.md`. The worktrees share parent directory
`/home/laufan/Projects/hyprland-ipc-gaps/`.

Framework fmt, warning-denied workspace/all-target clippy, and locked workspace
tests were repeated successfully (80 passed, 2 opt-in tests ignored). App,
bar and portal full suites also passed, including the final new special-workspace
tests. Nested compositor acceptance passed on 0.56.2 and 0.57.1.

Quickshell 0.3.2 built against the approved nixpkgs and Qt 6.11.1. Shelllist's
zero-warning qmllint and input-policy checks, actual hidden-popup and full host
IPC smokes, and NixOS sleep-recovery checks passed. Installed Lua configuration
verification reports config ok. Wrapper-provided 0.55.4 hyprctl eval still works
against 0.57.1. The popup's numbered activation context remains correct with an
open, focused special workspace, so no popup production code change was needed.

The full `local-build check /etc/nixos` is the next coordinated gate after user
approval to merge. The user then runs `rebuild`, reboots, verifies desktop and
hardware recovery, and confirms the result before Phase 2. Both update timers
remain held; staging is false, queued true and ready false.
