# Hyprland IPC contract

Target: Hyprland 0.57.1 with Lua configuration. The minimum mutation version is
0.57.1; a later version must still expose all builder APIs and pass coordinated
acceptance. Automatic display updates remain enabled by the host policy.
Deploy the framework and all path-dependent consumers as one coordinated graph;
rollback restores the compositor and consumers together.

## Session ownership and delivery

- A configured signature is exact for requests and event sockets across runtime
  and legacy roots. Invalid signatures fail explicitly. Discovery is permitted
  only when no signature is configured; mutations require a valid signature.
- Candidate sockets and their instance directories must belong to the effective
  user, directories must exclude group/world writes, and neither may be a symlink.
  Connected peer UID must match. This does not prevent same-user impersonation.
- Discovery, connection, writing and reading share one deadline, default two
  seconds and configurable with `Client::with_ipc_timeout`. Connection failure
  is `NotDelivered`; an exchange I/O failure or timeout is `OutcomeUnknown`.
  `InvalidReply` also never proves that a submitted effect did not happen.
- A connected command is never replayed. `ack` accepts only trimmed `ok`;
  other nonempty replies are `Rejected`, and empty replies are `InvalidReply`.
  Pin-and-snapshot batches explicitly parse their `ok` prefix and following JSON.
- If the configured instance cannot be reached, diagnostic discovery sends no
  command and never rebinds the client. Another live instance means `SessionGone`
  only if neither configured socket is live. No live alternative is an ordinary
  retryable connection failure. App/bar return an error through normal cleanup,
  preserving final writes and allowing systemd restart with current environment.

## Bounded text and event loss

- Replies are capped at 16 MiB; event frames at 64 KiB including the terminator.
  Limits apply to raw bytes, including unterminated frames. Invalid UTF-8 becomes
  U+FFFD; invalid JSON fails typed reads. No encoding detection is attempted.
  Invalid text alone never tears down the event stream.
- `Connected` identifies its actual instance. Connection controls are never
  dropped. Exact known v1/v2 event names share typed variants; both emitted forms
  remain observable, without deduplication. Unknown names remain `Unknown`.
- Moved-window addresses use `WindowAddress`: `0x` plus 1–16 hex digits, nonzero.
  Snapshot addresses remain strings so a consumer can filter one bad address.
  Named regular and special workspaces have an address/name and no numeric ID.
  Neither is workspace zero or a malformed ordinary move. Both move-event forms
  preserve the address; a display name never replaces it. Comma-bearing display data uses bounded
  splitting. Known malformed payloads remain `Malformed`.
- Compositor intake uses `try_send`. Saturation drops notifications and records
  one pending `Lagged`, delivered when room exists even if the socket is idle.
  Partial reads survive that delivery. `Lagged`/`Malformed` require resync and
  fence handoffs; workspace renumbering also fences existing handoff authority.
- Reconnect delays are 1, 2, 4, 8, 8... seconds. A received frame or a connection
  surviving one second resets the delay. Receiver closure cancels attempts,
  reads, control sends and backoff; quiet healthy streams have no idle timeout.

## Lua and guarded effects

- Rust builders return `Lua` without `eval`. `execute` is the shared mutation
  path. `Lua::then` composes validated sources within one compositor turn.
- Before effects, capabilities require Lua, the minimum version and all APIs in
  `capabilities::LUA_APIS`. The probe checks function types without dispatching.
  Only successful checks are cached, bound to that immutable exact-session client.
  Failed/transient checks are retried on the next call. Typed reads remain usable
  independently of mutation capabilities.
- Shared unguarded address focus/close/move preserve their callers' established
  semantics. Workspace guards separately validate positive workspace IDs, PID,
  mapped window identity, focus intent and destination monitor ownership at the
  effect turn. Consumer acknowledgement, journal admission, process provenance,
  deadlines and domain safety checks remain consumer responsibilities.
- The monitor in a pin-and-snapshot batch must pass the shared connector-name
  validator before connecting or creating references. Lua string quoting alone
  does not protect a batch delimiter. Special workspaces cannot satisfy an
  ordinary-workspace guard.
- Pin tokens refer to the exact weak window object, not a reusable address.
  Replacement requires the predecessor to be gone while identity remains known;
  stale/moved/lost observations never authorize guessed replacements. Prefix
  release removes only matching references, never windows or routing state.
- Bar owns `shelllist-bar-workspace-` tokens. Journal loading fences old
  observers without a capability probe. Immediately before the first handoff pin
  or close snapshot, the owner clears only this namespace, covering
  crashes between pinning and durable admission. Legacy unnamed orphans require
  a compositor restart; no broad reference-table reset is used. Failed cleanup
  admits no new pins and is retryable. Manual moves to named/special workspaces
  cancel affected handoffs even if a window returns/closes before reconciliation.

## Read models and evidence

Window address/PID/workspace and monitor identity remain required. Display fields
default; absent focus rank stays `i64::MAX`. Workspace numbers are optional for
named regular and special workspaces. Workspace/monitor/window reads retain
the same address-bearing identity. `WorkspaceRef::selector` produces numbered,
`name:<address>` or `special:<name>` selectors and rejects missing identity. Status batches workspaces, monitors and active window in one
turn; an empty active-window object is `None`. The bar publishes regular
workspaces with optional numeric IDs and addresses; active identities have null
numbers when absent. Its numbered strip and lifecycle journal exclude numberless
destinations. Lifecycle creation/close/reserved placement from named workspaces
remain unsupported rather than bypassing numbered identity guards. Urgency is not supplied
by stock workspace JSON and remains false in the existing bar projection.
Geometry resolves special rules and occupancy by name when a number is absent.
Empty special-workspace monitor entries are absent, and two missing numbers
never establish workspace identity by themselves.

Default CI runs transport/parser/guard unit tests and redacted 0.57.1 captures.
Before each coordinated release, run `tests/run-hyprland-acceptance.sh` using the
`hyprlandAcceptance` Nix app. It requires a renderer, starts only private compositor
and application processes, compiles every builder without executing it, constructs
dispatchers without dispatch, checks real guarded effects and parses live events.
Physical docking/suspend/hibernate and a production compositor restart are separate
user-run rollout checks. Tests must never claim those checks from nested evidence.
