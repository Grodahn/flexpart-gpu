"""Canonical scientific metrics for flexpart-gpu evaluation.

This module is the single definition site for all comparison metrics used by
``scripts/evaluate/evaluate_case.py``. It unifies the previously divergent
implementations in ``scripts/compare_concentrations.py`` and the scripts under
``scripts/etex/``.

Unification decisions (adopted definition first, rejected alternatives last):

- Pearson correlation returns ``None`` when either field has zero variance.
  Adopted from ``compare_concentrations.compute_metrics`` and
  ``compare_oracle_observations.metrics``. The ``0.0`` fallback used by
  ``compare_etex_fortran_obs``, ``compare_gpu_obs`` and
  ``compare_with_observations`` is rejected because a zero-variance field
  carries no correlation information; reporting ``0.0`` misreads as
  "uncorrelated".
- Normalized RMSE for gridded fields is ``rmse / max(mean|a|, mean|b|)``.
  Adopted from ``compare_concentrations.compute_metrics``. The
  ``rmse / max(obs)`` variant (``compare_etex_fortran_obs``,
  ``compare_gpu_obs``) and the ``rmse / range(obs)`` variant
  (``compare_with_observations``) are rejected for grid-to-grid comparison
  because they depend on a single extreme value or range. ETEX
  station metrics use NMSE (Chang and Hanna) instead; see
  :func:`etex_station_metrics`.
- Fractional bias and NMSE for ETEX stations follow Chang and Hanna (2004):
  ``FB = 2*(mean_mod - mean_obs)/(mean_mod + mean_obs)`` and
  ``NMSE = mean((mod-obs)^2)/(mean_obs*mean_mod)``. Both return ``None``
  when their denominator is not strictly positive. Adopted from
  ``compare_oracle_observations.metrics``. The ``0.0``-on-empty-denominator
  fallback in ``compare_with_observations`` is rejected.
- FAC2 is the fraction of strictly positive pairs with
  ``0.5 <= modeled/observed <= 2.0``. Adopted from
  ``compare_oracle_observations.metrics``.
- Horizontal moments use the equirectangular projection around the weighted
  mean latitude with metres per degree shared exactly with the corpus
  runner (``R * pi / 180``). Adopted conceptually from
  ``compare_concentrations.horizontal_grid_moments``; the legacy rounded
  111.195 km/deg is superseded by the exact formula so the runner
  cross-check compares identical conversions. The same function serves
  both particle-space and grid-space moments so the two are directly
  comparable.
- Sparse FLEXPART decoding uses value-sign run detection (physical values
  are ``abs(value)``). Adopted from ``compare_concentrations``; see
  ``io_fortran``.

Particle metrics and grid metrics are strictly separated. A normalized
concentration field comparison is a shape diagnostic only and must never be
reported as mass conservation. Mass conservation is only demonstrated by a
mass budget in kilograms (see :func:`mass_budget`).

All functions use the Python standard library only so the evaluation runs in
a minimal Python 3 environment without third-party packages.

Units used throughout this module:

- mass: kilograms (kg)
- concentration: picograms per cubic metre (pg/m3) for ETEX surface fields;
  raw FLEXPART ``grid_conc`` units are carried opaquely and never mixed
  with kilograms without an explicit cell-volume conversion
- horizontal distance: kilometres (km); covariance/eigenvalues: km^2
- height: metres (m)
- time: seconds (s) for intervals, ISO-8601 UTC strings for instants,
  hours (h) for ETEX lag metrics
- angles: degrees (deg)
"""

import math
from statistics import median

EARTH_RADIUS_M = 6_371_000.0
# Kilometres per degree of latitude, shared exactly with the corpus runner
# (src/bin/corpus-run.rs computes metres per degree as
# R_EARTH_M * PI / 180, i.e. this value times 1000). The legacy scripts use
# the rounded 111.195; the exact formula governs here so the runner
# cross-check compares identical conversions.
DEG_TO_KM = EARTH_RADIUS_M * math.pi / 180.0 / 1000.0
DETECTION_THRESHOLD_PG_M3 = 10.0


def _mean(values):
    if not values:
        return 0.0
    return sum(values) / len(values)


def _std_population(values, mean_value):
    if not values:
        return 0.0
    return math.sqrt(sum((v - mean_value) ** 2 for v in values) / len(values))


