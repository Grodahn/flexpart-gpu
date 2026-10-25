# Canonical production advection (#112 / #197)

The default `ForwardTimeLoopDriver::run_timestep` and
`BackwardTimeLoopDriver::run_timestep` require `MetTimeBracket::canonical`.
Prepare that owner through the shared-vector driver preparation method and retained
`CanonicalMeteorologySlot`, clone the returned resource Arc, drop the temporary
driver borrow, and lend the owner to the timestep. The source retains #184 U/V, #188 corrected interface W, the run-owned shared
AGL target and their exact #190/#30 identities. No source is reconstructed
from legacy wind fields. The canonical grid must match the particle coordinate
grid. Current and signed advanced times must both satisfy #89 coverage.

## Production sequence and atomicity

Both actual driver encoders use `gpu::advection_resident`:

`scientific particles -> current #171 queries -> U/V/W #87/#88/#89 samples ->
private boundary-adjusted predictor queries -> advanced-time U/V/W samples -> private corrected
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
| Forward `run_timestep` -> `submit_operators`, fused and validation modes, with/without compaction: U/V/W | **Migrated** to #190 complete prepared-vector owners and resident Petterssen; no production dual-wind upload | #197; complete same-source/same-target prepared U/V/W only |
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

Transport retains existing horizontal and lower-AGL handling. The run-owned
`SharedHeightTransportBoundary` retains #184's target; predictor/corrector shaders
read its device endpoint and status directly. `nxmax` is the validated regional
mother-grid `nxfield`. At `z >= top`, ordinary particles receive the original f32
`top - 100*(nxmax/3.e5)` placement. Correction adds half the wind difference to the
already adjusted predictor; a second upper action follows. Native tops can differ
across columns and times after complete #190 preparation. A nonfinite endpoint or
an f32 margin unable to place strictly below it fails atomically.

The finite proof and reproduction commands are in [transport-top adoption](transport-top.md).
The real-driver test records the private **advection-stage** result before the
unchanged downstream turbulence; it does not claim whole-timestep trajectory parity.
Requested `ForwardStepForcing::settling_enabled` fails before release/encoding in
both production drivers. #203 owns the missing validated flag/velocity and sticky
scientific-stop handoff. A scientific stop must never become a fatal meteorology
batch. No stop state is fabricated by this ordinary-particle implementation.

Production requires the complete typed vector; the older #173 center-W handoff
is diagnostic-only. Unready target/U/V/W dependencies reject the entire vector,
including faults outside the current stencil. PBL/deposition/convection and
operational provider decoding retain their existing owners. No tolerances or
oracle definitions change, and no full FLEXPART parity is claimed.

## Verification and retained evidence

`cargo test --lib gpu::meteorology::advection_production_tests -- --nocapture --test-threads=1`
requires actual WGSL execution through both real production drivers. It covers
constant/spatially varying U, distinct positions/times, signed backward motion,
active prefixes 1/3/130, predictor-only V/W and initial-U query-lane corruption,
cross-workgroup atomic preservation with one failing lane among otherwise valid
lanes, nontrivial inactive tails, interior-time
forward sampling, deferred failure rejection before a subsequent release/output,
and admission of differing native member heights. Deliberately
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
finite comparison policy at unit timestep/scale. The [#197 genuine replay and preparation failure matrix](shared-advection.md)
adds complete-vector production proof and both software/hardware evidence.
The existing paired ADV-ANA-001
runner retains its legacy diagnostic candidate and
`DIAGNOSTIC_NO_PARITY_VERDICT`; it does not establish canonical production parity.
