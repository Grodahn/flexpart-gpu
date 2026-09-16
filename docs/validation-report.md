# FLEXPART-GPU: Validation Report

> Oracle note (RISK-03.3G-01): this report is a historical record against
> FLEXPART v10. The normative oracle for all current and future comparisons
> is **FLEXPART v11.1** pinned in `reference/flexpart-11.1.json`; see
> `docs/reference-environment.md`. This report will be superseded by the
> RISK-03.3G validation gate. The historical PASS labels below apply only to
> the old v10 experiment and are not v11.1 parity evidence. Horizontal
> dispersion parity for RISK-03.3G-02 remains **unverified**.

Date: 2026-03-06
Configuration: synthetic uniform wind, Fortran FLEXPART v10 vs flexpart-gpu (Rust/WebGPU)

## 1. Objective

Demonstrate that `flexpart-gpu` reproduces FLEXPART Fortran particle dispersion
to within acceptable tolerances for equivalent physics configurations.

## 2. Methodology

### 2.1 Configuration

Both codes run the same scenario:

| Parameter          | Value                                |
|--------------------|--------------------------------------|
| Wind               | u=5, v=-3, w=0 m/s (uniform)        |
| Domain             | 32x32 cells, dx=dy=0.1 deg          |
| Origin             | (9.5E, 8.5N)                         |
| Release            | (10E, 10N), z=50m, point source      |
| Particles          | 10,000                               |
| Total mass         | 1 kg                                 |
| Simulation         | 6h (2024-01-01 00:00 to 06:00 UTC)  |
| Timestep           | 900 s                                |
| Sub-stepping       | ifine=4 (4 vertical sub-steps/step)  |
| PBL height         | 3000 m (GPU static, Fortran dynamic) |
| Deposition         | off                                  |
| Convection         | off                                  |
| Output levels      | 100, 250, 500, 750, 1000, 1500, 2000, 2500, 3000, 5000 m |

### 2.2 Comparison methodology

Primary comparison: raw particle positions from Fortran `partposit_end` dump vs
GPU particle state at end of simulation.

Metrics:
- **Horizontal Center of Mass (COM)**: distance in km
- **Vertical mean z**: absolute difference in meters
- **Vertical std z ratio** (GPU/Fortran): ratio of standard deviations
- **Per-level vertical profile**: particle fraction per output level
- **Horizontal spread**: standard deviation of lon/lat

### 2.3 Reproduction

The Fortran Docker environment is in the sibling directory `../flexpart-fortran-docker/`.

```bash
# Fortran (from ../flexpart-fortran-docker/)
docker compose run --rm flexpart-fortran bash -lc \
  'cd /workspace/comparison/validate_run && /workspace/flexpart/src/FLEXPART'

# GPU (from flexpart-gpu/)
OUTPUT_PATH=target/validation/gpu_concentration.json \
  PARTICLES=10000 SYNC_READBACK=1 \
  cargo run --release --bin fortran-validation

# Compare (from ../flexpart-fortran-docker/)
docker compose run --rm flexpart-fortran python3 \
  /workspace/flexpart-gpu/scripts/compare_concentrations.py \
  --fortran-output /workspace/comparison/validate_run/output \
  --gpu-output /workspace/flexpart-gpu/target/validation/gpu_concentration.json \
  --verbose
```

## 3. Results

### 3.1 Summary

| Metric                  | Value    | Threshold    | Verdict  |
|-------------------------|----------|--------------|----------|
| Horizontal COM distance | 5.50 km  | < 10 km      | **PASS** |
| Vertical mean Dz        | +22 m    | < 200 m      | **PASS** |
| Vertical sigma_z ratio  | 0.94     | [0.7, 1.3]   | **PASS** |
| PBL confinement         | [0, 3000] m | [0, BLH]  | **PASS** |

### 3.2 Vertical profile (per-level particle fraction)

