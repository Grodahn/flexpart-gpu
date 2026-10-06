# FLEXPART-GPU

`flexpart-gpu` is an open-source Rust/`wgpu` reimplementation of FLEXPART,
aiming to make scientifically validated Lagrangian atmospheric dispersion
available through a portable, GPU-first architecture across modern graphics and
compute backends.

The project currently targets behavioral and scientific compatibility with
FLEXPART 11.1 through reproducible, oracle-backed validation. Its longer-term
goal is to provide the scientific and open-source community with a transparent,
high-throughput GPU implementation that is not tied to a single GPU vendor.

Alongside the scientific goal, the repository also serves as a practical case
study in agent-assisted scientific software engineering: complex implementation
work is decomposed into explicit issue contracts, independently reviewed, and
accepted only through reproducible tests, provenance, and scientific validation.
AI-assisted development is treated as an engineering accelerator, not as a
substitute for scientific evidence.

The project is independent and unofficial. It is not affiliated with or endorsed
by the official FLEXPART development team.

## Project status

This repository is experimental and under active development. It already contains
a substantial GPU execution path, reproducible FLEXPART 11.1 oracle infrastructure,
versioned validation/provenance contracts, real-data ETEX tooling, and replicated
GPU/Fortran benchmark campaigns.

It must **not** currently be treated as a scientifically interchangeable
replacement for FLEXPART 11.1.

| Area | Current state |
|------|---------------|
| Reference & validation foundations | Pinned FLEXPART 11.1 oracle, stochastic identity strategy, versioned case contract, input-equivalence gate, and run provenance are established. |
| Portable GPU foundation | Shared `wgpu`/WGSL execution contract and GPU meteorology interpolation are established; production-consumer migration is ongoing. |
| Transport physics | GPU transport, turbulence, deposition, decay, compaction, and gridding paths exist; process-specific FLEXPART parity and production closure remain in progress. |
| Operational inputs & outputs | ETEX and canonical meteorology infrastructure exist; operational decoding, release/source-term closure, and final scientific output contracts remain active work. |
| Scientific release validation | Final ETEX-I and radionuclide validation are not yet complete. |
| Production-scale performance | Replicated 1M/10M benchmarks exist; the canonical Europe-scale workload, VRAM-budgeted cohort execution, and final hardware envelope are still ahead. |

Scientific or behavioral parity is considered established only where the relevant
production path has passed an explicitly defined comparison against the pinned
FLEXPART 11.1 reference environment. Isolated kernel agreement, successful
execution, or good benchmark performance is not sufficient evidence by itself.

See [ROADMAP.md](ROADMAP.md) for the current path to scientific and operational
production readiness.

## Goals

The project is organized around six main goals:

- reproduce FLEXPART 11.1 behavior with explicitly defined scientific contracts;
- keep FLEXPART 11.1 available as a pinned, reproducible reference oracle;
- execute the candidate model as a standalone Rust application with GPU compute
  implemented through `wgpu` and WGSL;
- remain portable across GPU vendors and supported `wgpu` backends instead of
  coupling the scientific implementation to one proprietary compute stack;
- make validation auditable through versioned inputs, provenance, raw outputs,
  metrics, thresholds, and fail-closed CI gates;
- explore whether agent-assisted development can accelerate complex open
  scientific software without weakening reviewability, reproducibility, or
  scientific standards.

Performance matters, but correctness and reproducibility take precedence over
speedups.

## Why this project

FLEXPART is a mature and widely used scientific model. This project explores
whether its forward Lagrangian dispersion workflow can be reimplemented as an
open, portable GPU application while retaining an explicit scientific validation
chain back to FLEXPART 11.1.

The intended benefit is twofold:

- **scientific computing:** make large particle populations and repeated
  atmospheric-dispersion workloads practical on commodity and accelerator GPUs
  across vendors;
- **open engineering research:** document how agent-assisted development can be
  used for a validation-heavy scientific codebase without relaxing provenance,
  review, testing, or reproducibility requirements.

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

## Roadmap

Development is organized as explicit, reviewable contracts rather than one broad
"port FLEXPART" task. The current high-level sequence is:

1. finish canonical GPU meteorology integration and remove transitional paths;
2. close release/source-term, particle-identity, mass-ledger, and scientific
   output contracts;
3. complete process-level FLEXPART 11.1 parity for transport and deposition;
4. run pre-registered ETEX-I and radionuclide release validation;
5. characterize the real production workload and scale beyond single-GPU VRAM
   with deterministic particle cohorts.

The detailed milestone map and issue references live in
[ROADMAP.md](ROADMAP.md).

## Contributing

Contributions are welcome from atmospheric-science, scientific-computing,
Rust/`wgpu`, GPU/HPC, validation, reproducibility, and technical-writing
backgrounds.

The repository is deliberately issue-driven: a contribution should have a
bounded contract, explicit proof obligations, and a reproducible validation path
before implementation begins. AI-assisted contributions are welcome, but they
are held to the same review and scientific-evidence requirements as any other
change.

Start with [CONTRIBUTING.md](CONTRIBUTING.md), then read [AGENTS.md](AGENTS.md)
for the detailed issue, review, validation, and fail-closed rules.

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
