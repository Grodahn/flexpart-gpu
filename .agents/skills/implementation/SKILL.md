---
name: implementation
description: Implement a FLEXPART-GPU issue or scoped code change efficiently, with strict scope control, targeted-first verification, bounded tool output, and minimal redundant test or CI work.
---

# Implementation Workflow

Use this skill for implementation tasks that change code, tests, shaders, build behavior, or runtime behavior.

## Contract and scope

- When an issue is supplied, treat that issue as the sole implementation contract.
- Treat open dependencies and unresolved scientific or architectural semantics as hard boundaries. Fail closed instead of inventing behavior.
- Do not implement adjacent behavior, clean up unrelated code, or fix unrelated test/CI failures.
- If correctness would require expanding the issue contract, stop that path and report or record the dependency instead.
- Preserve unrelated user changes. Use an isolated worktree only when the active worktree is dirty or being used for another task.
- Use one agent for one bounded implementation task unless explicit parallelism is required.

## Efficient inspection

- Fetch issue metadata, dependencies, relevant files, and checks in as few batched calls as practical.
- Batch independent read-only repository inspections.
- Use targeted searches and bounded snippets.
- Do not print full issue bodies, full diffs, complete API responses, or successful command logs.
- Treat an already-injected `AGENTS.md` as read; do not reopen, quote, or summarize it.
- Read only the source, tests, oracle/reference material, and documentation required to establish the current contract.

## Implementation

- Write or update tests before or alongside the implementation.
- Make related in-scope changes together before broad verification.
- Preserve established fail-closed behavior where a dependency still owns unresolved semantics.
- Do not broaden a change merely to make nearby code cleaner.

## Verification economy

Use the narrowest verification that can falsify the current change.

1. Run tests directly covering the changed behavior while iterating.
2. Do not run the full suite after every edit.
3. Batch related fixes before re-testing.
4. Once targeted tests are stable, run `cargo fmt --all -- --check` and `cargo clippy` as required by the repository.
5. Run the required broader test suite once after the implementation is stable.
6. Do not rerun a passing check unless relevant code changed afterward.
7. On failure, inspect only the relevant failing assertion, test, or final log section before widening the investigation.
8. Distinguish failures introduced by the change from established unrelated baseline or infrastructure failures.

## Push and CI

- Normally push once after local verification passes.
- If CI confirmation is part of the task, poll no more frequently than once per 60 seconds and request only concise status fields.
- If CI fails, inspect only the failed step and the minimal relevant log section.
- Fix and repush only when the failure is caused by the current change.
- For unrelated infrastructure or baseline failures, record concise evidence without expanding scope.

## Finish criteria

Finish only when:

- the issue-owned behavior is implemented without taking ownership of adjacent contracts;
- directly affected tests pass;
- required final formatting, linting, and broader tests have completed;
- CI is green when required, or a demonstrably unrelated external failure is documented; and
- the final response concisely lists the implementation, verification results, relevant PR/commit link, and remaining issue-owned dependencies.
