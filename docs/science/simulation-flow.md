# Simulation Flow

This document describes the execution sequence of a typical forward simulation,
from launch to output. It maps FLEXPART concepts to the actual Rust/WGSL
modules and shows what runs on CPU vs GPU.

Two execution paths exist (see [architecture.md](../architecture.md) for
rationale): the **fused Hanna+Langevin** path (production default) and the **separated**
path (diagnostic validation, enabled with `FLEXPART_GPU_VALIDATION=1`). Both use
the same canonical resident Petterssen sequence described in
[the production inventory](../resident-advection.md). Backward retains its
existing separated turbulence operators and signed negative advection timestep.

## High-Level Pipeline — Production (Fused Hanna+Langevin)

```
┌─────────────────────────────────────────────────────────────────────┐
│                          INITIALISATION                             │
│  Load config ─▶ Build GPU context ─▶ Load meteorology ─▶ Init      │
│  (COMMAND,       (wgpu adapter,       (ERA5 GRIB/binary,  release   │
│   RELEASES,       device, queue)        time brackets)    manager,  │
│   OUTGRID,                             Async prefetch     particle  │
│   SPECIES)                             (grib2_async.rs)   store     │
└─────────────────────────┬───────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      TIME LOOP  (per timestep dt)                   │
│                                                                     │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ 1. RELEASE            CPU   release/mod.rs                   │   │
│  │    Inject new particles at scheduled source times            │   │
│  └──────────────────────────────┬───────────────────────────────┘   │
│  ┌──────────────────────────────▼───────────────────────────────┐   │
│  │ 2. CANONICAL WIND     CPU→GPU  (once per source change)      │   │
│  │    Prepare #173 U/V/center-W + exact #30 runtime geometry    │   │
│  │    Resolve separate current/predicted #89 time selections   │   │
│  └──────────────────────────────┬───────────────────────────────┘   │
│  ┌──────────────────────────────▼───────────────────────────────┐   │
│  │ 3. SURFACE FIELDS     CPU→GPU                                │   │
│  │    Upload interpolated surface fields for GPU PBL compute    │   │
│  └──────────────────────────────┬───────────────────────────────┘   │
│  ┌──────────────────────────────▼───────────────────────────────┐   │
│  │ 4. GPU PBL DIAGNOSTICS  GPU  shaders/pbl_diagnostics.wgsl   │   │
│  │    Compute u*, w*, L, h per grid cell on GPU                 │   │
│  └──────────────────────────────┬───────────────────────────────┘   │
│  ┌──────────────────────────────▼───────────────────────────────┐   │
│  │ 5. GPU PHYSICS (ordered passes, single command encoder)     │   │
│  │    5a. Resident Petterssen into private timestep state       │   │
│  │    5b. Fused Hanna+Langevin (turbulence + PBL reflection)    │   │
│  │    5c. Dry deposition                                        │   │
│  │    5d. Wet deposition                                        │   │
│  │                                                              │   │
│  │    queue.submit(encoder)                                     │   │
│  └──────────────────────────────┬───────────────────────────────┘   │
│  ┌──────────────────────────────▼───────────────────────────────┐   │
│  │ 6. READBACK (optional) GPU→CPU                               │   │
│  │    Download particle buffer (at output times or if sync on)  │   │
│  └──────────────────────────────┬───────────────────────────────┘   │
│  ┌──────────────────────────────▼───────────────────────────────┐   │
│  │ 7. OUTPUT GRIDDING    GPU   shaders/concentration_gridding   │   │
│  │    (at output intervals)                                     │   │
│  │    Atomic-add particle masses to concentration grid          │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  Advance time:  t ← t + dt                                          │
│  Loop until t ≥ t_end                                               │
└─────────────────────────────────────────────────────────────────────┘
```

## High-Level Pipeline — Validation (Separated Dispatches)

When `FLEXPART_GPU_VALIDATION=1` is set, step 5 above uses the following
separate stages (same scoped equations, separate shaders for easier debugging
and comparison against Fortran):

