#!/usr/bin/env python3
"""Derive the #231 ICON-EU vertical-geometry decision evidence from real GRIB2 bytes.

Research tooling for issue #231 only. It builds no canonical #29 snapshot, no
#30 runtime geometry and no provider decoder (#32-B). Subcommands:

``inspect``
    Re-hash every file of a sample fetched with ``icon_eu_fetch.py`` against its
    retrieval manifest, require exactly the files of the #231 sample request,
    decode every message with ecCodes, cross-check the metadata (and, when
    libaec is installed, every value) with the independent octet reader
    ``icon_eu_grib2_sections``, and derive the full-field checks of
    docs/research/icon-eu-vertical-231.md plus a deterministic set of complete
    representative columns.

``summarize``
    Merge a primary and a secondary ``inspect`` result. Message identities and
    column values must be identical and every derived number must agree to a
    relative 1e-12 (summation order only); otherwise the evidence is refused.

The FLEXPART height reconstruction below is a double-precision *diagnostic*
replay of ``verttransform_mod.f90:1998-2029`` fed with provider pressure. It is
not an oracle; the Rust lock test checks the same replay against the pinned
compiled routine output frozen in ``fixtures/interpolation/shared-height-v1``.

Usage::

    python -I scripts/provider/icon_eu_vertical_231.py inspect --sample <fetch dir> \
        --request fixtures/meteorology/icon-eu-vertical-231-v1/sample-request.json \
        --label <decoder label> --output <inspect.json>
    python -I scripts/provider/icon_eu_vertical_231.py summarize --primary <a.json> \
        --secondary <b.json> --output fixtures/meteorology/icon-eu-vertical-231-v1/decoded-evidence.json
"""

import argparse
import json
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import icon_eu_audit as audit  # noqa: E402  (same research directory)
import icon_eu_fetch as fetch  # noqa: E402
import icon_eu_grib2_sections as sections  # noqa: E402

EVIDENCE_SCHEMA = "flexpart-gpu.icon-eu-vertical-evidence"
HALF_LEVELS = 75
FULL_LEVELS = 74
# ICON release-2024.10-public src/shared/mo_physical_constants.f90:96,108,121,148.
ICON_RD = 287.04  # J kg-1 K-1
ICON_GRAV = 9.80665  # m s-2
ICON_RV = 461.51  # J kg-1 K-1
ICON_VTMPC1 = ICON_RV / ICON_RD - 1.0
# FLEXPART 11.1 par_mod.f90:61 and verttransform_mod.f90:1998-2018.
FLEXPART_R_AIR = 287.05
FLEXPART_GA = 9.81
FLEXPART_SURFACE_VAPOUR_FACTOR = 0.378
FLEXPART_MOIST_FACTOR = 0.608
FLEXPART_TV_SWITCH_K = 0.2
# FLEXPART verttransform_mod.f90:313 shared-height initialization threshold.
FLEXPART_INIT_PRESSURE_PA = 100000.0
SUMMARY_RELATIVE_TOLERANCE = 1.0e-12
FIXED_COLUMN_LAT_LON = (50.0, 10.0)
COLUMN_RULES = (
    "flexpart_shared_height_init",
    "max_terrain",
    "min_terrain",
    "max_index_space_terrain_gradient",
    "max_lowest_level_reconstruction_difference",
    "max_hypsometric_deviation_mean_convention",
    "nearest_50n_10e",
)


def numpy_module():
    import numpy  # noqa: PLC0415  (optional for summarize)
    return numpy


def stats(array):
    numpy = numpy_module()
    values = numpy.asarray(array, dtype="<f8").ravel()
    return {
        "min": float(values.min()), "max": float(values.max()),
        "mean": float(values.mean()),
        "p01": float(numpy.percentile(values, 1)),
        "p50": float(numpy.percentile(values, 50)),
        "p99": float(numpy.percentile(values, 99)),
        "max_abs": float(numpy.abs(values).max()),
    }


