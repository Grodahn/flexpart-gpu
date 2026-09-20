# Corpus case v1 -> v2 migration notes (Issue #51)

All ten checked-in case documents are canonical `schema_version` 2.
The legacy v1 shape (`version`, `seeds`, uppercase overrides) is frozen and
unsupported: parsers reject v1 fail-closed, and no checked-in case uses it.
No scientific value changed in migration; only representation.

## Version handling

| v1 | v2 | Note |
|----|----|------|
| `version: 1` | `schema_version: 2` | Renamed. Documents carrying `version`, an unknown `schema_version`, no version key, or both keys are rejected with a field-specific version error. |

## Stochastic identity (separate RNG namespaces, Issue #50)

| v1 | v2 | Note |
|----|----|------|
| `seeds.base_philox_key` | `stochastic.candidate_philox.base_key` | Renamed, values preserved. |
| `seeds.base_counter` | `stochastic.candidate_philox.base_counter` | Renamed, values preserved. Now flows into the driver config and seed artifacts (was hard-coded zero). |
| `seeds.count` | `stochastic.candidate_philox.count` | Renamed, values preserved. Authoritative ensemble size; `--seeds` may only select a leading subset. |
| `seeds.repeats` (REPEAT-009 only) | `stochastic.candidate_philox.count = 2` + `derivation: reuse_base_identity_v1` | Split: the repeat count moves to `count`; exact key/counter reuse is an executable, versioned derivation policy instead of a side flag or hard-coded case ID. |
| `seeds.derivation` | `stochastic.candidate_philox.derivation` | Free-form prose is replaced by the closed versioned enum `wrapping_add_key0_v1` or `reuse_base_identity_v1`. The runner and audit execute this field directly; unknown values fail closed. |
| `seeds.requirement` (REPEAT-009) | `notes` | Moved to notes ("bit-identical particle states across repeats"). |
| `seeds` as a string (ADV-ANA-001 `"deterministic (no RNG consumed)"`) | `stochastic: {candidate_philox: null, oracle_seed: null}` | Removed free-form marker; deterministic cases declare no identity and validate without one. |
| - | `stochastic.oracle_seed` | Typed oracle stochastic namespace. `pristine-oracle` is seedless and carries no seedable strategy. `seedable-validation-oracle` references the completed #50 contract by stable strategy id/version/path and may carry a requested identity or seed=null for the #50 default-equivalent mode. Candidate Philox values are never interpreted as oracle identities. |
| `fortran_repeatability_note` (REPEAT-009) | `notes` | Moved verbatim. |

### Candidate Philox derivation semantics

`candidate_philox.derivation` is executable contract data, not a descriptive
string. `wrapping_add_key0_v1` derives seed `i` as
`[base_key[0].wrapping_add(i), base_key[1]]` with the declared base counter.
`reuse_base_identity_v1` reuses the exact declared base key and counter for
every repetition. The former `identical_repeats` side flag is forbidden.
Rust deserialization, the raw-Python generator, the candidate-output audit,
and the JSON Schema all reject unknown derivation values fail-closed.
## Chronology and meteorology coverage

Schema v2 timestamps use `YYYYMMDDHHMMSS`, but format alone is not treated as
valid time. Rust and the raw-Python generation path now parse real Gregorian
calendar dates (including leap-year/day validation) and fail closed on invalid
months, days, hours, minutes, or seconds.

`integration.start`, `dt_s`, `steps`, and `total_s` are validated together.
`total_s` must equal `dt_s * steps`; the simulation end is derived from the
validated start plus `total_s` rather than inferred from string ordering or a
hard-coded calendar date. The raw-Python COMMAND generator now derives both
`IBDATE`/`IBTIME` and `IEDATE`/`IETIME` from that integration block.

Release instants/windows must lie completely inside the simulation interval.
For real-weather cases, `meteorology.temporal_coverage` must be ordered, contain
valid Gregorian timestamps, and cover the entire simulation interval. Coverage
that starts even one second late or ends one second early is rejected.
## Species-dependent physics contracts

`release.species.id` is no longer sufficient by itself. Every schema-v2 case
pins a `release.species.physics_contract` containing a closed profile, contract
id/version/path, and the content-addressed Git blob SHA of the referenced
contract under `reference/species-physics/`.

