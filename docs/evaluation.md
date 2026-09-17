# Scientific metrics and evaluation (RISK-03.3G-01, item 3)

One documented command evaluates a case into a versioned JSON report plus a
readable summary. Reports are comparable across runs and name missing
metrics explicitly.

## Commands

Synthetic uniform-wind oracle comparison:

```bash
python scripts/evaluate/evaluate_case.py --case synthetic-uniform-wind \
    --fortran-output target/comparison/validate_run/output \
    --gpu-output target/validation/gpu_concentration.json \
    --output results/evaluation/synthetic-uniform-wind/report.json \
    --summary results/evaluation/synthetic-uniform-wind/summary.txt
```

ETEX paired observation comparison:

```bash
python scripts/evaluate/evaluate_case.py --case etex-mini \
    --measurements target/etex/mini/measurements.json \
    --fortran-output target/etex/mini/fortran_run/output \
    --gpu-output target/etex/mini/gpu_output.json \
    --output results/evaluation/etex-mini/report.json \
    --summary results/evaluation/etex-mini/summary.txt
```

Multi-seed aggregation (once the test corpus delivers independent seeds):

```bash
python scripts/evaluate/evaluate_case.py --case aggregate-seeds \
    --seed-reports results/evaluation/<case>/seed_*.json \
    --output results/evaluation/<case>/aggregated.json \
    --summary results/evaluation/<case>/aggregated-summary.txt
```