def expected_identities(request):
    """Return the identity tuples the #231 request must expand to, exactly once each."""
    identities = set()
    for _, relative in fetch.expand_request(request):
        info = audit.describe(relative[: -len(".bz2")])
        identities.add((info["kind"], info["variable"], info["step"], info["level"]))
    return identities


def decode(row, libaec):
    """Decode one file; returns its identity record and float64 values."""
    numpy = numpy_module()
    decoded = list(audit.eccodes_messages(row["grib_path"]))
    if len(decoded) != 1:
        raise audit.AuditError(f"{row['grib_path']}: expected exactly one message")
    keys, values = decoded[0]
    with open(row["grib_path"], "rb") as handle:
        message, = sections.split_messages(handle.read())
    parsed = sections.parse(message)
    problems = audit.cross_check(keys, parsed)
    independent_max_abs = None
    if libaec is not None:
        independent = numpy.asarray(sections.decode_values(message, libaec), dtype="<f8")
        independent_max_abs = float(numpy.abs(independent - values).max())
        if independent_max_abs != 0.0:
            problems.append(f"values differ from libaec by {independent_max_abs}")
    if problems:
        raise audit.AuditError(f"{row['grib_path']}: {problems}")
    if not numpy.isfinite(values).all():
        raise audit.AuditError(f"{row['grib_path']}: non-finite decoded value")
    product = parsed["product"]
    record = {
        "path": row["path"][: -len(".bz2")],
        "grib2_sha256": row["grib2_sha256"],
        "kind": row["kind"], "variable": row["variable"],
        "step": row["step"], "level": row["level"],
        "discipline_category_number": [parsed["discipline"], product["parameter_category"],
                                       product["parameter_number"]],
        "first_surface": [product["first_surface_type"], product["first_surface_scaled_value"]],
        "second_surface": [product["second_surface_type"], product["second_surface_scaled_value"]],
        "product_template": product["template"],
        "forecast_time": product["forecast_time"],
        "unit_of_time_range": product["unit_of_time_range"],
        "data_date_time": [int(keys["dataDate"]), int(keys["dataTime"])],
        "nlev": keys.get("nlev"), "number_of_vgrid_used": keys.get("numberOfVGridUsed"),
        "uuid_of_vgrid": keys.get("uuidOfVGrid"),
        "grid": [parsed["grid"][name] for name in (
            "ni", "nj", "first_lat_deg", "first_lon_deg", "last_lat_deg", "last_lon_deg",
            "di_deg", "dj_deg", "shape_of_earth")],
        "packing": {"template": parsed["representation"]["template"],
                    "bits_per_value": parsed["representation"]["bits_per_value"],
                    "binary_scale_factor": parsed["representation"]["binary_scale_factor"],
                    "decimal_scale_factor": parsed["representation"]["decimal_scale_factor"]},
        "values_sha256_f8le": audit.sha256_bytes(values.astype("<f8").tobytes()),
        "independent_values_max_abs_difference": independent_max_abs,
    }
    return record, values


def flexpart_ew(temperature_k):
    """FLEXPART qvsat_mod.f90:144-196 saturation vapour pressure over water [Pa]."""
    numpy = numpy_module()
    y = 373.16 / temperature_k
    a = -7.90298 * (y - 1.0) + 5.02808 * 0.43429 * numpy.log(y)
    c = -1.3816 * (-1.0 + 10.0 ** ((1.0 - 1.0 / y) * 11.344)) / 1.0e7
    d = 8.1328 * (-1.0 + 10.0 ** ((1.0 - y) * 3.49149)) / 1.0e3
    return 101324.6 * 10.0 ** (a + c + d)