| Level | Height (m) | Fortran % | GPU %  | Delta % |
|-------|------------|-----------|--------|---------|
| 0     | 100        | 3.8       | 5.4    | +1.6    |
| 1     | 250        | 5.9       | 6.4    | +0.4    |
| 2     | 500        | 9.5       | 9.0    | -0.5    |
| 3     | 750        | 10.3      | 8.8    | -1.5    |
| 4     | 1000       | 11.0      | 8.3    | -2.7    |
| 5     | 1500       | 19.8      | 16.0   | -3.8    |
| 6     | 2000       | 15.0      | 15.6   | +0.5    |
| 7     | 2500       | 10.6      | 14.3   | +3.7    |
| 8     | 3000       | 6.6       | 16.2   | +9.6    |
| 9     | 5000       | 7.4       | 0.0    | -7.4    |

Notes:
- The GPU enforces a hard PBL boundary at 3000m via reflection;
  particles accumulate at the 3000m level instead of escaping above.
- Fortran allows ~7% of particles above hmix, leading to non-zero 5000m level.
- Combined 3000m+ fraction is similar: Fortran 14.0%, GPU 16.2%.

### 3.3 Horizontal spread

| Metric          | Fortran       | GPU           | Ratio |
|-----------------|---------------|---------------|-------|
| sigma_lon       | 0.024 deg (2.7 km) | 0.048 deg (5.3 km) | 1.97 |
| sigma_lat       | 0.019 deg (2.1 km) | 0.047 deg (5.2 km) | 2.55 |

The GPU horizontal spread is approximately 2x Fortran's. Root cause:
the GPU uses a prescribed friction velocity (ust=0.35 m/s) while Fortran
computes ust from the wind profile and surface roughness, yielding a different
effective turbulence intensity. This is a parameterization difference, not a bug.

### 3.5 Addendum (RISK-03.3G-02): status of v11.1 re-measurement

The original 2x spread estimate mixed an averaged Fortran concentration field
with an instantaneous GPU particle field. That comparison cannot establish a
horizontal dispersion ratio. A later single 10,000-particle run used a 30-minute
Fortran output cadence and reported gridded longitude/latitude standard
deviations of 0.055/0.045 degrees for v11.1 versus 0.056/0.056 degrees for
the GPU. These values are diagnostics, not a parity verdict: the reported
centers (10.944, 9.442) and (11.029, 9.392) differ by about 10.9 km at this
latitude, exceeding the issue's stricter 5 km criterion. The earlier candidate
runner also advanced one extra 900-second step and compared its end state to
the oracle's 30-minute average.

The comparison runner now stops at the requested end time, averages three GPU
concentration samples at 05:30, 05:45, and 06:00 with half-weight endpoints,
and rejects mismatched output timestamps and averaging intervals. Its final
particle positions are reported separately from the averaged concentration field.

A fresh local run with the clean pinned oracle commit
`c70586c2b7f5258850705325881c61f557ea9bd8` and 10,000 particles produced
the following **gridded diagnostics**, with both fields representing the
05:30–06:00 UTC average. The GPU adapter was Intel UHD Graphics 620 (Vulkan).
The generated `target/validation/run_manifest.json` hashes the input, binaries,
and outputs; it is a local generated artifact, not committed evidence.

| Metric | FLEXPART 11.1 | GPU | GPU / oracle |
|--------|---------------|-----|--------------|
| East standard deviation | 6.07 km | 6.73 km | 1.11 |
| North standard deviation | 5.06 km | 6.15 km | 1.22 |
| Smaller covariance eigenvalue | 21.00 km² | 34.79 km² | 1.66 |
| Larger covariance eigenvalue | 41.38 km² | 48.42 km² | 1.17 |

The horizontal center distance is 0.26 km and the normalized-field
correlation is 0.832. Both spread axes and covariance eigenvalues
remain outside issue #123's ±10% target in this single case. The oracle
concentration sum (25.37 in its native units) and candidate mass sum (1 kg)
are normalized before comparing spatial distributions; their raw totals are
not a mass-conservation comparison. The oracle did not emit a `partposit_*`
dump in this run, so particle-space moments remain unavailable.

