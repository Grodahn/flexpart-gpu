# Shared-height U/V canonical queries (#185)

This handoff proves U/V query integration only. It does not establish full
production U/V/W meteorology parity. Scientific authority remains the genuine
pinned FLEXPART 11.1 `eta=no` #118 transformation and `QUERY` results described
in [the research decision](research/shared-height-118.md).

`CanonicalGpuField::from_shared_height` accepts U or V, exact immutable
Snapshots and #30 runtime views, and `Arc<PreparedSharedHeightUv>` resources
actually produced by [#184 preparation](shared-height-preparation.md). It checks
snapshot hashes, geometry identity/provenance, source timestamps, horizontal
grid, native coordinates, field/staggering, context token, and the exact shared
target owner across both time members. A separately initialized target is
rejected even if its expected height bytes match. Missing resources, changed
source identity, invalid chronology and other fields fail closed.

The canonical owner retains each prepared source and its run target. The
Snapshot/#30 borrows and existing plan lifetimes remain in force; keep owners
through the final submitted consumer. An adjacent bracket may retain the same
prepared member using `Arc::clone` with the exact corresponding source/runtime
and run target. No source is overwritten or target silently recreated.

Both `prepare_sample` and `prepare_resident` reuse the existing horizontal,
vertical and temporal GPU kernels. Device copies populate their existing plane
resources from #184's immutable buffers. The host-query grid is overwritten
from the authoritative device target before sampling. The resident adapter
copies that same common target into each lane's existing #88 height layout.
Only this explicit representation bypasses comparison of incompatible native
column profiles. Native U/V and all other fields retain their prior rejection.
The particle ABI, signed cells/fractions, capacity/active prefix, inactive tails,
bracket resolution and sampling equations are unchanged. Prepared queries
require finite AGL heights; ASL/terrain conversion remains unsupported.

Initialization, preparation, particle query production, sampling and the existing
status-aware fixture consumer can share one caller encoder and submission.
Encoding performs no submit, wait, poll, host particle reconstruction, scientific
CPU fallback or intermediate readback. Preparation may therefore still be
pending on the host when the plan is constructed. Before acceptance, a separate
device status pass requires target status 2, status 2 in every source column,
and finite prepared scalar values throughout each selected source.
It preserves all prior fatal state and cannot clear a preparation failure.
Resident status reset precedes these checks, and the existing final finite/lane
check and fixture consumer enforce cross-workgroup fatal atomicity.

Prepared host queries expose `EncodedMeteorologySample::status` using the same
shared-fatal/lane ABI: consumers must require a zero fatal word and valid lane
before accepting results. Native host queries keep `status=None`. An encoded
handoff is never a completed-execution verdict. Final validation readbacks are
explicit output boundaries.

The [integration fixture](../tests/shared_height_query.rs) compares every one of
the nine direct #118 `QUERY` rows, both U/V components, through both public APIs:
36 finite comparisons. It covers fractional x/y, nonuniform native heights,
both time endpoints and intermediate time, strict interior heights and both
vertical boundaries. Tolerance remains
`abs(a-b) <= 1e-6 + 1e-5*max(abs(a),abs(b))` m/s. Golden fixtures and #184's
scientific shader are unchanged. Fault cases cover missing/foreign resources,
ASL/time rejection, native-path rejection, adjacent bracket reuse, active-prefix
reuse, mixed lanes, inactive tails, failed/pending/unexecuted preparation,
failed targets, nonfinite corners and incomplete cross-workgroup execution.

Run `cargo test --test shared_height_query -- --nocapture --test-threads=1` and
`python scripts/interpolation/check_shared_height_query.py target/ci-gate/shared-height-query/report.json`.
Set `FLEXPART_GPU_SHARED_HEIGHT_ORACLE` to a fresh report from the supported
#118 launcher to bind the comparison to the regenerated executable/output.
`FLEXPART_GPU_SHARED_QUERY_EVIDENCE` selects a separate evidence directory for
software/hardware runs. Reports retain all rows, exact serialized canonical
inputs, source/target/#30 identities, device target bytes, shader/source hashes,
full adapter provenance and final status cases. The checker rejects incomplete,
stale, failed or skipped evidence; it applies no new scientific policy.
The #177 meteorology job retains this additional gate and artifacts; its
required `software-wgpu` aggregate independently rechecks them.

The production driver handoff still requires a complete validated U/V/W vector.
#188 W preparation and #190 W adoption remain prerequisites for the later
#112–#115 consumer work. No production driver or #184 remapping equation changes
belong to this handoff; #180 transport-top physics remains separate.

The complete vector is now adopted by production advection through
[#197](shared-advection.md), under its invariant-native-top/no-upper-crossing
restriction. #198 still owns transport-top behavior; #113-#115 remain separate.
