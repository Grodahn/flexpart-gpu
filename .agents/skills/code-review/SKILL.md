---
name: code-review
description: Review a FLEXPART-GPU PR against its current contract using bounded inspection and targeted-first validation; perform in-scope repairs and publish them when authorized by the request.
---

# Code Review and Repair Workflow

Use for PR reviews and authorized review-and-repair work. Apply current
`AGENTS.md` (injected content counts) and
[the shared verification policy](../../../docs/agent-validation.md#agent-workflow-policy).
A review-only request authorizes inspection and findings. When repairs and
publishing are authorized, complete them in one autonomous workflow without
repeated approval pauses. Use this skill alone for review with repairs unless
a distinct implementation task is requested.

## Scope and evidence

- The issue plus the explicit user request define the review contract. Apply
  **Issue Definition & Task Slicing** from current `AGENTS.md`. Keep adjacent
  behavior and unrelated baseline/CI failures outside the change.
- For GPU review or repair, read `docs/GPU_CONTRACT.md` first. Require the
  production GPU path, no silent CPU fallback, pinned-oracle authority, and
  actual device-execution evidence at the issue's required validation level.
- If correctness needs unresolved scientific semantics, an unnamed consumer,
  external data/oracle, or another issue's contract, stop expanding scope and
  record the dependency. Do not invent physics or new tolerances during review.
- Preserve unrelated user changes and use a single agent unless delegation is
  explicitly requested or required by applicable instructions.
- Batch PR/issue metadata, dependencies, changed filenames, review state, and
  concise check status. Inspect relevant patches, tests, callers, and normative
  sources fully enough for correctness; bound displayed output, not review depth.

## Review, repair, and verification

Compare the complete in-scope diff and its evidence with the contract. Report
confirmed correctness, scope, validation, or maintainability findings. Verify
suspected problems before editing; avoid speculative churn. With repair
authorization, fix confirmed in-scope findings together before the initial push.

Run directly affected tests first. For supported focused corpus checks, default
to `python scripts/agent_validation.py --check comparison --case <CASE>`;
oracle-only checks use `--check oracle`. Follow the shared policy for supported
selectors, valid caches, compact JSON, artifact paths, and failed-stage tails.
Never equate a diagnostic execution `PASS` with scientific parity, or a
candidate-only/zero-test/skipped run with required validation.

Once repairs are stable, run formatting, Clippy, Cargo tests, and issue-owned
production/device/oracle/CI gates as required by the shared policy. Apply its
strict instruction/documentation-only exception when appropriate. Review-only
work may reuse valid evidence for the reviewed head and add checks necessary
to resolve findings; it need not repeat every valid passing check.
Retain full logs, preserve exit codes, and report compact outcomes. Re-run
checks only when relevant changes or invalidated evidence require them.

## Publish and finish

For authorized repairs, normally push once after stable local verification.
Update the existing PR description to match the completed scope and validation.
Use an explicit lease for an authorized rebase. Poll concise CI status at least
60 seconds apart, verify the pushed head, and inspect only relevant failed-step
logs before widening investigation. Repush for in-scope repairs, required base
updates, or user requests. Document demonstrated unrelated failures without
changing adjacent code or describing the failed gate as green.

Finish with findings/fixes, verification and evidence paths, the PR link, and
remaining blockers/dependencies. State omitted or unavailable checks honestly.
Do not report completion of a scientific claim without its required evidence.
