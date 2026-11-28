# ReleaseSpec v1

`SimulationConfig::load` uses the existing RELEASES parser and calls
`ReleaseConfig::normalize_forward_mass` with the validated #10 species table.
The forward driver rechecks every inventory identity and lane before it
initializes the particle store. `ValidationCaseManifest::normalize_release`
maps the #51 v2 forward declaration to the same canonical validator.

The JSON envelope is `{"schema_version":1,"release":{...}}`. Deserialization
rejects unsupported versions, unknown fields and enum variants. A physical
interpretation change requires a new version and explicit migration.
Normalization preserves release record order, sorts inventories by explicit
mass slot, and canonicalizes signed zero. The typed geometry uses geographic
WGS84 decimal degrees, with east-positive longitude, in [-180,180] and [-90,90].
Timestamps are integer UTC Unix seconds, limited to Gregorian years 1 through
9999 without leap seconds. Heights are in metres relative to AGL or ASL.

## RELEASES adapters

The project adapter accepts repeated `&RELEASE ... /` blocks and one
comma-separated assignment record per line.

| Field | Accepted project syntax |
| --- | --- |
| Release ID | `NAME`, `RELEASE_NAME`, `ID` or `COMMENT`. If absent, the ID is derived from the one-based record ordinal. |
| Timing | `START_TIME` / `START` and `END_TIME` / `END`. Compact UTC timestamps can have 8, 10, 12 or 14 digits, with omitted trailing time fields set to zero. ISO 8601 requires seconds and a trailing `Z`. Equal endpoints mean `instant`. Strictly increasing endpoints mean `uniform_window`. An optional `PROFILE` must agree. |
| Point | `LON` / `LONGITUDE` and `LAT` / `LATITUDE`. |
| Horizontal extent | All four of `LON_MIN` / `XPOINT1` / `XLON1`, `LON_MAX` / `XPOINT2` / `XLON2`, `LAT_MIN` / `YPOINT1` / `YLAT1`, `LAT_MAX` / `YPOINT2` / `YLAT2`. Distinct bounds declare a distribution uniform in angular coordinates. One axis may be fixed. Antimeridian wraparound is unsupported. |
| Height | Both `Z_MIN` / `Z1` / `Z_BOTTOM` and `Z_MAX` / `Z2` / `Z_TOP`. Require `VERTICAL_REF` / `HEIGHT_REF` as `agl` or `asl`, or `ZKIND=1` or `2`. Equal heights are fixed. Distinct bounds declare a distribution uniform along the metre coordinate. AGL heights must be nonnegative. |
| Inventory | One scalar `MASS_KG` / `MASS` / `XMASS`, or one quoted list `MASS_SPECIES` / `SPECIES_MASS` / `SPECIES_MASSES`. Legacy aliases have a defined kg meaning. Explicit `MASS_UNIT` / `MASS_UNITS` supports `kg` or `g`; `MASS_KG` requires kg. An explicit gram input records the original grams under `declared_grams_to_kg_v1` and the canonical kg quantity in the same entry. |
| Species | `SPECIES` / `SPECIES_NAMES` binds names, with multiple names as a quoted comma-separated list. An unnamed scalar can only resolve the sole configured species. An unnamed mass list must cover the complete run table in its configured order. Each output inventory carries the resolved #10 name and zero-based particle mass slot. |
| Particle request | `PARTICLE_COUNT` / `PARTICLES` / `NPART` is a positive logical count for the release. `SAMPLING_SEED` carries optional downstream seed intent. |

Missing physical fields, unknown keys and duplicate or conflicting aliases
fail closed. Each quantity must be finite and nonnegative, with at least one
positive quantity per release. The canonical activity variant carries Bq and a
required UTC reference instant. Text RELEASES accepts mass inventories. The
forward driver rejects activity before particle initialization.

File provenance records the original UTF-8 input SHA-256, supplied path,
record ordinal and versioned adapter identity. The #51 adapter uses the
validated case ID. Explicit canonical callers provide a stable source ID.

