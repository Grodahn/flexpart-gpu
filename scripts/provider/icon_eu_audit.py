#!/usr/bin/env python3
"""Inspect real ICON-EU GRIB2 bytes and check them against the #230 contract.

Research tooling for issue #230. It is isolated from production decoding and
does not build canonical #29 snapshots (that is #32-B). Subcommands:

``inspect``
    Verify every downloaded file against its retrieval manifest, decode every
    message with ecCodes, cross-check the metadata (and, with libaec, the
    values) with the independent octet reader ``icon_eu_grib2_sections``, run
    the decoded-value science checks of docs/research/icon-eu-provider-230.md
    and write a derived evidence JSON. The evidence is derived; the raw bytes
    and their SHA-256 hashes remain authoritative.

``check``
    Check every message of a sample directory against the frozen message
    identities of the provider contract (cycle, parameter, level, grid, time
    unit, statistical processing, step coverage). Uses only the independent
    reader, so it runs without ecCodes. Any violation exits non-zero.

Usage::

    python -I scripts/provider/icon_eu_audit.py inspect --sample <fetch dir> \
        --label <decoder label> --output <evidence.json>
    python -I scripts/provider/icon_eu_audit.py check --sample <dir> \
        --contract fixtures/meteorology/icon-eu-audit-v1/provider-contract.json
"""

import argparse
import datetime
import hashlib
import json
import math
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import icon_eu_grib2_sections as sections  # noqa: E402  (independent reader, same directory)

EVIDENCE_SCHEMA = "flexpart-gpu.icon-eu-audit-evidence"
FILE_PATTERN = re.compile(
    r"icon-eu_europe_regular-lat-lon_(?P<kind>[a-z-]+)_(?P<cycle>\d{10})"
    r"(?:_(?P<step>\d{3}))?(?:_(?P<level>\d+))?_(?P<variable>[A-Z0-9_]+)\.grib2$")
ECCODES_KEYS = (
    "edition", "centre", "subCentre", "tablesVersion", "localTablesVersion",
    "generatingProcessIdentifier", "backgroundProcess", "typeOfGeneratingProcess",
    "productionStatusOfProcessedData", "typeOfProcessedData", "discipline",
    "parameterCategory", "parameterNumber", "shortName", "paramId", "name", "units",
    "typeOfLevel", "level", "typeOfFirstFixedSurface", "scaledValueOfFirstFixedSurface",
    "typeOfSecondFixedSurface", "scaledValueOfSecondFixedSurface", "numberOfVGridUsed",
    "nlev", "uuidOfVGrid", "productDefinitionTemplateNumber", "dataDate", "dataTime",
    "stepUnits", "stepType", "stepRange", "startStep", "endStep", "forecastTime",
    "indicatorOfUnitOfTimeRange", "typeOfStatisticalProcessing", "lengthOfTimeRange",
    "indicatorOfUnitForTimeRange", "validityDate", "validityTime", "gridType",
    "gridDefinitionTemplateNumber", "Ni", "Nj", "latitudeOfFirstGridPointInDegrees",
    "longitudeOfFirstGridPointInDegrees", "latitudeOfLastGridPointInDegrees",
    "longitudeOfLastGridPointInDegrees", "iDirectionIncrementInDegrees",
    "jDirectionIncrementInDegrees", "iScansNegatively", "jScansPositively",
    "jPointsAreConsecutive", "shapeOfTheEarth", "uvRelativeToGrid", "packingType",
    "dataRepresentationTemplateNumber", "bitsPerValue", "decimalScaleFactor",
    "binaryScaleFactor", "referenceValue", "ccsdsFlags", "ccsdsBlockSize", "ccsdsRsi",
    "bitmapPresent", "numberOfMissing", "numberOfValues", "numberOfDataPoints",
    "md5Section7", "totalLength", "localDefinitionNumber")
STRING_KEYS = {"shortName", "name", "units", "typeOfLevel", "stepUnits", "stepType",
               "stepRange", "startStep", "endStep", "gridType", "packingType",
               "md5Section7", "uuidOfVGrid", "typeOfProcessedData", "centre"}
# Physical constants used only by the consistency checks below.
DRY_AIR_GAS_CONSTANT = 287.05  # J kg-1 K-1, FLEXPART r_air
GRAVITY = 9.81  # m s-2, FLEXPART ga
ICON_EARTH_RADIUS_M = 6371229.0  # GRIB2 shapeOfTheEarth=6
PRECIPITATION_COMPONENTS = ("RAIN_GSP", "RAIN_CON", "SNOW_GSP", "SNOW_CON")
HHL_LEVELS = 75
FULL_LEVELS = 74