```
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │               GPU COMMAND ENCODER  (single submit)           │   │
│  │                                                              │   │
│  │  5a. ADVECTION       GPU   canonical resident Petterssen     │   │
│  │  5b. HANNA PARAMS    GPU   shaders/hanna_params.wgsl        │   │
│  │  5c. LANGEVIN        GPU   shaders/langevin.wgsl            │   │
│  │  5d. DRY DEPOSITION  GPU   shaders/dry_deposition.wgsl      │   │
│  │  5e. WET DEPOSITION  GPU   shaders/wet_deposition.wgsl      │   │
│  │                                                              │   │
│  │  queue.submit(encoder)                                       │   │
│  └──────────────────────────────────────────────────────────────┘   │
```

All other steps (release, wind upload, PBL, readback, gridding) are identical.

## Step-by-Step Detail

### 1. Release (`release/mod.rs`)

The `ReleaseManager` checks whether the current simulation time falls within
any active release window defined in `RELEASES`. If so, new particles are
initialised (position, mass, species) and uploaded to the GPU particle buffer.

**Fortran equivalent:** `releaseparticles` in `timemanager.f90`.

### 2. Canonical Wind Preparation (`simulation/timeloop/meteorology.rs`)

Both production drivers require a validated `CanonicalMeteorologyBracket` and
matching #30 runtime geometry. `prepare_canonical_meteorology` and the retained
`CanonicalMeteorologySlot` prepare persistent scalar U, V and normalized
model-center W GPU owners. No canonical metadata is inferred from legacy wind
fields. Operational legacy-only inputs fail closed pending #32.

Current and signed advanced times are resolved independently through #89.
A step outside source coverage rejects, including when a legacy surface-field
bounds policy permits clamping. The GPU samples each time through #87/#88/#89;
no sampled particle query or velocity is returned to the host between stages.
The previous dual-wind/one-alpha path is retained only by explicitly named
legacy diagnostic APIs.

### 3. PBL Diagnostics — GPU (`shaders/pbl_diagnostics.wgsl`)

Surface fields (friction velocity, sensible heat flux, surface stress,
2 m temperature) are uploaded to the GPU. A compute shader computes, per
grid cell:

- friction velocity u\*
- convective velocity scale w\*
- Obukhov length L
- mixing height h

The result is written to PBL buffers (separate `ustar`, `wstar`, `hmix`,
`oli` arrays) consumed by the fused Hanna+Langevin kernel (production) or
the separated Hanna kernel (validation).

**Dispatch:** `gpu/pbl.rs`

**Fortran equivalent:** `calcpar` → `obukhov`, `richardson`.

**Fallback:** The CPU-side reference (`io/pbl_params.rs`) still exists and is
used in CPU-only tests.

### 4. Advection — GPU

Each particle is advected by the mean wind using the Petterssen
predictor–corrector scheme:

1. Canonical sample at current position and time → (u₀, v₀, w₀)
2. Predict: x\_pred = x + dt · v₀
3. Canonical sample at predicted position and signed advanced time → (u₁, v₁, w₁)
4. Correct: x\_final = x + dt · 0.5·(v₀ + v₁)

Horizontal, vertical and temporal interpolation exclusively reuse #87/#88/#89
through #76/#171. Predictor queries use the existing split signed-cell/fraction
ABI and AGL metres without mutating scientific particles. Corrected state and
all downstream particle physics remain private until whole-step guarded
publication. A fatal U/V/W status in either stage preserves every scientific
particle byte and prevents downstream eligibility.

Velocities [m/s] are converted to grid displacement via `VelocityToGridScale`;
AGL height requires unit vertical scale. Existing turbulent U/V coupling and
boundary clamps remain; changing physical model tops fail closed pending #180.

**Consumer shaders:** `advection_predictor_query.wgsl`, `advection_corrector.wgsl`,
`advection_step_guard.wgsl`, `advection_step_commit.wgsl`.

