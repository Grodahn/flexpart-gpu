# Shared-height U/V preparation (#184)

This stage proves scoped U/V meteorology preparation only. It does not adopt
prepared fields in #76/#171 queries, modify interpolation, prepare W, or prove
production advection parity. The existing nonuniform-stencil rejection remains.
Scientific authority is the direct pinned #118 oracle described in
[the research decision](research/shared-height-118.md).

`gpu::shared_height::SharedHeightSource::upload` validates an immutable #29
Snapshot against its opaque #30 runtime view, then explicitly uploads physical
bottom-to-top lanes. Each lane contains AGL height, U, V and local pressure.
Artificial ground is exactly zero with required U10/V10 values. The source
records snapshot, geometry, coordinate, time, grid, input and device identities.
There is no new hypsometric calculation, CPU remapping or intermediate readback.

`SharedHeightPreparer::initialize` encodes GPU selection of the initial `(0,0)`
column and copies its native #30 heights plus ground into a run-lifetime target.
Only a regional mother grid, no restart/nests/poles, with initial pressure
strictly above 100000 Pa is supported. Other initialization variants fail
explicitly. Expected target bytes/hash are host audit metadata derived from #30;
the actual authoritative target used by remapping is produced on the GPU.
Validation compares those device bytes with the expectation and pinned oracle.

`prepare` encodes the pinned endpoint and strict interior remapping into two
immutable f32 scalar field resources in x-fastest `[target-level,y,x]` order.
The returned owner retains both source and target through `Arc`; clones share
resources rather than overwrite them. Retain the prepared owner through the
final submitted consumer. The target additionally retains its initial source.
Separate prepared owners hold both meteorological bracket members; an adjacent
bracket may reuse a member only after `require_reuse` confirms its exact source,
geometry, fields, time, target owner and device. Changed sources require a new
preparation against the retained run target. Native/target counts must match.

All encoding uses a caller-owned encoder. Initialization and preparation do
not submit, poll, wait or map. The composition owner remains responsible for
submission, scoped device errors and the final status checkpoint. Buffer sizes,
binding limits, dispatch counts and u32 indexing are checked before allocation.
Pipeline construction captures validation, internal and out-of-memory errors.

Target status is a single u32; preparation status is one u32 per column:
zero is unexecuted, one is pending, two is valid, three is invalid prerequisite
or geometry, four is a nonfinite result. Every consumer must require target
status two and all columns it consumes to be two. A failed source is not accepted
even when some output words were written. Status remains device-resident; the
test consumer demonstrates this handoff in one encoder and one submission.
Readbacks occur only at the explicit final validation boundary.

Unsupported: W in every form, restarts, nesting, global/polar routes, pressure
selection fallback or a different selected column, face staggering, changing
grid/vertical-coordinate definitions, unequal native/target counts, malformed or
nonfinite inputs, resource/device mismatches, and device limit violations.
Transport top-boundary physics, ingestion, ASL conversion and production
consumer adoption remain with their owning issues.

Verification uses `cargo test --test shared_height_gpu -- --test-threads=1`.
The required device tests never skip absent adapters. Set
`FLEXPART_GPU_SHARED_HEIGHT_ORACLE` to a freshly regenerated #118 `report.json`
to bind comparison to that executable and raw output. All 64 U/V values (two
times, both x/y rows, four levels) use #80's finite combined tolerance. The
generic execution-evidence comparison additionally applies its stricter OR
policy; it does not replace the combined row check. Reports, exact serialized
inputs, differences, source/target identities and adapter/shader/oracle hashes
are written to `target/ci-gate/shared-height-gpu/` or the explicitly selected
`FLEXPART_GPU_SHARED_HEIGHT_EVIDENCE` directory. The checker rejects missing
rows, execution, provenance, changed hashes or finite tolerance failures.
