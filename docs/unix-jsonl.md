# Direct Unix JSONL ingress — staged migration

## Stage 1: framework listener (not a deployment)

`JsonlListener::bind(runtime, daemon_name)` creates `runtime/<daemon_name>.sock`.
The caller supplies XDG_RUNTIME_DIR, or an isolated test equivalent. The directory
must already be canonical, same-effective-UID-owned and 0700. The socket is 0700;
every accepted connection is checked with kernel peer credentials before parsing.
There is no caller-supplied identity and no dependency on session-bus name tracking.
A no-follow 0600 advisory lock serializes cooperating binders. Live/unresponsive
sockets, foreign files and symlinks are not replaced. Stale owned sockets may be
removed under that lock. Drop unlinks only the inode this listener created; the
lock inode remains. This is same-UID isolation, not a sandbox against that UID.

`LocalJsonlService` adapters MUST use the same service boundary and domain
coordinators as their D-Bus interface, not an alternative admission path.
`SocketSession::owner()` is server-minted and disjoint from D-Bus unique names.
Subscriptions use `spawn_until(..., session.closed(), ...)`; operation cancellation
remains owner-scoped domain policy. `disconnected()` is idempotent, signals
cancellation rather than rollback, and is called again after late accepted work.
An adapter must not assume that losing a reply stops a physical effect.

The socket reuses the bridge's bounded UTF-8 framing, ClientMessage validation,
request/control lanes, output actor, correlation policy, route echo and release
semantics. There is no D-Bus round trip or stdio subprocess in this path. D-Bus and
the legacy `client` command remain available and unchanged during the experiment.
The socket has bounded connections, pending requests, output queues, record sizes
and per-record write deadlines. A blocked writer retires the whole connection,
never resumes a partially written record on another connection.

Ordinary acknowledgement expiry is outcome-unknown and NEVER retries an effect.
Accepted work keeps its admission permit while it finishes, even when its waiter
has gone. Late work runs owner cleanup again. Control/subscription uncertainty
retires the connection. Shutdown drains reply waiters to their existing deadlines;
EOF and errors close ownership immediately. Domain cleanup must itself complete:
the framework cannot safely invent rollback or abort an arbitrary hardware cleanup.

Defaults: eight connections, 64 requests plus the existing separate 16-control
lane per connection, 16 output slots, 32 pre-ack events, the standard request byte
bound, 32 MiB response records, 30 s acknowledgement and 5 s write deadlines.
These are outer transport bounds, not replacements for service/method limits.
Encoding still allocates the producer's JSON value and one serialized output
record; it is not a zero-copy or total-heap-bound claim.

Validation uses actual Unix sockets: lock/stale inode/mode protection, owner-scoped
cancellation and release, event-before-ack routing, independent connection cleanup,
shared service admission with a direct-bus owner, late-effect cleanup, malformed
frame resynchronization, byte-fragmented UTF-8, 8 MiB replies and stalled readers.
The existing D-Bus/bridge transport tests remain in the same suite.

## Next gates

1. Connect Bluetooth's real coordinators to the shared listener and retain D-Bus.
2. Exercise a Quickshell 0.3 Socket adapter with the unchanged JSONL router.
   Its `write()` has no exposed byte count or bytesWritten signal: prove a bounded
   acknowledgement-based sending window rather than assuming writes backpressure.
3. Measure isolated old/new paths: memory (RSS AND proportional/private memory),
   p50/p99, restart, non-reading peers, fragmented Unicode and large records.
   Mock QML tests alone do not establish Quickshell Socket behavior.
4. Only after those gates, enable the new path and migrate the other daemons.
   Audit revisions/gap recovery for every stream, including the D-Bus consumers.
   Snapshot-only watch coalescing is not a delta gap, but reconnect must still
   establish a fresh snapshot. No delta stream may silently survive lost history.

Socket activation is deliberately deferred. It can retain the LISTENING socket
and queue new connections across restart. It does not preserve already accepted
connections or their ownership; those still need generation retirement and a
fresh snapshot. No live service, bus name or hardware setting is changed by tests.