def pearson_correlation(observed, modeled):
    """Pearson correlation, or None when either series has zero variance.

    Both inputs are sequences of equal length. An empty input or a length
    mismatch raises ValueError. A constant series has undefined correlation;
    this function returns None instead of the legacy 0.0 fallback.
    """
    observed = [float(v) for v in observed]
    modeled = [float(v) for v in modeled]
    if len(observed) == 0 or len(observed) != len(modeled):
        raise ValueError("correlation needs nonempty paired values of equal length")
    mean_obs = _mean(observed)
    mean_mod = _mean(modeled)
    std_obs = _std_population(observed, mean_obs)
    std_mod = _std_population(modeled, mean_mod)
    if std_obs == 0.0 or std_mod == 0.0:
        return None
    covariance = sum((o - mean_obs) * (m - mean_mod) for o, m in zip(observed, modeled)) / len(observed)
    return covariance / (std_obs * std_mod)


def field_comparison_metrics(observed, modeled, label=""):
    """Compare two gridded fields given as flat sequences.

    Returns bias, MAE, RMSE, normalized RMSE (rmse divided by the larger of
    the two mean-absolute values, None when both fields are all zeros),
    Pearson correlation (None on zero variance), sample counts, nonzero
    counts and raw sums. Inputs are not normalized here; callers that need
    a shape-only diagnostic must normalize explicitly and must not present
    the result as mass conservation.
    """
    observed = [float(v) for v in observed]
    modeled = [float(v) for v in modeled]
    if len(observed) == 0 or len(observed) != len(modeled):
        raise ValueError("field comparison needs nonempty paired values")
    n = len(observed)
    diff = [m - o for o, m in zip(observed, modeled)]
    bias = _mean(diff)
    mae = _mean([abs(v) for v in diff])
    rmse = math.sqrt(_mean([v * v for v in diff]))
    mean_abs_obs = _mean([abs(v) for v in observed])
    mean_abs_mod = _mean([abs(v) for v in modeled])
    denom = max(mean_abs_obs, mean_abs_mod)
    nrmse = (rmse / denom) if denom > 0 else None
    return {
        "label": label,
        "sample_count": n,
        "nonzero_observed": int(sum(1 for v in observed if v != 0.0)),
        "nonzero_modeled": int(sum(1 for v in modeled if v != 0.0)),
        "sum_observed": float(sum(observed)),
        "sum_modeled": float(sum(modeled)),
        "bias": float(bias),
        "mae": float(mae),
        "rmse": float(rmse),
        "normalized_rmse": (float(nrmse) if nrmse is not None else None),
        "normalized_rmse_denominator": "max(mean_abs_observed, mean_abs_modeled)",
        "correlation": pearson_correlation(observed, modeled),
    }


def mass_budget(initial_mass_kg, remaining_mass_kg, dry_deposited_kg=0.0,
                wet_deposited_kg=0.0, decayed_kg=0.0):
    """Close a mass budget in kilograms.

    The accounted total is remaining + dry + wet + decayed mass. The error
    is accounted minus initial mass. All quantities are in kg. This is the
    only accepted demonstration of mass conservation; comparing normalized
    concentration fields is a shape diagnostic and is reported separately.
    """
    initial = float(initial_mass_kg)
    remaining = float(remaining_mass_kg)
    dry = float(dry_deposited_kg)
    wet = float(wet_deposited_kg)
    decayed = float(decayed_kg)
    for name, value in (("initial_mass_kg", initial), ("remaining_mass_kg", remaining),
                        ("dry_deposited_kg", dry), ("wet_deposited_kg", wet),
                        ("decayed_kg", decayed)):
        if not math.isfinite(value) or value < 0:
            raise ValueError(f"{name} must be finite and non-negative, got {value}")
    accounted = remaining + dry + wet + decayed
    absolute_error_kg = accounted - initial
    relative_error = (absolute_error_kg / initial) if initial > 0 else None
    return {
        "initial_mass_kg": initial,
        "remaining_mass_kg": remaining,
        "dry_deposited_kg": dry,
        "wet_deposited_kg": wet,
        "decayed_kg": decayed,
        "accounted_mass_kg": accounted,
        "absolute_error_kg": absolute_error_kg,
        "relative_error": relative_error,
        "unit": "kg",
    }


