# Scientific changelog

Track physics-affecting changes to flexpart-gpu. Every PR that modifies
shaders, physics kernels, or advection logic must add an entry here.

## Format

```
### YYYY-MM-DD — <one-line summary>
**Impact**: <physics | numerics | output-only | none>
**Files**: <list of changed shaders / physics modules>
**Validation**: <what was tested, results>
```

---

## Entries

### 2026-09-17 — Compare output-cell mass on both sides
**Impact**: output-only (comparison analysis; no trajectory or GPU calculation change)
**Files**: `scripts/compare_concentrations.py`,
`scripts/test_compare_concentrations.py`, `docs/validation-report.md`
**Validation**: The pinned FLEXPART 11.1 and GPU 10,000-particle outputs for
05:30–06:00 UTC were reanalyzed after converting the oracle concentration to
mass per cell using FLEXPART's output-cell area and layer thickness. Normalized field
correlation is 0.919 and the GPU-minus-oracle vertical center difference is
-5.2 m. Four focused regression tests pass. Scientific parity remains open.

### 2026-09-17 — Reproducible Issue #6 test corpus (point 2)
**Impact**: none (new fixtures, runners and documentation; no shader or physics change)
**Files**: `fixtures/corpus/`, `docs/corpus-matrix.md`, `src/bin/corpus-run.rs`,
`tests/integration/corpus.rs`, `scripts/run-corpus.sh`, `scripts/corpus/`,
`scripts/generate_synthetic_grib.py` (optional shear/surface overrides)
**Validation**: 6/6 corpus CI tests pass on software WGSL (18.9 s);
full synthetic candidate (8 cases, 10 Philox seeds) runs on the Microsoft Basic
Render Driver with mass conserved, PBL-confined and regime-separated vertical
mixing; restart/decay/convection stay blocked with verifiable causes; ETEX mini
remains `INPUT_EQUIVALENCE_NOT_DEMONSTRATED`.

### 2026-09-16 — Replace mini pressure-level weather with native ERA5
**Impact**: numerics (meteorological forcing and vertical interpolation)
**Files**: `fixtures/etex/native-mini/`,
`scripts/etex/prepare_native_era5.py`, `scripts/run-etex.sh`
**Validation**: Six 137-level ERA5 snapshots and separate eta-coordinate
velocity fields were SHA-256 verified. The pinned FLEXPART 11.1 oracle ran
with the native hybrid levels; the software-WGSL candidate ran with 16
interpolated AGL levels. The end-to-end ETEX mini workflow completed 48 GPU
steps, four paired three-hour output windows and 108 observation matches.
The candidate's ETEX station concentrations still differ substantially from
the oracle; vertical-coordinate equivalence and scientific parity are not
established by this smoke run.

### 2026-09-16 — Add a checked-in real ERA5 ETEX mini run
**Impact**: output-only (ETEX forcing conversion, grid and averaging windows)
**Files**: `scripts/etex/prepare_flexpart_input_from_npy.py`,
`scripts/etex/prepare_gpu_meteo.py`, `src/bin/etex-run.rs`,
`fixtures/etex/mini/`, `fixtures/etex/real/config/OUTGRID`
**Validation**: A SHA-256-verified 16-hour ERA5 subset drives the pinned
FLEXPART 11.1 oracle and software-WGSL candidate for 12 hours. The four
three-hour candidate windows now use 13 endpoint-inclusive samples with
half-weighted endpoints and stop after exactly 48 steps. The paired ETEX
comparison matched 108 station records. The pressure-level to hybrid-level
conversion for Fortran is approximate, while the candidate retains pressure
levels, so these concentration metrics are pipeline diagnostics only and do
not establish scientific parity.

