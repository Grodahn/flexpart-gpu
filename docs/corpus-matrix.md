# Test corpus matrix (Issue #6, point 2)

Normative oracle: unmodified FLEXPART 11.1 at
`c70586c2b7f5258850705325881c61f557ea9bd8`
(`reference/flexpart-11.1.json`, `docs/reference-environment.md`).
Only this fork is modified. Existing Docker, comparison and ETEX workflows
are reused; no second reference path is introduced.

`INPUT_EQUIVALENCE_NOT_DEMONSTRATED` (ETEX mini input audit,
`fixtures/etex/mini/README.md`) stays in force. No green corpus smoke test
overrides it.

## How to read this matrix

- **Candidate path**: `production` means the full productive WGSL path
  (`ForwardTimeLoopDriver`: dual-wind advection + fused Hanna/Langevin with
  `n_substeps=4` + GPU PBL diagnostics + deposition forcing).
  `isolated` means a single production WGSL kernel dispatched directly
  (advection-only, Langevin-only, deposition-only, convection-workflow).
  Isolated tests pin kernel math; only `production` runs exercise the full
  pipeline.
- **Oracle path**: `production` means the pinned Fortran `FLEXPART`
  executable in the in-fork Docker image
  (`docker/docker-compose.fortran.yml`) with versioned
  `fixtures/corpus/fortran/<CASE>/` inputs and synthetic GRIB meteorology
  from the single `scripts/generate_synthetic_grib.py` path.
- **Status**: `implemented` runs on both programs (or documents a precise
  single-program limitation without claiming parity). `blocked` names a
  verifiable dependency and is never reported as passed.

## Matrix

| #6 req | Case | Candidate fixture | Candidate runner / path | Oracle fixture | Oracle runner / path | Raw artifacts | Result / dependency |
|---|---|---|---|---|---|---|---|
| 1 | ADV-ANA-001 analytic advection, no turbulence | `fixtures/corpus/cases/ADV-ANA-001.json` | `cargo test --test integration corpus_analytic_advection` + `src/bin/corpus-run.rs`; `isolated` WGSL advection (60 dispatches, 10 m/s, 3600 s) | `fixtures/corpus/fortran/ADV-ANA-001/` (`LTURBULENCE=0`, `LCONVECTION=0`, inert `SPECIES_024`) | `scripts/run-corpus.sh oracle ADV-ANA-001`; `production` Fortran | `target/corpus/candidate/ADV-ANA-001/`, `target/corpus/oracle/ADV-ANA-001/`, `target/corpus/comparison_report.json` | Implemented. Analytic: 36000 m east, 0 m north/vertical, no spread, mass conserved. Thresholds: `thresholds.json` analytic_*. |
| 2 | WIND-UNI-002 uniform wind + H/V turbulence | `fixtures/corpus/cases/WIND-UNI-002.json` | `src/bin/corpus-run.rs`, `tests/integration/corpus.rs`; `production` driver (5/-3 m/s, 12x300 s, neutral PBL 1500 m) | `fixtures/corpus/fortran/WIND-UNI-002/` (`LTURBULENCE=1`, `CTL=5.0` modern Hanna, `LCONVECTION=0`) | `scripts/run-corpus.sh oracle WIND-UNI-002`; `production` Fortran | `target/corpus/candidate/WIND-UNI-002/seed_*.json` (10 Philox seeds) + oracle + metrics | Implemented. COM advects east/south; spread in x/y/z; PBL confined; mass conserved. Oracle spread metrics diagnostic (no parity gate). |
| 2 | WIND-SHEAR-003 sheared wind + H/V turbulence | `fixtures/corpus/cases/WIND-SHEAR-003.json` (`u(z)=2+0.004z`) | `production` driver | `fixtures/corpus/fortran/WIND-SHEAR-003/` + `METEO.txt` (`--u-shear-per-m 0.004`) | `production` Fortran | Same layout, 10 seeds | Implemented with open input note: candidate uses true-height shear; oracle uses a level-proxy shear in GRIB (documented approximation, diagnostic comparison). |
| 3 | PBL-STABLE-004 | `fixtures/corpus/cases/PBL-STABLE-004.json` (SSHF -20, OLI +0.02, BLH 500 m) | `production` driver | `fixtures/corpus/fortran/PBL-STABLE-004/` | `production` Fortran | 10 seeds + metrics | Implemented. Expect suppressed mixing (small std_z, mean near 50 m). Vertical quantiles diagnostic. |
| 3 | PBL-NEUTRAL-005 | `fixtures/corpus/cases/PBL-NEUTRAL-005.json` (SSHF 0, OLI 0, BLH 1500 m) | `production` driver; CI single-seed smoke | `fixtures/corpus/fortran/PBL-NEUTRAL-005/` | `production` Fortran | Same | Implemented. Moderate mixing; CI deterministic subset member. |
| 3 | PBL-UNSTABLE-006 | `fixtures/corpus/cases/PBL-UNSTABLE-006.json` (SSHF +150, OLI -0.02, BLH 2000 m, WST 1.5) | `production` driver | `fixtures/corpus/fortran/PBL-UNSTABLE-006/` | `production` Fortran | 10 seeds + metrics | Implemented. Vigorous mixing. Deep convection stays off (`LCONVECTION=0`). |
| 4 | DRY-007 dry-only | `fixtures/corpus/cases/DRY-007.json` (vdep 0.02 m/s) | `isolated` dry kernel (analytic `exp`) + `production` driver with dry forcing, wet zero | `fixtures/corpus/fortran/DRY-007/` (aerosol `SPECIES_040`, zero-precip GRIB) | `production` Fortran | Isolated analytic check in `deposition_decay` tests + driver budgets in corpus artifacts | Implemented. Isolated kernels assert analytic decay within `deposition_*` tolerances; driver/oracle budgets diagnostic. |
| 4 | WET-008 wet-only | `fixtures/corpus/cases/WET-008.json` (lambda 0.005 1/s, frac 1) | `isolated` wet kernel + `production` driver with wet forcing, dry zero | `fixtures/corpus/fortran/WET-008/` (aerosol `SPECIES_040`, precipitating GRIB) | `production` Fortran | Same | Implemented. Same threshold structure as dry. |
| 7 | REPEAT-009 repeatability | `fixtures/corpus/cases/REPEAT-009.json` | `production` driver twice, same Philox key/counter; `tests/integration/corpus.rs::corpus_repeatability_is_bit_identical` | n/a | Documented cause only | `target/corpus/candidate/REPEAT-009/` | Implemented (candidate). Fortran has no seed control (`random_mod.f90 alloc_random`: `iseed1=-7-i`, `iseed2=-88-i`, no namelist override) plus OpenMP scheduling nondeterminism; no multi-seed oracle parity is claimed. Stochastic corpus runs use 10 candidate Philox seeds vs one oracle realization, diagnostically. |
| 7 | RESTART-010 restart | — | Missing: no serialize/restore or continuation API in `src/simulation/timeloop.rs` at the corpus base | `fixtures/corpus/fortran/RESTART-010/` (oracle-only `IPIN`/`LOUTRESTART` note, `restart_mod.f90`) | Oracle capability documented, no paired run | None | **Blocked.** Must not be reported as passed. |
| 5 | DECAY-011 analytic decay | — | Missing from `production` driver (no decay kernel or `ForwardStepForcing` decay lane at the corpus base; isolated kernels cover deposition only) | Oracle supports `PDECAY` half-life (`examples/Nuclear/SPECIES/SPECIES_021`, Xe-133 453168 s) | No paired run | None | **Blocked.** Future gate: `exp(-lambda t)`, `lambda=ln2/half-life`, with decayed reservoir. Must not be reported as passed. |
| 6 | CONV-012 convection column | — | `isolated` only (`src/physics/convection.rs`, `src/gpu/convection.rs`, `tests/convection_chain.rs`); not orchestrated by the driver | Oracle supports `LCONVECTION`/`convect43c.f90` | No paired run | None | **Blocked.** Future gate: column-stochastic redistribution conserves mass and lifts bottom-heavy profiles. Must not be reported as passed. |
| 8 | ETEX-MINI-013 real weather | `fixtures/etex/native-mini/`, `fixtures/etex/mini/config/`, `fixtures/etex/data/` | `production` `etex-run` driver (16 AGL levels, omega-derived w); `scripts/run-etex.sh mini` | Same ERA5 source + `fixtures/etex/mini/config/` | `production` Fortran (137 hybrid levels, etadot) via `scripts/run-etex.sh mini` | `target/etex/mini/` (paired report, `run_manifest.json`, `input_equivalence_report.json`) | Implemented as pipeline/observation-pairing regression. **No parity verdict**: `INPUT_EQUIVALENCE_NOT_DEMONSTRATED`; metrics diagnostic. |