class AuditError(RuntimeError):
    """The sample is incomplete, inconsistent or violates the frozen contract."""


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def describe(path):
    match = FILE_PATTERN.search(os.path.basename(path))
    if not match:
        raise AuditError(f"unrecognised sample file name {path}")
    info = match.groupdict()
    info["step"] = None if info["step"] is None else int(info["step"])
    info["level"] = None if info["level"] is None else int(info["level"])
    return info


def load_manifest(sample):
    """Return manifest rows after re-hashing every GRIB2 file (fail closed)."""
    with open(os.path.join(sample, "retrieval-manifest.json"), encoding="utf-8") as handle:
        manifest = json.load(handle)
    rows = []
    for item in manifest["files"]:
        grib_path = os.path.join(sample, item["path"][: -len(".bz2")])
        with open(grib_path, "rb") as handle:
            data = handle.read()
        if sha256_bytes(data) != item["grib2_sha256"] or len(data) != item["grib2_bytes"]:
            raise AuditError(f"{grib_path}: bytes differ from the retrieval manifest")
        rows.append(dict(item, grib_path=grib_path, **describe(grib_path)))
    return manifest, rows


# --------------------------------------------------------------------------- ecCodes


def eccodes_module():
    import eccodes  # noqa: PLC0415  (optional dependency; check mode does not need it)
    return eccodes


def eccodes_messages(path):
    """Yield (keys, values) for every message in a file using ecCodes."""
    eccodes = eccodes_module()
    import numpy  # noqa: PLC0415
    with open(path, "rb") as handle:
        while True:
            handle_id = eccodes.codes_grib_new_from_file(handle)
            if handle_id is None:
                return
            try:
                keys = {}
                for key in ECCODES_KEYS:
                    if not eccodes.codes_is_defined(handle_id, key):
                        continue
                    if key in STRING_KEYS:
                        keys[key] = eccodes.codes_get_string(handle_id, key)
                    else:
                        keys[key] = eccodes.codes_get(handle_id, key)
                values = numpy.asarray(eccodes.codes_get_values(handle_id), dtype="<f8")
            finally:
                eccodes.codes_release(handle_id)
            yield keys, values


def value_summary(values):
    import numpy  # noqa: PLC0415
    finite = numpy.isfinite(values)
    return {
        "count": int(values.size),
        "non_finite": int(values.size - finite.sum()),
        "min": float(values.min()),
        "max": float(values.max()),
        "mean": float(values.mean()),
        "values_sha256_f8le": sha256_bytes(values.astype("<f8").tobytes()),
    }


def jsonable(value):
    if isinstance(value, float) and not math.isfinite(value):
        return repr(value)
    if hasattr(value, "item"):
        return value.item()
    return value


# ------------------------------------------------------------- independent cross-check

CROSS_CHECK = (
    ("edition", lambda p: p["edition"]),
    ("discipline", lambda p: p["discipline"]),
    ("parameterCategory", lambda p: p["product"]["parameter_category"]),
    ("parameterNumber", lambda p: p["product"]["parameter_number"]),
    ("productDefinitionTemplateNumber", lambda p: p["product"]["template"]),
    ("typeOfGeneratingProcess", lambda p: p["product"]["type_of_generating_process"]),
    ("indicatorOfUnitOfTimeRange", lambda p: p["product"]["unit_of_time_range"]),
    ("forecastTime", lambda p: p["product"]["forecast_time"]),
    ("Ni", lambda p: p["grid"]["ni"]),
    ("Nj", lambda p: p["grid"]["nj"]),
    ("shapeOfTheEarth", lambda p: p["grid"]["shape_of_earth"]),
    ("dataRepresentationTemplateNumber", lambda p: p["representation"]["template"]),
    ("bitsPerValue", lambda p: p["representation"]["bits_per_value"]),
    ("binaryScaleFactor", lambda p: p["representation"]["binary_scale_factor"]),
    ("decimalScaleFactor", lambda p: p["representation"]["decimal_scale_factor"]),
    ("numberOfValues", lambda p: p["representation"]["number_of_values"]),
    ("ccsdsFlags", lambda p: p["representation"]["ccsds_flags"]),
    ("ccsdsBlockSize", lambda p: p["representation"]["ccsds_block_size"]),
    ("ccsdsRsi", lambda p: p["representation"]["ccsds_rsi"]),
    ("totalLength", lambda p: p["total_length"]),
)
CROSS_CHECK_FLOAT = (
    ("latitudeOfFirstGridPointInDegrees", lambda p: p["grid"]["first_lat_deg"]),
    ("longitudeOfFirstGridPointInDegrees", lambda p: p["grid"]["first_lon_deg"]),
    ("latitudeOfLastGridPointInDegrees", lambda p: p["grid"]["last_lat_deg"]),
    ("longitudeOfLastGridPointInDegrees", lambda p: p["grid"]["last_lon_deg"]),
    ("iDirectionIncrementInDegrees", lambda p: p["grid"]["di_deg"]),
    ("jDirectionIncrementInDegrees", lambda p: p["grid"]["dj_deg"]),
    ("referenceValue", lambda p: p["representation"]["reference_value"]),
)
CROSS_CHECK_STAT = (
    ("typeOfStatisticalProcessing", "statistical_processing"),
    ("lengthOfTimeRange", "statistical_range_length"),
    ("indicatorOfUnitForTimeRange", "unit_of_statistical_range"),
)


