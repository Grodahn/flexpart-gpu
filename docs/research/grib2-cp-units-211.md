# GRIB2 convective precipitation units in pinned FLEXPART 11.1 (#211)

Status: **decided**. The `/1000` applied to GRIB2 CP at
`windfields_mod.f90:2145-2147` compensates an ecCodes encoder conversion.
For flex_extract 7.1.2 `FORMAT=GRIB2` output converted with ecCodes 2.24.2
and read by FLEXPART linked against ecCodes 2.24.2, CP and LSP are stored in
the mm/h that the consumers expect: **CORRECT_AS_PREPARED**. The result
depends on ecCodes definitions. With ecCodes 2.50.0 conversion the pinned
reader recognises neither field and stops. Every other GRIB2 class is
**UNRESOLVED**, and #32 must reject it. No reference defect is confirmed or
corrected.

Inspected from main revision `ad5c0c0` on 2026-10-27.

Machine-readable decision: [grib2-cp-units-v1.json](../../fixtures/accumulation/grib2-cp-units-v1.json).
Decoded evidence: [grib2-cp-units-evidence-v1.json](../../fixtures/accumulation/grib2-cp-units-evidence-v1.json).
Regression lock: [tests/grib2_cp_units.rs](../../tests/grib2_cp_units.rs).

Validation level:

- original-source analysis;
- execution of unmodified pinned flex_extract preparation and GRIB2
  conversion;
- execution of unmodified pinned `gridcheck_ecmwf` and `readwind_ecmwf` on
  complete prepared files.

**No GPU execution and no production-parity claim.**

## References

| Reference | Pin | Role |
| --- | --- | --- |
| FLEXPART 11.1 ([manifest](../../reference/flexpart-11.1.json)) | `c70586c2b7f5258850705325881c61f557ea9bd8`, `eta=no`, `numpf=1` | Reader and consumers |
| flex_extract 7.1.2 ([manifest](../../reference/flex-extract.json)) | `e0005c99ac81d12faa45a8ff799debbd592b0dc0` | Producer: deaccumulation, assembly, GRIB2 conversion |
| ecCodes | 2.24.2 (container `libeccodes 2.24.2-1`); variant 2.50.0 (PyPI `eccodeslib 2.50.0.33`) | Producer `grib_set` and reader decoding |

The #202 note ([precipitation-interval-mapping-202.md](precipitation-interval-mapping-202.md))
established the GRIB1 member semantics. Its runner is reused here unchanged.

## Phase 1 — reader crosswalk

`GRIB message → grib_get_real4_array('values') → branch match → value / conversion_factor → negative clip → lsprec/convprec [mm/h] → interpol_rain → wetdepo`

| Step | CP | LSP |
| --- | --- | --- |
| Common keys | `editionNumber`, `stepRange` → `ipf = stepRange+1` (`2060-2066`). GRIB2 also reads `discipline`, `parameterCategory`, `parameterNumber`, `typeOfFirstFixedSurface`, `level`, `paramId`. `conversion_factor = 1` is reset per message (`2085-2105`). | same |
| GRIB1 match | `indicatorOfParameter 143`, factor 1 (`2068-2080`) | `142`, factor 1 |
| GRIB2 match | `parameterCategory 1 ∧ parameterNumber 10` → 143, **factor 1000**, no `paramId` fallback (`2145-2147`) | `(1 ∧ 9) ∨ paramId 142` → 142, factor 1 (`2143-2144`) |
| Store | `convprec = value/conversion_factor`, clip < 0 (`2344-2349`) | `lsprec = value`, clip < 0 (`2336-2342`) |
| Declared units | `convprec [mm/h]` (`158`) | `lsprec [mm/h]` (`157`) |
| Admission | `gridcheck_ecmwf` counts recognised LSP/CP (`540-560`) and stops if none (`757-760`) | same |
| Consumers | `interpol_rain` bilinear, unscaled (`interpol_mod.f90:1347-1354`). `wetdepo` uses mm/h: `237, 246`; threshold `precsub = 0.01 mm/h` (`278, 331-343`); mm/h → m/s via `/3.6E6` (`611, 644`); area fractions (`716`). | same |

The divisor arrived with the LSP/CC `paramId` fallbacks in
`4fbe7a5` (v9.2 beta, 2014-05-23). The commit only says it keeps the code
"consistent with new readwind.f90" and gives no physical rationale.
The variable names therefore decide nothing. The executed chain below does.

## Phase 2 — producer

`prepare_flexpart.py:158-172` calls `deacc_fluxes`, then `create`, then
`process_output`.

1. `deacc_fluxes` (`EcFlexpart.py:1121-1143`) converts LSP and CP alike.
   It turns m accumulated into a mm/h interval mean (`/ (1/1000)`, then
   difference, then `/ DTIME`). `RRINT=0` then writes the `darain` point rate
   at `T` as GRIB1. Each message keeps its input `paramId` 142/143, whose
   ecCodes label is **m**, and has `stepRange 0`. **The label does not
   describe the content, which is mm/h.**
2. `create` (`1757-1765`) concatenates `fort.15`, `flux<T>`, `fort.16` and
   `OG_OROLSM` into `<PREFIX><YYMMDDHH>`.
