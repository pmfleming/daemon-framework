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

A configured instance signature pins reads, event sockets and mutations to that
instance across the runtime and legacy roots. Discovery (sorted runtime-root,
then legacy-root order) applies only when no signature is configured.
`request_exact` additionally requires a signature. Invalid or non-Unicode
signatures fail explicitly. Socket ownership, protected instance directories,
symlink rejection and connected peer credentials are checked before exchange.
Commands are never replayed after a connection succeeds.

Discovery, connection and command exchange share one deadline, defaulting to
two seconds. `Client::with_ipc_timeout` overrides it for that client.
Downcast `anyhow::Error` to `IpcError` to distinguish `NotDelivered`,
`OutcomeUnknown`, `Rejected`, `InvalidReply` and `SessionGone`. A timeout before
connection is not delivered; an exchange timeout or I/O failure has an unknown
outcome. A malformed/oversized reply is not permission to retry a mutation.
`ack` requires exactly `ok` after trimming whitespace; the pin-and-JSON batch
is explicitly a separate protocol. If the configured socket disappears,
diagnostic discovery can report a different live instance as `SessionGone`;
it sends no request and never rebinds the client. No live alternative remains
an ordinary connection failure. Daemons must drain their normal shutdown path
and exit non-zero on `SessionGone`, so systemd can restart with current session
environment instead of continuing journals under another session identity.

`Client::watch_events(self, sender)` preserves custom client configuration.
`Event::Connected { instance }` identifies the socket reached; other controls
are `Disconnected(Option<String>)`, `ConnectFailed(String)`, `SessionGone` and
`Lagged`. Compositor notifications use `Event::Compositor(CompositorEvent)`.
Known v1/v2 names share variants; moved windows carry validated `WindowAddress`
and `WorkspaceRef` identities. Special workspaces have no numeric ID. Unknown
names remain `Unknown`, and malformed known payloads remain `Malformed`.
Consumers resynchronize on `Lagged`/`Malformed` rather than trusting lost state.
`invalidates_geometry` and `invalidates_preferences` replace raw prefix lists.

Event intake uses nonblocking sends. A full consumer channel drops compositor
notifications and delivers one pending `Lagged` when capacity becomes available,
even if the socket then goes idle. Connection controls are never dropped.
Receiver closure cancels connection attempts, reads, control sends and backoff.
Reconnect delays grow from one to eight seconds, including accept-then-EOF
failures. Receiving a frame or staying connected for one second resets the delay.
Identical
consecutive connection failures are suppressed until their cause changes or a
connection succeeds. Quiet healthy streams have no idle timeout.

Replies are capped at 16 MiB and event frames at 64 KiB including terminators.
Invalid UTF-8 is replaced with U+FFFD after checking raw byte limits. Invalid
JSON still fails typed reads. Oversized frames disconnect with a protocol cause.

`Client::work_areas` uses one native JSON batch and resolves workspace rules into
monitor-keyed logical insets. Parsing, monitor selectors, window/group counts and
rule-order semantics are tested here. Replies are bounded to 16 MiB; invalid
snapshots fail explicitly. `CompositorEvent::invalidates_geometry` identifies invalidating
compositor events. bar-daemon owns the shared on-demand cache and subscriber
lifetime, rather than creating a poller per UI surface.

`Client::preferences` reads `animations:enabled` over the native command socket
and returns a typed `Preferences` value. Boolean and legacy integer 0/1 replies
are accepted; missing, malformed, contradictory or wrong-option replies fail
instead of guessing an enabled default. `CompositorEvent::invalidates_preferences` identifies
config reloads. bar-daemon owns the shared cache, failure retry and subscription;
Shelllist retains environment overrides and animation presentation policy.

### Monitor commands

All Rust command builders return `Lua`, containing source without an `eval`
prefix. Send it with `execute(&client, &lua)` (also re-exported from `workspace`).
`Lua::then` composes validated commands atomically; it accepts no raw string.
Before effects, `Client::capabilities` requires Lua configuration, Hyprland
0.57.1 or newer and every API in `capabilities::LUA_APIS`. Failures are retried on
the next call; only successful checks are shared by clones of that exact client.
The probe only checks function types. Reads do not require mutation capabilities.

`lua` also builds unguarded address-based focus/close/move, ordinary-workspace
focus and numeric/boolean `hl.config` patches. These retain the consumers'
existing effect semantics; workspace guards remain in `workspace`. Read-only
option batches use `Client::options`, with batch delimiters rejected in keys.

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

Pass the result to `execute`; there is no new transport,
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
