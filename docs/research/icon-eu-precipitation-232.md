# ICON-EU GRIB2 precipitation admission (#232)

Issue: [#232](https://github.com/Grodahn/flexpart-gpu/issues/232) (MET-INGEST-32-ICON-PRECIP),
parent [#32](https://github.com/Grodahn/flexpart-gpu/issues/32) via the
[#230 provider audit](icon-eu-provider-230.md), tracking #12.

Status: **decided.** DWD ICON-EU run accumulations are admissible as canonical
`large_scale_precipitation` / `convective_precipitation`
(`AccumulatedSinceReset`, kg m-2):

| Question | Verdict |
| --- | --- |
| Units | `AMOUNT_KG_PER_SQUARE_METRE`: the decoded value is the amount, scale 1 |
| Composition | `ADMIT_COMPONENT_SUM_WITH_CLOSURE`: LSP = `RAIN_GSP + SNOW_GSP`, CP = `RAIN_CON + SNOW_CON`, with a per-cell TOT_PREC closure check |
| Quantization | `ADMIT_BOUNDED`: a same-run CCSDS decrease inside `B` becomes a `+0.0` interval |

Nothing is implemented. #75/#90/#219 still reject every ICON-EU input. Adoption is
owned by [#240](https://github.com/Grodahn/flexpart-gpu/issues/240).

- Machine-readable decision: [decision-v1.json](../../fixtures/meteorology/icon-eu-precipitation-232/decision-v1.json).
- Evidence: [evidence-v1.json](../../fixtures/meteorology/icon-eu-precipitation-232/evidence-v1.json).
- Raw subset: [raw/](../../fixtures/meteorology/icon-eu-precipitation-232/raw/) holds `SNOW_GSP` steps 0–3, 47, 48 and `SNOW_CON` 47, 48. `SNOW_CON` 0–3 are already in the [#230 raw fixtures](../../fixtures/meteorology/icon-eu-audit-v1/raw/). All are unmodified DWD bytes (CC BY 4.0, *Source: Deutscher Wetterdienst*).
- Generator: [icon_eu_precipitation_decision.py](../../scripts/provider/icon_eu_precipitation_decision.py).
- Self-test: [test_icon_eu_precipitation_decision.py](../../scripts/provider/test_icon_eu_precipitation_decision.py).
- Rust lock: [tests/icon_eu_precipitation_decision.rs](../../tests/icon_eu_precipitation_decision.rs).

Validation level: normative-source analysis plus real-source decoding with two
decoders. **No GPU execution, no production change and no FLEXPART 11.1 parity
claim.** Pinned FLEXPART has no ICON reader, so no oracle exists for this path.

## References

| Reference | Pin | Role |
| --- | --- | --- |
| WMO GRIB2 code and template tables | [wmo-im/GRIB2](https://github.com/wmo-im/GRIB2) `ad61905d`; file SHA-256 in the artifact | Code table 4.2-0-1 rows 52/55/56/75/76/77; code table 4.10 note 92; table notes 2 and 23; template 5.42 note 83 |
| DWD ICON database reference, version 2.5.6 (2026) | [PDF](https://www.dwd.de/DWD/forschung/nwv/fepub/icon_database_main.pdf), SHA-256 `ef804f22…` | Table 5.6 (pp. 39–41) and footnote 5 (p. 43); TOT_PREC definition (p. 53); ICON-EU is the global run's nest (p. 6). Cited only; CC BY-NC-ND |
| FLEXPART 11.1 | `c70586c2` | GFS LSP = total − convective (`windfields_mod.f90:2936-2945, 3323-3343`); ECMWF LSP/CP reader (`2335-2349`); the split is used by `get_gridfract` (`wetdepo_mod.f90:681-716`) |
| #230 sample | `retrieval-manifest.json`, `decoded-evidence.json` | 45 precipitation messages of cycle 2026-10-31 00 UTC |
| #216 decision | `grib-packing-decrease-decision-v1.json` | GRIB1 bound, projection and rule order that this decision mirrors |

## 1. Units

All five messages are WMO code table 4.2-0-1 **rate** parameters (kg m-2 s-1):

- `TOT_PREC`: 52, total precipitation rate;
- `SNOW_CON`: 55, convective snowfall rate water equivalent;
- `SNOW_GSP`: 56, large-scale snowfall rate water equivalent;
- `RAIN_CON`: 76, convective rain rate;
- `RAIN_GSP`: 77, large-scale rain rate.

Each message uses PDT 4.8 with `typeOfStatisticalProcessing = 1` (accumulation)
from `forecastTime = 0`.

**Normative rule.** WMO code table 4.10, note 92 (on code 1, accumulation),
states that the accumulated value Y carries the code table 4.2 unit multiplied
by second. WMO note 2 deprecates the amount parameters 8/9/10 for this reason
and recommends a rate parameter with accumulation in the PDT, which is the DWD
encoding. The amount is therefore kg m-2 s-1 × s = **kg m-2**.

**DWD rule.** DWD Table 5.6 lists all six ICON fields as "accumulated since
model start" in kg m-2. Footnote 5 explains the mismatch: grib_dump shows
kg m-2 s-1 as GRIB2 requires, and the amount unit is that unit times s.

**Labels are not evidence.** For bit-identical values, ecCodes 2.24.2 labels
`RAIN_GSP` `kg m-2` (DWD local paramId 500134), while 2.48.0 labels it
`lsrr` `kg m**-2 s**-1`. Identity stays on raw octets, as #230 froze it.

**Magnitude check.** The 48 h `RAIN_GSP` maximum is 187.37 kg m-2. Read as a
rate it would be 674,536 mm/h.

The decoded value is the canonical amount. No ÷1000 (the #211 GRIB2 CP class),
×1000 (GRIB1 metres) or ×3600 conversion applies.

## 2. Composition and graupel

**Mapping:**

- `large_scale_precipitation` = `RAIN_GSP (0,1,77) + SNOW_GSP (0,1,56)`;
- `convective_precipitation` = `RAIN_CON (0,1,76) + SNOW_CON (0,1,55)`.

The canonical value is `c = f32(Y_rain + Y_snow)`. The `f64` sum is exact (§3).

**Why this is the FLEXPART split.** Both models split precipitation by the
producing scheme:

- ICON: GSP is the grid-scale microphysics, CON the convection parameterization;
- ECMWF: LSP comes from the cloud scheme, CP from the convection scheme.

Pinned FLEXPART defines LSP operationally as all non-convective precipitation.
Its GFS GRIB2 path sets `lsprec = total − convective` and zeroes the result
where convective ≥ total (`windfields_mod.f90:3323-3343`). The split matters,
because `get_gridfract` weights LSP and CP area fractions separately.

For global ICON runs with and without nests, DWD (p. 53) defines
`TOT_PREC = RAIN_GSP + SNOW_GSP + RAIN_CON + SNOW_CON`. ICON-EU is that nest
(p. 6). GSP rain + snow therefore equals total − convective, which is the
FLEXPART meaning. WMO note 23 gives the same total = convective + large-scale
convention.

This is an analogue by definition, not identical physics. The partition depends
on each model's schemes, and no parity claim follows.

**Graupel.** DWD adds `GRAU_GSP` (0,1,75) to TOT_PREC only for ICON-D2.
ICON-EU publishes no graupel field and needs none. An unpublished component
would show as a positive TOT_PREC residual, and no cell exceeds the closure
bound positively at any sample step.

**Closure rule (fail closed).** For each step, every cell must satisfy

```text
|TOT_PREC − (RAIN_GSP + SNOW_GSP + RAIN_CON + SNOW_CON)| ≤ q_TOT + Σ q_component
```

The check uses exact `f64` decoded values. By Q1 (§3), each message is within
one quantum of its true value, and DWD defines the true values to close. The
bound is inclusive, so the all-zero run-start anchor (all q = 0) passes.

- A violation in any cell rejects LSP and CP for that step (`component_closure_violation`).
- A missing TOT_PREC leaves the step `UNRESOLVED`.
- Nothing is masked or clipped.

**Sample result:**

| Steps | Closure | Step admission |
| --- | --- | --- |
| 0, 1, 2, 3, 47, 48 | every cell within the bound | `ADMIT` |
| 78, 81, 84 | 3 cells beyond the bound, all negative | `REJECT` |

The three cells lie at the eastern domain edge, 69.50–69.56° N and
62.44–62.50° E (columns 1375–1376 of 1377). Their residuals are −0.037,
−0.056 and −0.030 kg m-2, so the components exceed TOT_PREC by up to 3.3 times
the 0.0171 kg m-2 bound. The residuals are identical at 78, 81 and 84 h, so
they formed between 48 and 78 h. The sample has no steps in between, and the
cause is not established. #230 also flagged a fourth cell at 78 h; it is beyond
the half-quantum diagnostic but inside this bound.

## 3. CCSDS quantization (template 5.42)

**Packing identity.** WMO template note 83 states that 5.42 scales and
references the data exactly as template 5.0, `Y·10^D = R + X·2^E`
(Regulation 92.9.4, Note 4). The resulting integers X are then coded with the
lossless CCSDS 121.0 standard. Every sample message has `R = 0`, `D = 0` and no
bitmap. Each uses 16 bits, except the run-start fields, which use 0 bits and
are all zero.

**Bound.** The derivation is the #216 one, with q = 2^E·10^-D in kg m-2:

- **Q1:** a faithful quantizer returns one of the two bracketing levels,
  `|v − Y| < q`.
- **Q2:** for fixed (R, E, D), its level choice is monotone in v.

Take a pair whose true component accumulations do not decrease. A component
with identical (R, E, D) cannot decrease. A component with different keys
decreases by less than `q_prev + q_cur`.

In the admitted class, R = 0, D = 0 and `bitsPerValue ≤ 24`. Then every
`Y = X·2^E` is exact in `f32` and `f64`, and the `f64` sum of two components is
exact. The only further rounding is the `f32` conversion of each canonical sum:

```text
B = Σ_{k: (R,E,D) differ} (q_prev,k + q_cur,k) + ulp_f32(c_prev)/2 + ulp_f32(c_cur)/2   [kg m-2]
admit iff d = c_cur − c_prev < 0, not all components have identical keys, and −d < B
q = 0 for the bitsPerValue 0 run-start anchor
```

GRIB does not record the encoder's rounding mode, and the bound does not
assume one. Unlike #216, no `2^-50` terms are needed: decoding involves no
unit scale, and the sum is exact.

**Diagnostic tight bound.** With R = 0, nested power-of-two levels and
round-to-nearest, a component decrease is at least `−max(q_prev, q_cur)/2`.
The sample attains exactly this value for every component pair with decreases:

- one previous quantum when E rises by 1, two when E rises by 2;
- no decrease when (R, E, D) are identical (18 of 40 component pairs).

This is consistent with round-to-nearest but is reported as a diagnostic only.

| Canonical pair | LSP decreasing cells | LSP max −d/B | CP decreasing cells | CP max −d/B |
| --- | --- | --- | --- | --- |
| 1 → 2 h | 61 | 0.333 | 12,113 | 0.333 |
| 2 → 3 h | 32,696 | 0.395 | 19,444 | 0.333 |
| 3 → 47 h | 2,351 | 0.340 | 2,543 | 0.320 |
| 48 → 78 h | 957 | 0.333 | 37,706 | 0.333 |
| 0 → 1, 47 → 48, 78 → 81, 81 → 84 h | 0 | — | 0 | — |

No canonical cell violates `B`. The LSP 1 → 2 h pair shows why the sum is
taken per component: `RAIN_GSP` keeps E = −11, so only `SNOW_GSP` contributes
to `B`.

**Two decoders.** The 45 messages were decoded in the pinned
`flexpart-fortran` container:

- ecCodes 2.24.2;
- the independent octet reader plus libaec 1.0.6 (no ecCodes definitions).

The two decodings are bit-identical. Each value hash also equals the #230
evidence, which records equality with ecCodes 2.48.0. The self-test re-decodes
the committed subset with both decoders and reproduces the frozen component
pairs, which cover E +1, E +2, identical keys and the run-start anchor.

**Projection** is unchanged from #216/#219:

- an admitted pair gives a `+0.0` amount, SI rate and mm/h rate;
- the next interval starts from the unmodified lower value;
- the signed difference, `B` and the component message hashes go to a typed
  diagnostic outside `AccumulatedIntervalMetadata`.

## 4. Fail-closed classes

| Class | Verdict |
| --- | --- |
| ICON-EU regular-lat-lon, centre 78; four components plus TOT_PREC; template 5.42 with R = 0, D = 0, 1–24 bits (0 only at length 0), no bitmap; PDT 4.8 accumulation from the run start in minutes; reset = reference; closure verified | `ADMIT`; decreases `ADMIT_BOUNDED` inside `B` |
| Decrease with identical (R, E, D) in every component, or `−d ≥ B` | `REJECT` |
| Closure violated in any cell of the step | `REJECT` |
| Missing, duplicate or foreign component (e.g. graupel or a convective component in LSP) | `REJECT` |
| Time, reset, run or statistical-processing inconsistency; missing, non-finite or negative value | `REJECT` |
| TOT_PREC absent | `UNRESOLVED` |
| ICON global, ICON-D2 (graupel in TOT_PREC), ICON-EU-EPS, ICON-EU-NA2, native grid, third-party re-encodings | `UNRESOLVED` |
| Templates 5.0/5.2/5.3/5.40/5.41/5.200, bitmap, D ≠ 0, R ≠ 0, more than 24 bits, 0 bits after the run start | `UNRESOLVED` |
| Source unit other than kg m-2, other centres, GRIB1, NetCDF | `UNRESOLVED` |
| Every current input (no provider populates component records) | `UNRESOLVED`; runtime rejects, as today |

The artifact has 35 controls, each mutating the genuine LSP 1 → 2 h pair.
Together they reach all 21 rule outcomes, among them the exact boundary: one
`f32` step inside `B` is admitted, and the step at or beyond `B` is rejected.
The 8 genuine extreme-decrease cells, one per canonical pair with decreases,
are all `ADMIT_BOUNDED`.

## 5. Handoff

- **#32 provider.** For each step and field, the provider must:
  - decode the four components and TOT_PREC from raw octets;
  - record the component packing keys named in `handoff.provider_32`;
  - evaluate the closure on decoded `f64` values;
  - produce `c = f32(Y_rain + Y_snow)`.

  Nothing may come from ecCodes labels or defaults.
- **#75/#90 → [#240](https://github.com/Grodahn/flexpart-gpu/issues/240).**
  The #219 `SourceQuantization` record holds one GRIB1 `grid_simple` message
  per field. #240 extends it to two component records plus the closure result
  and adds this class to the #75 classifier. It reuses the #90 projection
  unchanged.
- **#235** owns interval and member semantics (hourly to 78 h, 3-hourly after,
  #207/#119 applicability).
- **#230 contract.** The `field_matrix` entries stay
  `REQUIRES_SCIENTIFIC_DECISION` until #235 and #240 close. #232 does not edit
  the contract.

## Reproduction

The sample bytes come from the #230 fetch, verified against its manifest. Use
`python -I`, because the downloads are untrusted data. `<sample>` holds
`2026103100/`.

```bash
docker run --rm -v <sample>:/data:ro -v "$PWD/scripts/provider:/tools:ro" -v "$PWD/fixtures/meteorology/icon-eu-audit-v1:/audit:ro" -v <out>:/out --entrypoint sh flexpart-fortran:latest -c 'cd /tmp && python3 -I /tools/icon_eu_precipitation_decision.py evidence --sample /data --manifest /audit/retrieval-manifest.json --audit-evidence /audit/decoded-evidence.json --out /out/evidence-v1.json'
```

```bash
python -I scripts/provider/icon_eu_precipitation_decision.py build --evidence fixtures/meteorology/icon-eu-precipitation-232/evidence-v1.json --decision fixtures/meteorology/icon-eu-precipitation-232/decision-v1.json
```

```bash
python -I scripts/provider/test_icon_eu_precipitation_decision.py
```

```bash
cargo test --test icon_eu_precipitation_decision
```

- `evidence` fails closed on:
  - a byte difference from the manifest;
  - a disagreement between ecCodes and libaec;
  - a value hash that differs from #230;
  - a parameter or length that does not match the file name.
- `build` regenerates only `evidence_sha256`, `sample_step_admission`,
  `genuine_rows` and `controls`. The self-test requires that rebuild to be
  byte-identical.
- The two-decoder self-test runs where ecCodes and libaec are installed (all
  9 tests pass in the pinned container). Elsewhere it is skipped.
