# shelllist-hyprland

Bounded Hyprland IPC discovery, command/event transport and typed compositor adapters shared by Shelllist's Rust daemons.

This crate lives in the `daemon-framework` workspace. `app-daemon` and
`bar-daemon` consume `../daemon-framework/crates/shelllist-hyprland` directly;
no separate checkout, Nix input or vendored copy is required. It does not add a
Hyprland dependency to `shelllist-daemon-core` or `shelllist-daemon-tokio`.

## Migration provenance

Imported from the standalone `shelllist-hyprland` repository at commit
`61376dd84a0a8f5edef2da02b11b36cd0e3edbf4` ("Phase 4: resolve compositor work
areas and workspace rules in Rust"). The package name, version and public API
are unchanged. Rust source/tests are copied unchanged; package metadata now
inherits workspace settings. The original repository is retained as the
historical archive, not a build input. Future changes belong in this workspace.

Treat the framework, app/bar consumers and downstream Shelllist/desktop input
updates as one coordinated release. The framework addition must be available
before consumer CI can use the new path; current-branch cross-repository CI will
only be green once all participating updates have landed. The local-build tool
validates all current worktrees together before publication. No service restart
or desktop rebuild is needed to validate this migration.

Run `cargo test -p shelllist-hyprland --locked` from the framework root. The
workspace CI and Shelllist's `frameworkWorkspace` check include these tests.

## API and ownership

Discovery prefers the requested instance across the runtime and legacy roots,
then tries other socket candidates in sorted runtime-root / legacy-root order.
Stale sockets that refuse connections are skipped. Commands are never replayed
on another instance after a connection succeeds. Candidates must be sockets owned
by the effective user, inside owned instance directories that are not group- or
world-writable; symlink sockets and instance directories are rejected. Connected
peer credentials must also match before commands are sent or events are read.
This prevents cross-user impersonation, not impersonation by the same user.

`Client::request` uses the command socket directly with one two-second deadline
covering discovery, connection, writing, and reading (16 MiB reply limit).
For state-changing commands, use `Client::request_exact`: it requires a valid
instance signature and only tries that instance in the runtime and legacy roots.
A missing/stale target fails rather than mutating another session. It uses the
same deadline and never replays a command after connection.
Malformed or non-Unicode `HYPRLAND_INSTANCE_SIGNATURE` values cause all IPC
operations to fail explicitly instead of enabling discovery fallback.
`Client::instance_signature` lets callers inspect/validate configuration early;
constructors remain infallible. An absent signature still permits ordinary discovery.
Event connections include discovery in the same deadline; event lines are capped
at 64 KiB including their terminator, even when a peer never sends a newline.
Oversized or invalid UTF-8 events disconnect and reconnect with normal backoff.
Idle event streams have no read timeout, and receiver closure cancels all phases.
`watch_events_detailed` reports `Connected`, `Disconnected(Option<String>)`,
`ConnectFailed(String)`, and raw `Message` events. Disconnections carry a cause
for read/protocol errors, or `None` for EOF. Connection failures are reported even
at startup; identical consecutive failures are suppressed until the cause changes
or a connection succeeds. Consumers can distinguish failure from a quiet stream
and adapt fallback polling without receiving repeated identical errors.
Reconnection is delayed by one second, including accept-then-EOF failures; closing
the receiver stops the watcher. Downstream event matches must handle
`ConnectFailed` and the new `Disconnected` payload. The original unit-valued
`watch_events` API remains available and also wakes on reported failures.

`Client::work_areas` uses one native JSON batch and resolves workspace rules into
monitor-keyed logical insets. Parsing, monitor selectors, window/group counts and
rule-order semantics are tested here. Replies are bounded to 16 MiB; invalid
snapshots fail explicitly. `work_area::geometry_event` identifies invalidating
compositor events. bar-daemon owns the shared on-demand cache and subscriber
lifetime, rather than creating a poller per UI surface.

`Client::preferences` reads `animations:enabled` over the native command socket
and returns a typed `Preferences` value. Boolean and legacy integer 0/1 replies
are accepted; missing, malformed, contradictory or wrong-option replies fail
instead of guessing an enabled default. `preferences::preference_event` identifies
config reloads. bar-daemon owns the shared cache, failure retry and subscription;
Shelllist retains environment overrides and animation presentation policy.

### Monitor commands

`monitor::command(output, Rule)` encodes a validated Lua monitor request without
sending it. `Rule::Disable` explicitly sets `disabled = true`; `Rule::Enable`
always sets `disabled = false`, because `hl.monitor` merges into the existing
rule. Supplying mode/geometry alone cannot re-enable a previously disabled output.

Enabled rules use typed `Mode::Preferred` / `Mode::Exact("WIDTHxHEIGHT@RATE[Hz]")`
and `Position::Auto` / `Position::Exact { x, y }`. Scale must be finite and positive;
transforms use the compositor's 0–7 enum. `transform: None` preserves the old rule's
transform; `Some(0)` explicitly resets it. `mirror: ""` explicitly clears mirroring;
a nonempty value names the source output. Exact output/mirror identifiers accept
only ASCII letters, digits, hyphens and underscores, never selectors or executable
Lua. Exact modes retain their precision and lose only the optional `Hz` suffix.

Pass the result to `Client::request_exact`; there is no new transport,
implicit retry, state cache or persistence. An `ok` reply acknowledges the command,
not an observed hardware transition. The framework owns encoding, syntax checks
and merge-semantics regression tests, including a private command-socket test.

bar-daemon maps both manual layout settings and automatic laptop restoration into
these rules. It retains connector eligibility, advertised-mode checks, domain
scale/position limits, topology and mirror validation, last-screen protection,
docking policy, serialized effects, persistent layouts, Preview/Keep/revert and
observation-based confirmation. Its integration tests keep exercising those guards.

Other domain models remain in their owning daemons. Shelllist keeps QScreen size,
pixel clamping, window placement and presentation-specific layer rules.
