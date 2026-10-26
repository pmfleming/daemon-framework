# Private mixed-load and matched-release evidence, v1

This is a **test/evidence contract**, not a runtime API or rollout gate waiver.
The bridge remains default. D-Bus and legacy clients stay supported. No live
services, effects, credentials, clipboard, hardware or host buses are permitted.

## Reuse boundary

`crates/shelllist-daemon-tokio/tests/support/qualification.rs` is test-only source
included by path. It has no transport implementation, producer method registry,
recovery policy or effect retry. Producers implement `Adapter` using their existing
private native client and fake backend. Do not copy this into daemon runtime code,
fork shared policy or substitute synthetic timings for a missing native adapter.

Each adapter owns one connection and validates the complete response, correlation,
owner, domain result and required baseline/events before returning. `Status` is a
read-only owned outcome/status query; it must never dispatch an effect. A fixture
may establish accepted fake work before timing, but keeps its admission until work
actually finishes. Owner cleanup means cancellation, not rollback. Missing outcome
evidence is unknown, not success. Secret authority, hidden acknowledgements,
confirmation/edit leases and private drafts must not be bypassed by an adapter.

## Deterministic workload

One independent adapter per owner runs concurrently, with **one step outstanding
per owner**. This is closed-loop mixed traffic, not an open-loop arrival-rate or
maximum outstanding-request benchmark. The fixed schedule is:

```
subscribe
for each warmup cycle, then each sample cycle:
    read, status, renew, read, cancel, subscribe
cancel
```

Renew cancels only that adapter's current subscription, subscribes again and
validates its new baseline. Cancel and subscribe validate domain admission and
results. Establishment/final cancellation and warmup are not latency samples, but
are included in total traffic. Payload bytes describe the fixture payload before
JSON encoding; actual encoded request/response bytes include LF. Report the
payload generator/encoding separately (e.g. 256 ASCII `x`, not real user data).

For `C = warmup_cycles + sample_cycles`, a simple one-request status/read and
one-baseline subscription adapter issues `7*C + 2` requests and receives `2*C + 1`
events per owner. This is an adapter-specific assertion, not a protocol promise.
Composite domain adapters report their actual traffic instead. With `S` measured
cycles, latency counts per owner are `read=2*S`, and `status=renew=cancel=subscribe=S`.

Bounds: 1–64 owners, at most 10,000 total cycles, at most 1,000,000 scheduled steps
across owners, 0–16 MiB logical payload, positive per-step and total deadlines
of at most 1,800 seconds each.
Limits are validated before allocating sample storage or dispatching. Helpers
never reconnect/retry. Deadline or validation failure yields **no completed run**;
the fixture closes its owned clients, cancels/drains fake work and stops private
children. Dropping the workload future must not be interpreted as aborting an
already accepted effect. There is no success report with omitted failed samples.

## Latency and memory output

`mixed` returns schema version 1, schedule, owner count (the `owners` array), logical
payload bytes, warmup/sample counts, deadlines (exact nanoseconds and truncated
millisecond convenience fields), in-flight bound, and per-owner:

- `latency.<step>.{n,p50_ns,p99_ns,min_ns,max_ns}`;
- `traffic_including_setup_warmup_cleanup.{requests,events,request_bytes,response_bytes}`.

Timings use a monotonic clock around the complete adapter step, including client
encoding, validated decoding/events and scheduling. Renewal is a **composite**
latency; it is not a single RPC latency. Percentiles use nearest rank:
`sorted[ceil(p*n)-1]`, with no interpolation. Keep owners/steps/runs separate; do
not average percentiles or compare differently weighted mixtures.

`memory_snapshot` takes an explicit list of live, unreaped **fixture-owned PIDs**;
never discover/read host processes. It emits each PID, executable path, Linux start
ticks and byte-valued `Rss`, `Pss`, `Private_Clean`, `Private_Dirty`, optional
`Private_Hugetlb` and `Private_Total`, plus sums across exactly that list.
`Private_Total = Private_Clean + Private_Dirty + Private_Hugetlb` (absent hugetlb=0).
Missing required fields, malformed/trailing units, unreadable/disappeared
processes, duplicate PIDs or a changed PID/start/executable identity fail; they
never become zero memory. An unavailable memory sample is a failed sample; retain
the failure separately rather than emitting a completed measurement with zeros. This is a point sample,
**not peak memory or a proof of a total heap bound**. Summed RSS double-counts shared
pages; PSS depends on concurrent sharers. Retain per-process values.