## Pinned FLEXPART 11.1 crosswalk

[Reference manifest](../reference/flexpart-11.1.json), revision
`c70586c2b7f5258850705325881c61f557ea9bd8`.
`FLEXPART.F90:431-437` calls `readreleases` after COMMAND parsing. The native
adapter accepts one leading `&RELEASES_CTRL` followed by `&RELEASE` records.

| Canonical field | Pinned Fortran counterpart | Treatment |
| --- | --- | --- |
| UTC window and profile | `readoptions_mod.f90:2152-2155,2434-2438,2482-2536`; `IDATE1/ITIME1` and `IDATE2/ITIME2` feed `ireleasestart` and `ireleaseend` | Preserve declared endpoints and normalize to UTC. Run-window clipping, short-window collapse and species-dependent temporal variation are deferred. |
| Horizontal geometry | `readoptions_mod.f90:2155-2156,2439-2442`; `LON1/LON2` and `LAT1/LAT2` become `xpoint1/2` and `ypoint1/2` | Direct geographic bounds, with a point for coincident bounds. Grid mapping and random draws in `initialise_mod.f90:222-238` are downstream. |
| Heights and datum | `readoptions_mod.f90:2100-2102,2157-2158,2443-2445`; `Z1/Z2/ZKIND` become `zpoint1/2` and `kindz` | Direct metres for `ZKIND=1` (AGL) and `2` (ASL). Omitting the datum or selecting pressure coordinates fails. `initialise_mod.f90:315-413` terrain conversion is downstream. |
| Species and lane | `readoptions_mod.f90:2149-2150,2179,2238-2247,2300`; `NSPEC`, `SPECNUM_REL`, `readspecies(specnum_rel(i),i)` | Resolve each number to a configured `SPECIES_NNN` and its #10 identity independently of header order. Maximum four species. |
| Released quantity | `readoptions_mod.f90:685-692,2159,2446-2448`; `MASS(i)` enters `xmass(numpoint,i)`. See `initialise_mod.f90:240-251` and the particle mass unit in `netcdf_output_mod.f90:1661` | Direct kg under `LDIRECT=1, IND_SOURCE=1`. Aggregate loading requires explicit native forward direction and honors the pinned mass-mode default for omitted `IND_SOURCE`. Mass mixing ratios are unsupported. |
| Particle count | `readoptions_mod.f90:2160,2449,2460-2467`; `PARTS` becomes `npart(numpoint)` | Direct positive logical count. Allocation, rounding and injection are downstream. |
| Release ID | `readoptions_mod.f90:2161,2450`; `COMMENT` becomes `compoint` | A supplied comment becomes the ID. An absent comment uses the record ordinal. |
| Release seed, provenance and schema | No corresponding namelist fields | Project-owned contract fields. |

The native kg interpretation follows the pinned source and the
[FLEXPART input/output unit table](https://acp.copernicus.org/articles/5/2461/2005/),
Table 1. The existing corpus and ETEX fixture tooling uses a conflicting gram
convention. A native file supplies its literal quantity in kg at this
boundary. The #51 candidate input uses its explicit manifest quantity in kg.
The oracle fixture and input-equivalence correction is tracked in [#259](https://github.com/Grodahn/flexpart-gpu/issues/259).

## Runtime handoff

- #169 receives the release ID, full interval-integrated inventory per
  validated species and lane, and the total requested logical count. It owns
  partitioning, rounding, precision and allocation evidence.
- #27 consumes that allocation with canonical geometry, datum, UTC profile
  and seed intent. It owns deterministic sampling, injection and buffer
  publication. #28 owns terrain-dependent behavior and paired release parity.
- The existing scheduler consumes explicit normalized mass rows. It accepts
  AGL and mass, with its current unseeded sampling. The spatial bounds must
  fit the release grid and the time interval must fit the run and end on a
  scheduler tick. Its particle time and mass representations constrain the
  usable subset of the canonical document.
- Older programmatic diagnostic and backward receptor entry points retain a
  transitional type. The production forward constructor takes `ReleaseSpec`.
