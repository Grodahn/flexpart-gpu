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

### Seedable validation oracle (issue #50)

For stochastic ensemble evidence, a separately identified
`seedable-validation-oracle` is built by applying exactly the versioned
validation-only patch `reference/flexpart-11.1-seedable.patch` to a fresh clone
of the pristine checkout, then building under the same frozen #49 profile.
The patched executable is **never** the normative oracle; its identity is
recorded in `target/corpus/oracle_seedable/experiment.json` and distinguished
from the pristine executable by SHA-256. The environment variable
`FLEXPART_VALIDATION_SEED` (canonical decimal in [1, 1000000000]) selects the
initial RNG state; unset/empty preserves the pristine default bit-exactly.
Invalid seeds are rejected with actionable error — no silent fallback.

Reproduce the seedable evidence (requires pristine #49 baseline first):

```bash
scripts/run-corpus.sh oracle-repeatability WIND-UNI-002 5
scripts/run-corpus.sh oracle-seed-identities WIND-UNI-002
```

The evidence report at `target/corpus/oracle_seed_identity_report.json`
characterizes default-equivalence, 10 distinct identities, and same-seed
repeatability.

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

Single-experiment tie-in: each invocation mints one random experiment UUID
(deliberately neither the executable hash nor a timestamp) and writes it to
every case's `experiment.json` (alongside profile id/version, executable SHA,
image, compiler, requested repetitions) before any repetition; case
directories are recreated fresh. Every repetition records
`oracle_executable.sha256` and `consumed_inputs.json` (SHA-256 of the
prepared run-directory options including `COMMAND`, `RELEASES`, `OUTGRID`,
plus the shared meteorology) BEFORE the FLEXPART invocation; post-run fixture
copies are traceability only, never input evidence. The comparator evaluates
only the cases the invocation ran (a single `CASE` yields a valid,
clearly scoped single-case report; canonical evidence uses the default: both
cases), requires the requested repetition count to match the directories
found, requires multi-case reports to share one experiment ID (mixed
experiments are rejected even when the executable hash is identical), ties
every repetition to the experiment executable (including the binary on disk),
and verifies all repetitions of a case consumed identical inputs. Stale
directories, foreign executables, changed inputs, or a missing experiment
record fail non-zero.

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
controlled independent oracle seeds belong to #50 (see
`reference/oracle-stochastic-identity.json` and `docs/oracle-stochastic-identity.md`);
the global artifact layout redesign belongs to #53.

Regression evidence: `python scripts/test_oracle_run_manifest.py` (canonical
settings accepted, every missing/changed setting rejected, uncontracted
overrides rejected, CLI rejects `OMP_NUM_THREADS=2`) and
`python scripts/corpus/test_compare_oracle_repeatability.py` (numeric diffs
quantified; contract violations, missing repetitions, stale repetitions tied
to a foreign executable, changed consumed inputs, missing experiment records,
and unscoped partial evaluations fail closed; single-case scoping yields a
valid single-case report).

## 4. flex_extract calc_etadot oracle (#70)

The preprocessing of the ECMWF eta-coordinate vertical velocity (deta/dt,
GRIB parameter 77) into the pressure vertical velocity consumed by FLEXPART
as omega is validated against **flex_extract v7.1.2** at the commit pinned in
`reference/flex-extract.json` (`calc_etadot`, regular-grid META=1,
METADIFF=0, MOMEGA=0, MDPDETA=1). This oracle is documented as
`flex-extract-7.1.2-calc-etadot` (revision 1) in the manifest's
`execution_profile`. It is separate from the FLEXPART 11.1 oracle above.

### Fetch and verify

```bash
git clone https://gitlab.phaidra.org/flexpart/flex_extract.git ../flex_extract
cd ../flex_extract
git checkout e0005c99ac81d12faa45a8ff799debbd592b0dc0  # read pinned_commit instead
cd ..
cargo run --bin reference-check -- verify --checkout ../flex_extract \
  --manifest reference/flex-extract.json
```

Verification is fail-closed like the FLEXPART tier: the checkout must be at
the pinned commit with an empty `git status --porcelain`.

### Build and run (Docker)

