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
The margin `100*(nxmax/3.e5)` is evaluated once in IEEE f32 on the host from the
validated `nxfield`, because WGSL division is not correctly rounded and a device
compiler may contract `top - 100*eps`. The device performs only the correctly
rounded subtraction from its own run-owned endpoint. The corrector reads the
predictor's private raw height, so both stages share one boundary decision.
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
Canonical production now requires a run-owned `TransportSettlingConfig` with an
explicit #10 species table. The driver derives the original flag once, including
backward receptor mass-slot bindings. A missing table fails before release;
the per-step `settling_enabled` request remains rejection-only for compatibility.
The supported enabled route is `ZeroByCarrier`: every bound carrier has explicit
finite density <= 0 and valid gas parameters. No settling velocity is calculated.
Positive density, mixed gas/aerosol tables and `Produced` velocity remain unsupported
pending #35/#37.

## Settling stop integration (#214)

`TransportSettlingRoute` retains direction, GPU-context identity and SHA-256 of
the complete release inventory, species table and original options. `lsettling`
starts true (`com_mod.f90:175`). With MQUASILAG=0, any release point with more than
one original f32 `xmass > 1.e-9` disables it for the entire run
(`readoptions_mod.f90:2574-2585`, `point_mod.f90:20`). Multiple release points
carrying separate single species keep it enabled. The exact MQUASILAG derivation
is tested (nspec>1 disables); execution with MQUASILAG remains rejected. Domain
fill, restart and backward compaction are also rejected. Optional receptor
`species_masses_kg` uses the run table's positional slots, like forward releases.
All carriers are checked, so no live source requires an unvalidated producer.

The private uniform enables the stop action only for enabled backward transport.
The predictor ORs bit 0 on inclusive `predicted_z >= top`, retains its raw height
and uses it for advanced sampling. The corrector consumes that stored height and
predictor bit, continues the original half-difference correction, then ORs bit 1
on inclusive corrected encounter. It never retests the predictor predicate on
the stopped route. Forward and genuinely disabled routes retain ordinary placement.
No velocity term, sampler change, target construction or tolerance is added.

`ResidentAdvectionStep` owns one GPU stop word per particle lane. Encoding clears
it, subsequent stage writes only OR bits, and the owner survives through guard,
commit and the deferred checkpoint. This is scientific fate, separate from all
fatal status regions. On success the guard clears only `FLAG_ACTIVE` on stopped
lanes before downstream physics. The raw-word commit publishes their corrected
state and unchanged mass atomically. The next timestep sees inactive particles
and zero new stop words. Backward source collections skip them. A fatal query
still prevents every published byte, including otherwise valid stopped peers.
There is no deposition reservoir, particle ABI or query/commit layout change.

The production matrix consumes all 64 immutable #213 v2 rows, both directions,
geometries and epochs, alongside v1 disabled controls. Scenarios 1-7 compare exact
f32 projections and stage fate; scenario 8 remains a trajectory diagnostic under
#204. The supplemental 130-lane mixed batch uses three workgroups, an inactive
sentinel and three capacity-tail lanes. Two actual backward calls prove permanent
inactivation without manually toggling a stopped particle. An advanced W query
fault beside a stopped lane proves fatal publication atomicity.

The extended `check_transport_top.py` requires v2 fixture identity, route/source/
shader/input hashes, actual adapter classification, exact stop bits, unchanged
mass/particle words, mixed and two-step evidence. Mutation tests reject missing
rows, altered bits, partial publication and incomplete lifecycle records. Existing
software-WGSL CI runs this extended matrix and auditor in every transport mode.
Hardware evidence must be produced with a recorded physical adapter separately.
The frozen #180/#203/#213 findings and fixture bytes remain unchanged; their
historical status tables describe the state before this integration.

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


For the complete #214 device acceptance, audit both adapter classes together:

```text
python scripts/transport/check_transport_top.py --report <software-report> --software --hardware-report <hardware-report> --oracle-run <fresh-v1-run>
cargo run --example check_gpu_evidence -- <software-fixture-comparison.json>
```

Every individual report also requires all flagged and lifecycle cases. A software
report alone supplies software evidence; the combined command fails if the
required hardware report is absent, stale or incorrectly classified. No physical
GPU performance claim is made from these functional executions.