def center_of_mass_grid(flat_values, nx, ny, nz, xlon0_deg, ylat0_deg,
                       dx_deg, dy_deg, heights_m):
    """Mass-weighted center of mass of a 3-D grid field.

    ``flat_values`` is a flat row-major sequence with index
    ``((ix * ny) + iy) * nz + iz``. Horizontal positions are cell centers.
    The vertical position weights each level by ``heights_m[iz]`` (the
    FLEXPART OUTHEIGHTS layer-top convention also used by
    ``compare_concentrations``). Returns None for an empty field.
    Finite, non-negative inputs are required.
    """
    values = [float(v) for v in flat_values]
    if len(values) != nx * ny * nz:
        raise ValueError(f"field length {len(values)} != nx*ny*nz {nx*ny*nz}")
    if len(heights_m) != nz:
        raise ValueError("heights_m length must equal nz")
    for v in values:
        if not math.isfinite(v) or v < 0:
            raise ValueError("grid field must be finite and non-negative")
    total = sum(values)
    if total <= 0:
        return None
    lon_sum = 0.0
    lat_sum = 0.0
    z_sum = 0.0
    for ix in range(nx):
        lon = xlon0_deg + (ix + 0.5) * dx_deg
        for iy in range(ny):
            lat = ylat0_deg + (iy + 0.5) * dy_deg
            for iz in range(nz):
                w = values[((ix * ny) + iy) * nz + iz]
                if w > 0:
                    lon_sum += w * lon
                    lat_sum += w * lat
                    z_sum += w * float(heights_m[iz])
    return {
        "lon_deg": lon_sum / total,
        "lat_deg": lat_sum / total,
        "z_m": z_sum / total,
        "total_weight": total,
    }


def center_of_mass_particles(lons_deg, lats_deg, heights_m, masses):
    """Mass-weighted center of mass of a particle ensemble.

    All inputs are equal-length sequences. Masses must be finite and
    non-negative. Returns None when the total mass is not positive.
    Units: lon/lat in degrees, height in metres, mass in kg.
    """
    lons = [float(v) for v in lons_deg]
    lats = [float(v) for v in lats_deg]
    heights = [float(v) for v in heights_m]
    weights = [float(v) for v in masses]
    if not (len(lons) == len(lats) == len(heights) == len(weights)):
        raise ValueError("particle arrays must have equal length")
    for w in weights:
        if not math.isfinite(w) or w < 0:
            raise ValueError("particle masses must be finite and non-negative")
    total = sum(weights)
    if total <= 0 or len(weights) == 0:
        return None
    return {
        "lon_deg": sum(lon * w for lon, w in zip(lons, weights)) / total,
        "lat_deg": sum(lat * w for lat, w in zip(lats, weights)) / total,
        "z_m": sum(z * w for z, w in zip(heights, weights)) / total,
        "total_mass_kg": total,
        "particle_count": len(weights),
    }


def horizontal_covariance(lons_deg, lats_deg, weights):
    """Weighted horizontal covariance in kilometres.

    Positions are in degrees, weights are non-negative (masses in kg for
    particles, column sums in field units for grids). The local projection
    is computed in metres exactly as the corpus runner does
    (``m_per_deg_lon = R * cos(lat) * pi / 180``,
    ``m_per_deg_lat = R * pi / 180``) and converted to kilometres, so the
    runner cross-check compares identical conversions. Returns the weighted
    mean position, the 2x2 covariance matrix in km^2, its ascending
    eigenvalues in km^2, and the east/north standard deviations in km.
    Returns None when the total weight is not positive.
    """
    lons = [float(v) for v in lons_deg]
    lats = [float(v) for v in lats_deg]
    w = [float(v) for v in weights]
    if not (len(lons) == len(lats) == len(w)):
        raise ValueError("position and weight arrays must have equal length")
    for value in w:
        if not math.isfinite(value) or value < 0:
            raise ValueError("covariance weights must be finite and non-negative")
    total = sum(w)
    if total <= 0 or len(w) == 0:
        return None
    lon_mean = sum(lon * wi for lon, wi in zip(lons, w)) / total
    lat_mean = sum(lat * wi for lat, wi in zip(lats, w)) / total
    lat_rad = math.radians(lat_mean)
    m_per_deg_lon = EARTH_RADIUS_M * math.cos(lat_rad) * math.pi / 180.0
    m_per_deg_lat = EARTH_RADIUS_M * math.pi / 180.0
    xs = [(lon - lon_mean) * m_per_deg_lon / 1000.0 for lon in lons]
    ys = [(lat - lat_mean) * m_per_deg_lat / 1000.0 for lat in lats]
    xx = sum(wi * x * x for wi, x in zip(w, xs)) / total
    yy = sum(wi * y * y for wi, y in zip(w, ys)) / total
    xy = sum(wi * x * y for wi, x, y in zip(w, xs, ys)) / total
    trace = xx + yy
    det = xx * yy - xy * xy
    disc = max(trace * trace / 4.0 - det, 0.0)
    root = math.sqrt(disc)
    eig_small = trace / 2.0 - root
    eig_large = trace / 2.0 + root
    return {
        "lon_mean_deg": lon_mean,
        "lat_mean_deg": lat_mean,
        "covariance_km2": [[xx, xy], [xy, yy]],
        "eigenvalues_km2": [eig_small, eig_large],
        "sigma_east_km": math.sqrt(max(xx, 0.0)),
        "sigma_north_km": math.sqrt(max(yy, 0.0)),
        "unit": "km",
    }


