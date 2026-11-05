# Hyprland IPC gap closure: Phase 5

The journal audit found concrete orphan routes. `workspace.close` pins a compositor
snapshot before inserting/persisting its CloseRecord. A crash or failed reply in
that gap leaves no durable per-token recovery list. Close dispatch releases each
recorded token only on a best-effort basis; restart marks pending closes unknown
instead of replaying them. Handoff recovery releases recorded tokens but cannot
discover an unrecorded close pin. Per-token cleanup alone is therefore insufficient.

All new bar close/handoff tokens use `shelllist-bar-workspace-`. On first journal
load, after validating the journal and before admitting new pins, the sole bar
owner releases this prefix in its exact compositor session. Restarted journal
observers remain fenced. `release_prefix` validates a nonempty token prefix and
only removes matching table references: it cannot move/close a window, dispatch
an action or erase another owner's namespace. Cleanup is idempotent on uncertain
replies. It is not a broad table reset. Older unnamed token prefixes cannot be
recovered by this new namespace; the final user-run compositor restart clears
those legacy references. No live cleanup was performed during implementation.

Reconnect delays are 1, 2, 4, 8, 8... seconds. An immediately closing connection
does not reset failures; a stream that receives a frame or remains established
for at least a second resets to one second. Controls remain reliable and receiver
closure still cancels backoff. `Client::with_ipc_timeout` overrides the default
two-second deadline shared by connection and exchange.

Validation: full locked framework, app, bar and portal suites passed, with
framework formatting and workspace/all-target warning-denied clippy. The crate
has 27 passing tests, including capped reconnect timing/reset and a custom
exchange deadline (allowing timer-wheel millisecond rounding). A new bar test
proves cleanup occurs once per loaded owner and again after restart. Existing
safety assertions still forbid every window/workspace effect; they explicitly
permit only the exact reference-housekeeping command. Default-client mutation
tests run without an inherited live signature; process fixtures supply their
own private signatures. Logs: `/tmp/hyprland-phase5-{framework-final,app,
bar-final,portal}.log`.
