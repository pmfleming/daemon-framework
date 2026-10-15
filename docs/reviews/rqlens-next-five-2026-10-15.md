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

## 3. Canonicalize window-group identities once

Sort each client's group-membership list once after decoding the snapshot.
Group-count selectors now hash borrowed slices, removing a temporary vector and
sort per candidate per evaluation. The group-filter predicate also avoids nested
returns. Member order is ignored, but duplicate members are deliberately retained
to preserve the previous key semantics; empty groups are still excluded.

The existing group/visibility regression now checks empty-group exclusion and
distinct multiplicities as well as shuffled-equivalent groups. No public types
or wire formats change.

Validation: formatting, strict workspace/all-target Clippy, `git diff --check`,
and all 90 workspace tests pass (additional cases in the existing test).

## 4. Normalize workspace-rule gaps at decoding

Decode rule gaps into optional fixed-size logical insets, sharing the CSS
shorthand parser with the global option. Each rule is normalized once rather
than reparsed for each matching monitor; snapshots no longer retain each gap's
JSON tree. The private deserializer intentionally maps invalid gap values to
`None`, preserving fallback to an earlier valid rule or the global option.

Additional cases cover null, booleans, objects, empty/malformed/oversized arrays,
invalid strings and nonfinite text. Valid fallback and monitor isolation are
checked for every case; existing shorthand and missing-field cases remain.

Validation: formatting, strict workspace/all-target Clippy, `git diff --check`,
and all 90 workspace tests pass (additional cases in the existing gap test).

## 5. Bound JSONL shutdown cancellation cleanup

Previously, post-drain cleanup could wait indefinitely for the cached connection
mutex, a new connection, the output actor's ID query or a cancellation reply.
One shared five-second control budget now bounds that entire best-effort pass.
Errors remain best-effort, no request is replayed, and owner-loss cleanup remains
the server-side fallback when the bridge connection closes. The lifecycle
contract is recorded in `docs/server-infrastructure.md`.

A deterministic regression holds the connection mutex across cleanup, uses a
short test budget and checks that cleanup returns without changing cached state
or emitting unsolicited output. It requires no live bus. This budget does not
bound stdout flushing or the intentionally unbounded accepted-call drain when
`shutdown_timeout` is `None`.

Validation: formatting, strict workspace/all-target Clippy, `git diff --check`,
and all 91 workspace tests pass. Final rustdoc with `-D warnings` also passes.

## Measurements and trade-offs

All-function sums include tests; Halstead observations describe syntax, not
engineering hours or measured runtime cost. Lower locality risk is better.

| Measurement | Before | After |
| --- | ---: | ---: |
| `create_directory` cognitive complexity | 6 | 4 |
| `window_count` cognitive complexity / function SLOC | 6 / 21 | 3 / 12 |
| All-function cognitive complexity sum | 501 | 501 |
| All-function cyclomatic complexity sum | 1,286 | 1,296 |
| Summed function Halstead effort, rounded | 4,172,957 | 4,260,574 |
| Rust nonblank source lines | 8,362 | 8,459 |
| Physical Rust lines, including tests | 9,142 | 9,250 |
| Token clone groups / duplicated lines | 3 / 42 | 3 / 42 |
| AST clone groups | 1 | 2 |
| Production reliability / escape-hatch findings | 0 / 0 | 0 / 0 |
| Observed-reuse leverage score sum | 750 | 750 |
| Locality risk sum | 33 | 33 |

These five changes prioritize repeated-work elimination and bounded cleanup,
not minimizing every aggregate metric. Added regression cases, decoding and
explicit timeout handling raise cyclomatic/effort/size totals; no aggregate
LOC or leverage/locality improvement is claimed. The extra AST clone group is
a coarse structural match between two unrelated one-line adapters,
`deserialize_gaps` and `parse_work_areas`, not repeated policy worth abstracting.
No new unsafe code, lint suppressions, dependencies or public API changes were
introduced. Public library symbols are not assumed dead from workspace-local
consumer counts.

Allocation/work improvements follow from the implementation: prefix search,
short-circuited metadata discovery, borrowed group keys and fixed-size rule gaps.
No runtime benchmark, allocation profile or end-to-end shutdown bound is claimed.

## Evidence and remaining limits

Baseline artifacts are in `target/quality-next-five/baseline/`, final artifacts
in `target/quality-next-five/after/`, and per-step hotspot artifacts/test/Clippy
logs are adjacent. Every implementation passed the full workspace suite and
strict Clippy before its commit. Final source measurements were captured before
the fifth commit; all six final artifacts share one input fingerprint.

RQLens evidence remains partial: Cargo project/helper dependency resolution is
incomplete, and the contract declaration macro is unexpanded. The existing
advertised Rust 1.85 MSRV mismatch remains; this pass validates Rust 1.95 only.
Consumer builds, Nix/deployment checks, coverage and live compositor behavior
were not validated.

```sh
for metric in hotspots clones escape-hatches reliability locality leverage; do
  ../rust-quality-lens/target/debug/rqlens measure "$metric" --config rqlens.toml
done
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --locked --no-deps
```
