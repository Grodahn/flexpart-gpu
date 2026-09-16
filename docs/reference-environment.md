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
eta=no`; the legacy `make serial` recipe does not exist in v11.1) and removes
the generated `gitversion.txt` stamp afterwards so the checkout stays clean
for re-verification. Native Windows builds are not supported: use the
manifest/checkout verification above plus Docker for the actual oracle runs.

## 4. What the oracle is used for

- Synthetic uniform-wind comparison (`scripts/compare-fortran.sh validate`,
  `src/bin/fortran-validation.rs`, `scripts/compare_concentrations.py`).
- ETEX-1 side-by-side runs (`scripts/run-etex.sh all-with-fortran`).
- Future parity gates in issues RISK-03.3G-03 and later.

Oracle outputs are produced at validation time and compared, never vendored
as fixtures. ETEX measurement/meteorology/source-term provenance is recorded
in `fixtures/etex/PROVENANCE.md`.

The standard synthetic validation setup (`scripts/compare-fortran.sh`
`validate`) uses an output cadence whose last window covers the run end, and
the comparison reads that last file - never a mid-run time average against an
instantaneous end state (see the RISK-03.3G-03 addendum in
`docs/validation-report.md`).

## 5. Software-adapter note

Oracle comparisons must run on a hardware GPU or document the adapter. Runs
on a software fallback adapter (`FLEXPART_GPU_SOFTWARE=1`) are valid for
plumbing but their timings must never be reported as GPU performance values.
