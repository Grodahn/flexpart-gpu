# Prepared-vector production advection (#197)

Both real production timestep drivers adopt the validated #184 U/V and #188 W
resources through #190's complete typed vector and the existing #76/#171 queries.
The [source handoff](canonical-timeloop-meteorology.md) defines run-target ownership,
source lifetimes, exact reuse and queue ordering. The [resident advection contract](resident-advection.md)
retains private predictor/corrector state, six independent statuses, active prefixes,
compaction and guarded all-or-nothing scientific publication.

## Supported envelope after #198

Production accepts the regional mother-grid AGL route, supported initial (0,0)
pressure selection, exact normalized-interface-omega provenance and complete
same-source/same-target U/V/W preparations. Native column and source-member tops
may differ. Transport always uses the retained **device** shared target endpoint,
not a native maximum. The inclusive upper action and adjusted-predictor correction
are documented in [transport-top adoption](transport-top.md).

Restart/fallback, global/nested/polar/ETA/ASL and incomplete/mixed preparations
remain rejected. Requested settling rejects explicitly pending
[#203](https://github.com/Grodahn/flexpart-gpu/issues/203); general settling remains
#35/#37. Adaptive PBL and wider precision are not newly supported. Existing
lower/horizontal handling and unrelated physics are unchanged. Operational
providers remain #32 and historical benchmark migration remains #199.

## Reproduction and evidence

Run the actual production matrix on the desired adapter:

```text
cargo test --lib gpu::meteorology::advection_production_tests -- --nocapture --test-threads=1
python scripts/check_resident_advection_evidence.py
python scripts/transport/shared_advection_evidence.py device --report target/ci-gate/shared-advection/validation-0-compaction-0/report.json --software
python scripts/transport/test_shared_advection_evidence.py
```

Set `FLEXPART_GPU_SOFTWARE=1` for software WGSL; use `=0` and the desired hardware
backend for a hardware run, checking `--hardware` instead. Repeat forward modes
`FLEXPART_GPU_VALIDATION=1` and `FLEXPART_GPU_COMPACTION=1` separately. Backward
retains its separated, noncompacting operators. `FLEXPART_GPU_SHARED_ADVECTION_EVIDENCE`
can select an independent evidence directory for each adapter. Default directories
include both mode values, so three CI configurations retain distinct reports.
Missing or skipped adapters, missing status or incomplete rows cannot pass.

The inherited #112 analytical matrix covers both real drivers, active prefixes
1/3/130, inactive tails, distinct current/predicted positions and signed times,
one failed lane with otherwise valid peers across workgroups, deferred checkpoints,
and varying-native-top admission. Query-lane fault injection is test-only; a failed
prepared source instead invalidates the complete vector, including remote columns.
The #197 matrix adds both real drivers with nonzero normalized interface W,
two interior heights, endpoints/interior temporal brackets and two successive
steps using the identical resource owners. Its 22 negative steps cover unexecuted/
failed targets, pending/invalid U/V, incomplete/failed/unknown W statuses, nonfinite
vector values and invalid/unrepresentable device transport endpoints. Fatal steps compare
all original particle bytes, including inactive tails. Adoption also rejects
mixed W sources, replaced run targets, missing motion and foreign contexts;
failed preparation leaves the original slot reusable.

For direct scientific comparison, first produce a complete verified #186 run via
its existing launcher, or reuse a verified retained run. Then:

```text
python scripts/transport/shared_advection_evidence.py replay --report target/ci-gate/shared-advection/validation-0-compaction-0/report.json --oracle-run target/interior-w-first --software
python scripts/transport/shared_advection_evidence.py check --report target/ci-gate/shared-advection/validation-0-compaction-0/report.json --oracle-run target/interior-w-first --software
```

The auditor verifies #186's exact pin, clean checkout, linked objects and source
hashes. An invariant-top control tiles its first initial thermodynamic profile
across members/columns while retaining the original U/V and native omega lanes.
It validates candidate input equivalence field by field, including surface U/V,
terrain, grid coordinates, hybrid coefficients, layouts, source times and native
motion conventions. Mutation tests reject changes to those independent inputs.
The existing #186 Fortran
harness gains only an audited input-plumbing branch for absolute query heights;
all initialization, transformation and public sampling calls are retained. The
existing linker/cache helper links that harness against the exact unchanged
pinned objects, recording executable, compiler, symbol, call-edge and object proof.
This avoids a second sampler or new scientific equations and reproduces each
actual device query coordinate/height/time exactly, without a coordinate tolerance.

All 96 scalar current/advanced U/V/W values are compared under the existing
#80/#190 policy `abs(a-b) <= 1e-6 + 1e-5*max(abs(a),abs(b))` m/s. The original
nonuniform #186 fractional-stencil queries remain the unchanged #190 query-layer
proof; their varying-native-top production route is now admitted with #198; this replay remains a wind-sampling proof. Displacement evidence
uses the existing owned analytical policy in the #112 matrix. No new trajectory
tolerance or full FLEXPART production parity claim is introduced.

Reports retain exact source/native/runtime encodings, target identity preimages,
prepared normalization/correction/source/context identities, six query/sample/status
regions, executable/source/shader hashes and adapter/backend. Replay retains raw
oracle input/output, build/link evidence, per-component differences and the original
pinned provenance. Required software CI audits all three device configurations;
the technical gate replays against its fresh genuine #186 build. Hardware evidence
must come from an actual recorded hardware adapter.

Provenance marker: **GPT6.1 Sol**.
