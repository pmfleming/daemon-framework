# Daemon family refactor (2026-10-04)

This is a bounded refactor pass, not a claim that every hotspot or architecture
risk is resolved. All six daemon repositories and the framework began clean.
Changes are local commits; nothing was deployed or pushed.

## Changes and boundaries

- **Framework/client reuse:** four clients now provide borrowed operation-ID
  selection to `CorrelationPolicy`; one implementation handles subscription
  fallback, precedence and allocation. App's accepted-only rule, Bluetooth's
  operation families, Clipboard's paths, NM's result envelope, event correlation
  and terminal names remain daemon policy. Bar retains `BasicCorrelation`.
- **Hyprland:** separate window filtering, group deduplication and fullscreen
  selection; sort borrowed group members rather than cloning their strings.
  Parse selector clauses and inclusive ranges directly. Preserve rule order,
  malformed-selector rejection and the existing permissive unknown-fullscreen
  behavior. Tests cover group ordering, conflicting flags and fullscreen scope.
- **App operations:** `OwnedOperations::drain` removes the temporary cloned
  ID/owner vector. One local cancellation routine retains terminal results.
  Registration takes its key from the result, removing the redundant
  `ActiveOperation` transfer type and the possibility of mismatched IDs.
- **Bar:** reuse framework broadcast forwarding; share the display sleep-epoch
  guard across docking, layout and focus. Read small projections rather than
  cloning unrelated snapshot domains. Flatten rollback handling, borrow native
  notification engines/action strings, and remove unused service/backend Clone
  implementations.
- **Lock boundary:** an intermediate subscription projection was withdrawn.
  Caller-controlled stream serialization must not extend the snapshot lock's
  critical section. The initial snapshot clone and atomic subscribe ordering
  are retained; rendering happens after releasing the lock. Duplicate stream
  behavior, lag notifications and work-area interest lifetime are preserved.
- **Dead API removal:** repository-wide Rust call-site searches found no consumer
  of registry `cancel_all`/`cancel_owner` or the one-shot owner-watch wrappers.
  Remove these obsolete alternatives, not unrelated public APIs simply lacking
  local tests. This is a source API removal; see the changelog and
  [migration guidance](server-infrastructure.md). All current consumers build.
- **Update worker:** reviewed and validated, but left unchanged. Its privileged
  process-group cleanup, environment policy and recovery transactions are not
  interchangeable with session-daemon task lifetimes. Adding a framework
  dependency without a second concrete consumer would increase coupling.

No wire versions, dependency pins, lint gates or waiver budgets changed.

## Provenance and measurement limits

Use **only** `target/family-review-frozen-before/` and
`target/family-review-frozen-after/` in each repository. Earlier
`family-review-before/` and `family-review-after/` captures are superseded:
RQLens helper inputs changed during the session, making those captures unsuitable
for before/after comparisons. The RQLens worktree was not edited by this pass.

Both sides were remeasured with a copied executable and a standalone snapshot of
its local helper sources, manifest and lockfile. Original daemon revisions were
measured in detached sibling worktrees, preserving their framework dependencies;
those temporary worktrees were removed afterwards. All 70 final artifacts share
helper digest `6f2a103a25ee74b6`, with stable source fingerprints within each batch.

- Executable: RQLens 0.1.0, SHA-256
  `7860a478776b2f8c6ac15b02fb445da1eb6b46239aa84eb03de4e2d6605b7ef8`.
- Frozen executable/helpers/configs: `target/family-review-instrument.tar.gz`.
- Summaries, measurement script and validation logs:
  `target/family-review-evidence/`.
- Producers: `hotspots`, `clones`, `escape-hatches`, `locality`, `leverage`.
- Rust/Cargo: 1.95; repository measurement policies retained, with explicit
  project/output roots and the same frozen helper override on both sides.

These are comparative **static observations**, not a complete quality gate.
Session-daemon evidence remains partial: macros, block-local items and some
semantic identities are unresolved; framework dependency fingerprinting also
reports incompleteness. In particular, this instrument does not list default
trait method bodies as hotspot function records. Moving correlation into trait
defaults changes that inventory. Do not interpret aggregate function totals as
proof of whole-program cyclomatic reduction. The concrete hotspot comparisons
below do not involve those default methods. Update-worker artifacts are complete.
There is no calibrated effort metric in these outputs, and no runtime benchmark
or hardware-performance improvement is claimed.