def cross_check(keys, parsed):
    """Return ecCodes-vs-octet disagreements (empty list means agreement)."""
    problems = []
    for key, getter in CROSS_CHECK:
        if key in keys and int(keys[key]) != int(getter(parsed)):
            problems.append(f"{key}: eccodes {keys[key]} vs octets {getter(parsed)}")
    for key, getter in CROSS_CHECK_FLOAT:
        if key in keys and abs(float(keys[key]) - float(getter(parsed))) > 1.0e-6:
            problems.append(f"{key}: eccodes {keys[key]} vs octets {getter(parsed)}")
    if parsed["product"]["template"] == 8:
        for key, field in CROSS_CHECK_STAT:
            if int(keys.get(key, -1)) != int(parsed["product"][field]):
                problems.append(f"{key}: eccodes {keys.get(key)} vs octets {parsed['product'][field]}")
    if int(keys.get("bitmapPresent", 0)) != 0 or parsed["bitmap_indicator"] != sections.NO_BITMAP:
        problems.append("bitmap present")
    return problems


# ---------------------------------------------------------------- science checks


class FieldStore:
    """Decoded values of the sample, loaded on demand and keyed by identity."""

    def __init__(self, rows):
        self.paths = {}
        for row in rows:
            self.paths[(row["cycle"], row["kind"], row["variable"], row["step"], row["level"])] = row["grib_path"]
        self.cycle = min(row["cycle"] for row in rows)

    def get(self, kind, variable, step=None, level=None, cycle=None):
        import numpy  # noqa: PLC0415
        key = (cycle or self.cycle, kind, variable, step, level)
        if key not in self.paths:
            raise AuditError(f"science check needs missing sample {key}")
        (keys, values), = list(eccodes_messages(self.paths[key]))
        return numpy.asarray(values, dtype="<f8").reshape(int(keys["Nj"]), int(keys["Ni"]))

    def keys(self, kind, variable, step=None, level=None):
        (keys, _), = list(eccodes_messages(self.paths[(self.cycle, kind, variable, step, level)]))
        return keys


def quantum(keys):
    return 2.0 ** int(keys["binaryScaleFactor"]) / 10.0 ** int(keys["decimalScaleFactor"])


def stats(array):
    import numpy  # noqa: PLC0415
    return {"min": float(numpy.min(array)), "max": float(numpy.max(array)),
            "mean": float(numpy.mean(array)), "p01": float(numpy.percentile(array, 1)),
            "p50": float(numpy.percentile(array, 50)), "p99": float(numpy.percentile(array, 99))}


