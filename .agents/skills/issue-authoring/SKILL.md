---
name: issue-authoring
description: Create, refine, split, or re-scope FLEXPART-GPU issues with atomic contracts, executable proof obligations, explicit validation levels, dependencies, and stop rules.
---

# Issue Authoring and Task Slicing

Use when creating or changing issue contracts. Do not load merely to implement
or review an already-defined issue.

Apply **Issue Definition & Task Slicing** in current
[AGENTS.md](../../../AGENTS.md#issue-definition--task-slicing), which owns the
complete normative rules. Injected current content counts as read. Preserve all
seven authoring rules, verification boundaries, requirement-to-proof mappings,
validation levels, fail-closed behavior, provenance, and PR scope boundaries.
Do not maintain an older duplicate of those rules in this skill.

For a GPU/scientific contract, read `docs/GPU_CONTRACT.md` and specify the
production GPU calculation, actual device-execution proof, and authoritative
pinned FLEXPART oracle owned by the issue. A CPU implementation is sufficient
only for an explicitly designated reference, oracle, tooling, research, data,
or other non-production task allowed by `AGENTS.md`.

For each acceptance criterion, name the implementation/API or artifact surface,
concrete test/check, and required result. Name normalized inputs, pinned
revision/routine/invocation, comparisons, tolerances, raw/decoded evidence, and
provenance where applicable. Resolve unknown semantics in a prerequisite
rather than inventing them in an implementation issue. Include the explicit
stop rule for newly discovered physics, consumers, references, or contracts.

Use [the shared verification policy](../../../docs/agent-validation.md#agent-workflow-policy)
when defining commands: supported focused checks should use the existing
compact runner. Its diagnostic `PASS` is not scientific parity and cannot
replace a required production/oracle/CI gate. Define the actual proof obligation
rather than mandating expensive clean rebuilds or full corpora for every edit.

Batch relevant read-only issue/dependency inspections and display concise
changes. Creating or editing GitHub issues still requires authorization from
the user request; this skill does not authorize unrelated external actions.

## Agent context

For new or materially refined implementation issues, include a compact Agent
context block when useful. Link the relevant domain in
[the canonical repository map](../../../docs/agent/repo-map.md); do not duplicate
architecture prose. Name read-first contracts, expected implementation paths,
allowed adjacent producer/consumer inspection, concrete out-of-scope areas, and
a focused command from [the test map](../../../docs/agent/test-map.md).
Use [the implementation issue template](../../../.github/ISSUE_TEMPLATE/implementation.md).

When an existing issue is materially refined or picked up for implementation
after #123, add/update this block within authorized issue editing. Do not bulk
backfill open issues or change scientific scope, dependencies, tolerances or
acceptance semantics to add navigation metadata. Root navigation keeps existing
issues discoverable without a metadata rewrite.
