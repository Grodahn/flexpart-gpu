---
name: implementation
description: Implement a scoped FLEXPART-GPU issue or repository change with targeted-first verification, compact oracle checks, bounded output, and strict scientific scope control.
---

# Implementation Workflow

Use for scoped changes to code, tests, shaders, scripts, build inputs, or
repository instructions. Read current `AGENTS.md` once (injected content counts)
and apply [the shared verification policy](../../../docs/agent-validation.md#agent-workflow-policy).

## Contract and inspection

- The issue and explicit user request define scope. Follow the current
  **Issue Definition & Task Slicing** rules in `AGENTS.md`; do not implement
  adjacent behavior or fix unrelated baseline/CI failures.
- Before GPU implementation, modification, composition, or review, read
  `docs/GPU_CONTRACT.md`. GPU-by-default production calculations, no silent
  CPU fallback, pinned-oracle authority, and actual device-execution proof apply.
- Resolve unknown scientific or architectural semantics at the owning contract.
  Stop that path and record a dependency if it requires new physics semantics,
  an unnamed consumer, external data/oracle, or another issue's contract.
- Preserve unrelated user changes. Use one agent unless delegation is explicitly
  requested or required. Isolate work when needed to avoid another active task.
- Batch independent read-only inspections. Start with changed filenames, diff
  statistics, selected GitHub fields, targeted searches, and bounded snippets.
  Read complete applicable contracts when needed; do not truncate the review.

## Implementation and verification

Write or update directly affected tests alongside implementation. Batch related
in-scope edits before broad verification. Use the narrowest relevant test first.
For a supported focused corpus case, default to
`python scripts/agent_validation.py --check comparison --case <CASE>`;
use `--check oracle` only when no candidate comparison is needed. Consult the
shared policy for selector limits, retained artifacts, cache reuse, bounded
failure diagnostics, and the distinction between execution and scientific verdicts.

Once stable, run final formatting, Clippy, Cargo tests, and all issue-owned
production/device/oracle/CI gates as required by the shared policy. Strictly
instruction/documentation-only changes use its explicit exception. A passing
focused diagnostic cannot replace an authoritative scientific gate. Reuse
passing checks only while their relevant inputs and environment remain valid.

Capture complete verification logs on disk, preserve exit status, and display
concise results and paths. Inspect the failed assertion/stage before widening
output. Use `--verbose` or `--clean` only for the documented diagnostic or
reproducibility reason; do not discard valid caches routinely.

## Push, CI, and completion

Within the user's authorization, normally push once after stable verification.
Update the existing PR when supplied; use an explicit lease for a requested
rebase of its branch. Repush only for an in-scope repair, required base update,
or user request. Poll concise CI status at least 60 seconds apart, confirm
checks belong to the pushed head, and inspect failed-step logs when needed.

Finish with the implemented scope, checks and evidence paths, PR/commit link,
and any unresolved dependency or blocked check. Required checks must pass or
have a demonstrated unrelated baseline/infrastructure failure documented;
that failure remains a failure. Do not claim parity from execution or green CI.