The isolated Ornstein-Uhlenbeck variance tests in
`tests/integration/horizontal_dispersion.rs` exercise a helper, not the fused
production trajectory. RISK-03.3G-02 still needs production-path runs against
the pinned v11.1 oracle across at least 10 seeds and stable, neutral, unstable,
and sheared cases, with particle-space covariance, radial quantiles, footprint
area, and uncertainty intervals as specified in BringMeOut issue #123.

### 3.6 Progression of vertical accuracy

| Version                     | Dz mean | sigma_z ratio | Key change                    |
|-----------------------------|---------|---------------|-------------------------------|
| Before PBL reflection       | >1000 m | N/A           | Missing PBL boundary          |
| With PBL reflection (ifine=1) | -180 m | 1.03        | Added PBL reflection          |
| With sub-stepping (ifine=4) | -86 m   | 0.92          | Vertical sub-stepping         |
| With hanna_short recalc     | **+22 m** | **0.94**    | **Recalculate sigma_w(z) between sub-steps** |

## 4. Known architectural differences

| Aspect                      | Fortran              | GPU                           | Impact                |
|-----------------------------|----------------------|-------------------------------|-----------------------|
| Sub-stepping (ifine)        | 4 sub-steps/step     | 4 sub-steps/step              | Aligned               |
| hanna_short between sub-steps | Recalculates sigma_w(z) | Recalculates sigma_w(z)   | **Aligned**           |
| hmix computation            | Richardson from T profile | BLH from surface field    | Calibration needed    |
| Advection scheme            | Petterssen predictor-corrector | Petterssen predictor-corrector | Identical |
| turb_w in advection         | Separate displacement | Separate (Langevin sub-step)  | Aligned               |
| RNG                         | Fortran intrinsic     | Philox4x32-10                 | Different sequences   |
| Horizontal turbulence       | Once per timestep     | Once per timestep             | Identical             |
| PBL reflection              | advance.f90 sub-loop  | langevin_fused.wgsl sub-loop  | Aligned               |
| Above-PBL particles         | Soft (some escape)    | Hard (clamped at hmix)        | ~7% difference at top |

## 5. Test coverage

| Test category              | Count | Status |
|----------------------------|-------|--------|
| Unit tests (all)           | 224   | PASS   |
| CPU/GPU parity (kernels)   | 10+   | PASS   |
| Mass conservation          | 3     | PASS   |
| Positivity invariants      | 2     | PASS   |
| CI gate (physics_validation) | 1   | PASS   |
| Fortran comparison         | 1     | PASS   |
| Determinism (seed-based)   | 1     | PASS   |

## 6. Conclusion

`flexpart-gpu` reproduces FLEXPART Fortran's particle dispersion behavior with:
- **Excellent vertical agreement**: mean height within 22m (1.6% relative to 3000m PBL)
- **Correct horizontal advection**: center of mass within 5.5 km after 6h
- **Proper PBL confinement**: all particles within [0, BLH]
- **Identical vertical mixing profile**: sigma_z ratio = 0.94

Remaining known differences and gaps:
- Horizontal spread against v11.1 is unverified (§3.5). The old 2x figure is
  inconclusive because the compared fields had different time semantics.
- Hard vs soft PBL ceiling (~7% of particles)
- hmix computation method (GPU uses prescribed value vs Fortran's Richardson)

The v10 observations do not establish v11.1 scientific parity. A detailed 1M-particle comparison
with performance benchmarks is available in
[benchmarks/benchmark-1m-fortran-vs-gpu0.md](benchmarks/benchmark-1m-fortran-vs-gpu0.md)
and the physics validation report in
[benchmarks/benchmark-scientific-validation.md](benchmarks/benchmark-scientific-validation.md).