def flexpart_heights(ps, t2m, td2m, pressure, temperature, humidity):
    """Replay FLEXPART's hypsometric model-level AGL heights with provider pressure.

    Arrays are top-down ``[level, column]``; integration runs from the surface
    boundary (ps, Tv from T2m/Td2m) upward as in verttransform_mod.f90:1998-2023.
    """
    numpy = numpy_module()
    const = FLEXPART_R_AIR / FLEXPART_GA
    tv_old = t2m * (1.0 + FLEXPART_SURFACE_VAPOUR_FACTOR * flexpart_ew(td2m) / ps)
    p_old = ps
    height = numpy.zeros(ps.shape)
    heights = numpy.empty(pressure.shape)
    for level in range(pressure.shape[0] - 1, -1, -1):
        tv = temperature[level] * (1.0 + FLEXPART_MOIST_FACTOR * humidity[level])
        log_pressure = numpy.log(p_old / pressure[level])
        changed = numpy.abs(tv - tv_old) > FLEXPART_TV_SWITCH_K
        safe_ratio = numpy.where(changed, numpy.log(tv / tv_old), 1.0)
        layer_tv = numpy.where(changed, (tv - tv_old) / safe_ratio, tv)
        height = height + const * log_pressure * layer_tv
        heights[level] = height
        tv_old, p_old = tv, pressure[level]
    return heights


def stack(fields, kind, variable, count, step=None):
    """Move one variable's levels 1..count into a top-down ``[level, column]`` array."""
    numpy = numpy_module()
    return numpy.stack([fields.pop((kind, variable, step, level)) for level in range(1, count + 1)])


def identity_science(records):
    """Grid, time, vertical-grid and layer-bound identity across all decoded messages."""
    def distinct(selector, subset=None):
        return sorted({json.dumps(selector(r)) for r in records if subset is None or subset(r)})

    vertical = lambda r: r["variable"] in ("HHL", "P", "T", "QV")  # noqa: E731
    problems = []
    for record in records:
        if record["variable"] == "HHL":
            expected_first, expected_second = [150, record["level"]], [101, 0]
        elif record["kind"] == "model-level":
            expected_first, expected_second = [150, record["level"]], [150, record["level"] + 1]
        else:
            continue
        if record["first_surface"] != expected_first or record["second_surface"][0] != expected_second[0] \
                or (expected_second[0] == 150 and record["second_surface"] != expected_second):
            problems.append(record["path"])
    return {
        "distinct_grids": distinct(lambda r: r["grid"]),
        "distinct_data_date_time": distinct(lambda r: r["data_date_time"]),
        "distinct_forecast_time_and_unit": distinct(lambda r: [r["forecast_time"], r["unit_of_time_range"]]),
        "distinct_vertical_grid": distinct(lambda r: [r["nlev"], r["number_of_vgrid_used"], r["uuid_of_vgrid"]],
                                           vertical),
        "hhl_surfaces": distinct(lambda r: [r["first_surface"][0], r["second_surface"]],
                                 lambda r: r["variable"] == "HHL"),
        "full_level_layer_bounds": "model-level message k has first surface (150, k) and second surface (150, k+1)",
        "surface_identity_problems": problems,
        "packing_templates": distinct(lambda r: r["packing"]["template"]),
    }


def full_levels_asl(hhl):
    """ICON full-level height, z_mc = (z_ifc(k) + z_ifc(k+1)) / 2 (mo_vertical_grid.f90:170-174)."""
    return 0.5 * (hhl[:-1] + hhl[1:])


def hypsometric_thickness(p, t, qv):
    """ICON-constant hydrostatic thickness between adjacent full levels k and k+1 [m]."""
    numpy = numpy_module()
    virtual = t * (1.0 + ICON_VTMPC1 * qv)
    upper, lower = virtual[:-1], virtual[1:]
    differs = upper != lower
    log_mean = numpy.where(differs, (upper - lower) / numpy.log(numpy.where(differs, upper / lower, 2.0)), upper)
    return ICON_RD / ICON_GRAV * log_mean * numpy.log(p[1:] / p[:-1])


