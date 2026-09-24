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
`MissingFieldSnapshot`, `NonInstantaneousSnapshot`, `InconsistentCalendar`,
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
