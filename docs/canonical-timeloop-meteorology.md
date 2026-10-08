# Canonical driver preparation (#173)

Both `ForwardTimeLoopDriver::prepare_canonical_meteorology` and
`BackwardTimeLoopDriver::prepare_canonical_meteorology` accept the same
`CanonicalMeteorologyBracket` and caller-held `CanonicalMeteorologySlot`.
These are real driver preparation entries using the driver's context, clock and
configured signed timestep. They prepare inputs for #112; they do not execute
canonical advection. Existing `run_timestep` and `MetTimeBracket` retain their
legacy behavior until that consumer migration.

Construct the bracket from two borrowed canonical `Snapshot`s and their exact
`VerticalRuntimeView`s. The fixed pair requires both geometry slots. Construction
routes full snapshots through #29 validation, runtime fingerprint/dimensions and
normalized W through #76's existing #30 binding check, and chronology/calendar
through #89. Only cell-center U/V and normalized model-center W are accepted.
Interface W, absent motion lineage, mismatched geometry, invalid units/signs and
unsupported time kinds fail explicitly. No provider data or placeholder heights
are inferred. Synthetic tests derive honest geometry with #30's public transform.

Driver preparation resolves **both** current and predicted time before uploading:
forward uses `current + dt`, backward uses `current - dt`. Both must be covered
by the source pair, even when the legacy driver's bounds mode permits clamping.
The result supplies distinct `MeteorologyTimeSelection`s for #171. No particle
positions are inspected, sampled or downloaded during preparation.

The slot owns one `Arc<CanonicalMeteorologyResources>`, containing exactly three
existing `CanonicalGpuField` owners in `CANONICAL_WIND_FIELDS` order: U, V, W.
`resources().bracket()` exposes immutable grid, field IDs, source times/hashes,
geometry identities/provenance and native-motion provenance. Reuse compares the
complete validated identity, including motion lineage, rather than timestamps
alone. Unchanged brackets perform no uploads. A transition uploads three new
owners and replaces the slot only on complete success. A rejected time or device
does not replace the slot and never invokes the legacy path.

`PreparedCanonicalMeteorology::context()` lends the exact driver context for
`CanonicalGpuField::prepare_resident`, query kernels and caller-owned encoding.
`resources()` lends an `Arc` that callers can clone to retain an old source
through in-flight work and bracket replacement. The timestep result borrows the
driver briefly; the retained source owner does not, so the driver can advance
while the slot persists. Source/geometry borrows prevent mutation or use beyond
the canonical inputs' lifetime. Keep retained owners until the last submitted
consumer completes. No upload, submission, wait or readback hides inside a query
handoff. The existing context ownership token (also used by particle buffers)
rejects independent contexts without relying on backend handle equality.

Construct a validated bracket once per source transition. For each step, clone
that borrowed handle (which does not clone snapshots or geometry), call the
appropriate driver entry with the retained slot, then use `prepared.resources().fields()[0..3]` with
`prepared.times.current` or `prepared.times.predicted`. #112 owns connecting those
resident plans to predictor/corrector producers and the production operator chain.
No second runtime, interpolation representation or equation is introduced here.

## Evidence and limits

Run `cargo test --test canonical_timeloop_meteorology -- --nocapture --test-threads=1`.
The [integration target](../tests/canonical_timeloop_meteorology.rs) requires a real
adapter, executes #171 with the same driver-prepared owners for all components
and both time selections, checks the inherited #88 comparison policy and exact
field/grid/source/geometry metadata, and retains
`target/ci-gate/canonical-timeloop-meteorology/report.json`.
The report retains exact hashed canonical and native-motion input encodings plus
derived runtimes for the original, changed-source and changed-lineage brackets.
Every GPU row names its source bracket. CI audits each row against that bracket's
input hashes, field metadata and geometry, and recomputes the constant-field
expectation using the existing comparison policy.
It covers host-negative preflight, context rejection, source/motion transitions,
old-owner execution after replacement, transactional failure and reuse after
actual mutable driver steps. Those clock-advance steps explicitly invoke the
existing legacy API; they do not claim canonical advection adoption.
The software WGSL workflow requires fresh revision-bound preparation evidence
alongside the existing #171 and pinned #87/#88/#89 gates.

#112 still owns advection migration; #32 owns operational provider decoding;
#118 owns differing geometry within fractional stencils. Interface W and new
vertical-motion semantics remain unsupported by this driver handoff. This proves
production input/resource readiness only, not FLEXPART production advection parity
or operational real-file acceptance. No scientific formulas, tolerances or
deviations from the Fortran reference are introduced.