def wzlev_style_minus_hhl(hhl, hsurf):
    """FLEXPART-style interior interface (mean of adjacent full levels) minus HHL, half levels 2..74."""
    full_agl = full_levels_asl(hhl) - hsurf
    return 0.5 * (full_agl[:-1] + full_agl[1:]) - (hhl[1:-1] - hsurf)


def wzlev_style_top_minus_hhl(hhl):
    """FLEXPART top-interface extrapolation (verttransform_mod.f90:2028-2029) minus HHL(1)."""
    full = full_levels_asl(hhl)
    return 0.5 * (full[0] + full[1]) + full[0] - full[1] - hhl[0]


def loglinear_surface_minus_ps(hhl, p, ps):
    """ln P extrapolated linearly in height from full levels 73/74 to HHL(75), minus PS [Pa]."""
    numpy = numpy_module()
    full = full_levels_asl(hhl)
    fraction = (hhl[-1] - full[-1]) / (full[-2] - full[-1])
    return numpy.exp(numpy.log(p[-1]) + (numpy.log(p[-2]) - numpy.log(p[-1])) * fraction) - ps


def column_record(rule, index, data):
    """Complete raw column plus its derived checks, recomputed from the column slice."""
    j, i = divmod(int(index), data["ni"])
    cut = slice(int(index), int(index) + 1)
    hhl, p, t, qv = (data[name][:, cut] for name in ("hhl", "p", "t", "qv"))
    hsurf, ps, t2m, td2m = (data[name][cut] for name in ("hsurf", "ps", "t2m", "td2m"))
    full_agl = full_levels_asl(hhl) - hsurf
    replay = flexpart_heights(ps, t2m, td2m, p, t, qv)
    geometric = 0.5 * (hhl[:-2] - hhl[2:])
    values = lambda array: [float(v) for v in array.ravel()]  # noqa: E731
    return {
        "rule": rule, "index": int(index), "i": i, "j": j,
        "lat_deg": data["lat0"] + j * data["dlat"], "lon_deg": data["lon0"] + i * data["dlon"],
        "hsurf_m": float(hsurf[0]), "ps_pa": float(ps[0]),
        "t2m_k": float(t2m[0]), "td2m_k": float(td2m[0]),
        "hhl_m": values(hhl), "p_pa": values(p), "t_k": values(t), "qv_kg_kg": values(qv),
        "derived": {
            "full_level_agl_m": values(full_agl),
            "flexpart_replay_agl_m": values(replay),
            "flexpart_replay_minus_hhl_full_m": values(replay - full_agl),
            "hypsometric_ratio_mean_convention": values(hypsometric_thickness(p, t, qv) / geometric),
            "wzlev_style_from_hhl_full_minus_hhl_interior_m": values(wzlev_style_minus_hhl(hhl, hsurf)),
            "wzlev_style_top_minus_hhl_top_m": float(wzlev_style_top_minus_hhl(hhl)[0]),
            "loglinear_surface_pressure_minus_ps_pa": float(loglinear_surface_minus_ps(hhl, p, ps)[0]),
        },
    }


