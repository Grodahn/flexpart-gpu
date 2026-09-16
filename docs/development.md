# Development Guide

## Prerequisites

### Local (without Docker)

- Rust stable toolchain (`rustup`)
- Vulkan-capable GPU + drivers
- System libraries: `libvulkan1`, `libeccodes-dev` (optional), `libnetcdf-dev`
  (optional)
- Python 3 + `numpy`, `eccodes`, `cdsapi` (for ETEX pipeline only)

### With Docker (recommended)

- Docker + Docker Compose
- GPU drivers on the host (Vulkan-capable)
- NVIDIA Container Toolkit (for NVIDIA GPUs only)

## Building

### Docker workflow

```bash
# Compose files live in docker/
GPU_COMPOSE="-f docker/docker-compose.yml"
GPU_NVIDIA_COMPOSE="-f docker/docker-compose.yml -f docker/docker-compose.nvidia.yml"

# Build the GPU image (one-time)
docker compose ${GPU_COMPOSE} build flexpart-gpu

# Start a shell inside the container
docker compose ${GPU_COMPOSE} run --rm flexpart-gpu bash

# NVIDIA GPU variant
docker compose ${GPU_NVIDIA_COMPOSE} \
  run --rm flexpart-gpu bash

# Inside the container:
cargo build --release
```

Build cache is persisted in named Docker volumes (`cargo-home`, `cargo-target`)
so incremental builds are fast (~5–15 s).

### Local workflow

```bash
cargo build --release
```

### Optional features

```bash
# Enable GRIB2 reading (requires eccodes)
cargo build --release --features eccodes

# Enable NetCDF I/O
cargo build --release --features netcdf
```

## GPU Preflight

Verify that the GPU backend is working:

```bash
# Docker
docker compose -f docker/docker-compose.yml run --rm flexpart-gpu cargo run --bin gpu-preflight

# Or via helper script
scripts/gpu-preflight.sh compose
scripts/gpu-preflight.sh nvidia

# Local
cargo run --bin gpu-preflight
```

This prints the detected GPU adapter, backend, and runs a minimal compute
shader smoke test.

Interpretation tip:

- Adapter `llvmpipe` / `Cpu` means CPU fallback (no physical GPU exposed).
- Use `scripts/gpu-preflight.sh nvidia` when you need explicit NVIDIA passthrough in Docker.

## Software adapter (machines without a hardware GPU)

Development machines without a suitable hardware GPU can still run the real
WGSL compute path through a software rasterizer:

- Linux: Mesa Lavapipe / LLVMpipe (Vulkan software rasterizer),
- Windows: D3D12 WARP (software rasterizer).

Request the software fallback adapter explicitly:

```bash
# One-shot CLI flag (works for gpu-preflight and the smoke test below)
cargo run --bin gpu-preflight -- --software

# Or persistent environment toggle (honored by GpuContext and preflight)
FLEXPART_GPU_SOFTWARE=1 cargo run --bin gpu-preflight
WGPU_FORCE_FALLBACK_ADAPTER=1 cargo run --bin gpu-preflight
```

The fallback adapter executes the same WGSL shaders as hardware. No separate
CPU replacement path is used. Wall-clock timings measured on a software
adapter must never be reported as GPU performance values.

Known limitation: the Windows software rasterizer (WARP) can crash with
`STATUS_ACCESS_VIOLATION` when many GPU test binaries run back to back in one
`cargo test` invocation. Running targets serially is stable:

```bash
FLEXPART_GPU_SOFTWARE=1 cargo test --test integration -- --test-threads=1
```

End-to-end infrastructure smoke test through the WGSL advection kernel:

```bash
# 4096 particles, uniform +10 m/s wind, 3600 s at dt=60 s.
# Analytical expectation: 36 km eastward displacement, no N/S or vertical drift.
FLEXPART_GPU_SOFTWARE=1 cargo test --test integration software_advection
```

## Running Tests

```bash
# All tests
cargo test

# Only integration tests
cargo test --test forward_timeloop
cargo test --test backward_timeloop
cargo test --test cpu_gpu_comparison

# Physics validation gate (fast, used in CI)
cargo test physics_validation_advection_turbulence_pbl

# Mass conservation
cargo test --test integration mass_conservation
```

### Test structure

```
tests/
├── forward_timeloop.rs             # Forward time-loop end-to-end
├── backward_timeloop.rs            # Backward mode
├── cpu_gpu_comparison.rs           # CPU/GPU parity across all kernels
├── convection_chain.rs             # Convective mixing chain
├── validation_etex.rs              # ETEX scaffold validation
└── integration/
    ├── mass_conservation.rs        # Mass budget (particles + deposits = initial)
    ├── physics_validation.rs       # CI gate: advection, PBL, dispersion
    ├── reference_environment.rs    # Oracle pin, ETEX provenance, fail-closed guard
    ├── scientific_invariants.rs    # Positivity, determinism
    ├── deposition_decay.rs         # Exponential decay verification
    ├── source_receptor_consistency.rs  # Forward/backward symmetry
    └── software_advection.rs       # SW-WGPU-ADVECTION-001 infrastructure smoke test
```

### What the CI gate checks

The `physics_validation_advection_turbulence_pbl` test runs a 1-hour
simulation (500 particles, 12 steps, dt=300 s) and verifies:

