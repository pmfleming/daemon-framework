# Framework quality campaign, 2026-10-30

Baseline: `aeb58d3d9044342b1b3b52c5a60fd51188c8d0d8`, clean `main`.
Only daemon-framework is owned by this worker. Public APIs, dependencies, wire
formats, domain admission, acknowledgements and retry policy are preserved.

## Evaluator and reproduction

RQLens source: sibling `rust-quality-lens` at
`dfe016202134f1ea5fc5fd0e0c5da73c4b2b32b0` (clean when captured).
Built current source, without modifying that checkout, with:

```sh
cd /home/laufan/Projects/daemon-framework
source .direnv/flake-profile-*.rc
export CARGO_BUILD_JOBS=2
cargo build --locked \
  --manifest-path /home/laufan/Projects/rust-quality-lens/Cargo.toml \
  --bin rqlens --target-dir "$PWD/target/quality-campaign/lens-build"
export XDG_CACHE_HOME="$PWD/target/quality-campaign/cache"
for metric in hotspots clones escape-hatches locality leverage reliability; do
  target/quality-campaign/lens-build/debug/rqlens measure "$metric"
done
```

Binary SHA-256: `77785955fe7437925f697452c6b837fa9b60e7eb7c6da14108c113021fecfb46`.
Unchanged local `rqlens.toml` SHA-256:
`7ede29190858152bc4a6aa870f54089ed89883cbdd10ecc462d06ba6c45f341a`.
Default Cargo-derived workspace scope, no new exclusions or policy changes.
Cached repository Nix environment supplies Rust/Cargo 1.95.0 and dbus-daemon.
Raw reports/logs stay ignored in `target/quality-campaign/`; `before/` retains
the six freshly generated artifacts (other copied artifacts are not evidence).
Models: architecture risk v4, Rust standard complexity v2, Halstead v1.

Baseline `cargo test --workspace --locked`: **73 passed, 1 ignored**, no failures;
doctests pass (zero examples). The ignored D-Bus qualification profile requires
an explicit matched release run; it is not passing evidence.

## Stage 1: socket request lifecycle

Baseline `socket::connection` has cognitive complexity **38**, cyclomatic **21**,
function SLOC **103**, and syntax-derived Halstead effort **175,991.80**.
Separate the accepted request/reply lifetime from connection framing/cleanup,
flatten framing and validation, and move the already-owned service into accepted
work. Keep admission before spawning, detached effects holding permits, control
timeout retirement, ordered shutdown, and both immediate/late disconnect cleanup.
Native socket tests provide the behavioral boundary; new saturation/drain evidence
is recorded with this stage's validation below.

Validation: `cargo fmt --all --check`, `git diff --check`, strict workspace/all-target
Clippy pass. `cargo test -p shelllist-daemon-tokio --locked socket::tests`:
**16 passed**, 28 filtered. Post-initial reports: `target/quality-campaign/post-initial/`.
Connection cognitive complexity is **18**; extracted request execution is **4**.
Production cognitive sum **472 → 456**, cyclomatic **1,148 → 1,150**, effort sum
**2,817,844.69 → 2,791,368.99**. Production physical lines **6,609 → 6,650**:
the lifecycle boundary and formatting increase lines; this is not a LOC win.

## Ranked next five (selected after stage 1)

1. **Converge JSONL frame validation.** The remaining connection hotspot is 18;
   both ingress paths repeat oversized-frame reporting and malformed/blank-record
   handling. Give that policy one owner in the existing JSONL module, retain
   transport-specific lifetimes and error contexts, and exercise invalid routed
   messages followed by valid native traffic.
2. **Avoid ordinary socket route clones.** Accepted work unconditionally clones
   the full `ClientRoute` (two owned strings), although only Release reads it.
   Clone only for Release; retain original addresses for replies and timeouts.
   Exercise routed calls, foreign cancellation, subscriptions/release and timeout.
3. **Remove redundant snapshot bookkeeping.** `snapshot` builds a BTreeSet over
   Git's entire tracked-path list solely to deduplicate conflict stages. Ask Git
   for deduplicated paths, demonstrate real unmerged-index behavior, and remove
   the private preparation forwarding API so all callers name capture policy.
4. **Unify decimal decomposition.** `fixed_decimal` is tied for the next largest
   production cognitive complexity (9). It repeats the same exponent condition
   for mantissa and exponent; select the pair together. Keep exact binary-rational
   tie detection and extend rounding vectors with an independent integer oracle.
5. **Consolidate overflow behavior tests and explicit ownership.** Clone evidence
   identifies repeated native overflow setup. Parameterize the two real overflow
   scenarios, retain their distinct assertions and add healthy-peer evidence to
   both. Replace the three measured wildcard imports with explicit dependencies.
   No scenario/assertion is removed solely to reduce test count.