def convention_science(hhl, p, t, qv):
    """Hydrostatic consistency of three candidate full-level height conventions."""
    numpy = numpy_module()
    hypsometric = hypsometric_thickness(p, t, qv)
    geometries = (
        ("half_level_arithmetic_mean", lambda: 0.5 * (hhl[:-2] - hhl[2:])),
        ("upper_half_level", lambda: hhl[:-2] - hhl[1:-1]),
        ("lower_half_level", lambda: hhl[1:-1] - hhl[2:]),
    )
    ratio_stats, deviation_stats = {}, {}
    closest = best = pair_p50 = worst_column = None
    for rank, (name, geometry) in enumerate(geometries):
        ratio = hypsometric / geometry()
        deviation = numpy.abs(ratio - 1.0)
        ratio_stats[name], deviation_stats[name] = stats(ratio), stats(deviation)
        if rank == 0:
            pair_p50 = [float(v) for v in numpy.median(ratio, axis=1)]
            worst_column = int(numpy.argmax(deviation.max(axis=0)))
            closest, best = numpy.zeros(ratio.shape, dtype="i1"), deviation
        else:
            better = deviation < best
            closest[better] = rank
            best = numpy.where(better, deviation, best)
        del ratio, deviation
    science = {
        "definition": "ratio = (Rd/g) * logmean(Tv_k, Tv_k+1) * ln(P_k+1/P_k) / geometric thickness between full levels k and k+1, k = 1..73, Tv = T*(1+(Rv/Rd-1)*QV) without condensate loading, ICON constants",
        "ratio": ratio_stats,
        "abs_ratio_minus_one": deviation_stats,
        "fraction_closest_to_hydrostatic": {
            name: float((closest == rank).mean()) for rank, (name, _) in enumerate(geometries)},
        "mean_convention_pair_p50": pair_p50,
    }
    return science, worst_column


def reconstruction_science(data, full_agl, hhl_agl):
    """FLEXPART replay heights versus HHL-derived heights; returns science and worst column."""
    numpy = numpy_module()
    replay = flexpart_heights(data["ps"], data["t2m"], data["td2m"], data["p"], data["t"], data["qv"])
    replay_wz = 0.5 * (replay[:-1] + replay[1:])
    interfaces = {
        "wzlev_from_flexpart_replay_minus_hhl_interior_m": stats(replay_wz - hhl_agl[1:-1]),
        "wzlev_top_from_flexpart_replay_minus_hhl_top_m": stats(replay_wz[0] + replay[0] - replay[1] - hhl_agl[0]),
    }
    del replay_wz
    difference = replay - full_agl
    init_candidates = numpy.flatnonzero(data["ps"] > FLEXPART_INIT_PRESSURE_PA)
    init_index = int(init_candidates[0]) if init_candidates.size else (data["nj"] - 1) * data["ni"]
    science = {
        "definition": "FLEXPART verttransform_mod.f90:1998-2023 replay (double precision, provider P in place of akz+bkz*ps, FLEXPART constants) minus (0.5*(HHL(k)+HHL(k+1)) - HSURF)",
        "all_levels": stats(difference),
        "lowest_level_74": stats(difference[-1]),
        "level_60": stats(difference[59]),
        "level_37": stats(difference[36]),
        "level_13": stats(difference[12]),
        "top_level_1": stats(difference[0]),
        "per_level_max_abs_m": [float(v) for v in numpy.abs(difference).max(axis=1)],
        "relative_to_full_level_agl": stats(difference / full_agl),
    }
    init = {
        "rule": "first canonical index (x fastest, south row first) with PS > 100000 Pa, verttransform_mod.f90:309-319",
        "index": init_index,
        "hhl_full_level_agl_m_bottom_up": [float(v) for v in full_agl[::-1, init_index]],
        "flexpart_replay_agl_m_bottom_up": [float(v) for v in replay[::-1, init_index]],
        "max_abs_difference_m": float(numpy.abs(difference[:, init_index]).max()),
    }
    worst = int(numpy.argmax(numpy.abs(difference[-1])))
    return science, interfaces, init, worst


