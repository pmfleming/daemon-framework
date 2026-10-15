# Test-suite reduction — 2026-10-15

Baseline: clean `373b42f`. Inventory includes both Cargo and Python unittest
tests, not just the Rust workspace. Starting count: **115** (91 Rust + 24 Python).
The whole-test target is `round(115 × 0.67) = 77`.

Final: **77 enabled tests** (61 Rust + 16 Python), **38 fewer / 33.04% removed**,
retaining 66.96% of the baseline. Each language also meets its independently
rounded 67% target. There are no new ignored/skipped tests, cfg gates, runner
filters or discovery changes. Doctests enumerate zero tests in both inventories.
Counts refer to discovered test entrypoints, not assertions or parameter rows.

| Suite | Before | After |
| --- | ---: | ---: |
| daemon-core library | 15 | 9 |
| Protocol-JavaScript generator | 2 | 1 |
| daemon-tokio | 38 | 27 |
| Hyprland | 23 | 14 |
| local-build unit tests | 10 | 7 |
| local-build CLI integration | 3 | 3 |
| **Rust total** | **91** | **61** |
| Python cache-maintenance tests | 24 | 16 |
| **Codebase total** | **115** | **77** |

This is not exclusively a count consolidation: over 300 Rust/Python test lines
were removed. Related contract cases share fixtures and tables, while redundant
assertions and low-value scenarios are actually deleted. All runtime code,
manifests and dependencies are unchanged.

## Removal and preservation map

- **Core wire/schema contracts:** consolidate canonical event identity,
  anti-spoofing, optional errors and exact large gap counters into one envelope
  contract. Default request fields versus required serialized result fields now
  accompany the method declaration/dispatch test. Duplicate-name rejection and
  valid names remain checked there; remove the thin fixture-extraction smoke test.
  The generator checks decoded contract JSON rather than object-key ordering;
  supported schemas, missing optional contracts and rejection of unsupported
  assertions/external references remain covered together.
- **Persistence and retained results:** remove the extra ancestor-sync matrix and
  JSON round-trip smoke, which largely repeated the atomic writer's lifecycle.
  Retain commit/abort/failed replacement, temporary cleanup, private file and
  directory permissions, symlink refusal and bounded/nonregular-file reads.
  Replace the separate retention-state/occupancy test with public owned lookups
  for partial TTL expiry, live-suffix retention, replacement and disabled TTL.
  Only a minimal timestamp fixture remains to avoid sleeps; private storage
  counts and mutation of the retention policy are no longer requirements.
- **Bridge deadlines/cancellation:** replace the private connection-mutex cleanup
  test and cancellation-predicate microtest with real-bus cancellation responses
  and a deliberately stalled Cancel reply in the call/deadline scenario.
  Explicit domain refusal retains ownership; success retires it; stalled cleanup
  is bounded and not replayed. Remove the old missing-`ok` compatibility-only
  assertion. Readiness rejection and late-subscription expiry share the existing
  real bus. Worker cleanup is observed through a lifetime channel instead of
  fixed sleeping and an exact `Arc` count. Slow applied calls still have unknown
  outcomes without failing healthy calls or being replayed.
- **Admission and lifecycle:** remove repeated direct-unit saturation setup in
  favor of direct-bus ingress and declared-policy tests. Keep owner requirements,
  request/result size limits, undeclared-work rejection, per-owner/method quotas,
  effect admission after timeout/caller loss, read expiry, panicking handlers,
  independent cleanup lanes and permit release. Probe public admission to verify
  restored capacity rather than requiring accounting maps to be empty. Remove
  the old-interface-only check. Combine invalid bridge limits with overload
  rejection, retaining addressed rejection, no spawn/dispatch and control bounds.
- **Worker/transport mechanics:** remove the synchronous blocking wrapper smoke;
  async admission/reply/panic behavior and control-lane isolation remain. Running
  job timeout and queued-reply closure now share the lane lifecycle fixture.
  Task shutdown also checks failed joins and explicit panic conversion. Registry
  completion/panic release, destruction and shutdown are exercised through public
  admission and lifetime channels, replacing fixed-yield/cardinality/refcount
  checks; the reused-ID cleanup race remains separate. Real-bus owner independence
  and bus failure share established monitors. Directed-delivery privacy, owner
  lag/stoppage, resume deduplication and consumer closure remain unchanged.
