# daemon-framework

Shared Rust infrastructure for Shelllist's five domain daemons: `app-daemon`, `bar-daemon`, `bt-daemon`, `clip-daemon` and `nm-daemon`.

## Workspace and boundaries

| Crate | Responsibility |
| --- | --- |
| `shelllist-daemon-core` | Protocol envelopes, JSONL wire types, fixtures, staged/atomic files, bounded reads and owner-scoped operations |
| `shelllist-daemon-tokio` | D-Bus/JSONL transport, subscriptions, connection-scoped owner monitoring, task groups, bounded blocking lanes, resume detection and async file helpers |
| `shelllist-hyprland` | Optional bounded Hyprland IPC, work-area and compositor-preference interpretation |
| `shelllist-local-build` | Native snapshot/build CLI and source-policy tests; not a daemon dependency |

`shelllist-protocol-js` is a binary in **shelllist-daemon-core**, not a separate crate. It generates frontend constants from daemon-owned registries.

Domain schemas, validation, persistence policy and effects stay in their owning daemons; fuzzy ranking and UI policy stay in Shelllist. Hyprland does not become a core/Tokio dependency: app/bar consume its crate directly, bar owns shared cache/subscriber lifetimes, and Shelllist owns placement/animation. See [Hyprland APIs](crates/shelllist-hyprland/README.md) and [server lifecycle guarantees](docs/server-infrastructure.md).

## Current-source development

All five daemons consume one current sibling framework checkout. Cargo uses path dependencies; Nix uses `local-build`, never vendored copies or per-consumer revision pins.

`tools/local-build` bootstraps through Cargo/Rust and requires Git/Nix. The packaged native command includes Git/Nix and needs neither Cargo nor Python. It captures tracked edits once, excludes ignored build products and resolves only a disposable lock. Register new files with `git add` or `git add -N`. Ordinary `nix build`/`flake check` can recreate unwanted local pins.

From this repository:

```sh
tools/local-build develop .
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
tools/local-build check .
tools/local-build check ../shelllist --keep-going
tools/local-build build ../app-daemon
```

Transport tests require `dbus-daemon`, supplied by the development/check environments. Local-build tests use real Git/filesystem fixtures and mock Nix; after dependencies are available, they require neither network nor a Nix daemon. Check coordinated framework/consumer changes together before deployment; desktop `rebuild` reuses one frozen graph for checks and switching.

### Snapshot and approval API

- `preflight ROOT`: aggregate worktree diagnostics without snapshotting or changing Git tracking. Preparation validates again.
- `prepare ROOT DESTINATION`: retain the captured graph and print JSON (`flake`, `sources`, `storeSources`). Destination must not exist.
- `--root-is-snapshot`: copy an already-frozen root; destination must be outside it.
- `--capture-root`: retain `.approval-root` before lock resolution and return `originalRoot`, so rebuild approval uses the actual captured configuration/lock.
- `prune-lock ROOT LOCK`: remove local input roots and unreachable nodes from the supplied lock, preserving remote pins.

`check`, `build`, `develop` and `run` clean up their temporary graphs after the child exits. For build/develop/run, place `--attr NAME` before ROOT and forwarded arguments after it. Source overrides are rejected. See [local-build](docs/local-build.md) for contracts and bootstrap details; trusted Nix sources remain part of the deployment trust boundary.

## Nix outputs and caching

The flake exposes `localBuild`, `protocolBindings` (also default), workspace checks and development shells for `x86_64-linux` and `aarch64-linux`. Crane shares compiled dependencies across framework tools/tests. Filtered daemon sources exclude deployment tooling and the local-build crate. Packages expose `rebuildCache` for bounded deployment GC roots; `localBuild.unwrappedProgram` lets integration fixtures intercept Nix without changing production tool resolution.

Pipe a daemon registry JSON document into:

```sh
tools/local-build run --attr protocolBindings .
```

## Routed JSONL clients

Calls, subscriptions and cancellations accept optional bridge-local metadata:

```json
{"op":"call","id":"view::page","method":"clipboard.history.query","params":{"limit":200},"route":{"consumerId":"view","localId":"page","generation":1,"kind":"call"}}
```

`kind` must match the operation: `call`, `subscription`, `base-subscription` or `control`. Routes are echoed on responses, including errors, without entering domain D-Bus APIs. Subscription events retain routes, including events buffered before acknowledgement. Frontends reject retired generations; unrouted clients retain their wire format.

Request addresses live with bounded tasks, not a permanent dictionary. Routed `release` with kind `control` releases acknowledged subscriptions for its consumer/generation; destroyed consumers must still cancel IDs from late subscribe replies. Closing stdin drains accepted calls and cancels tracked IDs. Failed cancellations retain ownership for cleanup.

Admission is bounded before spawning; cancellation/release have a separate control lane. Overload is never automatically replayed. Global transport-error notifications invalidate the connection. Deploy routed frontends with matching rebuilt daemon clients, not a JavaScript routing-table fallback.