def interface_pressure_science(hhl, full_asl, p, ps, t, qv):
    """Spread of candidate interior interface pressures and surface-pressure closure."""
    numpy = numpy_module()
    log_p = numpy.log(p)
    fraction = (hhl[1:-1] - full_asl[1:]) / (full_asl[:-1] - full_asl[1:])
    p_loglinear = numpy.exp(log_p[1:] + (log_p[:-1] - log_p[1:]) * fraction)
    del fraction, log_p
    arithmetic = 0.5 * (p[:-1] + p[1:]) - p_loglinear
    science = {
        "definition": "interior half levels 2..74; loglinear = ln P linear in height between adjacent full levels at HHL; arithmetic = FLEXPART non-ECMWF conv_mod.f90:629 form; geometric = sqrt(P_k P_k+1)",
        "arithmetic_minus_loglinear_pa": stats(arithmetic),
        "arithmetic_minus_loglinear_relative": stats(arithmetic / p_loglinear),
    }
    del arithmetic
    science["geometric_minus_loglinear_pa"] = stats(numpy.sqrt(p[:-1] * p[1:]) - p_loglinear)
    del p_loglinear
    science["surface_loglinear_extrapolation_minus_ps_pa"] = stats(loglinear_surface_minus_ps(hhl, p, ps))
    virtual_lowest = t[-1] * (1.0 + ICON_VTMPC1 * qv[-1])
    science["surface_hydrostatic_extrapolation_minus_ps_pa"] = stats(
        p[-1] * numpy.exp(ICON_GRAV * (full_asl[-1] - hhl[-1]) / (ICON_RD * virtual_lowest)) - ps)
    return science


def vertical_science(fields, records):
    """Full-field checks over all columns plus the deterministic representative columns."""
    numpy = numpy_module()
    first = records[0]["grid"]
    ni, nj = int(first[0]), int(first[1])
    lon0 = float(first[3]) - 360.0 if float(first[3]) > 180.0 else float(first[3])
    data = {
        "ni": ni, "nj": nj, "lat0": float(first[2]), "lon0": lon0,
        "dlat": float(first[7]), "dlon": float(first[6]),
        "hsurf": fields.pop(("time-invariant", "HSURF", None, None)),
        "ps": fields.pop(("single-level", "PS", 0, None)),
        "t2m": fields.pop(("single-level", "T_2M", 0, None)),
        "td2m": fields.pop(("single-level", "TD_2M", 0, None)),
        "hhl": stack(fields, "time-invariant", "HHL", HALF_LEVELS),
        "p": stack(fields, "model-level", "P", FULL_LEVELS, 0),
        "t": stack(fields, "model-level", "T", FULL_LEVELS, 0),
        "qv": stack(fields, "model-level", "QV", FULL_LEVELS, 0),
    }
    hhl, hsurf, p, ps = data["hhl"], data["hsurf"], data["p"], data["ps"]
    thickness = hhl[:-1] - hhl[1:]
    science = {
        "column_count": int(hsurf.size),
        "terrain": {
            "hhl75_minus_hsurf_m": stats(hhl[-1] - hsurf),
            "hhl75_equals_hsurf_everywhere": bool((hhl[-1] == hsurf).all()),
            "hsurf_m": stats(hsurf),
        },
        "half_levels": {
            "count": HALF_LEVELS,
            "strictly_decreasing_with_level_index": bool((thickness > 0.0).all()),
            "min_layer_thickness_m": float(thickness.min()),
            "lowest_layer_thickness_m": stats(thickness[-1]),
            "horizontally_constant_levels": [
                level + 1 for level in range(HALF_LEVELS) if float(numpy.ptp(hhl[level])) == 0.0],
            "top_m": float(hhl[0, 0]),
        },
        "full_level_pressure": {
            "strictly_increasing_with_level_index": bool((p[1:] > p[:-1]).all()),
            "lowest_below_surface_pressure_everywhere": bool((p[-1] < ps).all()),
            "ps_minus_p74_pa": stats(ps - p[-1]),
            "p1_pa": stats(p[0]),
        },
    }
    del thickness
    science["full_level_convention"], worst_hypsometric = convention_science(hhl, p, data["t"], data["qv"])
    full_asl = full_levels_asl(hhl)
    full_agl = full_asl - hsurf
    hhl_agl = hhl - hsurf
    reconstruction, replay_interfaces, init, worst_reconstruction = reconstruction_science(data, full_agl, hhl_agl)
    science["flexpart_reconstruction_minus_hhl_full_levels"] = reconstruction
    science["shared_height_init_column"] = init
    wz_minus = wzlev_style_minus_hhl(hhl, hsurf)
    curvature = 0.25 * (hhl[:-2] - 2.0 * hhl[1:-1] + hhl[2:])
    science["interfaces"] = {
        "surface_interface_agl_m": stats(hhl_agl[-1]),
        "wzlev_style_from_hhl_full_minus_hhl_interior_m": stats(wz_minus),
        "wzlev_style_equals_quarter_second_difference_max_abs_m": float(numpy.abs(wz_minus - curvature).max()),
        "wzlev_style_per_interface_max_abs_m": [float(v) for v in numpy.abs(wz_minus).max(axis=1)],
        "wzlev_style_top_minus_hhl_top_m": stats(wzlev_style_top_minus_hhl(hhl)),
        **replay_interfaces,
    }
    del wz_minus, curvature, full_agl, hhl_agl
    science["interface_pressure_candidates"] = interface_pressure_science(
        hhl, full_asl, p, ps, data["t"], data["qv"])
    del full_asl

    grid2d = hsurf.reshape(nj, ni)
    gradient = numpy.zeros_like(grid2d)
    gradient[1:-1, 1:-1] = numpy.hypot(0.5 * (grid2d[1:-1, 2:] - grid2d[1:-1, :-2]),
                                       0.5 * (grid2d[2:, 1:-1] - grid2d[:-2, 1:-1]))
    fixed_j = int(round((FIXED_COLUMN_LAT_LON[0] - data["lat0"]) / data["dlat"]))
    fixed_i = int(round((FIXED_COLUMN_LAT_LON[1] - data["lon0"]) / data["dlon"]))
    selected = dict(zip(COLUMN_RULES, (
        init["index"], int(numpy.argmax(hsurf)), int(numpy.argmin(hsurf)),
        int(numpy.argmax(gradient.ravel())), worst_reconstruction, worst_hypsometric,
        fixed_j * ni + fixed_i)))
    columns = [column_record(rule, index, data) for rule, index in selected.items()]
    return science, columns


