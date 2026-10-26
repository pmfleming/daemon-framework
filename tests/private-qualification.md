# Reusable private qualification checkpoint

Read the [evidence contract](../contracts/private-qualification.md) and
[matching sibling-source build rules](../docs/local-build.md) first.
No final latency/memory comparison is recorded here. Correctness timings, including
release-built correctness suites, are **not** matched performance measurements.

## Audit before implementation

| Existing tooling | Reused scope / limitation |
| --- | --- |
| `crates/shelllist-daemon-tokio/src/socket/tests.rs` | Real Unix listener/writer, routes, foreign cancellation, independent owner cleanup, fragmented/large records, pre-ack overflow, synchronous burst overflow on a single-worker runtime, stalled reader, admission retained after acknowledgement deadline and late accepted-work cleanup. Keep these safety fixtures; no second transport. |
| `crates/shelllist-daemon-tokio/src/jsonl_deadline_tests.rs`, `service_policy_tests.rs`, `service_tests.rs`, `transport_tests.rs` | Retained bridge/direct D-Bus deadlines, ownership and admitted-service policy. Private bus configuration is `crates/shelllist-daemon-tokio/tests/session.conf`, with no host-service activation. |
| `../bt-daemon/src/daemon/tests/socket/probe.rs` | Private native Quickshell acceptance, Bluetooth-specific alternating sequential snapshot timings and smaps process-group snapshots. Tied to Bluetooth Rig/QML; permits debug builds, reports narrower identity data, not reusable mixed load or final release acceptance. Do not replace its native Qt coverage. |
| `../shelllist/tests/benchmark-architecture.sh` | Private app/bar reliability summaries, debug limitations explicitly recorded; not a matched transport latency/memory harness. |
| `../shelllist/tests/generate-performance-benchmarks.js` | Synthetic JavaScript/policy workload; not native socket/backpressure evidence. |
| `docs/proxy-performance.md` | Existing ignored private method-proxy investigation, not socket-vs-bridge or whole-shell performance. |
| Producer native probes and clipboard payload tests | Preserve domain/native fixtures and exact clipboard limits; one daemon's payload fixture is not another's maximum. |

## What was added

- `crates/shelllist-daemon-tokio/tests/support/qualification.rs`: path-included,
  test-only `Adapter`/`Workload`/`Traffic`, bounded concurrent owner scheduling,
  per-owner/per-step nearest-rank p50/p99 and exact traffic counts, explicit PID
  smaps snapshots and SHA256 source/binary identities. No runtime exports or new
  Cargo dependencies/lockfile changes.
- `crates/shelllist-daemon-tokio/src/socket/tests/qualification.rs`: adapter over
  the **existing private socket test client/service**, three healthy owners and an
  unread fourth connection. Checks foreign status/cancel denial, retained healthy
  ownership, renewal baselines, sample/traffic counts and healthy reads after
  actual writer-driven stalled-peer retirement (before closing the unread client).
  Small output/pre-ack capacities and a 50 ms fixture write deadline deliberately
  exercise correctness, not production performance. Status here means owned fake
  subscription presence, **not operation terminal recovery**.
- Small test-only extension to `socket/tests.rs`: fake owned-status and measured
  raw response length, shared by the existing client helper. Runtime unchanged.

The suite also checks percentile ranks, missing/malformed memory fields, bounds,
SHA256, duplicate PID rejection and timeout failure without retry or a completed
sample. The mixed native fixture runs on a current-thread runtime; multi-worker
sustained qualification and heap high-water/plateau evidence remain separate gates.

### Fresh W1-FW implementation checkpoint (2026-10-26)

The inherited scaffold is input, not integration approval. Fresh additions:

- Mixed native requests now carry validated consumer/local/generation/kind routes;
  responses and renewal baselines must echo the exact route, not just an ID.
- A real-socket adapter times out **after** its subscription baseline is admitted.
  It runs once, returns no completed workload, and its owned resource remains
  readable by that owner until explicit connection cleanup. Foreign status/cancel
  is denied; a healthy owner's status/read/subscription survives. This is owned
  subscription evidence, not a new operation-terminal recovery claim. Existing
  `deadlines_do_not_abort_effects_or_free_shared_admission_and_late_work_is_cleaned`
  remains the accepted-effect/admission prerequisite.
