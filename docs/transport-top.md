# Original upper transport action in production (#198)

Both real `run_timestep` drivers use the original ordinary-particle action from
pinned FLEXPART 11.1 `c70586c2b7f5258850705325881c61f557ea9bd8`,
`advance_mod.f90:287-333,664-779,872-900`. The frozen scientific authority remains
[#180](research/transport-top-180.md), including its 24 private boundary and 72
public advance observations. No reference source, fixture or tolerance changes.

`SharedHeightTransportBoundary` validates the exact complete U/V/W owners, both
source times, target and context, and retains #184's run target through deferred
execution. The shaders bind its device height/status buffers. Runtime authority
never comes from the host target expectation, legacy wind, a native-column
maximum or a new target initializer. The regional mother grid supplies `nxmax`
through its validated `nxfield` count.

The actual sequence is current sampling, predictor movement, inclusive upper
action, sampling at the adjusted predictor and signed advanced time, correction
of that predictor, and another upper action. For ordinary particles the original
f32 expression is `top - 100*(nxmax/3.e5)`. The demonstrated device endpoint
`9521.84375 m` and `nxmax=2` yield exactly `9521.8427734375 m`.
Existing horizontal/lower treatment is preserved. The correction retains the
existing lower-boundary behavior; it does not adopt new reflection physics.

The predictor remains private. Six independent sample/preparation statuses,
cross-workgroup eligibility, downstream private physics/compaction and guarded
raw-word publication remain unchanged. Missing/failed target/U/V/W preparation
and invalid or f32-unrepresentable endpoint placement fail atomically, including
inactive tails. Owners remain alive through the caller's single scientific
submission; there is no intermediate scientific D2H, CPU fallback or new sampler.

## Supported and excluded routes

The complete prepared regional non-ETA AGL route now admits unequal native
column heights and unequal source-member heights. The transport top remains the
same run-lifetime target for all queries. Below/on/above starting states and
predictor/corrector/repeated encounters are exercised as legal routine inputs;
this does not establish release-file acceptance or operational ingestion.

Restart/fallback initialization, nested/global/polar grids, ETA/ASL, native or
incomplete W, mixed source/time/target/context and alternative precision/adaptive
PBL routes remain unsupported. Existing constructors reject these routes.
Requested settling (`ForwardStepForcing::settling_enabled`) rejects before
release/encoding in both production drivers and their legacy diagnostic paths.
There is no validated production settling flag/velocity or sticky `nstop` owner.
[#203](https://github.com/Grodahn/flexpart-gpu/issues/203)
owns that handoff; #35/#37 retain general physics. Original backward settling is
a scientific particle stop, never a fatal meteorology batch; this PR does not
fabricate that state or claim to execute the stopped route.

## Evidence and reproduction

Run the real-driver matrix with `FLEXPART_GPU_SOFTWARE=1` on software WGSL, or
`=0` on a recorded hardware adapter. Adapter absence fails. Run forward modes
`FLEXPART_GPU_VALIDATION=1` and `FLEXPART_GPU_COMPACTION=1` separately.

```text
cargo test --lib gpu::meteorology::advection_production_tests -- --nocapture --test-threads=1
python scripts/transport/check_transport_top.py --report target/ci-gate/transport-top/validation-0-compaction-0/report.json --software
python scripts/transport/test_check_transport_top.py
```

Fresh original executions use the unchanged #180 launcher and paired auditor:

```text
python scripts/transport/run_top_boundary_oracle.py --checkout <pristine-pin> --output-dir target/top-first
python scripts/transport/run_top_boundary_oracle.py --checkout <pristine-pin> --output-dir target/top-second
python scripts/transport/compare_top_boundary_builds.py --first target/top-first --second target/top-second --output target/top-pair.json
python scripts/transport/check_transport_top.py --report target/ci-gate/transport-top/validation-0-compaction-0/report.json --oracle-run target/top-first --software
```

Reports retain source snapshots/runtimes, exact prepared source/target and
normalization identities, device target/status, shader/source/executable hashes,
adapter/backend/driver, raw query/sample/status data and pre-physics advection
particle bytes. The matrix uses 130 particles across three workgroups and an
inactive sentinel. Its **explicit shared-grid meteorological controls** match
the genuine #180 driver, which likewise assigns shared winds before `advance`.
These test-only assignments are recorded; they are not presented as remapping
proof. #186/#197 separately retain genuine prepared-field query replay with the
existing velocity policy. The ordinary real production sampler is used for every
current and advanced query; the test never supplies expected boundary heights.

All 72 original rows are accounted for: scenarios 1–7 require exact projection
of the original final height to f32 (56 rows), scenario 8 retains eight trajectory
diagnostics, and eight settling requests reject explicitly. Original `PUSH` rows
are freshly audited unchanged; representable ordinary at-top placement is also
exercised through stationary public `advance`. The f64-only `top-0.0001` probe
cannot establish f32 predicate equivalence and is excluded.

Scenario 8 can differ from the f32 projection of original f64 accumulation even
with representable initial state. No trajectory tolerance is approved here.
[#204](https://github.com/Grodahn/flexpart-gpu/issues/204) owns that decision;
those rows have **no trajectory parity verdict**. Only sampled m/s use the
existing #80/#190 velocity policy. Boundary placements and sampling coordinates
are checked exactly. Advection snapshots precede unchanged downstream turbulence;
they are not complete-timestep or complete-FLEXPART trajectory parity evidence.

Software CI audits all three configurations and mutation regressions. The
technical gate pairs actual device results with a freshly verified original
build; the existing transport-top workflow independently reproduces two builds.
Hardware evidence must come from a real recorded adapter. Missing/skipped runs
are not passing evidence. #198 must remain open if its required accepted-route
evidence is incomplete; this implementation claims no complete FLEXPART parity.

Provenance marker: **GPT6.1 Sol**.
