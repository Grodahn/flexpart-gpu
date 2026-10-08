# Compact oracle and validation execution

Issue #92 adds an agent-facing command around the existing #6/#48 corpus
runner. It does not define another oracle, evaluator, threshold, case manifest,
or evidence schema.

## Start here

Run the smallest paired check for one canonical corpus case:

```bash
python scripts/agent_validation.py --check comparison --case ADV-ANA-001
```

The successful default output is one JSON line. `state` describes execution
(`PASS`, `FAIL`, `BLOCKED`, or `ERROR`); it is not a scientific parity verdict.
`scientific_verdict` is copied from the existing corpus comparison report and
remains `DIAGNOSTIC_NO_PARITY_VERDICT`. The JSON names the candidate revision,
the pinned FLEXPART 11.1 commit, build-cache disposition, elapsed time, complete
stage logs, and evidence directories.

Use an oracle-only check when candidate execution is irrelevant:

```bash
python scripts/agent_validation.py --check oracle --case ADV-ANA-001
```

The selector accepts only implemented cases owned by the synthetic corpus
runner and having both canonical candidate and Fortran fixtures. Blocked,
candidate-only (`REPEAT-009`), and ETEX cases fail at argument validation
instead of starting an unrelated or incomplete workflow.

Use `--verbose` only after the compact result or bounded failure tail is not
enough. It mirrors complete logs to stderr while preserving the same log files.
The summary labels this mode `verbose-plus-json` and counts both mirrored log
bytes and the final JSON line; compact mode counts only the JSON line.
Use `--clean` for a reproducibility check; it forces a no-cache Docker image
build and a clean Fortran compile. A clean run is deliberately not the normal
development loop.

Full CI, corpus, ETEX, repeatability, and seed-identity gates remain separate:
run the gate required by the owning issue. In particular, this focused command
does not replace `scripts/ci-gate.sh`, `scripts/run-corpus.sh all`, or the #60/#61
production/CI orchestration once those issues require them.

Focused deposition comparisons also run and audit `ADV-ANA-001`, the inert
same-grid oracle case already consumed by the existing budget calibration.
The comparison is restricted to the requested case plus that declared
dependency, and the focused manifest hashes both oracle input/output trees.
This prevents unrelated retained oracle results from changing a focused run.

## Agent workflow policy

This is the shared verification policy for repository agents. `AGENTS.md` and
the implementation/review skills route here; #92 / PR #95 supply the existing
compact execution infrastructure. This policy changes neither its scientific
semantics nor the required CI gates.

Choose the smallest check that can falsify the issue's claim:

| Task or claim | First check | Stable final verification |
| --- | --- | --- |
| Instructions/documentation only | Check local links, skill frontmatter, current-main rule preservation, and `git diff --check` | Review the complete diff and confirm only instruction/documentation files changed; explain omitted executable checks in the PR. |
| Rust or WGSL change | Relevant unit/integration target and exact test or narrow filter | `cargo fmt --all -- --check`, `cargo clippy`, `cargo test`, plus every issue-owned device/oracle/production gate. |
| Supported focused corpus comparison | `python scripts/agent_validation.py --check comparison --case <CASE>` | Required broader corpus, production, and CI gates once the change is stable. |
| Oracle-only investigation | `python scripts/agent_validation.py --check oracle --case <CASE>` | The owning oracle/reproducibility check; oracle-only success proves no candidate comparison. |
| Scripts, workflows, fixtures, or build inputs | Directly affected checks and artifact/provenance audits | Required consumer and CI gates; these changes do not qualify for the documentation-only exception. |

For a supported focused corpus check, use `agent_validation.py` by default
instead of manually chaining candidate, oracle, audit, comparison, and manifest
commands. Select the issue-relevant case; `ADV-ANA-001` is an example, not a
universal scientific test. Check selectors against the current runner and
corpus index. Unsupported cases require their owning documented path, not a
substituted case or a candidate-only result presented as paired validation.

Normal iterations keep Cargo, Docker, and the validated oracle cache. Let the
existing cache identity checks rebuild stale inputs. Do not delete `target/`,
run `cargo clean`, force Docker rebuilds, or pass `--clean` routinely. Use a
clean run only for an explicit reproducibility obligation or a diagnosed cache
problem. Do not duplicate oracle-building or comparison semantics in a new
agent-specific wrapper.

Start with compact JSON and the process exit status. Read `state`,
`scientific_verdict`, case/revision identity, stage results, and evidence paths.
`PASS` proves successful execution of the selected path only; the current
comparison verdict remains `DIAGNOSTIC_NO_PARITY_VERDICT`. `FAIL`, `BLOCKED`,
`ERROR`, non-zero exit, missing evidence, or skipped required execution cannot
be described as passing validation. Keep actual WGSL/device execution and
scientific comparison evidence separate as required by `GPU_CONTRACT.md`.