def inspect(sample, request_path, label):
    """Decode and cross-check one sample with the available decoders; fail closed on any mismatch."""
    with open(request_path, encoding="utf-8") as handle:
        request_bytes = handle.read().replace("\r\n", "\n").encode("utf-8")
    request = json.loads(request_bytes)
    manifest, rows = audit.load_manifest(sample)
    identities = [(row["kind"], row["variable"], row["step"], row["level"]) for row in rows]
    if sorted(set(identities)) != sorted(expected_identities(request)) or len(identities) != len(set(identities)):
        raise audit.AuditError("sample file set differs from the #231 request")
    if {row["cycle"] for row in rows} != {request["cycle"]}:
        raise audit.AuditError("sample cycle differs from the #231 request")
    libaec = sections.load_libaec()
    fields, records = {}, []
    for row in sorted(rows, key=lambda r: r["path"]):
        record, values = decode(row, libaec)
        records.append(record)
        fields[(row["kind"], row["variable"], row["step"], row["level"])] = values
    identity = identity_science(records)
    singleton = ("distinct_grids", "distinct_data_date_time", "distinct_forecast_time_and_unit",
                 "distinct_vertical_grid", "hhl_surfaces", "packing_templates")
    if identity["surface_identity_problems"] or any(len(identity[key]) != 1 for key in singleton):
        raise audit.AuditError(f"vertical identity is not unique: {identity}")
    science, columns = vertical_science(fields, records)
    science["identity"] = identity
    with open(os.path.join(sample, "retrieval-manifest.json"), encoding="utf-8") as handle:
        manifest_bytes = handle.read().replace("\r\n", "\n").encode("utf-8")
    return {
        "schema": EVIDENCE_SCHEMA, "version": 1, "issue": 231,
        "request_sha256_lf": audit.sha256_bytes(request_bytes),
        "retrieval_manifest_sha256_lf": audit.sha256_bytes(manifest_bytes),
        "retrieval_file_count": len(manifest["files"]),
        "decoder": {"label": label, "eccodes": audit.eccodes_module().codes_get_api_version(),
                    "independent_values": "octet reader + libaec" if libaec is not None else None},
        "messages": records, "science": science, "columns": columns,
    }