3. With `FORMAT GRIB2`, `process_output` (`1893-1902`) runs
   `grib_set -s edition=2,productDefinitionTemplateNumber=8` on every file.
   It changes nothing else. The default format is GRIB1
   (`ControlFile.py:391`). Nine shipped controls select GRIB2, among them
   `CONTROL_OD.OPER.FC.operational`, `CONTROL_OD.OPER.FC.eta.global` and
   `CONTROL_EA5`.

The edition change re-encodes each message through the ecCodes `paramId`
concept. In ecCodes 2.24.2 (`grib2/localConcepts/ecmf/paramId.def:693-705`):

| paramId | GRIB2 encoding | Effect on values |
| --- | --- | --- |
| 142 lsp | `192/128/142` (ECMWF local) | none |
| 143 cp | `0/1/10` (WMO), **`unitsFactor = 1000`** | × 1000; decodes as paramId 3063 `acpcp`, `kg m**-2` |

The ×1000 is the m → kg m⁻² conversion of the encoder. It is applied to
values that are already mm/h, so the decoded GRIB2 CP holds 1000 × mm/h
under a `kg m**-2` label.

The definitions changed in later releases (ecCodes GitHub tags):

- 2.30.0: CP unchanged; LSP moves to local `0/1/196`.
- 2.31.0 and later (through 2.50.0): CP is `0/1/195` and LSP `0/1/196`, both
  in m with no `unitsFactor`.

## Phase 3 — executed example

| Item | Classification |
| --- | --- |
| Accumulated LSP/CP inputs and series | #202 declared example: 3×3 grid, `DTIME=3`, 16 intervals, 24-bit packing |
| `deacc_fluxes`, `process_output` | Original producer, unmodified pinned source, executed |
| Prepared `flux<T>` LSP/CP messages | Original-producer evidence |
| Model-level, surface and orography messages | Candidate-created support fields so the reader gets complete files; not under audit |
| ecCodes 2.50.0 conversion | The same unmodified `process_output`, run on the same pre-conversion files under another ecCodes install |
| Reader | Unmodified `gridcheck_ecmwf` + `readwind_ecmwf`, built from a `git archive` scratch copy and linked by [the harness](../../scripts/precipitation/grib2_cp_reader_oracle.f90). The pristine checkout was verified clean before and after. |

The example uses two member times, 2023-11-01 12 UTC and 2023-11-02 06 UTC.
Each file has nine nodes per field, including zero CP nodes, nonzero CP
nodes and a nonzero LSP control.

## Phase 4 — measured conversion

The table shows node (0,0) at 2023-11-01 12 UTC. Every row is in the
evidence.

| Quantity | CP | LSP |
| --- | --- | --- |
| Written by `deacc_fluxes` | 0.2000000477 mm/h | 1.0000002384 mm/h |
| GRIB1 decoded (label `m`, E = −24 / −23, D = 0) | 0.2000000477 | 1.0000002384 |
| GRIB2 (2.24.2) decoded | **200.0000610** `kg m**-2`, `0/1/10`, paramId 3063, E = −14, D = 0 | 1.0000002384 `m`, `192/128/142` |
| Reader branch | GRIB2 `/1000` | factor 1 |
| Reader stored, GRIB1 file | 0.2000000477 mm/h | 1.000000238 mm/h |
| Reader stored, GRIB2 file | 0.2000000626 mm/h | 1.000000238 mm/h |
| Power-of-1000 exponent, decoded GRIB2 / written | **1** | 0 |
| Power-of-1000 exponent, stored GRIB2 / written | **0** | 0 |

Over all 36 rows of the accepted class (18 CP, 6 of them zero, and 18 LSP),
stored values match the written values within the #71 policy
`1e-6 + 1e-5·|o|`:

- GRIB2 CP differs from GRIB1 CP by at most 2.235e-8 mm/h, from re-packing.
- GRIB2 LSP is bit-identical to GRIB1 LSP.
- Zero CP stays exactly zero.

The binary-scale shift (−24 → −14) only reflects the 1000× larger range.
It is not the unit conversion.

Without the divisor, CP would be stored 1000 times too large.

