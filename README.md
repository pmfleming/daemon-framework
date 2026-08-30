# daemon-framework

Shared Rust infrastructure and process services for the Shelllist daemon family.

## Workspace crates

- `shelllist-daemon-core` — runtime-independent protocol, envelope, JSONL wire, fixture, and secure state helpers.
- `shelllist-daemon-tokio` — Tokio and session D-Bus transport, ordered output, ownership, subscription, and shutdown helpers.
- `shelllist-search` — typo-tolerant fuzzy result ranking process used by Shelllist.

Domain policy remains in `app-daemon`, `bar-daemon`, `bt-daemon`, `clip-daemon`, and `nm-daemon`. This workspace contains only reusable process infrastructure and services.

## Development

```bash
nix develop
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
nix flake check
rqlens measure hotspots
```

Run the search JSONL service with:

```bash
nix run .#shelllistSearch
```

Each input line is one search request and each output line is its ranked key response.