def grid_column_weights(flat_values, nx, ny, nz):
    """Sum a row-major (nx, ny, nz) field over height into (nx*ny) columns."""
    values = [float(v) for v in flat_values]
    if len(values) != nx * ny * nz:
        raise ValueError("field length does not match grid shape")
    columns = []
    centers_lon_idx = []
    centers_lat_idx = []
    for ix in range(nx):
        for iy in range(ny):
            s = 0.0
            for iz in range(nz):
                s += values[((ix * ny) + iy) * nz + iz]
            columns.append(s)
            centers_lon_idx.append(ix)
            centers_lat_idx.append(iy)
    return columns, centers_lon_idx, centers_lat_idx


def horizontal_covariance_grid(flat_values, nx, ny, nz, xlon0_deg, ylat0_deg,
                               dx_deg, dy_deg):
    """Horizontal covariance of a 3-D grid field via its column sums."""
    columns, ix_list, iy_list = grid_column_weights(flat_values, nx, ny, nz)
    lons = [xlon0_deg + (ix + 0.5) * dx_deg for ix in ix_list]
    lats = [ylat0_deg + (iy + 0.5) * dy_deg for iy in iy_list]
    return horizontal_covariance(lons, lats, columns)


def vertical_quantiles(heights_m, masses, quantiles=(0.05, 0.25, 0.5, 0.75, 0.95)):
    """Mass-weighted vertical quantiles in metres.

    Particles are sorted by height; the quantile q is the height at which
    the cumulative mass first reaches q times the total mass, with linear
    interpolation between bracketing particles. Returns None when the total
    mass is not positive. An empty quantile list returns an empty dict.
    """
    heights = [float(v) for v in heights_m]
    weights = [float(v) for v in masses]
    if len(heights) != len(weights):
        raise ValueError("heights and masses must have equal length")
    for w in weights:
        if not math.isfinite(w) or w < 0:
            raise ValueError("quantile weights must be finite and non-negative")
    for q in quantiles:
        if not 0.0 <= q <= 1.0:
            raise ValueError(f"quantile out of [0, 1]: {q}")
    total = sum(weights)
    if total <= 0 or len(weights) == 0:
        return None
    order = sorted(range(len(heights)), key=lambda i: heights[i])
    sorted_h = [heights[i] for i in order]
    sorted_w = [weights[i] for i in order]
    cumulative = []
    running = 0.0
    for w in sorted_w:
        running += w
        cumulative.append(running / total)
    result = {}
    for q in quantiles:
        if q <= 0.0:
            result[str(q)] = sorted_h[0]
            continue
        if q >= 1.0:
            result[str(q)] = sorted_h[-1]
            continue
        idx = next(i for i, c in enumerate(cumulative) if c >= q)
        if idx == 0:
            result[str(q)] = sorted_h[0]
        else:
            c0 = cumulative[idx - 1]
            c1 = cumulative[idx]
            h0 = sorted_h[idx - 1]
            h1 = sorted_h[idx]
            frac = 0.0 if c1 == c0 else (q - c0) / (c1 - c0)
            result[str(q)] = h0 + frac * (h1 - h0)
    return {"quantiles_m": result, "total_mass_kg": total, "unit": "m"}