These are bounded implementation/test changes, not whole-program dead-code claims.
The only removed API planned above is private to the local-build binary crate.
Final metrics, coverage, validation and limitations will be recorded below.

## Stage 2: shared frame validation

Both socket and bridge now call `jsonl::parse_frame`; framing stays cancellation
safe and oversized records still drain before parsing. Byte-limit messages and
route validation are unchanged. Extend the existing native framing test with
silent blank input, malformed JSON and a wrongly routed `hold` call, followed by
successful Unicode traffic. That proves invalid input did not consume admission.

`cargo test -p shelllist-daemon-tokio --locked`: **43 passed, 1 ignored**;
doctests have zero examples. Crate/all-target strict Clippy and formatting pass.
Logs: `target/quality-campaign/stage2-{tests,clippy}.log`.

## Stage 3: route ownership

Only Release clones a route for detached dispatch. Calls, Subscribe and Cancel
avoid copying both route strings; their original address stays with the reply
waiter, including timeout/error responses. Release retains the same clone and
owner/generation query. A syntactic `.clone()` count understates that retained
Release cost because the implementation now uses `.cloned()`.

Extended existing native tests assert addressed call success, addressed foreign
cancellation rejection (despite equal consumer/generation metadata), and addressed
effect timeout. Existing early-event/release and late-effect cleanup assertions
remain. Selected native tests **16 passed**; crate/all-target strict Clippy,
formatting and diff checks pass. Logs: `stage3-{tests,clippy}.log` in the raw directory.

## Stage 4: snapshot bookkeeping

Use `git ls-files --cached --deduplicate -z` instead of collecting every tracked
path into a BTreeSet. Git remains the inventory owner; dirty contents, deletions,
ordering, metadata and confinement are retained. Remove the private `prepare`
forwarder and rename its five-argument implementation; every internal caller now
states both frozen-root and capture-root policy. The CLI is unchanged.

The new conflicted-file scenario passed against the previous implementation
(`stage4-before-tests.log`). Extend it to a conflicted symlink as well; the final
**5 unit + 3 CLI integration tests pass**, including approval-lock preservation.
Negative control: temporarily omitting `--deduplicate` makes this same test fail
at `copy symlink` with EEXIST (`stage4-negative-dedup.log`); the correct source was
restored. This expected failure is sensitivity evidence, not a passing test run.
Crate/all-target strict Clippy and formatting pass (`stage4-{tests,clippy}.log`).

## Stage 5: decimal decomposition

Select mantissa/exponent together at the normal/subnormal boundary instead of
testing the same exponent twice. Keep exact rational tie detection, negative-zero
normalization, limits and formatter output unchanged. Replace the decimal test's
wildcard import with its actual dependency.

The existing test now adds **1,028 exact-eighth/digit vectors**, with an independent
integer half-away-from-zero oracle, and four subnormal/normal boundary vectors.
These supplement, rather than replace, the existing decimal, nonfinite and bound
assertions. The expanded owner passes before and after the refactor. These are
vector counts, not 1,028 distinct behaviors or a numeric branch-coverage claim.
Core crate **12 library + 1 generator tests pass**, zero doctests; strict all-target
Clippy, formatting and diff checks pass. Raw logs: `stage5-{before-tests,tests,clippy}.log`.

## Stage 6: test ownership and consolidation

Replace two native overflow tests with one owner enumerating pre-ack and
synchronous overflow. Both original scenarios passed in the baseline and stages
1–3. The shared owner retains the pre-ack transport-error assertion, rejects any
later pre-ack output, requires actual EOF, verifies resource cleanup and executes
on a single runtime thread. The synchronous producer still asserts closed state
internally. Both scenarios now demonstrate a healthy peer after accepted work
actually releases admission; late pre-ack work is explicitly released.

Reuse the existing successful-deadline helper, retaining all three-second bounds
and explicit expected-timeout assertions. Consolidate six invalid-workload vectors
under their existing owner; keep separate zero/65-owner and 64-owner aggregate
budget cases. No guard vector is dropped. Move the existing typed route fixture
to shared transport test support and reuse it in actor/native tests; repeated local
IDs deliberately exercise isolation. This replaces repeated valid JSON route
construction; malformed wire vectors remain literal. Replace all remaining test
wildcard imports, including the two outside RQLens's default source-root inventory.

Stage-local Tokio suite: **42 passed, 1 ignored**. Final workspace validation and
comparable measurements below include the shared route-fixture follow-up too.

## Comparable baseline/final evidence

Same evaluator binary/config/helper inputs: v7 fingerprints are complete, config
digest `2cc64ebfc70637d7`, helper digest `896ede594acf7592`. All six focused producers
exit zero. This is not `rqlens enforce` or a complete-policy pass.

