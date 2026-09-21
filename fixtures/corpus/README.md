# Issue #6 test corpus (RISK-03.3G-01, point 2)

Small reproducible cases that exercise the productive WGSL candidate path
(`ForwardTimeLoopDriver` with WGSL advection, fused Hanna/Langevin turbulence,
PBL diagnostics and deposition) and the pinned unmodified FLEXPART 11.1 oracle.

Start from `fixtures/corpus/corpus.json` (versioned index) and
`docs/corpus-matrix.md` (requirement-to-case mapping). Run locally with
`scripts/run-corpus.sh`; CI runs only the small deterministic subset
(see `.github/workflows/software-wgpu.yml`).

## Layout

- `corpus.json`: versioned case index. Each entry carries its Issue #6
  requirement, fixture paths, runner, expected physical properties with units,
  fixed configuration, documented call, and status (`implemented` or `blocked`
  with a verifiable dependency).
- `cases/*.json`: canonical `schema_version` 2 validation-case manifests
  (Issue #51: meteorology/candidate domain, optional explicit comparison
  explicit `output_grid` (required for every v2 case), release, wind, surface,
  integration, physics switches, deposition forcing, stochastic identities,
  execution profile, explicit oracle-execution policy, stable oracle meteorology profile,
  oracle overrides, expected artifacts, and content-addressed species-physics
  references under `reference/species-physics/`). Their checked-in machine-readable
  structural contract is `schemas/validation-case-v2.schema.json` (JSON Schema
  Draft 2020-12); cross-field scientific invariants remain enforced by
  `src/validation/case.rs`. Timestamp chronology (Gregorian validity, release-window containment, and full meteorology coverage of the simulation interval) is enforced fail-closed by the Rust contract and mirrored by the raw-Python generator. Units are documented per field. No candidate output
  is used as a reference. The legacy v1 shape is frozen and rejected; see
  `cases/MIGRATION_NOTES.md` for the field-by-field v1 -> v2 mapping.
- `fortran/<CASE>/`: versioned Fortran oracle inputs (`COMMAND`, `RELEASES`,
  `OUTGRID`, `AGECLASSES`, `RECEPTORS`, `SPECIES/`). They reuse the existing
  synthetic-GRIB workflow (`scripts/generate_synthetic_grib.py`) and the
  in-fork Docker oracle (`docker/docker-compose.fortran.yml`). No second
  reference path is introduced.
- `thresholds.json`: versioned scientific thresholds with justification.
  Oracle-vs-candidate metrics are diagnostic; no parity gate is versioned here.

## Real-weather case

The ERA5/ETEX mini run (`fixtures/etex/native-mini/`,
`fixtures/etex/mini/`, `scripts/run-etex.sh mini`) is the real-weather corpus
member (`ETEX-MINI-013`). Its manifest mirrors the existing mini pipeline:
65×41×16 prepared meteorology, the real ETEX release, 900 s candidate timestep,
and the distinct 64×40×5 concentration output grid. No new large datasets are added. Its input audit
status remains `INPUT_EQUIVALENCE_NOT_DEMONSTRATED`
(`fixtures/etex/mini/README.md`, `target/etex/mini/input_equivalence_report.json`);
a green corpus smoke test must never override that audit.

## Provenance

`scripts/run-corpus.sh all` writes raw candidate and oracle outputs under
`target/corpus/`, machine-readable comparisons under
`target/corpus/comparison_report.json`, and provenance under
`target/corpus/run_manifest.json` (input/output hashes, oracle commit,
candidate revision, Fortran build, adapter, seeds). Oracle outputs are
produced at validation time and never vendored as fixtures.
