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
    --run-manifest target/validation/run_manifest.json \
    --candidate-log target/validation/gpu.log \
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
values. Exit code 2 marks integrity errors, exit code 1 a violated
in-scope gate.

Provenance flags: `--candidate-log` (extracts the single unambiguous wgpu
adapter line), `--seed` / `--oracle-seed`, `--oracle-checkout`
(fail-closed verification of the pinned unmodified oracle),
`--run-manifest` (hash-verified execution provenance, see below),
`--input-equivalence-report` (ETEX input audit to consume, see below),
`--candidate-executable` / `--oracle-executable` (hashes),
`--candidate-revision` (explicitly declared, labeled as unverified),
`--note`.

## Execution provenance (no misattribution)

The candidate revision is only reported when it is tied to the supplied
artifacts, with an explicit `revision_source`:

- `embedded-in-artifact`: every supplied file embeds the same revision
  (corpus `seed_*.json` carry `candidate_revision`; mixed revisions are an
  integrity error);
- `run-manifest-hash-verified`: `--run-manifest` points at an oracle run
  manifest whose input/output hashes match the supplied files;
- `declared-flag`: `--candidate-revision` was given explicitly and is
  labeled as not hash-verified.

Otherwise the revision is `null` with a `candidate.revision` missing entry
naming the gap (including partially unattributed embedded sets: one seed
without a usable embedded revision leaves the whole set unverified rather
than attributing the remainder). The evaluator checkout's HEAD is never
attributed to previously generated outputs. Likewise the oracle output is
only attributed to the pinned oracle (`attribution: run-manifest`) when
every consumed oracle output is hash-covered by a run manifest whose oracle
entry is the pinned clean commit; the same complete-coverage rule applies
to candidate outputs. A verified source checkout alone (`--oracle-checkout`)
proves the sources, recorded as `checkout_verified`, but leaves output
attribution `unverified` because it does not link supplied outputs to a
run.

## Report schema (version 1.0.0)

Top-level keys: `schema_version`, `case_id`, `created_utc`,
`overall_status` (`INTEGRITY_ERROR`, `DIAGNOSTIC`, `PASS`, `FAIL`),
`status_detail`, `time_window` (ISO-8601 UTC bounds, averaging/sampling
seconds, sample count, endpoint weight, window count), `grid` (nx/ny/nz,
origin in degrees, resolution in degrees, heights in metres), `oracle`
(name, version, pinned commit, attribution, checkout verification, seed and
seed controllability), `candidate` (revision, revision source, worktree
dirtiness, adapter, seed and seed controllability), `input_sha256`, `alignment` (grid and window
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
- Horizontal moments use the equirectangular projection around the weighted
  mean latitude with metres per degree shared exactly with the corpus
  runner (`R * pi / 180`; the legacy rounded 111.195 km/deg is superseded)
  for both particle and grid weights.
- Sparse FLEXPART decoding uses value-sign run detection with physical
  values `abs(value)`.

Particle metrics and grid metrics are strictly separated:

- `particle_metrics` holds the per-seed kilogram mass budgets (remaining
  plus dry, wet and decayed deposition versus released mass, each with a
  per-seed PASS/FAIL verdict), particle-space center of mass, horizontal
  covariance/eigenvalues, vertical quantiles and, when the oracle wrote a
  `partposit` dump, the oracle particle ensemble. The mass-closure gate
  evaluates every seed and the worst absolute error governs, so
  opposite-sign errors cannot cancel into a pass (verified: -40%/+40%
  seeds report `MASS_BUDGET_CLOSE: FAIL` with worst error 0.4).
- `grid_metrics` holds the time-averaged concentration shape diagnostic
  (correlation, normalized RMSE, footprint overlap as Figure of Merit in
  Space, and per-level fractions) and mass-weighted spatial moments (center
  distance and covariances). The shape diagnostic compares concentration:
  the candidate mass field (kg per cell) is converted to kg/m3 with
  latitude-dependent cell volumes and OUTHEIGHTS thicknesses before
  normalization. A normalized field comparison is never mass-conservation
  evidence; the legacy mass-against-concentration weighting (correlation
  0.65 on the reference artifacts) is superseded by the
  concentration-against-concentration comparison. For spatial moments, the
  oracle concentration is multiplied by FLEXPART cell volume, the candidate
  stays in kg per cell, and vertical centers use layer midpoints.
- ETEX `etex` holds independent model-versus-observation metrics for both
  models (FB, NMSE, correlation, FAC2 with positive-pair counts) plus
  arrival-time and peak diagnostics in hours (detection threshold
  10 pg/m3), plus the consumed `input_equivalence_audit` record (status and
  SHA-256 of the `--input-equivalence-report` file). Pairs with incomplete
  model coverage are excluded and counted as skipped; negative
  concentrations are integrity errors. Missing measurements are never
  interpolated. Without the audit flag, input equivalence is recorded as
  unverified rather than asserted.

## Thresholds (versioned separately)

`evaluation/thresholds/scientific-thresholds-v1.json` with justification in
`evaluation/thresholds/README.md`. Thresholds live outside
`scripts/evaluate/` so code changes cannot silently move a gate, and they
must never be loosened to make a candidate pass. Version 1 enforces the
kilogram mass-budget closure, field positivity and grid/window alignment;
all gridded shape and ETEX observation comparisons stay diagnostic.

## Multi-seed policy

Stochastic parity needs at least 10 independent seeds with a controllable
oracle seed. Seed files are validated as distinct, correctly derived Philox
identities before any statistics are computed: every file must carry a
unique `seed_index` and a key/counter matching the case derivation
(`[base0 + index, base1]`, zeroed counter; `ADV-ANA-001` uses `[0, 0]`),
and duplicate identities are an integrity error. The only exception is
`REPEAT-009`, whose designed repeat reruns one identity across its files.
`aggregate-seeds` applies the same rule to recorded candidate seeds and
marks independence `unverified` when reports record no seeds. Aggregation
reports mean, sample standard deviation, min, max, median and an approximate
95% confidence interval per metric. Runs without a controllable oracle seed
are labeled `NOT_SUITABLE_FOR_MULTISEED_PARITY` and stay diagnostic. The
current runners do not expose seeds, so all present results are single-seed
diagnostics.

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
cross-checked per seed on an unweighted basis with unit-aware tolerances;
the check verdict is `CONSISTENT` or `DIVERGENT`, and a divergence is an
integrity error for the whole report; the geographic conversion is shared
exactly as `R * pi / 180`, verified by a test built from the Rust formula):

