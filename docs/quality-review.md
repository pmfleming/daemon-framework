# Quality review and refactor

The measurements below predate the [native local-build migration](local-build.md).
The build utility and its original 13 Python test scenarios now live in Rust and
run through Cargo; historical measurements and validation commands are retained
as evidence of those earlier passes, not totals for the expanded workspace.

## Follow-up review: baseline `d64d8c7`

Measured with the local `rust-quality-lens/target/debug/rqlens` 0.1.0 and
Rust 1.95.0. Focused baseline/current artifacts are in
`target/quality-pass-before/` and `target/quality-pass-after/`.

### Changes

- **Buffered terminal events:** previously, replay emitted every buffered event
  even after a terminal event retired its ID. Live delivery and replay now share
  `OutputState::accept_event`, so subsequent buffered events are suppressed too.
  Remove the intermediate disposition enum and separate async event handler.
- **Task lifecycle reuse:** `TaskGroup` and `OwnedTaskRegistry` share
  abort-all-before-join behavior. Synchronous blocking calls reuse the async
  admission/reply/panic path; the synchronous-only calling contract remains.
- **Locality:** internal dependencies reference their owning modules rather than
  public re-export facades. Event decisions stay in output state; stdout selection
  stays in the JSONL entrypoint, removing a redundant actor factory wrapper.
- **Allocations:** release queries carry only consumer ID and generation, avoiding
  the unused local-ID string clone. Envelope construction no longer builds a
  temporary map or removes fields that it immediately overwrites.
- **Smaller decision paths:** separate root priority from per-root Hyprland
  discovery, reuse constructor signature validation, use bounded route-length
  checks, and remove duplicated correlation handling and test policy overrides.
- **Tests:** cover all envelope correlation variants, terminal replay suppression,
  synchronous blocking results/panics/shutdown, and missing-signature socket
  discovery. Share blocking fixtures. Remove an overlapping actor-ordering test
  that did not deterministically exercise buffering; deterministic early-event
  routing and priority-channel tests remain. Move a drop-guard type to test-module
  scope so RQLens can resolve its implementation.

No exported API was removed, no lint allowances or unsafe blocks were added,
and no uncalled public library API was assumed dead. Remaining ownership clones
are retained rather than traded for more complicated lifetimes.

### Measurements

Production excludes inline test modules and `*_tests.rs`; physical Rust lines
include comments/blanks. Function SLOC is RQLens's extracted function measure,
not whole-file LOC or a calibrated effort estimate.

| Metric | Before | After |
| --- | ---: | ---: |
| Production cyclomatic sum / maximum | 632 / 13 | 628 / 13 |
| Production cognitive sum / maximum | 252 / 9 | 249 / 8 |
| All-function cyclomatic / cognitive sums | 796 / 281 | 793 / 280 |
| Production function SLOC | 2,496 | 2,447 |
| Duplicated lines / percentage | 97 / 2.00% | 75 / 1.56% |
| Production / total physical Rust lines | 3,629 / 5,314 | 3,590 / 5,284 |
| Explicit `.clone()` sites, production / all | 25 / 38 | 25 / 38 |
| Escape-hatch / production panic-path findings | 0 / 0 | 0 / 0 |
| Mean module leverage / locality | 13.33 / 99.11 | 13.33 / 99.11 |
| Maximum function hotspot score | 57.62 | 57.62 |

The route-query change eliminates one string clone per release, despite unchanged
clone-call site counts. Reuse and responsibility placement improve in the code,
but module leverage/locality scores **do not improve**. Lower function SLOC and
duplication are maintenance-effort proxies, not measured engineering effort or
runtime performance. The highest remaining hotspot is `file::create_directory`;
its filesystem durability/error branches were not removed to improve a score.

Evidence remains partial: the quality fingerprint reports an incomplete Cargo
project/helper path dependency graph, and architecture has 42 unresolved
references. The block-local test-type limitation is resolved. The existing
1.85 manifest/toolchain pin versus let-chain syntax mismatch remains; this run
validates 1.95, not MSRV compliance.

Validation: formatting, strict workspace/all-target Clippy, 57 Rust tests
(one existing live Hyprland probe ignored), doctests, and 13 Python tests pass.
All-target locked checks of the live `app-daemon` and `nm-daemon` siblings pass;
other siblings and live compositor behavior were not revalidated in this pass.
Reproduce with the focused commands at the end of this document; copy each run's
six artifacts before editing to retain comparable evidence.

---

## Previous review

Baseline: `db9d8fe`, compared with this working tree. Measurements use the local
`/home/laufan/Projects/rust-quality-lens/target/debug/rqlens` (0.1.0), architecture
model v4, complexity model v2, observed-reuse leverage v2, and Rust 1.95.0.
The local checkout is named `rust-quality-lens`, not `rustqualitylens`.