def unweighted_quantiles_linear(values, quantiles=(0.1, 0.5, 0.9)):
    """Unweighted linear-index quantiles (corpus-runner convention).

    Values are sorted ascending; the quantile q is read at position
    ``q * (n - 1)`` with linear interpolation between bracketing values.
    This matches ``compute_metrics`` in ``src/bin/corpus-run.rs`` exactly
    and is used only to cross-check the two implementations. Scientific
    reporting uses :func:`vertical_quantiles`, whose mass-weighted
    first-reach convention also handles non-uniform particle masses.
    """
    flat = [float(v) for v in values]
    if not flat:
        raise ValueError("quantiles need at least one value")
    for q in quantiles:
        if not 0.0 <= q <= 1.0:
            raise ValueError(f"quantile out of [0, 1]: {q}")
    ordered = sorted(flat)
    n = len(ordered)
    result = {}
    for q in quantiles:
        pos = q * (n - 1)
        low = int(math.floor(pos))
        high = int(math.ceil(pos))
        result[str(q)] = ordered[low] + (ordered[high] - ordered[low]) * (pos - low)
    return {"quantiles": result, "count": n}


def footprint_overlap(observed, modeled, threshold=0.0):
    """Overlap of binary footprints where the field exceeds a threshold.

    Both fields are flat sequences of equal length in identical units;
    ``threshold`` is in the same units. Returns the count of active cells
    in each field, their intersection and union, and the Figure of Merit
    in Space (intersection over union, None when the union is empty).
    """
    observed = [float(v) for v in observed]
    modeled = [float(v) for v in modeled]
    if len(observed) == 0 or len(observed) != len(modeled):
        raise ValueError("footprint comparison needs nonempty paired values")
    obs_active = [v > threshold for v in observed]
    mod_active = [v > threshold for v in modeled]
    intersection = sum(1 for o, m in zip(obs_active, mod_active) if o and m)
    union = sum(1 for o, m in zip(obs_active, mod_active) if o or m)
    return {
        "threshold": float(threshold),
        "observed_active_cells": int(sum(obs_active)),
        "modeled_active_cells": int(sum(mod_active)),
        "intersection_cells": int(intersection),
        "union_cells": int(union),
        "figure_of_merit_in_space": (float(intersection) / float(union)) if union > 0 else None,
    }


def etex_station_metrics(observed_pg_m3, modeled_pg_m3):
    """Independent model-versus-observation metrics for ETEX stations.

    Units are pg/m3 for bias and RMSE. Fractional bias (FB) and normalized
    mean square error (NMSE) follow Chang and Hanna (2004) and are
    dimensionless; both are None when their denominator is not strictly
    positive. Correlation is None on zero variance. FAC2 is the fraction
    of strictly positive pairs with 0.5 <= modeled/observed <= 2.0, None
    when no strictly positive pair exists; ``fac2_pairs`` reports that
    denominator. Negative concentrations raise ValueError (integrity
    error); missing pairs must be excluded by the caller and counted in
    the report's ``missing`` section instead of being interpolated.
    """
    observed = [float(v) for v in observed_pg_m3]
    modeled = [float(v) for v in modeled_pg_m3]
    if len(observed) == 0 or len(observed) != len(modeled):
        raise ValueError("ETEX metrics need nonempty paired values")
    for v in observed + modeled:
        if not math.isfinite(v) or v < 0:
            raise ValueError("ETEX concentrations must be finite and non-negative")
    n = len(observed)
    diff = [m - o for o, m in zip(observed, modeled)]
    mean_obs = _mean(observed)
    mean_mod = _mean(modeled)
    fb = (2.0 * _mean(diff) / (mean_obs + mean_mod)) if (mean_obs + mean_mod) > 0 else None
    nmse = (_mean([d * d for d in diff]) / (mean_obs * mean_mod)
            if mean_obs > 0 and mean_mod > 0 else None)
    positive = [(o, m) for o, m in zip(observed, modeled) if o > 0 and m > 0]
    fac2 = None
    if positive:
        inside = sum(1 for o, m in positive if 0.5 <= m / o <= 2.0)
        fac2 = inside / len(positive)
    return {
        "n": n,
        "unit": "pg/m3",
        "bias_pg_m3": float(_mean(diff)),
        "rmse_pg_m3": float(math.sqrt(_mean([d * d for d in diff]))),
        "correlation": pearson_correlation(observed, modeled),
        "fractional_bias": (float(fb) if fb is not None else None),
        "nmse": (float(nmse) if nmse is not None else None),
        "fac2": (float(fac2) if fac2 is not None else None),
        "fac2_pairs": int(len(positive)),
        "mean_observed_pg_m3": float(mean_obs),
        "mean_modeled_pg_m3": float(mean_mod),
    }