- Identity children/output/input are bounded and direct children are reaped on
  failure. Private builtin-only shell vectors exercise nonzero exit, output flood
  and a child that never reads its bounded input. No descendant-spawning fixture
  is passed to the identity runner.
- `tests/support/qualification_identity_tests.rs` exercises private Git committed,
  ignored, untracked, intent-to-add, staged and edited inputs; clean-only refusal;
  changed commit/tree/diff and binary hashes; subdirectory-root refusal; missing
  binaries/PIDs and malformed/overflowing memory. Unknown is never a zero sample.
  Source capture now fences ordinary concurrent changes; memory captures also
  recheck executable identity. Workload deadlines are capped at 1,800 seconds;
  exact nanosecond fields avoid hiding sub-millisecond deadlines as zero. These remain point/snapshot checks, not atomic
  kernel-wide process-tree or adversarial edit-and-restore proofs.

No runtime API, admission/transport policy, dependencies, lockfiles or other
repositories changed. Git source-identity children clear ambient Git redirects,
tracing/global configuration and optional index writes; only trusted PATH and
explicit noninteractive configuration are retained. Fresh commands/results and retained failure history are
recorded below; no Lens or final-performance pass is claimed.

## Fresh implementation validation

Run root:
`/home/laufan/Projects/shelllist-orchestration/20261026T202406Z/tasks/03-shared-qualification`.
All commands ran from its `worktrees/daemon-framework` through that run's
`check.sh`, private HOME/XDG, unavailable host buses/compositor and two build
workers. Logs are task artifacts, not packaged inputs. Rust/Cargo 1.95.0,
`x86_64-unknown-linux-gnu`.

| Check | Result / artifact |
| --- | --- |
| Initial targeted private qualification | Six passes before the route/timeout additions; `artifacts/initial-qualification.log`. |
| Formatting, full workspace and strict all-target Clippy | **73 passed, zero failed, one existing private proxy-cost profile ignored**; `artifacts/workspace-and-clippy-requeued.log`. All seven qualification cases and existing admission/no-replay/bridge cases included. |
| Source preflight + matching Nix workspace | Passed; **73 release correctness passes / same existing ignore**; `artifacts/snapshot-check.log`. No source override, lock update or preflight bypass. |
| First workspace attempt | Tool stopped its 1000s outer wait while queued for the shared lock; empty `artifacts/workspace-and-clippy.log`. No build/test began; retained as not-run, not a failed test or a pass. Requeued normally. |

Exact successful full gates (set `CHECK` as below):

```sh
CHECK=/home/laufan/Projects/shelllist-orchestration/20261026T202406Z/check.sh
"$CHECK" timeout 900 nix develop . --no-write-lock-file -c bash -c '
  set -e
  rustc -Vv; cargo -V
  cargo fmt --all
  cargo fmt --all -- --check
  cargo test --locked --workspace -- --test-threads=1
  cargo clippy --locked --workspace --all-targets -- -D warnings
'
"$CHECK" timeout 1800 nix develop . --no-write-lock-file -c bash -c '
  set -e
  cargo fmt --all
  tools/local-build preflight .
  env -u QUALIFICATION_SELF_TEST_REPORT tools/local-build build \
    --attr checks.x86_64-linux.workspace . --no-link --print-build-logs
'
```

Matching Nix source: `/nix/store/3bkrdcvbzay1lwpdalhqvhn6vp0a93g0-source`;
derivation: `/nix/store/3jwvwq0zbzyq2gqvrb8qh7y0dsvh5lml-daemon-framework-workspace-check-0.1.0.drv`;
result: `/nix/store/ya3bjgw7rrmlvr2a1hzksm6fsp249ji2-daemon-framework-workspace-check-0.1.0`.
The snapshot graph has only this framework root (no local sibling dependencies).
The sandbox release test ELF is named in the log but not retained/hashed by that
marker-output check; do not cite it as a retained measurement binary. Final
committed source/debug-ELF identities and any post-commit checks are in the task
handoff. Later evidence-only Markdown edits do not change the Nix Cargo fileset.
These functional gates alone do not complete Rust Lens review. The separate
resumed quality invocation and its limits are recorded in the quality section below. No new skips,
final benchmarks, services, installs, real clipboard/apps/hardware or deployment.