def check_vertical(store):
    """HHL count/order/flat top, HHL bottom vs HSURF, P ordering and hydrostatic consistency."""
    import numpy  # noqa: PLC0415
    hsurf = store.get("time-invariant", "HSURF")
    previous = store.get("time-invariant", "HHL", level=1)
    constant_levels, min_thickness = [], []
    for level in range(1, HHL_LEVELS + 1):
        current = previous if level == 1 else store.get("time-invariant", "HHL", level=level)
        if float(current.max() - current.min()) == 0.0:
            constant_levels.append(level)
        if level > 1:
            min_thickness.append(float((previous - current).min()))
        if level == FULL_LEVELS:
            hhl_74 = current
        previous = current
    hhl_75 = previous
    ps = store.get("single-level", "PS", step=0)
    p74 = store.get("model-level", "P", step=0, level=74)
    p73 = store.get("model-level", "P", step=0, level=73)
    t74 = store.get("model-level", "T", step=0, level=74)
    t73 = store.get("model-level", "T", step=0, level=73)
    q74 = store.get("model-level", "QV", step=0, level=74)
    q73 = store.get("model-level", "QV", step=0, level=73)
    hhl_73 = store.get("time-invariant", "HHL", level=73)
    z74 = 0.5 * (hhl_74 + hhl_75)
    z73 = 0.5 * (hhl_73 + hhl_74)
    virtual_mean = 0.5 * (t74 * (1 + 0.608 * q74) + t73 * (1 + 0.608 * q73))
    hypsometric = DRY_AIR_GAS_CONSTANT / GRAVITY * virtual_mean * numpy.log(p74 / p73)
    ratio = hypsometric / (z73 - z74)
    return {
        "hhl_level_count": HHL_LEVELS,
        "hhl_strictly_decreasing_with_level_index": bool(min(min_thickness) > 0.0),
        "hhl_min_layer_thickness_m": min(min_thickness),
        "hhl_lowest_layer_min_thickness_m": min_thickness[-1],
        "hhl_horizontally_constant_levels": constant_levels,
        "hhl_level1_m": float(store.get("time-invariant", "HHL", level=1)[0, 0]),
        "hhl75_minus_hsurf_m": stats(hhl_75 - hsurf),
        "p74_below_surface_pressure_everywhere": bool((p74 < ps).all()),
        "p73_below_p74_everywhere": bool((p73 < p74).all()),
        "ps_minus_p74_pa": stats(ps - p74),
        "hydrostatic_ratio_levels_73_74": stats(ratio),
        "hydrostatic_definition": "ratio = (Rd/g) * mean(Tv73,Tv74) * ln(p74/p73) / (zf73 - zf74), zf(k) = 0.5*(HHL(k)+HHL(k+1)), Rd=287.05, g=9.81",
    }


def check_surface_w(store):
    """Correlate W at the surface half level with the kinematic terrain term v.grad(h)."""
    import numpy  # noqa: PLC0415
    hsurf = store.get("time-invariant", "HSURF")
    u74 = store.get("model-level", "U", step=0, level=74)
    v74 = store.get("model-level", "V", step=0, level=74)
    w75 = store.get("model-level", "W", step=0, level=75)
    w74 = store.get("model-level", "W", step=0, level=74)
    keys = store.keys("time-invariant", "HSURF")
    if float(keys["iDirectionIncrementInDegrees"]) != float(keys["jDirectionIncrementInDegrees"]):
        raise AuditError("surface-W check assumes equal i/j grid increments")
    first_lat = float(keys["latitudeOfFirstGridPointInDegrees"])
    increment = float(keys["jDirectionIncrementInDegrees"])
    lat = numpy.deg2rad(first_lat + increment * numpy.arange(hsurf.shape[0]))[:, None]
    step_rad = numpy.deg2rad(increment)
    dhdx = (hsurf[1:-1, 2:] - hsurf[1:-1, :-2]) / (2 * ICON_EARTH_RADIUS_M * numpy.cos(lat[1:-1]) * step_rad)
    dhdy = (hsurf[2:, 1:-1] - hsurf[:-2, 1:-1]) / (2 * ICON_EARTH_RADIUS_M * step_rad)
    kinematic = u74[1:-1, 1:-1] * dhdx + v74[1:-1, 1:-1] * dhdy
    surface = w75[1:-1, 1:-1]
    slope = float((surface * kinematic).sum() / (kinematic * kinematic).sum())
    flat = numpy.abs(dhdx) + numpy.abs(dhdy) < 1.0e-5
    return {
        "w75_abs_max_m_s": float(numpy.abs(w75).max()),
        "w75_fraction_nonzero": float((w75 != 0).mean()),
        "w75_vs_v_grad_h_correlation": float(numpy.corrcoef(surface.ravel(), kinematic.ravel())[0, 1]),
        "w75_vs_v_grad_h_regression_slope": slope,
        "w75_abs_mean_over_flat_cells_m_s": float(numpy.abs(surface[flat]).mean()),
        "w74_vs_w75_correlation": float(numpy.corrcoef(w74.ravel(), w75.ravel())[0, 1]),
        "definition": "v.grad(h) = U74*dHSURF/dx + V74*dHSURF/dy, centred differences on the message grid, sphere R=6371229 m; flat cells |grad h| < 1e-5",
    }


