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
- same-run negative delta (decreasing accumulation without a declared reset),
  except a #216 `ADMIT_BOUNDED` pair with source records (see
  [below](#same-run-decreases-from-grib-packing-216));
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

**1. Decision.** `ADMIT_BOUNDED`, frozen by #216: the
[research note](research/grib-packing-accumulation-216.md) and the
[decision artifact](../fixtures/accumulation/grib-packing-decrease-decision-v1.json).
Independent per-message GRIB quantization can make a flat accumulation decrease.
Pinned flex_extract differences such a pair, and every original consumer clips it
to zero (`darain`, the FLEXPART reader). #219 implements the rule below without
widening it.

**2. Implemented envelope (#219).**

- **Carrier:** an optional per-`Field` `source_quantization` record
  (`src/meteorology/quantization.rs`) with `source_id`, `source_message_sha256` and the
  #216 `packing` keys under their artifact names: `encoding`, `message_sha256`,
  `mars_class`, `paramId`, `bitsPerValue`, `binaryScaleFactor` (E),
  `decimalScaleFactor` (D), `referenceValue` (R, the GRIB packing reference, not the
  reset), `bitmap_present`, `forecast_reference_s`, `step_s`, `source_unit`,
  `canonical_per_source_unit` and `canonical_conversion`. It is absent for every
  existing input and is never defaulted or inferred. The MARS class comes from
  provider/control provenance only.
- **Classifier:** `classify_packing_decrease`
  (`src/meteorology/accumulation/packing_decrease.rs`) keeps the rejection order of
  `grib_packing_decision.py::classify`. `d = c_i - c_{i-1}` and
  `B = 1000 (2^E_prev + 2^E_cur) + r_prev + r_cur`, with
  `r_k = ulp_f32(c_k)/2 + 2^-50 (|c_k| + ulp_f32(c_k))`, are evaluated exactly on
  dyadic rationals from the uploaded `f32` values. Admission requires `d < 0`,
  `(R, E, D)` not identical and `-d < B` (strict).
- **Supported class:** LSP/CP `AccumulatedSinceReset` in kg/m2; GRIB1
  `grib1_grid_simple`; explicit `mars_class = OD`; `paramId` 142 (LSP) or 143 (CP);
  `bitsPerValue >= 1`; `D = 0`; no bitmap; forecast reference `== R_i` and
  reference + step `== t_i`; source unit `m`; scale `1000`; conversion
  `f64_decode_unit_scale_then_f32_nearest`; pair `R_{i-1} == R_i < t_{i-1}` with the
  same grid and source identity.
- **Entry points:** `validate_interval_sequence_with_sources`,
  `resolve_interval_sequence_with_sources` and `build_accumulation_report_with_sources`
  take an `AccumulatedSeries` (field, unit, grid identity, cell, observations, one
  optional record per observation). The original functions are unchanged.

**3. Rejected input classes.** Each fails closed before dispatch:

| Input | Result |
| --- | --- |
| No record on either observation (all current providers) | `NegativeDelta`, unchanged |
| Record on one observation only | `PackingDecreaseRejected`, `REJECT/identity_mismatch` |
| Missing, `EA` or other MARS class | `UNRESOLVED/unsupported_source_class` |
| GRIB2, second-order/complex/spherical, `bitsPerValue = 0`, `D != 0`, bitmap | `UNRESOLVED/unsupported_packing` |
| Other conversion, or a value that is not exactly the uploaded `f32` | `UNRESOLVED/unsupported_conversion` |
| Packing hash differs from the observation message, or wrong `paramId` | `REJECT/packing_provenance_mismatch` |
| Reference or step inconsistent with the declared reset/valid time | `REJECT/run_identity_mismatch` |
| Source unit not `m` or scale not `1000` | `REJECT/unit_mismatch` |
| Identical `(R, E, D)` | `REJECT/identical_packing_parameters` |
| `-d >= B` | `REJECT/decrease_exceeds_quantization_bound` |
| Malformed record (digest, non-finite R, ranges, unknown key) | `ContractError::InvalidSourceQuantization` at snapshot validation |

The rule cannot bind a hash to message bytes; that integrity belongs to the #32 decoder.

**4. Diagnostic representation.** An admitted pair yields
`AmountKgPerSquareMeter(+0.0)`, `RateKgPerSquareMeterPerSecond(+0.0)` and
`RateMillimeterPerHour(+0.0)`. The signed difference lives only in a typed
`PackingDecreaseAdmission`: interval and cell index, interval bounds, both uploaded
`f32` values, `d` and `B` (exact fraction and nearest `f64`), both source records
(hashes and packing keys) and the decision identity. It is never an amount or rate.
`AccumulatedIntervalMetadata` is unchanged and remains cell-invariant. Report rows
pass and carry the admission. The next interval differences from the unmodified lower
value, so per declared run `sum(interval amounts) = final total + sum(projected
decreases)`, each decrease below its `B`.

On the device (#90) the host classifies the exact uploaded `f32` values and marks the
entry `reset_applied = 2` (`TRANSFORM_MODE_PACKING_ADMISSION`; never a reset; layout
unchanged). WGSL writes `max(curr - prev, 0) = +0.0` amount and `+0.0` rates. Mode 2
reaches a buffer only through `ClassifiedTransformInputs`, whose constructors run the
classifier. Validation requires exactly one matching admission per position and
`curr < prev`; raw `AccumulatedTransformInputs::from_inputs` rejects mode 2.
`CanonicalGpuField::quantization_admissions` and `IntervalRateGrid::quantization_admissions`
expose the list. #207 copies it into `PrecipitationSourceIdentity.quantization_admissions`,
which is part of the member input hash. #76 `prepare_sample` returns `+0.0` for all three
quantities. Neither #76 nor #207 adds a clip, and the #207 kernel is unchanged.

For operational GRIB a decrease inside `B` is only consistent with packing; it does not
prove that the field is monotonic. For the genuine #202 example, causation is proven by
construction.

**5. Genuine GPU production acceptance.** The genuine #202 messages are uploaded
through `CanonicalGpuField::upload` with records read from the committed bytes and
cross-checked against the #216 artifact (`tests/support/grib202.rs`). #90 and #207 run
in one encoder and one submission. All 18 series equal `max(deac_vals, 0)` (+0.0 for
the 3 admissions) and all 234 members equal the original `darain` under the #71
policy, on hardware and software WGSL adapters. See
[precipitation members](precipitation-members.md#evidence).

**6. Remaining responsibilities.** #32 decodes GRIB1 LSP/CP and populates the record.
#119 samples the members in time. GRIB2 (#211), `D != 0`, other packings and ERA5
`EA` remain unsupported.

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
  three packing-induced same-run decreases of the genuine #202 GRIB example are
  admitted by the #216 rule (#219).
- `fixtures/accumulation/grib-packing-decrease-decision-v1.json` — #216
  decision, raw GRIB metadata, offending rows, bound and negative controls;
  locked by `tests/grib_packing_decision.rs`. `tests/accumulation_contract.rs`
  replays all 32 controls through the production classifier and checks the three
  genuine admissions against the exact `B` of the artifact.