## Reproduce safely

Run from this family's `daemon-framework`, never the original checkout. In this
orchestration all heavy commands must use the shared helper (set `CHECK` to its
absolute `.../20261026T202406Z/check.sh` path). It supplies private HOME/config/cache/
runtime and nonexistent buses/compositor. Tests create only their own private
socket directories; fixture constructors do not start production services.

```sh
# Stage exact new input files before sibling snapshot builds (git add -N is OK).
# Optional report is outside the repository and explicitly correctness-only.
export QUALIFICATION_SELF_TEST_REPORT="$ORCHESTRATION_TASK_DIR/artifacts/self-test.json"
"$CHECK" timeout 900 nix develop . --no-write-lock-file -c bash -c '
  set -e
  cargo fmt --all -- --check
  cargo test --locked -p shelllist-daemon-tokio socket::tests -- --test-threads=1
  cargo clippy --locked --workspace --all-targets -- -D warnings
'
# Full bridge/D-Bus regression scope, after inspecting private fixture config:
"$CHECK" timeout 900 nix develop . --no-write-lock-file -c \
  cargo test --locked --workspace -- --test-threads=1
# Matching tracked source snapshot; never override inputs/bypass preflight:
"$CHECK" timeout 1200 nix develop . --no-write-lock-file -c \
  env -u QUALIFICATION_SELF_TEST_REPORT tools/local-build build \
  --attr checks.x86_64-linux.workspace . --no-link --print-build-logs
```

Do **not** export the worktree self-test report variable for a sandbox snapshot:
its source has no Git metadata. Ordinary package correctness tests do not require
Git source identity; the opt-in report fails rather than inventing it. The report
contains source HEAD/tree/diff hash and test ELF hash, exact workload/counts and
point snapshots for the combined in-process fake producer/Rust driver. It cannot
separate producer memory, measure QML or compare bridge/socket.

## Adoption by the producer/native lanes

Include the support source by path from a **test module** in your producer's
matching sibling-source fixture. For a module directly in `tests/` (adjust the
relative depth for nested modules):

```rust
#[path = "../../daemon-framework/crates/shelllist-daemon-tokio/tests/support/qualification.rs"]
mod qualification;
```

Use the framework source input that the existing sibling snapshot supplies.
All five current producer flakes' `postUnpack` copy
`daemonFramework.lib.daemonSource pkgs` to `../daemon-framework`; that source
includes this Tokio crate's `tests/support/` and its nested identity-test companion.
Thus a source-file test adapter needs **no runtime export, package edit, dependency
install or second transport**. Keep both support files in the input. Verify that
layout through each producer's matching snapshot check when adopting it; if a new
runner excludes it, request a build-input adapter rather than copying policy.
The helper uses existing `anyhow`, `serde_json`, `futures` and Tokio dependencies
plus trusted Git/coreutils identity commands. Identity self-tests create only
private Git repositories with empty templates and per-child author environment;
they neither edit Git configuration nor run against dependency worktrees.

Implement `Adapter::step` over your existing native client. Do not implement another
JSONL router/transport. Consume and validate your existing domain envelopes,
revision/epoch barriers, baselines, route IDs and owner-filtered statuses. Count
actual encoded bytes, wire requests and events. Keep effect setup fake; status
must not cause a launch, capture, paste, hardware call or retry. The framework
self-test `Peer` is a reference for fixture mechanics, **not producer protocol**.

Before `mixed`, establish two owners and test foreign cancellation/status denial
without retiring the rightful owner's work. After `mixed`, prove final owned
resources are gone, exercise a healthy read, close clients and finish/drain the
private fixture under an outer deadline. Keep existing burst/stalled-writer and
late-work admission tests; `mixed` alone does not assert those producer policies.

Exact adapter gaps at this checkpoint:

| Lane | Existing private fixture to extend | Still needed (not implemented here) |
| --- | --- | --- |
| Bluetooth | `src/daemon/tests/socket/probe.rs` / parent Rig | Adapter over existing snapshot/subscription client, completed lifecycle/status contract from BT owner, fake retained operation and foreign isolation; retained bridge path with identical workload. Do not invent absent status APIs. |
| Networking | `src/daemon/socket/tests.rs`, `src/daemon/socket/tests/probe.rs` | Existing scripted NM fixture/native client, collection renewal and independent owned `operation.status`, preserving prompts/secret authority; matching bridge adapter. |
| Applications | `tests/session/socket.rs` / session support | Existing fake launcher and routed client, catalog read/invalidation baseline, `applications.operation.status`, fake accepted-work setup and retained bridge path; no real application launch. |
| Bar | `src/daemon/socket/tests/native.rs` and battery socket tests | Extend current read-only gate deliberately: `bar.snapshot`, selected domain streams and owned `battery.operation.status` with fake battery executor; retain accepted-work guards and unrelated demand. No arbitrary effect enablement. |
| Clipboard | `src/daemon/socket/tests/native.rs`, `tests/payload.rs` | Existing read-only fake backend and real private socket client, history barriers, separately authorized fake operation setup/status and owner isolation. Current native read-only allowlist does not permit `clipboard.operation.status`; request a scoped fixture adapter, not a permissive gate. Preserve hidden-session/lease guards. |

Actual Qt/native production shared-session coverage remains in those lanes. A
Rust adapter run is not evidence about Quickshell write buffering, UI frames,
frontend generations/drafts, real daemon OS-process crashes or installed sessions.

### Required producer test-driver interface (adoption contract; not yet implemented)

Each owner supplies one **explicit-only ignored libtest case** over its existing
private fixture, selected with `--exact "$ADAPTER_TEST" --ignored`. It must refuse
debug assertions, missing inputs, unsupported payloads/transports and existing
report paths in measurement mode, rather than skip or manufacture a sample.
The fixture—not this helper—implements these test-only inputs:

| Environment input | Required interpretation |
| --- | --- |
| `QUALIFICATION_MODE=matched-release` | Check release binary/build/source tuple and the reviewed scenario before starting. |
| `QUALIFICATION_TRANSPORT=socket` or `bridge` | Select the existing native client or retained bridge over the **same** fake producer setup, no automatic fallback. |
| `QUALIFICATION_REPORT` | New absolute bounded JSON per-run report (`schema_version=1`, `classification=matched-release`, selected `transport`, scenario/build/sources/binaries, `process_tree`, workload and memory phases); only publish after validation and cleanup. The coordinator assembles the paired envelope. Retain errors separately, never overwrite/append a partial success. |
| `QUALIFICATION_WARMUP_CYCLES=50`, `QUALIFICATION_SAMPLE_CYCLES=2000`, `QUALIFICATION_OWNERS=4` | Fixed matched schedule; one step per owner outstanding. |
| `QUALIFICATION_PAYLOAD_BYTES=256` or `65536` | Producer-valid deterministic fixture payload, with generator and actual encoded byte counts. Unsupported scenario fails. |

Use production per-step/admission/queue settings and `total_deadline=180s`; outer
fixture startup/memory/cleanup must fit the 240s process deadline below. Snapshot
source identities before timed work. Reports name any narrower producer-valid
payload alternative; those form separate scenarios, not an unnoticed substitution.
A fixture may reuse a domain-owned operation status after fake acceptance; it may
not dispatch that effect from `Status`, infer success from absence or answer a
prompt. Existing read-only native allowlists remain intact unless the producer
owner adds an explicit fake-status fixture. These env names are an adapter
acceptance contract, **not newly implemented producer CLI/runtime APIs**.

Each report must enumerate a fixture-owned process tree from its spawn ledger:
PID, parent PID/role, start ticks, executable path/hash and included/excluded
reason. Include all retained bridge/wrapper targets, frontend and engine/helper
children that actually exist. Keep live owned children unreaped while sampling;
refuse if the ledger is incomplete or identities change. No host `/proc` scan or
process-group guessing. `memory_snapshot` consumes an explicit PID list and
cannot certify that list's completeness. The framework self-test only includes
its own combined service/driver PID, so it is not such a tree comparison.