def check_precipitation(store, rows):
    """Closure TOT_PREC vs components, accumulation monotonicity and unit plausibility."""
    import numpy  # noqa: PLC0415
    steps = sorted({row["step"] for row in rows if row["variable"] == "TOT_PREC" and row["cycle"] == store.cycle})
    closure, monotonic = [], []
    previous = {}
    for step in steps:
        total = store.get("single-level", "TOT_PREC", step=step)
        components = {name: store.get("single-level", name, step=step) for name in PRECIPITATION_COMPONENTS}
        tolerance = 0.5 * sum(quantum(store.keys("single-level", name, step=step))
                              for name in PRECIPITATION_COMPONENTS + ("TOT_PREC",))
        residual = total - sum(components.values())
        closure.append({"step": step, "residual_kg_m2": stats(residual),
                        "half_quantum_sum_kg_m2": tolerance,
                        "fraction_abs_residual_above_tolerance": float((numpy.abs(residual) > tolerance).mean()),
                        "fraction_residual_below_minus_tolerance": float((residual < -tolerance).mean())})
        for name, values in dict(components, TOT_PREC=total).items():
            if name in previous:
                last_step, last_values = previous[name]
                delta = values - last_values
                monotonic.append({"variable": name, "from_step": last_step, "to_step": step,
                                  "decreasing_cells": int((delta < 0).sum()),
                                  "most_negative_delta": float(delta.min()),
                                  "later_quantum": quantum(store.keys("single-level", name, step=step))})
            previous[name] = (step, values)
    rain_48 = store.get("single-level", "RAIN_GSP", step=48)
    return {"steps": steps, "closure": closure, "monotonic_pairs": monotonic,
            "rain_gsp_48h_max_value": float(rain_48.max()),
            "rain_gsp_48h_max_if_read_as_rate_mm_per_h": float(rain_48.max() * 3600.0)}


def check_surface_fluxes(store):
    """Daytime sign of averaged fluxes, de-averaging amplification and stress/wind sign."""
    import numpy  # noqa: PLC0415
    land = store.get("time-invariant", "FR_LAND") > 0.5
    result = {}
    for name in ("ASHFL_S", "ASOB_S", "AUMFL_S", "AVMFL_S"):
        a12 = store.get("single-level", name, step=12)
        a13 = store.get("single-level", name, step=13)
        hourly = 13.0 * a13 - 12.0 * a12
        result[name] = {
            "average_0_13_land_mean": float(a13[land].mean()),
            "derived_mean_12_13_land_mean": float(hourly[land].mean()),
            "derived_mean_12_13_min": float(hourly.min()),
            "derived_mean_12_13_max": float(hourly.max()),
            "quantum_step13": quantum(store.keys("single-level", name, step=13)),
            "derived_mean_quantization_bound": 13 * 0.5 * quantum(store.keys("single-level", name, step=13))
            + 12 * 0.5 * quantum(store.keys("single-level", name, step=12)),
        }
    u10 = store.get("single-level", "U_10M", step=13)
    v10 = store.get("single-level", "V_10M", step=13)
    aum = 13.0 * store.get("single-level", "AUMFL_S", step=13) - 12.0 * store.get("single-level", "AUMFL_S", step=12)
    avm = 13.0 * store.get("single-level", "AVMFL_S", step=13) - 12.0 * store.get("single-level", "AVMFL_S", step=12)
    result["AUMFL_S_vs_U_10M_correlation"] = float(numpy.corrcoef(aum.ravel(), u10.ravel())[0, 1])
    result["AVMFL_S_vs_V_10M_correlation"] = float(numpy.corrcoef(avm.ravel(), v10.ravel())[0, 1])
    return result


def check_snow_and_cloud(store):
    import numpy  # noqa: PLC0415
    depth = store.get("single-level", "H_SNOW", step=0)
    water = store.get("single-level", "W_SNOW", step=0)
    snow = (depth > 0.05) & (water < 1000.0)
    density = water[snow] / depth[snow]
    clct = store.get("single-level", "CLCT", step=0)
    clc = store.get("model-level", "CLC", step=1, level=60)
    return {
        "snow_bulk_density_kg_m3_where_depth_gt_5cm": stats(density),
        "w_snow_max_kg_m2": float(water.max()),
        "clct_range_percent": [float(clct.min()), float(clct.max())],
        "clc60_range_percent": [float(clc.min()), float(clc.max())],
    }