## Findings addressed

- **Hyprland event lifetime:** separate a connected event stream from reconnect
  policy. One receiver-closure cancellation boundary covers connection attempts,
  reads, sends and backoff. Preserve connection notifications and EOF backoff.
- **Work-area complexity:** select directional candidates without mutable winner
  bookkeeping, preserving first-match ties. Parse selector conjunctions without
  a mutable cursor. Search rules backwards for the last valid matching gaps,
  preserving invalid-rule fallback and logical-coordinate interpretation.
- **Subscription ownership:** reuse core `OwnedOperations` for admission,
  duplicate IDs, owner checks and terminal claims instead of maintaining those
  rules twice. Add `insert_with` so rejected tasks are never constructed/spawned.
  Preserve the start gate, generation guard, shutdown and public error messages.
- **Routing allocations:** serialize the response address, then move its route
  into live subscription ownership. No retained-route clone is needed; repeated
  unaddressed responses still preserve ownership.
- **Redundant code and tests:** remove the single-use `absolute_path` wrapper,
  redundant task-owner bookkeeping and the separate state-test ID counter.
  Share RAII temporary-directory and native socket fixtures; temporary state
  files now also get cleanup on test failure. Replace the wildcard import.
- **Correctness:** reject `.` and `..` as Hyprland instance signatures. Previously
  these passed validation despite naming the runtime root or its parent.
  Extend regression coverage for rejected factories, duplicate task IDs, invalid
  late rules, selector whitespace/syntax, directional ties and disabled monitors.

Existing public APIs and wire formats are retained; `insert_with` is additive.
No exported API was deleted just because this workspace has no local callers:
these crates serve sibling daemons. No whole-program dead-code claim is made.

## Measurements

Production excludes inline test modules and `*_tests.rs`. Physical Rust lines
include comments/blanks. Clone calls are syntax counts, not allocation counts.

| Metric | Before | After |
| --- | ---: | ---: |
| Production cyclomatic sum / maximum | 635 / 13 | 632 / 13 |
| Production cognitive sum / maximum | 277 / 12 | 252 / 9 |
| Maximum function hotspot score | 67.37 | 57.62 |
| All-function cyclomatic / cognitive sums | 793 / 302 | 796 / 281 |
| Duplicated lines / percentage | 142 / 2.92% | 97 / 2.00% |
| Escape-hatch findings (test glob imports) | 1 | 0 |
| Explicit `.clone()` sites, production / all | 27 / 39 | 25 / 38 |
| Total Rust lines, including tests | 5,318 | 5,314 |
| Mean module leverage | 13.33 | 13.33 |
| Mean module locality | 99.11 | 99.11 |

The all-function cyclomatic sum rises with helpers and stronger tests. The clone
count includes one spelling change (`String::clone` to `str::to_owned`) in task
registration; that allocation remains. The actual avoided production clone is
`ClientRoute`, including its owned strings. An added test clones a JSON fixture.

Shared ownership logic and fixtures improve reuse in the implementation, but
RQLens leverage/locality scores do **not** improve. Locality was already saturated
outside the two public re-export facades; unresolved cross-crate identities also
limit reuse evidence. RQLens emits no calibrated effort metric here. Smaller
hotspots and less duplicated policy suggest lower maintenance effort, not a
measured engineering-effort or runtime-performance improvement.

Both runs parse all 27 Rust files, but evidence remains **partial**: incomplete
quality fingerprints, unresolved semantic dependency references, and a block-local
test type in `task.rs`. Treat the figures as comparative static evidence, not a
complete quality gate. Artifacts are in `target/refactor-before/` and
`target/refactor-after/`; previous review artifacts use a different baseline.

## Validation and remaining limits

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo test --workspace --locked`: 57 passed (baseline 56); the existing live
  Hyprland probe remains ignored. Doctests passed.
- `python3 tools/test-local-build.py`: 13 passed.
- Live-sibling `cargo check --all-targets --locked`: app, NetworkManager,
  Bluetooth and bar passed. Bluetooth/bar required the approved Bluetooth
  development shell for native dependencies; initial plain-shell attempts failed.
- Clipboard remains blocked by `clipboard-history-core` requiring nightly.
- Existing MSRV mismatch remains: manifests advertise 1.85 but source uses let
  chains (stable in 1.88). This review verifies 1.95, not the advertised MSRV.

Reproduce focused measurements:

```sh
RQL=/home/laufan/Projects/rust-quality-lens/target/debug/rqlens
for metric in hotspots clones escape-hatches locality leverage reliability; do
    "$RQL" measure "$metric"
done
```
