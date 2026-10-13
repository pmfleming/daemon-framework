# Test-suite reduction — 2026-10-13

Baseline is the working tree after the preceding RQLens refactor, not `HEAD`.
Cargo enumerated **117 tests: 115 enabled and two ignored**. The whole-test target
is `round(117 × 0.67) = 78`. Final: **78 enabled tests, none ignored** — 39 fewer
registered tests, a **33.33% reduction**. No tests were hidden behind new cfgs,
ignored, or removed from workspace discovery. This pass changes tests only;
production changes already present from the preceding refactor are retained.

| Cargo suite | Before | After |
| --- | ---: | ---: |
| daemon-core | 13 | 10 |
| daemon-tokio | 43 | 32 |
| hyprland | 36 | 23 |
| local-build unit tests | 21 | 10 |
| local-build CLI integration tests | 4 | 3 |
| **Total** | **117** | **78** |

This is not just wrapping the old suite in fewer functions: roughly 600 Rust
lines were removed. Related boundary cases remain table-driven where they share
one contract, while redundant setups, loops and assertions were deleted.

## Removal and consolidation rationale

- **Core envelopes/routing:** remove allocation-address identity assertions and
  redundant compatibility/default-deserialization smoke tests. Semantic success,
  optional error fields, correlation precedence, valid/invalid route metadata,
  addressed responses and shutdown wire shape remain covered. Pointer identity
  and Serde's default-null implementation are no longer suite requirements.
- **JSONL deadlines:** remove the ignored manual timing sample and two synthetic
  timeout-wrapper tests. Retain real-bus slow/applied-call isolation, no replay,
  domain failures, readiness rejection, late subscription cleanup after owner
  loss, overload rejection and cancellation-success policy. Remove the separate
  normal owner-loss/registry integration test: real-bus monitor independence,
  the harder late-subscription case and registry lifecycle tests cover its
  essential responsibilities at their owning boundaries.
- **Actor/ownership:** remove the closed-actor ID-query implementation check.
  Replace 20,000 repetitive requests with a just-over-capacity cancellation
  history case; ordinary replies already verify non-retention. Keep response
  priority, early-event routing, terminal buffered-event suppression, owner and
  generation isolation, failure retention, reset and bounded history. Remove
  the exact event-identity callback count, which constrained implementation
  rather than output behavior.
- **Lifecycle/framing:** combine monitor stopped/late/dropped cases into its
  lag/failure test, task shutdown checks into the reentrant-cleanup regression,
  registry destruction into its lifecycle test, and resume startup/stall cases
  into the deduplication sequence. Retain failed joins, stale cleanup versus
  reused IDs, blocking admission/panics/shutdown and consumer-closure tests.
  Fold invalid UTF-8 into the framing boundary matrix; limits 0, 1, 3 and 8
  distinguish bytes from characters. Oversized suffix draining and cancelled
  reads remain separate regression tests.
- **Hyprland IPC:** remove redundant request/preference/monitor transport smoke
  tests, the direct timeout-wrapper test, the duplicate signature-helper test
  and the ignored live compositor probe. Keep a real batch request, public
  signature validation, exact-instance targeting, two-root fallback, no replay
  after connection, read deadlines, peer credentials and filesystem protection.
  Move missing/unreadable discovery-root cases into discovery testing. Exercise
  CRLF, blank/partial lines, exact frame limits, invalid UTF-8 and oversize
  failures through the real event stream instead of duplicate parser/forwarder
  fixtures. Reconnection, failure causes, backoff and receiver cancellation remain.
- **Hyprland domain rules:** combine CSS validation with gap/rule precedence,
  directional geometry with ties/disabled monitors, and preferred/auto encoding
  with exact/disable/mirror encoding. Remove the extreme-value test that mandated
  accepting arbitrary geometry without future policy limits; retain ordinary
  mirror encoding, Lua-injection rejection, invalid numeric inputs and selectors.
- **Local build:** consolidate dirty/deleted/ignored files, untracked rejection,
  symlink confinement and metadata preservation into one snapshot lifecycle.
  Merge related root-policy rejection and daemon-dependency matrices; combine
  remote-pin preservation with follows/cycle pruning. Remove Rc/pointer identity
  tests: distinct-overlay traversal still checks semantic isolation, and malformed
  overlays are now rejected through preflight. Move CLI argument/override matrices
  from parser-only tests into the executable integration tests. Combine manifest
  and prune CLI checks with approval capture, retaining original-lock preservation.
  Keep failure cleanup, graph cycles/collisions, shared snapshots, preflight
  aggregation and no-live-lock-write regressions.

## Verification and evidence

- `cargo test --workspace --locked`: **78 passed, 0 failed, 0 ignored**; doctests pass.
- Formatting, workspace/all-target Clippy with `-D warnings`, rustdoc with
  `-D warnings`, and `git diff --check` pass on Rust/Cargo 1.95.
- Before/after Cargo inventories, baseline source snapshot, test logs and the
  reduction-only diff are in `target/test-reduction/`. This separates this pass
  from the already-uncommitted production refactor.
- Coverage percentages and mutation scores were not measured (`cargo-llvm-cov`
  is unavailable). The mapping above is a reviewed behavioral-coverage rationale,
  not a claim of unchanged numerical coverage. Live compositor checks, consumer
  builds and deployment were not run. The earlier RQLens review records the
  preceding state; its metrics are not a measurement of this reduced suite.

Count all registered tests, including ignored tests, with:

```sh
cargo test --workspace --locked -- --list > /tmp/daemon-framework-tests.txt
# Only test records; exclude per-target summaries and doctest headers.
grep -c ': test$' /tmp/daemon-framework-tests.txt
```

The target applies to this reduction, not a permanent ceiling preventing new
regression tests when behavior changes.