- **Framing:** drop the million-byte replay of the same bounded-drain path.
  The boundary/chunk-size matrix now includes an oversized shutdown-shaped suffix
  followed by a valid record, alongside byte/UTF-8/CRLF/EOF and allocation bounds.
  Cancellation-safe partial reads and discard-state retention remain separate.
- **Hyprland IPC:** eliminate duplicate invalid-signature/exact-target fixtures
  and private exact-discovery setup, retaining rejection through public calls,
  no wrong-instance mutation, two-root preferred candidates and stale-socket
  fallback. Filesystem ownership/symlink checks also cover peer credentials.
  Reply size, UTF-8 and deadlines share the reply-contract test; connection,
  reconnection, EOF backoff and initial-failure suppression share the event
  lifecycle test. Event limits, receiver closure and no replay after connection
  remain. Remove the batch-request wrapper smoke and redundant Serde parsing
  checks, retaining required batch data, trailing-document rejection, invalid
  geometry, normalized insets, gap precedence/fallback and geometry events.
- **Hyprland rule interpretation:** combine related selector and command rejection
  matrices; remove repeated connector-name and invalid-flag permutations, unknown
  mode acceptance goldens, and a scientific-notation rejection requirement that
  needlessly constrained future safe encoding. Keep Lua-injection rejection,
  numeric/transform bounds, logical geometry, named/special workspaces, directional
  ties/disabled monitors, fullscreen, visibility and group semantics. Named-prefix
  and exact-name selectors are now checked independently, avoiding one rule
  masking another in the old fixture.
- **Local build:** fold URL-pin checks and daemon crate/vendor matrices into the
  source-policy fixture. Root and captured-source standalone inputs, divergent
  follows, remote pins, URL encoding and current sibling dependencies remain
  checked. Shared-source snapshot success and failed locking now share one graph
  and verify the original live lock in both cases. Keep preflight aggregation,
  distinct overlays, cycles/collisions, dirty/deleted/ignored files, metadata,
  symlink confinement, lock follows/reachability and all CLI integration tests.
- **Python maintenance safety:** consolidate same-contract no-op/dry-run checks,
  release/cross-compilation profile retention, source-marker guards, literal-path
  and hardlink accounting, unsafe-target variants, symlink levels and target
  selection. Preserve active-build locking, eviction order, stopping at the size
  bound, unchanged lock inodes, tracked/source/review preservation, disabled mode,
  CLI validation, Git hook/fsmonitor disabling and execution from another cwd.

## Validation and evidence

- Full Cargo suite: **61 passed, zero failed/ignored**; doctests pass.
- Python unittest discovery: **16 passed, zero failed/skipped**.
- Formatting, workspace/all-target Clippy with `-D warnings`, rustdoc with
  `-D warnings`, and `git diff --check` pass on Rust/Cargo 1.95.
- Automated comparison confirms unchanged runtime prefixes in mixed Rust
  production/test files; all other edited source files contain tests only.
- **9/9 selected deliberate faults detected** in a disposable copy: accepting a
  refused cancellation, corrupt event identity, accepting truncated frames,
  wrong-instance mutation fallback, premature effect-permit release, snapshot
  symlink escape, broken owner isolation, discarding the live cache suffix and
  destructive dry-run cleanup. Each Rust probe failed an executed assertion,
  not compilation or discovery. The healthy copied suite passed first.

Inventories, baseline source archive, logs, fault-probe script/results and
restored disposable probe tree are in `target/test-reduction-2026-10-15/`.
The live production sources were never fault-injected. These selected probes
support specific critical contracts; they are not an exhaustive mutation score
or proof of unchanged numerical coverage. `cargo-llvm-cov` is unavailable, so
coverage percentages were not measured. Consumer builds, deployment/Nix checks,
live compositor behavior and MSRV compatibility were not validated.

Reproduce the entrypoint counts with:

```sh
cargo test --workspace --locked -- --list > /tmp/framework-tests.txt
grep -c ': test$' /tmp/framework-tests.txt # 61
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py' -v # 16
```

The target applies to this pass, not a permanent ceiling on new essential
regression tests.