## Queued final serial measurement plan — NOT RUN

Coordinator prerequisites: reviewed adapters and all correctness gates passed;
matching producer/bridge/frontend release builds from clean committed sibling
inputs; no concurrent worker builds/tests/benchmarks for the **entire** reserved
window. Holding the shared check lock is necessary but not sufficient to establish
host idleness. No host desktop/service test is authorized by this plan.

1. Through `CHECK`, preflight and build matching sibling-source release artifacts.
   Record exact commands/toolchain/target/profile, snapshot/package identities,
   `source_identity(root, true)` for every sibling input, `binary_identity` for
   every actual executable and hashes for any external fixture/QML inputs. Keep
   bridge and producer binaries from the same build. No original target reuse.
2. Use the reviewed producer fake fixture with private HOME/config/cache/runtime,
   nonexistent frontend buses/compositor, and a no-activation private bus only
   where the retained bridge needs it. Hold/reap all child PIDs and bound logs,
   cleanup, workload and whole-run time. Record included/excluded processes.
3. Freeze scenario v1: **4 owners, 50 warmup + 2,000 sample cycles per owner**,
   one outstanding step/owner, **256-byte ASCII `x` payload**; a separate scenario
   uses **65,536 bytes**, if valid for that producer. Do not pad domain methods with
   unknown parameters to force an invalid payload; define producer-valid seeded
   response/request fixtures and record both logical and encoded sizes instead.
   Each owner yields 4,000 read and 2,000 samples of each other step per run.
   Use production admission/output/deadline settings; explicit bounded fixture
   run deadline (e.g. 180 seconds), abort/report rather than omit failed samples.
4. Reserve **three ABBA blocks**, A=socket, B=retained bridge, recreating equivalent
   private fixtures for each run. This gives six runs per path per payload. Preserve
   per-owner/step/run samples and exact traffic counts; no averaged p99. Record
   path order and any failed run before deciding on another complete block.
5. In separate fixture-controlled memory phases hold the same fake streams/work:
   connected idle, post-identical-warmup held state, post-cancel/quiescence. Take
   ten point snapshots per phase at 100 ms intervals for both paths using explicit
   stable fixture-owned PID lists. Keep driver, producer, bridge and frontend
   contributions separate and report summed RSS/PSS/private only for equal scope.
   The sampler is outside latency timers. This is not a peak-memory measurement.
6. Repeat the producer's burst/overflow/stalled-peer isolation and accepted-work
   cleanup gates separately, including single/multi-worker coverage as available.
   Do not mix deliberately short correctness deadlines into performance runs.
7. Assemble the contract envelope with all runs, limitations and comparison
   rationale. Coordinator reviews evidence; no automatic approval/default change,
   activation, rollout, or whole-shell extrapolation follows.

### Exact queued command recipe

**Do not execute until all five producer adapter cases above exist and are
reviewed, the coordinator reserves the whole serial window, and every other
worker/build is stopped.** No such producer case or performance result is supplied
by this framework-only checkpoint. The variables below are mandatory reviewed
inputs, not permission to choose an installed/original-checkout binary.

From each frozen producer family checkout, first build/capture (outside the
measurement window is fine; new files must be staged before capture):

```sh
CHECK=/home/laufan/Projects/shelllist-orchestration/20261026T202406Z/check.sh
"$CHECK" timeout 1800 nix develop . --no-write-lock-file -c bash -c '
  set -e
  ../daemon-framework/tools/local-build preflight .
  ../daemon-framework/tools/local-build develop . --command \
    cargo test --release --locked --lib --no-run --message-format=json
'
```

Retain the exact compiler-artifact JSON/test ELF, sibling snapshot/store source
map, toolchain/target/profile and bridge/producer build commands. The domain owner
must supply any required compile feature flags and record the resulting **exact**
command (never reuse a different-feature ELF). The driver/bridge/producer/frontend
identities must match the frozen source tuple; capture all sibling roots with
`source_identity(root, true)` and actual executable hashes with `binary_identity`.
Cargo artifact discovery in the captured graph is the authority for `DRIVER`,
not a guessed `target/release` filename. Each producer report's spawn ledger seals
the full process-tree identities described above.