**Orchestration:** `gpu/advection_resident.rs`; the existing sampler kernels
retain their own scientific contracts and pinned oracle evidence.

**Fortran reference:** pinned `advance_mod.f90:664-779` (`petterssen_corr`).
This scoped adoption does not establish full FLEXPART boundary/stochastic parity.

### 5. Hanna Turbulence Parameters + Langevin — GPU

In **production mode**, these two stages are fused into a single dispatch
(`langevin_fused.wgsl`). For each particle, the kernel:

1. Looks up PBL fields (u\*, w\*, L, h) from the PBL grid buffers
2. Computes σ\_u, σ\_v, σ\_w, T\_Lu, T\_Lv, T\_Lw, dσ\_w/dz inline
   (Hanna 1982, identical logic to `hanna_params.wgsl`)
3. Updates turb\_u, turb\_v (horizontal Langevin, full dt)
4. Sub-steps turb\_w with PBL reflection (Thomson 1987)

This eliminates the intermediate HannaParams buffer (~64 MB for 1M particles)
and one dispatch barrier.

In **validation mode**, these are two separate dispatches:
- `hanna_params.wgsl` writes HannaParams to a per-particle buffer
- `langevin.wgsl` reads HannaParams and applies the Langevin update

**Shaders:**
- `shaders/langevin_fused.wgsl` (production)
- `shaders/hanna_params.wgsl` + `shaders/langevin.wgsl` (validation)

**Dispatch:**
- `gpu/langevin_fused.rs` (production)
- `gpu/hanna.rs` + `gpu/langevin.rs` (validation)

**Fortran equivalent:** `hanna.f90` + `advance.f90` (Langevin section) +
`hanna_short.f90`.

### 6. Dry Deposition — GPU

For particles with `pos_z < 2 · h_ref` (h\_ref = 15 m):

```
survival = exp(−v_d · |dt| / (2 · h_ref))
mass ← mass · survival
```

The deposition velocity v\_d is provided per particle by the forcing vector.

**Shaders:** `shaders/dry_deposition.wgsl`

**Dispatch:** `gpu/deposition.rs`

**Fortran equivalent:** `drydepokernel.f90`, `getvdep.f90`.

### 7. Wet Deposition — GPU

```
P_wet = gr_fraction · (1 − exp(−λ · |dt|))
mass ← mass · (1 − P_wet)
```

The scavenging coefficient λ and precipitating fraction are forcing inputs.

**Shaders:** `shaders/wet_deposition.wgsl`

**Dispatch:** `gpu/wet_deposition.rs`

**Fortran equivalent:** `wetdepo.f90`, `wetdepokernel.f90`.

### 8. Host Readback (optional)

If `sync_particle_store_each_step` is enabled, the full particle buffer is
downloaded from GPU to CPU after each step. In production mode readback is
deferred (fire-and-forget) for maximum throughput.

### 9. Concentration Gridding — GPU (`shaders/concentration_gridding.wgsl`)

At output intervals, particle masses are binned into the 3D output grid
(OUTGRID) using atomic integer additions. The grid is then downloaded to CPU
and written to the output file.

**Dispatch:** `gpu/gridding.rs`

**Fortran equivalent:** `conccalc.f90` → `concoutput.f90`.

## GPU Command Encoding

### Production (fused Hanna+Langevin)

Dependency-ordered resident advection and physics passes share one caller-owned
encoder and submission. Downstream particle stages operate on private state:

```
encoder = device.create_command_encoder()
encoder.dispatch(pbl_diagnostics)        // per grid cell
advection = ResidentAdvectionStep::encode(...) // six samples, predictor/corrector, eligibility
encoder.dispatch(langevin_fused)         // inline Hanna + Langevin + PBL reflection
encoder.dispatch(dry_deposition)         // mass survival
encoder.dispatch(wet_deposition)         // mass survival
advection.encode_commit(...)            // all required samples must pass
queue.submit(encoder)
```

### Validation (separated dispatches)

The same resident advection stages feed separated turbulence passes within one
command encoder:

