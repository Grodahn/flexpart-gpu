---
name: code-review
description: Review a FLEXPART-GPU pull request against its issue contract, repair every confirmed in-scope finding in the same autonomous run, verify once efficiently, push, and confirm CI without redundant inspection or test cycles.
---

# Code Review and Repair Workflow

Use this skill for pull-request reviews, including reviews where confirmed findings should be fixed. Review and repair are one autonomous workflow; do not pause for approval between review, implementation, verification, push, and CI confirmation unless explicitly requested.

## Scope

- The referenced issue is the sole implementation contract.
- Treat open dependencies as hard boundaries. Fail closed where their semantics are unresolved.
- Do not implement adjacent behavior, clean up unrelated code, or fix unrelated test/CI failures.
- If correctness would require expanding the issue contract, stop that path and report or record the dependency.
- Preserve unrelated user changes. Use an isolated worktree only if the active worktree is dirty or on another task.
- Use a single agent for the review-and-repair task.

## Efficient inspection

- Fetch issue and PR metadata, changed files, review state, and checks in as few batched GitHub calls as practical.
- Batch independent read-only repository inspections.
- Use targeted searches and bounded snippets.
- Do not emit full issue bodies, full diffs, complete API responses, successful test logs, or complete CI logs.
- Prefer filenames, diff statistics, exact failing assertions, and the last relevant failure lines.
- Treat an already-injected `AGENTS.md` as read; do not reopen, quote, or summarize it.
- Keep intermediate updates to meaningful milestones or blockers only.

## Review and repair

1. Compare the PR against the issue contract and relevant dependencies.
2. Identify only confirmed correctness, scope, validation, or maintainability findings that belong to that contract.
3. Fix all confirmed in-scope findings together before the initial push.
4. Do not convert speculative concerns into code churn; verify a suspected problem before editing.
5. Update the PR title or description only when needed to state the actual completed scope or unresolved dependencies accurately.

## Verification economy

1. First run tests directly covering changed or repaired behavior.
2. Run `rustfmt` and `cargo clippy` as required by the repository.
3. Run the broader required test suite once after the repair is stable.
4. Do not rerun a passing check unless relevant code changed afterward.
5. Capture successful command output compactly.
6. On failure, inspect only the relevant failing test, step, or minimal log section before widening the investigation.
7. Distinguish failures caused by the PR from established unrelated baseline or infrastructure failures.

## Push and CI

- Push once after local verification passes.
- Poll CI no more frequently than once per 60 seconds and request only concise status fields.
- If CI fails, inspect the failed step and minimal relevant log section.
- Fix and repush only when the failure is caused by this PR.
- For an unrelated infrastructure or baseline failure, provide concise evidence without expanding scope.

## Finish criteria

Finish only when:

- every confirmed in-scope review finding is fixed;
- required local checks have completed;
- PR checks are green, or a demonstrably unrelated external failure is documented; and
- the final response concisely lists the findings/fixes, verification results, PR link, and any remaining issue-owned dependency.