def identity_checks(rows, messages):
    """Grid, cycle, vertical-grid UUID and time-unit identity across all messages."""
    grids, uuids, problems = set(), set(), []
    for row, keys in zip(rows, messages):
        grids.add(tuple(keys[k] for k in (
            "Ni", "Nj", "latitudeOfFirstGridPointInDegrees", "longitudeOfFirstGridPointInDegrees",
            "latitudeOfLastGridPointInDegrees", "longitudeOfLastGridPointInDegrees",
            "iDirectionIncrementInDegrees", "jDirectionIncrementInDegrees", "iScansNegatively",
            "jScansPositively", "jPointsAreConsecutive", "shapeOfTheEarth")))
        reference = "%08d%02d" % (int(keys["dataDate"]), int(keys["dataTime"]) // 100)
        if reference != row["cycle"]:
            problems.append(f"{row['path']}: reference {reference} vs cycle {row['cycle']}")
        if int(keys["indicatorOfUnitOfTimeRange"]) != 0:
            problems.append(f"{row['path']}: time unit {keys['indicatorOfUnitOfTimeRange']}")
        if "uuidOfVGrid" in keys:
            uuids.add(keys["uuidOfVGrid"])
    return {"distinct_grids": [list(grid) for grid in sorted(grids)],
            "distinct_vertical_grid_uuids": sorted(uuids),
            "cycle_and_time_unit_problems": problems}


def inspect(sample, label):
    eccodes = eccodes_module()
    manifest, rows = load_manifest(sample)
    libaec = sections.load_libaec()
    files, messages, problems = [], [], []
    for row in rows:
        with open(row["grib_path"], "rb") as handle:
            raw = handle.read()
        parsed_messages = sections.split_messages(raw)
        decoded = list(eccodes_messages(row["grib_path"]))
        if len(decoded) != 1 or len(parsed_messages) != 1:
            raise AuditError(f"{row['path']}: expected exactly one message")
        (keys, values), = decoded
        parsed = sections.parse(parsed_messages[0])
        mismatches = cross_check(keys, parsed)
        independent = None
        if libaec is not None:
            import numpy  # noqa: PLC0415
            other = numpy.asarray(sections.decode_values(parsed_messages[0], libaec), dtype="<f8")
            independent = float(numpy.abs(other - values).max())
        problems.extend(f"{row['path']}: {item}" for item in mismatches)
        messages.append(keys)
        files.append({
            "path": row["path"][: -len(".bz2")],
            "grib2_sha256": row["grib2_sha256"],
            "eccodes_keys": {key: jsonable(value) for key, value in keys.items()},
            "octet_metadata": parsed,
            "eccodes_values": value_summary(values),
            "independent_max_abs_difference": independent,
            "cross_check_problems": mismatches,
        })
    store = FieldStore(rows)
    science = {
        "identity": identity_checks(rows, messages),
        "vertical": check_vertical(store),
        "surface_w": check_surface_w(store),
        "precipitation": check_precipitation(store, rows),
        "surface_fluxes": check_surface_fluxes(store),
        "snow_and_cloud": check_snow_and_cloud(store),
    }
    return {
        "schema": EVIDENCE_SCHEMA,
        "version": 1,
        "issue": 230,
        "label": label,
        "generated_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "decoder": {"eccodes_api_version": eccodes.codes_get_api_version(),
                    "python": sys.version.split()[0],
                    "independent_values_decoded_with_libaec": libaec is not None},
        "retrieval_manifest_sha256": sha256_bytes(open(os.path.join(sample, "retrieval-manifest.json"), "rb").read()),
        "file_count": len(files),
        "cross_check_problem_count": len(problems),
        "files": files,
        "science": science,
    }


# ------------------------------------------------------------------ contract check


def offset_time(reference, minutes):
    base = datetime.datetime.strptime(reference, "%Y-%m-%dT%H:%M:%SZ")
    return (base + datetime.timedelta(minutes=minutes)).strftime("%Y-%m-%dT%H:%M:%SZ")


def check_message(parsed, expected, grid, cycle, step, level):
    """Return violations of one message against its frozen contract identity."""
    violations = []
    product = parsed["product"]
    reference = "%s-%s-%sT%s:00:00Z" % (cycle[0:4], cycle[4:6], cycle[6:8], cycle[8:10])
    actual_grid = {key: parsed["grid"][key] for key in grid}
    if actual_grid != grid:
        violations.append(f"grid {actual_grid} differs from contract grid")
    if parsed["identification"]["reference_time"] != reference:
        violations.append(f"reference time {parsed['identification']['reference_time']} is not cycle {reference}")
    if parsed["identification"]["centre"] != expected["centre"]:
        violations.append(f"centre {parsed['identification']['centre']}")
    identity = (parsed["discipline"], product["parameter_category"], product["parameter_number"])
    if list(identity) != expected["parameter"]:
        violations.append(f"parameter {identity} is not {expected['parameter']} ({expected['unit']})")
    if product["template"] != expected["product_template"]:
        violations.append(f"product template 4.{product['template']}")
    if product["first_surface_type"] != expected["first_surface_type"]:
        violations.append(f"first fixed surface {product['first_surface_type']}")
    if product["second_surface_type"] != expected["second_surface_type"]:
        violations.append(f"second fixed surface {product['second_surface_type']}")
    expected_level = level if level is not None else expected.get("surface_value", 0)
    if product["first_surface_scaled_value"] != expected_level:
        violations.append(f"level {product['first_surface_scaled_value']} is not {expected_level}")
    if product["unit_of_time_range"] != 0:
        violations.append(f"time unit indicator {product['unit_of_time_range']} is not minutes (0)")
    minutes = 0 if step is None else 60 * step
    if product["template"] == 0:
        if product["forecast_time"] != minutes:
            violations.append(f"forecast time {product['forecast_time']} min is not step {step}")
    else:
        if product["statistical_processing"] != expected.get("statistical_processing"):
            violations.append(f"statistical processing {product['statistical_processing']} is not "
                              f"{expected.get('statistical_processing')}")
        if product["forecast_time"] != 0:
            violations.append(f"statistics start {product['forecast_time']} min after the cycle, not at the run start")
        if product["unit_of_statistical_range"] != 0 or product["statistical_range_length"] != minutes:
            violations.append(f"statistical range {product['statistical_range_length']} "
                              f"(unit {product['unit_of_statistical_range']}) is not 0..step {step}")
        if product["time_range_count"] != 1 or product["end_of_interval"] != offset_time(reference, minutes):
            violations.append(f"end of interval {product['end_of_interval']} does not match step {step}")
    if parsed["representation"]["template"] != sections.CCSDS_TEMPLATE:
        violations.append("unexpected packing")
    return violations