The current closed profiles are:

- `species_024_inert_v1`: pinned upstream tracer; dry deposition, wet
  deposition, and decay are all disabled.
- `species_040_dry_constant_v1`: DRY-007; derived from the pinned tracer by
  setting `PDRYVEL=2.0` cm/s, matching the candidate 0.02 m/s dry forcing.
- `species_040_wet_aerosol_v1`: WET-008; derived from the pinned aerosol by
  removing the FLEXPART-11.1-incompatible `PNDIA` key. The oracle aerosol also
  carries dry-deposition parameters; that candidate/oracle asymmetry is
  explicit in the contract and case representation notes.

Each contract records the pinned upstream SPECIES path and SHA-256 plus the
checked-in derived fixture path/blob identity. Rust and raw-Python validation
require the canonical reference exactly and require the case physics switches
to match the selected candidate profile. Unknown profiles, mismatched hashes,
or decay without a dedicated profile fail closed.

Active dry/wet deposition now also requires an explicit `deposition` block;
there is no longer an escape hatch where forcing can be supplied implicitly by
the execution pipeline. ETEX-MINI-013 was corrected to the physics actually
used by `scripts/run-etex.sh`: pinned inert `SPECIES_024`, with dry/wet removal
disabled on both paths.
## Oracle command overrides (generated namelist stays uppercase)