### 2026-09-16 — Align synthetic comparison time windows
**Impact**: output-only (validation runner timing and concentration averaging)
**Files**: `src/bin/fortran-validation.rs`,
`scripts/compare_concentrations.py`, `scripts/compare-fortran.sh`
**Validation**: The candidate now runs exactly 24 900-second steps and
averages its 05:30, 05:45, and 06:00 concentration samples with FLEXPART's
half-weight endpoints for the oracle's 05:30–06:00 window. The comparator
rejects mismatched output windows. A local 10,000-particle run against pinned
FLEXPART 11.1 measures gridded covariance and center differences; these are
diagnostics from one run, so RISK-03.3G-02 parity remains unverified.

### 2026-09-16 — Enforce the software WGSL advection acceptance gate
**Impact**: none (test coverage and CI)
**Files**: `tests/integration/software_advection.rs`,
`.github/workflows/software-wgpu.yml`
**Validation**: The required software adapter is asserted before dispatch.
The test measures signed eastward motion and individual particle end positions
against the #139 200 m bound. Lavapipe CI runs the same WGSL path; local WARP
execution is used during development.

### 2026-09-16 — Pair ETEX concentration windows with the Fortran oracle
**Impact**: output-only (three-hour mean replaces an end-time snapshot)
**Files**: `src/bin/etex-run.rs`, `scripts/etex/compare_oracle_observations.py`,
`fixtures/etex/real/config/COMMAND`, `fixtures/etex/real/config/OUTGRID`
**Validation**: The ETEX driver now samples every 900 seconds and averages
12 post-step samples for each 10,800-second window. The later mini-run entry
above corrects this to FLEXPART's 13 endpoint-inclusive, half-weighted
samples. The paired comparator rejects unmatched windows,
missing fields, and incomplete station coverage. Focused synthetic comparator
tests and a release build passed. A real ERA5/ETEX run is still required
before reporting observational metrics or a parity result.

### 2026-09-16 — Pin oracle environment and capture run provenance
**Impact**: none (Fortran build environment and comparison reporting)
**Files**: `docker/Dockerfile.fortran`, `scripts/compare-fortran.sh`,
`scripts/run-etex.sh`, `scripts/write_oracle_run_manifest.py`
**Validation**: The image uses a digest-pinned Ubuntu base, a dated apt
snapshot and the portable `arch=x86-64` Fortran build profile. Both Docker
comparison paths record the image, installed packages, executable hash,
revision, adapter and raw-artifact hashes. The manifest states when a random
seed is unavailable; it does not claim scientific parity.

### 2026-03-06 — Fused Hanna+Langevin default production path
**Impact**: none (identical physics, different execution path)
**Files**: `langevin_fused.wgsl`, `gpu/langevin_fused.rs`, `simulation/timeloop.rs`
**Validation**: The production path fuses Hanna PBL turbulence parameterisation
and Langevin velocity update into a single GPU dispatch (`langevin_fused.wgsl`),
eliminating the intermediate HannaParams buffer and one dispatch barrier.
Physics is identical to the separated shaders (verified by textual diff of
every function). The separated Hanna → Langevin path is retained under
`FLEXPART_GPU_VALIDATION=1` for scientific validation.

*Note*: A full mega-kernel approach (advection + Hanna + Langevin + deposition
in one dispatch) was attempted and abandoned due to severe register pressure
on the target GPU. The mega-kernel source remains in `particle_step.wgsl` /
`particle_step.rs` for reference only.

### 2026-03-06 — GPU-side PBL diagnostics
**Impact**: numerics (minor — f32 vs previous CPU f32, same formulas)
**Files**: `pbl_diagnostics.wgsl`, `gpu/pbl.rs`, `simulation/timeloop.rs`
**Validation**: PBL parameters (u*, w*, L, h) now computed on GPU per grid
cell. Same formulas as CPU reference (`io/pbl_params.rs`). CPU reference
retained for tests.