Corpus particle ensembles (Issue #6 test corpus, per-seed particle files):

```bash
python scripts/evaluate/evaluate_case.py --case corpus-seeds \
    --case-def fixtures/corpus/cases/WIND-UNI-002.json \
    --candidate-seeds "target/corpus/candidate/WIND-UNI-002/seed_*.json" \
    --fortran-output target/corpus/oracle/WIND-UNI-002 \
    --output results/evaluation/WIND-UNI-002/report.json \
    --summary results/evaluation/WIND-UNI-002/summary.txt
```

Unit tests for the formulas (analytical fields, no oracle needed):

```bash
python scripts/evaluate/test_metrics.py
python scripts/evaluate/test_evaluate.py
```

The evaluation needs only the Python standard library. A non-zero exit
means failure: incompatible inputs or missing oracle artifacts produce an
`INTEGRITY_ERROR` report instead of silently interpolating or substituting
values.

Useful provenance flags: `--candidate-log` (extracts the single unambiguous
wgpu adapter line), `--seed` / `--oracle-seed`, `--candidate-checkout`,
`--oracle-checkout` (fail-closed verification of the pinned unmodified
oracle), `--candidate-executable` / `--oracle-executable` (hashes),
`--candidate-revision`, `--note`.

## Report schema (version 1.0.0)

Top-level keys: `schema_version`, `case_id`, `created_utc`,
`overall_status` (`INTEGRITY_ERROR`, `DIAGNOSTIC`, `PASS`, `FAIL`),
`status_detail`, `time_window` (ISO-8601 UTC bounds, averaging/sampling
seconds, sample count, endpoint weight, window count), `grid` (nx/ny/nz,
origin in degrees, resolution in degrees, heights in metres), `oracle`
(name, version, pinned commit, checkout verification, seed and seed
controllability), `candidate` (revision, worktree dirtiness, adapter, seed
and seed controllability), `input_sha256`, `alignment` (grid and window
checks), `particle_metrics`, `grid_metrics`, `etex`, `multiseed`,
`thresholds` (version plus per-criterion verdicts), `missing_metrics`
(name plus reason for every unavailable metric), `notes`.

## Unified metric definitions

`scripts/evaluate/metrics.py` is the single definition site. It adopts the
correct calculations from `scripts/compare_concentrations.py` and
`scripts/etex/compare_oracle_observations.py` and retires the contradictions
in the older helpers (`compare_etex_fortran_obs.py`, `compare_gpu_obs.py`,
`compare_with_observations.py`):

- Pearson correlation returns `None` on zero variance (the legacy `0.0`
  fallback is rejected as misleading).
- Gridded normalized RMSE is `rmse / max(mean|a|, mean|b|)` with an explicit
  denominator label (the `rmse / max(obs)` and `rmse / range(obs)` variants
  are rejected for grid-to-grid comparison).
- ETEX fractional bias and NMSE follow Chang and Hanna (2004) with `None`
  on non-positive denominators; FAC2 uses strictly positive pairs with
  `0.5 <= modeled/observed <= 2.0`.
- Horizontal moments use the `111.195 km/deg` equirectangular projection
  around the weighted mean latitude for both particle and grid weights.
- Sparse FLEXPART decoding uses value-sign run detection with physical
  values `abs(value)`.

Particle metrics and grid metrics are strictly separated:

- `particle_metrics` holds the kilogram mass budget (remaining plus dry,
  wet and decayed deposition versus released mass), particle-space center
  of mass, horizontal covariance/eigenvalues, vertical quantiles and, when
  the oracle wrote a `partposit` dump, the oracle particle ensemble.
- `grid_metrics` holds the time-averaged concentration shape diagnostic
  (correlation, normalized RMSE, footprint overlap as Figure of Merit in
  Space, concentration-weighted center distance, covariances, per-level
  fractions). Grid diagnostics compare concentration against concentration:
  the candidate mass field (kg per cell) is converted to kg/m3 with
  latitude-dependent cell volumes and OUTHEIGHTS thicknesses before
  normalization. A normalized field comparison is never mass-conservation
  evidence; the legacy mass-against-concentration weighting (correlation
  0.65 on the reference artifacts) is superseded by the
  concentration-against-concentration comparison (correlation 0.90 on the
  same artifacts).
- ETEX `etex` holds independent model-versus-observation metrics for both
  models (FB, NMSE, correlation, FAC2 with positive-pair counts) plus
  arrival-time and peak diagnostics in hours (detection threshold
  10 pg/m3). Pairs with incomplete model coverage are excluded and counted
  as skipped; negative concentrations are integrity errors. Missing
  measurements are never interpolated.

## Thresholds (versioned separately)

`evaluation/thresholds/scientific-thresholds-v1.json` with justification in
`evaluation/thresholds/README.md`. Thresholds live outside
`scripts/evaluate/` so code changes cannot silently move a gate, and they
must never be loosened to make a candidate pass. Version 1 enforces the
kilogram mass-budget closure, field positivity and grid/window alignment;
all gridded shape and ETEX observation comparisons stay diagnostic.

## Multi-seed policy

Stochastic parity needs at least 10 independent seeds with a controllable
oracle seed. Aggregation reports mean, sample standard deviation, min, max,
median and an approximate 95% confidence interval per metric. Runs without
a controllable oracle seed are labeled `NOT_SUITABLE_FOR_MULTISEED_PARITY`
and stay diagnostic. The current runners do not expose seeds, so all
present results are single-seed diagnostics.

## Artifact contract with the parallel test-corpus work

The test corpus (Issue #6, point 2) owns its fixtures and scenario runner;
this evaluation does not modify them (`fixtures/corpus/`,
`scripts/corpus/`, `src/bin/corpus-run.rs`, `scripts/run-corpus.sh`,
`scripts/generate_synthetic_grib.py` shear extension). Early coordination
record (this section) fixes the shared formats:

- Case inputs: `fixtures/corpus/cases/<CASE>.json` (versioned; release mass
  in kg, physics switches, Philox seed derivation, units per field).
- Candidate outputs: `target/corpus/candidate/<CASE>/seed_*.json`, one file
  per seed with `case_id`, `seed_index`, `philox_key`/`philox_counter`,
  `adapter`, `candidate_revision` and per-particle `lon_deg`/`lat_deg`/`z_m`
 /`mass_kg` records plus the runner's own `metrics` block.
- Oracle outputs: `target/corpus/oracle/<CASE>/` in standard FLEXPART
  layout (`header`, `dates`, `grid_conc_*`, `partposit_*` where written).
- Provenance: `target/corpus/run_manifest.json` (input/output hashes,
  oracle commit, candidate revision, adapter, seeds).
- ETEX mini stays the real-weather member with
  `INPUT_EQUIVALENCE_NOT_DEMONSTRATED`; no green corpus check overrides
  that audit.

The `corpus-seeds` command consumes exactly these files:

```bash
python scripts/evaluate/evaluate_case.py --case corpus-seeds \
    --case-def fixtures/corpus/cases/WIND-UNI-002.json \
    --candidate-seeds "target/corpus/candidate/WIND-UNI-002/seed_*.json" \
    --fortran-output target/corpus/oracle/WIND-UNI-002 \
    --output results/evaluation/WIND-UNI-002/report.json \
    --summary results/evaluation/WIND-UNI-002/summary.txt
```

Definition mapping (single canonical meaning, two implementations that are
cross-checked per seed and reported as `runner_metrics_cross_check`):

| Runner `metrics` (m^2) | Evaluation (`metrics.py`, km^2 unless noted) |
|---|---|
| `total_mass_kg`, `mass_conservation_rel_error` | `mass_budget` (kg; gate `MASS_BUDGET_CLOSE` 1e-5 matches corpus `mass_conservation_rel`) |
| `com_lon_deg`, `com_lat_deg`, `com_z_m` | `center_of_mass_particles` (deg, deg, m) |
| `cov_east_m2`, `cov_north_m2`, `horizontal_eigenvalues_m2` | `horizontal_covariance` (km^2; 1 km^2 = 1e6 m^2) |
| `z_p10_m`, `z_p50_m`, `z_p90_m`, `z_mean_m`, `z_std_m` | `vertical_quantiles` (mass-weighted first-reach convention; the runner uses unweighted linear-index interpolation, so small differences up to one inter-particle spacing are expected and reported, not hidden) |

Threshold mapping: corpus `mass_conservation_rel` 1e-5 is the same gate as
`MASS_BUDGET_CLOSE`; corpus `oracle_parity_gates: null` matches this
evaluation's diagnostic-only oracle comparisons.

## Measured results on existing artifacts

Reference artifacts (pinned oracle `c70586c2b7f5258850705325881c61f557ea9bd8`):

- Oracle: `target/comparison/validate_run/output` (12 `grid_conc_*` windows,
  `header`, `dates`; no `partposit` dump).
- Candidate: `target/validation/gpu_concentration.json` (32x32x10 grid,
  05:30-06:00 UTC averaging window, 3 samples with half-weight endpoints).
- Measurements: `fixtures/etex/data` parses to 168 stations and 3105
  records (30.3% detection rate, maximum 12570 pg/m3 at station 8021).

Synthetic uniform-wind evaluation (`overall_status: DIAGNOSTIC`):

- Mass budget closes: released 1.0 kg versus remaining 1.0 kg, relative
  error -9e-10 (`MASS_BUDGET_CLOSE: PASS`).
- Grid/window alignment holds (`GRID_WINDOW_ALIGNMENT: PASS`).
- Concentration-against-concentration shape: correlation 0.90, normalized
  RMSE 11.6 (large because the denominator over a sparse normalized field
  is tiny; reported as-is, not tuned), footprint overlap (FMS) 0.70 over
  128 union cells.
- Center distance 0.24 km; covariance eigenvalue ratios (candidate over
  oracle) 2.08 and 1.30, outside any ±10% parity band: single-seed
  diagnostic, no parity verdict.
- Vertical level fractions are reported per layer; the candidate puts more
  mass into the 1500-3000 m layers than the oracle in this run.
- Missing as named: oracle `partposit` (no dump written), candidate adapter
  (no log supplied), candidate seed (not exposed).

ETEX path verification used a minimal synthetic 2x2 fixture (one window,
one station pair): oracle-versus-observation FB 0.0 / NMSE 0.0 / FAC2 1.0
and candidate-versus-observation FB -1.81 / NMSE 18.3 / FAC2 0.0, with
correlation `None` on the single pair. Missing-input and grid-mismatch
runs fail closed with exit code 2 and an `INTEGRITY_ERROR` report.

## ETEX mini status

The existing ETEX mini run remains a pipeline and observation-pairing
regression. Its input audit still reports `INPUT_EQUIVALENCE_NOT_DEMONSTRATED`
(137 native hybrid levels with eta-coordinate velocity versus 16 fixed AGL
levels with omega-derived velocity, plus differing timesteps and PBL
diagnostics); see `fixtures/etex/mini/README.md` and
`scripts/etex/audit_input_equivalence.py`. This evaluation does not change
that audit and claims no concentration parity.