Production function records exclude test directories/files and `tests`,
`test_support`, `*_tests` namespaces. Physical Rust LOC uses all 58 tracked Rust
files, separating test directories, `tests.rs`/`*_tests.rs` and trailing inline
test modules. Module declarations remain with their file prefix. Counts include
comments/blanks; they are distinct from RQLens function SLOC and nonblank clone
scope. Architecture means include all 56 inventoried modules, including tests.

| Metric | Baseline | Final |
| --- | ---: | ---: |
| Production cognitive sum / max | 472 / 38 | 448 / 13 |
| Production cyclomatic sum / max | 1,148 / 21 | 1,144 / 17 |
| Production function Halstead effort sum | 2,817,844.69 | 2,780,719.21 |
| Maximum production function effort | 175,991.80 | 108,574.21 |
| Production function SLOC | 4,529 | 4,552 |
| Maximum production hotspot score | 242.82 | 112.67 |
| Production / test physical Rust LOC | 6,609 / 4,727 | 6,622 / 4,846 |
| Total physical Rust LOC | 11,336 | 11,468 |
| Named test cases / ignored | 74 / 1 | 74 / 1 |
| Duplicated lines / percentage | 141 / 1.45% | 150 / 1.52% |
| Explicit `.clone()` sites, production / all | 48 / 82 | 46 / 80 |
| Measured escape-hatch records | 3 | 0 |
| Production reliability findings | 0 | 0 |
| Mean module leverage / locality | 22.500 / 98.621 | 24.107 / 98.286 |
| LLVM covered / instrumented lines | 4,446 / 5,355 | 4,498 / 5,395 |
| LLVM line coverage | 83.03% | 83.37% |
| LLVM function coverage | 77.29% | 77.46% |

`connection` cognitive complexity falls **38 → 13**, cyclomatic **21 → 14**;
decimal decomposition falls **9 → 7** cognitive and **7 → 6** cyclomatic.
The extracted socket execution function owns real accepted-work/reply policy;
the removed local-build forwarding function offsets its function count.

Tradeoffs: **the total LOC and duplication objectives are not achieved**. Stage 6
removes 12 physical test lines net and combines two case owners without dropping
either overflow scenario, but the campaign adds 119 test lines and 13 production
lines overall. New saturation/drain, invalid-route, addressed-timeout, conflicted
index and numeric vectors outweigh consolidation. Repeated native traffic setup
and small independent error-format implementations remain clone candidates; they
are not forced into wrappers to lower the score. Shared test ownership increases
measured reuse and cross-module dependencies: locality falls **0.335 points**.
The 1.32% effort-sum reduction is a syntax metric, not measured developer time or
runtime performance. Actual common-path route/string copies are removed despite
the clone-site metric's spelling limitation described in stage 3.

## Final validation and limits

In the cached repository Nix development environment, with `CARGO_BUILD_JOBS=2`:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
git diff --check
```

All pass: **73 tests passed, 1 ignored**, zero failed; doctests have zero examples.
Final logs: `target/quality-campaign/final-{fmt,clippy,tests}.log`.

Coverage uses the existing lens Nix environment, also Rust 1.95.0:

```sh
source /home/laufan/Projects/rust-quality-lens/.direnv/flake-profile-*.rc
export CARGO_BUILD_JOBS=2
cargo llvm-cov --workspace --locked --json \
  --output-path target/quality-campaign/final-coverage.json
```

Baseline uses the identical command with `baseline-coverage.json`. Both runs pass
73 tests, ignore the same profiling test, and report 42 instrumented files.
Coverage includes inline test code and is **not production-only coverage**.
Branch and MC/DC coverage are not measured. Exact decimal vectors and the snapshot
negative control are behavioral/sensitivity evidence, not coverage percentages.

Snapshot-based Nix gate command:

```sh
CARGO_BUILD_JOBS=2 NIX_BUILD_CORES=2 \
  tools/local-build check . --keep-going --max-jobs 1 --cores 2
```

**All three checks pass**, including the current-source rerun after the shared
route-fixture follow-up. Log: `target/quality-campaign/final-nix-check.log`.

Limitations: native syntax evidence remains **partial** because a test invocation
of `declare_methods!` is unexpanded; semantic architecture leaves **13 references
unresolved** in both runs. Complete input fingerprints do not cure those limits.
Only the six listed lens producers were run. Optional mutation/feature-matrix,
live compositor and external consumer gates were not run. The explicit snapshot
negative control is not a general mutation-testing score. No safely removable
public production API was established; removed preparation indirection was private
to the CLI binary. Manifests, dependencies, locks, public signatures and protocol
fixtures are unchanged. Rust 1.95 is validated; the pre-existing advertised 1.85
MSRV is not validated. Raw reports remain ignored; the final commit's source is
remeasured after commit so artifact fingerprints identify the completed head.