```
encoder = device.create_command_encoder()
encoder.dispatch(pbl_diagnostics)
advection = ResidentAdvectionStep::encode(...)
encoder.dispatch(hanna)
encoder.dispatch(langevin)
encoder.dispatch(dry_deposition)
encoder.dispatch(wet_deposition)
advection.encode_commit(...)
queue.submit(encoder)
```

## Comparison with Fortran FLEXPART

| Aspect | Fortran (`timemanager.f90`) | GPU (`simulation/timeloop.rs`) |
|--------|---------------------------|-------------------------------|
| Met I/O | `getfields` reads GRIB each step | Canonical wind/geometry upload on source change; operational decoding pending #32 |
| Wind interpolation | CPU, per-particle, per-step | GPU #87/#88/#89 at current and predicted positions/times |
| PBL | `calcpar` (u\*, L, h) on CPU | `pbl_diagnostics.wgsl` on GPU (per grid cell) |
| Convection | Emanuel scheme (`convmix`) | Not yet implemented |
| Advection | `advance.f90` (per-particle loop) | GPU shader, all particles in parallel |
| Turbulence | `hanna` + Langevin in `advance.f90` | Fused Hanna+Langevin (prod) or two separate passes (validation) |
| Deposition | `wetdepo`, `drydepokernel` | Separate dry + wet dispatches (both paths) |
| Output | `conccalc` + `concoutput` | GPU gridding + CPU download |
| Nested grids | Supported | Not yet implemented |

## Source Files

| Module | Path | CPU/GPU |
|--------|------|---------|
| Time loop orchestration | `src/simulation/timeloop.rs` | CPU (async) |
| Release manager | `src/release/mod.rs` | CPU |
| Met bracket management | `src/io/temporal.rs` | CPU |
| Async GRIB prefetch | `src/io/grib2_async.rs` | CPU (background thread) |
| PBL parameters (CPU ref.) | `src/io/pbl_params.rs` | CPU |
| **Fused Hanna+Langevin (production)** | `src/shaders/langevin_fused.wgsl` | **GPU** |
| **Fused H+L dispatch** | `src/gpu/langevin_fused.rs` | **CPU→GPU** |
| GPU PBL diagnostics | `src/shaders/pbl_diagnostics.wgsl` | GPU |
| GPU PBL dispatch | `src/gpu/pbl.rs` | CPU→GPU |
| Advection consumers | `src/shaders/advection_predictor_query.wgsl`, `advection_corrector.wgsl`, `advection_step_guard.wgsl`, `advection_step_commit.wgsl` | GPU |
| Advection orchestration | `src/gpu/advection_resident.rs` | CPU encode, GPU-resident intermediates |
| Hanna kernel (validation) | `src/shaders/hanna_params.wgsl` | GPU |
| Hanna dispatch (validation) | `src/gpu/hanna.rs` | CPU→GPU |
| Langevin kernel (validation) | `src/shaders/langevin.wgsl` | GPU |
| Langevin dispatch (validation) | `src/gpu/langevin.rs` | CPU→GPU |
| Dry deposition | `src/shaders/dry_deposition.wgsl` | GPU |
| Dry deposition dispatch | `src/gpu/deposition.rs` | CPU→GPU |
| Wet deposition | `src/shaders/wet_deposition.wgsl` | GPU |
| Wet deposition dispatch | `src/gpu/wet_deposition.rs` | CPU→GPU |
| Active particle compaction | `src/shaders/compaction.wgsl` | GPU |
| Compaction dispatch | `src/gpu/compaction.rs` | CPU→GPU |
| Concentration gridding | `src/shaders/concentration_gridding.wgsl` | GPU |
| Gridding dispatch | `src/gpu/gridding.rs` | CPU→GPU |
| Particle buffer | `src/gpu/buffers.rs` | GPU memory |
| Coordinate transforms | `src/coords/mod.rs` | CPU + GPU |
| ETEX real-file driver (blocked pending #32 canonical inputs) | `src/bin/etex-run.rs` | CPU (main) |
