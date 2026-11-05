# Phase 1S upgrade preparation

## Deployment follow-up — 2026-11-05

The preparation record below is historical. The user approved merging the local
branches, ran `rebuild`, and rebooted. The first deployment failed because the
new Mesa binary required a newer glibc than the base desktop applications.
NixOS repair `ffa4d53` builds both Mesa architectures from the display pin's
recipe against base OS libraries and adds actual loader regression checks.
Its `config/scripts/tests/graphics-driver-load.md` records the failure and repair
evidence. Full coordinated checks and a complete non-activating system build
passed before the second deployment.

The user reports **rebuild and reboot successful**. Read-only verification of
the running session confirms:

- `/run/current-system` is the repaired `8sjx3024llw86dark52hbjsbk28b17wj`
  generation, matching the successful rebuild/deployment record.
- Hyprland **0.57.1**, `configProvider=lua`, DRM backend, Linux **6.18.60**,
  Aquamarine **0.16.0**, and the repaired Mesa **26.2.6** driver are active.
- Shelllist, app-daemon and bar-daemon user services are active. No occurrences
  of the earlier missing-GLIBC, Mesa-loader or graphics-context failures were
  found in this boot's journal at verification time.
- `update-worker status` reports a valid baseline and
  **staged=false, queued=false, ready=false**. Both delayed update timers are
  active again after reboot, satisfying the plan's temporary-hold release.
- `crates/shelllist-hyprland/tests/fixtures/0.57.1/deployed.json` captures the
  real session's monitors, workspaces, clients, active window, workspace rules
  and gaps. Titles, application classes, monitor metadata, process identities
  and window addresses are anonymized. The snapshot/work-area fixture test now
  parses this capture as well as the private-compositor captures. No special
  workspace was open, and no live compositor mutation was used for capture.

The user explicitly confirms boot/desktop testing and says hardware recovery
is still untested. Docking/undocking, suspend and hibernation on this
generation therefore still require results under the plan's Phase 1S
"Confirming it worked" checklist. Special-workspace UI interaction and portal
launcher placement also retain their earlier private-session evidence rather
than a new user-confirmed real-session journey. Phase 2 implementation has not
started; the required physical recovery validation remains pending. The
Hyprland crate suite passed with the deployed fixture (15 tests, one separate
opt-in renderer test ignored), and framework `cargo fmt --check` passed.

## Original preparation record

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
