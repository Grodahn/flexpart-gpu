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

Use `--verbose` only after the compact result or bounded failure tail is not
enough. It mirrors complete logs to stderr while preserving the same log files.
Use `--clean` for a reproducibility check; it forces a no-cache Docker image
build and a clean Fortran compile. A clean run is deliberately not the normal
development loop.

Full CI, corpus, ETEX, repeatability, and seed-identity gates remain separate:
run the gate required by the owning issue. In particular, this focused command
does not replace `scripts/ci-gate.sh`, `scripts/run-corpus.sh all`, or the #60/#61
production/CI orchestration once those issues require them.

## Retained artifacts

- Compact summary and complete stage logs:
  `target/agent-validation/<CASE>/`.
- Candidate and raw/decoded oracle evidence (unchanged existing layout):
  `target/corpus/candidate/<CASE>/` and `target/corpus/oracle/<CASE>/`.
- Focused existing-format comparison report:
  `target/agent-validation/<CASE>/comparison-report.json`.
- Focused existing-format provenance manifest:
  `target/agent-validation/<CASE>/run-manifest.json`.
- Oracle build identity and full build transcript:
  `target/oracle-cache/build.json` and `target/oracle-cache/build.log`.

Failure output contains the failed stage and at most 30 trailing log lines. The
complete transcript is never truncated on disk.

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
- retained FLEXPART executable SHA-256.

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

Measured on Windows/Docker Desktop with the pinned checkout and `ADV-ANA-001`
(2026-09-27):

| Representative path | Agent commands | Build behavior | Terminal bytes | Wall time |
|---|---:|---|---:|---:|
| Legacy-equivalent warm image build + unconditional clean Fortran compile + oracle run | 1 oracle command (4 commands for a manual paired comparison) | image layers reused, executable rebuilt | 10,167 | 44.7 s |
| Compact oracle, unchanged retained inputs | 1 | image and executable reused | 1,163 | 4.5 s |
| Compact paired comparison plus provenance manifest, warm Cargo + oracle caches | 1 | image and executable reused | 2,085 | 122.1 s |

The legacy-equivalent preparation component was measured independently at
40.2 s and 4,105 terminal bytes; its oracle-run component was 4.5 s and 6,062
log bytes. The compact warm oracle was therefore about 9.9x faster and emitted
about 88.6% fewer terminal bytes. The first explicit `--clean` run also passed
(294.8 s), then the unchanged run reported `REUSED`; this proves that reuse is
an optimization rather than a hidden prerequisite. The paired report retained
the existing `DIAGNOSTIC_NO_PARITY_VERDICT` status.

Before Docker and the pinned checkout were prepared, the same command returned
`BLOCKED` with a bounded diagnostic and non-zero status. Missing prerequisites
therefore never become a successful paired validation. Repository unit tests
also exercise cache invalidation, focused command selection, bounded
diagnostics, and the existing focused audit/report selectors.
