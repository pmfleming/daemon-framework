# Hyprland IPC gap closure: Phase 2

Implemented on local `hyprland-ipc-gaps` worktrees after the user confirmed the
repaired 0.57.1 rebuild/reboot. The user then explicitly authorized continuing
through all remaining phases despite untested docking/suspend/hibernate.
Those physical checks remain unverified. No production service was restarted.

The additive framework commit `d03e3d9` precedes the coordinated event API change:
`WindowAddress`, shared `ack`, downcastable `IpcError`, a single connection/exchange
deadline and custom-client watching. Configured sessions are exact across both
socket roots; absent signatures retain discovery. A failed connection may probe
another instance without writing a command, solely to diagnose `SessionGone`.
No request is replayed after connecting. Before-connect errors are NotDelivered;
exchange I/O/timeout errors are OutcomeUnknown. Oversized replies are InvalidReply.

The final event API identifies Connected instances, parses known names exactly,
preserves moved-window special identities, and represents unknown/malformed
events explicitly. Compositor sends never await a slow consumer: one pending
Lagged is delivered when room opens, including while idle. Partial frame reads
remain alive during loss delivery. Connection transitions are never dropped.
Both daemons terminate through their existing shutdown code on SessionGone,
returning an error for systemd restart rather than rebinding session journals.

Consumer review:

- The bar's previous `starts_with("ok")` checked a single dispatch reply, not
  a batch; accepting `okay` or `ok` followed by an error was unnecessary. It now
  uses the shared exact acknowledgement, as do focus settings and app commands.
  The pin-plus-JSON batch remains an explicitly documented separate protocol.
- Handoff intake consumes typed events. Disconnect, failed connection, loss,
  malformed payload and workspace renumbering fence replacement observation.
  A valid special-workspace move does not become malformed or workspace zero.
- Geometry/preference reads use the same pinned Client and cannot discover a
  replacement compositor while retaining the previous session's journal.
- The portal launcher validates the shared bounded address type and requires
  its inherited instance signature. Its caller inherits the graphical session
  environment; the running Shelllist service has the current signature. Legacy
  command removal and capability checking belong to Phase 3.

Validation:

- Framework formatting, workspace/all-target warning-denied clippy and locked
  workspace suite passed: 86 tests (before the additional one-socket liveness regression), two opt-in tests ignored. The Hyprland crate
  contributes 22 passing tests, including actual captured 0.57.1 events, comma
  payloads, special moves, diagnostic-only targeting, discovered identity,
  full-channel intake and partial-frame preservation across Lagged delivery.
- App suite: 64 unit tests and 19 existing integration tests passed. A new
  separate process test renames the private compositor instance, disconnects
  the stream, observes a nonzero daemon exit and reopens its history database.
- Bar suite: 327 unit tests and 33 existing integration tests passed. The new
  private-bus process test also proves nonzero session-loss exit after stores
  open; unit tests verify retained fatal signaling and handoff fencing.
- Portal launcher: four tests passed. No QML changed in this phase.

The initial bar process-test fixture failed because its runtime directory was
mode 0755; the native socket correctly rejected it. Mode 0700 fixes the fixture.
The final test passes. Private buses disable service activation; no host bus or
live compositor is mutated. The real compositor restart remains a user-run
final rollout check, separate from these isolated process tests.
