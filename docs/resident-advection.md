# Canonical production advection (#112)

The default `ForwardTimeLoopDriver::run_timestep` and
`BackwardTimeLoopDriver::run_timestep` require `MetTimeBracket::canonical`.
Prepare that owner through the completed #173 driver method and retained
`CanonicalMeteorologySlot`, clone the returned resource Arc, drop the temporary
driver borrow, and lend the owner to the timestep. The source contains exact
U/V/normalized-center-W identities and #30 geometry. No source is reconstructed
from legacy wind fields. The canonical grid must match the particle coordinate
grid. Current and signed advanced times must both satisfy #89 coverage.

## Production sequence and atomicity

Both actual driver encoders use `gpu::advection_resident`:

`scientific particles -> current #171 queries -> U/V/W #87/#88/#89 samples ->
private predictor queries -> advanced-time U/V/W samples -> private corrected
particles -> status eligibility -> existing downstream physics/compaction ->
guarded scientific-state publication`.

All queries, values, coordinates and Petterssen arithmetic remain on-device.
Predictors read scientific particles without mutating them. Signed cells and
fractions are displaced separately; height remains metres AGL. Horizontal
velocity includes the existing turbulent U/V in both stages; W remains the
mean vertical wind, with the existing Langevin vertical transport downstream.
Backward advection uses negative dt; backward turbulence/deposition/decay retain
the existing positive magnitude. Existing operator order and owning submission
are preserved. The consumer shaders perform no field interpolation.

The six scalar samples have independent resident status owners. Device copies
retain their fatal words and lane reasons in one timestep buffer; no later reset
can erase an earlier field/stage failure. A D2D particle snapshot is private
transaction state. After all samples and corrected arithmetic finish across all
workgroups, a separate eligibility pass makes private lanes inactive on any
fatal status. Downstream particle physics therefore receives no eligible state
from a failed step. The final guarded raw-word publication leaves *every* original
particle byte untouched on failure, including inactive tails and padding.
Compaction may operate on private state; failed compacted state is never published
or accepted by its host count consumer.

Forward status is observed at the previous-step synchronization before another
release or sort can mutate particles, and at explicit
per-step/output/finalization host checkpoints. Backward checks it before particle
readback/source attribution. Deferred forward output checks status before output
acceptance. No particle query or sampled wind travels GPU -> CPU -> GPU.

## Migration inventory

| Call path / fields | Source and disposition | Owner / limits |
| --- | --- | --- |
| Forward `run_timestep` -> `submit_operators`, fused and validation modes, with/without compaction: U/V/W | **Migrated** to #173 owners and resident Petterssen; no production dual-wind upload | #112; canonical cell-center U/V and normalized center W only |
| Backward `run_timestep`: U/V/W | **Migrated**, negative advection dt; required real-driver evidence | #112; existing backward source-attribution checkpoint |
| Default `run_to_end` in either driver | **Migrated** through the canonical timestep; inclusive last step also requires advanced-time coverage | #112 / #89 |
| `etex-run` real-file production caller | **Blocked**: legacy inputs supply no validated canonical bracket; default API rejects explicitly | #32; no legacy fallback |
| `run_legacy_diagnostic_timestep` / `run_legacy_diagnostic_to_end`, existing synthetic corpus/ETEX scaffold, Fortran diagnostics and timestep benchmarks | **Diagnostic only**, explicitly named APIs retaining buffer/texture dual-wind behavior; canonical owners are rejected | #117 owns broader cleanup; these checks are not migrated production evidence |
| Standalone `gpu::advection` single/dual buffer/texture helpers and standalone fused `particle_step` | **Not applicable** to migrated drivers; retained standalone diagnostics | #117; no calls from canonical operator branch |
| Legacy single-wind forward setup helpers | **Not applicable**, unused compatibility diagnostics | #117 |
| PBL, deposition and convection meteorology consumers | **Not migrated** by this change; their existing operators/forcing remain | #113–#115 |

The default API cannot execute a legacy-only bracket successfully. Explicit
legacy diagnostics cannot accept canonical fields. Host-query `prepare_sample`
is absent from canonical transport. Legacy wind in a canonical `MetTimeBracket`
is ignored by advection; its existing surface fields remain the PBL input.

## Supported boundary and unresolved inputs

Transport retains horizontal domain clamping and the existing zero-AGL through
highest-model-center vertical clamp. Its top is obtained from exact canonical
#30 physical columns, never from legacy level counts or placeholder heights.
The currently supported top is invariant across columns and source members.
Different tops reject explicitly pending
[#180](https://github.com/Grodahn/flexpart-gpu/issues/180), which owns the missing
transport boundary decision. Fractional nonuniform stencils remain #118; interface
W remains unsupported; operational provider decoding remains #32. No tolerances
or pinned oracle definitions are changed. This is not full FLEXPART parity.

## Verification and retained evidence

`cargo test --lib gpu::meteorology::advection_production_tests -- --nocapture --test-threads=1`
requires actual WGSL execution through both real production drivers. It covers
constant/spatially varying U, distinct positions/times, signed backward motion,
active prefixes 1/3/130, predictor-only V/W and initial-U device-source poisoning,
cross-workgroup atomic preservation with one failing lane among otherwise valid
lanes, nontrivial inactive tails, interior-time
forward sampling, deferred failure rejection before a subsequent release/output,
and fail-closed changing-top geometry. Deliberately
contradictory legacy wind proves it is not sampled by the canonical branch.
Run it separately with `FLEXPART_GPU_VALIDATION=1` and
`FLEXPART_GPU_COMPACTION=1`; CI retains all configurations.

`target/ci-gate/resident-advection-production/` retains exact canonical/native
input encodings and derived runtimes, source hashes, adapter identity,
field/time/geometry metadata, final device-written stage queries/sampled values,
all status words, shader hashes and the candidate revision. Query/value downloads
are test-only final evidence after the owning production submission, never inputs
to the transport calculation. Existing #87/#88/#89 oracle gates and #173/#171
integration tests remain authoritative for the reused sampler. New displacement
checks are analytical production integration evidence, using the existing #88
finite comparison policy at unit timestep/scale. The existing paired ADV-ANA-001
runner retains its legacy diagnostic candidate and
`DIAGNOSTIC_NO_PARITY_VERDICT`; it does not establish canonical production parity.