After explicit coordinator reservation, repeat for each producer/scenario; `OUT`
is a new private task artifact directory, `DRIVER` the reviewed captured release
ELF and `ADAPTER_TEST` the owner's exact libtest case name:

```sh
: "${CHECK:?}" "${OUT:?}" "${DRIVER:?}" "${ADAPTER_TEST:?}"
for payload in 256 65536; do
  for block in 1 2 3; do
    slot=0
    for transport in socket bridge bridge socket; do
      slot=$((slot + 1))
      stem="$OUT/p${payload}-b${block}-s${slot}-${transport}"
      # One lock covers the complete ABBA window externally: coordinator must
      # prevent interleaved checks between these individual lock acquisitions.
      QUALIFICATION_MODE=matched-release \
      QUALIFICATION_TRANSPORT="$transport" \
      QUALIFICATION_REPORT="$stem.json" \
      QUALIFICATION_WARMUP_CYCLES=50 QUALIFICATION_SAMPLE_CYCLES=2000 \
      QUALIFICATION_OWNERS=4 QUALIFICATION_PAYLOAD_BYTES="$payload" \
        "$CHECK" timeout 240 "$DRIVER" --exact "$ADAPTER_TEST" \
        --ignored --test-threads=1 > "$stem.log" 2>&1 || exit "$?"
      test -s "$stem.json" || exit 1
      test "$(wc -c < "$stem.log")" -le 1048576 || exit 1
      grep -q 'test result: ok\. 1 passed; 0 failed;' "$stem.log" || exit 1
      python3 -I - "$stem.json" "$transport" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
assert 0 < path.stat().st_size <= 1048576
report = json.loads(path.read_bytes())
assert report["schema_version"] == 1
assert report["classification"] == "matched-release"
assert report["transport"] == sys.argv[2]
assert report["process_tree"]
for key in ("scenario", "build", "sources", "binaries", "workload", "memory_samples"):
    assert report[key], key
PY
    done
  done
done
```

Run this snippet with `set -eu` and an outer coordinator deadline of 7200s per
producer (24 runs, each at most 240s); paths are new and logs are capped by the
reviewed fixture (metadata only, at most 1 MiB/run; kill/reap all private children
on failure). The snippet rejects zero filtered tests and absent/oversized/basic malformed
reports. Producer review must additionally validate domain results, complete
process scope and matched identities; these shallow checks cannot certify them.
A missing adapter/report, failure or unavailable metric stops the block and is
retained—not a successful missing/zero result. The matching environment in each
`CHECK` is private; a private bus with no host activation is created only by the
reviewed fixture when necessary. Actual frontend binaries, if included, must use
the pinned offscreen/software setup and fake endpoints. Memory phases (ten samples
at 100ms each) are outside latency timers. Do not extend a deadline or pad an
invalid domain method to make this recipe pass.

## W1-FW separate Rust quality review — 2026-10-26

This closes the inherited **missing extraction/refactor loop for the bounded
framework checkpoint**, not producer adoption, qualification or integration.
Inventory covered the full `563e7f9` → implementation `e029103` diff: all seven
files, including inherited README/socket fixture and nested support/identity tests.
No generated companions or repository Python/shell/Nix code changed.

Pinned RQLens `d28f3d936fa3c0d4c0a7ea753f436517077cb37d`, CLI SHA256
`622b2c793a574781cd40a190a457f58af4b8c222ee5efce04c4e77488c50929a`.
The static-only task config (`quality/rqlens.toml`, SHA256
`817410bd2e5cb1b8a52212608ce84dff874642e24044bce3cab427a2aeae9557`)
measures Tokio `src` **and `tests`**, including the nested changed files.
It points `helper_manifest` at that pinned source's `rust_helpers/Cargo.toml`;
metadata is offline, semantic/macro execution disabled. No coverage, audit,
`measure all`, verify, correctness-run, runtime or performance producer ran.

Normal analyzer-managed **locked helper compilation** was performed in this task's
private cache, as explicitly permitted. Helper source digest `6a098384d350404d`;
`rust_facts` SHA256 `302635f92d79b17835cd9009002b4f511d526e0cb1c6c0d580f3e068a88686ec`;
`ast_hasher` SHA256 `751582c8765d206635cbaa6261cc352e8b72711a84e61671447970f298b34adf`.
Pinned sources/manifests/locks remained unchanged; the main CLI was not rebuilt.