- Advection direction matches prescribed wind.
- Particles confined within [0, BLH].
- Vertical mixing produces non-zero spread.
- Mass is conserved.

Execution time: ~1.2 s.

## Running Benchmarks

See [benchmarks.md](benchmarks.md) for the full methodology. Quick start:

```bash
# GPU fire-and-forget (1M particles)
FLEXPART_BENCH_MAX_PARTICLES=1000000 \
  cargo bench --bench advection -- 'pipeline_end_to_end_timeloop/forward_step'

# CPU baseline
FLEXPART_BENCH_MAX_PARTICLES=1000000 \
  cargo bench --bench advection -- 'pipeline_end_to_end_timeloop_cpu'

# Per-kernel breakdown
FLEXPART_BENCH_MAX_PARTICLES=1000000 \
  cargo bench --bench advection -- 'pipeline_stage_'
```

## Fortran Comparison

The Fortran oracle environment lives in this repository
(`docker/Dockerfile.fortran`, `docker/docker-compose.fortran.yml`); only the
pinned upstream sources are a sibling checkout (`../flexpart`, see
[reference-environment.md](reference-environment.md)).

```bash
# 1) Verify the oracle pin, build the image, compile FLEXPART
cargo run --bin reference-check -- verify --checkout ../flexpart
scripts/compare-fortran.sh compose setup

# 2) Run Fortran (from flexpart-gpu/)
docker compose -f docker/docker-compose.fortran.yml run --rm flexpart-fortran bash -lc \
  'cd /workspace/comparison/validate_run && /workspace/flexpart/src/FLEXPART'

# 3) Run GPU (from flexpart-gpu/)
OUTPUT_PATH=target/validation/gpu_concentration.json \
  PARTICLES=1000000 SYNC_READBACK=1 \
  cargo run --release --bin fortran-validation

# 4) Compare (from flexpart-gpu/)
docker compose -f docker/docker-compose.fortran.yml run --rm flexpart-fortran python3 \
  /workspace/flexpart-gpu/scripts/compare_concentrations.py \
  --fortran-output /workspace/comparison/validate_run/output \
  --gpu-output /workspace/flexpart-gpu/target/validation/gpu_concentration.json \
  --verbose
```

See [benchmarks.md](benchmarks.md) for detailed methodology and reports.

## ETEX Real-Data Pipeline

See [quickstart.md](quickstart.md) for the step-by-step guide. Short version:

```bash
scripts/run-etex.sh status    # check prerequisites
scripts/run-etex.sh all       # GPU-only pipeline
scripts/run-etex.sh all-with-fortran  # optional Fortran comparison
```

## Available Binaries

| Binary | Purpose |
|--------|---------|
| `flexpart-gpu` | Default binary (main.rs) |
| `gpu-preflight` | GPU backend detection and smoke test |
| `fortran-validation` | Synthetic scenario for Fortran comparison |
| `etex-run` | Real ETEX-1 simulation driver |
| `etex-validation` | ETEX validation harness |
| `bench-timeloop` | Standalone timeloop benchmark |

## Available Scripts

| Script | Purpose |
|--------|---------|
| `scripts/run-etex.sh` | ETEX pipeline (GPU-only by default, optional Fortran step) |
| `scripts/compare-fortran.sh` | GPU vs Fortran synthetic comparison |
| `scripts/gpu-preflight.sh` | GPU backend check (Docker wrapper) |
| `scripts/validate-etex.sh` | ETEX validation runner |
| `scripts/etex/*.py` | ERA5 download, met preparation, obs parsing, comparison |
| `scripts/compare_concentrations.py` | Concentration field comparison |
| `scripts/generate_synthetic_grib.py` | Synthetic GRIB generation for testing |

## Code Conventions

See [AGENTS.md](../AGENTS.md) for the full coding guidelines. Key points:

- All code, comments, and documentation in **English**.
- Every public function gets a `///` doc-comment.
- Reference the Fortran source file and line range when porting a routine.
- Cite the scientific reference for non-trivial formulas.
- Every GPU kernel has a CPU-side reference implementation.
- Physics-affecting changes must add an entry to
  [scientific-changelog.md](scientific-changelog.md).

## Environment Variables

### Runtime

| Variable | Default | Description |
|----------|---------|-------------|
| `WGPU_BACKEND` | `vulkan` | GPU backend (`vulkan`, `metal`, `gl`) |
| `FLEXPART_GPU_SOFTWARE` | unset (hardware) | Set to `1`/`true` to request the software fallback adapter (real WGSL path on Lavapipe/WARP) |
| `WGPU_FORCE_FALLBACK_ADAPTER` | unset (hardware) | Alias for `FLEXPART_GPU_SOFTWARE` |
| `RUST_LOG` | — | Logging level (`info`, `debug`, `trace`) |

### Benchmarks

See [benchmarks.md](benchmarks.md) for the full list. Key ones:

| Variable | Default | Description |
|----------|---------|-------------|
| `FLEXPART_BENCH_MAX_PARTICLES` | `1000000` | Max particles per scenario |
| `FLEXPART_BENCH_SAMPLE_SIZE` | `10` | Criterion sample count |
| `FLEXPART_BENCH_TIMELOOP_SYNC_HOST` | `0` | Enable host readback per step |
