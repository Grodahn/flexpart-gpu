# GRIB-packing same-run accumulation decreases (#216)

Status: **decided — `ADMIT_BOUNDED`, not implemented.** A same-run decrease of a
GRIB1 simple-packed LSP/CP accumulation is admissible only inside a bound derived
from both messages' packing metadata, and only as a `+0.0` interval amount. The
canonical schema does not carry that metadata yet. Until
[#219](https://github.com/Grodahn/flexpart-gpu/issues/219) lands, #75/#90 keep
rejecting every same-run decrease, and #207's genuine-input path stays blocked.

Machine-readable decision: [grib-packing-decrease-decision-v1.json](../../fixtures/accumulation/grib-packing-decrease-decision-v1.json).
ecCodes 2.24.2 keys: [grib-packing-eccodes-keys-v1.json](../../fixtures/accumulation/grib-packing-eccodes-keys-v1.json).
Raw input: [the four #202 GRIB files](../../fixtures/accumulation/precipitation-202-accumulated-grib/),
stored as `<trace name>.grib` (SHA-256 equal to `accumulated_input_sha256` of the
#202 evidence, which keys them by trace name).
Regression lock: [tests/grib_packing_decision.rs](../../tests/grib_packing_decision.rs).
Generator: [grib_packing_decision.py](../../scripts/precipitation/grib_packing_decision.py).

Validation level: original-source analysis plus exact bit-level decoding with three
decoders (ecCodes, a stdlib Python parser, a Rust parser). **No GPU execution, no
production change and no parity claim.**

## References

| Reference | Pin | Role |
| --- | --- | --- |
| flex_extract 7.1.2 | `e0005c99`, `EcFlexpart.py` blob `65ba97f7`, `disaggregation.py` blob `12ec74a7` | `deacc_fluxes` differencing (`EcFlexpart.py:1123-1143`); `darain` clip (`disaggregation.py:144-147`) |
| FLEXPART 11.1 | `c70586c2` | Reader clips negative `lsprec`/`convprec` (`windfields_mod.f90:2340,2348`) |
| #202 raw trace | `d16ed0dd…` | Encoder input amounts, decoded inputs, image `flex-extract` `sha256:717f14df…`, ecCodes 2.24.2 |
| #207 oracle | `precipitation-darain-oracle-v1.json` | `deac_vals` before clipping and 234 `darain` results |

## Phase 1 — the three rows

All three rows are reproduced bit for bit from the committed bytes. The table
pairs observation `i-1` (the previous step) with observation `i`. Δ = 3 h,
and both observations share the run's forecast base and declared reset.

| | LSP node (1,0) | CP node (1,0) | CP node (2,1) |
| --- | --- | --- | --- |
| Canonical cell, observation `i` | 1, 9 | 1, 9 | 5, 14 |
| Interval | (27 h, 30 h] | (27 h, 30 h] | (42 h, 45 h] |
| Run: forecast base, steps | 24 h, 3 → 6 | 24 h, 3 → 6 | 36 h, 6 → 9 |
| Encoder input (m), both steps | 0.0009 | 0.000225 | 0.0012 |
| `binaryScaleFactor` E | −32 → −31 | −34 → −33 | −32 → −31 |
| Packed integer X | 3865471 → 1932735 | 3865471 → 1932735 | 5153961 → 2576980 |
| Decoded (m) | 9.000001009553671e-4 → 8.999998681247234e-4 | 2.2500002523884177e-4 → 2.2499996703118086e-4 | 1.2000000569969416e-3 → 1.199999824166298e-3 |
| Decoded difference (m) | −2⁻³² (one previous quantum) | −2⁻³⁴ | −2⁻³² |
| flex_extract `deac_vals` (mm/h) | −7.761021455128987e-8 | −1.9402553637822468e-8 | −7.761021455128987e-8 |
| Canonical `f32` (kg/m2) | 0.90000010 → 0.89999986 | 0.22500002 → 0.22499996 | 1.20000005 → 1.19999981 |
| Canonical difference | −2.384e-7 (−4 ulp) | −5.960e-8 (−4 ulp) | −2.384e-7 (−2 ulp) |
| `darain` input → used | negative → 0.0 | negative → 0.0 | negative → 0.0 |
| #75/#90 | `NegativeDelta { index: 9 }` | `NegativeDelta { index: 9 }` | `NegativeDelta { index: 14 }` |

The canonical decreases are 4, 4 and 2 `f32` ulp, not "1–2 ulp" as first
reported. Each row fails in `validate_interval_metadata`
(`src/meteorology/accumulation.rs`). `CanonicalGpuField::upload` reaches it through
`upload_accumulation → validate_interval_sequence`, so the complete LSP or CP
field fails, and the second guard in `validate_transform_inputs` is never reached.
Had the rows passed, the #90 `f32` arithmetic would give −7.947e-8 mm/h for the two
rows that equal −7.761e-8 in flex_extract. The artifact records this value as a
diagnostic only.

**Precision boundaries:**

- The negative sign is already present in the ecCodes-decoded double values.
- flex_extract's `float64` arithmetic, `(Y_i/0.001 − Y_{i−1}/0.001)/3`, keeps the sign.
- The `f32` conversion cannot create a decrease because round-to-nearest is
  monotone. This holds on all 216 same-run pairs.

**`darain` clipping, established independently:**

1. The source does it: `x[x < 0.] = 0.` for all four inputs before either quotient.
2. Recomputing `darain` with an explicit clip from the pre-clip `deac_vals`
   reproduces all 234 oracle results bit for bit.
3. Without the clip, five values change. Examples: the LSP member at 97200 s
   becomes −1.009e-7, the CP member at 97200 s becomes −2.5e-8, and the 86400 s
   members shift in the eighth digit.
4. Replacing the three rows with `+0.0` reproduces all 234 results.

## Phase 2 — packing mathematics

All 32 messages share these properties:

- GRIB edition 1, ECMWF table 128, paramId 142/143, units `m`;
- 3×3 regular lat/lon grid, no bitmap;
- BDS flag 0 (grid-point simple packing, float data; ecCodes `grid_simple`);
- `bitsPerValue` 24, `decimalScaleFactor` 0, `referenceValue` 0
  (every field contains dry cells);
- `timeRangeIndicator` 0 (ecCodes `stepType` `instant`). The GRIB time range
  therefore does not declare an accumulation: the run identity (accumulation since
  the forecast base) comes from the parameter convention plus `dataDate`/`dataTime`.

ecCodes decodes `Y = (X·2^E + R)·10^-D`. With R = 0 and D = 0 this is exact in
double. The stdlib and Rust parsers equal ecCodes bit for bit.

**Encoding.** The encoder inputs are the #202 trace's fixed interval amounts,
summed left to right in `float64` and divided by 1000. They reproduce all 288
packed integers through `X = floor((v − R)·2^-E + 0.5)`; the largest error is
0.986 of a half quantum. Every message uses the smallest E that fits its range.
E steps up when the field maximum crosses a packing range:

- LSP: 0.003 → 0.006 m;
- CP, first pair: 0.0006 → 0.0012 m;
- CP, second pair: 0.0039 → 0.004125 m.

A flat cell is therefore re-quantized on a coarser grid. Of the 80 truly flat
same-run pairs, 75 stay flat, 2 increase and 3 decrease. 108 of 216 pairs change E.
No decrease occurs where E is unchanged, and no true increase decodes as a decrease.

**Bound.** For one message, `q = 2^E·10^-D` in metres. A faithful quantizer
satisfies two conditions:

- **Q1:** it returns one of the two levels bracketing the true value, so `|v − Y| < q`.
- **Q2:** for fixed (R, E, D), its level choice is monotone in v.

If the true accumulation does not decrease (`v_i ≥ v_{i−1}`), Q1 gives
`Y_i − Y_{i−1} > −(q_{i−1} + q_i)`. By Q2, messages with identical (R, E, D)
cannot decrease at all. GRIB does not record the encoder's rounding mode, so the
round-to-nearest bound `(q_{i−1}+q_i)/2` is reported as a diagnostic only.
`bitsPerValue` does not enter the bound: two 24-bit messages differ in q whenever
their E differ.

At the canonical boundary, `c = f32(Y·1000)` is computed in double. One `f32`
rounding plus at most three `f64` roundings (decode, unit scale) give:

```text
B = 1000 (q_prev + q_cur) + r_prev + r_cur      [kg/m2]
r_k = ulp_f32(c_k)/2 + 2^-50 (|c_k| + ulp_f32(c_k))
admit iff d = c_i − c_{i−1} < 0, (R,E,D) not identical, and −d < B
```

The rows lie at 0.31, 0.31 and 0.29 of `B`, and at 1/3 of the source bound
`q_prev + q_cur`. The bound changes with every message's E, so no constant
epsilon reproduces it.

**What each source of evidence establishes:**

- **Encoded messages:** they only bound the possible error (Q1/Q2).
- **This fixture:** the unquantized values are known. Both observations of each
  row encode the identical `float64` value, and the stored integers follow from it
  and each message's own E. Packing causation is therefore **proven by
  construction**.
- **Decoded sequence alone:** it proves nothing; q is required.
- **Operational GRIB:** the unquantized model value is absent, so a decrease inside
  `B` is only **consistent with** quantization. It is indistinguishable from a
  flat accumulation, never proof. Proof would need the producer's unquantized
  values or an encoder record for both messages, which MARS/GRIB does not provide.

## Phase 3 — candidate policies

| Policy | Genuine #202 example | Misclassification risk | Verdict |
| --- | --- | --- | --- |
| A. Strict rejection (current #75) | LSP and CP uploads fail; 0 of 18 series reach #207 | None, but it rejects decreases that faithful independent packing must produce | Rejected as a permanent policy; kept for every input outside the admitted class |
| B. Evidence-bounded | All 18 series; 3 rows admitted | A real decrease below `B` is admitted as 0. It is below the encoding resolution, and the original chain uses 0 as well | **Selected** |
| C. Unconditional (flex_extract) | All 18 series | Admits 15 of the 29 controls that B rejects or leaves unresolved. These include a hidden 36 h restart (−9 mm, silently clipped by `darain`), a decrease of twice `B`, identical packing parameters and missing provenance | Rejected |

flex_extract trusts message order and differences any pair. Under C, an
undeclared restart or a corrupt message becomes a negative rate that `darain` or
the FLEXPART reader silently zeroes, losing the interval.

The admission is precipitation-specific. Schema v1 admits `AccumulatedSinceReset`
only for LSP/CP, and accumulated surface fluxes are signed quantities whose
decreases are physical. B cannot detect a corruption that stays inside `B`;
message integrity belongs to #32.

## Phase 4 — where the negative value goes

Every original consumer uses `max(deac_vals, 0)`:

- `darain` clips before both quotients;
- the leading persistence and BASETIME writes are clipped by the FLEXPART reader;
- the next interval uses the unmodified lower accumulation (`orig_vals`).

The decision therefore introduces **no signed intermediate**:

- **Canonical #90 products stay non-negative.** An admitted pair yields `+0.0`
  amount, `rate_si` and `rate_mm_per_h`. The next interval is
  `c_{i+1} − c_i` from the unmodified lower `c_i`, with no carry-forward maximum.
  The run total equals the final value plus the projected decreases, each below its `B`.
- **Projection owners:** #75 classifies on the host. #90 WGSL writes
  `max(curr − prev, 0)` for admitted entries only. This matches `darain`'s clip
  exactly, because `darain(…0…)` equals `darain(…negative…)` after clipping.
- **Provenance:** for each admitted (interval, cell), a typed diagnostic records
  the signed canonical difference, `B`, both message hashes and the packing keys.
  It lives outside the cell-invariant `AccumulatedIntervalMetadata`, which upload
  compares across cells. It is never an `AmountKgPerSquareMeter` or a rate newtype.

Consumer audit:

| Consumer | Reads | Effect |
| --- | --- | --- |
| #76 `CanonicalGpuField::prepare_sample` (`AccumulatedInterval`) | Amount, SI rate, mm/h of one interval | Receives `+0.0`; no production caller today |
| #207 `interval_rate_grid`/`encode_interval_rates` | Complete mm/h grid in place | Clip is a no-op on `+0.0`; members equal the original; kernel unchanged |
| #114 wet deposition, #119 sampling | Will consume #76/#207 products | Never see the signed difference |
| Wet scavenging today | Legacy wind interface, not #90 | Unaffected |

## Phase 5 — decision and classes

| Class | Verdict |
| --- | --- |
| LSP/CP `AccumulatedSinceReset`, kg/m2, GRIB1 table 128 paramId 142/143 `grid_simple`, `bitsPerValue ≥ 1`, D = 0, no bitmap, declared conversion, consistent run/message/paramId provenance, (R, E, D) differ, −d < B | `ADMIT_BOUNDED` |
| Any input without canonical packing provenance (all current inputs) | `UNRESOLVED`; runtime rejects (`NegativeDelta` today, fail closed after #219) |
| Identical (R, E, D); −d ≥ B; identity, time, reset, unit, finiteness, message or paramId provenance inconsistency; non-precipitation fields | `REJECT` |
| GRIB1 second-order/complex/spherical, `bitsPerValue` 0, D ≠ 0, bitmap; GRIB2 (any template), NetCDF scale/offset, ERA5 class `EA`, other conversion paths | `UNRESOLVED` (runtime: reject) |

All three genuine rows are `ADMIT_BOUNDED`. The 29 controls (each mutating one
genuine pair) cover:

- the exact bound: 12 `f32` steps are admitted and 13 are rejected against
  `B` = 12.7 steps;
- twice the bound;
- the hidden restart, with consistent and with conflicting provenance;
- identical packing; missing provenance, provenance for another message, and a
  provenance `paramId` that does not match the field;
- D ≠ 0 and unsupported encodings;
- reversed or missing time and reset ambiguity;
- mixed grid or source, a missing observation, wrong units, NaN/∞ and negative
  amounts.

## Missing provenance and follow-up

The canonical `Field` carries unit, sign, time and `f32` values only, and
`Snapshot` provenance is the schema identity. The smallest prerequisite is an
optional per-`Field` source-quantization record:

- encoding, message SHA-256 and `paramId`;
- `bitsPerValue`, E, D, R and the bitmap flag;
- forecast reference and step;
- source unit, canonical scale and conversion path.

#75 must not infer or default any key. #32 populates the record when it decodes
GRIB. [#219](https://github.com/Grodahn/flexpart-gpu/issues/219) owns the record,
the #75/#90 admission and the genuine #202 → #90 → #207 device replay. #207 closes
only on that replay (see #219's closing gate), not on this decision.

## Reproduction

```bash
docker run --rm -v <repo>/fixtures/accumulation/precipitation-202-accumulated-grib:/workspace/grib:ro -v <repo>/scripts/precipitation:/workspace/scripts:ro -v <repo>/fixtures/accumulation:/workspace/out --entrypoint python3 flex-extract:latest -I /workspace/scripts/grib_packing_decision.py eccodes-keys --grib-dir /workspace/grib --out /workspace/out/grib-packing-eccodes-keys-v1.json
```

```bash
python -I scripts/precipitation/grib_packing_decision.py build --repo . --revision <commit> --out fixtures/accumulation/grib-packing-decrease-decision-v1.json
```

```bash
cargo test --test grib_packing_decision
```
