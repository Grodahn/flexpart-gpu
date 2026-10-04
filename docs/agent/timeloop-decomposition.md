# Time-loop decomposition (#126)

## Pre-move inventory

Recorded against `17fb779` before code movement. The original
`src/simulation/timeloop.rs` contains 2,549 lines (99,312 non-newline
characters): configuration/validation, forcing shapes and caches, reports,
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