Name every sample phase and included/excluded producer, driver, bridge, frontend
and child process. Take both paths at identical phases and repeat. A suitable
separate memory phase is: connected idle; same fake work/streams established and
held after identical warmup; post-cancel/quiescence. This needs fixture-controlled
barriers outside `mixed`; the helper does not guess process scope or sampling
phases. Never silently compare an in-process producer/driver against a separate
producer, or direct client alone against bridge-plus-client.

## Matched-release envelope (required for a performance claim)

Store a JSON envelope containing:

```
{
  "schema_version": 1,
  "classification": "matched-release",
  "scenario": { "id": "producer-owned-version", "adapter": "...",
                "methods_streams_and_status_setup": "...", "payload_generator": "...",
                "queue_admission_and_deadline_configuration": { } },
  "build": { "commands": ["... --release --locked ..."], "toolchain": "...",
             "target": "...", "profile": "release", "debug_assertions": false },
  "sources": { "each_sibling_input": "source_identity result" },
  "binaries": { "producer_driver_bridge_frontend_as_applicable": "binary_identity result" },
  "environment": { "kernel_cpu": "...", "serial_exclusive_window": "...",
                   "path_order_and_repetition": "..." },
  "socket": { "workload": "mixed result", "memory_samples": [] },
  "bridge": { "workload": "mixed result", "memory_samples": [] },
  "limitations": ["private fake backend; not whole-shell/installed-session acceptance"]
}
```

The quoted placeholders above are schema descriptions, not valid measurement
values. The envelope is assembled/reviewed by the producer fixture; this helper
**does not certify** release profile, matched adapters, child completeness or
exclusive host access. `source_identity(root, true)` requires clean committed
inputs and records canonical path, HEAD, tree, dirty flag and SHA256 of the binary
Git diff. Capture all source siblings, generated frontend inputs and lockfiles
through the matching sibling-source build. For correctness-only worktree runs,
`false` permits tracked edits and records their diff hash; untracked inputs always
fail. Stage new files before source capture. Ignored files are not source identity.

`binary_identity` records canonical executable path and actual SHA256. Record every
loaded producer/test-driver/bridge/frontend executable (including wrapped target
binaries, not merely launch scripts). Record fixture/QML file hashes when not
already covered by the source tree. Keep exact build/test commands and package or
snapshot identities. Never select an original checkout's stale target binary.
Source capture requires Git and the canonical repository root (subdirectories
cannot identify only part of a HEAD tree). It compares HEAD, status and the full
tracked binary diff again before returning; callers must still freeze inputs.
Concurrent edit-and-restore/exec-and-restore is not an atomic snapshot guarantee.
Packaged correctness tests need not have `.git`.

Identity commands are trusted Git/coreutils only: one direct child, two bounded
pipe threads, a **10-second per-command deadline**, **16 MiB maximum input/output**,
no inherited stdin and discarded stderr. Exceeding a bound, a nonzero exit or a
pipe error fails identity capture; the direct child is killed/reaped before pipe
threads join. Git pager, fsmonitor, external diff and textconv helpers are disabled.
This is not a general process-tree launcher: do not pass shell scripts, producer
binaries or descendant-spawning commands to it. Fixtures must separately own and
reap all their producer/bridge/frontend children under an outer deadline. Git and
coreutils must come from the matching trusted development/package environment,
not a fixture-writable PATH. A source diff larger than the cap is **unavailable
identity**, not a truncated hash or permission to raise the limit silently.

Socket and retained bridge must use the **same producer build**, logical workload,
payloads, owners, status fixtures, samples, admission and production deadlines.
Only transport selection changes. Backend setup is private and deterministic.
Run multiple ABBA blocks in a coordinator-reserved serial window; record every
run, including failures. Do not tune production deadlines to make a comparison
pass. Short-deadline backpressure tests are correctness evidence only. No latency,
memory saving, daemon-wide ceiling, startup or default-change claim follows from
a debug/self-test run. See [audit, adoption and queued recipe](../tests/private-qualification.md).
