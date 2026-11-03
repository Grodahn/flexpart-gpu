# ICON-EU time-averaged surface fluxes and stress (#233)

Issue: [#233](https://github.com/Grodahn/flexpart-gpu/issues/233)
(MET-INGEST-32-ICON-FLUX), parent [#32](https://github.com/Grodahn/flexpart-gpu/issues/32)
via the [#230 provider audit](icon-eu-provider-230.md), tracking #12.

This is a research and decision issue. It contains no decoder, no adapter, no
PBL or deposition code and no #120 change. Evidence level: real-source
decoding plus decoded-value consistency checks. No oracle, production-path or
GPU validation was performed or claimed.

## Outcome

| Canonical field | ICON source | Sign | Canonical temporal kind | `Instantaneous` | `calcpar`-equivalent consumers |
| --- | --- | --- | --- | --- | --- |
| `sensible_heat_flux` | `ASHFL_S` (0/0/11) | positive downward → **negate** | `IntervalMean` (`ADMISSIBLE_WITH_PROVEN_TRANSFORMATION`) | `NOT_ADMISSIBLE` | `REQUIRES_NEW_TEMPORAL_ORACLE` (#244) |
| `surface_solar_radiation` | `ASOB_S` (0/4/9) | positive downward net → identity, clip within-bound negatives to 0 | `IntervalMean` | `NOT_ADMISSIBLE` | `REQUIRES_NEW_TEMPORAL_ORACLE` (#244) |
| `surface_stress_eastward` | `AUMFL_S` (0/2/17) | positive eastward → identity | `IntervalMean` | `NOT_ADMISSIBLE` | `REQUIRES_NEW_TEMPORAL_ORACLE` (#244) |
| `surface_stress_northward` | `AVMFL_S` (0/2/18) | positive northward → identity | `IntervalMean` | `NOT_ADMISSIBLE` | `REQUIRES_NEW_TEMPORAL_ORACLE` (#244) |

The #230 derived fields `friction_velocity`, `inverse_obukhov_length`,
`convective_velocity_scale` and `mixing_height` move from the #233 blocker to
[#244](https://github.com/Grodahn/flexpart-gpu/issues/244). The last three are
also still blocked by #231.

Machine-readable artifacts (`fixtures/meteorology/icon-eu-flux-233-v1/`):

| Artifact | Role |
| --- | --- |
| `decision.json` | Versioned decision `flexpart-gpu.icon-eu-flux-decision` v1: per-field verdicts, signs, temporal kinds, quantization rule and bounds, fail-closed rules, evidence hashes, follow-ups |
| `flux-evidence.json` | Derived evidence from the pinned decoder: identity, quantum and SHA-256 per message, bound per interval, sign and short-wave checks. The raw bytes stay authoritative |
| `retrieval-manifest.json` | URL, HTTP `Last-Modified`, sizes and SHA-256 (bz2 and GRIB2) of all 426 files |
| `sample-request.json` | Bounded request that expands to exactly those 426 files |

No raw messages are committed. The audit tests reuse the #230 step-0 `ASHFL_S`
message.

## Sample

| Item | Value |
| --- | --- |
| Product | ICON-EU deterministic `icon-eu_europe_regular-lat-lon`, DWD Open Data (CC BY 4.0, *Source: Deutscher Wetterdienst*) |
| Cycle | 2026-11-03T00:00:00Z |
| Flux fields | `ASHFL_S`, `ASOB_S`, `AUMFL_S`, `AVMFL_S` at **every** published step: 0–78 h hourly and 81–120 h every 3 h (93 steps) |
| Controls | `ASWDIR_S`, `ASWDIFD_S`, `ASWDIFU_S` (10 steps); `U_10M`, `V_10M` (7 steps); `T_G`, `T_2M` (4 steps); `FR_LAND`, `HSURF` |
| Download | 2026-11-03 14:31–14:33 UTC, 426 files, 416.4 MB bz2 / 416.8 MB GRIB2 |
| Decoders | primary: ecCodes 2.24.2 in the pinned `flexpart-fortran:latest` container; secondary: host ecCodes 2.48.0. `compare`: 0 mismatches (floats at relative 1e-9, everything else exact). Metadata comes from the independent octet reader and is cross-checked against ecCodes keys |
| Identity | 0 problems. Every average is PDT 4.8, statistical processing 0, averaging start 0, one time range, minutes, length `60·step`, interval end = valid time. All four fields are exactly 0 at step 0 |

## Normative references

- **DWD ICON database reference manual** v2.5.6 (2026),
  [PDF](https://www.dwd.de/SharedDocs/downloads/DE/modelldokumentationen/nwv/icon/icon_dbbeschr_aktuell.pdf),
  SHA-256 `ef804f22…dcfe41`:
  - Table 5.6 lists all four as `avg` "average since model start" with
    barycentric (BCT) lat-lon interpolation.
  - §7.1.1 defines `Ā(t) = (1/t)∫₀ᵗ ψ dt` with `Ā(0) = 0`. It gives the
    interval mean `(t₂Ā(t₂) − t₁Ā(t₁))/(t₂ − t₁)` and the PDT 4.8 keys.
  - §6.3 defines `ASOB_S` as the surface short-wave **net** flux. `ASWDIR_S` is
    diagnosed a posteriori from `F_net = F_dir + F_dif,down − F_dif,up`.
  - §7.2: BCT is linear with fixed weights, so de-averaging commutes with the
    interpolation.
  - The manual states **no** flux sign convention.
- **Pinned FLEXPART 11.1** `c70586c2`:
  - The GRIB2 reader maps SHF = 0/0/11 surface, SR = 0/4/9 surface,
    EWSS = 0/2/38 and NSSS = 0/2/37 (`windfields_mod.f90:504-511`). It reads
    `sshf` unscaled and clips `ssr < 0` to 0 (`:2351-2387`).
  - `obukhov` uses `thetastar = hf/(rhoa·cpa·ustar)`
    (`getfields_mod.f90:1449`), and the convective branch runs for `hf < 0`
    (`:1682-1684`). The `sshf` input is therefore positive **downward**.
  - `ssr` reaches `getvdep` as `gr` (`getfields_mod.f90:1064-1067`;
    `drydepo_mod.f90:1010,1518,1533`).
  - Stress enters only as `sqrt(ewss²+nsss²)` (`windfields_mod.f90:2579`).
- **Pinned flex_extract 7.1.2** `e0005c99`: `deacc_fluxes`
  (`EcFlexpart.py:1123-1143`) and `dapoly` (`disaggregation.py:62-103`) turn
  ECMWF accumulations into the point values FLEXPART reads
  ([#120 crosswalk](../temporal-interpolation.md#surface-flux-eligibility-120)).
- **Pinned ecCodes 2.24.2 definitions:** ECMWF `ssr`/`sshf` are the same WMO
  parameters as `ASOB_S`/`ASHFL_S` with `typeOfStatisticalProcessing=1`, and
  the DWD `edzw` concepts carry `=0`.

## 1. Temporal semantics and #120

- An ICON average is a mean over `[run start, valid time]`. The consecutive-step
  difference above gives an exact mean over `[t₁, t₂]`. That is the same kind of
  quantity `deacc_fluxes` produces from ECMWF accumulations. It maps to
  canonical `TemporalKind::IntervalMean` with `interval_start = cycle + t₁` and
  `interval_end = valid_time = cycle + t₂`, which the #29 `SurfaceFluxRate`
  policy already accepts.
- Interval lengths are 1 h through 78 h, then 3 h to 120 h, for 00/06/12/18 UTC
  cycles. For 03/09/15/21 UTC cycles they are 1 h through 30 h, then 6 h.
- Step 0 never becomes a canonical value. It has no interval, and its zeros would
  look like an absent flux to FLEXPART's `hflswitch`/`strswitch` logic.
- **Instantaneous: `NOT_ADMISSIBLE`.** #120 admits only point values at the
  validity time. A mean is not a point value and is never relabelled, so #74/#89
  sampling stays unavailable.
- **`calcpar`-equivalent consumers: `REQUIRES_NEW_TEMPORAL_ORACLE`.** FLEXPART
  evaluates nonlinear diagnostics at each `memtime` from point values: `ustar`
  from `|stress|` and the Obukhov length from `sshf` and `ustar`. Feeding
  interval means instead is a different operation. Applying pinned `dapoly` to
  the ICON means is structurally possible, because they have the
  `deacc_fluxes` form. It is not proven, though:
  - the first and last boundaries need edge rules;
  - `dapoly` assumes equal-length intervals, which the 78 → 81 h change breaks;
  - the quantization bound propagates through its weights.

  [#244](https://github.com/Grodahn/flexpart-gpu/issues/244) owns this
  reconstruction decision and its direct pinned-routine oracle.

## 2. Signs

| Check (`flux-evidence.json`) | Result | Verdict |
| --- | --- | --- |
| `sensible_heat_sign`, 12–13 UTC land mean of the derived `ASHFL_S` | −28.2 W m-2 (night 23–24 UTC: +15.9) | positive downward |
| Sign of −`ASHFL_S` vs sign of `T_G − T_2M` (\|ΔT\| > 2 K at both ends, \|H\| > 5 W m-2) | 99.90 % of 225 540 cells (12–13 h); 99.59 % of 218 522 cells (23–24 h) | canonical upward = −`ASHFL_S` |
| `stress_sign`: sign of `AUMFL_S`/`AVMFL_S` interval means vs bracketing `U_10M`/`V_10M` (\|wind\| > 3 m s-1 at both ends) | 100 % in all 8 field–interval pairs (243 k–323 k cells); correlation +0.70 to +0.84 | identity, positive eastward/northward |
| `shortwave_budget`: `ASOB_S` vs `ASWDIR_S + ASWDIFD_S − ASWDIFU_S` | closes within packing error in ≥ 99.26 % of cells per step; `ASOB_S` never exceeds downward short-wave; land mean net/downward 0.84 | net short-wave, positive downward |

- **Sensible heat:** `ASHFL_S` has the ECMWF/FLEXPART `sshf` convention.
  Canonical `sensible_heat_flux` (`positive_upward_flux`) is `−ASHFL_S`, the
  same negation #120 requires for SSHF.
- **Short-wave:** `ASOB_S` is the WMO parameter that FLEXPART's GRIB2 reader
  assigns to `ssr`, and it is net short-wave. So it **is** the quantity
  `getvdep` consumes as `gr`, even though the Fortran comment calls `gr`
  "global radiation". The ICON global radiation `ASWDIR_S + ASWDIFD_S` is
  *not* that quantity. Negative derived means within the quantization bound are
  clipped to 0, which is FLEXPART's own `ssr` clip.
- **Stress:** `AUMFL_S`/`AVMFL_S` are WMO "momentum flux" parameters
  (0/2/17–18), not ECMWF's 0/2/38/37, and the pinned reader would not
  recognise them. A downward flux of eastward momentum is the eastward surface
  stress, and the decoded sign matches the 10 m wind in every checked cell. The
  stress components therefore map with an explicit identity transformation to
  `positive_eastward`/`positive_northward`. FLEXPART uses only their magnitude.

## 3. Quantization bound and fail-closed limits

Packing is CCSDS 5.42 with 16 bits. A decoded value is `(R + X·2^E)/10^D`,
where `R` is the IEEE float32 reference. Per message the error bound is

`e = ½·2^E/10^D + ½·spacing(float32(R))/10^D`

The second term is needed: five cells at 17–18 h exceed the pure half-quantum
bound by exactly 2.4·10⁻⁷ W m-2, which is the float32 spacing of `R` times 17.
The interval-mean bound is

`B(t₁, t₂) = (t₂·e(t₂) + t₁·e(t₁)) / (t₂ − t₁)`

It grows linearly with lead time. The rounding model is confirmed by the data:
night-time interior `ASOB_S` means fill `[−B, +B]` symmetrically (77–78 h:
−0.3008 to +0.3008 W m-2 for B = 0.3027). Floor packing would be asymmetric.

Exact bounds for this cycle (all 92 intervals per field, with the GRIB2 SHA-256
of both messages, are in `flux-evidence.json` → `interval_bounds`):

| Field | 0–1 h | 12–13 h | 47–48 h | 77–78 h (last hourly) | max hourly | max 3-hourly (78–120 h) |
| --- | --- | --- | --- | --- | --- | --- |
| `ASHFL_S` (W m-2) | 0.0039 | 0.0978 | 0.186 | 0.303 | 0.303 | 0.155 |
| `ASOB_S` (W m-2) | 0.0005 | 0.0977 | 0.186 | 0.303 | 0.303 | 0.154 |
| `AUMFL_S` (N m-2) | 6.1e-5 | 7.7e-4 | 2.9e-3 | 4.7e-3 | 4.7e-3 | 2.4e-3 |
| `AVMFL_S` (N m-2) | 3.1e-5 | 7.7e-4 | 1.5e-3 | 2.4e-3 | 2.5e-3 | 1.2e-3 |

**No magnitude cap.** flex_extract's de-accumulation of ECMWF forecast
accumulations has the same lead-time amplification and no cap. A cap would be
a new consumer-sensitivity tolerance outside this issue. Instead, the bound
travels with every derived field. The fail-closed rules are in `decision.json`:

1. Reject any message whose octets deviate from the expected identity. File
   names are never evidence.
2. Derive only between consecutive published steps of one cycle. Reject missing
   steps, mixed cycles and duplicates.
3. Never emit step 0 or a derived mean as `Instantaneous`.
4. Reject when the packing metadata for the bound is absent or not
   template 5.42/5.0.
5. `ASOB_S`: clip `[−B, 0)` to 0. Reject the field when any ingested cell is
   below −B, or any night cell (sun below −1° for the whole interval) is outside
   ±B.
6. `ASHFL_S`: negate.

### Real-data violations

Rule 5 fires on this cycle:

- 53 cell-intervals fall below −B and 63 night cell-intervals lie outside ±B, in
  20 of 92 intervals (mostly 12–28 h), up to 6.2·B.
- **All** of them lie in the three easternmost grid columns (62.375–62.5° E)
  at 68.0–69.9° N. No interior cell violates the bound.
- The cause is not established. The documented BCT interpolation is linear
  with fixed weights, so it cannot cause this by itself.
- The other three fields have no physical bound, so the same defect could not be
  detected there.

A domain containing those cells therefore rejects `ASOB_S` for the affected
intervals. #244 decides how they are admitted for point reconstruction.

The short-wave budget does not close beyond packing error in up to 0.73 % of
cells per sampled step (6 643 cells at 6 h, 4 at 78 h). Of those cells, 98–100 %
are land, with median orography 170–910 m against a domain median of 68 m. The
residuals are almost always negative, at most 21 % of the downward flux.
Terrain-radiation treatment is suspected but not established. This does not
affect the sign or net-flux verdicts. Both anomalies are recorded as
`open_items`.

## Reproduction

Run from the repository root. Downloads go to a new empty directory, and
`python -I` is used because the bytes are untrusted.

```bash
python -I scripts/provider/icon_eu_fetch.py --request fixtures/meteorology/icon-eu-flux-233-v1/sample-request.json --output <new-dir> --expect fixtures/meteorology/icon-eu-flux-233-v1/retrieval-manifest.json
```

```bash
docker run --rm -v <new-dir>:/data:ro -v "$PWD/scripts/provider:/tools:ro" -v <out>:/out --entrypoint sh flexpart-fortran:latest -c 'cd /tmp && python3 -I /tools/icon_eu_flux_audit.py inspect --sample /data --label container-eccodes-2.24.2 --output /out/primary.json'
```

```bash
python -I scripts/provider/icon_eu_flux_audit.py inspect --sample <new-dir> --label host-eccodes-2.48.0 --output <out>/secondary.json
```

```bash
python -I scripts/provider/icon_eu_flux_audit.py compare --primary <out>/primary.json --secondary <out>/secondary.json
```

- DWD keeps a cycle for about 24 h. After that, `--expect` makes a fetch fail
  closed, and reproduction needs a byte-identical archive passed with
  `--base-url`. A fresh `primary.json` must be byte-identical to the frozen
  `flux-evidence.json` up to the decoder label.
- **Offline tests:** `python -I scripts/provider/test_icon_eu_flux_audit.py`
  runs 23 tests. They cover the formulas, the float32 term (also for negative references),
  missing-message and step-coverage failures, the identity
  rejection of mutated real octets (accumulation, hours, averaging start, cycle,
  parameter), the solar ephemeris, and decision ↔ evidence ↔ manifest/request
  hash consistency, including recomputation of every interval bound.
- **Rust lock:** `cargo test --test icon_eu_flux_decision` pins the decision
  hash, ties it to `FIELD_SPECS` (unit, sign, `IntervalMean`), forbids
  `Instantaneous`, binds the evidence/manifest/#230-contract hashes, and
  requires every #230 row blocked by #233 to be decided or handed to #244.

The tooling is research-only. It builds no canonical snapshot and is not
imported by production code.
