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

- Canonical axes are explicit. Schema v1 serializes volume fields as X,Y,Z and surface
  fields as X,Y.
- Units and sign conventions are part of the field contract and are validated.
- Surface sensible heat flux is positive upward at the canonical boundary. This matches
  the PBL API in src/io/pbl_params.rs; ECMWF/provider conventions must be converted by
  the adapter.
- Canonical vertical velocity is positive upward in m/s. Pressure velocity or native
  eta-dot representations must remain outside the physics boundary until normalized.
- Precipitation is represented as water-equivalent interval total in kg/m2, or as an
  explicit accumulation with reset metadata. A scalar with unknown accumulation window
  is invalid.
- Hybrid coordinates carry A/B coefficients and an explicit dependency on canonical
  surface_pressure. #30 owns the actual transformation.
- Missing/non-finite values are rejected by schema-v1 validation rather than silently
  substituted.
- Unknown calendars or enum values fail deserialization. Supported calendars in v1 are
  Gregorian and proleptic Gregorian.

## P0 field matrix

The source names below are the FLEXPART 11.1/com_mod or readwind concepts already
documented by the current Rust port. The matrix is intentionally independent of a
specific provider encoding.

| Canonical id | FLEXPART 11.1 counterpart / trace | Canonical unit/sign | Rank | Main consumers |
| --- | --- | --- | --- | --- |
| wind_u | com_mod uu; readwind wind U | m/s, eastward + | 3-D | advection, PBL, convection |
| wind_v | com_mod vv; readwind wind V | m/s, northward + | 3-D | advection, PBL, convection |
| vertical_velocity | com_mod ww after vertical normalization; native eta-dot/omega must be converted before physics | m/s, upward + | 3-D | vertical advection, convection |
| temperature | com_mod tt | K | 3-D | vertical transform, PBL, convection, wet/dry deposition |
| specific_humidity | com_mod qv | kg/kg | 3-D | vertical transform, convection |
| pressure | com_mod prs / reconstructed model pressure | Pa | 3-D | vertical transform, density, deposition |
| air_density | com_mod rho; derived from canonical thermodynamic state where not supplied | kg/m3 | 3-D | turbulence/deposition support |
| density_gradient | com_mod drhodz | kg/m4 | 3-D | vertical transport forcing |
| surface_pressure | com_mod ps; ECMWF sp | Pa | 2-D | hybrid pressure reconstruction, PBL |
| orography | com_mod oro; ECMWF geopotential z normalized to height | m ASL | 2-D | terrain/AGL-ASL transform |
| land_sea_mask | com_mod lsm | fraction 0..1 | 2-D | land/surface mapping |
| wind_u10m | com_mod u10; ECMWF 10u | m/s, eastward + | 2-D | PBL/friction diagnostics |
| wind_v10m | com_mod v10; ECMWF 10v | m/s, northward + | 2-D | PBL/friction diagnostics |
| temperature2m | com_mod tt2; ECMWF 2t | K | 2-D | PBL, dry deposition |
| dewpoint2m | com_mod td2; ECMWF 2d | K | 2-D | near-surface moisture diagnostics |
| large_scale_precipitation | com_mod lsprec; ECMWF lsp normalized with interval/reset semantics | kg/m2 interval total | 2-D | wet deposition |
| convective_precipitation | com_mod convprec; ECMWF cp normalized with interval/reset semantics | kg/m2 interval total | 2-D | convection, wet deposition |
| total_cloud_cover | readwind total-cloud-cover input; wet-deposition cloud exposure | fraction 0..1 | 2-D | wet deposition |
| cloud_liquid_water | cloud-water path feeding FLEXPART wet-deposition ctwc semantics | kg/kg | 3-D | wet deposition, convection |
| cloud_ice_water | cloud-ice path feeding FLEXPART wet-deposition ctwc/phase semantics | kg/kg | 3-D | wet deposition |
| sensible_heat_flux | com_mod sshf after sign normalization | W/m2, upward + | 2-D | PBL/Obukhov diagnosis |
| surface_solar_radiation | com_mod ssr | W/m2 | 2-D | PBL diagnostics |
| surface_stress_eastward | ECMWF ewss/readwind stress input before magnitude derivation | N/m2, eastward + | 2-D | PBL/friction velocity |
| surface_stress_northward | ECMWF nsss/readwind stress input before magnitude derivation | N/m2, northward + | 2-D | PBL/friction velocity |
| friction_velocity | com_mod ustar; may be diagnosed from canonical stress | m/s | 2-D | PBL, dry deposition |
| convective_velocity_scale | com_mod wstar | m/s | 2-D | PBL, convection |
| mixing_height | com_mod hmix | m AGL | 2-D | turbulence, convection, deposition |
| tropopause_height | com_mod tropopause | m | 2-D | vertical transport regime |
| inverse_obukhov_length | com_mod oli | 1/m, signed | 2-D | PBL, dry deposition |
| roughness_length | dry-deposition/PBL surface mapping input; no provider name is allowed past normalization | m | 2-D | PBL, dry deposition |
| land_use_class | FLEXPART land-use mapping/table input; class ids must be explicit and provenance-tracked | class index | 2-D | dry deposition |

