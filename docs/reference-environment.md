# FLEXPART 11.1 reference environment (RISK-03.3G-01)

This document defines the reproducible oracle setup behind every
`flexpart-gpu` vs Fortran comparison. The normative oracle is **FLEXPART
v11.1** at the pinned commit in `reference/flexpart-11.1.json`. The upstream
repository is read-only for this project: it is never modified, and no
issues or pull requests are filed there from this work.

## 1. Fetch the unmodified oracle sources

```bash
git clone https://gitlab.phaidra.org/flexpart/flexpart.git ../flexpart
cd ../flexpart
git checkout c70586c2b7f5258850705325881c61f557ea9bd8
```

Always read the exact commit from `reference/flexpart-11.1.json` (field
`pinned_commit`) instead of copying it from chat logs or docs.

## 2. Verify the checkout (fail-closed)

```bash
# Rust verifier (works on any host with git; no Fortran needed)
cargo run --bin reference-check -- verify --checkout ../flexpart

# Show the bundled manifest
cargo run --bin reference-check -- show
```

Verification requires all of the following, otherwise it exits non-zero:

1. `../flexpart` is a git work tree,
2. `git rev-parse HEAD` equals the pinned commit,
3. `git status --porcelain` is empty (unmodified sources).

The comparison entry points (`scripts/compare-fortran.sh`, `scripts/run-etex.sh`)
run this check before any Fortran build.

## 3. Build the oracle (Linux/Docker)

The Fortran worker needs gfortran, ecCodes, and NetCDF-Fortran. Build the
in-fork oracle image and compile the pinned sources:

```bash
docker compose -f docker/docker-compose.fortran.yml build flexpart-fortran
scripts/compare-fortran.sh compose setup
```

The setup compiles with the v11.1 build system (`make -f makefile_gfortran
eta=no arch=x86-64`; the legacy `make serial` recipe does not exist in v11.1)
and removes
the generated `gitversion.txt` stamp afterwards so the checkout stays clean
for re-verification. Native Windows builds are not supported: use the
manifest/checkout verification above plus Docker for the actual oracle runs.

The Dockerfile pins the Ubuntu 22.04 image digest and the Ubuntu package
snapshot dated 2026-09-10. The compiler uses the Fortran makefile's
`arch=x86-64` profile instead of host-specific `-march=native`. Each Docker
comparison writes `run_manifest.json` with the resolved image ID, package
versions, compiler version and flags, source revisions, adapter, and SHA-256
hashes of executable, inputs and outputs. Runners that do not expose a random
seed record it as unavailable; such a run cannot support a multi-seed parity
claim.

### Frozen single-thread oracle execution and determinism contract (#49)

`reference/flexpart-11.1.json` versions the canonical execution profile under
`execution_profile` (id `flexpart-11.1-single-thread`, version 1): pinned
commit, clean-checkout requirement, build (`eta=no arch=x86-64`, gfortran,
`-O3`, `-fopenmp`, `-march=x86-64`), Docker boundary (pinned Ubuntu digest and
snapshot), the eight explicit OpenMP runtime settings with
`OMP_NUM_THREADS=1`, required input files, `external_seed_control:
unavailable`, and the two repeatability cases (`ADV-ANA-001` deterministic,
`WIND-UNI-002` stochastic, minimum five repetitions each).

