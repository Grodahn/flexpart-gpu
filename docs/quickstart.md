# Quickstart ETEX (first simulation)

This guide shows the simplest way to run an ETEX-style simulation
with `flexpart-gpu`, without modifying the engine source code.

The paired ETEX workflow runs both `flexpart-gpu` and the pinned FLEXPART
11.1 oracle. It reports diagnostics against station observations and does
not assert scientific parity.

## 1) Prerequisites

- Rust toolchain (`cargo`)
- Python 3 with `numpy`, `eccodes`, `xarray`, `gcsfs`, and `zarr`
- Docker + Docker Compose
- the unmodified FLEXPART 11.1 checkout at `../flexpart` (see
  [reference-environment.md](reference-environment.md))

Optional (for GPU execution inside Docker):

- `docker/docker-compose.yml` (default GPU container)
- `docker/docker-compose.nvidia.yml` (NVIDIA overlay)

ERA5 is downloaded from the public ARCO-ERA5 dataset. No CDS token is used.

## 2) Check pipeline status

From the `flexpart-gpu` root:

```bash
scripts/run-etex.sh status
```

This command reports what is missing (ETEX data, ERA5, outputs already
produced, etc.).

## 3) Run the paired pipeline

```bash
scripts/run-etex.sh all
```

The script chains:

1. parsing ETEX measurements,
2. downloading ERA5,
3. preparing both model inputs from the same ERA5 arrays,
4. running the pinned FLEXPART 11.1 oracle,
5. running the WGSL candidate,
6. comparing paired three-hour windows with observations.

## 4) Run and debug step by step

If `all` fails, run individual steps:

```bash
scripts/run-etex.sh parse
scripts/run-etex.sh download
scripts/run-etex.sh prepare
scripts/run-etex.sh fortran
scripts/run-etex.sh gpu
scripts/run-etex.sh compare
scripts/run-etex.sh report
```

`all-with-fortran` is an alias for `all`.

## 5) Scenario configuration files

The reference ETEX scenario is located in:

- `fixtures/etex/real/config/COMMAND`
- `fixtures/etex/real/config/RELEASES`
- `fixtures/etex/real/config/OUTGRID`
- `fixtures/etex/real/config/SPECIES/*`

To create a new simulation, duplicate this folder, edit these files,
and adjust the script (or variables) to point to your new `config`.

## 6) Produced artifacts

The pipeline writes mainly to `target/etex/`, including:

- `gpu_output.json`
- `comparison_report.json`
- `gpu.log`

When Fortran comparison is enabled, it also writes:

- `fortran.log`

## 7) Practical notes

- You do not need to write a new C++/Rust program for each case:
  standard usage relies on config files + a launcher script.
- If you change the physics (kernels, timeloop), then yes, recompilation
  is required.
- If CDS credentials are missing, the pipeline may stop at the ERA5
  download step.

## 8) Optional Docker usage (GPU path)

From the `flexpart-gpu` root:

```bash
# Default containerized GPU run
docker compose -f docker/docker-compose.yml run --rm flexpart-gpu bash

# NVIDIA containerized GPU run
docker compose -f docker/docker-compose.yml -f docker/docker-compose.nvidia.yml \
  run --rm flexpart-gpu bash
```

Helper wrappers (recommended):

```bash
scripts/gpu-preflight.sh compose
scripts/gpu-preflight.sh nvidia
```

## 9) GPU mode decision guide

Use this quick decision path:

1. Try local first (simplest path):

```bash
scripts/gpu-preflight.sh local
```

2. If you want Docker without NVIDIA-specific overlay:

```bash
scripts/gpu-preflight.sh compose
```

3. If you need NVIDIA passthrough in Docker:

```bash
scripts/gpu-preflight.sh nvidia
```

How to interpret the result:

- If adapter shows your physical GPU (for example NVIDIA), you are using real GPU.
- If adapter shows `llvmpipe` / `Cpu`, you are running on CPU fallback.
- `compose` can use either real GPU or CPU fallback depending on host/container GPU exposure.
- `nvidia` is the preferred Docker mode when you want to force NVIDIA GPU access.

## 10) Further reading

- Benchmarks: `docs/benchmarks.md`
- Validation report: `docs/validation-report.md`
- Scientific validation: `docs/benchmarks/benchmark-scientific-validation.md`