## Results

Function totals include tests and all functions inventoried by RQLens. Physical
Rust LOC includes all Git-tracked `.rs` files, including tests and tracked vendor
sources (which were not changed). Clone calls mean literal `.clone()` sites, not
allocation counts. RQLens duplicated lines measure source similarity, not cloning.

| Repository | Code revisions, before → after | Reported cyclomatic sum | Reported cognitive sum | Rust LOC delta |
|---|---|---:|---:|---:|
| framework | `bb1f9a6` → `b3e4361` | 781 → 777 | 318 → 297 | +30 |
| app | `5ca88f0` → `b38aadc` | 1422 → 1424 | 555 → 556 | -27 |
| bar | `61b8151` → `c582866` | 3563 → 3561 | 1742 → 1737 | -1 |
| Bluetooth | `9628f76` → `1e84c54` | 2025 → 2025 | 787 → 787 | -14 |
| Clipboard | `3e2f319` → `dfd1e6e` | 1714 → 1714 | 556 → 556 | -17 |
| NetworkManager | `cf09351` → `5687a30` | 4707 → 4709 | 1433 → 1433 | +3 |
| update worker | `918127d` unchanged | 883 → 883 | 273 → 273 | 0 |

Framework baseline `bb1f9a6` has the same Rust sources as initial `c13f27d`;
it only adds the review plan. New/extended regression coverage contributes to
App/NM total increases. Framework duplicated lines increase 124 → 132, while
Bar decreases 1373 → 1328; not every individual metric improves.

| Concrete production hotspot | Cyclomatic | Cognitive |
|---|---:|---:|
| framework workspace `term` | 17 → 13 | 15 → 5 |
| framework `window_count` | 15 → 6 | 15 → 6 |
| framework selector `matches` | 8 → 7 | 9 → 7 |
| framework `insets` | 8 → 8 | 14 → 11 |
| bar display `reconcile` | 18 → 17 | 13 → 12 |
| bar focus `change` | 17 → 17 | 13 → 9 |
| bar subscription `forward_events` | 9 → 7 | 8 → 7 |

Framework inventoried maxima fall **17 → 13 cyclomatic**, **15 → 12 cognitive**.
Across the family:

- Physical Rust lines: **103,282 → 103,256 (-26)**, including new regression tests.
- Literal clone sites: **1,054 → 1,052**. Additionally, whole-snapshot cloning at
  display-policy reads and an Arc clone on each native-notification lookup are
  avoided; the literal-site metric does not quantify those savings.
- Reported duplicated lines: **4,003 → 3,966**.
- Escape-hatch findings: **37 → 33**, specifically four wildcard test imports;
  this is not a claim of eliminating unsafe code or dependency escape hatches.
- Per-repository mean locality/leverage scores are **unchanged**. The concrete
  reuse gain is four consumers of common response correlation plus Bar's use of
  existing forwarding machinery. Domain decisions stay beside their state;
  no numerical architecture-score improvement is claimed.

## Validation and remaining work

Formatting, locked Cargo tests, and Clippy with `-D warnings` passed for every
repository (framework uses `--workspace --all-targets` for Clippy; daemon Clippy
uses `--all-targets`). These are executed command results, not a full RQLens
`verify` run or a coverage/mutation/security-audit claim.

| Repository | Passed tests | Ignored |
|---|---:|---:|
| framework | 54 | 1 |
| app | 49 | 3 |
| bar | 141 | 2 |
| Bluetooth | 54 | 0 |
| Clipboard | 47 | 0 |
| NetworkManager | 129 | 3 |
| update worker | 53 | 1 |
| **Total** | **527** | **10** |

Build environments came from the approved current-source `tools/local-build.py`
development shells. Clipboard used its existing package-scoped bootstrap setting
for its dependencies; no new escape hatch was added. No service activation or
live hardware exercise was performed. Advertised older MSRVs were not certified.

Follow-up opportunities remain in Bar's display monitor/lid policy, NM's workflow
and dispatch hotspots, and the update worker's effect orchestration. Preserve
cancellation, rollback and security boundaries rather than replacing distinct
policies with a universal daemon abstraction. Extend RQLens's default-trait-body
inventory before making whole-source complexity claims, and use a portfolio-wide
architecture analysis if numerical cross-repository leverage is required.
