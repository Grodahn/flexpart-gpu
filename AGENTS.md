# AGENTS.md — Coding Guidelines for FLEXPART-GPU

## Project Overview

FLEXPART-GPU is a standalone Rust/WebGPU reimplementation of the FLEXPART Lagrangian
particle dispersion model. The current development priority is to close the scientific and
behavioral gap to a pinned FLEXPART 11.1 reference through reproducible, oracle-backed
validation while retaining a GPU-oriented execution architecture.

Scientific correctness and reproducibility take precedence over performance. Do not claim
FLEXPART 11.1 parity, production readiness, or operational suitability from successful
execution, isolated kernel tests, or benchmarks alone. Such claims require the explicitly
defined production-path validation evidence and gates owned by the relevant issue.

The project is independent and unofficial and is not affiliated with or endorsed by the
official FLEXPART development team.

**Stack**: Rust + wgpu + WGSL shaders

**Maintainer**: Andre Schmitz (`andre_schmitz@web.de`)

---

## Code Quality Standards

### Readability First

- **Meaningful names**: variables, functions, and types must carry intent.
  `particle_position` not `pp`. `wind_field_u` not `wfu`.
- **Small functions**: each function does one thing. If it needs a comment
  explaining *what* it does, it should be split or renamed.
- **Flat over nested**: prefer early returns and guard clauses over deep nesting.
- **Constants over magic numbers**: all physical constants and tuning parameters
  must be named constants with units in the name or doc-comment.

### Language

- **All code comments, doc-comments, documentation files, and commit messages
  must be written in English.** No exceptions.

### Documentation

- Every public function, struct, and module gets a `///` doc-comment explaining
  **why** it exists and **what physical quantity** it represents when applicable.
- Reference the FLEXPART Fortran source file and line range when porting a routine
  (e.g. `/// Ported from advance.f90:120-185`).
- Cite the scientific reference for non-trivial formulas
  (e.g. `/// Hanna (1982), Eq. 4.12`).
- Internal implementation comments explain *why*, never *what*.

### Rust Conventions

- Use `rustfmt` defaults. No custom formatting rules.
- Use `clippy` with `#![warn(clippy::all, clippy::pedantic)]`.
- Prefer strong typing: newtypes for physical quantities when confusion is possible
  (e.g. `Meters(f32)` vs `Seconds(f32)`).
- Error handling: use `thiserror` for library errors, `anyhow` only in binaries/tests.
- No `unwrap()` in library code. Use `expect("reason")` only when the invariant
  is proven and documented.

### WGSL Shader Conventions

- One compute kernel per file in `src/shaders/`.
- Name the file after the physical process: `advection.wgsl`, `hanna_turbulence.wgsl`.
- Group bindings logically: group 0 = particles, group 1 = wind field, group 2 = parameters.
- Comment the physical equation being implemented at the top of each kernel.
- Use `f32` throughout (justified: Monte Carlo convergence dominates over float precision).

---

## Required Agent Workflows

Workflow-specific instructions live under `.agents/skills/` and are mandatory when applicable:

- For implementing an issue or scoped code change, invoke `$implementation`.
- For reviewing a pull request, including review-and-repair work, invoke `$code-review`.
- For creating, refining, splitting, or re-scoping GitHub issues, invoke `$issue-authoring`.
- A PR review that includes repairs uses `$code-review` alone unless a distinct implementation task is explicitly requested.
- Do not reopen, quote, or summarize this `AGENTS.md` when it has already been injected or provided by the harness.

## Agent Execution Efficiency

Minimize model/tool round trips and command output without weakening correctness or scientific verification.

- Batch independent repository and GitHub inspections where practical.
- Prefer targeted searches, bounded snippets, filenames, diff statistics, failing assertions, and the last relevant failure lines.
- Do not emit full issue bodies, full diffs, complete API responses, complete successful test logs, or complete CI logs unless they are specifically needed to diagnose a failure.
- Make related in-scope changes together rather than repeatedly alternating between inspection, editing, and broad verification.
- Run the narrowest tests that can falsify the changed behavior first. Run broader required verification once after the implementation is stable.
- Do not rerun a passing check unless relevant code changed afterward.
- On failure, inspect only the relevant failing test, step, or log section before widening the investigation.
- Distinguish failures caused by the requested change from established unrelated baseline or infrastructure failures.
- When CI confirmation is part of the task, poll no more frequently than once per 60 seconds and request concise status fields.
- Normally push once after local verification passes. Repush only when a subsequent failure is caused by the current change.
- Prefer one autonomous agent run for one bounded task. Do not create extra agents or approval pauses unless the task requires them.
- Do not optimize against an arbitrary maximum number of tool calls; minimize redundant calls while preserving correctness and required evidence.

---

## Testing Requirements

### What Must Be Tested

1. **Physics kernels** (CPU reference): every GPU kernel has a CPU-side reference
   implementation tested against known analytical solutions or FLEXPART Fortran output.
2. **Interpolation**: trilinear wind interpolation tested against hand-computed values
   on a small synthetic grid.
3. **Turbulence (Hanna)**: verify sigma_u, sigma_v, sigma_w, TL against published
   tables for stable/neutral/unstable conditions.
4. **Particle conservation**: total particle mass must be conserved (within f32 tolerance)
   when deposition is disabled.
5. **GPU vs CPU parity**: after each kernel port, run both paths on the same input
   and assert max relative error < 1e-4.

### Test Organization

```
tests/
├── unit/              # Pure function tests (interpolation, Hanna, RNG)
├── integration/       # Multi-step tests (advection + turbulence pipeline)
└── validation/        # Comparison against FLEXPART Fortran reference output

benches/               # Performance benchmarks (criterion) — at crate root, not under tests/
└── advection.rs
```

### Test Naming

- `test_{module}_{scenario}_{expected}` e.g. `test_hanna_stable_sigma_w_matches_table`
- Benchmark names: `bench_{kernel}_{particle_count}` e.g. `bench_advection_1M`

---

## Git & Collaboration

- **Commits**: imperative mood, concise subject line. Body references the Fortran
  source or scientific paper when relevant.
- **Commit language policy**: commit subject and body must be written in English.
  If a non-English commit message is discovered in local history, rewrite it to
  English before sharing the branch.
- **Branches**: `feat/`, `fix/`, `refactor/`, `bench/`, `docs/` prefixes.
- **PR descriptions**: state what Fortran routine is being ported and how validation
  was performed.

---

## Multi-Agent Workflow

### Model requirement

All sub-agents **must** use the **most capable model available**. This project
involves scientific computing (atmospheric physics, GPU kernel programming,
Fortran→Rust translation) that requires strong reasoning capabilities. Lighter
or faster models may only be used for trivial tasks (formatting, file moves,
renaming).

### Per-task protocol

1. Use the mandatory workflow skill named in **Required Agent Workflows** when applicable.
2. For benchmarking/performance tasks, read `docs/benchmarks.md` first and follow
   its methodology (scenario sizing, warm-up/sample settings, and GPU/CPU recipe separation).
3. Read the referenced Fortran source when needed to understand the algorithm being ported.
4. Write tests before or alongside the implementation (not after).
5. Once the implementation is stable, run the required final `cargo clippy` and `cargo test`
   verification as described by the applicable workflow skill.
6. Document any deviation from the Fortran reference in `docs/scientific-changelog.md`.
