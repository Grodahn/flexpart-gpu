---
name: issue-authoring
description: Create, refine, split, or re-scope FLEXPART-GPU GitHub issues, including acceptance criteria, dependencies, proof obligations, validation levels, and follow-up issue boundaries.
---

# Issue Authoring and Task Slicing

Use this skill when the task is to create or refine issue contracts. Do not load it merely to implement or review an already-defined issue.

## Issue Definition & Task Slicing

When creating or refining GitHub issues, optimize for **small, atomic, independently\nverifiable claims**, not for broad feature descriptions. An issue should ideally prove one thing.

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

1. **One issue owns one semantically coherent contract.**
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