### Current consumer cross-check

The existing code already exposes the following provider-independent consumer surfaces:

- src/wind/mod.rs: uu/vv/ww/tt/qv/prs/rho/drhodz equivalents plus ps/u10/v10/tt2/td2,
  lsprec/convprec/sshf/ssr/ustar/wstar/hmix/tropopause/oli and oro/lsm.
- src/io/pbl_params.rs: surface pressure, 2 m temperature, 10 m winds, stress,
  sensible heat flux, radiation, hmix, ustar and inverse Obukhov length.
- src/physics/wet_scavenging.rs: large-scale/convective precipitation, cloud cover,
  local temperature and cloud water/ice content.
- src/physics/deposition.rs: ustar, Obukhov length, temperature, pressure plus
  land-use/roughness/resistance mapping.
- src/physics/convection.rs: the current simplified path consumes convective precipitation,
  wstar and boundary-layer height; #23/#24 will replace this with the full v11.1 contract.

## Handoff to #30/#31/#32

#29 records coordinate and staggering meaning only.

- #30 implements hybrid pressure/height and AGL/ASL transformations.
- #31 implements spatial/vertical/temporal sampling and accumulated-field interpolation.
- #32 maps concrete GRIB/NetCDF/provider variables into this schema.

No provider decoder should require a physics module to understand provider metadata.

## Known implementation status on the #29 branch

Implemented:

- versioned serializable schema/types;
- explicit grid, vertical, unit, sign, calendar, time/accumulation and staggering metadata;
- fail-closed validation of schema identity, dimensions, required fields, units/signs,
  vertical metadata, accumulation windows/resets and basic physical domains;
- machine-readable Provenance value containing schema id/version;
- checked-in synthetic 3-D fixture and round-trip test;
- checked-in real-data native-level fixture derived from the repository's
  independently sourced ERA5/ETEX corpus
  (fixtures/meteorology/era5-etex-native-v1.json + .provenance.json): 2x2 cells,
  137 native levels, 1994-10-23 15:00 UTC, fields wind_u/wind_v/temperature/
  specific_humidity/surface_pressure, with full hybrid A/B metadata normalized per the
  FLEXPART 11.1 half-level averaging convention;
- Requirements::real_data_native_levels() covering exactly the represented fields;
  the real-data fixture intentionally does not satisfy Requirements::advection() because
  reconstructed 3-D pressure and upward-positive vertical velocity are #30 transforms;
- provenance integrity test that recomputes the artifact digest, checks the pinned
  source checksums, the documented retrieval identity, the selected slice indices,
  the represented/omitted field lists, the baseline FLEXPART 11.1 oracle reference and
  the half-level averaging consistency of level_values/interface_values.

Still required before #29 can close:

- wire schema id/version into the repository-wide run-manifest path that owns candidate
  provenance;
- finish the pinned FLEXPART 11.1 source crosswalk for cloud phase/water and land-use/season
  mapping when #23/#33/#36 oracle contracts expose the exact v11.1 branches;
- add provider-adapter normalization in #32, not here.
