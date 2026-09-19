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
| `oracle_command_overrides.CTL` | `oracle_command_overrides.ctl` | Renamed, values preserved. Required; must be finite. |
| `oracle_command_overrides.IFINE` | `oracle_command_overrides.ifine` | Renamed, values preserved. Required; must be in 1..=10. |
| - | `oracle_command_overrides.ldrydep/lwetdep/ldecay` | Introduced as optional flags (exactly 0/1 when present). Unset in migrated cases; deposition is selected via `physics_switches` + SPECIES. |
| (absent in DRY-007/WET-008) | explicit `lturbulence/ctl/ifine/lconvection` | Introduced with the values the generator previously defaulted to (`1/5.0/4/0`), matching the checked-in fixtures byte-for-byte. No scientific change. |

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

## Units

V1 `units` blocks were partial per case; v2 carries the full explicit set.
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
