# Complete shared-height U/V/W queries (#190)

`CanonicalGpuField::from_shared_height_vector` returns U, V and W owners for the
existing #76 `prepare_sample` and #171 `prepare_resident` APIs. It accepts exact
Snapshots, #30 runtime views, #184 prepared U/V and #188 corrected W. No scientific
calculation occurs during adoption. The genuine pinned FLEXPART 11.1 authority is
[#186](research/interior-w-186.md), full correction only, under interpolation
contract section 11. Production consumers and #180 remain separate.

The constructor checks complete source counts, chronology, source/geometry/time,
horizontal grid, context, the identical run target owner and the exact U/V owner
retained by W. Normalized interface-motion provenance must match the supplied
opaque #30 runtime. A canonical W field, when present, must match that normalized
motion. W can also come directly from #30's normalized motion without inserting
another field into the immutable Snapshot. Query-time bracket resolution uses
the already validated same-source U/V time in that case. W output metadata is
scalar m/s positive upward on shared metric levels; `shared_height.corrected_w`
retains the distinct source-interface, normalization, horizontal and correction
identities, including their hashes. The U/V-only API and serialized identity
remain compatible.

Each component retains all corrected W owners and their native uploads, U/V,
target and context dependencies. Keep canonical owners and query plans alive
through the last submitted consumer. Adjacent brackets may clone the unchanged
source owners and must retain the same run target. Construction checks host
identities; it cannot certify device completion. Before accepting any component,
the existing preparation-status kernel requires target status 2, every U/V/W
column status 2, and finite U/V/W values across each selected source. Failures
outside the current query stencil also invalidate the vector.

Preparation, field copies, the existing horizontal -> vertical -> temporal
samplers, final lane validation and fixture consumption compose in the caller's
encoder. No query applies another correction or normalization. No intermediate
download, submission, polling or CPU fallback is introduced. Prepared host-query
consumers must check `EncodedMeteorologySample::status` before accepting values.
Resident consumers must check every component's complete lane/fatal status before
publishing a vector; the bounded integration fixture demonstrates this with
independent component statuses and all-or-nothing writes across workgroups.

Only this typed prepared representation lifts differing-native-height rejection
for shared-grid W. Native/unprepared and unsupported initializer, center-W,
global/nested/polar/ETA, ASL/terrain and other unproved routes retain their existing
rejection. The production driver full-vector guard and #112–#115 are unchanged.

## Verification and evidence

Regenerate the genuine oracle with the existing launcher:

```text
python scripts/interpolation/run_shared_height_oracle.py --interior-w --checkout <pristine-flexpart> --output-dir target/interior-w-query-oracle
```

Set `FLEXPART_GPU_INTERIOR_W_ORACLE` to that fresh `report.json`, then run:

```text
cargo test --test shared_height_vector -- --nocapture --test-threads=1
python scripts/interpolation/check_shared_height_vector.py target/ci-gate/shared-height-vector/report.json --hardware
python scripts/interpolation/test_check_shared_height_vector.py
```

Repeat on a real software WGSL adapter with `FLEXPART_GPU_SOFTWARE=1` and
`--software`. `FLEXPART_GPU_SHARED_VECTOR_EVIDENCE` selects a separate directory
for each adapter. Required software CI executes the target and independently
audits its retained report again in the required aggregate.

The [fixture](../tests/shared_height_vector.rs) checks all 15 full-correction
QUERY rows at both endpoints and midpoint, target endpoints, exact target levels
and strict interiors: 90 scalar public-API comparisons plus 45 vector-consumer
comparisons. The finite velocity rule stays
`abs(a-b) <= 1e-6 + 1e-5*max(abs(a),abs(b))` m/s. Reports retain actual outputs,
differences, source/target/bracket and normalized-interface preimages, correction
lineage, executable/source/shader hashes, adapter/backend, preparation statuses,
scoped errors and `GpuCalculationEvidence`. The auditor verifies the genuine
oracle's exact pin, clean checkout, linkage and artifact hashes. Missing,
incomplete, skipped or nonfinite execution cannot pass.

Negative cases cover identities, source reuse, native and ASL rejection, time and
capacity guards, inactive lanes/tails, cross-workgroup failure, independent
incomplete W, pending/unexecuted/failed preparation, failed dependencies and
nonfinite corners. The native-interface test supplies schema-valid normalized W
and requires the specific unsupported multi-column error. The evidence auditor
binds query sources to the exact prepared U/V metadata and checks physical signs,
grid/geometry provenance, temporal brackets and scalar query layouts.
Existing composition, resident, driver and #87/#88/#89 gates
remain required. This evidence proves the canonical query handoff; it does not
claim full FLEXPART production parity.