Enforcement reuses the #6 oracle path. Compose sets the runtime settings
literally so inherited host OpenMP values cannot override them. Every oracle
invocation (single-result and repeatability) runs
`scripts/write_oracle_run_manifest.py check-runtime-profile` in the actual
container process environment immediately before FLEXPART. Missing or changed
settings and uncontracted `OMP_`/`GOMP_`/`KMP_` overrides are rejected
non-zero; the resolved settings and reference-manifest hash are recorded per
run. The repeatability runner builds the oracle executable once, verifies the
same SHA-256 before every repetition, generates shared meteorology once per
case, and preserves each repetition separately under
`target/corpus/oracle_repeatability/<CASE>/rep_XX/` (raw `header`/`dates`/
`grid_conc_*`, decoded `oracle_summary.json`, `runtime_profile.json`,
`fortran.log`, post-run `COMMAND`/`RELEASES`/`OUTGRID` copies). The pinned
checkout is verified clean before the experiment and after build and runs
(the v11.1 makefile's `FLEXPART.f90` stamp is restored on the host); a dirty
or wrong-revision checkout fails non-zero.

Single-experiment tie-in: each run recreates its case directories fresh and
writes a per-case `experiment.json` (profile id/version, executable SHA,
image, compiler, requested repetitions) before any repetition. Every
repetition records `oracle_executable.sha256` and `consumed_inputs.json`
(SHA-256 of the prepared run-directory options including `COMMAND`,
`RELEASES`, `OUTGRID`, plus the shared meteorology) BEFORE the FLEXPART
invocation; post-run fixture copies are traceability only, never input
evidence. The comparator evaluates only the cases the invocation ran (a
single `CASE` yields a valid single-case report; canonical evidence uses the
default: both cases), requires the requested repetition count to match the
directories found, ties every repetition to the experiment executable
(including the binary on disk), and verifies all repetitions of a case
consumed identical inputs. Stale directories, foreign executables, changed
inputs, or a missing experiment record fail non-zero.

`scripts/corpus/compare_oracle_repeatability.py` hashes every raw and decoded
artifact, checks the frozen profile, and writes
`target/corpus/oracle_repeatability_report.json` with executable, image,
compiler, input, runtime, and seed identity plus per-repetition
byte-identical-to-baseline results. Non-identical artifacts name the differing
files and decoded fields with max absolute/relative differences. The report
status is `ORACLE_REPEATABILITY_CHARACTERIZED_NO_PARITY_VERDICT`: execution
repeatability only, never candidate-vs-oracle physics parity.

Reproduce with a pinned checkout (override `FLEXPART_DIR` when this repository
is an isolated worktree; on Windows use a space-free path such as the 8.3
short path):

```bash
scripts/run-corpus.sh oracle-repeatability all 5
# Single-case debugging yields a valid single-case report:
scripts/run-corpus.sh oracle-repeatability ADV-ANA-001 5
```

Observed result (2026-09-18, Docker Desktop, `flexpart-fortran:latest`
resolved to `sha256:2d10cfc9a51c91056beda6a9d55840a334dccc7212222306540004b667a6259b`,
`GNU Fortran 11.4.0`, executable
`a58c365b0e68c2136de335dccac8ebccd94ba44de9d7f44f1983bb878872e3da`):
both `ADV-ANA-001` and `WIND-UNI-002` were bitwise repeatable across all five
repetitions (raw SHA-256 and decoded summary identical to baseline). This
holds for the pristine default RNG initialization under one thread; the
repetitions are not independent stochastic realizations.

Limitations: single-thread profile only; the `latest` image tag is mutable so
only the resolved image ID is attributable; the executable is rebuilt per
experiment and its SHA must be recorded; FLEXPART logs a `RECEPTORS cannot be
opened` warning and continues without receptor output (run still succeeds);
disabling turbulence does not imply zero RNG consumption during startup;
controlled independent oracle seeds belong to #50; the global artifact layout
redesign belongs to #53.

Regression evidence: `python scripts/test_oracle_run_manifest.py` (canonical
settings accepted, every missing/changed setting rejected, uncontracted
overrides rejected, CLI rejects `OMP_NUM_THREADS=2`) and
`python scripts/corpus/test_compare_oracle_repeatability.py` (numeric diffs
quantified; contract violations, missing repetitions, stale repetitions tied
to a foreign executable, changed consumed inputs, missing experiment records,
and unscoped partial evaluations fail closed; single-case scoping yields a
valid single-case report).

## 4. What the oracle is used for

- Synthetic uniform-wind comparison (`scripts/compare-fortran.sh validate`,
  `src/bin/fortran-validation.rs`, `scripts/compare_concentrations.py`).
- ETEX-1 paired runs (`scripts/run-etex.sh all`): prepare both model inputs
  from the same independently downloaded ERA5 arrays, execute the pinned
  Fortran oracle and the WGSL candidate, and compare complete three-hour
  concentration windows with DATEM observations. The report contains model
  diagnostics and input checksums; it does not assert scientific parity.
- Checked-in ETEX-1 mini smoke run (`scripts/run-etex.sh mini`): verify six
  native ERA5 snapshots in `fixtures/etex/native-mini/`, derive Fortran and
  GPU inputs, audit their actual prepared fields and scenario settings, run
  both models over 12 hours, and pair their outputs with independent station
  observations. The audit is available separately with
  `ETEX_PROFILE=mini scripts/run-etex.sh audit`; its report is recorded in
  the run manifest. See `fixtures/etex/mini/README.md` for the remaining
  scientific differences. Metrics are diagnostic.
- Future parity gates in issues RISK-03.3G-03 and later.

Oracle outputs are produced at validation time and compared, never vendored
as fixtures. ETEX measurement/meteorology/source-term provenance is recorded
in `fixtures/etex/PROVENANCE.md`.

The ETEX workflow requires Python packages `eccodes`, `numpy`, `xarray`,
`gcsfs`, and `zarr`, Docker Compose, and Cargo. Run `scripts/run-etex.sh status`
to inspect local inputs and outputs. `compare` fails when either model output
is absent or the model windows do not match. A complete ETEX run also needs
the externally downloaded ERA5 arrays; a build or synthetic smoke test alone
does not validate ETEX.

In CI, the pinned oracle is cloned from the upstream repository at the
`pinned_commit` in `reference/flexpart-11.1.json` and verified with
`reference-check -- verify` before any build (see `docs/ci-gates.md`).
The small per-PR gate builds the oracle Docker image and compiles FLEXPART;
the larger synthetic and ETEX runs stay local/manual.

The standard synthetic validation setup (`scripts/compare-fortran.sh`
`validate`) uses an output cadence whose last window covers the run end, and
the comparison reads that last file - never a mid-run time average against an
instantaneous end state (see the RISK-03.3G-03 addendum in
`docs/validation-report.md`).

## 5. Software-adapter note

Oracle comparisons must run on a hardware GPU or document the adapter. Runs
on a software fallback adapter (`FLEXPART_GPU_SOFTWARE=1`) are valid for
plumbing but their timings must never be reported as GPU performance values.
