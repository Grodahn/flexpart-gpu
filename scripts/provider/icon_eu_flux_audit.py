#!/usr/bin/env python3
"""Derive the #233 evidence for ICON-EU run-start averaged surface fluxes.

Research tooling for issue #233 only. It builds no canonical #29 snapshot, is
not imported by production code and is not a provider adapter (#32-B/C).
Subcommands:

``inspect``
    Re-hash every file of a ``icon_eu_fetch.py`` sample against its retrieval
    manifest, read the metadata of every message with the independent octet
    reader ``icon_eu_grib2_sections`` (cross-checked against ecCodes keys),
    decode the values with ecCodes and write the derived evidence JSON:
    per-message identity and packing quantum, the exact quantization bound of
    every de-averaged interval mean, and the decoded sign/semantics checks
    of docs/research/icon-eu-surface-fluxes-233.md. Any identity or
    cross-check problem exits non-zero. The raw bytes and their SHA-256
    hashes remain authoritative; the evidence is derived.

``compare``
    Fail unless two evidence files (for example the pinned container
    ecCodes 2.24.2 and a host ecCodes) agree: exact for integers, strings and
    hashes, relative ``COMPARE_RELATIVE_TOLERANCE`` for floats (summation
    order only).

De-averaging follows the DWD ICON database reference manual (v2.5.6,
section 7.1.1): a field averaged over ``[0, t]`` gives the mean over
``[t1, t2]`` as ``(t2*A(t2) - t1*A(t1)) / (t2 - t1)``.

Usage::

    python -I scripts/provider/icon_eu_flux_audit.py inspect --sample <fetch dir> \
        --label <decoder label> --output <evidence.json>
    python -I scripts/provider/icon_eu_flux_audit.py compare --primary <a.json> --secondary <b.json>
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

EVIDENCE_SCHEMA = "flexpart-gpu.icon-eu-flux-evidence"
EVIDENCE_VERSION = 1
PRODUCT_PREFIX = "icon-eu_europe_regular-lat-lon"
FILE_PATTERN = re.compile(
    r"^icon-eu_europe_regular-lat-lon_(?P<kind>single-level|time-invariant)_(?P<cycle>\d{10})"
    r"(?:_(?P<step>\d{3}))?_(?P<variable>[A-Z0-9_]+?)\.grib2$")

# Source identities read from the octets: (discipline, category, number).
# Run-start averages (PDT 4.8, statistical processing 0), DWD manual table 5.6.
AVERAGED_FLUXES = {
    "ASHFL_S": (0, 0, 11),   # sensible heat net flux, W m-2
    "ASOB_S": (0, 4, 9),     # net short-wave radiation flux, W m-2
    "AUMFL_S": (0, 2, 17),   # momentum flux u-component, N m-2
    "AVMFL_S": (0, 2, 18),   # momentum flux v-component, N m-2
}
AVERAGED_SHORTWAVE_COMPONENTS = {
    "ASWDIR_S": (0, 4, 198),   # downward direct short-wave (DWD local), W m-2
    "ASWDIFD_S": (0, 4, 199),  # downward diffuse short-wave (DWD local), W m-2
    "ASWDIFU_S": (0, 4, 8),    # upward diffuse short-wave, W m-2
}
INSTANTANEOUS_FIELDS = {
    "U_10M": ((0, 2, 2), 103, 10),
    "V_10M": ((0, 2, 3), 103, 10),
    "T_2M": ((0, 0, 0), 103, 2),
    "T_G": ((0, 0, 0), 1, 0),
}
STATIC_FIELDS = {"FR_LAND": (2, 0, 0), "HSURF": (0, 3, 6)}

PRODUCT_TEMPLATE_STATISTICAL = 8
PRODUCT_TEMPLATE_INSTANTANEOUS = 0
STATISTICAL_AVERAGE = 0
TIME_UNIT_MINUTE = 0
SURFACE_GROUND = 1
MINUTES_PER_HOUR = 60

# Float64 evaluation of t2*A2 - t1*A1 adds rounding of order 1e-13 W m-2; a
# value exactly on the quantization bound must not be reported as a violation.
FLOAT_EVALUATION_SLACK_RELATIVE = 1.0e-9
FLOAT_EVALUATION_SLACK_ABSOLUTE = 1.0e-12
# A cell is "night" for a whole interval only when the sun stays at least
# this far below the geometric horizon at every sample. The margin covers the
# about 0.2 deg error of the Spencer (1971) ephemeris, the 0.0625 deg cell
# extent and the absence of refraction in the model radiation (deg).
NIGHT_MAX_SOLAR_ELEVATION_DEG = -1.0
SOLAR_SAMPLE_MINUTES = 5
LAND_FRACTION_THRESHOLD = 0.5
# Ratios net/downward are evaluated only where the downward short-wave
# average exceeds this value, and residuals are normalised by at least it (W m-2).
LIT_DOWNWARD_SHORTWAVE_W_M2 = 1.0
# Sign checks use only cells with a clear forcing signal at both interval ends.
SIGN_CHECK_MIN_TEMPERATURE_DIFFERENCE_K = 2.0
SIGN_CHECK_MIN_SENSIBLE_HEAT_FLUX_W_M2 = 5.0
SIGN_CHECK_MIN_WIND_SPEED_M_S = 3.0
MAX_LISTED_CELLS = 50
COMPARE_RELATIVE_TOLERANCE = 1.0e-9

SENSIBLE_HEAT_SIGN_INTERVALS = ((12, 13), (23, 24))
STRESS_SIGN_INTERVALS = ((11, 12), (12, 13), (23, 24), (47, 48))
STRESS_WIND_PAIRS = (("AUMFL_S", "U_10M"), ("AVMFL_S", "V_10M"))


class AuditError(RuntimeError):
    """The sample or its evidence is incomplete or inconsistent; never a pass."""


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def canonical_sha256(path):
    """SHA-256 of a text file with LF line endings (checkout-independent)."""
    with open(path, "rb") as handle:
        return sha256_bytes(handle.read().replace(b"\r\n", b"\n"))


# --------------------------------------------------------------------------- sample


def describe(path):
    match = FILE_PATTERN.match(os.path.basename(path))
    if not match:
        raise AuditError(f"unrecognised sample file name {path}")
    info = match.groupdict()
    info["step"] = None if info["step"] is None else int(info["step"])
    return info


def load_sample(sample, manifest_path):
    """Return ``{(variable, step): row}`` after re-hashing every GRIB2 file."""
    with open(manifest_path, encoding="utf-8") as handle:
        manifest = json.load(handle)
    rows = {}
    for item in manifest["files"]:
        relative = item["path"][: -len(".bz2")]
        path = os.path.join(sample, relative)
        with open(path, "rb") as handle:
            data = handle.read()
        if sha256_bytes(data) != item["grib2_sha256"] or len(data) != item["grib2_bytes"]:
            raise AuditError(f"{relative}: bytes differ from the retrieval manifest")
        info = describe(path)
        key = (info["variable"], info["step"])
        if key in rows:
            raise AuditError(f"{relative}: duplicate message identity {key}")
        rows[key] = dict(info, path=relative.replace(os.sep, "/"), data=data,
                         grib2_sha256=item["grib2_sha256"])
    return manifest, rows


def cycle_time(cycle):
    return datetime.datetime.strptime(cycle, "%Y%m%d%H").replace(tzinfo=datetime.timezone.utc)


def iso(moment):
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


# --------------------------------------------------------------------------- identity


def quantum(representation):
    """Packing step 2^E / 10^D of a simple/CCSDS-packed GRIB2 message."""
    return 2.0 ** representation["binary_scale_factor"] / 10.0 ** representation["decimal_scale_factor"]


def packing_error_bound(representation):
    """Largest decoded-value error of one message for round-to-nearest packing.

    Half a quantum, plus half the IEEE float32 spacing of the stored
    reference value R: an encoder may round the field minimum to float32
    after choosing the integer codes, which shifts every decoded value by up
    to that amount (observed as 2.4e-7 W m-2 excesses in the real sample).
    """
    import numpy  # noqa: PLC0415
    # numpy.spacing is negative for negative arguments; the bound needs its size.
    reference_spacing = abs(float(numpy.spacing(numpy.float32(representation["reference_value"]))))
    return 0.5 * quantum(representation) + 0.5 * reference_spacing / 10.0 ** representation["decimal_scale_factor"]


def expected_identity(variable):
    """Return (parameter triple, product template, surface type, surface value)."""
    if variable in AVERAGED_FLUXES:
        return AVERAGED_FLUXES[variable], PRODUCT_TEMPLATE_STATISTICAL, SURFACE_GROUND, 0
    if variable in AVERAGED_SHORTWAVE_COMPONENTS:
        return AVERAGED_SHORTWAVE_COMPONENTS[variable], PRODUCT_TEMPLATE_STATISTICAL, SURFACE_GROUND, 0
    if variable in INSTANTANEOUS_FIELDS:
        triple, surface, value = INSTANTANEOUS_FIELDS[variable]
        return triple, PRODUCT_TEMPLATE_INSTANTANEOUS, surface, value
    if variable in STATIC_FIELDS:
        return STATIC_FIELDS[variable], PRODUCT_TEMPLATE_INSTANTANEOUS, SURFACE_GROUND, 0
    raise AuditError(f"variable {variable} is not part of the #233 sample")


def identity_problems(parsed, variable, cycle, step):
    """List every deviation of one message from its expected #233 identity.

    Run-start averages must be PDT 4.8 averages over ``[run start, valid
    time]`` in minutes with exactly one time range; the file name is never
    trusted.
    """
    triple, template, surface, surface_value = expected_identity(variable)
    product = parsed["product"]
    problems = []

    def expect(label, actual, wanted):
        if actual != wanted:
            problems.append(f"{label} {actual!r} != {wanted!r}")

    expect("parameter", (parsed["discipline"], product["parameter_category"],
                         product["parameter_number"]), triple)
    expect("product template", product["template"], template)
    expect("first surface", product["first_surface_type"], surface)
    if surface != SURFACE_GROUND:
        expect("first surface value", product["first_surface_scaled_value"], surface_value)
    expect("reference time", parsed["identification"]["reference_time"], iso(cycle))
    if variable in STATIC_FIELDS:
        return problems
    expect("unit of time range", product["unit_of_time_range"], TIME_UNIT_MINUTE)
    if template == PRODUCT_TEMPLATE_INSTANTANEOUS:
        expect("forecast time", product["forecast_time"], step * MINUTES_PER_HOUR)
        return problems
    expect("forecast time (averaging start)", product["forecast_time"], 0)
    expect("statistical processing", product["statistical_processing"], STATISTICAL_AVERAGE)
    expect("time range count", product["time_range_count"], 1)
    expect("unit of statistical range", product["unit_of_statistical_range"], TIME_UNIT_MINUTE)
    expect("statistical range length", product["statistical_range_length"], step * MINUTES_PER_HOUR)
    expect("end of interval", product["end_of_interval"],
           iso(cycle + datetime.timedelta(hours=step)))
    return problems


def eccodes_cross_check(keys, parsed):
    """Compare ecCodes keys with the independent octet reader."""
    representation = parsed["representation"]
    pairs = (
        ("discipline", parsed["discipline"]),
        ("parameterCategory", parsed["product"]["parameter_category"]),
        ("parameterNumber", parsed["product"]["parameter_number"]),
        ("binaryScaleFactor", representation["binary_scale_factor"]),
        ("decimalScaleFactor", representation["decimal_scale_factor"]),
        ("bitsPerValue", representation["bits_per_value"]),
        ("Ni", parsed["grid"]["ni"]),
        ("Nj", parsed["grid"]["nj"]),
    )
    pairs += (("referenceValue", representation["reference_value"]),)
    return [f"{key}: ecCodes {keys[key]!r} vs octets {octets!r}"
            for key, octets in pairs if keys[key] != octets]


ECCODES_KEYS = ("discipline", "parameterCategory", "parameterNumber", "binaryScaleFactor",
                "decimalScaleFactor", "bitsPerValue", "Ni", "Nj", "referenceValue")


def decode(row):
    """Decode one single-message file; returns (parsed metadata, ecCodes values, problems)."""
    import eccodes  # noqa: PLC0415  (decoder dependency of ``inspect`` only)
    import numpy  # noqa: PLC0415
    messages = sections.split_messages(row["data"])
    if len(messages) != 1:
        raise AuditError(f"{row['path']}: expected one message, found {len(messages)}")
    parsed = sections.parse(messages[0])
    handle = eccodes.codes_new_from_message(messages[0])
    try:
        keys = {key: eccodes.codes_get(handle, key) for key in ECCODES_KEYS}
        values = numpy.asarray(eccodes.codes_get_values(handle), dtype="<f8")
    finally:
        eccodes.codes_release(handle)
    return parsed, values, eccodes_cross_check(keys, parsed)


# --------------------------------------------------------------------------- physics helpers


def interval_mean(start_hour, start_average, end_hour, end_average):
    """Mean over [start, end] from two run-start averages (DWD manual 7.1.1)."""
    return (end_hour * end_average - start_hour * start_average) / (end_hour - start_hour)


def quantization_bound(start_hour, start_error, end_hour, end_error):
    """Largest packing error of ``interval_mean`` (DWD manual 7.1.1 formula).

    ``interval_mean`` multiplies the packing error of each run-start average
    (``packing_error_bound``) by t/(t2 - t1), so the bound grows linearly with
    lead time for a fixed interval length.
    """
    return (end_hour * end_error + start_hour * start_error) / (end_hour - start_hour)


def beyond(values, bound):
    """Mask of values outside +-bound after float-evaluation slack."""
    import numpy  # noqa: PLC0415
    limit = bound * (1.0 + FLOAT_EVALUATION_SLACK_RELATIVE) + FLOAT_EVALUATION_SLACK_ABSOLUTE
    return numpy.abs(values) > limit


def below(values, bound):
    """Mask of values below -bound after float-evaluation slack."""
    limit = bound * (1.0 + FLOAT_EVALUATION_SLACK_RELATIVE) + FLOAT_EVALUATION_SLACK_ABSOLUTE
    return values < -limit


def solar_elevation_deg(lat_deg, lon_deg, moment):
    """Geometric solar elevation for a UTC instant.

    Spencer (1971) Fourier series for declination and equation of time, as
    in the NOAA "General Solar Position Calculations" note; accuracy about
    0.2 deg, adequate for the night mask margin.
    """
    import numpy  # noqa: PLC0415
    start = moment.replace(month=1, day=1, hour=0, minute=0, second=0)
    day_fraction = (moment - start).total_seconds() / 86400.0
    gamma = 2.0 * math.pi / 365.0 * (day_fraction - 0.5)
    equation_of_time_min = 229.18 * (
        0.000075 + 0.001868 * math.cos(gamma) - 0.032077 * math.sin(gamma)
        - 0.014615 * math.cos(2 * gamma) - 0.040849 * math.sin(2 * gamma))
    declination = (0.006918 - 0.399912 * math.cos(gamma) + 0.070257 * math.sin(gamma)
                   - 0.006758 * math.cos(2 * gamma) + 0.000907 * math.sin(2 * gamma)
                   - 0.002697 * math.cos(3 * gamma) + 0.00148 * math.sin(3 * gamma))
    utc_minutes = moment.hour * 60 + moment.minute + moment.second / 60.0
    true_solar_minutes = utc_minutes + equation_of_time_min + 4.0 * lon_deg
    hour_angle = numpy.radians(true_solar_minutes / 4.0 - 180.0)
    latitude = numpy.radians(lat_deg)
    sine = (numpy.sin(latitude) * math.sin(declination)
            + numpy.cos(latitude) * math.cos(declination) * numpy.cos(hour_angle))
    return numpy.degrees(numpy.arcsin(numpy.clip(sine, -1.0, 1.0)))


def night_mask(lat2d, lon2d, cycle, start_hour, end_hour):
    """Cells whose sun stays below the night margin during the whole interval."""
    import numpy  # noqa: PLC0415
    highest = numpy.full(lat2d.shape, -90.0)
    minutes = start_hour * MINUTES_PER_HOUR
    while minutes <= end_hour * MINUTES_PER_HOUR:
        moment = cycle + datetime.timedelta(minutes=minutes)
        numpy.maximum(highest, solar_elevation_deg(lat2d, lon2d, moment), out=highest)
        minutes += SOLAR_SAMPLE_MINUTES
    return highest < NIGHT_MAX_SOLAR_ELEVATION_DEG


def correlation(left, right):
    import numpy  # noqa: PLC0415
    return float(numpy.corrcoef(left.ravel(), right.ravel())[0, 1])


# --------------------------------------------------------------------------- evidence


class Store:
    """Decoded fields keyed by (variable, step), reshaped to (nj, ni)."""

    def __init__(self, rows, cycle):
        self.cycle = cycle
        self.fields, self.meta, self.messages, self.problems = {}, {}, [], []
        grids = set()
        for key in sorted(rows, key=lambda item: (item[0], -1 if item[1] is None else item[1])):
            row = rows[key]
            parsed, values, problems = decode(row)
            step = row["step"]
            problems += identity_problems(parsed, row["variable"], cycle, step)
            self.problems += [f"{row['path']}: {problem}" for problem in problems]
            grid = parsed["grid"]
            grids.add(tuple(sorted(grid.items())))
            self.fields[key] = values.reshape(grid["nj"], grid["ni"])
            self.meta[key] = {"quantum": quantum(parsed["representation"]),
                              "packing_error": packing_error_bound(parsed["representation"]),
                              "sha256": row["grib2_sha256"]}
            self.messages.append(message_record(row, parsed, values))
        if len(grids) != 1:
            raise AuditError(f"sample holds {len(grids)} distinct grids")
        self.grid = dict(grids.pop())

    def steps(self, variable):
        return sorted(step for name, step in self.fields if name == variable)

    def field(self, variable, step=None):
        """Decoded values of one required message; absence fails closed."""
        if (variable, step) not in self.fields:
            raise AuditError(f"required message {variable} step {step} is missing from the sample")
        return self.fields[(variable, step)]

    def check_flux_coverage(self):
        """All four averages must share one step list that starts at the run start."""
        reference = self.steps("ASHFL_S")
        if len(reference) < 2 or reference[0] != 0:
            raise AuditError(f"flux steps {reference} do not start at step 0 with a following step")
        for variable in AVERAGED_FLUXES:
            if self.steps(variable) != reference:
                raise AuditError(f"{variable} steps differ from ASHFL_S steps")

    def coordinates(self):
        """(lat2d, lon2d) of the regular grid; scanning mode 64 = i east, j north."""
        import numpy  # noqa: PLC0415
        grid = self.grid
        if grid["scanning_mode"] != 64:
            raise AuditError(f"unsupported scanning mode {grid['scanning_mode']}")
        lat = grid["first_lat_deg"] + grid["dj_deg"] * numpy.arange(grid["nj"])
        first_lon = grid["first_lon_deg"] - 360.0 if grid["first_lon_deg"] > 180.0 else grid["first_lon_deg"]
        lon = first_lon + grid["di_deg"] * numpy.arange(grid["ni"])
        return numpy.meshgrid(lat, lon, indexing="ij")


def message_record(row, parsed, values):
    product, representation = parsed["product"], parsed["representation"]
    record = {
        "path": row["path"],
        "variable": row["variable"],
        "step_h": row["step"],
        "grib2_sha256": row["grib2_sha256"],
        "parameter": [parsed["discipline"], product["parameter_category"], product["parameter_number"]],
        "product_template": product["template"],
        "reference_value": representation["reference_value"],
        "binary_scale_factor": representation["binary_scale_factor"],
        "decimal_scale_factor": representation["decimal_scale_factor"],
        "bits_per_value": representation["bits_per_value"],
        "quantum": quantum(representation),
        "packing_error_bound": packing_error_bound(representation),
        "min": float(values.min()),
        "max": float(values.max()),
    }
    if product["template"] == PRODUCT_TEMPLATE_STATISTICAL:
        record.update({
            "statistical_processing": product["statistical_processing"],
            "averaging_start_minutes": product["forecast_time"],
            "averaging_length_minutes": product["statistical_range_length"],
            "end_of_interval": product["end_of_interval"],
        })
    return record


def consecutive_intervals(steps):
    """Consecutive published step pairs (t1, t2) in ascending order."""
    return list(zip(steps[:-1], steps[1:]))


def step_zero_evidence(store):
    result = {}
    for variable in AVERAGED_FLUXES:
        values = store.field(variable, 0)
        result[variable] = {"all_zero": bool((values == 0.0).all()),
                            "grib2_sha256": store.meta[(variable, 0)]["sha256"]}
    return result


def bound_evidence(store):
    """Exact quantization bound and observed range of every derived interval mean."""
    result = {}
    for variable in AVERAGED_FLUXES:
        rows = []
        for start, end in consecutive_intervals(store.steps(variable)):
            first, last = store.meta[(variable, start)], store.meta[(variable, end)]
            mean = interval_mean(start, store.field(variable, start), end, store.field(variable, end))
            rows.append({
                "start_h": start, "end_h": end, "length_h": end - start,
                "start_grib2_sha256": first["sha256"], "end_grib2_sha256": last["sha256"],
                "start_quantum": first["quantum"], "end_quantum": last["quantum"],
                "quantization_bound": quantization_bound(start, first["packing_error"], end, last["packing_error"]),
                "min": float(mean.min()), "max": float(mean.max()),
            })
        result[variable] = rows
    return result


def cell_list(mask, values, lat2d, lon2d, ni):
    """First ``MAX_LISTED_CELLS`` masked cells; ``mask_extent`` covers all of them."""
    import numpy  # noqa: PLC0415
    cells = []
    for j, i in zip(*numpy.nonzero(mask)):
        cells.append({"j": int(j), "i": int(i), "lat": float(lat2d[j, i]), "lon": float(lon2d[j, i]),
                      "columns_from_east_edge": int(ni - 1 - i), "value": float(values[j, i])})
    return cells[:MAX_LISTED_CELLS]


def mask_extent(mask, lat2d, ni):
    """Columns from the east edge and latitude range of every masked cell."""
    import numpy  # noqa: PLC0415
    rows, columns = numpy.nonzero(mask)
    if not rows.size:
        return {"columns_from_east_edge": [], "latitude_range": None}
    latitudes = lat2d[rows, columns]
    return {"columns_from_east_edge": sorted({int(ni - 1 - column) for column in columns}),
            "latitude_range": [float(latitudes.min()), float(latitudes.max())]}


def shortwave_consistency(store, bounds):
    """Net short-wave non-negativity and night zero of every derived mean."""
    lat2d, lon2d = store.coordinates()
    rows = []
    for item in bounds["ASOB_S"]:
        start, end, bound = item["start_h"], item["end_h"], item["quantization_bound"]
        mean = interval_mean(start, store.field("ASOB_S", start), end, store.field("ASOB_S", end))
        negative = below(mean, bound)
        night = night_mask(lat2d, lon2d, store.cycle, start, end)
        night_beyond = night & beyond(mean, bound)
        rows.append({
            "start_h": start, "end_h": end, "quantization_bound": bound,
            "min": float(mean.min()),
            "violation_extent": mask_extent(negative | night_beyond, lat2d, store.grid["ni"]),
            "below_minus_bound_cells": int(negative.sum()),
            "below_minus_bound": cell_list(negative, mean, lat2d, lon2d, store.grid["ni"]),
            "night_cells": int(night.sum()),
            "night_min": float(mean[night].min()) if night.any() else None,
            "night_max": float(mean[night].max()) if night.any() else None,
            "night_beyond_bound_cells": int(night_beyond.sum()),
            "night_beyond_bound": cell_list(night_beyond, mean, lat2d, lon2d, store.grid["ni"]),
        })
    return rows


def shortwave_budget(store):
    """ASOB_S against ASWDIR_S + ASWDIFD_S - ASWDIFU_S (net = down - up, positive down)."""
    import numpy  # noqa: PLC0415
    land = store.field("FR_LAND") > LAND_FRACTION_THRESHOLD
    hsurf = store.field("HSURF")
    rows = []
    for step in store.steps("ASWDIFU_S"):
        if step == 0:
            continue
        names = ("ASOB_S",) + tuple(AVERAGED_SHORTWAVE_COMPONENTS)
        net, direct, diffuse_down, diffuse_up = (store.field(name, step) for name in names)
        residual = net - (direct + diffuse_down - diffuse_up)
        bound = sum(store.meta[(name, step)]["packing_error"] for name in names)
        downward = direct + diffuse_down
        lit = land & (downward > LIT_DOWNWARD_SHORTWAVE_W_M2)
        open_cells = beyond(residual, bound)
        rows.append({
            "step_h": step,
            "packing_error_sum": bound,
            "min_residual": float(residual.min()),
            "max_residual": float(residual.max()),
            "beyond_bound_cells": int(open_cells.sum()),
            "beyond_bound_land_fraction": float(land[open_cells].mean()) if open_cells.any() else None,
            "beyond_bound_median_hsurf_m": float(numpy.median(hsurf[open_cells])) if open_cells.any() else None,
            "max_relative_residual": float((numpy.abs(residual) / numpy.maximum(downward, LIT_DOWNWARD_SHORTWAVE_W_M2)).max()),
            "net_above_downward_cells": int((net > downward + bound).sum()),
            "land_mean_net_over_downward": float((net[lit] / downward[lit]).mean()) if lit.any() else None,
        })
    return rows


def sensible_heat_sign(store):
    """Canonical upward flux -ASHFL_S against the ground-air temperature contrast."""
    import numpy  # noqa: PLC0415
    land = store.field("FR_LAND") > LAND_FRACTION_THRESHOLD
    rows = []
    for start, end in SENSIBLE_HEAT_SIGN_INTERVALS:
        mean = interval_mean(start, store.field("ASHFL_S", start), end, store.field("ASHFL_S", end))
        contrast = [store.field("T_G", step) - store.field("T_2M", step) for step in (start, end)]
        clear = ((numpy.sign(contrast[0]) == numpy.sign(contrast[1]))
                 & (numpy.minimum(abs(contrast[0]), abs(contrast[1])) > SIGN_CHECK_MIN_TEMPERATURE_DIFFERENCE_K)
                 & (abs(mean) > SIGN_CHECK_MIN_SENSIBLE_HEAT_FLUX_W_M2))
        agrees = numpy.sign(-mean[clear]) == numpy.sign(contrast[1][clear])
        rows.append({
            "start_h": start, "end_h": end,
            "land_mean_ashfl_s": float(mean[land].mean()),
            "checked_cells": int(clear.sum()),
            "fraction_minus_ashfl_sign_equals_tg_minus_t2m_sign": float(agrees.mean()),
        })
    return rows


def stress_sign(store):
    """Interval-mean momentum flux against the mean of the bracketing 10 m wind."""
    import numpy  # noqa: PLC0415
    rows = []
    for start, end in STRESS_SIGN_INTERVALS:
        for flux_name, wind_name in STRESS_WIND_PAIRS:
            flux = interval_mean(start, store.field(flux_name, start), end, store.field(flux_name, end))
            winds = [store.field(wind_name, step) for step in (start, end)]
            clear = ((numpy.sign(winds[0]) == numpy.sign(winds[1]))
                     & (numpy.minimum(abs(winds[0]), abs(winds[1])) > SIGN_CHECK_MIN_WIND_SPEED_M_S))
            agrees = numpy.sign(flux[clear]) == numpy.sign(winds[1][clear])
            rows.append({
                "start_h": start, "end_h": end, "flux": flux_name, "wind": wind_name,
                "correlation_with_mean_wind": correlation(flux, 0.5 * (winds[0] + winds[1])),
                "checked_cells": int(clear.sum()),
                "fraction_same_sign_as_wind": float(agrees.mean()),
            })
    return rows


def summary(consistency):
    extents = [row["violation_extent"] for row in consistency if row["violation_extent"]["latitude_range"]]
    excess = [row for row in consistency if row["below_minus_bound_cells"]]
    return {
        "asob_intervals": len(consistency),
        "asob_intervals_with_negative_beyond_bound": [[row["start_h"], row["end_h"]] for row in excess],
        "asob_negative_beyond_bound_cell_intervals": sum(row["below_minus_bound_cells"] for row in consistency),
        "asob_night_beyond_bound_cell_intervals": sum(row["night_beyond_bound_cells"] for row in consistency),
        "asob_worst_negative_over_bound": max(
            (-row["min"] / row["quantization_bound"] for row in excess), default=0.0),
        "asob_violation_columns_from_east_edge": sorted(
            {column for extent in extents for column in extent["columns_from_east_edge"]}),
        "asob_violation_latitude_range": [
            min((extent["latitude_range"][0] for extent in extents), default=None),
            max((extent["latitude_range"][1] for extent in extents), default=None)],
    }


def inspect(sample, manifest_path, label):
    import eccodes  # noqa: PLC0415
    import numpy  # noqa: PLC0415
    manifest, rows = load_sample(sample, manifest_path)
    cycles = {row["cycle"] for row in rows.values()}
    if len(cycles) != 1:
        raise AuditError(f"sample mixes cycles {sorted(cycles)}")
    store = Store(rows, cycle_time(cycles.pop()))
    if store.problems:
        raise AuditError("identity/cross-check problems:\n" + "\n".join(store.problems))
    store.check_flux_coverage()
    bounds = bound_evidence(store)
    consistency = shortwave_consistency(store, bounds)
    return {
        "schema": EVIDENCE_SCHEMA,
        "version": EVIDENCE_VERSION,
        "issue": 233,
        "decoder": {"label": label, "eccodes": eccodes.codes_get_api_version(), "numpy": numpy.__version__},
        "retrieval_manifest_sha256": canonical_sha256(manifest_path),
        "request_sha256": manifest["request_sha256"],
        "cycle": iso(store.cycle),
        "grid": store.grid,
        "file_count": len(rows),
        "identity_problem_count": len(store.problems),
        "messages": store.messages,
        "step_zero": step_zero_evidence(store),
        "interval_bounds": bounds,
        "shortwave_interval_consistency": consistency,
        "shortwave_budget": shortwave_budget(store),
        "sensible_heat_sign": sensible_heat_sign(store),
        "stress_sign": stress_sign(store),
        "summary": summary(consistency),
    }


# --------------------------------------------------------------------------- compare


def differences(left, right, path="$"):
    """Yield mismatches between two evidence trees (decoder label excluded)."""
    if path == "$.decoder":
        return
    if isinstance(left, dict) and isinstance(right, dict):
        if set(left) != set(right):
            yield f"{path}: keys differ"
            return
        for key in left:
            yield from differences(left[key], right[key], f"{path}.{key}")
    elif isinstance(left, list) and isinstance(right, list):
        if len(left) != len(right):
            yield f"{path}: lengths differ"
            return
        for index, (a, b) in enumerate(zip(left, right)):
            yield from differences(a, b, f"{path}[{index}]")
    elif isinstance(left, float) or isinstance(right, float):
        if not (isinstance(left, (int, float)) and isinstance(right, (int, float))
                and math.isclose(left, right, rel_tol=COMPARE_RELATIVE_TOLERANCE, abs_tol=0.0)):
            yield f"{path}: {left!r} vs {right!r}"
    elif left != right:
        yield f"{path}: {left!r} vs {right!r}"


def write_json(value, path):
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(value, handle, indent=1, sort_keys=True)
        handle.write("\n")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    inspect_parser = sub.add_parser("inspect")
    inspect_parser.add_argument("--sample", required=True, help="fetch output directory")
    inspect_parser.add_argument("--manifest", help="default: <sample>/retrieval-manifest.json")
    inspect_parser.add_argument("--label", required=True)
    inspect_parser.add_argument("--output", required=True)
    compare_parser = sub.add_parser("compare")
    compare_parser.add_argument("--primary", required=True)
    compare_parser.add_argument("--secondary", required=True)
    args = parser.parse_args(argv)
    if args.command == "inspect":
        manifest = args.manifest or os.path.join(args.sample, "retrieval-manifest.json")
        evidence = inspect(args.sample, manifest, args.label)
        write_json(evidence, args.output)
        print(f"{evidence['file_count']} files, {evidence['identity_problem_count']} identity problems, "
              f"{evidence['summary']['asob_negative_beyond_bound_cell_intervals']} ASOB_S cell-intervals "
              "below -bound")
        return 0
    with open(args.primary, encoding="utf-8") as handle:
        primary = json.load(handle)
    with open(args.secondary, encoding="utf-8") as handle:
        secondary = json.load(handle)
    mismatches = list(differences(primary, secondary))
    for item in mismatches[:20]:
        print(f"MISMATCH {item}")
    print(f"{len(mismatches)} mismatches")
    return 1 if mismatches else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (AuditError, sections.SectionError) as error:
        print(f"icon_eu_flux_audit: FAILED: {error}", file=sys.stderr)
        sys.exit(1)