def numeric_leaves(value, prefix=""):
    """Yield ``(path, leaf)`` pairs of a nested JSON value."""
    if isinstance(value, dict):
        for key, item in value.items():
            yield from numeric_leaves(item, f"{prefix}.{key}")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield from numeric_leaves(item, f"{prefix}[{index}]")
    else:
        yield prefix, value


def summarize(primary, secondary):
    """Merge two decoder runs only when identities, raw columns and derived numbers agree."""
    if primary["decoder"]["independent_values"] is None:
        raise audit.AuditError("primary evidence lacks the independent libaec value decode")
    strip = lambda records: [{k: v for k, v in r.items() if k != "independent_values_max_abs_difference"}  # noqa: E731
                             for r in records]
    if strip(primary["messages"]) != strip(secondary["messages"]):
        raise audit.AuditError("message identities or decoded value hashes differ between decoders")
    raw = lambda columns: [{k: v for k, v in c.items() if k != "derived"} for c in columns]  # noqa: E731
    if raw(primary["columns"]) != raw(secondary["columns"]):
        raise audit.AuditError("representative column selection or raw values differ between decoders")
    worst = 0.0
    left, right = dict(numeric_leaves(primary["science"])), dict(numeric_leaves(secondary["science"]))
    left.update(dict(numeric_leaves(primary["columns"], "columns")))
    right.update(dict(numeric_leaves(secondary["columns"], "columns")))
    if left.keys() != right.keys():
        raise audit.AuditError("derived evidence structure differs between decoders")
    for key, value in left.items():
        other = right[key]
        if isinstance(value, float) and isinstance(other, float):
            scale = max(abs(value), abs(other))
            difference = 0.0 if scale == 0.0 else abs(value - other) / scale
            if difference > SUMMARY_RELATIVE_TOLERANCE:
                raise audit.AuditError(f"{key}: {value} vs {other}")
            worst = max(worst, difference)
        elif value != other:
            raise audit.AuditError(f"{key}: {value!r} vs {other!r}")
    evidence = dict(primary)
    evidence["status"] = "DERIVED_EVIDENCE_RAW_BYTES_AUTHORITATIVE"
    evidence["secondary_decoder"] = secondary["decoder"]
    evidence["secondary_max_relative_difference"] = worst
    return evidence


def write_json(path, value):
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(value, handle, indent=1, sort_keys=False)
        handle.write("\n")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    inspect_parser = sub.add_parser("inspect")
    inspect_parser.add_argument("--sample", required=True)
    inspect_parser.add_argument("--request", required=True)
    inspect_parser.add_argument("--label", required=True)
    inspect_parser.add_argument("--output", required=True)
    summarize_parser = sub.add_parser("summarize")
    summarize_parser.add_argument("--primary", required=True)
    summarize_parser.add_argument("--secondary", required=True)
    summarize_parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)
    if args.command == "inspect":
        write_json(args.output, inspect(args.sample, args.request, args.label))
    else:
        loaded = []
        for path in (args.primary, args.secondary):
            with open(path, encoding="utf-8") as handle:
                loaded.append(json.load(handle))
        write_json(args.output, summarize(*loaded))
    print(f"wrote {args.output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (audit.AuditError, sections.SectionError, fetch.FetchError) as error:
        print(f"icon_eu_vertical_231: FAILED: {error}", file=sys.stderr)
        sys.exit(1)