With ecCodes 2.50.0 conversion the reader never reaches the divisor. It logs
`undefined GRiB2 message` for `0/1/195` and `0/1/196` and stops at
`windfields_mod.f90:760` ("Conditions for precipitation interpolation not
fulfilled"), exit status 1, on both files.

## Phase 5 — verdicts

| Class | Verdict | Basis |
| --- | --- | --- |
| flex_extract 7.1.2 GRIB1 | CORRECT_AS_PREPARED | Executed control |
| flex_extract 7.1.2 GRIB2, ecCodes 2.24.2 producer and reader | **CORRECT_AS_PREPARED** | Executed |
| GRIB2 via ecCodes 2.50.0, reader 2.24.2 | UNRESOLVED | Executed; no value stored, so no units verdict. The reader fails closed. |
| GRIB2 via ecCodes ≥ 2.31, reader ecCodes ≥ 2.31 | UNRESOLVED | Source-level hazard: LSP would pass via `paramId 142`, while CP `0/1/195` matches no branch and is silently never stored |
| GRIB2 via ecCodes 2.25–2.30 | UNRESOLVED | Definitions differ (LSP moved at 2.30); not executed |
| GRIB2 not produced by the flex_extract `grib_set` route | UNRESOLVED | No original producer identified. The reader would divide any `0/1/10` message by 1000 and treat it as mm/h. |
| RRINT=1 / numpf=3 GRIB2 | UNRESOLVED | #212 |

The `/1000` is therefore neither a confirmed bug nor universally correct.
It is a compensation that is valid for one demonstrated encoder.

The CP/LSP asymmetry comes from how ecCodes 2.24.2 encodes the two
`paramId`s. It does not reflect different prepared physics.

If a discrepancy appears for another class, it belongs to the combination of
producer and ecCodes definitions. It is not a units error in the reference
for the demonstrated class.

## Phase 6 — handoff to #32

The decision artifact (`handoff_32`) fixes the following requirements:

- **Canonical form.** CP and LSP are point rates in mm/h at the member time
  (#202/#207 semantics), stored unscaled.
- **Accepted inputs.** Only the two `CORRECT_AS_PREPARED` identities are
  accepted. Each must carry the exact GRIB2 keys listed in the artifact and
  producer provenance naming flex_extract 7.1.2 and its ecCodes release.
- **Conversion ownership.** #32 divides the decoded `0/1/10` CP by exactly
  1000, as the explicit inverse of the producer `unitsFactor`, and records
  this in provenance. LSP stays unscaled. Neither the `units` label, the WMO
  table nor the FLEXPART source alone justifies a factor.
- **Negative values.** They are clipped to 0 after the conversion, as the
  reader does (`2340, 2348`), and the clip is recorded in provenance.
- **Rejections.** The artifact lists nine fail-closed rejection codes. They
  cover:
  - unknown producers or ecCodes releases;
  - `0/1/195` / `0/1/196`;
  - unexpected `stepRange` or statistical processing;
  - missing or duplicate members;
  - labels used as conversion evidence;
  - non-finite values.
- **Regression fixtures.** The frozen rows of both editions, plus rejection
  cases for the 2.50.0 messages and for a `0/1/10` message without accepted
  provenance.

No follow-up implementation issue is needed. Executing the ecCodes ≥ 2.31
reader hazard would need pinned FLEXPART built against a newer ecCodes. That
work would only refine a class that #32 already rejects.

## Reproduction

```bash
docker run --rm -v <repo>:/workspace/flexpart-gpu -v <flex_extract clone>:/workspace/flex_extract:ro --entrypoint python3 flex-extract:latest -I /workspace/flexpart-gpu/scripts/precipitation/grib2_cp_units_trace.py --flex-extract-git /workspace/flex_extract --out /workspace/flexpart-gpu/target/issue-211/flex-extract
```

```bash
docker run --rm -v <repo>:/workspace/flexpart-gpu python:3.11-slim bash -c 'pip install -q eccodes==2.50.0 eccodeslib==2.50.0.33 eckitlib==2.5.0.33 numpy==1.26.4 && SP=$(python -c "import eccodeslib,os;print(os.path.dirname(eccodeslib.__file__))") && PATH=$SP/bin:$PATH python -I /workspace/flexpart-gpu/scripts/precipitation/grib2_cp_eccodes_variant.py --trace /workspace/flexpart-gpu/target/issue-211/flex-extract --label prepared-grib2-eccodes-2.50.0'
```

```bash
docker run --rm -v <repo>:/workspace/flexpart-gpu -v <clean FLEXPART pin>:/workspace/flexpart:ro --entrypoint bash flexpart-fortran:latest /workspace/flexpart-gpu/scripts/precipitation/grib2_cp_reader_oracle.sh /workspace/flexpart-gpu/target/issue-211/flex-extract /workspace/flexpart-gpu/target/issue-211/oracle
```

```bash
python -I scripts/precipitation/decode_grib2_cp_units_evidence.py --trace target/issue-211/flex-extract --oracle target/issue-211/oracle --variant prepared-grib2-eccodes-2.50.0 --out fixtures/accumulation/grib2-cp-units-evidence-v1.json
```

The prepared files are byte-identical across reruns. The evidence records
their SHA-256 values, plus those of the reader binary, outputs and logs.

Raw files, logs and the reader build stay locally under `target/issue-211/`;
they are not CI artifacts.

Environment:

- images `flex-extract` `sha256:717f14dfd256…` and `flexpart-fortran`
  `sha256:324bb63d7311…`;
- gfortran 11.4.0, Python 3.11.18 for the variant.

## Out-of-scope observations

- ecCodes 2.24.2 encodes snow depth (141) as `0/1/254`. The GRIB2 SD branch
  (`0/1/11`, `/1000`) therefore never matches flex_extract GRIB2 output, and
  `sd` stays unread.
- The GRIB2 point rates carry PDT 8 with `typeOfStatisticalProcessing 0`
  (average) and `stepRange 0`. Apart from `stepRange`, the reader ignores
  these keys.
