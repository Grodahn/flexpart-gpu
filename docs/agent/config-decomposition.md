# Configuration decomposition (#129)

## Pre-move responsibility and field inventory

Recorded before moving production code at `f4fe2d22e1158ca4b32bfc95323c00d6626a5a99`.
`src/config/mod.rs` contains 2,361 lines / 88,095 checkout bytes (85,734 bytes
with LF line endings). Its public facade exports `SimulationConfig`,
`CommandConfig`, `ReleaseConfig`, `OutputGridConfig`, `SpeciesConfig`,
`SpeciesKind::{Gas,Aerosol}`, and `ConfigError`.
All five structs derive the same Serde traits and `PartialEq`; their field
declaration order determines compact/pretty JSON order. Every field below is
public and serialized under its Rust name. There are no rename/skip attributes.
Only the two `version` fields have explicit `#[serde(default)]`; absent
`Option` fields deserialize to `None`. Other fields remain required. Direct
Serde deserialization does not run `validate()`. No type implements `Default`.

| Responsibility | Externally visible fields | Existing file-parser defaults |
| --- | --- | --- |
| Aggregate loading and cross-domain validation | `SimulationConfig`: `version`, `command`, `releases`, `outgrid`, `species` | `load()` sets `version=None`; loads COMMAND, RELEASES, OUTGRID, SPECIES in that order |
| COMMAND run timing | `start_time`, `end_time`, `output_interval_seconds`, `sync_interval_seconds`, `raw` | Intervals absent: `None`; missing IBTIME/IETIME with date: `000000` |
| RELEASES geometry/inventory | `name`, `start_time`, `end_time`, `lon`, `lat`, `z_min`, `z_max`, `mass_kg`, `particle_count`, `species_masses_kg`, `raw` | `release_{ordinal}` (1-based), `z_min=0.0`, `z_max=z_min`, `mass_kg=1.0`, `particle_count=1`, species masses absent: `None` |
| OUTGRID bounds/dimensions/resolution | `lon_min`, `lon_max`, `lat_min`, `lat_max`, `nx`, `ny`, `nz`, `dx`, `dy`, `dz`, `raw` | `dx=(lon_max-lon_min)/nx`, `dy=(lat_max-lat_min)/ny`; other fields required |
| SPECIES identity, units and process activation | `name`, `version`, `molecular_weight`, `dry_deposition_velocity`, `decay_constant`, `half_life_s`, `wet_a_gas`, `wet_b_gas`, `crain_aero`, `csnow_aero`, `ccn_aero`, `in_aero`, `relative_diffusivity`, `henry`, `surface_reactivity_f0`, `particle_density_kg_m3`, `mean_diameter_um`, `diameter_sigma`, `source_file`, `raw` | Name falls back to nonempty file stem; `source_file` is filename; version/numeric values absent: `None`. Sentinel rules below apply |
| Shared grammar, numeric conversion, timestamps, version policy | Private `ConfigMap=BTreeMap<String,String>`, helpers; public errors | Keys lowercased; last duplicate wins; first alias in each list wins; unknown keys retained in `raw`. Version absent or 1 accepted; other versions rejected |

### File key aliases and precedence

Lists preserve first-present precedence, including sentinel-valued keys.

- COMMAND start: `start_time/start/ibdatetime`, then `ibdate` + `ibtime`;
  end: `end_time/end/iedatetime`, then `iedate` + `ietime`.
  Output interval: `loutstep/outstep/output_interval_seconds`;
  sync: `lsynctime/sync_interval_seconds`.
- RELEASES name: `name/release_name/id`; start: `start_time/start`;
  end: `end_time/end`; lon: `lon/longitude`; lat: `lat/latitude`;
  lower height: `z_min/z1/z_bottom`; upper: `z_max/z2/z_top`;
  mass: `mass_kg/mass/xmass`; count: `particle_count/particles/npart`;
  species masses: `mass_species/species_mass/species_masses`.
- OUTGRID lon lower: `lon_min/xlon0/xmin`; upper: `lon_max/xlon1/xmax`;
  lat lower: `lat_min/ylat0/ymin`; upper: `lat_max/ylat1/ymax`;
  dimensions: `nx`, `ny`, `nz`; spacing: `dx/xres`, `dy/yres`, `dz/zres`.
- SPECIES version: `version/species_version`; name:
  `name/species/specname/pspecies`; molecular weight:
  `molecular_weight/mol_weight/weightmolar/pweightmolar`;
  wet coefficients: `pweta_gas/weta_gas`, `pwetb_gas/wetb_gas`,
  `pcrain_aero/crain_aero`, `pcsnow_aero/csnow_aero`,
  `pccn_aero/ccn_aero`, `pin_aero/in_aero`;
  diffusivity: `preldiff/reldiff`; Henry: `phenry/henry`;
  reactivity: `pf0/f0`; density: `pdensity/density`;
  width: `pdsigma/dsigma`; shape: `pshape/shape`.
  `pdryvel` takes precedence over `dry_deposition_velocity/vdep`;
  `pdecay` over `decay_constant/decay`; `pdia/pdquer` over
  `mean_diameter_um/diameter_um/dquer_um`.

### Existing validation and fail-closed boundaries

- COMMAND requires a namelist; uses its first section. Timestamps strip
  nondigits, pad 8/10/12 digits to 14, and check year/month/day/hour/minute/second
  ranges. Run start must not follow end; specified intervals must be positive.
- RELEASES accepts repeated RELEASE namelists or one key/value record per
  nonempty line. Requires an entry, chronology, geographic ranges, ordered
  heights, positive scalar mass/count. Species mass lists require 1..=4 finite
  nonnegative entries and positive total; quotes protect comma-separated lists.
