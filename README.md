# FLEXPART-GPU

`flexpart-gpu` is a standalone Rust/WebGPU reimplementation of FLEXPART.
The current development focus is to close the scientific and behavioral gap to
FLEXPART 11.1 through reproducible, oracle-backed validation while retaining a
GPU-oriented execution architecture.

The project is independent and unofficial. It is not affiliated with or endorsed
by the official FLEXPART development team.

## Project status

This repository is experimental and under active development.

Substantial model, meteorology, GPU, validation, and benchmarking infrastructure
already exists, but `flexpart-gpu` must **not** currently be treated as a
scientifically interchangeable replacement for FLEXPART 11.1.

Scientific or behavioral parity is considered established only where the relevant
production path has passed an explicitly defined comparison against the pinned
FLEXPART 11.1 reference environment. Isolated kernel agreement, successful
execution, or good benchmark performance is not sufficient evidence by itself.

## Goals

The project is organized around five main goals:

- reproduce FLEXPART 11.1 behavior with explicitly defined scientific contracts;
- keep FLEXPART 11.1 available as a pinned, reproducible reference oracle;
- execute the candidate model as a standalone Rust application with GPU compute
  implemented through `wgpu` and WGSL;
- make validation auditable through versioned inputs, provenance, raw outputs,
  metrics, thresholds, and fail-closed CI gates;
- preserve the option for high-throughput operational workloads once scientific
  correctness has been demonstrated.

Performance matters, but correctness and reproducibility take precedence over
speedups.

## Validation model

The preferred validation path is a paired comparison:

```text
canonical scenario / meteorology
              |
              +------------------------+
              |                        |
              v                        v
   pinned FLEXPART 11.1         flexpart-gpu
        reference                 candidate
              |                        |
              v                        v
        raw outputs                raw outputs
              |                        |
              +-----------+------------+
                          |
                          v
              normalized comparison
                          |
                          v
              metrics / thresholds /
              provenance / CI gates
```

Validation workflows are designed to fail closed: missing oracle runs, stale or
incomplete artifacts, decoder failures, absent GPU execution where required, or
missing comparison metrics must not be reported as successful paired validation.

See:

- [Reference environment](docs/reference-environment.md)
- [Evaluation and comparison model](docs/evaluation.md)
- [CI gates](docs/ci-gates.md)
- [Scientific changelog](docs/scientific-changelog.md)

## Quick start

The current end-to-end example uses the ETEX scenario and can run the candidate
and pinned FLEXPART 11.1 reference from the same prepared meteorological inputs.

Check what is available locally:

```bash
scripts/run-etex.sh status
```

Run the paired workflow:

```bash
scripts/run-etex.sh all
```

For prerequisites, GPU modes, generated artifacts, and step-by-step execution,
see [docs/quickstart.md](docs/quickstart.md).

## Architecture

`flexpart-gpu` is a standalone Rust application rather than an FFI layer around
the Fortran implementation.

At a high level:

- Rust owns configuration, meteorological I/O, releases, orchestration, and
  validation-facing host logic;
- `wgpu` manages GPU resources and dispatch;
- WGSL kernels implement the GPU physics path;
- CPU-side implementations and analytical cases provide lower-level engineering
  checks;
- FLEXPART 11.1 remains the normative external reference where parity with
  upstream behavior is claimed.

The detailed source tree and execution flow are documented in
[docs/architecture.md](docs/architecture.md).

## Documentation

The full documentation index is available at [docs/README.md](docs/README.md).

Useful entry points:

- [Quickstart](docs/quickstart.md)
- [Architecture and source tree](docs/architecture.md)
- [Development guide](docs/development.md)
- [Scientific foundations](docs/science/README.md)
- [Meteorology contract](docs/meteorology-contract.md)
- [Validation report](docs/validation-report.md)
- [Benchmark methodology](docs/benchmarks.md)

Contributors and coding agents should also read [AGENTS.md](AGENTS.md), especially
the rules for issue scope, proof obligations, production-path validation, and
fail-closed behavior.

## Upstream relationship and attribution

This project reimplements and, in places, derives algorithmic structure from
FLEXPART. Primary credit for the underlying model, scientific methods, and
historical validation belongs to the FLEXPART developers and scientific
community.

Upstream references:

- FLEXPART home page: <https://www.flexpart.eu/>
- FLEXPART v11 repository: <https://gitlab.phaidra.org/flexpart/flexpart>
- Bakels et al. (2024), *Geoscientific Model Development* 17, 7595-7624:
  <https://doi.org/10.5194/gmd-17-7595-2024>

See [NOTICE.md](NOTICE.md) for attribution and distribution notes.

## License

`flexpart-gpu` is published under `GPL-3.0-or-later`, consistent with the
upstream licensing obligations applicable to this work.

See [LICENSE](LICENSE) and [NOTICE.md](NOTICE.md) for details.

## Maintainer

Andre Schmitz  
Email: `andre_schmitz@web.de`
