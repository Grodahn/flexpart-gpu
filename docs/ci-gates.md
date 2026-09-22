# CI gates and reproducible automation (RISK-03.3G-01, Issue #6 point 4)

Technical gates only. A green build or smoke test does not establish
scientific parity and does not close Issue #6.

## 1. What runs in CI today versus locally

### In CI on every pull request

| Job | Workflow | What it proves |
|-----|----------|----------------|
| `software-wgpu` | `.github/workflows/software-wgpu.yml` | Real WGSL advection (`SW-WGPU-ADVECTION-001`, 4096 particles, +10 m/s, 3600 s, 36.0 ± 0.2 km) on Mesa Lavapipe. Fails on a missing software adapter, a skipped test, or a missing result; uploads its log as an artifact. |
| `technical-gate` | `.github/workflows/validation-gate.yml` | Small deterministic technical gate via `scripts/ci-gate.sh`: pinned clean oracle verification, oracle Docker build and Fortran compile, `gpu-preflight --software`, the analytical `SW-WGPU-ADVECTION-001` case, and a 1000-particle synthetic candidate smoke on the software adapter, with input/output/provenance checks and machine-readable reports. Fails on any missing adapter, skipped GPU test, missing oracle artifact, or failed comparison; uploads `target/ci-gate/` as an artifact. |

### Local only (reproducible, not per-PR)

| Command | Prerequisites | What it does |
|---------|---------------|--------------|
| `cargo run --bin reference-check -- verify --checkout ../flexpart` | Git, no Fortran needed | Fail-closed pin and cleanliness check of the oracle checkout. |
| `cargo run --bin gpu-preflight -- --software` | Rust, Vulkan software rasterizer (Lavapipe on Linux, WARP on Windows) | Proves a real software-WGSL adapter and runs the tiny compute smoke test. |
| `FLEXPART_GPU_SOFTWARE=1 cargo test --test integration software_advection` | Same as above | Analytical displacement check, no oracle needed. |
| `scripts/compare-fortran.sh compose validate` | Docker + Compose, pinned `../flexpart`, Cargo, Python 3 + NumPy | Full synthetic oracle-versus-candidate comparison (10 000 particles, 6 h, 32×32 grid): builds the oracle image, compiles FLEXPART, generates synthetic GRIB, runs both models, compares averaged concentration windows, writes `target/validation/comparison_report.json` and `run_manifest.json`. |
| `scripts/run-etex.sh mini` | Docker + Compose, Cargo, Python 3 + NumPy, checked-in `fixtures/etex/native-mini/` | Paired ETEX-1 mini run from six native ERA5 snapshots (12 h, 48 GPU steps, four three-hour windows, 108 observation matches). No ERA5 download needed. Repeatable without manual file editing. |
| `ETEX_PROFILE=mini scripts/run-etex.sh audit` | Same as above, after `prepare`/`fortran` | Repeats the input-equivalence audit without rerunning the simulations. |
| `scripts/run-etex.sh all` | Above plus ARCO-ERA5 network access (`numpy`, `xarray`, `gcsfs`, `zarr`, `eccodes`) | Full ERA5 download and paired real-data run. Manual only. |

`scripts/compare-fortran.sh local` is intentionally unimplemented; Docker mode
is required for Fortran comparisons.

## 2. Small deterministic CI gate

Entry point: `scripts/ci-gate.sh` (also executed by `validation-gate.yml`).

```
# Full gate (Linux/Docker, matches CI):
scripts/ci-gate.sh

# With explicit options:
scripts/ci-gate.sh --particles 1000 --oracle-checkout ../flexpart

# Candidate-only iteration without Docker (reports INCOMPLETE and fails by design):
scripts/ci-gate.sh --skip-oracle-build
```

Fail-closed steps:

1. Verify the oracle checkout at the `pinned_commit` from
   `reference/flexpart-11.1.json` is clean (`reference-check -- verify`).
2. Build `flexpart-fortran:latest` and compile `FLEXPART` with
   `make -f makefile_gfortran eta=no arch=x86-64`; require the executable,
   hash it, and require the checkout to stay clean (`gitversion.txt` removed).
3. Run `gpu-preflight --software`; require `software adapter: true` and
   `smoke test: PASS` (a skipped smoke test fails).
4. Run `SW-WGPU-ADVECTION-001` with `--exact`; require `test result: ok`,
   `1 passed`, and the analytical marker (a filtered-out or skipped test fails).
