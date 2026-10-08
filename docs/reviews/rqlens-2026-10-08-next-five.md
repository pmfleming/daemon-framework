# Next five quality improvements — 2026-10-08

Baseline: `693a34d`, initially clean. This pass prioritizes avoidable allocation,
one dependency cycle, and cancellation ownership. It does not claim that all
aggregate scores improve.

## Five changes

1. **Empty overlays retain shared inputs** (`policy.rs`). Copy-on-write now
   skips empty maps as well as absent overlays. The existing ownership test
   checks pointer identity for both cases and isolation for a nonempty overlay.
   Nonempty overlays still copy when their input map is shared.
2. **Graph/policy dependency cycle removed** (`graph.rs`, `policy.rs`). Graph
   traversal owns the validation coordinator; policy receives individual source
   facts and whether the framework was captured. Validation order and rejection
   rules are unchanged. RQLens confirms removal of `policy -> graph`, with no new
   dependency edges or modules. Existing policy and snapshot tests exercise the
   relocated coordinator.
3. **Bounded, allocation-free CSS shorthand expansion** (`work_area.rs`). Text
   and array forms share one iterator-based expansion routine. It examines at
   most five components, rejects excess/invalid/nonfinite values, and uses no
   scratch vector. Regression cases cover all four shorthand lengths in both
   formats, missing components, invalid types, infinities and excess values.
   The surrounding JSON document still has its existing allocation and size bound.
4. **Event identity extracted once** (`output_actor.rs`). Incoming commands
   extract an ID at the actor boundary. Buffered events carry that ID through
   delivery and terminal cleanup instead of re-running the policy and allocating
   replacement IDs. Existing terminal-event tests now count policy calls for
   immediate and buffered delivery, including suppression of later events.
   CorrelationPolicy signatures are unchanged; the retained-identity contract is
   documented. The four sibling daemon overrides were inspected: all derive IDs
   solely from the supplied stream/event.
5. **One cancellation scope for logind reconnection** (`resume.rs`). A single
   outer select cancels connection attempts, signal reads, sends and retry sleep.
   Only errors retry; normal completion means the consumer has closed. A biased
   closure branch prevents starting a connection when already closed. The
   no-consumer regression now covers both the resume publisher and its logind
   worker without accessing a live bus.

## Measurements and trade-offs

| Metric | Before | After |
| --- | ---: | ---: |
| Production cognitive sum / maximum | 344 / 8 | 343 / 8 |
| Production cyclomatic sum / maximum | 862 / 13 | 866 / 13 |
| Production function SLOC | 3,294 | 3,288 |
| Sum of production function Halstead observations | 1,916,435.76 | 1,908,829.63 |
| Maximum production function Halstead effort | 46,894.57 | 46,894.57 |
| Physical Rust lines, including tests | 7,550 | 7,602 |
| Dependency edges / modules | 93 / 38 | 92 / 38 |
| Mean locality | 99.29 | 99.29 |
| Mean observed-reuse leverage | 16.32 | 16.05 |
| Token clone groups / duplicated lines | 5 / 65 | 5 / 65 |
| Production reliability / escape-hatch findings | 0 / 0 | 0 / 0 |

Explicit bounded parsing and the separate validation coordinator increase the
cyclomatic sum slightly; the maximum is unchanged. Total Rust lines increase
with regression assertions, comments and explicit boundaries despite a small
production SLOC decrease. Leverage's dependency-count heuristic loses a consumer
when the unwanted cycle edge disappears; this is not lost useful reuse. Locality
is already saturated for the affected modules and does not change. There are no
new lint suppressions, unsafe code, dependencies or removed public APIs.

Production scope is a function/file naming heuristic, not compiler/cfg analysis.
Halstead values describe syntax, not developer hours; summing function
observations is not a whole-program effort measure. Allocation improvements are
established by code structure and identity/callback assertions, not a benchmark.

## Validation and provenance

- **90 tests passed, one existing live Hyprland probe ignored**; doctests passed.
- Formatting, strict workspace/all-target Clippy, rustdoc with `-D warnings`,
  and `git diff --check` passed.
- Consumer suites were not run. No consumer edits, deployments, service restarts
  or live compositor probes were performed.

Artifacts and logs: `target/framework-review-next-five/{baseline,after}/` and
adjacent files. Both captures use the preceding review's frozen local RQLens
instrument, SHA-256
`09b72768d645f97886ae4b22ced05553bbcac988fac290745e66f97405e94053`, and Rust/Cargo
1.95. Baseline fingerprint: `8f7b46145b2e95e6`; final fingerprint:
`2ba7832792e731cb`. Each batch has a consistent fingerprint.

Evidence remains **partial** because Cargo metadata does not fully resolve the
project/helper dependency graph. Map reports 43 unresolved references in both
captures. Coverage, correctness-run, runtime performance and MSRV compliance
are not established; the pre-existing 1.85 pin/declaration mismatch is unchanged.

```sh
R=target/framework-review-next-five
for metric in hotspots clones escape-hatches locality leverage reliability map; do
  target/framework-review-current/instrument/rqlens measure "$metric" --config "$R/rqlens.toml"
done
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --locked --no-deps
```
