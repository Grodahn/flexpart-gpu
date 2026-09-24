# Canonical meteorology contract (schema v1)

Issue: #29 / RISK-03.3G-10a  
Parent: #12

## Boundary

The canonical meteorology module is the only provider-independent input contract for later
vertical transforms (#30), interpolation (#31), operational decoding (#32), and P0 physics.

Provider variable names, GRIB parameter ids, NetCDF variable names, provider units, and
provider sign conventions must be normalized before a Snapshot is accepted.

Schema identity:

- id: flexpart-gpu.canonical-meteorology
- version: 1

The Rust implementation lives in src/meteorology/mod.rs.

## Canonical invariants

- Canonical axes are explicit. Schema v1 serializes volume fields as X,Y,Z, surface
  fields as X,Y, and FLEXPART land-use fractions as X,Y,Class(13). Every field declares
  `storage_order`; schema v1 supports `x_fastest` with the last axis outermost.
- `xlon0_deg` / `ylat0_deg` are the X=0/Y=0 **scalar cell-center** coordinates.
  An X-face is half a `dx` west of the corresponding center; a Y-face is half a
  `dy` south. This is schema-v1 semantics, not a provider convention.
- X periodicity is also canonical: a grid wraps iff `nx * dx_deg == 360°` within
  schema tolerance. Other grids are non-periodic and their full cell coverage must
  fit inside the declared longitude domain. Full latitude cell coverage must remain
  inside [-90°, 90°]. Invalid/wrapping regional geometry fails at the boundary.
- Horizontal and vertical staggering are validated per field rather than by dimensions alone.
  Schema v1 permits cell-centered fields generally, plus `wind_u` on X faces, `wind_v` on Y
  faces, and `vertical_velocity` on level interfaces. Scalar face staggering, cross-axis wind
  staggering, surface-field vertical staggering, and scalar interface staggering fail closed.
- Units and sign conventions are part of the field contract and are validated.
- Surface sensible heat flux is positive upward at the canonical boundary. This matches
  the PBL API in src/io/pbl_params.rs; ECMWF/provider conventions must be converted by
  the adapter.
- Canonical vertical velocity is positive upward in m/s. Pressure velocity or native
  eta-dot representations must remain outside the physics boundary until normalized.
- Precipitation is represented as water-equivalent interval total in kg/m2, or as an
  explicit accumulation with reset metadata. A scalar with unknown accumulation window
  is invalid. Deriving interval amounts/rates from `AccumulatedSinceReset` fields is
  owned by #75; the canonical rules and fail-closed cases are documented in
  `docs/accumulation-contract.md`.
- Hybrid coordinates carry the native **interface/half-level** A/B coefficients, an explicit
  reference surface pressure used only to make serialized reference pressures checkable, and an
  explicit dependency on canonical surface_pressure. #30 owns reconstruction at the actual local
  surface pressure. Full-level coefficients are derived from adjacent interfaces rather than stored
  as a lossy substitute.
- Missing/non-finite values are rejected by schema-v1 validation rather than silently
  substituted.
- Time-invariant ancillary fields use explicit `static` semantics. In schema v1 this
  applies to `orography`, `land_sea_mask`, and `land_use_fractions`; their serialized
  timestamp is provenance-only and must never trigger temporal interpolation.
- Every non-static field in one `Snapshot` must share the same validity timestamp and
  calendar. Interval/accumulated fields may have their own start/reset metadata, but the
  interval end is the common snapshot validity time. Mixed dynamic times/calendars fail closed.
- Unknown calendars or enum values fail deserialization. Supported calendars in v1 are
  Gregorian and proleptic Gregorian.

## P0 field matrix

This crosswalk is derived backwards from the pinned FLEXPART 11.1 oracle
`c70586c2b7f5258850705325881c61f557ea9bd8`. File/routine names below are oracle
trace points, not provider identifiers required by the canonical schema. Provider-specific GRIB
or NetCDF names remain #32 adapter concerns.

The matrix distinguishes **normalized/source forcing** from **FLEXPART-derived diagnostics**.
`Requirements::p0_complete()` describes the complete physics-ready P0 snapshot. The named
physics requirement sets are derived directly from `FIELD_SPECS`; `advection` is deliberately
wind-only (`wind_u`, `wind_v`, `vertical_velocity`), while thermodynamic fields belong to the
PBL/convection/deposition/transform paths that actually consume them.

The compact contract table below is mechanically tied to `FIELD_SPECS` in
`src/meteorology/mod.rs`. Unit tests fail when a field, unit, sign, temporal policy or
physics requirement-set membership drifts from this table. The detailed oracle trace tables
that follow add scientific provenance and interpolation notes on top of this machine-checked core.

<!-- BEGIN GENERATED FIELD SPEC MATRIX -->
| Canonical id | Unit | Sign | Temporal policy | Machine requirement sets |
| --- | --- | --- | --- | --- |
| `wind_u` | `meter_per_second` | `positive_eastward` | `instantaneous` | `advection`, `pbl_turbulence` |
| `wind_v` | `meter_per_second` | `positive_northward` | `instantaneous` | `advection`, `pbl_turbulence` |
| `vertical_velocity` | `meter_per_second` | `positive_upward` | `instantaneous` | `advection` |
| `temperature` | `kelvin` | `signed_scalar` | `instantaneous` | `pbl_turbulence`, `convection`, `wet_deposition`, `settling` |
| `specific_humidity` | `kilogram_per_kilogram` | `non_negative` | `instantaneous` | `pbl_turbulence`, `convection`, `wet_deposition` |
| `pressure` | `pascal` | `non_negative` | `instantaneous` | `convection` |
| `air_density` | `kilogram_per_cubic_meter` | `non_negative` | `instantaneous` | `pbl_turbulence`, `wet_deposition`, `settling` |
| `density_gradient` | `kilogram_per_quartic_meter` | `signed_scalar` | `instantaneous` | `pbl_turbulence` |
| `surface_pressure` | `pascal` | `non_negative` | `instantaneous` | `pbl_turbulence`, `convection`, `dry_deposition` |
| `orography` | `meter` | `signed_scalar` | `static` | — |
| `land_sea_mask` | `fraction` | `non_negative` | `static` | — |
| `snow_depth` | `meter` | `non_negative` | `instantaneous` | `dry_deposition` |
| `wind_u10m` | `meter_per_second` | `positive_eastward` | `instantaneous` | `pbl_turbulence` |
| `wind_v10m` | `meter_per_second` | `positive_northward` | `instantaneous` | `pbl_turbulence` |
| `temperature2m` | `kelvin` | `signed_scalar` | `instantaneous` | `pbl_turbulence`, `convection`, `dry_deposition` |
| `dewpoint2m` | `kelvin` | `signed_scalar` | `instantaneous` | `pbl_turbulence`, `convection`, `dry_deposition` |
| `large_scale_precipitation` | `kilogram_per_square_meter` | `non_negative` | `precipitation_amount` | `wet_deposition`, `dry_deposition` |
| `convective_precipitation` | `kilogram_per_square_meter` | `non_negative` | `precipitation_amount` | `wet_deposition`, `dry_deposition` |
| `total_cloud_cover` | `fraction` | `non_negative` | `instantaneous` | `wet_deposition` |
| `cloud_total_water` | `kilogram_per_kilogram` | `non_negative` | `instantaneous` | `wet_deposition` |
| `sensible_heat_flux` | `watt_per_square_meter` | `positive_upward_flux` | `surface_flux_rate` | `pbl_turbulence` |
| `surface_solar_radiation` | `watt_per_square_meter` | `non_negative` | `surface_flux_rate` | `dry_deposition` |
| `surface_stress_eastward` | `newton_per_square_meter` | `positive_eastward` | `surface_flux_rate` | `pbl_turbulence` |
| `surface_stress_northward` | `newton_per_square_meter` | `positive_northward` | `surface_flux_rate` | `pbl_turbulence` |
| `friction_velocity` | `meter_per_second` | `non_negative` | `instantaneous` | `pbl_turbulence`, `dry_deposition` |
| `convective_velocity_scale` | `meter_per_second` | `non_negative` | `instantaneous` | `pbl_turbulence` |
| `mixing_height` | `meter` | `non_negative` | `instantaneous` | `pbl_turbulence` |
| `tropopause_height` | `meter` | `non_negative` | `instantaneous` | `pbl_turbulence` |
| `inverse_obukhov_length` | `per_meter` | `signed_scalar` | `instantaneous` | `pbl_turbulence`, `dry_deposition` |
| `land_use_fractions` | `fraction` | `non_negative` | `static` | `dry_deposition` |
<!-- END GENERATED FIELD SPEC MATRIX -->

<!-- BEGIN DETAILED FIELD TRACE MATRIX -->
### Normalized/source forcing

| Canonical id | Physical meaning | Pinned FLEXPART 11.1 trace | Layout / canonical unit and sign | Time semantics | #31 interpolation / handoff | Consumers |
| --- | --- | --- | --- | --- | --- | --- |
| `wind_u` | horizontal wind, east component | `windfields_mod::readwind_ecmwf` U -> `uuh`; `verttransform_ecmwf_windfields` -> `uu` | X,Y,Z; cell center or X face; m/s eastward + | instantaneous | continuous 3-D spatial + temporal sampling | advection; PBL/turbulence |
| `wind_v` | horizontal wind, north component | readwind V -> `vvh`; vertical transform -> `vv` | X,Y,Z; cell center or Y face; m/s northward + | instantaneous | continuous 3-D spatial + temporal sampling | advection; PBL/turbulence |
| `vertical_velocity` | geometric vertical air velocity at the physics boundary | readwind omega/eta-dot -> `wwh`; `verttransform_ecmwf_windfields` converts with `pinmconv` to `ww` | X,Y,Z; level center or interface; m/s upward + | instantaneous | #30 performs representation/sign conversion; #31 samples resulting field | vertical advection |
| `temperature` | 3-D air temperature | readwind T -> `tth`; vertical transform -> `tt` | X,Y,Z level center; K | instantaneous | continuous 3-D | #30; PBL; Emanuel convection; wet deposition; settling |
| `specific_humidity` | water-vapour mass fraction | readwind Q -> `qvh`; vertical transform -> `qv` | X,Y,Z level center; kg/kg, non-negative | instantaneous | continuous 3-D | #30; PBL; Emanuel convection; cloud fallback |
| `surface_pressure` | surface pressure | readwind SP -> `ps` | X,Y cell center; Pa, positive | instantaneous | continuous 2-D | #30 hybrid pressure; PBL; Emanuel convection; dry deposition |
| `orography` | surface elevation above mean sea level | readwind geopotential -> `oro` after division by g | X,Y; m ASL, signed (below-sea-level terrain allowed) | `static`; timestamp provenance-only | static spatial field; no temporal interpolation | #30 AGL/ASL |
| `land_sea_mask` | land fraction / land-sea discriminator | readwind -> `lsm`; `drydepo_mod::assignland` only uses it as fallback when detailed inventory is absent | X,Y; fraction [0,1] | `static`; timestamp provenance-only | static | surface provenance/fallback only; P0 dry deposition requires explicit land-use fractions |
| `snow_depth` | snow water equivalent depth | readwind SD -> `sd`; passed by `getfields_mod::calcpar` to `drydepo_mod::getvdep` | X,Y; m water equivalent, non-negative | instantaneous | continuous 2-D forcing at met time | dry deposition |
| `wind_u10m` | 10-m eastward wind | readwind -> `u10` | X,Y; m/s eastward + | instantaneous | continuous 2-D | `pbl_profile` fallback / near-surface diagnosis |
| `wind_v10m` | 10-m northward wind | readwind -> `v10` | X,Y; m/s northward + | instantaneous | continuous 2-D | `pbl_profile` fallback / near-surface diagnosis |
| `temperature2m` | 2-m air temperature | readwind -> `tt2` | X,Y; K | instantaneous | continuous 2-D | PBL; Emanuel convection; dry deposition |
| `dewpoint2m` | 2-m dew-point temperature | readwind -> `td2` | X,Y; K | instantaneous | continuous 2-D | PBL; Emanuel convection; dry-deposition RH derivation |
| `large_scale_precipitation` | water-equivalent large-scale precipitation amount over a known interval | readwind -> `lsprec`; FLEXPART internal forcing is converted to mm/h before `interpol_rain` | X,Y; kg/m2 interval amount, non-negative | interval total or accumulation with explicit reset | #75 converts amount to interval rate; #31 applies FLEXPART-compatible rain interpolation | wet deposition; dry-deposition precipitation forcing |
| `convective_precipitation` | water-equivalent convective precipitation amount over a known interval | readwind -> `convprec`; internal forcing converted to mm/h | X,Y; kg/m2 interval amount, non-negative | interval total or accumulation with explicit reset | same special rain class as LSP | wet deposition; dry deposition. **Not** an Emanuel-convection input |
| `total_cloud_cover` | grid-cell total cloud fraction | readwind -> `tcc`; `wetdepo_mod::get_wetscav -> interpol_rain` returns `cc` | X,Y; fraction [0,1] | instantaneous | wet-deposition rain/cloud interpolation class | wet deposition |
| `cloud_total_water` | total cloud-water mixing ratio (liquid + ice); phase is not encoded in this field | readwind may supply separate CLWC/CIWC or combined QC; `verttransform_ecmwf_cloud` combines separate fields before cloud integration | X,Y,Z level center; kg/kg, non-negative | instantaneous | #32 normalizes provider representation to total water; #30 normalizes vertical geometry | wet-deposition cloud-state derivation |
| `sensible_heat_flux` | surface sensible heat-flux rate | readwind -> `sshf`; `calcpar -> obukhov` | X,Y; W/m2, upward + at canonical boundary | instantaneous or explicit interval mean; accumulated energy is invalid past boundary | continuous 2-D | PBL/Obukhov |
| `surface_solar_radiation` | surface short-wave/net-solar radiation rate used by dry deposition | readwind -> `ssr`; `calcpar -> getvdep` as `gr` | X,Y; W/m2, non-negative | instantaneous or explicit interval mean | continuous 2-D | dry deposition |
| `surface_stress_eastward` | eastward turbulent surface-stress component | readwind EWSS; FLEXPART forms `sfcstress=sqrt(ewss^2+nsss^2)` | X,Y; N/m2 eastward + | instantaneous or explicit interval mean | components normalized before PBL diagnosis | friction-velocity diagnosis |
| `surface_stress_northward` | northward turbulent surface-stress component | readwind NSSS; combined into `sfcstress` | X,Y; N/m2 northward + | instantaneous or explicit interval mean | same as EWSS | friction-velocity diagnosis |
| `land_use_fractions` | fractional cover of each FLEXPART dry-deposition land-use class | `drydepo_mod::assignland -> xlanduse(ix,jy,1:numclass)`; pinned contract has 13 classes and normalizes fractions to unity | X,Y,Class(13); fraction [0,1], per-cell sum=1 | `static`; timestamp provenance-only | no generic temporal interpolation; spatial preparation must preserve normalized fractions | `drydepo_mod::getvdep` / resistance weighting |

All required source/forcing fields reject non-finite values. Missing required fields fail before physics.
The land-use field is deliberately **13 fractions**, not one categorical class index: FLEXPART computes
weighted deposition over all classes. Canonical class-axis index 0..12 maps exactly to Oracle
`xlanduse` index 1..13; the class labels and Wesely resistance-table ordering are the pinned
`readlanduse`/`sfcdepo.t` ordering and must not be re-sorted by an adapter. Likewise,
`roughness_length` is not a P0 meteorological field:
the dry-deposition path uses class-table roughness and a water-surface value derived internally.

### FLEXPART-derived physics-ready fields

These are canonical fields because later consumers need the derived state, but they are not independent
provider requirements.

| Canonical id | Physical meaning / oracle derivation | Layout / unit | Time semantics | Consumers |
| --- | --- | --- | --- | --- |
| `pressure` | layer pressure; ECMWF path reconstructs with `akz + bkz * ps` / half-level `akm + bkm * ps` in `verttransform_mod` and `conv_mod::calcmatrix` | X,Y,Z; Pa | instantaneous derived | #30; convection; deposition thermodynamics |
| `air_density` | moist-air density from pressure/virtual temperature in `verttransform_ecmwf_heights` -> `rho` | X,Y,Z; kg/m3 | instantaneous derived | turbulence; cloud-water integration; settling |
| `density_gradient` | d(rho)/dz in `verttransform_ecmwf_windfields`; the actual finite-difference formula has SI unit kg/m4 even though an old Fortran declaration comment says kg/m2 | X,Y,Z; kg/m4, signed | instantaneous derived | PBL/turbulent vertical transport |
| `friction_velocity` | `ustar`; `getfields_mod::calcpar -> scalev(ps,tt2,td2,sfcstress)` or profile fallback | X,Y; m/s | instantaneous derived | PBL; dry deposition |
| `convective_velocity_scale` | `wstar`, diagnosed by `calcpar -> richardson` together with mixing height | X,Y; m/s | instantaneous derived | PBL/turbulence |
| `mixing_height` | boundary-layer height `hmix`, diagnosed in `calcpar -> richardson` | X,Y; m AGL | instantaneous derived | PBL/turbulence |
| `tropopause_height` | thermal tropopause `tropopause`, diagnosed in `calcpar` from thermodynamic profile | X,Y; m | instantaneous derived | vertical-regime logic |
| `inverse_obukhov_length` | `oli=1/L`; `calcpar -> obukhov` from surface/thermodynamic forcing | X,Y; 1/m, signed | instantaneous derived | PBL; dry deposition |

<!-- END DETAILED FIELD TRACE MATRIX -->

### Required derived wet-deposition state (not provider fields)

FLEXPART does **not** preserve provider liquid/ice separation as a scavenging input.
#32 therefore normalizes separate CLWC+CIWC or provider-combined QC into canonical
`cloud_total_water`. The preparation path `verttransform_ecmwf_cloud/identify_cloud` produces:

| Derived quantity | Oracle trace and canonical handoff |
| --- | --- |
| column total cloud water `ctwc` | vertical integral `cloud_total_water * rho * dz`, kg/m2 |
| cloud bottom | first cloudy model height, metres above the internal surface-relative height origin |
| cloud top | top cloudy model height, with FLEXPART cloud-bound corrections |
| precipitation rate | #31 converts canonical interval amounts to the oracle mm/h forcing before `interpol_rain` |

`wetdepo_mod::get_wetscav` then calls `interpol_rain` for LSP, convective precipitation,
total cloud cover, particle temperature, `ctwc`, cloud bottom and cloud top. Rain/snow activation
uses temperature; provider CLWC vs CIWC is not passed as two independent scavenging coefficients.
The typed process-state representation and branch-by-branch rate validation remain #33, but there is
no longer an undocumented meteorological input between #29 and #33.

### Process-to-field proof map

- **Advection / #7:** `uu/vv/ww` -> `wind_u/wind_v/vertical_velocity`. #30 owns conversion of
  provider pressure/eta vertical motion into canonical upward-positive m/s; #31 owns sampling.
- **PBL/turbulence / #8:** `calcpar` derives `ustar/oli/hmix/wstar/tropopause` from
  `ps,tt2,td2,u10,v10,sshf,sfcstress` plus the lowest 3-D thermodynamic/wind profile.
  `rho/drhodz` are consumed by turbulent vertical transport.
- **Emanuel convection / #9:** the pinned `conv_mod::convmix` interpolates `ps,tt2,td2`
  and the 3-D `tth/qvh` thermodynamic profile; `calcmatrix` reconstructs layer/interface
  pressure from hybrid A/B + surface pressure before calling Emanuel convection. The existing
  simplified Rust convection helper's `convective_precipitation/wstar/hmix` inputs are therefore
  **not** the normative #9 meteorology contract; #23/#24 must replace that provisional path.
- **Wet deposition / #13:** `get_wetscav -> interpol_rain` consumes LSP, convective precipitation,
  TCC, particle temperature, CTWC and cloud bounds. CTWC/bounds are derived from the canonical
  cloud-water/thermodynamic state as described above.
- **Dry deposition / #14:** `getfields_mod::calcpar -> drydepo_mod::getvdep` consumes
  `ustar,tt2,ps,L,ssr,RH,precipitation_rate,sd` and `xlanduse(13)`. RH is derived from
  `td2/tt2/ps`; total precipitation rate is derived from LSP+convective precipitation; season is
  derived from simulation date and latitude. Roughness is a land-use table/internal water-surface
  derivation, not a provider meteorology field.
- **Gravitational settling / #14/#35:** `settling_mod::get_settling` consumes local air
  temperature and air density in addition to species/carrier properties.

The corresponding machine-readable subsets are `Requirements::pbl_turbulence()`,
`Requirements::convection()`, `Requirements::wet_deposition()`,
`Requirements::dry_deposition()`, and `Requirements::settling()`. The detailed source/derived
field tables are also coverage-checked against every canonical `FieldId`, so adding a new
canonical field without an oracle/consumer trace entry fails the contract tests.

### Missing-value and temporal policy

- Schema-v1 has no provider sentinel values: required missing fields, NaN/Inf and impossible domains
  fail at the canonical boundary. Specific humidity is additionally constrained to the physical
  mass-fraction range `[0,1]`.
- Normal dynamic state variables use `instantaneous` snapshot semantics and must share one
  validity time/calendar per Snapshot. Orography, land/sea mask and land-use fractions use
  explicit `static` semantics; their timestamp is provenance-only.
- Surface rate/flux fields (`sensible_heat_flux`, `surface_solar_radiation`, stress components)
  may be instantaneous or explicit interval means; accumulated energy/momentum must be normalized
  before crossing the boundary.
- Precipitation remains an interval amount (`kg/m2`, numerically equivalent to mm water amount)
  with explicit start/end and reset metadata. For `accumulated_since_reset`, schema v1 requires
  `reset_epoch_seconds == interval_start_epoch_seconds`; a different reset origin is ambiguous and
  fails normalization. #31 performs the only amount-to-rate conversion.
- Land-use fractions are bounded [0,1] and must sum to 1 within `1e-5` independently for each cell.
- Provider-specific missing-value handling, accumulated-field decoding and unit conversion belong to
  #32 and must fail rather than silently default when source semantics are ambiguous.

## Handoff to #30/#31/#32

#29 records coordinate and staggering meaning only.

- #30 implements hybrid pressure/height and AGL/ASL transformations.
- #31 implements spatial/vertical/temporal sampling and accumulated-field interpolation.
- #32 maps concrete GRIB/NetCDF/provider variables into this schema.

No provider decoder should require a physics module to understand provider metadata.

## Known implementation status on the #29 branch

Implemented:

- versioned serializable schema/types;
- central `FIELD_SPECS` table as the single source for canonical unit/sign/temporal policy
  and named physics requirement-set membership; `Requirements::*` is derived from it;
- machine-checked compact field matrix in this document; contract tests fail on stale,
  missing or extra rows;
- explicit grid, vertical, unit, sign, calendar, time/accumulation and staggering metadata;
- fail-closed validation of schema identity, dimensions, required fields, units/signs,
  vertical metadata, accumulation windows/resets and basic physical domains;
- machine-readable Snapshot provenance plus repository-wide run-manifest provenance containing
  canonical meteorology schema id/version and a SHA-256 of the checked-in identity source; the
  manifest writer can additionally bind and hash the actual canonical meteorology snapshot(s) used
  by a run, while runs that do not consume canonical meteorology record that no runtime input was bound;
- checked-in synthetic 3-D fixture and round-trip test;
- checked-in real-data native-level fixture derived from the repository's
  independently sourced ERA5/ETEX corpus
  (fixtures/meteorology/era5-etex-native-v1.json + .provenance.json): 2x2 cells,
  137 native levels, 1994-10-23 15:00 UTC, fields wind_u/wind_v/temperature/
  specific_humidity/surface_pressure, with all 138 native hybrid interface A/B coefficients
  preserved and reference full-level pressures checked against the FLEXPART 11.1 half-level
  averaging convention;
- Requirements::real_data_native_levels() covering exactly the represented fields;
  the real-data fixture intentionally does not satisfy Requirements::advection() because
  reconstructed 3-D pressure and upward-positive vertical velocity are #30 transforms;
- provenance integrity test that recomputes the artifact digest, checks the pinned
  source checksums, the documented retrieval identity, the selected slice indices,
  the represented/omitted field lists, the baseline FLEXPART 11.1 oracle reference and
  the half-level averaging consistency of level_values/interface_values;
- representative fail-closed tests for missing fields, unit mismatch, dimensions,
  unsupported calendar values, field-specific staggering, invalid hybrid metadata and missing
  hybrid `surface_pressure`, ambiguous/invalid accumulation reset semantics, specific-humidity
  bounds, whole-grid longitude/latitude extent, regional seam crossing, canonical global
  periodicity, static ancillary semantics and mixed dynamic validity times/calendars;

The P0 meteorology crosswalk is now frozen for #29. The compact semantic matrix is generated from
`FIELD_SPECS`, and tests require the detailed oracle/consumer trace section to cover every canonical
field. Process-specific formula/rate parity remains
owned by #23/#33/#36, and provider-adapter normalization remains #32 non-scope; neither requires an
additional meteorological input to be invented outside this contract.
