# Time-loop decomposition (#126)

## Pre-move inventory

Recorded against `17fb779` before code movement. The original
`src/simulation/timeloop.rs` contains 2,572 lines (102,324 bytes with LF newlines): configuration/validation, forcing shapes and caches, reports,
environment switches/profiling, errors, forward resource ownership and lifecycle,
meteorology prefetch/upload, forward timestep sequencing, GPU operator encoding,
particle synchronization/sorting/compaction bookkeeping, gridding, backward
receptor attribution, timestamp/bracket helpers, and unit tests.

The meteorology handoff is the existing `MetTimeBracket` of legacy wind/surface
fields. #76/#77 canonical migration is outside this decomposition. There is no
mass-ledger object or output-window scheduler in this file. The caller consumes
step reports/particle state/probabilities and invokes concentration gridding;
`src/bin/corpus-run.rs` owns its budget accounting. Those ownership boundaries
must stay where they are. No new scheduler, ledger, or error state machine is
needed to split this file.

## Preserved forward order

1. Reject a completed run; optionally spatial-sort/upload host particles.
2. Start profiling; format current timestamp; inject/upload scheduled releases.
   With compaction, widen the dispatch count to the host active count.
3. Resolve bracket alpha; upload dual wind only when bracket timestamps change;
   interpolate surface fields; upload the current PBL ping-pong slot's surface
   inputs, or retain the explicit existing PBL diagnostic override.
4. Convert dt; validate forcing species shape; determine zero-forcing skips;
   materialize/upload dry forcing, then wet scavenging and fraction at full
   capacity. This preparation precedes the wait for the previous submission.
5. Wait for the previous pending GPU submission; clear its pending flag.
6. Build deposition/decay parameters; create one caller-owned encoder. Encode
   optional GPU PBL -> dual-wind advection -> fused Hanna/Langevin (production),
   or Hanna -> Langevin (validation) -> optional dry -> optional wet -> optional
   decay -> optional compaction/gather. Submit that encoder once.
7. Mark pending; flip the PBL slot. If profiling, wait and clear pending. Commit
   the next Philox counter. Optionally download dry probabilities, then wet
   probabilities; each successful readback clears pending.
8. With compaction, wait if needed, download active count, update dispatch count
   and host compaction bookkeeping. With host sync, wait if needed, download
   particles and recount active flags.
9. Build timing and the step report using the pre-advance timestamp/index; only
   then increment the index and saturating-add dt. End time is inclusive.
   `run_to_end` repeats this sequence and drains pending work with `finalize`.

Gridding remains an explicit caller-requested GPU/readback operation; callers
must finalize pending work as documented. Finalize only drains pending work;
it does not implicitly refresh the host store. Existing cached active-count
semantics when host sync is off are preserved. Errors propagate at their original
positions; the refactor adds no rollback or successful skip.

## Preserved backward order

Reject completed run -> timestamp/receptor release -> alpha/bracket upload ->
surface interpolation/PBL input upload -> positive dt/forcing validation and
uploads -> one encoder: optional GPU PBL -> negative-dt advection -> Hanna ->
positive-dt Langevin -> optional dry -> optional wet -> optional positive-dt
decay -> one submit -> wait -> counter update -> particle readback -> source
collection/report -> index increment/saturating-subtract dt. End time is
inclusive. Backward always synchronizes; it has no compaction or overlap.

## Verification boundaries

Baseline and post-move focused forward/backward tests, an operator-call order
regression and a forward transport/deposition/decay regression establish this
structural claim. GPU execution must be observed, not inferred from skip-capable
legacy tests. Final checks are formatting, Clippy, all Cargo tests, navigation
checks and the existing technical/software-WGSL CI gates. This work defines no
scientific tolerance, kernel, oracle or new parity claim.

## Final responsibility layout and context comparison

The stable `simulation::timeloop` facade retains the existing public API. Its
private modules are `config`, `error`, `forcing`, `reports`, `time`, `options`,
`meteorology` (bracket type), `backward`, and `forward`. Forward owns the original
run/resource state and lifecycle; its private `timestep`, `meteorology`,
`forcing`, `operators`, `particles`, and `output` modules implement cohesive
orchestration responsibilities. Prepared meteorology/forcing records pass only
existing decisions and timing values to the ordered submission stage. Resource
ownership stays on the driver, with no second runtime or compatibility adapter.

Representative task: inspect/change the forward physics sequencing under its
own future contract. Before: the entire monolith, 2572 lines,
102,324 bytes with LF newlines. After: facade, driver resource/lifecycle owner,
timestep coordinator, and operator owner, 772 lines, 33,074 bytes
(about 68% less source). Common GPU/scientific contracts and focused tests remain
required and are excluded from both counts. Forcing/meteorology/cache internals,
backward attribution and timestamp validation need not be loaded for this task;
follow their named handoffs if the task crosses them. This is a measured source
selection reduction, not a token, latency or scientific-performance claim.

No scientific deviations are introduced. No mass-ledger or output scheduling
consumer is moved. Original pending-work, cached host-state, error propagation,
operator arguments and branch conditions remain intact.

## Validation and separate findings

The pre-move and post-move forward/backward targets passed. Added regressions
cover the actual encoded operator call sequence, preparation/wait/readback/report
boundaries, transport before the dry-deposition height check, existing combined
mass evolution, inclusive end time and deferred host/output behavior. Required
device regressions fail on missing adapters. Initial implementation validation
passed for fused and separated forward paths,
focused physics integration, all 690 Cargo tests and Clippy.
Navigation audit and its 26 regression tests passed. A retained lexical audit
checked 64 unchanged routine bodies, three unchanged extracted phases, and
reconstruction of the timestep body (ignoring comments/import ordering,
whitespace and formatter-added trailing commas).

Full `cargo fmt --all -- --check` fails on 27 unchanged baseline files;
format checks on all changed Rust surfaces pass. Those unrelated files were
not reformatted. The local compact paired `ADV-ANA-001` check ran the candidate
successfully but remains BLOCKED because the pinned oracle checkout is absent;
it is not paired validation. Existing remote technical and software-WGSL gates
remain required. The software gate now runs both forward variants and backward,
requires the new device regression markers and retains their logs.

An additional optional-compaction probe exposed an existing dry-deposition
resource-length mismatch (active dispatch 1 versus capacity 8). Reproduction
against the exact original driver at `17fb779` failed identically. This path
was stopped and [follow-up #135](https://github.com/Grodahn/flexpart-gpu/issues/135)
was created; #126 preserves the original failure. No semantic repair is included.

## Review hardening

The order regression also requires exactly one occurrence of each preserved
operator/submit call, so an extra dispatch cannot hide in an otherwise matching
subsequence. Preparation/retry coverage checks the existing bracket, species
shape and per-slot forcing errors: release happens before preparation failure,
time/index do not advance, and a successful retry processes the existing
release once. Both software-CI variants require the new error/retry device marker.

Forward GPU tests share a test-only mutex so concurrently scheduled cases do
not reproduce the observed Windows WARP access violation. Poison recovery keeps
a failed case from masking the remaining tests. This affects test scheduling;
the simulation's GPU submission/readback behavior and production routines are
unchanged. The private phase handoffs now document their synchronization and
error-state guarantees where agents read them.