Retain full output under `target/` and inspect the compact diagnostic first.
Then inspect only the failed stage's named log and a bounded relevant excerpt
(normally at most 30 lines of at most 500 characters each). Widen the excerpt
or use `--verbose` only when that evidence is insufficient. For other commands,
redirect stdout/stderr to a named log, preserve the subprocess exit status,
and report the command, outcome, and log path; never let a tail/filter command
mask failure. A filtered test with zero executed tests proves nothing.

Run broader required checks once at the stable final state. Reuse a passing
result only while its relevant source, fixtures, inputs, dependencies,
toolchain, adapter/backend, and configuration remain valid; a relevant rebase
or merge requires affected checks again. Record omitted, blocked, or unrelated
failing checks with concise evidence rather than weakening a gate or expanding
the task. Confirm that required CI checks belong to the pushed PR head, poll
status no more often than every 60 seconds, and fetch failed-step logs only
when needed. Green technical CI is not scientific parity.

## Retained artifacts

- Compact summary and complete stage logs:
  `target/agent-validation/<CASE>/<RUN-ID>/`. Each default invocation creates
  a new run directory, so later checks do not overwrite earlier transcripts.
  An explicit `--output-dir` must likewise be empty.
- Candidate and raw/decoded oracle evidence (unchanged existing layout):
  `target/corpus/candidate/<CASE>/` and `target/corpus/oracle/<CASE>/`.
- Focused existing-format comparison report:
  `target/agent-validation/<CASE>/<RUN-ID>/comparison-report.json`.
- Focused existing-format provenance manifest:
  `target/agent-validation/<CASE>/<RUN-ID>/run-manifest.json`.
- Oracle build identity and full build transcript:
  retained cache copies named `<STAGE>-build.json` and `<STAGE>-build.log` in
  the run directory. The active cache also remains at
  `target/oracle-cache/build.json` and `target/oracle-cache/build.log`.

Failure output contains the failed stage and at most 30 trailing log lines of
at most 500 characters each. The complete transcript is never truncated on
disk.

## Cache identity and invalidation

`scripts/run-corpus.sh oracle` now prepares the oracle once before executing
one or several cases. It reuses the retained image and executable only when all
of these identities match:

- pinned oracle Git commit;
- `docker/Dockerfile.fortran` SHA-256 (therefore base image, Ubuntu snapshot,
  and package/toolchain recipe);
- `docker/docker-compose.fortran.yml` SHA-256;
- `reference/flexpart-11.1.json` SHA-256;
- pinned checkout `src/makefile_gfortran` SHA-256;
- literal build arguments `FC=gfortran eta=no arch=x86-64 -j4`;
- resolved Docker image ID;
- retained FLEXPART executable SHA-256;
- retained full build-log SHA-256.

A missing or malformed record, missing image/executable, changed hash, changed
revision, or explicit `--clean` produces a rebuild. The normal path performs no
package installation or source download; Docker may resolve packages only when
the deterministic image inputs require a rebuild. The pinned checkout is still
verified clean before reuse and restored/verified after compilation.

Cargo's existing `target/` cache and Docker's layer cache continue to persist.
The new record makes reuse of the previously persistent-but-unconditionally-
discarded Fortran executable explicit and fail-closed.

## Before/after audit

The pre-change paths were inspected at issue #92's starting revision. For a
single paired `ADV-ANA-001` check, an agent had to select and interpret four
commands (candidate, oracle, input audit, comparison). The oracle command
always invoked `docker compose build`, `make clean`, and a complete Fortran
compile. `run-corpus.sh oracle all` performed that sequence once per oracle
case because it lived inside `step_oracle_case`. Docker layers and Cargo output
persisted, but the retained oracle objects/executable were unconditionally
discarded. Successful compiler, container, candidate, audit, and comparison
output all went to the terminal even when artifacts already contained it.

The new focused path uses one agent command, runs the unchanged existing stages,
builds at most once for a multi-case oracle invocation, and reuses an exactly
identified image/executable on an unchanged second run. Successful terminal
output is one JSON line; full output moves to named logs. Wall-clock and byte
measurements for each run are recorded in `summary.json` and the stage logs.
Before each selected case runs, only that case's generated candidate/oracle
directories are cleared, preventing stale seeds or raw slices from entering a
focused report while leaving unrelated cases untouched.

Measured on Windows/Docker Desktop with the pinned checkout and `ADV-ANA-001`
(2026-09-28):

