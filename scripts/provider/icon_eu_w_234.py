#!/usr/bin/env python3
"""Reproduce #234 research evidence from the pinned ICON-EU exports."""
import argparse
from fractions import Fraction
import json
import math
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from icon_eu_audit import eccodes_messages, sha256_bytes, value_summary  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
METEO = ROOT / "fixtures" / "meteorology"
AUDIT = METEO / "icon-eu-audit-v1"
VERTICAL = METEO / "icon-eu-vertical-231-v1"
DECISION = METEO / "icon-eu-w-234-v1"
DECISION_PATH = (DECISION / "decision.json").relative_to(ROOT)
EVIDENCE_PATH = (DECISION / "evidence.json").relative_to(ROOT)
DECISION_SHA256 = "b344483b8bbbe71bc3afde51dfa7e8d6f3c9861a94ae48227f79348909c19313"
LEVELS = (1, 37, 73, 74, 75)
RADIUS_M = 6_371_229


def load(path):
    return json.loads(path.read_bytes())


def digest(path):
    return sha256_bytes(path.read_bytes())


def locked_decision(root):
    path = root / DECISION_PATH
    data = path.read_bytes()
    if sha256_bytes(data) != DECISION_SHA256:
        raise ValueError(f"{DECISION_PATH}: decision digest mismatch")
    return json.loads(data)


def verify_artifacts(root, artifacts, *, replay):
    for artifact, expected in artifacts.items():
        path = Path(artifact)
        if replay and path == EVIDENCE_PATH:
            continue
        if digest(root / path) != expected:
            raise ValueError(f"{artifact}: artifact digest mismatch")


def half_wind(upper, lower, upper_height, lower_height, interface_height):
    if not lower_height < interface_height < upper_height:
        raise ValueError("interface is not bracketed by full levels")
    weight = (interface_height - lower_height) / (upper_height - lower_height)
    return lower + weight * (upper - lower)


def relative(w, u, v, east_slope, north_slope):
    return w - u * east_slope - v * north_slope


def centered_slope(plus_height, minus_height, neighbor_distance):
    return (plus_height - minus_height) / neighbor_distance


def plane_slopes(plane):
    height, east, north = plane
    return (centered_slope(height + 2 * east, height - 2 * east, 4),
            centered_slope(height + 3 * north, height - 3 * north, 6))


def particle_reference(w, u, v, interface, terrain):
    """Exact trajectory on affine surfaces, evaluated via positions at ±1 s."""
    def coordinates(t):
        x, y, z = u * t, v * t, interface[0] + w * t
        ground = terrain[0] + terrain[1] * x + terrain[2] * y
        plane = interface[0] + interface[1] * x + interface[2] * y
        return z - plane, z - ground

    return tuple((after - before) / 2
                 for before, after in zip(coordinates(Fraction(-1)),
                                          coordinates(Fraction(1))))


def analytical():
    cases = {
        "flat": (3, (20, 0, 0), (10, 0, 0), (3, 3)),
        "tilted_surface": (3, (10, 1, -2), (10, 1, -2), (0, 0)),
        "tilted_interior": (2, (20, 1, -2), (10, 2, 1), (-1, -9)),
        "flat_lid_over_slopes": (0, (20, 0, 0), (10, 2, 1), (0, -11)),
        "northward_downhill": (0, (20, 0, -2), (10, 0, -2), (2, 2)),
    }
    # Heights 12 and 4 bracket an interface at 6. Exact winds there are (5,1).
    u = half_wind(11, 3, 12, 4, 6)
    v = half_wind(4, 0, 12, 4, 6)
    if (u, v) != (5, 1):
        raise ValueError("nonuniform full-to-half interpolation failed")
    result = {}
    for name, (w, plane, ground, expected) in cases.items():
        got = relative(w, u, v, *plane_slopes(plane)), relative(w, u, v, *plane_slopes(ground))
        ref = particle_reference(w, Fraction(5), Fraction(1), plane, ground)
        if got != expected or ref != expected:
            raise ValueError(f"{name}: {got}, {ref} differ from {expected}")
        result[name] = {"interface_relative_m_s": expected[0], "agl_rate_m_s": expected[1]}
    return result


def audit_rows():
    evidence = load(AUDIT / "decoded-evidence.json")
    rows = {}
    for row in evidence["rows"]:
        name = Path(row["path"]).stem
        if "_time-invariant_2026103100_" in name:
            rows[name.split("_2026103100_")[1]] = row
        elif "_model-level_2026103100_000_" in name:
            rows["000_" + name.split("_000_")[1]] = row
    return evidence, rows


def retained_wind():
    evidence, rows = audit_rows()
    manifest = load(AUDIT / "retrieval-manifest.json")
    raw_hashes = {item["path"].removesuffix(".bz2"): item["grib2_sha256"]
                  for item in manifest["files"]}
    selected = {}
    for key in ("000_1_W", "000_37_W", "000_74_W", "000_75_W",
                "000_73_U", "000_74_U", "000_73_V", "000_74_V"):
        row = rows[key]
        if (row["grib2_sha256"] != raw_hashes[row["path"]]
                or not row["values_identical_across_decoders"]
                or row["independent_max_abs_difference"] != 0):
            raise ValueError(f"{key}: unbound or inconsistent #230 evidence")
        selected[key] = {"grib2_sha256": row["grib2_sha256"], **row["values"]}
    return {"cycle": "2026103100", "forecast_hour": 0, "fields": selected,
            "surface_diagnostic": evidence["science"]["surface_w"]}


