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
python scripts/evaluate/test_invariants.py
python scripts/evaluate/test_distributional.py
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
`--metric-contract` (Issue #55 distributional metric contract, see below),
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

## Report schema (version 1.2.0)

Top-level keys: `schema_version`, `case_id`, `created_utc`,
`overall_status` (`INTEGRITY_ERROR`, `DIAGNOSTIC`, `PASS`, `FAIL`),
`status_detail`, `time_window` (ISO-8601 UTC bounds, averaging/sampling
seconds, sample count, endpoint weight, window count), `grid` (nx/ny/nz,
origin in degrees, resolution in degrees, heights in metres), `oracle`
(name, version, pinned commit, attribution, checkout verification, seed and
seed controllability), `candidate` (revision, revision source, worktree
dirtiness, adapter, seed and seed controllability), `input_sha256`, `alignment` (grid and window
checks), `particle_metrics`, `grid_metrics`, `etex`, `multiseed`,
`thresholds` (version plus per-criterion verdicts),
`deterministic_invariants` (Issue #54 gate block, see below; `null` means
not evaluated and must be read as `BLOCKED`), `missing_metrics` (name plus
reason for every unavailable metric), `notes`, and
`distributional_metrics` (Issue #55 metric records, see below; `null` means
not extracted because the evaluation stopped). Version 1.2.0 is a strict
superset of 1.1.0, which is a strict superset of 1.0.0; the #54 invariant
contract still names 1.1.0, the schema that introduced its block
(`report.SUPERSET_CHAIN`). `aggregate-seeds` accepts only reports of the
current version and contracts; regenerate 1.1.0 reports, which carry no
metric records.

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

- `particle_metrics` holds the per-seed kilogram mass budgets (the
  canonical `INV-MASS-CLOSURE` record: airborne plus every reservoir of a
  process the case enables, versus released mass, with its per-seed
  verdict), particle-space center of mass, horizontal
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
all gridded shape and ETEX observation comparisons stay diagnostic. The
deterministic-invariant contract references these criteria (and the
corpus `analytic_advection_*` bounds) by id and reads their values from
the threshold files; it versions no threshold of its own.

## Deterministic invariant gate (Issue #54)

`scripts/evaluate/invariants.py` is the single evaluation path for the
versioned contract
[`evaluation/invariants/deterministic-invariants-v1.json`](../evaluation/invariants/deterministic-invariants-v1.json)
(`flexpart-gpu.deterministic-invariants` v1). Every `evaluate_case.py`
report embeds its result as `deterministic_invariants`; `--invariants`
selects the contract file. Invariants are unary per realization (one
candidate `seed_*.json`, or one oracle output directory) except the
ensemble exact-repeat check. They are hard gates: no stochastic spread,
averaging or favorable aggregate statistic can excuse them.

| Invariant (v1) | Family | Applies when (#51 case) | Comparison |
|---|---|---|---|
| `INV-FIELD-FINITE` | finite fields | always | exact, every cell |
| `INV-FIELD-NONNEGATIVE` | non-negative fields | always | exact, no epsilon (`MASS_POSITIVITY`) |
| `INV-MASS-CLOSURE` | mass/reservoir closure | `simulation_direction` forward | `MASS_BUDGET_CLOSE` (1e-5, abs_lt) |
| `INV-DISABLED-DRY-DEPOSITION-ZERO` | disabled physics | dry deposition off | exact zero, else closure bound |
| `INV-DISABLED-WET-DEPOSITION-ZERO` | disabled physics | wet deposition off | exact zero, else closure bound |
| `INV-DISABLED-DECAY-NO-LOSS` | disabled physics | decay off | exact zero, else closure bound |
| `INV-ANALYTIC-UNIFORM-ADVECTION` | analytic process | no turbulence/convection/deposition/decay, uniform wind, point instant release, forward, deterministic | corpus `analytic_advection_*` (360/1/0.01 m), worst particle, scope `ADV-ANA-001` |
| `INV-ANALYTIC-DECAY` | analytic process | decay on | unavailable (BLOCKED) |
| `INV-DETERMINISTIC-REPEAT` | deterministic process | Philox derivation `reuse_base_identity_v1` | exact equality |
| `INV-CASE-IDENTITY` | output consistency | always | exact |
| `INV-OUTPUT-UNITS` | output consistency | always | kg, kg/m3, time-averaged mass concentration |
| `INV-PARTICLE-COUNT` | output consistency | always (candidate) | exact |
| `INV-OUTPUT-GRID` | output consistency | `output_grid` declared | `report.check_grid_alignment` (1e-5) |
| `INV-OUTPUT-CHRONOLOGY` | output consistency | `output`/`integration` declared | exact start, interval, averaging, sampling, `start + k*interval_s` stamps |
| `INV-PROVENANCE-BINDING` | evidence requirements | always | one #53 execution per realization |

Each definition records meaning, roles, required artifacts, quantities and
units, normalization, owner, comparison policy, threshold reference and the
PASS/FAIL/missing-evidence/integrity behavior. The case manifest decides
whether a process is declared; the contract decides how it is checked.

### Verdicts and precedence

- `PASS`: applicable, evidence verified, assertion holds.
- `FAIL`: applicable, evidence usable, assertion violated
  (`MASS_DEFICIT`, `MASS_EXCESS`, `DISABLED_PROCESS_NONZERO_RESERVOIR`,
  `UNEXPLAINED_MASS_LOSS`, `ANALYTIC_DISPLACEMENT_EXCEEDED`,
  `REPEAT_NOT_IDENTICAL`).
- `INTEGRITY_ERROR`: malformed, non-finite, negative (v1
  `MASS_POSITIVITY` classifies negatives as integrity errors),
  contradictory, hash-mismatched or misattributed evidence. Evaluation
  stops before any statistic is computed and the report keeps the
  offending field/cell records.
- `NOT_APPLICABLE`: the validated case explicitly excludes the invariant.
  A missing case manifest or field is `BLOCKED`, never `NOT_APPLICABLE`.
- `BLOCKED`: applicable but evidence or capability is unavailable.

Precedence: `INTEGRITY_ERROR > FAIL > BLOCKED > PASS > NOT_APPLICABLE`;
an empty set is `BLOCKED`. Every record keeps a stable `error_id` from the
contract's closed `error_ids` catalog. The gate lists every FAIL,
INTEGRITY_ERROR and BLOCKED entry next to the aggregate. Report effect:
invariant `FAIL` makes the report `FAIL` (exit 1), `INTEGRITY_ERROR` makes
it `INTEGRITY_ERROR` (exit 2); `PASS` and `BLOCKED` never raise
`overall_status` above `DIAGNOSTIC`.

### Evidence, #52 and #53 prerequisites

- Candidate: raw seed-file values (no coercion), `particle_count`,
  `active_particles`, `deposited_dry_kg`, `deposited_wet_kg`, optional
  `decayed_kg`, the declared initial mass. A missing ledger is never zero.
- Oracle: binary `header` and every `grid_conc_*` concentration, wet and
  dry block (`io_fortran.read_grid_conc_blocks` rejects truncated,
  inconsistent or trailing records as `ORACLE_OUTPUT_INCOMPLETE`), plus a
  `partposit_*` dump when present. No regridding or resampling happens.
  FLEXPART's sparse format uses the value sign as a run delimiter, so
  decoded oracle cells are non-negative by construction; the finite check
  still covers every cell.
- Contract identity: `contract_sha256` hashes canonical JSON (sorted keys,
  compact separators), so checkouts with different line endings agree.
- #53: every consumed file goes through `build_provenance` /
  `run_provenance.verify_artifact_set` before evaluation;
  `run_provenance.executions_for_artifact` binds each realization to
  exactly one execution whose role, case id, case-manifest hash and
  `seed_index` must agree. Two realizations claiming one execution are an
  integrity error. Without a manifest, or with a legacy one, the binding is
  `BLOCKED`.
- #52: `paired_scoring` is separate from the unary verdict:
  `NOT_REQUESTED` (no oracle), `BLOCKED` (oracle without
  `--require-input-equivalence`, or `INPUT_EQUIVALENT` without complete #53
  binding), `ELIGIBLE` (both). A refused gate stops with `INPUT_MISMATCH`,
  `INPUT_EQUIVALENCE_NOT_DEMONSTRATED` or
  `INPUT_EQUIVALENCE_INTEGRITY_ERROR`. Invariants never manufacture
  equivalence, and eligibility is not parity.

### Runner diagnostics versus canonical verdicts

`corpus-run` writes `budget_closure_rel_error` and `budget_status`; they
remain runner-local diagnostics (`runner_budget_status` in the per-seed
record). The canonical verdict is `INV-MASS-CLOSURE`, recomputed from the
case inventory and the recorded ledgers with `metrics.mass_budget`. The
legacy `MASS_BUDGET_CLOSE` threshold evaluation consumes the worst
evaluated closure error.

### Unavailable evidence and physics (BLOCKED, follow-up owned elsewhere)

- Oracle kilogram mass ledger: FLEXPART 11.1 binary output carries
  mass-proportional grid sums only, so oracle closure and the oracle
  disabled-decay bound are `BLOCKED`.
- Oracle particle positions: `partposit` is NetCDF-only in 11.1, so oracle
  analytic advection is `BLOCKED` unless a dump is supplied.
- Candidate gridded output: corpus seed files carry end-state particles
  only, so candidate grid and chronology checks are `BLOCKED`.
- Decay: no species contract binds a half-life, `corpus-run` writes no
  decay ledger and no decay threshold is versioned; `INV-ANALYTIC-DECAY`
  is `BLOCKED` whenever decay is enabled (DECAY-011).
- Particle termination ledger: deactivated particles make closure
  `BLOCKED` (`MISSING_TERMINATION_RESERVOIR`), never dry deposition.
- Backward runs: closure is `NOT_APPLICABLE`; chronology is `BLOCKED`.
- `synthetic-uniform-wind` and ETEX consume no #51 case manifest, so only
  field invariants run there; case-dependent invariants stay `BLOCKED`.

On the local ADV-ANA-001 artifacts (candidate revision
`7e71838b04ad58d8e8638b5610322d21e89600a7`, pinned-oracle output without a
run manifest) the candidate passes every evaluable invariant, including
analytic advection for all 1024 particles, the oracle passes field,
deposition-zero, grid and chronology checks, and the aggregate is
`BLOCKED` because provenance, the oracle kg ledger and candidate gridded
output are unavailable.

### Downstream handoff (#55-#59)

Consumers read `deterministic_invariants`, keep every realization record
and aggregate with `invariants.aggregate_realizations` (or the contract
precedence). Distributional or ensemble scoring may proceed only when the
aggregate is `PASS` and `paired_scoring.status` is `ELIGIBLE`. A
per-seed FAIL, INTEGRITY_ERROR or BLOCKED is never averaged away:
`aggregate-seeds` already carries every seed's realizations and fails when
any seed fails. The #55 metric records apply this rule per record
(`eligible_for_paired_scoring`); see
[Distributional metric contract](#distributional-metric-contract-issue-55).

## Distributional metric contract (Issue #55)

`scripts/evaluate/distributional.py` is the single extraction path for the
versioned contract
[`evaluation/metrics/distributional-metrics-v1.json`](../evaluation/metrics/distributional-metrics-v1.json)
(`flexpart-gpu.distributional-metrics` v1, report schema 1.2.0, normative
oracle FLEXPART 11.1 `c70586c2b7f5258850705325881c61f557ea9bd8`). It defines
what each metric means, which evidence it needs and how it is represented.
It defines no threshold, ensemble statistic, confidence interval, margin or
verdict. Distributional similarity never replaces #52 input equivalence,
#53 provenance or the #54 hard invariants, and a computed metric never
declares scientific equivalence.

### Inventory of existing metric code

| Metric family | Existing function | Units / weighting | Required evidence | Limitation found | Action |
|---|---|---|---|---|---|
| Center of mass (particles) | `metrics.center_of_mass_particles` | deg, m; declared weights | particle lon/lat/z, masses | naive longitude mean | wrapped; wrap-spanning ensembles UNSUPPORTED |
| Center of mass (grid) | `metrics.center_of_mass_grid`, `horizontal_covariance_grid` means | deg, m; cell mass | grid, OUTHEIGHTS | needs mass, not concentration | wrapped with volume/area weights (`io_gpu.cell_volume_m3`) |
| Covariance / eigenvalues | `metrics.horizontal_covariance(_grid)` | km^2, population | positions + weights | no orientation; no negative-eigenvalue check | wrapped; `principal_axes` added |
| Projection | inside `horizontal_covariance` | km, R = 6371 km | - | not reusable for radii | extracted to `local_tangent_offsets_km` (same formula) |
| Vertical quantiles | `metrics.vertical_quantiles` | m, mass-weighted first reach | heights + weights | zero-weight entries affect brackets | body moved to `weighted_quantiles` (identical math); contract excludes zero weights |
| Runner quantiles | `metrics.unweighted_quantiles_linear` | m, unweighted | heights | cross-check convention only | retained, not a contract metric |
| Field comparison | `metrics.field_comparison_metrics`, `pearson_correlation` | field units | aligned fields | none (correlation `None` on zero variance) | wrapped |
| FB / NMSE | `metrics.etex_station_metrics` | dimensionless | non-negative pairs | ETEX-named but domain-agnostic | reused for grid cells |
| Footprint overlap | `metrics.footprint_overlap` | cells | aligned fields | threshold 0 implicit in the evaluator | wrapped as explicit `support_overlap`; threshold-free mass footprints added |
| Mass budget | `metrics.mass_budget` via `INV-MASS-CLOSURE` | kg | #54 ledger | - | read from the #54 record, never recomputed |
| Seed aggregation | `metrics.aggregate_seed_values` | - | - | ensemble statistics belong to #58 | retained for the legacy `multiseed` block; not used by the contract |
| Legacy `grid_metrics.vertical_profile` | in `run_synthetic` | concentration sums | grid | fractions of summed concentration, not mass | retained for compatibility; superseded by `grid.vertical.layer_fraction` |
| Oracle partposit moments | in `run_corpus_seeds` | `release/N` per particle | partposit | count weights labeled as mass | retained in the legacy block; contract labels them `particle_count` |

New functions in `metrics.py`: `local_tangent_offsets_km`, `principal_axes`,
`weighted_quantiles`, `mass_containment_footprint`,
`area_weighted_set_overlap`, `normalized_mass_overlap`. No existing formula
was changed; `test_metrics.py` still passes unchanged.

### Stable metric identifiers (definition version 1)

| Representation | Metric ids |
|---|---|
| Particle space (unary) | `particle.horizontal.center_of_mass`, `particle.vertical.mean_height`, `particle.horizontal.covariance`, `particle.horizontal.principal_axes`, `particle.horizontal.radial_containment` (r50/r90/r95), `particle.vertical.quantiles` (5-95%, IQR, p10-p90), `particle.vertical.layer_fraction`, `particle.vertical.fraction_above_mixing_height` (UNSUPPORTED) |
| Gridded field (unary) | `grid.horizontal.center_of_mass`, `grid.vertical.mean_height`, `grid.horizontal.covariance`, `grid.horizontal.principal_axes`, `grid.horizontal.radial_containment`, `grid.horizontal.mass_footprint_area` (50%/90%), `grid.horizontal.hotspot`, `grid.vertical.layer_fraction`, `grid.field.integrated_total` |
| Gridded field (paired) | `paired.grid.field_statistics`, `paired.grid.shape_statistics`, `paired.grid.support_overlap`, `paired.grid.mass_footprint_overlap`, `paired.grid.center_separation` |
| Ledger (unary) | `budget.mass_closure`, `budget.deposited_mass_total` |

Particle and grid metrics never share an identifier, and there are no
paired particle metrics: pairing candidate and oracle particle ensembles is
ensemble work for #58. Each definition records meaning, quantities, allowed
weightings, outputs with units, required artifacts, aggregation domain,
time window, normalization, quantile convention, missing-data and
degenerate behavior, owner and process references.

### Units, weighting and conventions

- Weightings: `particle_mass_kg` (declared kg masses; any other unit is
  `WEIGHT_UNIT_INVALID`), `particle_count` (each particle 1; never labeled
  mass), `cell_mass_proportional` (value*volume, value*area or value per
  cell from the field's declared `measure`), `cell_sample_unweighted` (one
  sample per aligned cell), `none` (ledger values). Candidate seeds yield
  both particle weightings; the oracle `partposit` reader decodes no masses,
  so oracle particle records are count-weighted only.
- Horizontal distances use the equirectangular local-tangent projection at
  the weighted mean latitude with R = 6371 km, identical to `corpus-run`.
  Covariances are population moments (divide by total weight) in km^2;
  eigenvalues are reported major first; the azimuth is degrees clockwise
  from north in [0, 180) and `null` when isotropic
  (`major - minor <= max(1e-9 * trace, 1e-12 km^2)`). A minor eigenvalue
  below `-1e-12 * trace` is an `INTEGRITY_ERROR`; smaller negatives are
  roundoff, set to 0 and flagged `roundoff_clamped`.
- Quantiles: stable ascending sort, first entry whose cumulative weight
  fraction reaches q, linear interpolation to the preceding entry,
  zero-weight entries excluded. Radial radii use distances from the
  centroid of the same weighting.
- Mass footprints rank columns by column mass per cell area and add whole
  cells until the fraction is reached (ties by lowest index); no
  concentration cutoff is involved. `support_overlap` is the only
  threshold-based overlap and its rule (`value > 0`) is declared.
  `mass_footprint_overlap` is area-weighted IoU; `normalized_mass_overlap`
  is `sum(min(p_c, p_o))`.
- Vertical metrics require a declared reference (`agl`/`asl`); layer
  fractions require the layer reference to equal the height reference.
  FLEXPART OUTHEIGHTS, `ztra1` and candidate `Particle.pos_z` are AGL.
  Grid layer fractions use actual layer volumes, so non-uniform layers are
  handled.
- FLEXPART 11.1 binary deposition is `1e12 * mass / area`, accumulated since
  the run start (`binary_output_mod.f90:1200-1233`; the deposition grids are
  zeroed only in `outgrid_init`, `outgrid_mod.f90:344`, called once from
  `FLEXPART.f90:358`). Oracle deposition fields
  are therefore `per_area` with window `accumulated_since_start`; their
  kilogram totals are `UNSUPPORTED/FIELD_MASS_SCALE_UNDECLARED` because the
  scale factors have no versioned output contract (#40). Concentration
  fields are `time_average` windows ending at the file stamp.
- Absolute paired statistics require one unit and measure; shape-only
  statistics accept a unit scale difference for the same quantity. A
  correlation of a constant field is `null`, never 1.0; a perfect
  correlation or shape match is not a magnitude or mass statement.

### Record format and states

`distributional_metrics.records[]` holds one record per metric, realization
(or candidate/oracle pair), weighting, quantity and field: `metric_id`,
`definition_version`, `contract_version`, `classification`
(`diagnostic`/`paired`), `representation`, `role`, `status`, `reason_code`,
`reason`, `value` (only when `COMPUTED`), `units`, `weighting`,
`normalization`, `quantity`, `species`, `vertical_reference`, `parameters`,
`case` (id + manifest SHA-256), `time_window`, `aggregation_domain`,
`field_ids`, `source_artifacts` (labels + SHA-256), `comparability_key`,
the realization identity (role, seed index, #53 execution identity, #54
verdict, stochastic namespace `candidate_philox` or
`flexpart_oracle_internal`), for paired records the #52 input-equivalence
identity, and `eligible_for_paired_scoring`. The block carries the contract
identity, the #54 aggregate verdict, the paired-scoring status and counts
per state. Reports are written with `allow_nan=False`.

| State | Meaning |
|---|---|
| `COMPUTED` | evidence valid and compatible |
| `DEGENERATE` | evidence valid, metric undefined (`ZERO_TOTAL_WEIGHT`, `ZERO_VARIANCE`, `ZERO_DENOMINATOR`, `EMPTY_FOOTPRINT`); no substitute center or value |
| `MISSING_EVIDENCE` | particle dump, masses, grid, reservoir, ledger or layer geometry absent; never reconstructed |
| `INCOMPATIBLE` | grid, unit, quantity, species, window, vertical reference, transformation or contract version differs |
| `BLOCKED` | #54 aggregate not `PASS`, #52 not `INPUT_EQUIVALENT`, or #53 binding incomplete |
| `INTEGRITY_ERROR` | non-finite/negative weights, invalid coordinates, wrong weight unit, incomplete field, undeclared layout, negative eigenvalue, mutated artifact hash, mixed execution provenance |
| `UNSUPPORTED` | defined but no attributable evidence yet (mixing height, wrap-spanning domains, undeclared kg scale) |

### Prerequisites and grid transformations

- Unary records are extracted only after #53 verification and only when no
  #54 `INTEGRITY_ERROR` stopped the evaluation. Each record carries its
  realization's #54 verdict, so a `FAIL` stays visible next to good metrics.
  Source artifact hashes are re-checked at extraction.
- Paired records are computed only when the #54 aggregate is `PASS` and
  `paired_scoring.status` is `ELIGIBLE`; otherwise they are `BLOCKED` with
  `value: null`. Compatibility and integrity are checked first.
- `eligible_for_paired_scoring` is true only for `COMPUTED` records of an
  evaluation whose #54 aggregate is `PASS` and paired scoring `ELIGIBLE`.
  The block never changes `overall_status`.
- Only the identity transformation is permitted. Different grids are
  `GRID_MISMATCH`; a declared non-identity transformation is
  `NONCONSERVATIVE_TRANSFORMATION`; a requested conservative regrid is
  `REGRIDDING_UNAVAILABLE`, because no validated conservative mapping exists.

### Case-specific mapping

| Process | Metric ids | Available now | Missing evidence | Threshold owner |
|---|---|---|---|---|
| #7 horizontal turbulence | particle center/covariance/principal axes/radial containment; grid 90% footprint area; paired footprint overlap, center separation, shape | candidate particle metrics (mass and count); oracle grid metrics | oracle partposit masses; candidate grids; conservative regridding for the "90% footprint after regridding" criterion | #7 via #59 |
| #8 / #64 vertical transport | particle mean height, quantiles, layer fraction, fraction above mixing height; grid mean height, layer fraction | candidate and oracle-grid vertical metrics | per-particle mixing height (`fraction_above_mixing_height` UNSUPPORTED) | #64 via #59 |
| #13 wet deposition | `budget.deposited_mass_total`, `budget.mass_closure`, grid center/spread/footprint/hotspot, paired field/shape/footprint | candidate wet reservoir when written; oracle wet-field shape and location | candidate wet grid (#41/#42); oracle kg scale (#40) | #34 via #59 |
| #14 dry deposition / settling | as #13 for dry, plus particle vertical quantiles | candidate dry reservoir; oracle dry-field shape and location | candidate dry grid; oracle kg scale; #114 is not consumed | #37 via #59 |
| #16 output semantics | paired field/shape/support statistics, integrated total, grid layer fraction, hotspot | unary oracle field metrics | versioned output schema (#40), candidate serialization (#42) | #42 via #59 |

Deposition quantities are extracted only when the #51 case enables that
process (all quantities without a case manifest).

### Downstream handoff (#58/#59)

`distributional.collect_for_ensemble(reports)` returns every record with its
`source_report`, groups keyed by role and `comparability_key`, and the #54
aggregate over all sources from `invariants.aggregate_realizations`. It
rejects mixed contract versions or digests and mixed case ids; a report
without a block is ineligible. `eligible` is true only when the #54
aggregate is `PASS` and every source block is eligible. Candidate (Philox)
and oracle (FLEXPART internal seeds) realizations are independent samples
and are never paired by seed index. `aggregate-seeds` carries the records
unchanged and aggregates no metric value. #59 references metrics by
`metric_id`, `definition_version` and output name; thresholds stay with the
physics issues.

### Real-artifact demonstration

`corpus-seeds` on the local ADV-ANA-001 artifacts (one candidate seed,
1024 particles; pinned-oracle `grid_conc` output for 00:30 and 01:00, no
`partposit`, no run manifest, no #52 report) produces 66 records:

- 31 `COMPUTED` unary records: candidate particle metrics for both
  weightings (center 10.5037E 50.0000N, zero spread and 100 m at every
  quantile, as expected for analytic advection) and the candidate kg budget
  (#54 `PASS`, relative error 0); no reservoir records because the case
  disables deposition; oracle
  concentration metrics per output time (e.g. centroid 10.1333E then
  10.3750E, 90% footprint 3 cells / 238 km^2, east-west major axis).
- 31 `MISSING_EVIDENCE`: candidate grids (corpus-run writes none), oracle
  particle dump, oracle kg ledger, and all 5 paired metrics.
- 4 `UNSUPPORTED`: candidate fraction above mixing height (both
  weightings) and oracle kg totals.
- No paired metric is computed: the #54 aggregate is `BLOCKED` (provenance
  and oracle ledger unavailable). The candidate end-state centroid and the
  oracle time-averaged grid centroid differ in representation and window and
  are not compared.

No synthetic `validate_run` artifacts were available locally, so the
synthetic path is exercised by tests only.

### Unimplemented requirements (follow-ups)

- Fraction above the mixing height needs per-particle mixing-height evidence (#64).
- Oracle particle mass weighting needs decoded `partposit` masses; FLEXPART
  11.1 writes `partposit` as NetCDF only (#57).
- Paired metrics across different grids need a validated conservative
  regridding contract (follow-up to #7/#16).
- Kilogram totals of FLEXPART binary grids need a versioned output unit
  contract (#40).
- Candidate gridded corpus output (#41/#42).
- Paired scoring stays unreachable while the oracle kg ledger keeps the #54
  aggregate `BLOCKED`; that rule belongs to #54 and is not relaxed here.
- #51 case manifests still reference `evaluation-metrics report-schema-1.0.0`
  in `validation_definition_refs.metric_contracts`; adopting the new contract
  identity there is a separate case-data migration.

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