Actual `hotspots`, `clones`, `reliability` and `test-quality` BEFORE/AFTER envelopes
are retained under the task's `quality/before/`, `quality/after/` and final clean-head
`quality/final-after/`. `quality/inventory-before.json`, `TRIAGE.md`, `comparison.json`
and logs retain exact source/config/tool identities and introduced/retained review.
BEFORE is clean implementation commit/tree `e029103` /
`01645c7f972520d4b5fedb7d0ef9970f829dc841`; the first AFTER is the tracked refactor
before its quality commit. Final clean quality-head identities belong to the task
handoff, avoiding a self-referential tracked commit hash.

The actual changed hotspot justified simplifying `identity_command`: its reader
sends one bounded completion, so receive it once, then wait for child exit under
the **same** deadline. Removed optional-output polling state; no helper splitting,
new policy layer, threshold/exclusion changes or weaker cleanup. The existing
builtin-only blocked-input/reaping test now also closes stdout while leaving the
child alive: EOF must not become success, extend the deadline or block input-thread
cleanup. All existing failure/ownership/traffic/percentile tests remain.

| Same-config observation | BEFORE → AFTER |
| --- | --- |
| `identity_command` SLOC | 55 → 48 |
| Cyclomatic / cognitive complexity | 14 / 17 → 10 / 5 |
| Advisory hotspot score / Halstead effort | 97.05 / 75020.27 → 53.76 / 50925.30 |
| Clone groups / duplicated lines | 10 / 111 → 10 / 111 |
| Duplication percent | 1.94% → 1.95% (source denominator 5708 → 5699, not extra cloned lines) |
| Reliability findings | 190 → 190, all test-scope advisory |
| Test-quality inventory / findings | 47 / 1 → 47 / 1; same ignored private profile, zero assertionless |

Retained findings are not suppressed: 94 reliability advisories touch changed
files, 49 on original-baseline added/modified lines (45 unwrap, three expect, one
panic) and 45 on unchanged lines in the existing socket fixture. They are explicit
fake-fixture invariants/failure checks, not production fallback success. The tiny
new directory-constructor token match must stay standalone for producer by-path
inclusion; do not couple it to socket fixtures. The seed's workload-config match
keeps materially different deadlines/scenarios explicit. Other clone groups are
unchanged fixture setup/error formatting. Keep distinct overflow, admission,
writer-failure and owner-isolation protections. Larger schedule/memory-validation
functions retain meaningful guards rather than being split to game metrics.

Affected gates passed through the same shared helper/private environment:
formatting; **73 workspace passes / one existing private proxy-cost profile ignored**;
strict all-target Clippy; source preflight and matching Nix workspace with **73
release correctness passes / same ignore**. `quality/after-and-checks.log` retains
results/source graph; the handoff retains the exact combined command. Matching source is
`/nix/store/ykkvdszjqg0aysidmsylpx7kgn0h3pvz-source`, derivation
`/nix/store/12wcbg0qsgr6ymbglkb9mmlxgikfqywx-daemon-framework-workspace-check-0.1.0.drv`,
result `/nix/store/vpxl4dlaxsz93cg7cfg20g1avpzqdpjn-daemon-framework-workspace-check-0.1.0`.
As before, this marker check does not retain/hash the sandbox release test ELF.
The handoff records the final clean-head AFTER and debug ELF/native rerun plus
language-specific syntax checks for the task driver/document recipes.

All four envelopes report complete **authored syntax** extraction (28 source files;
31 for automatic test discovery). This is not compiler-expanded cfg/macro coverage,
resolved semantic types, architecture completeness or proof of test value. No
ownership/fault/coverage campaign is configured or claimed. Raw uncertainty and
advisories remain review inputs; no universal quality-green or rollout approval.
All five real producer adapters/status/bridge/process-ledger gates, sustained
multi-worker/heap qualification and the reserved serial recipe above remain open.
No final benchmarks, live effects, runtime API, package/lock or other-domain edits.
