# Quality review and refactor

Baseline: `b45f10d`, compared with this working-tree refactor. Measurements use
`/home/laufan/Projects/rust-quality-lens/target/debug/rqlens` (0.1.0), architecture
model v4, complexity model v2, and Rust 1.95.0.

## Findings addressed

- **JSONL task lifetime:** an input/protocol-output I/O error could return before
  aborting the event and owner watchers. Reuse `AbortOnDrop` for both watchers,
  including caller cancellation, while preserving normal drain order.
- **Atomic-file complexity:** isolate exclusive temporary-file creation/retry;
  configure open options once. Merge duplicate directory-mode branches without
  changing symlink refusal, permission enforcement, cleanup, or durability.
- **D-Bus coupling and duplication:** move name-change handling into `owner.rs`
  and share signal setup. Remove the duplicate proxy and single-use predicate/
  dispatcher wrappers. D-Bus transport now exposes a signal stream rather than
  depending on the JSONL output actor; JSON decoding/readiness belong in JSONL.
- **Correlation ownership:** one map owns live IDs and optional subscription
  routes, eliminating a duplicated subscription key. Share ID queries and
  cancellation/terminal cleanup; discard cancelled pending events in place.
  Move event payloads rather than explicitly cloning them before wrapping.
- **Admission and parsing:** borrow request slots, clone task handles only after
  admission, move rejected request IDs, and share protocol-error handling.
- **Tests:** replace eight wildcard imports with explicit dependencies; share
  routing fixtures. Test immediate/buffered terminal-event routing and repeated
  unaddressed responses, and exercise the extracted D-Bus signal-stream API.

Public APIs and wire formats are unchanged. Remaining exported APIs were not
removed just because this workspace has no local callers: sibling daemons use
this library. No whole-program dead-code elimination is claimed.

## Measurements

Production excludes inline test modules and `*_tests.rs`. Physical Rust lines
include comments/blanks; `.clone()` counts are literal call sites, not allocation
measurements. Duplication is RQLens token/AST evidence, not runtime cloning.

| Metric | Before | After |
| --- | ---: | ---: |
| Production cyclomatic sum / maximum | 515 / 16 | 506 / 12 |
| Production cognitive sum / maximum | 204 / 8 | 193 / 6 |
| Maximum function hotspot score | 76.50 | 57.62 |
| All-function cyclomatic / cognitive sums | 633 / 216 | 637 / 208 |
| Duplicated lines / percentage | 130 / 3.28% | 64 / 1.62% |
| Escape-hatch findings (test glob imports) | 8 | 0 |
| Explicit `.clone()` call sites | 44 | 39 |
| Production Rust lines | 2,891 | 2,847 |
| Total Rust lines, including tests | 4,356 | 4,350 |
| Mean non-test-module leverage | 67.71 | 67.83 |
| Mean all-module leverage | 67.35 | 67.33 |
| Mean all-module locality | 98.96 | 98.96 |

The extra test/fixture functions increase aggregate cyclomatic complexity.
Leverage improves slightly outside tests, not across the whole project. Locality
is unchanged: all implementation modules already score 100; the two public
re-export facades account for the remaining risk. Keeping related behavior
local and removing a D-Bus-to-output dependency does not move that saturated
score. RQLens does not emit a calibrated effort metric here; no quantified
engineering-effort or runtime-performance improvement is claimed.

Both runs parsed all 23 Rust files, but artifacts are **partial**: RQLens could
not resolve its complete project/helper dependency fingerprint. Architecture
identity also includes syntax fallbacks for unresolved rust-analyzer references.
Treat the numbers as comparative static evidence, not a complete quality gate.
Fresh artifacts are in `target/quality-review-before/` and
`target/quality-review-after/`; unrelated older analysis artifacts are excluded.

## Validation and limitations

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo test --workspace --locked`: 45 passed (baseline 44), plus doctests.
- Live-sibling `cargo check --all-targets --locked`: app, bar, Bluetooth and
  NetworkManager passed. Bar/Bluetooth use the approved Bluetooth development
  shell for native dependencies; app/NetworkManager use the framework shell.
- Clipboard's compatibility check is blocked by its dependency
  `clipboard-history-core` requiring nightly; no bootstrap escape hatch was used.
- MSRV remains an existing follow-up: manifests/toolchain pin advertise Rust
  1.85, but the source already uses let chains (stabilized in 1.88). This review
  verifies 1.95 only; it does not certify the advertised MSRV.

Reproduce focused measurements from the framework development shell:

```sh
RQL=/home/laufan/Projects/rust-quality-lens/target/debug/rqlens
for metric in hotspots clones escape-hatches locality leverage reliability; do
    "$RQL" measure "$metric"
done
```