- OUTGRID accepts its first namelist or plain assignments. Requires ordered
  geographic bounds, positive dimensions/resolutions. Preserve the existing
  comparisons exactly; this decomposition adds no numeric policy.
- SPECIES reads sorted regular files. Missing/non-directory/empty paths and
  more than four species fail. First SPECIES_PARAMS section takes precedence
  over SPECIES, then plain assignments. Positive-only parameters map missing,
  nonpositive and nonfinite values to `None`; PF0 preserves finite zero.
  PDRYVEL maps finite nonpositive values to `None`, converts positive cm/s by
  `0.01`, and rejects nonfinite values; legacy m/s accepts finite zero.
  PDECAY maps finite nonpositive values to disabled, rejects nonfinite/overflow,
  and converts positive half-life using exactly `0.693_147/value`; legacy
  decay constants accept finite zero. Positive PDIA/PDQUER converts metres by
  `1_000_000.0`; missing/nonpositive/nonfinite values disable diameter.
  Missing shape is accepted; supplied shape must equal zero.
  Validation preserves positivity/finiteness rules, gas/particle exclusion,
  gas wet-removal Henry requirement, density/diameter requirement and aerosol
  width >1. Process predicates derive from those fields; no independent switches.
- Aggregate validation order is version, COMMAND, OUTGRID, maximum species,
  nonempty releases, each release, each species, then release mass cardinality
  against configured species. Shorter mass lists and absent lists remain accepted.
- `ConfigError` preserves `MissingPath`, `ReadFile` (with IO source), `Parse`,
  `MissingKey`, `InvalidValue`, `Validation`, their fields and display text.
  Shared grammar preserves quotes, comments (`!`/`#` outside quotes), namelist
  terminators, numeric parse errors and raw assignment retention.

### Concrete callers and domain boundaries

Existing imports use `crate::config` / `flexpart_gpu::config`: release injection,
forward/backward time loops, validation loading, species mapping, settling,
candidate binaries, integration tests and the advection benchmark. Public fields
are constructed directly by those callers and remain public.
Meteorology provider configuration and GPU/runtime settings do not live in this
module. Their opaque COMMAND keys remain in `raw`; no new typed settings or
domain modules are introduced for them.

## Final layout and representative context

The canonical caller path is still `config::{SimulationConfig, CommandConfig,
ReleaseConfig, OutputGridConfig, SpeciesConfig, SpeciesKind, ConfigError}`.
The facade contains only private module declarations and these re-exports.
No callers need changed imports or compatibility wrappers.

| Private module | Ownership | LF source bytes / lines (including colocated tests) |
| --- | --- | --- |
| `mod.rs` | Stable facade | 1,016 / 29 |
| `command.rs` | COMMAND run timing | 4,127 / 124 |
| `release.rs` | RELEASES geometry/inventory | 11,104 / 311 |
| `output.rs` | OUTGRID bounds/dimensions/spacing | 5,644 / 163 |
| `species.rs` | SPECIES aliases, unit/sentinel conversions, validation, process predicates | 35,307 / 894 |
| `simulation.rs` | Aggregate loading, cross-domain validation/order | 19,998 / 543 |
| `parsing.rs` | Shared grammar, scalar conversion, timestamps and version policy | 11,760 / 393 |
| `error.rs` | Public error variants and display/source behavior | 961 / 30 |
| `test_support.rs` | Test-only temporary directory helper | 544 / 18 |

Modules are private. Public types, fields and methods retain their visibility
because concrete callers use them. Shared parser functions, map alias and version
constant have `pub(super)` visibility only; quote trimming and timestamp
normalization stay private. Domain-specific species conversions and all `from_map`
helpers stay private to their owner. Production imports are explicit and limited
to each domain's dependencies. Domain tests can access their owner's private
helpers without widening the public API; shared temporary-file support is test-only.

Representative task: inspect inferred OUTGRID horizontal spacing and zero-dimension
rejection. Before: the entire config module, 2,361 lines / 85,734 LF UTF-8 bytes.
After: stable facade + OUTGRID owner with colocated tests, 192 lines / 6,660 bytes.
That is 91.9% fewer lines and 92.2% fewer source bytes. Both measurements include
tests and use LF consistently; the original Windows checkout has 88,095 bytes
with CRLF. Shared parsing/error files are named adjacent handoffs only when the
task changes grammar/conversion/failure representation. This is a source-context
measurement, not a token count or timing benchmark.

## Preservation evidence

The [public-facade tests](../../tests/config_contract.rs) captured the
[144-entry transcript](../../tests/fixtures/config-behavior-v1.json) against the
pre-move code, before decomposing it. It freezes compact/pretty serialized bytes,
accepted/rejected parser inputs, defaults, alias precedence, timestamps,
sentinels/conversions, process activation predicates, aggregate validation order
and round trips. Temporary species paths and checkout CRLF are normalized for
Windows/Linux portability; all other transcript content is compared exactly.
The final test has no baseline-update mode.

All 26 original tests passed before movement and remain with their domain owners
after movement. A token audit of all 40 production items confirms identical
attributes, fields, signatures, literals and bodies after excluding imports,
documentation, parser sibling visibility and rustfmt trailing commas. It also
confirms the original test-name set. Logs are retained under
`target/issue-129/`; the audit is supplemental review evidence, not a replacement
for the executable regression and broader gates.

No scientific calculation, configuration schema, default, validation branch,
alias or runtime caller changes. There are no deviations to record in the
scientific changelog and no newly resolved configuration semantics or follow-ups.
