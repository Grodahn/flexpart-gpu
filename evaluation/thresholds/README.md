# Scientific thresholds

Versioned pass/fail gates for evaluation reports. Thresholds live here, not
in `scripts/evaluate/`, so a code change cannot silently move a gate.

- Current file: `scientific-thresholds-v1.json` (`version: v1`).
- Report schema: `1.0.0` (see `docs/evaluation.md`).
- Change policy: any threshold change requires a new versioned file with
  date and scientific justification. Loosening a threshold because a
  candidate fails is prohibited.

## What v1 enforces

- `MASS_BUDGET_CLOSE`: the candidate mass budget in kilograms must close to
  1e-5 relative. Only a kilogram budget counts; a normalized concentration
  field comparison never satisfies mass conservation.
- `MASS_POSITIVITY`: all mass and concentration fields must be finite and
  non-negative. Violations are integrity errors.
- `GRID_WINDOW_ALIGNMENT`: time windows, averaging, sampling, grid cells
  and layer heights must match before any comparison. Mismatches fail
  instead of being interpolated.

## What v1 leaves diagnostic

- Synthetic grid shape diagnostics (correlation, footprint overlap, center
  distance, covariance eigenvalue ratios): single-seed shape information
  only. Stochastic parity needs at least 10 independent seeds with a
  controllable oracle seed.
- All ETEX mini observation metrics: the mini run is a pipeline and
  observation-pairing regression. Its input audit reports
  `INPUT_EQUIVALENCE_NOT_DEMONSTRATED` and no concentration parity is
  claimed.

## Multiseed policy

At least 10 independent seeds are required for a stochastic parity claim.
Runs without a controllable oracle seed are labeled
`NOT_SUITABLE_FOR_MULTISEED_PARITY` and stay diagnostic.