| Runner `metrics` (m^2) | Evaluation (`metrics.py`, km^2 unless noted) |
|---|---|
| `total_mass_kg`, `mass_conservation_rel_error` | `mass_budget` (kg; gate `MASS_BUDGET_CLOSE` 1e-5 matches corpus `mass_conservation_rel`; worst seed governs) |
| `com_lon_deg`, `com_lat_deg`, `com_z_m` | `center_of_mass_particles` (deg, deg, m; unweighted basis for the check) |
| `cov_east_m2`, `cov_north_m2`, `horizontal_eigenvalues_m2` | `horizontal_covariance` (km^2; 1 km^2 = 1e6 m^2; unweighted basis for the check) |
| `z_p10_m`, `z_p50_m`, `z_p90_m`, `z_mean_m`, `z_std_m` | `unweighted_quantiles_linear` for the check (formula-identical to the runner); scientific reporting uses mass-weighted `vertical_quantiles` |

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

Synthetic uniform-wind evaluation of the local paired run
(`overall_status: DIAGNOSTIC`):

- Mass budget closes: released 1.0 kg versus remaining 1.0 kg, relative
  error +2.1e-9 (`MASS_BUDGET_CLOSE: PASS`).
- Grid/window alignment holds (`GRID_WINDOW_ALIGNMENT: PASS`).
- Concentration-against-concentration shape: correlation 0.956, normalized
  RMSE 6.59 (large because the denominator over a sparse normalized field
  is tiny; reported as-is, not tuned), footprint overlap (FMS) 0.679 over
  168 union cells. This shape correlation uses a different normalization
  from the mass-per-cell correlation of 0.919 in `validation-report.md`.
- Mass-weighted center distance 0.204 km; covariance eigenvalue ratios
  (candidate over oracle) 1.68 and 1.18, outside the ±10% parity band: single-seed
  diagnostic, no parity verdict.
- Vertical level fractions are reported per layer; the candidate puts more
  mass into the 1500-3000 m layers than the oracle in this run.
- Missing as named: oracle `partposit` (no dump written) and candidate seed
  (not exposed). The local run manifest ties both outputs to their revisions;
  the candidate log identifies the software adapter.

Review-fix verification (all reproduced locally against the reference
artifacts):

- Canceling mass errors (-40%/+40% across two seeds) report
  `MASS_BUDGET_CLOSE: FAIL` with worst error 0.4 and overall `FAIL`.
- Ten files repeating one seed identity are rejected with
  `INTEGRITY_ERROR` (duplicate `seed_index` / Philox identity).
- A 1.5e6 m^2 eigenvalue perturbation in a runner block reports
  `runner cross-check DIVERGENT` with overall `INTEGRITY_ERROR`.
- A nonexistent `--fortran-output` directory fails closed with
  `INTEGRITY_ERROR` instead of a candidate-only `DIAGNOSTIC`.

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
`scripts/etex/audit_input_equivalence.py`. This evaluation consumes that
audit via `--input-equivalence-report` (recording its status and hash) but
does not change it, and claims no concentration parity. Without the audit
flag, input equivalence is recorded as unverified rather than asserted.
