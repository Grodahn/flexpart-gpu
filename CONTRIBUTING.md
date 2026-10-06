# Contributing to flexpart-gpu

Thank you for considering a contribution.

`flexpart-gpu` sits at the intersection of atmospheric science, scientific
software validation, Rust, portable GPU compute, and high-performance particle
simulation. Contributions do not need to span all of those areas: focused work
with a clear verification boundary is preferred.

## Where contributions help

Useful contribution areas include:

- **Atmospheric science / FLEXPART:** physics cross-checks, oracle fixtures,
  process-level validation, ETEX and radionuclide validation.
- **Rust / wgpu / WGSL:** GPU execution, resource management, shader structure,
  cross-backend behavior, performance instrumentation.
- **HPC / numerical computing:** memory layout, batching/cohort execution,
  profiling, scaling, determinism, numerical robustness.
- **Validation / reproducibility:** manifests, provenance, metrics, CI evidence,
  fail-closed tooling.
- **Documentation / developer experience:** reproducible setup, architecture
  documentation, examples, benchmark reporting.

## Before starting work

1. Read [README.md](README.md) for project scope and current status.
2. Read [ROADMAP.md](ROADMAP.md) to understand the production-readiness path.
3. Read [docs/development.md](docs/development.md) for build/test setup.
4. Read [AGENTS.md](AGENTS.md). Despite the name, it is also the repository's
   detailed engineering contract for human contributors: issue slicing, proof
   obligations, production-path evidence, review scope, and fail-closed behavior.
5. Work from an existing issue where possible. If the required scientific or
   architectural contract is missing, open a focused issue before implementing
   a broad solution.

## Issue-first development

Issues are intended to be executable engineering/scientific contracts, not just
task titles.

A good implementation issue should make clear:

- the single claim being implemented or tested;
- its dependencies and non-scope;
- which production path is affected;
- how success is demonstrated;
- which artifacts or measurements are required;
- what must fail closed rather than silently fall back.

If implementation uncovers an independent scientific or architectural claim,
split it into a separate issue instead of silently expanding scope.

## Pull requests

A pull request should:

- reference the issue it implements;
- stay within that issue's contract;
- explain the implementation and any scientific/numerical choices;
- include the required tests and evidence;
- identify the exact GPU/backend/hardware context for performance claims;
- preserve or extend provenance where scientific outputs change;
- update documentation and the scientific changelog where required.

Performance improvements must be measured against an appropriate baseline.
Scientific claims must be validated against the repository's declared oracle or
analytic contract. A faster result is not accepted if it weakens scientific
semantics.

## AI-assisted contributions

AI-assisted development is welcome and is part of the project's engineering
research question.

The standard is intentionally the same regardless of who or what produced the
patch:

- the issue is the implementation contract;
- generated code is not trusted without review;
- scientific behavior must be supported by reproducible evidence;
- tests must exercise the actual production path where the issue requires it;
- missing evidence must not be converted into a successful claim.

Please disclose material AI assistance in the PR description when it helps
reviewers understand how the change was produced or reviewed.

## External research and attribution

Published algorithms, architecture ideas, and performance techniques should be
credited where they materially influence an issue or implementation.

Prefer attribution in the issue and PR description, with source-code comments
only when an external design rationale is necessary to understand a non-obvious
implementation choice.

Do not imply source-code provenance when only a published idea or measurement was
used as prior art.

See [NOTICE.md](NOTICE.md) for upstream FLEXPART attribution and distribution
notes.

## Running tests

See [docs/development.md](docs/development.md) for the complete development and
test workflow.

At minimum, run the focused tests required by the issue plus the repository
formatting/linting gates described in [AGENTS.md](AGENTS.md). GPU performance
claims require real hardware; software adapters are valid for WGSL execution
checks but not for performance evidence.

## Review philosophy

Reviews should distinguish between:

- correctness blockers;
- scientific/validation blockers;
- performance observations;
- maintainability improvements;
- unrelated follow-up work.

Do not broaden a PR merely because adjacent cleanup is visible. Record unrelated
work as a follow-up issue.

## Questions and proposed work

If you are interested in contributing but unsure where to start, open or comment
on an issue describing the area you want to work on. Small, independently
verifiable contributions are preferred over large speculative rewrites.
