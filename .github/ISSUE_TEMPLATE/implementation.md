---
name: Scoped implementation
about: Define one independently verifiable implementation contract
title: ""
labels: ""
assignees: ""
---

## Single claim

State one finite behavioral/scientific claim. Follow [issue-authoring guidance](https://github.com/Grodahn/flexpart-gpu/blob/main/.agents/skills/issue-authoring/SKILL.md).
Split independent proof boundaries and resolve scientific/architectural unknowns first.

## Scope and dependencies

Name supported inputs/outputs, owning dependencies and explicit exclusions.

## Agent context

Use when useful; keep this block small and link the relevant section of the
[canonical navigation map](https://github.com/Grodahn/flexpart-gpu/blob/main/docs/agent/repo-map.md) instead of copying architecture prose.

- Read first: domain map row and applicable normative contract.
- Expected implementation surface: exact host/kernel/API or artifact paths.
- Allowed adjacent inspection: named producer/consumer handoffs.
- Out of scope: concrete neighboring semantics and modules.
- Focused verification: command from [test map](https://github.com/Grodahn/flexpart-gpu/blob/main/docs/agent/test-map.md).

## Acceptance and proof obligations

| Requirement | Implementation surface | Test/check | Required artifact/result and validation level |
| --- | --- | --- | --- |
| Finite claim | Exact API/artifact | Executable command | Explicit postcondition |

For GPU/scientific claims, name the production path, actual device-execution proof,
pinned oracle revision/routine/invocation, normalized inputs, raw/decoded outputs,
hashes, metrics and tolerances. Do not substitute a diagnostic comparison for an
authoritative oracle.

## Stop rule

Stop scope expansion if a new physics semantic, unnamed consumer, external
data/reference or another issue's contract is needed. Record a dependency or
follow-up; expand this issue only if necessary to make its original claim correct.