def check_series(steps, required_steps):
    """Return the missing steps of a required consecutive coverage."""
    return sorted(set(required_steps) - set(steps))


def check_sample(sample, contract, profile):
    """Check every message of a sample directory; return a list of violations."""
    identities = contract["message_identities"]
    grid = contract["grid"]["octet_identity"]
    violations, seen = [], {}
    names = sorted(name for root, _, files in os.walk(sample) for name in files if name.endswith(".grib2"))
    paths = {name: os.path.join(root, name) for root, _, files in os.walk(sample) for name in files}
    for name in names:
        info = describe(name)
        expected = identities.get(info["variable"])
        if expected is None:
            violations.append(f"{name}: variable not in contract")
            continue
        with open(paths[name], "rb") as handle:
            data = handle.read()
        try:
            messages = sections.split_messages(data)
            if len(messages) != 1:
                raise sections.SectionError(f"{len(messages)} messages in one file")
            parsed = sections.parse(messages[0])
        except sections.SectionError as error:
            violations.append(f"{name}: corrupt message: {error}")
            continue
        if info["cycle"] != contract["cycle"]["cycle"]:
            violations.append(f"{name}: forecast cycle {info['cycle']} is not the contract cycle")
        for item in check_message(parsed, expected, grid, contract["cycle"]["cycle"], info["step"], info["level"]):
            violations.append(f"{name}: {item}")
        seen.setdefault(info["variable"], []).append(info["step"])
    if profile not in contract["coverage_profiles"]:
        raise AuditError(f"unknown coverage profile {profile!r}")
    for variable, required in contract["coverage_profiles"][profile]["steps"].items():
        missing = check_series(seen.get(variable, []), required)
        if missing:
            violations.append(f"{variable}: insufficient temporal coverage, missing steps {missing}")
    return violations


LABEL_KEYS = ("shortName", "paramId", "units", "typeOfLevel", "stepType", "stepRange")


def compact_row(primary, secondary):
    """One committed evidence row: octet identity, packing, values and decoder labels."""
    octets, product, rep = primary["octet_metadata"], primary["octet_metadata"]["product"], \
        primary["octet_metadata"]["representation"]
    row = {
        "path": primary["path"],
        "grib2_sha256": primary["grib2_sha256"],
        "parameter": [octets["discipline"], product["parameter_category"], product["parameter_number"]],
        "product_template": product["template"],
        "first_surface": [product["first_surface_type"], product["first_surface_scaled_value"]],
        "second_surface_type": product["second_surface_type"],
        "reference_time": octets["identification"]["reference_time"],
        "unit_of_time_range": product["unit_of_time_range"],
        "forecast_time": product["forecast_time"],
        "packing": [rep["template"], rep["bits_per_value"], rep["binary_scale_factor"],
                    rep["decimal_scale_factor"], rep["reference_value"], rep["ccsds_flags"],
                    rep["ccsds_block_size"], rep["ccsds_rsi"]],
        "values": {key: primary["eccodes_values"][key] for key in ("min", "max", "mean", "values_sha256_f8le")},
        "independent_max_abs_difference": primary["independent_max_abs_difference"],
        "values_identical_across_decoders":
            primary["eccodes_values"]["values_sha256_f8le"] == secondary["eccodes_values"]["values_sha256_f8le"],
        "labels": {"primary": {key: primary["eccodes_keys"].get(key) for key in LABEL_KEYS},
                   "secondary": {key: secondary["eccodes_keys"].get(key) for key in LABEL_KEYS}},
    }
    if product["template"] == 8:
        row["statistics"] = [product["statistical_processing"], product["unit_of_statistical_range"],
                             product["statistical_range_length"], product["end_of_interval"]]
    if "uuidOfVGrid" in primary["eccodes_keys"]:
        row["vertical_grid"] = [primary["eccodes_keys"]["uuidOfVGrid"], primary["eccodes_keys"]["nlev"]]
    return row


