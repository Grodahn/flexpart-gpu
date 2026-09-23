# Accumulated-field interval/reset contract

Status: normative contract for issue #75 ("RISK-03.3G-10c5: accumulated-field
interval/reset semantics").

Parent: #31 (meteorology interpolation epic)
Depends on: #71 (interpolation oracle), #29 (canonical meteorology schema).

## Boundary

#75 owns the canonical transformation of accumulated meteorological observations
plus explicit interval/reset metadata into derived **interval amounts** and
**interval rates**. It does **not** own:

- temporal interpolation at member-question times (#74);
- horizontal/vertical spatial interpolation (#73);
- the composition/sampling API that consumes rates (#76);
- provider decoding/normalization of raw fields into the canonical schema (#32);
- provider assumptions such as "ERA5 resets every N hours" — #75 takes explicit
  per-field reset/interval metadata and fails closed when it is missing or
  internally inconsistent.

Rate handoff surface: `RateMillimeterPerHour` for the interpolation path that
consumes already-normalized mm/h rates (FLEXPART `interpol_rain`), plus
`RateKgPerSquareMeterPerSecond` SI for any other consumer. The handoff from
`1 kg/m^2 == 1 mm` water depth uses an exact `3600` factor; interval rates are
derived per declared interval, never from a single sample divided by an assumed
constant window.

## Old issue: accumulated value, new issue: interval amount/rate

FLEXPART 11.1 `interpol_mod.f90:1209-1582` (`interpol_rain`) consumes
`lsprec`/`convprec` as already-normalized mm/h rates:
`reset_deaccumulation` is `not_performed_here` and is owned by #75. The oracle
`dtt = dt/3` drift quirk (`interpol_mod.f90:1302-1316`) is a sampling detail of
#74/#76 and must **not** be emulated here as a deaccumulation rule. #75 begins
from the canonical schema's `AccumulatedSinceReset` fields and produces the
`mm/h` grids that the #71 oracle already assumed.

## Deterministic derivation

Given a monotonically increasing sequence of observations
`(valid_time t_i, reset start R_i, accumulated amount a_i)` for one field/cell:

- Leading interval `[R_0, t_0]` carries amount `a_0`.
- Same-run (`R_i == R_{i-1}`): interval amount is the delta `a_i - a_{i-1}`.
- Contiguous declared reset at `R_i == t_{i-1}`: interval `[t_{i-1}, t_i]`
  carries amount `a_i` as a fresh run.
- Non-finite or negative amounts, and negative same-run deltas, fail closed.
- For a single-run sequence the sum of derived interval amounts telescopes to
  the final accumulated source value, so interval-integrated precipitation is
  preserved to `1e-6` relative in canonical tests.

## Fail-closed rules

The resolver rejects, with no partial interval emitted:

- empty sequences;
- non-increasing valid times;
- backwards resets (`R_i < R_{i-1}`);
- reset at or after its valid time (`R_i >= t_i`);
- mid-window reset (`R_i < t_{i-1}`): ambiguous overlap;
- forward reset leaving an uncovered gap (`t_{i-1} < R_i < t_i`);
- same-run negative delta (decreasing accumulation without a declared reset);
- negative or non-finite accumulated amounts.
- observations following a failed observation record a dependency failure
  instead of deriving values from rejected input.

Every rejection is a typed `AccumulationError`; the machine-readable report
carries a per-interval `verdict` (`passed`/`failed` + reason) instead of dropping
silent rows.

## Evidence

- `fixtures/accumulation/contract-v1.json` — canonical fixture: per-cell
  accumulated inputs, expected interval amounts and mm/h rates, the declared
  reset scenario, and the fail-closed scenarios. Digest pinned in
  `tests/accumulation_contract.rs`.
- The `rain-layer-fields-bilinear-rates` case reproduces the #71 golden sample
  (LSP `159.75` mm/h, CP `3.0` mm/h at `(1.25, 0.5)`) by feeding the resolver's
  rate grids through the frozen #71 closed-form bilinear arithmetic with
  `dtt = dt/3`.
- `tests/accumulation_contract.rs` — integration contract test, mirrors
  `tests/interpolation_contract.rs` conventions and pins the fixture digest.
- Module unit tests in `src/meteorology/accumulation.rs` cover hand-computed
  sequences for every derivation and rejection rule above.