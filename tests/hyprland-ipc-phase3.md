# Hyprland IPC gap closure: Phase 3

All Rust mutation builders return a `Lua` value without a transport prefix.
`execute` requires successful exact-session capabilities, then sends one `eval`
and checks the shared acknowledgement. Composite move-and-pin admission remains
one compositor turn through `Lua::then`; it was not split into two requests.
The guarded window/workspace checks and daemon-owned journal fences remain.

Capabilities read status before version and require Lua and 0.57.1 or newer.
An exact, non-mutating function-type probe covers every builder API. Failures
are not cached. Success belongs to the immutable client's signature and is
shared only by its clones. Unit tests reject legacy config, an older version
and an incomplete API set before any requested effect, then prove retry and
successful caching. A source-coverage test fails for unprobed builder APIs.

App, bar and portal commands now use these builders. Legacy dispatch retries
are removed, so a rejection or uncertain exchange cannot trigger a second
command. Address-based operations retain their earlier unguarded semantics.
Monitor/layout and focus-setting domain validation and readback remain in bar;
only encoding/execution and validated read-only option batches moved here.
Shelllist's popup/property/layer helpers now emit Lua exclusively. QML's chooser
interaction model is unchanged.

Validation on 2026-11-05:

- The complete function-type probe returns `ok` on the running 0.57.1 Lua
  compositor without executing effects. The private compositor acceptance
  passes through checked execution, including the special-workspace builder.
- Framework formatting, warning-denied workspace/all-target clippy and full
  locked workspace suite pass. The Hyprland crate has 24 passing unit tests.
- The portal's four tests pass with the explicit status/version/probe/effect
  protocol and no legacy fallback. App/bar suites retain the established safety
  tests. Changed assertions account for source-only Lua builders and the typed
  rejection message; placement receipts and validation checks remain asserted.
- Nix `qmlLint` passes with max warnings zero. The actual QtTest dispatch test
  passes (three Qt lifecycle/test passes) and checks selector quoting and each
  emitted Lua dispatcher.
- A private 0.57.1 GPU session runs the changed Shelllist source with Quickshell
  0.3.2: numbered context remains correct while a special workspace is focused,
  hidden popup smoke passes, and full host ping/status/listSurfaces/list/quit
  pass. Ghostty and both Qt rendering contexts use AMD OpenGL 4.6 / Mesa 26.2.6
  on the upgraded kernel 6.18.60. No production service was restarted.

Raw logs are `/tmp/hyprland-phase3-{framework-final,app-final,bar-final,portal,
private,qt-dispatch,qmllint,hidden-popover}.log`. The private GPU evidence root
is `/tmp/ipc-h5yr0mo9`. Hardware docking/sleep/hibernation remain untested, as
explicitly accepted by the user before continuing through the remaining phases.