def summarize(primary, secondary):
    """Merge two decoder runs into the compact committed evidence (fail on disagreement)."""
    by_path = {item["path"]: item for item in secondary["files"]}
    if set(by_path) != {item["path"] for item in primary["files"]}:
        raise AuditError("decoder runs inspected different file sets")
    rows = [compact_row(item, by_path[item["path"]]) for item in primary["files"]]
    if not all(row["values_identical_across_decoders"] for row in rows):
        raise AuditError("decoded values differ between decoders")
    if primary["cross_check_problem_count"] or secondary["cross_check_problem_count"]:
        raise AuditError("ecCodes and octet metadata disagree")
    return {
        "schema": EVIDENCE_SCHEMA + "-compact",
        "version": 1,
        "issue": 230,
        "status": "DERIVED_EVIDENCE: the raw GRIB2 bytes identified by grib2_sha256 are authoritative",
        "retrieval_manifest_sha256": primary["retrieval_manifest_sha256"],
        "decoders": {"primary": dict(primary["decoder"], label=primary["label"], generated_utc=primary["generated_utc"]),
                     "secondary": dict(secondary["decoder"], label=secondary["label"],
                                       generated_utc=secondary["generated_utc"])},
        "file_count": len(rows),
        "rows": rows,
        "science": primary["science"],
        "secondary_science_max_relative_difference": max_relative_difference(
            primary["science"], secondary["science"]),
    }


def max_relative_difference(left, right):
    """Largest relative difference between numeric leaves; structure must match exactly."""
    if isinstance(left, dict):
        if set(left) != set(right):
            raise AuditError("science sections differ in structure")
        return max([max_relative_difference(left[key], right[key]) for key in left] or [0.0])
    if isinstance(left, list):
        if len(left) != len(right):
            raise AuditError("science sections differ in structure")
        return max([max_relative_difference(a, b) for a, b in zip(left, right)] or [0.0])
    if isinstance(left, bool) or not isinstance(left, (int, float)):
        if left != right:
            raise AuditError(f"science value {left!r} differs from {right!r}")
        return 0.0
    if left == right:
        return 0.0
    return abs(left - right) / max(abs(left), abs(right))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    inspect_parser = sub.add_parser("inspect")
    inspect_parser.add_argument("--sample", required=True)
    inspect_parser.add_argument("--label", required=True)
    inspect_parser.add_argument("--output", required=True)
    summarize_parser = sub.add_parser("summarize")
    summarize_parser.add_argument("--primary", required=True)
    summarize_parser.add_argument("--secondary", required=True)
    summarize_parser.add_argument("--output", required=True)
    check_parser = sub.add_parser("check")
    check_parser.add_argument("--sample", required=True)
    check_parser.add_argument("--contract", required=True)
    check_parser.add_argument("--profile", required=True, help="coverage profile in the contract")
    args = parser.parse_args(argv)
    if args.command == "summarize":
        with open(args.primary, encoding="utf-8") as handle:
            primary = json.load(handle)
        with open(args.secondary, encoding="utf-8") as handle:
            secondary = json.load(handle)
        compact = summarize(primary, secondary)
        with open(args.output, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(compact, handle, indent=1, sort_keys=True)
            handle.write("\n")
        print(f"{compact['file_count']} rows written to {args.output}")
        return 0
    if args.command == "inspect":
        evidence = inspect(args.sample, args.label)
        with open(args.output, "w", encoding="utf-8") as handle:
            json.dump(evidence, handle, indent=1, sort_keys=True)
            handle.write("\n")
        print(f"{evidence['file_count']} files, {evidence['cross_check_problem_count']} cross-check problems")
        return 1 if evidence["cross_check_problem_count"] else 0
    with open(args.contract, encoding="utf-8") as handle:
        contract = json.load(handle)
    violations = check_sample(args.sample, contract, args.profile)
    for item in violations:
        print(f"VIOLATION {item}")
    print(f"{len(violations)} contract violations")
    return 1 if violations else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (AuditError, sections.SectionError) as error:
        print(f"icon_eu_audit: FAILED: {error}", file=sys.stderr)
        sys.exit(1)