## Pre-existing tests vs corpus production runs

| Existing test | What it pins | Why it is not a corpus production run |
|---|---|---|
| `SW-WGPU-ADVECTION-001` (`software_advection.rs`) | Isolated uniform advection math | Advection kernel only; no Langevin, PBL, deposition, or driver orchestration. ADV-ANA-001 reuses its analytic expectation through the corpus runner. |
| `horizontal_dispersion.rs` | Discrete Ornstein-Uhlenbeck kernel math (legacy `n_substeps=0`) | Langevin kernel only; production uses fused `n_substeps=4`. |
| `deposition_decay.rs`, `scientific_invariants.rs` | Isolated deposition/gridding math and budgets | Kernel-level analytic checks; DRY-007/WET-008 add the driver-level runs. |
| `physics_validation.rs` | One neutral production-driver smoke (500 particles, 12 steps) | Single regime; corpus adds uniform/shear, three PBL regimes, deposition variants, 10-seed handling and oracle pairing. |
| `convection_chain.rs` | Isolated simplified-Emanuel chain | No driver orchestration; CONV-012 stays blocked. |
| `mass_conservation.rs` | CPU/GPU advection+turbulence mass invariance (direct kernel calls) | Not through the driver; corpus mass checks run through the driver. |

## Metrics and provenance

`scripts/corpus/compare_corpus.py` computes machine-readable metrics per
implemented case: total mass and conservation error, center of mass,
covariance with eigenvalues, vertical quantiles (p10/p50/p90, mean/std),
gridded overlap and field correlation (where a shared grid exists), and
process budgets (airborne vs deposited/decayed where applicable). Oracle
comparisons are diagnostic; no threshold in `thresholds.json` turns them
into a parity pass.

`scripts/run-corpus.sh all` records `target/corpus/run_manifest.json` with
SHA-256 of every input and raw output, oracle commit and cleanliness,
candidate revision and dirtiness, Fortran compiler/build, adapter identity
and Philox seeds. Candidate outputs are never used as references.
