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
  #216 decided one bounded exception for GRIB-packed LSP/CP; it is not active
  until [#219](https://github.com/Grodahn/flexpart-gpu/issues/219) (see
  [below](#same-run-decreases-from-grib-packing-216)).
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
silent rows. The report also carries an overall verdict: an empty source
sequence, incomplete oracle-reference coverage, or any failed derivation or
requested oracle comparison makes the report fail closed. Omitting oracle
references entirely is the explicit derivation-only report mode.

## Same-run decreases from GRIB packing (#216)

Decision: **`ADMIT_BOUNDED`, not implemented.** See the
[research note](research/grib-packing-accumulation-216.md) and the
[decision artifact](../fixtures/accumulation/grib-packing-decrease-decision-v1.json).
The canonical schema does not yet carry the packing metadata the rule needs, so
the "same-run negative delta" rejection above still applies to every input.
[#219](https://github.com/Grodahn/flexpart-gpu/issues/219) owns the carrier, the
#75/#90 change and the genuine #202 → #90 → #207 replay.

Independent per-message GRIB quantization can make a flat accumulation decrease.
Pinned flex_extract differences such a pair, and every consumer of the result
clips it to zero (`darain`, the FLEXPART reader). The rule #219 implements:

- **Supported class:** LSP/CP `AccumulatedSinceReset` in kg/m2, from GRIB1
  table 128 paramId 142/143 in metres, `grid_simple`, `bitsPerValue >= 1`,
  `decimalScaleFactor` 0, no bitmap. The canonical value is
  `f32_nearest(Y * 1000)` computed in double.
- **Required provenance**, on both observations: message SHA-256, `paramId`
  (LSP 142, CP 143), E, D, R, `bitsPerValue`, bitmap flag, forecast reference
  `== R_i`, reference + step `== t_i`, source unit, scale and conversion path.
  Nothing may be defaulted.
- **Pair:** consecutive declared observations of one cell with
  `R_{i-1} == R_i < t_{i-1}`.
- **Bound** in kg/m2: `B = 1000 (2^E_prev + 2^E_cur) + r_prev + r_cur`, with
  `r_k = ulp_f32(c_k)/2 + 2^-50 (|c_k| + ulp_f32(c_k))`. It holds for any
  quantizer that returns one of the two levels bracketing the true value. It
  depends only on the two messages and the two canonical values; it is not a
  tolerance.
- **Admission:** `d = c_i - c_{i-1} < 0`, (R, E, D) differ between the messages,
  and `-d < B`. Identical (R, E, D) cannot produce a decrease, so such a pair is
  rejected. So are `-d >= B` and any identity, time, reset, unit, message or
  `paramId` inconsistency. Missing provenance (all current inputs), other
  encodings, D != 0 and GRIB2 remain unresolved and are rejected.
- **Representation:** an admitted interval has amount and rates `+0.0`; no #90
  buffer holds a negative value.
  - The next interval uses the unmodified lower `c_i`, as flex_extract does. The
    run total exceeds the final value by the admitted decreases, each below its `B`.
  - The signed difference and `B` are kept as typed diagnostics outside the
    cell-invariant interval metadata.
  - #76 and #207 consume the result unchanged.

For the genuine #202 example this admits all three rows, at 0.29–0.31 of `B`.
Packing causation is proven there by construction. For operational GRIB a
decrease inside `B` is only consistent with packing.

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
- `fixtures/accumulation/precipitation-member-mapping-v1.json` — #202 decision:
  these interval-mean rates cannot directly populate FLEXPART `numpf=1`
  precipitation members (verdict INCOMPATIBLE; the missing `RRINT=0`
  reconstruction is #207). The `rain-layer-fields-bilinear-rates` memory-time
  association is audited there as arithmetic-only evidence. See
  [the research note](research/precipitation-interval-mapping-202.md).
- [Precipitation members (#207)](precipitation-members.md): GPU `darain`
  reconstruction that consumes the complete #90 mm/h rate grid in place. The
  genuine #202 GRIB example contains packing-induced same-run decreases that
  this contract rejects until #219 implements the #216 decision.
- `fixtures/accumulation/grib-packing-decrease-decision-v1.json` — #216
  decision, raw GRIB metadata, offending rows, bound and negative controls;
  locked by `tests/grib_packing_decision.rs`.
