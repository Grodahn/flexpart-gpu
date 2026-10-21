# Prepared-vector production advection (#197)

Both real production timestep drivers adopt the validated #184 U/V and #188 W
resources through #190's complete typed vector and the existing #76/#171 queries.
The [source handoff](canonical-timeloop-meteorology.md) defines run-target ownership,
source lifetimes, exact reuse and queue ordering. The [resident advection contract](resident-advection.md)
retains private predictor/corrector state, six independent statuses, active prefixes,
compaction and guarded all-or-nothing scientific publication.

## Interim supported envelope

Production accepts the regional mother-grid AGL route, supported initial (0,0)
pressure selection, exact normalized-interface-omega provenance and complete
same-source/same-target U/V/W preparations. Native top heights must be identical
across every column/member and equal the retained target endpoint. Initial,
predicted and corrected positions at/above the upper limit reject atomically.
No pinned upper-boundary action is implemented here: [#198](https://github.com/Grodahn/flexpart-gpu/issues/198)
owns `pushpartdown`, adjusted-predictor correction and admission of varying native tops.
Existing lower/horizontal boundary handling and unrelated physics remain unchanged.
Restart/fallback, global/nested/polar/ETA/ASL, missing sources and unsupported
normalization remain fail-closed. Operational providers remain #32.

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
and variable-top rejection. Query-lane fault injection is test-only; a failed
prepared source instead invalidates the complete vector, including remote columns.
The #197 matrix adds both real drivers with nonzero normalized interface W,
two interior heights, endpoints/interior temporal brackets and two successive
steps using the identical resource owners. Its 26 negative steps cover unexecuted/
failed targets, pending/invalid U/V, incomplete/failed/unknown W statuses, nonfinite
vector values and initial/predictor/corrector top rejection. Fatal steps compare
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
It validates candidate input equivalence field by field. The existing #186 Fortran
harness gains only an audited input-plumbing branch for absolute query heights;
all initialization, transformation and public sampling calls are retained. The
existing linker/cache helper links that harness against the exact unchanged
pinned objects, recording executable, compiler, symbol, call-edge and object proof.
This avoids a second sampler or new scientific equations and reproduces each
actual device query coordinate/height/time exactly, without a coordinate tolerance.

All 96 scalar current/advanced U/V/W values are compared under the existing
#80/#190 policy `abs(a-b) <= 1e-6 + 1e-5*max(abs(a),abs(b))` m/s. The original
nonuniform #186 fractional-stencil queries remain the unchanged #190 query-layer
proof; their varying-top production route stays rejected. Displacement evidence
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