5. Run `fortran-validation` with `PARTICLES=1000`, `SYNC_READBACK=1`,
   `RUST_LOG=info` on the software adapter; require a non-empty JSON output
   with the expected averaging window (`1800/900 s`, 3 samples,
   `endpoint_weight 0.5`, 24 steps), exactly one `wgpu adapter` record
   mentioning the software fallback, and active particles.
6. Write `run-manifest.json` via `scripts/write_oracle_run_manifest.py`
   (oracle revision, candidate revision, image ID, packages, compiler,
   executable/input/output hashes, adapter, seed note) and
   `ci-gate-report.json` (schema v1, see §5); require both to exist.

Fail-closed step 2b (#30, direct vertical routine):

- Compile the pinned `verttransform_ecmwf_heights` routine against the
  pristine `FLEXPART` objects, run synthetic and real ERA5/ETEX columns,
  and compare candidate + conformance-harness outputs against the routine
  oracle with prescribed tolerances (see `docs/vertical-transform.md`).

Fail-closed step 2c (#70, calc_etadot preprocessing oracle):

- Runs when the pinned flex_extract 7.1.2 checkout exists at
  `../flex_extract` (or `--flex-extract-checkout <dir>`); otherwise the tier
  is reported `NOT_WIRED` and never `PASS`. Passing
  `--require-flex-extract-oracle` makes that state fail the overall gate; the
  GitHub validation workflow always enables this requirement. The driver
  (`scripts/vertical/flex_extract_etadot_oracle.sh`) verifies the checkout
  against `reference/flex-extract.json`, builds `calc_etadot` in a scratch
  dir (extra packages `libemos-dev`/`libemos-bin`/`libemos-data`/
  `libopenjp2-7-dev` via `Dockerfile.flex-extract`), runs the pinned
  `Testing/Installation/Calc_etadot` example (must print `CONGRATULATIONS`),
  extracts canonical snapshot/motion/oracle JSONs, runs the
  `eta-dot-column-report` candidate, and compares every grid point x level
  against the oracle field (f32-vs-f64 tolerances, worst attributable
  relative error ≤ 3e-5). The same pinned executable is then run on a second
  checked-in ETEX/ERA5 case built from one complete real native-model-level
  column at 48.0 N, 2.0 W. Its raw eta-dot, T/U/V/Q and hybrid A/B coordinate
  cover all 137 model levels and its surface pressure comes from the matching
  ERA5 surface snapshot. The selected source column is replicated onto the
  pinned 6x6 installation-test work grid; those 36 copies are plumbing, not
  independent ERA5 columns. Because `calc_etadot` requires spectral ln(ps),
  the selected real surface pressure is encoded as a spatially constant
  spherical-harmonic field and the comparator verifies the reconstructed
  pressure against the source value. The proof therefore covers the complete
  real 137-level eta-dot recurrence without zero-filled levels. The real-data
  report must pass all 137 x 6 x 6 = 4932 replicated oracle/candidate
  comparisons and records the single selected source column explicitly. The
  validated native contract
  is deliberately narrowed to `positive_eta_increasing`; the opposite sign
  convention remains fail-closed. The tier records run provenance for both
  oracle executions with the concrete image ID, compiler, executable hash,
  consumed fort.* hashes, source manifests and fort.15 output hash. The
  checkout must stay pristine after the run.

Fail-closed step 2d (#71, interpolation oracle contract):

- Regenerate and verify all **eight** sampling cases with the pinned FLEXPART
  11.1 objects. The `horizontal-geographic-interior` case starts from
  longitude/latitude and calls the real pinned `point_mod::coordtrafo` before
  `find_grid_indices`, `find_grid_distances` and `hor_interpol_4d`; this
  makes the documented Lon/Lat -> grid mapping part of the oracle evidence
  instead of a documentation-only formula. This is explicitly an **oracle exercise
  path**, not a pristine production call chain: production `coordtrafo` is used for
  release-point initialization, while runtime particle sampling enters the interpolation
  layer in grid coordinates. The pack also retains the six
  synthetic index/vertical/temporal/rain cases and the real
  `real-era5-etex-temperature-column` case. CI re-packs
  `fixtures/interpolation/contract-v1.json` plus provenance and fails on any
  golden, source/object hash, routine-list, or semantic drift.

Any missing adapter, skipped GPU test, missing oracle artifact, or failed
comparison exits non-zero. Unwired corpus cases are listed as `NOT_WIRED`,
never as `PASS`; no placeholder reports success.

Approximate runtime (cold cache):

| Step | Linux CI (Lavapipe) | Windows local (WARP) |
|------|---------------------|----------------------|
| Oracle image build + Fortran compile | 4–8 min | Docker only; skipped locally with `--skip-oracle-build` |
| `gpu-preflight --software` | <30 s | <10 s |
| `SW-WGPU-ADVECTION-001` (debug test) | 1–3 min | ~35 s (debug), faster in release |
| `fortran-validation` 1000 particles (release) | 1–3 min plus one-time ~1 min compile | ~20 s plus compile |
| Total small gate | ~10–15 min | ~2 min candidate-only |

## 3. Larger corpus (local reproduction, manual CI)

The larger corpus stays local; the full ETEX mini run never executes on
every pull request.

```bash
# Synthetic oracle-versus-candidate (10k particles, 6 h):
scripts/compare-fortran.sh compose validate

# ETEX-1 mini (native ERA5, 12 h, no download, no manual editing):
scripts/run-etex.sh mini

# Repeat only the input audit:
ETEX_PROFILE=mini scripts/run-etex.sh audit

# Pipeline status at any time:
scripts/run-etex.sh status
```

`extended-validation.yml` (`workflow_dispatch` only) provides the same
profiles manually (`synthetic-validate`, `etex-mini-audit`, `etex-mini`)
with a 120-minute timeout and 30-day artifacts. Enable a schedule only
once runtime, data access, and artifact size are proven justifiable; the
checked-in mini fixtures are ~32 MB (four model-level GRIB files plus one
surface archive), while full ERA5 downloads are larger and network-bound.

## 4. Reports, logs, and run manifests as CI artifacts

Every gate run uploads machine-readable reports plus raw logs; all are
traceable to one concrete run via `GITHUB_RUN_ID`/`GITHUB_SHA` (or
`local` plus the candidate revision).

`target/ci-gate/` contains:

- `ci-gate-report.json` (schema v1: gate status, oracle/candidate revisions,
  build environment, adapter, allow-listed cases, `NOT_WIRED` corpus entries,
  input/output hashes, fixed Philox key/counter with a multi-seed disclaimer,
  `scientific_verdict: NOT_EVALUATED`).
- `run-manifest.json` (`PROVENANCE_ONLY_NO_PARITY_VERDICT`: image ID,
  packages, compiler, executable/input/output hashes, adapter line).
- `build-env.txt` (`rustc`, `cargo`, `docker`, Python, OS, revisions).
- `oracle-verify.log`, `oracle-build.log`, `oracle-executable.sha256`.
- `flex-extract-etadot.log`, `flex-extract-oracle/` when step 2c ran
  (`verify.log`, `oracle-build-run.log`, `run-provenance.json`,
  `oracle-json/{snapshot,motion,oracle}.json`, `candidate.json`,
  `comparison-report.json`).
- `interpolation/contract-v1.json`, `interpolation/contract-v1.provenance.json`,
  `interpolation/oracle-build/interpolation-oracle.compiler-version.txt`,
  `interpolation/oracle-build/interpolation-oracle.linked-objects.txt`, direct
  oracle outputs (including `horizontal-geographic-interior.out`) and
  `interpolation/reproducibility-check.log`.
- `gpu-preflight.log`, `sw-wgpu-advection.log`.
- `candidate-run.log`, `candidate-output.json`,
  `candidate-output-check.log`, `candidate-executable.sha256` (when built).
- `ci-gate.log` (full console transcript).

`software-wgpu.yml` additionally uploads its smoke log as
`software-wgpu-smoke-<run_id>`. `validation-gate.yml` uploads the whole
`target/ci-gate/` directory as `ci-technical-gate-<run_id>`.

## 5. Report schema (stable contract for parallel tracks)

`scripts/ci-gate-report.py` writes schema `1.0`:

```json
{
  "schema_version": "1.0",
  "gate_id": "ci-technical-gate",
  "status": "TECHNICAL_PASS",
  "scientific_verdict": "NOT_EVALUATED",
  "oracle": {"pinned_commit": "…", "actual_commit": "…"},
  "etadot_oracle": {
    "manifest": "reference/flex-extract.json",
    "pinned_commit": "…",
    "actual_commit": "…",
    "worktree_clean": true,
    "status": "PASS|FAIL|NOT_WIRED"
  },
  "candidate": {"revision": "…"},
  "adapter": {"name": "llvmpipe", "is_software": true},
  "cases": [{"case_id": "SW-WGPU-ADVECTION-001", "status": "PASS"}],
  "pending_corpus_cases": [{"case_id": "…", "status": "NOT_WIRED"}],
  "input_sha256": {},
  "output_sha256": {},
  "seeds": {"note": "fixed smoke key only; ≥10 seeds required for science"}
}
```

Coordination with the parallel tracks (early contract, no implementation
taken over here):

- The test-corpus track owns `scripts/run-corpus.sh` (`candidate [CASE]
  [--seeds N]`, `oracle [CASE]`, `compare`, `manifest`, `audit`, `all`),
  `src/bin/corpus-run.rs` (`--case`, `--all`, `--seeds`, `--out-dir`),
  `cargo test --test integration corpus_*` (deterministic single-seed CI
  subset, <5 s), and `fixtures/corpus/` (`corpus.json`, `cases/*.json`,
  `fortran/<CASE>/`, `thresholds.json` with `oracle_parity_gates: null`).
  Cases include `ADV-ANA-001`, `WIND-UNI-002`, `WIND-SHEAR-003`,
  `PBL-STABLE-004/005/006`, `DRY-007`, `WET-008`, `REPEAT-009` (plus blocked
  `RESTART-010`, `DECAY-011`, `CONV-012` and `ETEX-MINI-013` via
  `scripts/run-etex.sh mini`). Raw outputs live under
  `target/corpus/candidate/<CASE>/seed_*.json` and
  `target/corpus/oracle/<CASE>/`; the comparison writes
  `target/corpus/comparison_report.json` (mass, COM, covariance/eigenvalues,
  vertical quantiles, overlap, correlation, process budgets, oracle
  diagnostic only) and `target/corpus/run_manifest.json` (hashes, revisions,
  compiler, adapter, seeds). See `docs/corpus-matrix.md` (parallel track).
  This gate only records `corpus_runner: PRESENT_NOT_WIRED_IN_THIS_GATE` or
  `NOT_FOUND_EXPECTED_FROM_PARALLEL_TRACK` and never runs unlisted cases.
- The scientific-metrics track owns `scripts/evaluate/evaluate_case.py`
  (`--case synthetic-uniform-wind|etex-mini|aggregate-seeds` with
  `--fortran-output`, `--gpu-output`, `--output`, `--summary`, plus
  `--candidate-log`, `--seed`, provenance flags) and
  `scripts/evaluate/metrics.py` as the single metric definition site
  (Pearson `None` on zero variance, normalized RMSE, Chang–Hanna FB/NMSE,
  FAC2, equirectangular moments, sign-run sparse decoding). Reports use
  schema `1.0.0` with `overall_status` (`INTEGRITY_ERROR`, `DIAGNOSTIC`,
  `PASS`, `FAIL`), `thresholds` (versioned verdicts), and `missing_metrics`.
  See `docs/evaluation.md` (parallel track). This gate records diagnostics
  only and applies no scientific thresholds.
- Further small cases (for example `ADV-ANA-001` or `PBL-NEUTRAL-005`) are
  bound into `CI_CASE_ALLOWLIST` in `scripts/ci-gate.sh` only once their
  runner and metric format are stable. Corpus tests that skip on a missing
  adapter must be wrapped with an explicit no-skip check (fail on
  `no WGSL adapter — skipping`); no placeholder may report `PASS`.

## 6. Technical gates versus scientific parity

- `TECHNICAL_PASS` means plumbing works: the oracle is pinned and builds,
  the software adapter executes real WGSL, the analytical displacement is
  within 36.0 ± 0.2 km, and the small candidate smoke produces checked
  outputs with recorded provenance.
- It does not mean the candidate matches FLEXPART 11.1 scientifically.
  `compare_concentrations.py` and `compare_oracle_observations.py` already
  report `NOT_EVALUATED` / `METRICS_COMPUTED_NO_PARITY_VERDICT`; this gate
  preserves that separation.
- Versioned scientific thresholds are applied only when inputs and metrics
  are demonstrably suitable. The existing
  `fixtures/etex/mini/input-equivalence-thresholds.json` is explicitly
  limited to technical decoding tolerances, not parity. No new scientific
  thresholds are introduced here.
