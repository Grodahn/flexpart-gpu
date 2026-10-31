# ICON-EU provider feasibility audit (#230)

Issue: [#230](https://github.com/Grodahn/flexpart-gpu/issues/230) (MET-INGEST-32A),
parent [#32](https://github.com/Grodahn/flexpart-gpu/issues/32), tracking #12.

This is a research, source-data audit and provider-contract decision. It is
**not** an operational decoder, not a canonical snapshot, and not a FLEXPART
11.1 parity result. Evidence level: real-source decoding plus decoded-value
consistency checks. No production-path, oracle or GPU validation was performed
or claimed.

## Outcome

**Decision: `NO_GO`** for DWD ICON-EU deterministic regular-lat-lon GRIB2 as
the *first* operational #32 ingestion provider under the current
#29/#30/#31 contracts and the pinned FLEXPART 11.1 oracle.

- The real product decodes cleanly. One cycle was fetched (2026-10-31 00 UTC,
  247 files) and every message decodes in the repository's pinned ecCodes 2.24.2
  container. Values are bit-identical to ecCodes 2.48.0 and to an independent
  octet reader plus libaec.
- Nine canonical fields map as-is or with a proven unit/orientation
  transformation: surface pressure, orography, land fraction, 10 m wind, 2 m
  temperature/dew point, snow water equivalent and total cloud cover. None of the
  production consumers can run on that subset alone.
- Every consumer (advection, PBL/turbulence, convection, wet/dry deposition,
  settling) is blocked by at least one unresolved scientific question:
  - **Vertical geometry:** ICON uses a height-based terrain-following coordinate
    with 3-D `HHL`, which #29 schema v1 and #30 cannot represent.
  - **Vertical velocity:** ICON `W` is earth-relative geometric velocity with a
    non-zero terrain boundary value. FLEXPART uses a coordinate-relative W with
    surface W forced to zero.
  - **Precipitation:** values are rate parameters carrying run accumulations,
    with CCSDS-quantized same-run decreases. The flex_extract member
    preparation does not exist for ICON.
  - **Surface fluxes:** only time averages since the run start are published.
  - **Land use:** the FLEXPART land-use classes are absent.
- Pinned FLEXPART 11.1 (`c70586c2`) has only `readwind_ecmwf` and
  `readwind_gfs`. No pinned oracle consumes ICON input.

**Recommendation:** implement #32 first for the existing ERA5/ETEX
flex_extract-prepared path. Keep ICON-EU as a later provider adapter. Its
prerequisites are the five decision issues #231–#235.

Machine-readable artifacts (`fixtures/meteorology/icon-eu-audit-v1/`):

| Artifact | Role |
| --- | --- |
| `provider-contract.json` | Versioned contract `icon-eu-regular-lat-lon-feasibility` v1: decision, per-field matrix, message identities, grid/vertical/time identity, feasibility, follow-ups |
| `retrieval-manifest.json` | URL, HTTP `Last-Modified`, sizes and SHA-256 (bz2 and GRIB2) of all 247 audited files |
| `decoded-evidence.json` | Derived evidence per message (octet identity, packing, value summary, decoder labels) and the science checks. The raw bytes stay authoritative |
| `sample-request.json` | Bounded request that expands to exactly the 247 files |
| `raw/` | 10 unmodified real GRIB2 messages (55 KB) for offline tests |

## Selected cycle and source identity

| Item | Value |
| --- | --- |
| Product | ICON-EU deterministic, `icon-eu_europe_regular-lat-lon`; excludes ICON global, ICON-D2, ICON-EU-EPS, the native triangular grid and third-party APIs |
| Source | `https://opendata.dwd.de/weather/nwp/icon-eu/grib/00/<variable>/` |
| Initialization / forecast reference time | 2026-10-31T00:00:00Z (GRIB section 1, significance 1 = start of forecast) |
| Valid times sampled | +0, 1, 2, 3, 12, 13, 47, 48, 78, 81, 84 h; time-invariant files |
| Download | 2026-10-31 19:30–19:31 UTC, two independent retrievals; the 235 overlapping files are byte-identical |
| Volume | 247 files, 158.9 MB bz2, 169.2 MB GRIB2 |
| Producer keys (all 247) | edition 2, centre 78 (`edzw`), sub-centre 255, tables 19 / local 1, generating process 2, type of generating process 2 (forecast), production status 0 (operational), local-use section present |
| Model identity | Bound by centre + generating process + grid + vertical-grid UUID + SHA-256. The name "ICON-EU" appears only in the DWD path/file name, which is never used as evidence |
| Control | 2026-10-31 03 UTC `TOT_PREC` steps 0–1, used only for the wrong-cycle test |
| Licence | CC BY 4.0 per the [DWD legal notice](https://www.dwd.de/EN/service/legal_notice/legal_notice_node.html). Attribution: *Source: Deutscher Wetterdienst (DWD)*. The committed raw messages are unmodified |

Representative hashes (full list in `retrieval-manifest.json`):

| File (prefix `icon-eu_europe_regular-lat-lon_`) | GRIB2 bytes | GRIB2 SHA-256 (prefix) |
| --- | --- | --- |
| `time-invariant_2026103100_HSURF` | 747,052 | `415f3e1ea5982b02` |
| `time-invariant_2026103100_75_HHL` | 747,076 | `6b9f96e92ad11236` |
| `model-level_2026103100_000_74_T` | 1,079,605 | `b20ac6eed9e3f3bf` |
| `model-level_2026103100_000_75_W` | 664,773 | `bac36fcbaafa5a4d` |
| `single-level_2026103100_000_PS` | 925,790 | `69516bccdaa03969` |
| `single-level_2026103100_048_TOT_PREC` | 702,187 | `8cf537f413a03cd8` |
| `single-level_2026103100_048_RAIN_GSP` | 489,864 | `06d3472f40bbb7ad` |

DWD Open Data is rolling. Each cycle-hour directory is overwritten by the next
day's run (about 24 h retention), so these URLs are not immutable research
artifacts. The SHA-256 manifest identifies the audited bytes; reproducing the
audit later requires an archive of those bytes.

## Decoder and codec

| Decoder | Identity | Result |
| --- | --- | --- |
| Primary | ecCodes 2.24.2 (Debian `libeccodes0 2.24.2-1`, `libaec0 1.0.6-1`, `python3-eccodes 1.4.0`) in `flexpart-fortran:latest` `sha256:324bb63d…80d84`, the repository's pinned Fortran/flex_extract container | 247/247 decoded, 0 metadata cross-check problems |
| Secondary | ecCodes 2.48.0 (Python wheel, features `AEC MEMFS JPG PNG`) | 247/247; values bit-identical to the primary decoder |
| Independent | [`icon_eu_grib2_sections.py`](../../scripts/provider/icon_eu_grib2_sections.py): own WMO octet parser plus a direct `libaec` `aec_buffer_decode` call, no ecCodes tables | max abs difference 0.0 for all 247 messages |

- **Packing:** every message uses data representation template **5.42
  (CCSDS)**, 16 bits per value (0 for constant fields), CCSDS flags 14,
  block size 32, RSI 128, no bitmap and no missing values. The pinned toolchain
  supports it (ecCodes is built against libaec). The repository's Rust
  [`src/io/grib2.rs`](../../src/io/grib2.rs) is an ERA5 MVP with fixed
  variable/level assumptions and must not be reused for ICON-EU.
- **Label drift between ecCodes versions:** `shortName`, `paramId`, `name`
  and `units` differ for several messages. Examples: `SNOW_CON` paramId 500052
  vs 231057, `ASOB_S` shortName `ASOB_S` vs `avg_snswrf`, `Z0` paramId 173 vs 244.
  `stepRange`/`stepUnits` are printed in minutes by 2.24.2 (`0-5040`) and in
  hours by 2.48.0 (`0-84`). The provider identity is therefore frozen on raw
  octets: discipline/category/number, templates, fixed surfaces and
  statistical processing (`message_identities` in the contract). This is the
  same lesson as #211.
- **Time unit:** `indicatorOfUnitOfTimeRange = 0` (minutes) in every message.
  A decoder that assumes hours mis-times every step by a factor of 60.
- **Conversion:** decoding yields IEEE doubles of 16-bit quantized values.
  The quantum `2^E / 10^D` per message is recorded. No lossy conversion is
  introduced by this audit.

## GRIB2 message semantics

All messages share one grid and one cycle. Parameters are WMO GRIB2 table
4.2 entries read from the octets. Units are the WMO table units as shipped
in the pinned ecCodes definitions (`grib2/tables/19`).

| Source | (d, c, n) and WMO meaning | Level (type) | PDT / statistics | Notes |
| --- | --- | --- | --- | --- |
| U, V | (0,2,2)/(0,2,3) wind components, m s-1 | full levels 1–74 (150/150) | 4.0 instantaneous | earth-relative (`uvRelativeToGrid=0`) |
| W | (0,2,9) geometric vertical velocity, m s-1 | half levels 1–75 (150) | 4.0 | non-zero at the surface half level |
| T, QV, P | (0,0,0) K; (0,1,0) specific humidity kg/kg; (0,3,0) Pa | full levels 1–74 | 4.0 | pressure only on full levels |
| QC, QI | (0,1,22) cloud mixing ratio; (0,1,82) cloud ice mixing ratio, kg/kg | full levels | 4.0 | WMO "mixing ratio" label |
| CLC, CLCT | (0,6,22) / (0,6,1) cloud cover, % | full levels / surface | 4.0 | range exactly [0, 100] |
| HHL | (0,3,6) geometric height, m | half levels 1–75 (150 → 101 MSL) | 4.0, time-invariant | levels 1–13 constant |
| HSURF | (0,3,6) m | ground → MSL (1 → 101) | 4.0, time-invariant | equals HHL(75) exactly |
| FR_LAND | (2,0,0) land cover proportion | surface | 4.0, time-invariant | |
| PS | (0,3,0) Pa | surface (1) | 4.0 | |
| T_2M, TD_2M, U_10M, V_10M | (0,0,0), (0,0,6), (0,2,2), (0,2,3) | 103 at 2 m / 10 m | 4.0 | |
| W_SNOW / H_SNOW | (0,1,60) snow depth water equivalent kg m-2 / (0,1,11) depth m | surface | 4.0 | glacier cells hold 16000 kg m-2 |
| ASHFL_S, ASOB_S | (0,0,11) sensible heat net flux; (0,4,9) net short-wave, W m-2 | surface | 4.8, **average** (type 0) from run start | zero at step 0 |
| AUMFL_S, AVMFL_S | (0,2,17)/(0,2,18) momentum flux, N m-2 | surface | 4.8, average from run start | |
| TOT_PREC | (0,1,52) total precipitation **rate**, kg m-2 s-1 | surface | 4.8, **accumulation** (type 1) from run start | |
| RAIN_GSP, RAIN_CON | (0,1,77)/(0,1,76) large-scale/convective rain **rate** | surface | 4.8, accumulation from run start | |
| SNOW_GSP, SNOW_CON | (0,1,56)/(0,1,55) large-scale/convective snowfall **rate** water equivalent | surface | 4.8, accumulation from run start | |
| OMEGA | (0,2,8) pressure vertical velocity, Pa s-1 | 20 isobaric levels 50–1000 hPa only | 4.0 | |
| MH, HBAS_CON, HTOP_CON, Z0 | mixed-layer depth; convective cloud base/top (MSL, −500 sentinel); roughness | surface | 4.0 | provider diagnostics, not canonical |

DWD files in one variable directory may mix level types. For example
`u/` contains both model-level and 500 hPa files. The contract checker rejects
the pressure-level `U` as `first fixed surface 100`, so directory and file
names never establish identity.

## Grid geometry

- Regular lat/lon (template 3.0), `Ni × Nj = 1377 × 657`, 0.0625°.
  First point 29.5° N / 336.5° E, last 70.5° N / 62.5° E. Scanning mode 64:
  i eastward fastest, j south-to-north. Spherical earth, R = 6 371 229 m. The
  grid is identical in all 247 messages.
- All fields sit on the same cell-centred A-grid points. The native ICON edge
  velocities are already interpolated by DWD.
- Canonical mapping: `nx=1377, ny=657, xlon0=-23.5, ylat0=29.5,
  dx=dy=0.0625`, `minus180_to180`, non-periodic. The only transformation is the
  longitude normalization 336.5° → −23.5°. No reordering is needed, because
  canonical `x_fastest` storage with the southern row first equals the GRIB scan
  order. Classification `SUPPORTED_WITH_PROVEN_TRANSFORMATION`; the Rust lock
  deserializes it into the canonical `HorizontalGrid`.
- Spacing is 6.95 km north–south. East–west it is 6.02 km at 30° N, 4.47 km at
  50° N and 2.38 km at 70° N. This is grid spacing, not effective resolution.
  The export is interpolated from the native ICON-EU triangular grid (documented
  at about 6.5 km), so its effective meteorological resolution is not finer than
  that. This audit does not measure it.

## Canonical field compatibility matrix

The complete 13-column matrix is `field_matrix` in `provider-contract.json`.
The tests check it against all 30 `FIELD_SPECS` identities. `quantity` is the
source-to-canonical quantity mapping; `classification` is the admission into a
canonical snapshot for the field's consumers.

| Canonical id | FLEXPART consumer | ICON source | Quantity | Classification | Blocker |
| --- | --- | --- | --- | --- | --- |
| `wind_u`, `wind_v` | `uu/vv`; advection, PBL | U, V full levels | as-is | `REQUIRES_SCIENTIFIC_DECISION` | #231 vertical coordinate |
| `vertical_velocity` | `ww` from eta-dot/omega with surface W = 0; advection | W half levels | unresolved | `REQUIRES_SCIENTIFIC_DECISION` | #234, #231 |
| `temperature`, `specific_humidity` | `tt/qv`; #30, PBL, convection, deposition, settling | T, QV | as-is | `REQUIRES_SCIENTIFIC_DECISION` | #231 |
| `pressure` | derived from hybrid A/B in #30 / `calcmatrix` | P (provider-supplied) | unresolved | `REQUIRES_SCIENTIFIC_DECISION` | #231 |
| `air_density`, `density_gradient` | FLEXPART-derived | — | derived | `REQUIRES_SCIENTIFIC_DECISION` | #231 |
| `surface_pressure` | `ps` | PS | as-is | `SUPPORTED_AS_IS` | — |
| `orography` | `oro`, AGL/ASL | HSURF (m above MSL, static) | as-is | `SUPPORTED_AS_IS` | — |
| `land_sea_mask` | `lsm` fallback | FR_LAND (static) | as-is | `SUPPORTED_AS_IS` | — |
| `snow_depth` | `sd` → `getvdep` | W_SNOW kg m-2 | ÷ 1000 → m w.e. | `SUPPORTED_WITH_PROVEN_TRANSFORMATION` | — |
| `wind_u10m`, `wind_v10m` | `u10/v10` | U_10M, V_10M | as-is | `SUPPORTED_AS_IS` | — |
| `temperature2m`, `dewpoint2m` | `tt2/td2` | T_2M, TD_2M | as-is | `SUPPORTED_AS_IS` | — |
| `large_scale_precipitation` | `lsprec` via #90/#207/#119 | RAIN_GSP + SNOW_GSP | unresolved | `REQUIRES_SCIENTIFIC_DECISION` | #232, #235 |
| `convective_precipitation` | `convprec` via #90/#207/#119 | RAIN_CON + SNOW_CON | unresolved | `REQUIRES_SCIENTIFIC_DECISION` | #232, #235 |
| `total_cloud_cover` | `tcc` → `interpol_rain` | CLCT % | × 0.01 | `SUPPORTED_WITH_PROVEN_TRANSFORMATION` | — |
| `cloud_total_water` | CLWC+CIWC → `ctwc`, cloud bounds | QC + QI | unresolved (WMO "mixing ratio" label) | `REQUIRES_SCIENTIFIC_DECISION` | #231 |
| `sensible_heat_flux` | `sshf` → `obukhov` | ASHFL_S (average, positive downward) | unresolved | `REQUIRES_SCIENTIFIC_DECISION` | #233 |
| `surface_solar_radiation` | `ssr` → `getvdep` | ASOB_S (average) | unresolved | `REQUIRES_SCIENTIFIC_DECISION` | #233 |
| `surface_stress_eastward/northward` | `sfcstress` → `ustar` | AUMFL_S/AVMFL_S (average) | unresolved | `REQUIRES_SCIENTIFIC_DECISION` | #233 |
| `friction_velocity`, `inverse_obukhov_length` | FLEXPART-derived (`calcpar`) | — | derived | `REQUIRES_SCIENTIFIC_DECISION` | #233, #231 |
| `convective_velocity_scale`, `mixing_height`, `tropopause_height` | FLEXPART-derived (`richardson`, `calcpar`) | — (ICON MH not a substitute) | derived | `REQUIRES_SCIENTIFIC_DECISION` | #231, #233 |
| `land_use_fractions` | `assignland` 13 classes from FLEXPART's own inventory | none (SOILTYP/PLCOV/LAI/Z0 are not substitutes) | missing | `MISSING_SOURCE` | #36 (provider-independent) |

Provider fields that must **not** substitute a canonical quantity are listed
in `non_canonical_provider_fields` (all `UNSUPPORTED`). They include OMEGA
(isobaric only), MH, HBAS_CON/HTOP_CON, CLC, H_SNOW, Z0, and TOT_PREC, which is
used only for closure.

The current `source_quantization` record (#219) supports only the GRIB1
`grid_simple` class. ICON CCSDS packing is not representable there; #232 owns
that question.

## Vertical geometry and W staggering

Findings from actual HHL and model-level messages (`science.vertical`,
`science.surface_w`):

- **Levels:** 75 HHL half levels and 74 full levels. Level 1 is the model
  top (22 770.33 m) and level 75 is the surface. HHL decreases strictly with
  level index in every column; the minimum layer thickness is 17.2 m and the
  lowest layer is at least 19.6 m thick. Levels 1–13 are horizontally constant
  (flat upper coordinate surfaces).
- **Identity:** every HHL and model-level message carries the same
  vertical-grid identity (`nlev=75`, `numberOfVGridUsed=4`, UUID
  `f4e27f3af03da229299dca84d11c2e40`). The time-invariant geometry therefore
  belongs to this run's model levels.
- **Surface/terrain:** `HHL(75) − HSURF` is exactly 0 in every cell. HHL is
  height above mean sea level (second surface 101), so AGL = HHL − HSURF.
- **Pressure:** `P(73) < P(74) < PS` everywhere; `PS − P(74)` ranges
  81.6–133.6 Pa. No interface pressure or A/B coefficients exist.
- **Full-level heights:** taking `z_full(k) = (HHL(k) + HHL(k+1))/2`, the
  hypsometric thickness of layer 73–74 from P/T/QV agrees with the geometric
  thickness to a ratio of median 0.9995 (p01 0.994, p99 1.003). This
  *supports* the half-level-mean convention. It is not a proof, and the
  convention is left to #231.
- **Staggering:** U/V/T/QV/P/QC/QI/CLC sit on full levels (layers between half
  levels k and k+1). W (and TKE) sit on the 75 half levels.
- **Canonical compatibility:** #29 schema v1 `GeometricHeight` holds one 1-D
  profile, so it cannot represent column-varying HHL. #30
  `reconstruct_hybrid_pressure` rejects every non-hybrid coordinate.
  Building the #30 runtime geometry and the #184/#188/#190 shared-height U/V/W
  resources from ICON requires the decision in **#231**. That issue also
  decides whether provider HHL or FLEXPART's hypsometric reconstruction is
  authoritative.
- **W semantics:** W at the surface half level is non-zero over slopes. It
  correlates 0.915 with `U74·∂HSURF/∂x + V74·∂HSURF/∂y` (regression slope 0.79)
  and averages 1.5e-5 m s-1 over flat cells. That is the earth-relative
  kinematic terrain boundary condition. Pinned FLEXPART instead reads
  eta-dot/omega and sets the surface W level to zero
  (`windfields_mod.f90:2470-2473`, eta-dot code 77 relabelled 135 at
  `:2077-2078`). It then converts with `pinmconv`
  (`verttransform_mod.f90:544-621, 1437-1448`), a coordinate-relative motion.
  ICON W is therefore not an identity `GeometricVelocity` input, and the ECMWF
  omega path must not be copied. OMEGA is published only on 20 isobaric levels
  and is `UNSUPPORTED`. **#234** owns the derivation.

## Precipitation, time and accumulation

Findings from the real precipitation messages at steps 0, 1, 2, 3, 47, 48, 78,
81 and 84 (`science.precipitation`):

- **Interpretation:** all five components are WMO **rate** parameters
  (`TOT_PREC` 0-1-52, `SNOW_CON` 55, `SNOW_GSP` 56, `RAIN_CON` 76, `RAIN_GSP` 77,
  kg m-2 s-1) carried with PDT 4.8 statistical processing 1 (accumulation). The
  accumulation starts at the run start (`forecastTime = 0`), its length is
  `60·step` minutes, and the interval end is the valid time. The values are
  **run accumulations** in kg m-2, never reset within the run. The `RAIN_GSP`
  48 h maximum of 187.37 would be 674 536 mm/h as a rate. ecCodes labels
  `TOT_PREC`/`SNOW_*` `kg m-2` but `RAIN_*` `kg m-2 s-1`, so the label is a
  decoder-table artefact.
- **Closure:** `TOT_PREC − (RAIN_GSP + RAIN_CON + SNOW_GSP + SNOW_CON)` is
  within the sum of half quanta in every cell for all steps up to 48 h. At
  78–84 h, three or four cells exceed it (minimum −0.056 kg m-2). No graupel
  component is published or needed for closure in this sample. The
  grid-scale/convective split exists (GSP vs CON); its equivalence to the
  ECMWF/FLEXPART LSP/CP split is **#232**.
- **Monotonicity:** same-run decreases occur in every component, for example
  11 257 `TOT_PREC` cells between 1 h and 2 h and 39 558 `RAIN_CON` cells between
  48 h and 78 h. The most negative delta always equals exactly −½ of the later
  message's quantum, which is the signature of 16-bit CCSDS re-quantization.
  #75 rejects such decreases, and the #216/#219 admission covers only its GRIB1
  class. No clipping or reset is introduced here; **#232** owns the decision.
- **Preparation:** the #202/#207/#119 chain is validated for flex_extract
  `RRINT=0`, `numpf=1` reconstructed point members from ECMWF data. ICON files
  have no such preparation. Applying `darain` to ICON hourly interval means is
  an unproven equivalence claim owned by **#235**. The #211 `/1000` GRIB2 CP
  conversion belongs to a different producer class and does not apply.
- **Canonical fit:** the schema can express `AccumulatedSinceReset` with
  `reset = interval_start = initialization` for each component, but admitting
  it is blocked by the points above.

## Surface fluxes

From `science.surface_fluxes`:

- ASHFL_S, ASOB_S, AUMFL_S and AVMFL_S are PDT 4.8 averages from the run start.
  They are zero at step 0, and no instantaneous counterpart is published.
- The 12–13 h interval mean derived as `13·A13 − 12·A12` has quantization bounds
  of 0.049 W m-2 (ASHFL_S) and 0.098 W m-2 (ASOB_S); the bound grows with lead
  time. The derived ASOB_S mean has a minimum of −0.34 W m-2, beyond that bound.
- **Sign:** the midday land mean of ASHFL_S is −29.7 W m-2, so the field is
  positive downward and the canonical (positive upward) value needs a sign flip.
  AUMFL_S/AVMFL_S correlate +0.80/+0.78 with U_10M/V_10M.
- #120 admits only instantaneous point values for surface-flux time sampling,
  so averages may not be relabelled. **#233** owns the decision.

## 48-hour operational feasibility

- **Cadence (2026-10-31 listings):**
  - 00/06/12/18 UTC cycles: 93 steps, hourly 0–78 h, then 3-hourly to 120 h.
  - 03/09/15/21 UTC cycles: 34 steps, hourly 0–30 h, then 36, 42 and 48 h.
  - A 48 h run therefore needs a main cycle. Starting at its initial time gives
    hourly input over the whole window, and the following steps exist for the
    trailing interval. No preceding meteorology exists inside a cycle, and
    cross-cycle mixing is forbidden.
- **Interval needs:** a 15 min simulation/output interval does not require
  15 min input. Whether hourly input suffices is process-specific: #235 decides
  it for precipitation, #233 for fluxes, and the existing #74/#89 instantaneous
  time interpolation covers state fields once they are canonical.
- **Required fields at every step:** U, V, W, T, QV, P, QC and QI on all 74/75
  levels, plus 16 surface fields. All are published at every step of a main
  cycle.
- **Download for steps 0–48:** 29 841 files and 23.0 GB bz2 (about 24.5 GB
  GRIB2), plus 77 time-invariant files (45.5 MB). DWD serves full-domain files
  only, so a subdomain reduces memory but not download volume.
- **Memory (f32, full domain of 904 689 cells):** about 1.88 GB of canonical
  3-D source fields per time slice (519 level planes). The #30-derived runtime
  geometry adds about 1.89 GB (522 planes: interface pressure, level and
  interface heights AGL/ASL, level pressure, normalized W). That gives about
  3.8 GB per slice and 7.5 GB for two resident time brackets, before particles,
  output grids and the #184/#188 shared-height resources. These scale linearly
  with subdomain area. Host RAM for decode/staging is of the same order per
  slice.
- **Partial downloads:** a missing or hash-mismatched file must fail closed for
  the whole cycle.
- **Reproducibility:** with about 24 h retention, operational and validation use
  must archive bytes with their manifest.
- The #116 benchmark was not run.

## Decoded-data verification results

`decoded-evidence.json` (`science` section, ecCodes 2.24.2 primary; ecCodes
2.48.0 agrees to relative 3e-14, which is float summation order only):

| Check | Result |
| --- | --- |
| Grid identity across 247 messages | 1 distinct grid |
| Cycle and time-unit identity | 0 problems; minutes in every message |
| Vertical-grid UUID across HHL and model levels | 1 UUID |
| HHL count/order | 75, strictly decreasing, levels 1–13 flat |
| HHL(75) − HSURF | 0 m everywhere |
| P(74) < PS, P(73) < P(74) | true everywhere |
| Hypsometric ratio, layer 73–74 | median 0.9995, p01 0.994, p99 1.003 |
| W(75) vs v·∇h | r = 0.915, slope 0.79 |
| Precip closure (≤ 48 h) | 0 cells beyond the half-quantum sum |
| Same-run decreases | present; bounded by ½ later quantum |
| Midday ASHFL_S land mean | −29.7 W m-2 (positive downward) |
| Snow bulk density W_SNOW / H_SNOW (depth > 5 cm) | median 362 kg m-3, range 142–652 |
| Decoders | values bit-identical (2.24.2, 2.48.0, independent libaec) |

## Unresolved fields and scientific blockers

| Issue | Decision owned |
| --- | --- |
| [#231](https://github.com/Grodahn/flexpart-gpu/issues/231) | Canonical #29/#30 vertical geometry for HHL height-based terrain-following sources; authority of provider pressure/heights |
| [#234](https://github.com/Grodahn/flexpart-gpu/issues/234) | FLEXPART-compatible use of ICON geometric W (depends on #231) |
| [#232](https://github.com/Grodahn/flexpart-gpu/issues/232) | GRIB2 precipitation admission: rate-parameter units, GSP/CON composition, CCSDS quantization decreases |
| [#235](https://github.com/Grodahn/flexpart-gpu/issues/235) | Precipitation forcing time semantics and #207/#119 applicability (depends on #232) |
| [#233](https://github.com/Grodahn/flexpart-gpu/issues/233) | Time-averaged surface fluxes and stress: de-averaging, sign, #120 eligibility |
| #36 (existing) | Provider-independent 13-class land-use preparation for dry deposition |

The cloud-water definition (WMO "mixing ratio" label vs ICON specific
content) is recorded in the matrix. It must be settled before
`cloud_total_water` can be admitted; that admission is already blocked by #231.

## Provider comparison and decision

| Role | ICON-EU (candidate A) | ERA5 / ETEX (path B) |
| --- | --- | --- |
| Purpose | future 48 h European forecasts at about 7 km | reproducible scientific validation |
| Data | 0.0625° regular lat/lon, 74 height-based levels, hourly to 78 h | 0.25°, 137 hybrid levels, hourly reanalysis |
| FLEXPART 11.1 path | none (no ICON reader, no pinned oracle) | `readwind_ecmwf` on flex_extract-prepared input |
| Repository evidence | this audit only | #30 real-column oracle, #202/#207/#119/#211/#216/#219 chain, ETEX fixture |
| Limits | open semantics in #231–#235 | historical only; cannot supply forecasts |

The two roles are not interchangeable. ICON-EU is **not** a justified
exception to #32's ERA5/ETEX-first preference today. The verdict is `NO_GO` for
ICON-EU as the first operational ingestion; it is a candidate later adapter
once #231–#235 conclude with supported transformations.

## Follow-up implementation boundaries

These are proposed contracts. They are not implemented here and not yet created
as issues; #32 itself still owns the ERA5-first ingestion.

- **32-B — canonical operational decoding.** Actual provider bytes become
  complete validated #29 snapshots with source/transformation provenance and
  hashes. ERA5/ETEX comes first. ICON-EU may enter only for fields that are
  `SUPPORTED_*` after #231–#235. Missing or ambiguous inputs fail before
  simulation.
- **32-C — provider semantic adapters.** These contain only transformations
  approved by a decision (#231–#235 for ICON-EU; existing decisions for ERA5),
  each with independent evidence and typed failure modes. There are no
  provider-specific defaults in physics.
- **32-D — production integration.** Real provider data runs through
  #30/#31/#76 into the supported production physics without hand-injected
  meteorology. It includes an end-to-end check against independently decoded
  source values.

## Reproduction

Commands are run from the repository root. Downloads go to a new empty
directory outside the repository. Use `python -I`, because the downloads are
untrusted data.

```bash
python -I scripts/provider/icon_eu_fetch.py --request fixtures/meteorology/icon-eu-audit-v1/sample-request.json --output <new-dir> --expect fixtures/meteorology/icon-eu-audit-v1/retrieval-manifest.json
```

```bash
docker run --rm -v <new-dir>:/data:ro -v "$PWD/scripts/provider:/tools:ro" -v <out>:/out --entrypoint sh flexpart-fortran:latest -c 'cd /tmp && python3 -I /tools/icon_eu_audit.py inspect --sample /data --label container-eccodes-2.24.2 --output /out/primary.json'
```

```bash
python -I scripts/provider/icon_eu_audit.py inspect --sample <new-dir> --label eccodes-2.48.0 --output <out>/secondary.json
```

```bash
python -I scripts/provider/icon_eu_audit.py summarize --primary <out>/primary.json --secondary <out>/secondary.json --output <out>/decoded-evidence.json
```

```bash
python -I scripts/provider/icon_eu_audit.py check --sample <new-dir>/2026103100 --contract fixtures/meteorology/icon-eu-audit-v1/provider-contract.json --profile audit_sample
```

- **Fetch:** after the DWD retention window (about 24 h) the fetch fails closed.
  Reproduction then needs a byte-identical archive passed with `--base-url`.
  `--expect` rejects any byte difference.
- **Full-sample check:** the `check` run reports exactly 28 violations, and all
  of them are intended. 22 reject the sampled non-canonical diagnostic messages
  as unknown variables: SOILTYP, PLCOV, LAI, RLAT and RLON once each; MH,
  HBAS_CON, HTOP_CON, H_SNOW and Z0 at three steps; OMEGA at two steps. The other
  6 reject the two 500 hPa `U` files by level type (first surface, second
  surface and level value each).
- **Offline self-tests:** `python -I scripts/provider/test_icon_eu_audit.py`
  runs 31 tests over the committed `raw/` messages. They cover:
  - corrupted and truncated messages, and a bitmap;
  - wrong variable behind a correct file name, wrong cycle (including one hidden
    behind a correct file name), wrong level and wrong grid;
  - incorrect units/parameter and wrong time unit;
  - insufficient temporal coverage;
  - unknown or average statistical processing, and accumulation not from the
    run start;
  - matrix coverage of all 30 canonical fields, evidence references,
    decision/matrix consistency and identity-vs-evidence agreement.

  The ecCodes/libaec cross-checks run when those libraries are installed (all
  31 pass in the pinned container).
- **Rust lock:** `cargo test --test icon_eu_provider_contract` pins the contract
  hash and ties it to `FIELD_SPECS` and the canonical `HorizontalGrid`.

Isolation: the tooling in `scripts/provider/` is research-only. It builds no
canonical snapshot, is not imported by production code, and is not a parallel
ingestion framework.
