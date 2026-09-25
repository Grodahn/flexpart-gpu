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

## GPU Contract

For any task that implements, modifies, composes, or reviews GPU execution code, read and obey
`docs/GPU_CONTRACT.md` before making changes. That document is the normative repository-level
GPU architecture, memory/transfer, execution/error, numerical, and verification contract.

**Scientific production calculations are GPU-by-default.** Any issue that implements a
physics-relevant or numerical calculation used by the supported production path must execute
that calculation on the GPU and satisfy `docs/GPU_CONTRACT.md`, including proof of actual
device execution and the issue-owned authoritative oracle/validation evidence.

A CPU implementation does not satisfy such an issue merely because the issue omits the word
"GPU". CPU-only completion is permitted only when the issue explicitly defines the work as a
CPU reference implementation, oracle generation, validation tooling, research/decision work,
data/provider infrastructure, or another non-production calculation.

Silent CPU fallback must never turn an unsupported or failed GPU production path into successful
execution. Existing CPU implementations may remain temporarily for migration diagnostics where
explicitly useful, but they are not the normative production implementation unless the owning
issue explicitly says otherwise.

When an issue is ambiguous about whether a calculation belongs to the production GPU path,
resolve that ambiguity at the issue/architecture level before implementation rather than
defaulting to CPU.

Do not introduce ticket-local GPU runtime abstractions, hidden host/device transfers, silent CPU
fallbacks, or numerical policies that conflict with `docs/GPU_CONTRACT.md`. If a task requires a
GPU-contract change, stop that path and update the owning contract issue/document explicitly
rather than silently diverging.

The pinned FLEXPART oracle owned by the relevant issue is authoritative for scientific parity
when it provides adequate coverage. A separate CPU reference implementation is not a general
prerequisite for GPU work. Existing CPU implementations may be retained temporarily and compared
for migration diagnostics, but CPU/GPU agreement must not replace the required FLEXPART-oracle
proof unless the owning issue explicitly defines CPU parity as normative.

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
- Use the floating-point precision and numerical policy defined by `docs/GPU_CONTRACT.md`.

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

### Issue-authoring hard rules

The following rules are intended to prevent review-driven scope expansion and long
implementation/correction loops:

1. **One issue owns one fachlich coherent contract.**
   Define one behavioral or scientific contract that can be implemented and verified as
   a unit. If the issue needs several independently provable contracts, split it or make
   it a sub-epic with child issues.

2. **Separate contract definition, data migration, and consumer adoption.**
   Defining a schema/API, migrating existing fixtures/data, updating all consumers, and
   removing compatibility fallbacks are different verification boundaries. Do not bundle
   them into one implementation issue unless they are genuinely inseparable and the issue
   names the exact combined proof obligation.

3. **Do not use open-ended negative acceptance criteria.**
   Requirements such as "no hidden defaults", "all ambiguity removed", or "fully
   equivalent" are not finite unless the issue enumerates the concrete defaults,
   ambiguities, or fields in scope. Treat newly discovered semantics outside that list as
   follow-up work unless they invalidate the current issue's stated claim.

4. **Make normative references executable and unambiguous.**
   When parity with FLEXPART or another oracle is required, state exactly what counts as
   the normative reference: pinned revision, routine/binary/artifact, invocation path,
   inputs, and comparison. A reimplementation of the same equations is not an independent
   oracle unless the issue explicitly says that it is sufficient.

5. **Specify downstream handoff surfaces concretely.**
   Avoid wording such as "usable by #N" without defining what #N receives. Name the
   required API/artifact fields, ownership of interpolation/conversion/validation,
   mutability/lifetime expectations where relevant, and fail-closed behavior. Downstream
   code should not need to reconstruct semantics that this issue owns.

6. **Resolve scientific or architectural unknowns before implementation.**
   If a required conversion, reference behavior, data provenance, or oracle semantics are
   not independently known, create a prerequisite research/decision/oracle issue first.
   The implementation issue may explicitly fail closed on the unresolved path rather than
   inventing behavior during coding.

7. **Give implementation agents an explicit stop rule.**
   If implementation discovers that satisfying the issue would require a new
   physics-relevant semantic, a previously unnamed runtime consumer, a new external data
   source/reference, or ownership of another issue's contract, stop expanding the current
   scope. Record the dependency or create a follow-up issue. Expand the current issue only
   when the newly discovered work is necessary to make its original claim correct.

These rules favor **decision and proof boundaries** over line-count or component boundaries.
A small diff can still contain several independent claims; a larger diff can be acceptable
when it proves one tightly bounded contract.

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

1. **Physics/GPU calculations**: validate GPU results against the authoritative analytical or
   pinned FLEXPART oracle owned by the relevant issue. A separate CPU implementation is optional
   unless the issue explicitly requires it.
2. **Interpolation**: validate the supported GPU interpolation production path against the pinned
   FLEXPART interpolation oracle/fixtures and issue-specific boundary cases.
3. **Turbulence (Hanna)**: verify sigma_u, sigma_v, sigma_w, TL against published
   tables for stable/neutral/unstable conditions.
4. **Particle conservation**: total particle mass must be conserved within the tolerance defined
   by the owning issue/GPU contract when deposition is disabled.
5. **GPU execution proof**: GPU issues must prove both numerical correctness and actual device
   execution. CPU/GPU comparison may be used diagnostically during migration but is not a
   substitute for authoritative oracle validation.

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
2. For any GPU implementation, modification, composition, or review task, read and obey
   `docs/GPU_CONTRACT.md` before touching GPU code.
3. When creating or refining issues, follow **Issue Definition & Task Slicing** above before implementation starts.
4. For benchmarking/performance tasks, read `docs/benchmarks.md` first and follow
   its methodology (scenario sizing, warm-up/sample settings, and GPU/CPU recipe separation).
5. Read the referenced Fortran source to understand the algorithm being ported.
6. Write tests before or alongside the implementation (not after).
7. Run `cargo clippy` and `cargo test` before marking a task as done.
8. Document any deviation from the Fortran reference in `docs/scientific-changelog.md`.