def geometry(archive):
    import numpy as np
    from eccodes import __version__ as eccodes_version

    _, audit = audit_rows()
    evidence = load(VERTICAL / "decoded-evidence.json")
    manifest = load(VERTICAL / "retrieval-manifest.json")
    raw_hashes = {item["path"].removesuffix(".bz2"): item["grib2_sha256"]
                  for item in manifest["files"]}
    rows = {row["level"]: row for row in evidence["messages"] if row["variable"] == "HHL"}
    # #230 identifies exactly one rectangular grid and northward/eastward scan.
    nx, ny, lat0, _, _, _, dx, dy, scan_i, scan_j, consecutive_j, sphere = (
        load(AUDIT / "decoded-evidence.json")["science"]["identity"]["distinct_grids"][0])
    if (scan_i, scan_j, consecutive_j, sphere) != (0, 1, 0, 6):
        raise ValueError("unexpected grid orientation or Earth shape")
    latitude = np.radians(lat0 + dy * np.arange(1, ny - 1))[:, None]
    east_distance = 2 * RADIUS_M * np.cos(latitude) * math.radians(dx)
    # Keep both spacings f64 even when the diagnostic numerator is f32.
    north_distance = np.float64(2 * RADIUS_M * math.radians(dy))

    def gradients(height):
        return (centered_slope(height[1:-1, 2:], height[1:-1, :-2], east_distance),
                centered_slope(height[2:, 1:-1], height[:-2, 1:-1], north_distance))

    def max_abs(values):
        return float(np.abs(values).max())

    heights = {}
    provenance = {}
    for level in LEVELS:
        row = rows[level]
        path = row["path"]
        raw = archive / path
        if digest(raw) != raw_hashes[path] or raw_hashes[path] != row["grib2_sha256"]:
            raise ValueError(f"{path}: GRIB2 digest mismatch")
        (_, values), = eccodes_messages(raw)
        summary = value_summary(values)
        expected = audit[f"{level}_HHL"]["values"]["values_sha256_f8le"]
        if (summary["count"] != nx * ny or summary["non_finite"]
                or summary["values_sha256_f8le"] != expected
                or row["values_sha256_f8le"] != expected):
            raise ValueError(f"{path}: decoded geometry differs from a pinned cycle")
        heights[level] = values.reshape(ny, nx)
        provenance[str(level)] = {"archive_grib2_sha256": raw_hashes[path],
                                  "audit_230_grib2_sha256": audit[f"{level}_HHL"]["grib2_sha256"],
                                  "values_sha256_f8le": expected}

    slopes = {}
    for level in (1, 37, 74, 75):
        plane = heights[level]
        east, north = gradients(plane)
        agl_east, agl_north = gradients(plane - heights[75])
        f32_east, f32_north = gradients(plane.astype("float32"))
        slopes[str(level)] = {
            "max_abs_asl_slope": [max_abs(east), max_abs(north)],
            "max_abs_agl_slope": [max_abs(agl_east), max_abs(agl_north)],
            "max_abs_f32_height_slope_difference": [max_abs(f32_east - east),
                                                     max_abs(f32_north - north)],
        }

    w74_weight = (heights[73] - heights[74]) / (heights[73] - heights[75])
    if not np.all((w74_weight > 0) & (w74_weight < 1)):
        raise ValueError("level 74 has unbracketed full-to-half wind")
    return {"archive_cycle": "2026110300", "eccodes": eccodes_version, "numpy": np.__version__,
            "centered_interior_count": (nx - 2) * (ny - 2), "hhl": provenance,
            "slopes": slopes, "half_level_74_lower_wind_weight_min_max":
            [float(w74_weight.min()), float(w74_weight.max())]}


def wind_bounds(wind, measured_geometry):
    fields = wind["fields"]
    slopes = measured_geometry["slopes"]["74"]
    max_u, max_v = (
        max(abs(fields[f"000_{k}_{c}"][end]) for k in (73, 74) for end in ("min", "max"))
        for c in ("U", "V")
    )
    east, north = slopes["max_abs_asl_slope"]
    correction = max_u * east + max_v * north
    east_agl, north_agl = slopes["max_abs_agl_slope"]
    east_f32, north_f32 = slopes["max_abs_f32_height_slope_difference"]
    level = fields["000_74_W"]
    return {"level_74_abs_slope_correction_upper_bound_m_s": correction,
            "level_74_relative_w_enclosure_m_s": [level["min"] - correction,
                                                   level["max"] + correction],
            "level_74_abs_agl_slope_term_upper_bound_m_s": max_u * east_agl + max_v * north_agl,
            "level_74_f32_height_slope_effect_upper_bound_m_s": max_u * east_f32 + max_v * north_f32}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--geometry-archive", type=Path, required=True)
    args = parser.parse_args()
    decision = locked_decision(ROOT)
    verify_artifacts(ROOT, decision["artifact_sha256"], replay=True)
    measured_geometry = geometry(args.geometry_archive)
    wind = retained_wind()
    result = {"schema": "flexpart-gpu.icon-eu-w-research-evidence", "version": 1,
              "analytic_exact": analytical(), "wind_230_retained": wind,
              "geometry_redecoded": measured_geometry,
              "interior_bounds_from_extrema": wind_bounds(wind, measured_geometry)}
    sys.stdout.reconfigure(newline="\n")
    print(json.dumps(result, indent=2, sort_keys=True, allow_nan=False))


if __name__ == "__main__":
    main()
