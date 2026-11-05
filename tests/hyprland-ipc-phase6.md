# Hyprland IPC gap closure: Phase 6

The contract in `contracts/hyprland-ipc.md` now includes targeting, delivery
classification, acknowledgement, capability gating, bounded decoding, loss,
guards, read models and reference cleanup. It retains the Phase 1 geometry and
batch-delimiter requirements. The crate README documents workspace ownership,
typed execution and the opt-in acceptance command.

`guarded_close` had no consumer beyond its unit test. It is removed; those
injection/PID/workspace assertions now exercise `close_pinned`. Additional unit
tests cover pinning, observation, release, replacement, follow and focus guards,
including guard-before-effect ordering and explicit pinned-state classifications.
Unknown/invalid IPC replies remain errors, never disappearance evidence.

The private compositor test now:

- Requires the target 0.57.1 capabilities and compiles all twenty representative
  builder/branch outputs using `assert(load(source))`, without running them.
- Constructs all four dispatcher kinds without dispatch and compares the
  workspace/client state before and after these no-effect checks.
- Covers pinned Present/Moved/Gone/Lost states, selective prefix release,
  normal close, atomic weak-reference replacement and expired follow intent.
- Reads a real special-workspace snapshot/status and consumes real typed events
  through `Client::watch_events`, including a valid special WindowMoved event.

`tests/run-hyprland-acceptance.sh` and the flake's `hyprlandAcceptance` package/app
supply the opt-in release gate. Nix supplies the exact Rust toolchain and Weston
test applications. The script resolves an existing Wayland parent, isolates all
command effects in child compositors and puts Cargo outputs in a writable cache
when using immutable Nix source. Renderer acceptance stays outside default CI.

Validation on 2026-11-05: framework formatting, warning-denied workspace/all-target
clippy and full locked workspace tests passed (30 Hyprland unit tests). The direct
script and the built Nix app both passed the expanded private test. ShellCheck
passed the script. Private evidence roots were `/tmp/ws-VH9G3G` and
`/tmp/ws-2ATGN6`; logs are `/tmp/hyprland-phase6-{crate,framework,acceptance,
acceptance-package,nix-acceptance}.log`. The earlier Phase 3 Qt/lint/GPU hidden-popup
checks exercise the unchanged final QML; final coordinated deployment checks
remain the release-wide gate after merging.

The combined app/bar diff review retained domain validation, process provenance,
durable admission before effects, weak identity, monotonic retirement, focus and
foreign-monitor guards, and observation-based display/placement confirmation.
Typed API changes remove duplicate encoding and tighten loss/error handling.
The exact Nix-built 0.57.1 Window.cpp confirms both move-event forms carry the
workspace address; v2 additionally carries the display name. A paired regression
checks workspace 7 displaying as "6", preserving address identity in both forms.
No live service restart, deployment, production compositor restart, docking,
suspend or hibernation test was performed. The user explicitly deferred physical
recovery checks; activation and the real compositor restart remain user-controlled.
