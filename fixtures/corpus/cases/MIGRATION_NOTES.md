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
| `seeds.repeats` (REPEAT-009 only) | `stochastic.candidate_philox.count = 2` + `identical_repeats: true` | Split: the repeat count moves to `count`; the identical-reuse semantics move to the new explicit `identical_repeats` flag (default `false`) instead of hard-coded case IDs in tooling. |
| `seeds.derivation` | `stochastic.candidate_philox.derivation` | Preserved verbatim (including the PBL-NEUTRAL-005 CI seed-0 suffix). REPEAT-009 documents identical reuse. |
| `seeds.requirement` (REPEAT-009) | `notes` | Moved to notes ("bit-identical particle states across repeats"). |
| `seeds` as a string (ADV-ANA-001 `"deterministic (no RNG consumed)"`) | `stochastic: {candidate_philox: null, oracle_seed: null}` | Removed free-form marker; deterministic cases declare no identity and validate without one. |
| - | `stochastic.oracle_seed` | Introduced (non-scientific runner metadata): `seedable-validation-oracle`, seed 1, 5 repetitions for cases with an oracle counterpart; `null` for REPEAT-009, which has no oracle counterpart. |
| `fortran_repeatability_note` (REPEAT-009) | `notes` | Moved verbatim. |

## Oracle command overrides (generated namelist stays uppercase)

| v1 | v2 | Note |
|----|----|------|
| `oracle_command_overrides.LTURBULENCE` | `oracle_command_overrides.lturbulence` | Renamed, values preserved. Required; flags must be exactly 0/1. |
| `oracle_command_overrides.LCONVECTION` | `oracle_command_overrides.lconvection` | Renamed, values preserved. Required; flags must be exactly 0/1. |
| `oracle_command_overrides.CTL` | `oracle_command_overrides.ctl` | Renamed, values preserved. Required; must be consistent with the declared `turbulence_formulation` (see the Issue #67 section below). |
| `oracle_command_overrides.IFINE` | `oracle_command_overrides.ifine` | Renamed, values preserved. Required; must be in 1..=10 (`readoptions_mod.f90:624` silently clamps `IFINE` to `>= 1`). |
| - | `oracle_command_overrides.ldrydep/lwetdep/ldecay` | Introduced as optional flags (exactly 0/1 when present). Unset in migrated cases; deposition is selected via `physics_switches` + SPECIES. |
| (absent in DRY-007/WET-008) | explicit `lturbulence/ctl/ifine/lconvection` | Introduced with the values the generator previously defaulted to (`1/5.0/4/0`), matching the checked-in fixtures byte-for-byte. No scientific change. |
| (none) | `oracle_command_overrides.turbulence_formulation` | Introduced (Issue #67). Required typed enum making the frozen FLEXPART mode machine-readable; see below. |

## Turbulence/integration formulation (Issue #67)

The corpus freezes exactly one FLEXPART turbulence/time-step formulation,
declared explicitly by the required

`oracle_command_overrides.turbulence_formulation` field (= `adaptive_w_sigw`).
Scientific choice is never inferred from a numeric `CTL` value alone.

Rationale from the pinned oracle (`reference/flexpart-11.1.json`, commit
`c70586c2b7f5258850705325881c61f557ea9bd8`):

- FLEXPART derives the dispersion method from the sign of `CTL`
  (`readoptions_mod.f90:786-795`): `CTL > 0` selects the adaptive method
  (`method=1`, `mintime=minstep=1`); `CTL <= 0` selects the fixed-timestep
  method (`method=0`, `mintime=lsynctime`). The adaptive method sizes the
  particle step from the local Lagrangian time scales
  (`advance_mod.f90:557-568`).
- FLEXPART derives the Markov-chain formulation from the magnitude of `CTL`
  (`readoptions_mod.f90:626,645-650`): `CTL >= 0.1` selects the w/sigw
  formulation (`turbswitch=.true.`); `CTL < 0.1` silently selects the w
  formulation and re-sets `ifine=1`.

Every checked-in case runs `CTL=5.0 / IFINE=4` (adaptive, w/sigw). The
contract states that choice explicitly and rejects, in both the Rust
validator and the Python generator (single documented threshold constant
`CTL_W_SIGW_FORMULATION_THRESHOLD` / `CTL_FORMULATION_THRESHOLD = 0.1`):

- missing or unknown `turbulence_formulation` values (typed enum; this schema
  revision supports exactly `adaptive_w_sigw`);
- `CTL = 0`: the oracle computes `ctl = 1./ctl` unconditionally
  (`readoptions_mod.f90:653`) and sizes particle steps from it, so this is a
  division by zero producing a divergent step;
- `CTL < 0`: the fixed-timestep method (`method=0`) is a valid FLEXPART mode
  — it is the oracle's own `CTL=-5.0` default — and is *deliberately
  unsupported* by this validation contract (the error names the mode and the
  reason instead of hiding behind `CTL >= 0.1`);
- `0 < CTL < 0.1`: silently re-interpreted as the w formulation with
  `ifine=1`, inconsistent with the declared formulation;
- `IFINE = 0`: silently clamped to 1 by `max(ifine,1)`
  (`readoptions_mod.f90:624`); the contract requires `1..=10`.

## Deposition forcing

| v1 | v2 | Note |
|----|----|------|
| `deposition.dry_deposition_velocity_m_s` | `deposition.dry_deposition_velocity_m_s` | Preserved key-for-key (DRY-007 `0.02`, WET-008 `0.0`). |
| `deposition.dry_reference_height_m` | `deposition.dry_reference_height_m` | Preserved (DRY-007 `15.0`). Required when dry deposition is on. |
| `deposition.wet_scavenging_coefficient_s_inv` | `deposition.wet_scavenging_coefficient_s_inv` | Preserved (DRY `0.0`, WET `0.005`). |
| `deposition.wet_precipitating_fraction` | `deposition.wet_precipitating_fraction` | Preserved (DRY `0.0`, WET `1.0`). |
| `deposition.isolated_check` | `notes` | Moved: analytic kernel-check description preserved in notes; not a driver input. |
| - | `deposition: null` (absent) | Introduced convention: cases with deposition off carry no block; ETEX-MINI-013 (switches on, forcing defined by its own pipeline) also carries no block. An absent block never means zero forcing. |

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

V1 `units` blocks were partial per case; v2 carries the full explicit set.
The ADV-ANA-001 `displacement: m` unit is preserved as the typed
`units.displacement` field (it was silently discarded before strict
parsing).

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
path), `expected_artifacts` (candidate/oracle dirs, comparison report, run
manifest; `oracle_dir` omitted for REPEAT-009, which has no oracle run),
`representation_differences`, and `input_equivalence: not_applicable`
(synthetic cases) are new in v2 and identical in kind to the previously
migrated ADV-ANA-001/WIND-UNI-002/ETEX-MINI-013.

## v1 support statement

V1 is not supported and has no sunset period: all checked-in documents are
v2, and every parser (Rust `ValidationCaseManifest::parse`, the Fortran
fixture generator/auditor, the evaluator) rejects v1 fail-closed. The
previous scattered v1 adapters are removed, not deprecated.