def etex_timing(pairs, model_key, detection_threshold_pg_m3=DETECTION_THRESHOLD_PG_M3):
    """Arrival-time and peak diagnostics per station.

    ``pairs`` is a sequence of dicts with ``station``, ``start_time`` and
    ``end_time`` (ISO-8601 ``YYYY-MM-DD HH:MM``), ``observed_pg_m3`` and the
    model value under ``model_key``. Arrival is the first record with value
    at or above the detection threshold (default 10 pg/m3). Lags are
    model minus observation in hours. Peak diagnostics compare each
    station's maximum observed and modeled records. Stations without an
    arrival on both sides contribute to the station count but not to the
    lag median. Returns station counts and medians (None when undefined).
    """
    from datetime import datetime
    by_station = {}
    for record in pairs:
        by_station.setdefault(record["station"], []).append(record)
    arrival_lags_h = []
    peak_lags_h = []
    peak_ratios = []
    arrival_stations = 0
    peak_stations = 0
    for records in by_station.values():
        ordered = sorted(records, key=lambda r: r["start_time"])
        obs_arrival = next((r for r in ordered if r["observed_pg_m3"] >= detection_threshold_pg_m3), None)
        mod_arrival = next((r for r in ordered if r[model_key] >= detection_threshold_pg_m3), None)
        if obs_arrival is not None and mod_arrival is not None:
            arrival_stations += 1
            delta = (datetime.fromisoformat(mod_arrival["start_time"])
                     - datetime.fromisoformat(obs_arrival["start_time"])).total_seconds() / 3600.0
            arrival_lags_h.append(delta)
        obs_peak = max(ordered, key=lambda r: r["observed_pg_m3"])
        mod_peak = max(ordered, key=lambda r: r[model_key])
        if obs_peak["observed_pg_m3"] > 0:
            peak_stations += 1
            peak_lags_h.append(
                (datetime.fromisoformat(mod_peak["start_time"])
                 - datetime.fromisoformat(obs_peak["start_time"])).total_seconds() / 3600.0)
            peak_ratios.append(mod_peak[model_key] / obs_peak["observed_pg_m3"])
    return {
        "detection_threshold_pg_m3": float(detection_threshold_pg_m3),
        "arrival_stations": int(arrival_stations),
        "median_arrival_error_h": (float(median(arrival_lags_h)) if arrival_lags_h else None),
        "peak_stations": int(peak_stations),
        "median_peak_time_error_h": (float(median(peak_lags_h)) if peak_lags_h else None),
        "median_peak_magnitude_ratio": (float(median(peak_ratios)) if peak_ratios else None),
        "unit": "h",
    }


def aggregate_seed_values(values):
    """Aggregate one scalar metric over independent seeds.

    Returns count, mean, sample standard deviation (None for n < 2),
    minimum, maximum, median and an approximate 95% confidence interval
    for the mean (mean +/- 1.96*std/sqrt(n), None for n < 2). Values must
    be finite. Reports with fewer than 10 seeds are flagged by the caller
    as insufficient for a multi-seed parity claim.
    """
    flat = [float(v) for v in values]
    if len(flat) == 0:
        raise ValueError("seed aggregation needs at least one value")
    for v in flat:
        if not math.isfinite(v):
            raise ValueError("seed values must be finite")
    n = len(flat)
    mean_value = _mean(flat)
    if n >= 2:
        variance = sum((v - mean_value) ** 2 for v in flat) / (n - 1)
        std_value = math.sqrt(variance)
        half_width = 1.96 * std_value / math.sqrt(n)
        ci = [mean_value - half_width, mean_value + half_width]
    else:
        std_value = None
        ci = None
    return {
        "n_seeds": n,
        "mean": float(mean_value),
        "std": (float(std_value) if std_value is not None else None),
        "min": float(min(flat)),
        "max": float(max(flat)),
        "median": float(median(flat)),
        "ci95_mean": ([float(ci[0]), float(ci[1])] if ci is not None else None),
        "sufficient_for_parity_claim": bool(n >= 10),
    }