| Representative path | Agent commands | Build behavior | Terminal bytes | Wall time |
|---|---:|---|---:|---:|
| Legacy-equivalent warm image build + unconditional clean Fortran compile + oracle run | 1 oracle command (4 commands for a manual paired comparison) | image layers reused, executable rebuilt | 10,167 | 44.7 s |
| Compact oracle, unchanged retained inputs | 1 | image and executable reused | 1,809 | 5.2 s |
| Compact paired comparison plus provenance manifest, warm Cargo + oracle caches | 1 | image and executable reused | 2,873 | 9.1 s |

The legacy-equivalent preparation component was measured independently at
40.2 s and 4,105 terminal bytes; its oracle-run component was 4.5 s and 6,062
log bytes. The compact warm oracle was therefore about 8.6x faster and emitted
about 82.2% fewer terminal bytes. The first explicit `--clean` run also passed
(260.7 s), then the unchanged run reported `REUSED`; this proves that reuse is
an optimization rather than a hidden prerequisite. The paired report retained
the existing `DIAGNOSTIC_NO_PARITY_VERDICT` status.

Before Docker and the pinned checkout were prepared, the same command returned
`BLOCKED` with a bounded diagnostic and non-zero status. Missing prerequisites
therefore never become a successful paired validation. Repository unit tests
also exercise cache invalidation, focused command selection, bounded
diagnostics, and the existing focused audit/report selectors.

## Compact domain test orchestration

Issue #174 adds `python scripts/agent_tests.py --domain DOMAIN --level focused|domain`.
Use `--list` for the exact finite selection and the
[profile table](agent/test-map.md#compact-domain-runner) for ownership. Supported
v1 domains are `meteorology-resident`, `transport-advection`, `simulation`, and
`validation-provenance`. There is no generic full profile. Existing final
formatting, Clippy, Cargo, production, pinned-oracle and CI gates still apply.

Each invocation creates `target/agent-tests/<domain>/<unique-run-id>/` with a
versioned `summary.json`, exact commands/environment, complete combined binary
stdout/stderr stage logs, subprocess exit codes, elapsed seconds, output bytes,
test counts where available, and retained device reports. Success emits one
JSON line; failure includes at most eight assertion lines and the existing
30-line/500-character bounded tail. `terminal_bytes` measures this JSON line.
`state` is `PASS`, `FAIL`, `BLOCKED` or `ERROR`; stages additionally distinguish
`SKIPPED` and `NOT_RUN`. Unknown test counts are null per stage (preflight and
paired scripts are not libtest suites); aggregate counts sum known tests only.
A subprocess nonzero exit is returned unchanged, including Unix signals.
Wrapper evidence failures return 1 while retaining the subprocess's actual 0.
An absent executable returns 127 with a null subprocess status.

Missing adapters, zero tests, skipped selections, stale resident reports and
missing/malformed mandatory evidence never pass. Resident evidence is checked
against existing #171 identities and execution fields; advection requires the
existing adapter/displacement markers and successful exact test. Paired checks
delegate to `agent_validation.py`, retain its pinned identity and diagnostic
scientific verdict, and consume its existing reports. This adds no comparison
algorithm, tolerance, fixture, or oracle build path.

Normal Cargo/Docker caches remain intact. The resident producer has a fixed
report path: runner invocations use an exclusive `target/agent-tests/resident.lock`,
reject unchanged evidence, and copy the fresh report into their run directory.
Do not run the legacy resident command concurrently in the same checkout.
After a killed runner, inspect the lock and remove it only after confirming no
resident invocation remains. Other test-owned artifacts keep their existing
locations; paired manifests/reports live inside the unique run, while raw
corpus outputs retain the existing shared lifecycle. The runner similarly locks paired profiles with `target/agent-tests/paired.lock`.
Avoid concurrent legacy paired corpus invocations in one checkout as required
by that existing lifecycle.
`PASS` means selected checks executed, never scientific parity or hardware GPU
performance. Software-device wall times measure orchestration only.

The simulation domain profile runs both full forward variants and backward,
but currently returns `BLOCKED` for backward: that legacy test can return early
on `NoAdapter` without a device execution marker. A separate owner-provided
backward execution-evidence prerequisite is needed; #174 does not rewrite the
test. Simulation focused uses the required forward order marker and preflight.
Likewise, the existing manifest tests skip when a pinned FLEXPART checkout is
absent at the repository sibling `../flexpart`. This makes the
validation-provenance domain profile nonzero with an explicit `SKIPPED` stage.
Provide that existing prerequisite; do not treat a skipped profile as validation. These limitations leave mandatory CI gates unchanged.