### 2026-03-06 — GPU-side dual-wind temporal interpolation
**Impact**: numerics (minor — interpolation order changed)
**Files**: `advection_dual_wind.wgsl`, `advection_texture_dual_wind.wgsl`,
`gpu/advection.rs`, `simulation/timeloop.rs`
**Validation**: Wind brackets (t0, t1) uploaded once per met change. GPU
performs `(1−α)·t0 + α·t1` inline during advection. Removes per-step CPU
interpolation and wind re-upload. Interpolation result is mathematically
identical (same linear formula).

### 2026-03-06 — Active particle compaction
**Impact**: none (reorders particle buffer, physics unchanged)
**Files**: `compaction.wgsl`, `gpu/compaction.rs`, `simulation/timeloop.rs`
**Validation**: Prefix-sum compaction packs active particles to the front
of the buffer, reducing wasted GPU work on inactive particles.

### 2026-03-06 — hanna_short recalculation between sub-steps
**Impact**: physics
**Files**: `langevin.wgsl`
**Validation**: Fortran comparison shows vertical mean gap reduced from 86m
to 22m (74% improvement). sigma_z ratio = 0.94. Between each vertical
sub-step (except the last), `sigw`, `dsigwdz`, and `tlw` are recalculated
at the particle's new height using the Hanna (1982) profile equations,
exactly matching Fortran's `hanna_short(zt)` call in `advance.f90`.

### 2026-03-06 — Vertical turbulence sub-stepping (ifine=4)
**Impact**: physics
**Files**: `langevin.wgsl`, `advection.wgsl`, `advection_texture.wgsl`,
`gpu/langevin.rs`, `physics/langevin.rs`, `simulation/timeloop.rs`
**Validation**: All 35 tests pass. `physics_validation_advection_turbulence_pbl`
confirms PBL confinement, mass conservation, and advection direction with
n_substeps=4. Vertical turbulence now matches Fortran's `ifine` sub-stepping:
each timestep splits the vertical Langevin update into 4 sub-steps with
`dt_sub = dt/4`, applying displacement and PBL reflection between each.
Advection shader no longer applies `turb_w` to vertical displacement.

### 2026-03-06 — PBL boundary reflection kernel
**Impact**: physics
**Files**: `pbl_reflection.wgsl`, `gpu/pbl_reflection.rs`,
`simulation/timeloop.rs`
**Validation**: Fortran comparison shows GPU particles confined within
[0, BLH] with correct reflection behavior. Vertical mean z gap reduced
from >1000m to 180m vs Fortran.

### 2026-03-06 — Fix has_level_heights() ground-level detection
**Impact**: physics (critical)
**Files**: `advection.wgsl`, `advection_texture.wgsl`,
`concentration_gridding.wgsl`
**Validation**: Fixed bug where `level_heights[0]=0.0` caused fallback to
grid-level indexing. Now checks top-most level. Prevents particle clamping
to [0, nz-1] meters when first wind level is at ground.

### 2026-03-06 — Apply turbulent velocities to advection
**Impact**: physics (critical)
**Files**: `advection.wgsl`, `advection_texture.wgsl`
**Validation**: Particles now disperse correctly. Before this fix, Langevin
computed turb_u/v/w but advection ignored them — dispersion was zero.

### 2026-03-06 — Fix pos_z meters-vs-levels in advection and gridding
**Impact**: physics (critical)
**Files**: `advection.wgsl`, `advection_texture.wgsl`,
`concentration_gridding.wgsl`
**Validation**: `pos_z` (meters) is now correctly converted to fractional
grid level for wind sampling and to output level for gridding. Before,
particles were clamped to z < 3m.

---

## PR checklist (copy into PR description)

```markdown
### Scientific impact

- [ ] Does this PR modify shaders, physics modules, or advection logic?
- [ ] If yes: entry added to `docs/scientific-changelog.md`
- [ ] `cargo test` passes (all integration + scientific invariant tests)
- [ ] `physics_validation_advection_turbulence_pbl` test passes
- [ ] No new linter warnings in modified files
```
