# AGENTS.md — Coding Guidelines for FLEXPART-GPU

## Project Overview

FLEXPART-GPU is a Rust/WebGPU port of the FLEXPART Lagrangian particle dispersion model's
compute kernels. The goal is real-time atmospheric dispersion simulation on commodity GPUs
for emergency response (industrial accidents, Seveso sites).

**Stack**: Rust + wgpu + WGSL shaders

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

## Issue Definition & Task Slicing

When creating, refining, or implementing GitHub issues, optimize for **small, atomic,
independently verifiable claims**, not for broad feature descriptions. An issue should
ideally prove one thing.

### Split aggressively at verification boundaries

Do **not** combine multiple independent claims such as:

- build/reproducibility infrastructure;
- input-equivalence or unit-conversion logic;
- raw-output capture/decoding;
- comparison semantics;
- provenance/manifests;
- CI orchestration;
- scientific parity of a physical process.

If each claim can fail independently, prefer separate issues with explicit dependencies.
A large child issue that contains several such tracks should be treated as a sub-epic,
not as one implementation task.

### Every acceptance criterion needs a proof obligation

For each acceptance criterion, define how completion is demonstrated. Prefer an explicit
mapping of:

`requirement -> implementation surface -> test/check -> required artifact/result`

Avoid vague criteria such as "reproducible", "equivalent", "validated", "works in CI",
or "parity achieved" unless the issue also states exactly what evidence makes that claim true.

Examples:

- "Equivalent inputs" must identify the canonical source of truth, required unit conversions,
  normalized fields to compare, and a fail-closed equality/audit check.
- "Reproducible oracle build" must state pinned revisions/environment, required hashes and
  the postcondition that the oracle checkout remains clean after build/run.
- "Workflow succeeds" must state which missing/stale artifacts or failed subprocesses make
  the workflow exit non-zero.
- "Scientific parity" must name the production path, cases/ensembles, metrics, thresholds,
  uncertainty treatment, and raw evidence required for the verdict.

### Test the production path when the claim is about production behavior

An isolated kernel/helper test is not evidence for end-to-end production-path behavior.
Issues and PRs must state whether evidence is:

- analytical/unit-level;
- isolated kernel-level;
- production-path integration;
- paired FLEXPART-11.1 oracle validation;
- observational validation.

Do not substitute a lower validation level for a higher one unless the issue explicitly
allows it.

### Fail closed

Validation, CI, comparison, and provenance workflows must never turn missing prerequisites,
skipped execution, absent adapters, absent oracle outputs, stale artifacts, decoder failures,
or incomplete metrics into a successful result. Candidate-only execution may exist as an
explicit mode, but it must not be reported as paired validation.

### Define artifact and provenance contracts explicitly

If an issue depends on generated files or manifests, enumerate the required consumed inputs
and produced outputs. For scientific comparisons, this normally includes the case/config
source, derived oracle/candidate inputs, meteorology, executable/revision identity, seeds,
adapter provenance, raw outputs, decoded outputs, comparison report, and hashes where
reproducibility requires them.

### Keep PRs aligned with issue boundaries

A PR should normally satisfy one atomic issue or one clearly stated slice of a sub-epic.
Do not claim completion of adjacent scientific or infrastructure issues merely because the
same branch contains partial work for them. If review reveals a separate independently
verifiable problem, prefer a follow-up issue/PR rather than silently expanding scope.

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

1. Read this file before starting any task.
2. When creating or refining issues, follow **Issue Definition & Task Slicing** above before implementation starts.
3. For benchmarking/performance tasks, read `docs/benchmarks.md` first and follow
   its methodology (scenario sizing, warm-up/sample settings, and GPU/CPU recipe separation).
4. Read the referenced Fortran source to understand the algorithm being ported.
5. Write tests before or alongside the implementation (not after).
6. Run `cargo clippy` and `cargo test` before marking a task as done.
7. Document any deviation from the Fortran reference in `docs/scientific-changelog.md`.