The `calc_etadot` build needs gfortran, ecCodes, EMOSLIB (`-lemosR64`, which
resolves the spectral routines `set90`/`set99`/`jsspol`/`fft99`) and OpenJPEG
(`-lopenjp2`, a static-link dependency of ecCodes). The container image comes
from `docker/Dockerfile.flex-extract` (base `flexpart-fortran:latest` plus
`libemos-dev`/`libemos-bin`/`libemos-data`/`libopenjp2-7-dev`, all resolved
from the same pinned Ubuntu snapshot):

```bash
docker compose -f docker/docker-compose.fortran.yml build flex-extract
scripts/vertical/flex_extract_etadot_oracle.sh --flex-extract-checkout ../flex_extract
```

The driver copies `Source/Fortran` and the `Testing/Installation/Calc_etadot`
example inputs into scratch dirs under `target/ci-gate/flex-extract-oracle/`
so the checkout stays byte-for-byte pristine, compiles `calc_etadot` with the
manifested LIB line, runs the example (must print
`CONGRATULATIONS`), extracts canonical snapshot/motion/oracle JSONs, runs the
`eta-dot-column-report` candidate, and compares the full 6x6 field x levels
88-91 (36 points x 4 levels) against the oracle reference with f32-vs-f64
tolerances (worst attributable relative error 3e-5, absolute 1e-7 Pa/s).
Build and run logs, the extracted JSONs, `run-provenance.json` and the
comparison report land in `target/ci-gate/flex-extract-oracle/`.
`run-provenance.json` binds the concrete container image ID, compiler
version, oracle executable hash, consumed fort.* hashes and fort.15 output
hash to the comparison. The extracted oracle metadata identifies the pinned
upstream Calc_etadot fixture as a native-model-level source and derives its
valid time from the GRIB metadata rather than injecting a synthetic timestamp.

The driver additionally runs the same pinned `calc_etadot` executable on a
full checked-in ETEX/ERA5 eta-dot profile at 1994-10-23 15:00 UTC:
`fixtures/etex/native-mini` supplies param 77 on all 137 model levels, the
matching real ERA5 U/V/T/Q fields, and the complete 138-interface A/B
coordinate. This exercises the recursive `ETAR(K)-ETAR(K-1)` path through
the entire native column instead of zero-filling the unrequested upper levels.
The checked-in surface-pressure archive is grid-point data, whereas
`calc_etadot` requires spectral ln(ps) on `fort.12`; therefore this proof
case transparently reuses the spectral ln(ps) carrier from the pinned pristine
installation fixture and replaces only its PV array with the real ERA5
138-interface A/B coefficients. That carrier is separately hashed and recorded
in `source-provenance.json`; it is **not** claimed to be ERA5 surface
pressure. The real-data acceptance report is
`real-era5-137/comparison-report.json` and requires all
137 x 65 x 41 = 365105 eta-dot-to-Pa/s values to pass.

Observed result (2026-09-21, Docker Desktop, `flex-extract:latest` built
from `flexpart-fortran:latest` `sha256:cafb19c…` with gfortran 11.4.0):
`STOP SUCCESSFULLY FINISHED calc_etadot: CONGRATULATIONS`, fort.15 = 21987
bytes, and the candidate matched the oracle field with worst relative error
1.03e-5 and worst absolute error 2.5e-8 Pa/s. The `calc_etadot.f90` blob at
the pinned commit hashes to sha256
`160F267F8741F23D13FDBA2F7A88F110BB131AA84AD7894FA43605258E55B0D9`; the source is
also pinned by git blob `741eba91eab049df23a560219d0f2656a6cc9881`, with the
hash computed from canonical Git bytes so host line-ending conversion cannot
change the fingerprint and is cross-checked by the comparison harness
together with the ETAR transform source-snippet contract and the fort.4
config.

Only the `positive_eta_increasing` raw eta-dot sign convention is accepted by
the validated native contract. The opposite sign convention remains
fail-closed until separately demonstrated against an independent oracle.

### CI wiring

`validation-gate.yml` clones the pinned flex_extract checkout (sibling
`../flex_extract`) and the gate runs step 2c automatically. When the
checkout is absent, the tier is reported `NOT_WIRED`, never `PASS`, and the
rest of the gate is unaffected (see `docs/ci-gates.md`).

## 5. What the oracle is used for

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

## 6. Software-adapter note

Oracle comparisons must run on a hardware GPU or document the adapter. Runs
on a software fallback adapter (`FLEXPART_GPU_SOFTWARE=1`) are valid for
plumbing but their timings must never be reported as GPU performance values.
