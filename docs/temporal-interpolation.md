# Canonical temporal interpolation (#74)

Status: implementation contract for issue #74 on top of the frozen #71 oracle
semantics. The normative primitive is `docs/interpolation-contract.md` section 4
(`find_time_vars` + `temporal_interpolation`, pinned FLEXPART 11.1
`c70586c2b7f5258850705325881c61f557ea9bd8`).

## Scope

Provides `flexpart_gpu::meteorology::temporal`:

- `sample_field(FieldId, &[&Snapshot], RequestedSampleTime) -> Result<TemporalSample, TemporalError>`
- `extract_series` validation, bracketing-pair selection, weight computation
- machine-readable comparison report types and `build_comparison_report`
- `src/bin/temporal-interpolation-report.rs` report driver

The report driver accepts scenario schema version 1 only and validates every
loaded snapshot against the canonical #29 contract before comparison.

Only instantaneous fields are in scope. Static fields, accumulated-field rate
sampling and precipitation `interpol_rain` semantics are owned elsewhere (#75)
and fail closed here.

Eligibility is explicit per field identity (`instantaneous_sampling_basis`):
every `TemporalPolicy::Instantaneous` field (`instantaneous_state`) plus the four
`INSTANTANEOUS_SURFACE_FLUX_FIELDS` approved by #120 (`point_in_time_surface_flux`,
see [below](#surface-flux-eligibility-120)). The resolved bracket and #76 handoff
metadata carry that basis. Every series member must still be stored as
`TemporalKind::Instantaneous` without interval or accumulation metadata.

## Semantics

For snapshots at strictly increasing times `t_0 < ... < t_n` and a request `t`:

1. Validate the series (`extract_series`): instantaneity, coverage, calendar
   consistency, finite values, monotonic timestamps, equal shapes/staggering.
2. Select the leftmost bracketing pair `(i, i+1)` with `t_i <= t <= t_{i+1}`.

   - exact interior timestamp `t = t_k` with `k < n`: pair `(k-1, k)` and the
     snapshot value is returned unchanged (weights `dt1 = span`, `dt2 = 0`);
   - exact first/last timestamp: inclusive edge pair `(0,1)` / `(n-2, n-1)`;
   - strict interior point: `dt1 = t - t_i`, `dt2 = t_{i+1} - t`,
     `output = (v_i * dt2 + v_{i+1} * dt1) / (dt1 + dt2)`.

3. Report the application kind: `FirstEndpoint`, `LastEndpoint`,
   `ExactSourceTimestamp`, or `LinearInterior`.

## Fail-closed behavior

Requests outside `[t_0, t_n]` are rejected with `BeforeFirstCoverage` /
`AfterLastCoverage`. This is the explicit #74 policy divergence documented in
`docs/interpolation-contract.md` section 4: the raw FLEXPART primitives
perform linear extrapolation without any range guard, while #74 never
extrapolates.

Other errors: `UnsupportedTemporalPolicy`, `InsufficientTemporalCoverage`,
`MissingFieldSnapshot`, `NonInstantaneousSnapshot`,
`AmbiguousInstantaneousMetadata`, `InconsistentCalendar`,
`CalendarMismatch`, `NonMonotonicTimestamps` (duplicates and descending),
`InconsistentFieldShape`, `InconsistentStaggering`, `NonFiniteValue`,
`OutOfBoundsElement`, `EmptyOracleQueries`, `NonFiniteResult`.

## Evaluation evidence

- Frozen #71 `temporal-bilinear` case reproduces at ITIME 0 / 1800 / 3600
  (values 10 / 15 / 20, exact closed-form `dts`), plus a recomputation from the
  frozen closed form.
- Synthetic time-linear temperature field matches hand-computed values to
  relative tolerance 1e-5 where f32-representable.
- Fail-closed matrix covers each rejection class; report driver runs
  end-to-end on both scenario fixtures and emits the machine-readable report
  schema `flexpart-gpu.temporal-interpolation-report` v1.
- 8/8 tests in `tests/temporal_interpolation.rs`.

## Surface-flux eligibility (#120)

Decision: the canonical `Instantaneous` representation of all four
`SurfaceFluxRate` fields is `INSTANTANEOUS_SUPPORTED` for #74/#89 sampling.
`IntervalMean` remains `INTERVAL_REPRESENTATION` (#76 exact-interval #87 path
only); accumulated energy/momentum is invalid at the canonical boundary. No
field is `UNRESOLVED`. Authority: pinned FLEXPART 11.1
`c70586c2b7f5258850705325881c61f557ea9bd8` and its normative preprocessor,
flex_extract 7.1.2 `e0005c99ac81d12faa45a8ff799debbd592b0dc0`
([pin](../reference/flex-extract.json)).

### Source crosswalk

`original source -> preprocessing -> canonical representation -> temporal semantics -> original consumer`

| Canonical field | Original source and reader | Preprocessing | Canonical units/sign | Original consumer |
| --- | --- | --- | --- | --- |
| `sensible_heat_flux` | ECMWF SSHF, GRIB 146 -> `readwind_ecmwf` `sshf(:,:,1,n)` (`windfields_mod.f90:2351-2361`); missing flux -> `pbl_profile` estimate clamped to [-400, 200] (`2583-2609`) | flex_extract de-accumulation + `dapoly` point value | W/m2, upward positive = `-sshf`: FLEXPART keeps ECMWF downward-positive, e.g. `obukhov` `thetastar=hf/(rhoa*cpa*ustar)` (`getfields_mod.f90:1449-1451`) | `calcpar(n)` -> `obukhov`, `richardson` (`getfields_mod.f90:997-1031`) |
| `surface_solar_radiation` | ECMWF SSR, GRIB 176 -> `ssr(:,:,1,n)`, negative values clipped to 0 (`2362-2368`) | same | W/m2, non-negative after the clip | `calcpar(n)` -> `getvdep` `gr` (`getfields_mod.f90:1064-1067`, `drydepo_mod.f90:1010,1518-1533`) |
| `surface_stress_eastward` | ECMWF EWSS, GRIB 180 -> `readwind_ecmwf` local `ewss` (`2369-2378`); zero-pair neighbour fill (`2543-2578`) | same | N/m2, eastward positive | only `sfcstress=sqrt(ewss**2+nsss**2)` (`2579`) -> `scalev` `ustar` (`getfields_mod.f90:978-981`) |
| `surface_stress_northward` | ECMWF NSSS, GRIB 181 -> local `nsss` (`2379-2387`); same fill | same | N/m2, northward positive | as EWSS |

flex_extract `EcFlexpart.deacc_fluxes` (`EcFlexpart.py:1123-1143`) converts the
ECMWF accumulations (J/m2, N m-2 s) to interval means by `/3600/dtime`.
`disaggregation.dapoly` (`disaggregation.py:62-103`) then combines four
consecutive means with weights `-1/12, 7/12, 7/12, -1/12` into a value at
their central boundary. It is written with `stepRange 0` at that validity time
(`EcFlexpart.py:1180-1211`) and documented as the point for subsequent linear
interpolation. `readwind_ecmwf` reads it unscaled at `memtime(n)`. The four
fields therefore share one native temporal representation: a point value at the
GRIB validity time, not an interval mean.

### Memory selection and consumer order

`getfields` holds two memory slots and reads `memind(2)` once `itime` reaches
`memtime(2)` (`getfields_mod.f90:159-203`). Each slot is consumed only at its own
validity time by nonlinear `calcpar` diagnostics (`ustar`, `oli`, `hmix`, `wstar`,
`vdep`). Only those derived quantities are time-interpolated
(`interpol_mod.f90:706-737`, `1119-1158`; `drydepo_mod.f90:805-814`). The pinned
production model never time-interpolates the raw fluxes. Consumers that reproduce
FLEXPART order (#113, #114) must request exact source timestamps
(`first_endpoint`, `exact_source_timestamp`, `last_endpoint`). These return the
snapshot value bit-exactly. A `linear_interior` raw-flux sample is a valid linear
interpolation of point values. It is not FLEXPART's PBL or deposition operation
order and supports no consumer parity claim.

### Accepted representation and fail-closed boundaries

Accepted: one of the four approved field identities. Each series member uses
`TemporalKind::Instantaneous`, no interval/accumulation metadata, one calendar,
strictly increasing timestamps and the canonical unit/sign. The unchanged #74
endpoint, coverage, duplicate, calendar and non-finite rules apply. `IntervalMean`,
`IntervalTotal`, `AccumulatedSinceReset` and `Static` members fail with
`NonInstantaneousSnapshot`. Instantaneous members carrying interval/accumulation
metadata fail with `AmbiguousInstantaneousMetadata`.

Provider handoff (#32): adapters must negate SSHF and keep the SSR clip. They
must apply the stress fill per snapshot before canonicalization. Only interior
`dapoly` reconstructions may be labeled `Instantaneous`. flex_extract's
period-edge records are an interval mean (first) or a reused reconstruction
(last two) (`EcFlexpart.py:1193-1250`). They must be labeled `IntervalMean` or
rejected. The canonical metadata cannot detect that mislabeling.

### Evidence

- Direct oracle: `python scripts/interpolation/run_surface_flux_temporal_oracle.py --checkout <pristine-flexpart> --flex-extract <pristine-flex_extract> --output-dir target/<new-dir>`
  builds pristine FLEXPART in the pinned Fortran image. It loads the four fields
  into memory slots (`sshf`/`ssr` module arrays; EWSS/NSSS driver arrays of the
  same shape) and calls genuine `find_grid_indices`, `find_grid_distances`,
  `find_time_vars`, `hor_interpol_4d` and `temporal_interpolation`. Linked call
  edges, sources, pins and artifacts are hashed. Frozen raw/decoded evidence:
  [`surface-flux-temporal-v1`](../fixtures/interpolation/surface-flux-temporal-v1/report.json),
  308 records over 3 snapshots and 4x3 nodes. Coverage spans first, interior,
  exact-interior and last times plus 5 fractional positions. The technical
  validation gate rebuilds the oracle fresh and fails unless its raw input,
  raw output, decoded records and provenance equal the frozen fixture, so the
  device comparison against the fixture also binds the fresh run.
- Device: `cargo test --test surface_flux_temporal_gpu -- --test-threads=1` runs
  #89 at every node and #76 #87 -> #89 at fractional positions in one caller
  encoder. It applies the #74/#89 policy (abs `1e-6` OR rel `1e-5`) and writes
  `target/ci-gate/surface-flux-temporal-gpu/report.json` with per-field
  PASS/FAIL/UNSUPPORTED verdicts. The audit is
  `scripts/interpolation/check_surface_flux_temporal_gpu.py [--software]`.
- Validation level: paired direct pinned-routine evidence on the fields' memory
  slots. It does not show PBL, turbulence or deposition parity.
