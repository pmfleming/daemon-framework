# Next five quality improvements — 2026-10-15

Baseline: `bba2856`, the checkpoint of the preceding refactor. Each numbered
change is implemented, validated and committed before starting the next one.
RQLens baseline artifacts and per-step hotspot captures/logs are retained in
`target/quality-next-five/`; the instrument is the local
`../rust-quality-lens/target/debug/rqlens` with unchanged `rqlens.toml`.

## 1. Prune only the expired result-cache prefix

`RecentResults` appends records in timestamp order, including replacements.
Use the deque's partition-point search and drain the expired prefix instead of
scanning every retained entry on each read/record. This changes expiry selection
from a full scan to a logarithmic search; dropping expired values still costs
work proportional to the expired prefix. No IDs or values are cloned.

A deterministic regression covers disabled TTL, samples earlier than all
records, a partially expired prefix, exact-boundary expiry and eventual emptying.
Existing replacement, ownership and capacity tests remain.

Validation: formatting, strict workspace/all-target Clippy, `git diff --check`,
and all 89 workspace tests pass. No runtime benchmark claim is made.

## 2. Skip ancestor-sync preparation when disabled

`create_directory` now short-circuits ancestor discovery before filesystem
existence checks when `sync_new_ancestors` is false. The collected list itself
controls the sync loop, removing a redundant nested policy branch. Directory
creation, private modes, symlink rejection and root-to-leaf sync order remain.

A regression writes and replaces nested state with ancestor syncing both disabled
and enabled, independently of parent syncing. It checks contents, restoration of
private directory/file modes and temporary-file cleanup.

Validation: formatting, strict workspace/all-target Clippy, `git diff --check`,
and all 90 workspace tests pass.