| v1 | v2 | Note |
|----|----|------|
| `oracle_command_overrides.LTURBULENCE` | `oracle_command_overrides.lturbulence` | Renamed, values preserved. Required; flags must be exactly 0/1. |
| `oracle_command_overrides.LCONVECTION` | `oracle_command_overrides.lconvection` | Renamed, values preserved. Required; flags must be exactly 0/1. |
| `oracle_command_overrides.CTL` | `oracle_command_overrides.ctl` | Renamed, values preserved. Required; must be consistent with the declared `turbulence_formulation` (see the Issue #67 section below). |
| `oracle_command_overrides.IFINE` | `oracle_command_overrides.ifine` | Renamed, values preserved. Required; must be in 1..=10 (`readoptions_mod.f90:624` silently clamps `IFINE` to `>= 1`). |
| - | `oracle_command_overrides.ldrydep/lwetdep/ldecay` | Introduced as optional flags (exactly 0/1 when present). Unset in migrated cases; deposition is selected via `physics_switches` + SPECIES. |
| (absent in DRY-007/WET-008) | explicit `lturbulence/ctl/ifine/lconvection` | Introduced with the values the generator previously defaulted to (`1/5.0/4/0`), matching the synthetic fixtures. ETEX keeps its separate historical `CTL=-5.0 / IFINE=4` COMMAND values. |
| (none) | `oracle_command_overrides.turbulence_formulation` | Introduced (Issue #67). Required typed enum making the actual FLEXPART mode machine-readable; synthetic cases use `adaptive_w_sigw`, ETEX-MINI-013 preserves `fixed_sync_w`. |
| workflow/COMMAND value | `oracle_command_overrides.lsynctime_s` | Required explicit FLEXPART `LSYNCTIME` [s]. Synthetic cases use `300`; ETEX-MINI-013 preserves its real `900`. No case may inherit a global sync default. |

## Turbulence/integration formulation (Issue #67)

Schema v2 represents the two FLEXPART turbulence/time-step formulations that
are actually present in the checked-in validation cases. The scientific choice
is explicit in `oracle_command_overrides.turbulence_formulation`; it is never
inferred from a case ID or from `CTL` alone.

Rationale from the pinned oracle (`reference/flexpart-11.1.json`, commit
`c70586c2b7f5258850705325881c61f557ea9bd8`):

- FLEXPART derives the dispersion method from the sign of `CTL`
  (`readoptions_mod.f90:786-795`): `CTL > 0` selects the adaptive method
  (`method=1`, `mintime=minstep=1`); `CTL <= 0` selects the fixed-timestep
  method (`method=0`, `mintime=lsynctime`).
- FLEXPART derives the Markov-chain formulation from the magnitude of `CTL`
  (`readoptions_mod.f90:626,645-650`): `CTL >= 0.1` selects w/sigw;
  `CTL < 0.1` selects w and forces effective `ifine=1`.

The closed v2 modes are:

- `adaptive_w_sigw`: synthetic corpus cases, `CTL=5.0`, raw `IFINE=4`,
  explicit `LSYNCTIME=300`.
- `fixed_sync_w`: ETEX-MINI-013, preserving the real ETEX COMMAND:
  `CTL=-5.0`, raw `IFINE=4`, explicit `LSYNCTIME=900`. FLEXPART therefore
  runs method=0 with `mintime=lsynctime`, selects the w formulation, and
  forces effective `ifine=1` internally. The manifest intentionally records
  both the raw COMMAND IFINE and the typed effective formulation.

Validation rejects a CTL/formulation mismatch in either direction, rejects
`CTL=0`, and still rejects the unsupported positive sub-threshold combination
`0 < CTL < 0.1`. `IFINE=0` is rejected because FLEXPART would silently
clamp it to 1. `lsynctime_s` is required explicitly for every case.

## Deposition forcing

| v1 | v2 | Note |
|----|----|------|
| `deposition.dry_deposition_velocity_m_s` | `deposition.dry_deposition_velocity_m_s` | Preserved key-for-key (DRY-007 `0.02`, WET-008 `0.0`). |
| `deposition.dry_reference_height_m` | `deposition.dry_reference_height_m` | Preserved (DRY-007 `15.0`). Required when dry deposition is on. |
| `deposition.wet_scavenging_coefficient_s_inv` | `deposition.wet_scavenging_coefficient_s_inv` | Preserved (DRY `0.0`, WET `0.005`). |
| `deposition.wet_precipitating_fraction` | `deposition.wet_precipitating_fraction` | Preserved (DRY `0.0`, WET `1.0`). |
| `deposition.isolated_check` | `notes` | Moved: analytic kernel-check description preserved in notes; not a driver input. |
| - | `deposition: null` (absent) | Introduced convention: cases with deposition off carry no block. ETEX-MINI-013 now explicitly declares inert/no-removal species physics and therefore also carries no deposition block. |

## Free-text expectations and provenance notes

| v1 | v2 | Note |
|----|----|------|
| `expected_property` (PBL cases) | `representation_differences.notes` | Moved verbatim. |
| `wind.note` (WIND-SHEAR-003 shear formula) | `notes` | Moved verbatim. |
| `oracle_meteo_note` (WIND-SHEAR-003) | `notes` | Moved verbatim. |
| `oracle_species_note` (DRY-007/WET-008) | `notes` + `representation_differences.notes` | Moved verbatim (species provenance stays in notes; comparison role summarized in representation differences). |
| `based_on` (REPEAT-009) | `notes` | Moved ("based_on PBL-NEUTRAL-005" plus the shared-configuration statement). |

## Closed contract (no silent field loss)

Every v2 object uses `#[serde(deny_unknown_fields)]`: typos, stray
extension keys, legacy `seeds`/`seed` fields, and uppercase override keys
fail with an error naming the offending field. Deliberate extensions belong
in `notes` or a versioned schema revision, never in ad-hoc keys.
`write_to_file` validates before serializing so invalid in-memory values
can never be written as apparently valid documents.

## Units

V1 `units` blocks were partial per case. V2 now validates units required by
the active semantics instead of checking only `units.wind`. Every case
declares canonical model units for wind, height, mass, time and concentration;
surface cases additionally require pressure, temperature, heat-flux and
inverse-Obukhov units, while shear/deposition/scavenging units are required
when those semantics are active. ETEX observation values may be pg/m3, but
the model-output quantity in the case contract is canonical kg/m3; observation
conversion remains evaluation-layer responsibility. ADV-ANA-001 keeps its
explicit displacement unit.

## Coordinate semantics and source normalization

v1 used loosely typed `lon_deg`/`lat_deg`/`z_m` without vertical reference
or horizontal convention metadata, and `seeds` with free-form derivation.
v2 introduces typed enums and structures so runners never guess:

| v1 | v2 | Note |
|----|----|------|
| (none) | `domain.horizontal_ref` | Required: `GeographicLonLatDegrees`. |
| (none) | `domain.wind_heights_ref` | Required: `agl` (all checked-in cases). |
| (none) | `release.vertical_ref` | Required: `agl` (all checked-in cases). |
| `release.lon_deg`/`lat_deg`/`z_m` | `release.geometry` (Point/Box) | Typed; `kind` disambiguates. |
| (none) | `release.geometry.vertical_ref` | AGL (checked-in cases). |
| `release` (implicit instant) | `release.timing` | Explicit: `Instant { at: "YYYYMMDDHHMMSS" }` equals integration start. |
| (none) | `release.species` | Required: `SPECIES_<NNN>` mapping to FLEXPART `SPECNUM_REL`. |
| `release.mass_kg_total` | `release.inventory` | Typed: `quantity_kg` + `unit` (`kg`). |
| `release.mass_kg_per_particle` | `release.mass_kg_per_particle` | Kept; validated consistent with `quantity_kg / particle_count` within 1e-6. |
| (none) | `domain.wind_heights_ref` | Required: `agl` (all checked-in cases). |
| `domain`/`release` (implicit) | `require_source_containment` | Synthetic cases must lie inside domain; ETEX-MINI-013 waives it with note. |

All checked-in cases use `SourceGeometry::Point` with `VerticalRef::Agl`,
`ReleaseTiming::Instant` matching the integration start, and
`SpeciesRef { id: "SPECIES_024" }` (or `SPECIES_040` for DRY/WET).
Inventory uses `MassUnit::Kg`. The manifest `vertical_ref` maps to
FLEXPART `RELEASES` `ZKIND=1` (meters above ground, confirmed by the
ETEX input-equivalence audit). `HorizontalCoordRef::GeographicLonLatDegrees`
maps to FLEXPART geographic coordinates. The generator and audit reject
legacy fields (`ZKIND` still written as `1` in the Fortran namelist, but
the manifest never accepts `ZKIND` directly).
Preserved values: heat_flux/inv_obukhov/height (PBL), shear (SHEAR),
deposition_velocity/rate (DRY), scavenging/precipitating_fraction/mass
(WET). Introduced (non-scientific, standard SI): `wind: m/s`, `mass: kg`,
`time: s`, plus `height`, `pressure`, `temperature` where the case uses
those quantities. Removed without replacement: `reference_height`,
`rate`, `precipitating_fraction`, `positions` (dimensionless or covered by
`height`/`mass`; values live in the blocks they describe).

## Integration, release, domain, wind, surface, switches

Unchanged shapes and values; only representation notes:
- `integration.dt_s/total_s`: JSON integers (`300`, `3600`) render as floats (`300.0`, `3600.0`). Same values.
- `release`: added derived `mass_kg_per_particle` (total/count) for readability; `mass_kg_total` unchanged and authoritative.
- `domain`, `wind` (uniform/shear parameters), `surface`, `physics_switches`: values preserved exactly. REPEAT-009 gains the neutral `surface` and neutral `physics_switches` (turbulence on) matching its documented `based_on` configuration; v1 carried neither because it referenced the neutral case by name.

## Introduced runner metadata (non-scientific)

`execution_profile` (frozen `flexpart-11.1-single-thread` v1 + manifest
path), `expected_artifacts.required` (stable artifact IDs plus producer/class, with no filesystem paths or hashes), and `representation_differences` are new in v2 and identical in kind to the previously migrated ADV-ANA-001/WIND-UNI-002/ETEX-MINI-013. Concrete artifact locations, hashes and immutable run attribution remain owned by #53.

## Simulation direction and output semantics

The FLEXPART COMMAND keys `LDIRECT` / `LOUTSTEP` / `LOUTAVER` /
`LOUTSAMPLE` were previously written by the corpus tooling from hard-coded
constants. They are now required, explicitly declared manifest fields
(Issue #51 / #57). The raw numeric namelist keys stay on the generated
Fortran; the manifest carries the canonical semantic values and the
generator/audit derive the namelist from them. No generator-side default
substitutes for a missing block.

| v1 | v2 | Note |
|----|----|------|
| (implicit forward) | `simulation_direction` | Required typed enum: `forward` (`LDIRECT=1`) / `backward` (`LDIRECT=-1`). Schema v2 validates only `forward`: FLEXPART backward runs are valid but produce source-receptor/residence-time semantics for IOUT=1, which are not yet represented by the current concentration-only `OutputQuantity`. |
| (generator constant `1800`) | `output.interval_s` | Output interval [s], FLEXPART `LOUTSTEP`. Synthetic cases `1800`; ETEX-MINI-013 `10800` (its real COMMAND). |
| (generator constant `1800`) | `output.averaging_window_s` | Averaging window [s], FLEXPART `LOUTAVER`. Synthetic cases `1800`; ETEX-MINI-013 `10800`. |
| (generator constant `300`) | `output.sampling_interval_s` | Sampling interval [s], FLEXPART `LOUTSAMPLE`. Synthetic cases `300`; ETEX-MINI-013 `900`. |
| (none) | `output.quantity` | Scientific quantity of each output field; this schema revision supports exactly `time_averaged_mass_concentration_kg_m3`. |

Validated fail-closed in both the Rust validator and the Python generator:
every timing value is whole positive seconds; `sampling_interval_s <=
averaging_window_s <= interval_s`; each case declares
`oracle_command_overrides.lsynctime_s` explicitly. Synthetic cases
use 300 s; ETEX-MINI-013 preserves 900 s. All three output timings must be
multiples of that case-specific value, while averaging/output intervals must
each be at least `2*LSYNCTIME`, matching
the pinned FLEXPART `readoptions_mod.f90` checks. The Python raw path accepts
only the same closed output quantity as the Rust enum. Backward simulations
are rejected until their source-receptor output semantics and units are
modeled explicitly. A missing `simulation_direction` or `output` block (or
any sub-field) is rejected with a field-specific error.
Synthetic generated COMMAND values remain `LDIRECT=1`, `LOUTSTEP=1800`,
`LOUTAVER=1800`, `LOUTSAMPLE=300`, `LSYNCTIME=300`. ETEX-MINI-013 instead
preserves its existing real COMMAND timing (`LOUTSTEP/LOUTAVER=10800`,
`LOUTSAMPLE/LSYNCTIME=900`) and fixed `CTL=-5` formulation.

## Machine-readable schema contract

Schema v2 has a checked-in JSON Schema Draft 2020-12 contract at
`schemas/validation-case-v2.schema.json`. It is the versioned structural
contract for external tooling: required fields, closed objects, enums, tagged
unions, artifact classes, stochastic namespaces, and schema version are
machine-readable without compiling Rust.

Rust remains authoritative for cross-field scientific invariants that JSON
Schema does not encode here (for example timestep arithmetic, coupled physics
switches, FLEXPART execution semantics, and stochastic-strategy consistency).
Regression tests validate every checked-in case against both the JSON Schema
and `ValidationCaseManifest::parse`, and include fail-closed negative checks
for unsupported versions, missing required output semantics, and unknown
top-level fields.
## Input-equivalence ownership and error semantics

Schema v2 deliberately carries **no input-equivalence verdict**. A case may declare structured representation differences and `known_input_equivalence_limitations`, but only #52 may emit the authoritative `DEMONSTRATED` / `NOT_DEMONSTRATED` gate result. Legacy/ad-hoc `input_equivalence` fields are rejected by the closed contract. ETEX-MINI-013 preserves its existing `INPUT_EQUIVALENCE_NOT_DEMONSTRATED` limitation as declarative context without turning schema validation into an equivalence decision.

`expected_artifacts` now declares stable artifact IDs, producers and semantic classes (`raw_model_output`, `decoded_model_output`, `comparison_report`, `run_manifest`) rather than target paths. #53 owns concrete paths, hashes and immutable run attribution.

Manifest file writes report a distinct `WriteFile` error instead of being misclassified as read failures. Unit-value disagreements use the dedicated `UnitMismatch` validation error; missing unit fields still report `MissingField`.

## External metric and threshold references

Schema v2 carries `validation_definition_refs` instead of copying metric
formulas or numeric gates into cases. Checked-in cases reference
`scripts/evaluate/metrics.py` (report-schema 1.0.0),
`evaluation/thresholds/scientific-thresholds-v1.json`, and
`fixtures/corpus/thresholds.json`. Later #43/#55 contracts can replace or
extend these stable references without embedding threshold values in #51.

## v1 support statement

V1 is not supported and has no sunset period: all checked-in documents are
v2, and every parser (Rust `ValidationCaseManifest::parse`, the Fortran
fixture generator/auditor, the evaluator) rejects v1 fail-closed. The
previous scattered v1 adapters are removed, not deprecated.
