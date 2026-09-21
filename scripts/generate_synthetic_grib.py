#!/usr/bin/env python3
"""Generate synthetic GRIB1 files for FLEXPART Fortran comparison tests.

Creates ECMWF-style GRIB1 files with uniform wind fields on hybrid eta
levels so that FLEXPART can be run without real meteorological data.

Prerequisites:
    apt install python3-eccodes python3-numpy
    OR: pip install eccodes-python numpy

Usage:
    python3 generate_synthetic_grib.py \\
        --output-dir target/comparison/meteo \\
        --nx 32 --ny 32 --nz 12 \\
        --u-wind 0.5 --v-wind -0.3 --w-wind 0.0 \\
        --start-date 20240101 --hours 6
"""

import argparse
import json
import os
import sys
from pathlib import Path

try:
    import numpy as np
except ImportError:
    print("Missing numpy. Install: pip install numpy", file=sys.stderr)
    sys.exit(1)

try:
    import eccodes
except ImportError:
    print("Missing eccodes. Install: pip install eccodes-python", file=sys.stderr)
    sys.exit(1)

REPO = Path(__file__).resolve().parents[1]
DEFAULT_PROFILE = "reference/oracle-meteorology/synthetic-grib-v1.json"
PROFILE_ID = "flexpart-synthetic-grib-v1"
PROFILE_VERSION = 1

UPPER_TEMPERATURE_BASE_K = 220.0
UPPER_TEMPERATURE_LEVEL_INCREMENT_K = 6.5
UPPER_TEMPERATURE_FLOOR_K = 200.0
SPECIFIC_HUMIDITY_BASE = 0.001
SPECIFIC_HUMIDITY_SPAN = 0.009
SURFACE_PRESSURE_PA = 101325.0
TEMPERATURE_2M_K = 289.0
DEWPOINT_2M_K = 284.0
SOLAR_RADIATION_W_M2 = 220.0
EASTWARD_SURFACE_STRESS_N_M2 = 0.1
NORTHWARD_SURFACE_STRESS_N_M2 = 0.1

# Simplified 12-level hybrid eta coefficients (13 half-level boundaries).
# Pressure at half-level k: p(k) = A(k) + B(k) * ps
# These are a rough approximation of ECMWF L60-like levels spanning
# ~10 hPa (top) to surface.
HALF_LEVEL_A = np.array([
    0.0,        2000.0,     5000.0,    10000.0,
    15000.0,    20000.0,    22000.0,   18000.0,
    12000.0,     6000.0,     2000.0,      500.0,
    0.0,
], dtype=np.float64)

HALF_LEVEL_B = np.array([
    0.0,        0.0,        0.0,        0.0,
    0.0,        0.0,        0.05,       0.15,
    0.30,       0.50,       0.70,       0.85,
    1.0,
], dtype=np.float64)


def write_grib1_message(fout, param_id: int, level_type: int, level: int,
                         nx: int, ny: int, values: np.ndarray,
                         date: int, time: int, nz: int):
    """Write a single GRIB1 message with full ECMWF-compatible metadata."""
    msgid = eccodes.codes_grib_new_from_samples("GRIB1")

    eccodes.codes_set(msgid, "centre", 98)  # ECMWF
    eccodes.codes_set(msgid, "editionNumber", 1)
    eccodes.codes_set(msgid, "table2Version", 128)
    eccodes.codes_set(msgid, "dataDate", date)
    eccodes.codes_set(msgid, "dataTime", time // 100)
    eccodes.codes_set(msgid, "indicatorOfParameter", param_id)
    eccodes.codes_set(msgid, "indicatorOfTypeOfLevel", level_type)
    eccodes.codes_set(msgid, "level", level)

    eccodes.codes_set(msgid, "gridType", "regular_ll")
    eccodes.codes_set(msgid, "Ni", nx)
    eccodes.codes_set(msgid, "Nj", ny)
    eccodes.codes_set(msgid, "iDirectionIncrementInDegrees", 360.0 / nx)
    eccodes.codes_set(msgid, "jDirectionIncrementInDegrees", 180.0 / (ny - 1))
    eccodes.codes_set(msgid, "latitudeOfFirstGridPointInDegrees", 90.0)
    eccodes.codes_set(msgid, "longitudeOfFirstGridPointInDegrees", 0.0)
    eccodes.codes_set(msgid, "latitudeOfLastGridPointInDegrees", -90.0)
    eccodes.codes_set(msgid, "longitudeOfLastGridPointInDegrees",
                      360.0 - 360.0 / nx)
    eccodes.codes_set(msgid, "jScansPositively", 0)

    # Hybrid eta level vertical coordinate (pv array)
    n_half = nz + 1
    pv = np.concatenate([HALF_LEVEL_A[:n_half], HALF_LEVEL_B[:n_half]])
    eccodes.codes_set(msgid, "PVPresent", 1)
    eccodes.codes_set_double_array(msgid, "pv", pv.tolist())

    eccodes.codes_set(msgid, "packingType", "grid_simple")
    eccodes.codes_set(msgid, "bitsPerValue", 16)
    eccodes.codes_set_values(msgid, values.flatten().astype(np.float64))

    eccodes.codes_write(msgid, fout)
    eccodes.codes_release(msgid)


def load_profile(path_text: str) -> dict:
    path = Path(path_text)
    if not path.is_absolute():
        path = REPO / path
    profile = json.loads(path.read_text(encoding="utf-8"))
    if profile.get("id") != PROFILE_ID or profile.get("version") != PROFILE_VERSION:
        raise SystemExit(
            f"unsupported synthetic meteorology profile {profile.get('id')!r} "
            f"v{profile.get('version')!r}; expected {PROFILE_ID} v{PROFILE_VERSION}"
        )
    expected_a = np.asarray(profile.get("hybrid_half_level_a_pa"), dtype=np.float64)
    expected_b = np.asarray(profile.get("hybrid_half_level_b"), dtype=np.float64)
    if not np.array_equal(expected_a, HALF_LEVEL_A) or not np.array_equal(expected_b, HALF_LEVEL_B):
        raise SystemExit("synthetic meteorology hybrid-level constants drift from versioned profile")
    upper = profile.get("upper_air_profile") or {}
    fixed = profile.get("fixed_surface_fields") or {}
    checks = {
        "temperature_base_k": UPPER_TEMPERATURE_BASE_K,
        "temperature_level_increment_k": UPPER_TEMPERATURE_LEVEL_INCREMENT_K,
        "temperature_floor_k": UPPER_TEMPERATURE_FLOOR_K,
        "specific_humidity_base": SPECIFIC_HUMIDITY_BASE,
        "specific_humidity_span": SPECIFIC_HUMIDITY_SPAN,
    }
    for key, expected in checks.items():
        if float(upper.get(key, float("nan"))) != expected:
            raise SystemExit(f"synthetic meteorology upper-air constant {key} drifted from profile")
    fixed_checks = {
        "surface_pressure_pa": SURFACE_PRESSURE_PA,
        "temperature_2m_k": TEMPERATURE_2M_K,
        "dewpoint_2m_k": DEWPOINT_2M_K,
        "solar_radiation_w_m2": SOLAR_RADIATION_W_M2,
        "eastward_surface_stress_n_m2": EASTWARD_SURFACE_STRESS_N_M2,
        "northward_surface_stress_n_m2": NORTHWARD_SURFACE_STRESS_N_M2,
    }
    for key, expected in fixed_checks.items():
        if float(fixed.get(key, float("nan"))) != expected:
            raise SystemExit(f"synthetic meteorology surface constant {key} drifted from profile")
    return profile


LEVEL_SURFACE = 1
LEVEL_HYBRID = 109

PARAM_U = 131
PARAM_V = 132
PARAM_W = 135
PARAM_T = 130
PARAM_Q = 133
PARAM_SP = 134
PARAM_T2M = 167
PARAM_TD2M = 168
PARAM_U10M = 165
PARAM_V10M = 166
PARAM_SSHF = 146
PARAM_SSR = 176
PARAM_EWSS = 180
PARAM_NSSS = 181
PARAM_LSP = 142
PARAM_CP = 143
PARAM_BLH = 159     # boundary layer height
PARAM_LNSP = 152    # log of surface pressure


def generate_one_timestep(output_path: str, nx: int, ny: int, nz: int,
                           u_wind: float, v_wind: float, w_wind: float,
                           date: int, time: int,
                           u_shear_per_m: float = 0.0,
                           sshf: float = 40.0,
                           blh: float = 800.0,
                           lsp: float = 0.0,
                           cp: float = 0.0):
    npoints = nx * ny
    uniform = lambda val: np.full(npoints, val, dtype=np.float64)

    with open(output_path, "wb") as fout:
        for k in range(1, nz + 1):
            # Approximate level height for shear mapping. Hybrid-eta levels
            # have no fixed height; use a linear proxy so the corpus shear
            # case is reproducible. The candidate uses true height u(z), so
            # shear input equivalence remains an open comparison item.
            level_height_proxy_m = float(k) / float(nz) * 8000.0
            u_level = u_wind + u_shear_per_m * level_height_proxy_m
            write_grib1_message(fout, PARAM_U, LEVEL_HYBRID, k, nx, ny,
                                uniform(u_level), date, time, nz)
            write_grib1_message(fout, PARAM_V, LEVEL_HYBRID, k, nx, ny,
                                uniform(v_wind), date, time, nz)
            write_grib1_message(fout, PARAM_W, LEVEL_HYBRID, k, nx, ny,
                                uniform(w_wind), date, time, nz)
            temp_k = max(UPPER_TEMPERATURE_BASE_K + UPPER_TEMPERATURE_LEVEL_INCREMENT_K * (nz - k), UPPER_TEMPERATURE_FLOOR_K)
            write_grib1_message(fout, PARAM_T, LEVEL_HYBRID, k, nx, ny,
                                uniform(temp_k), date, time, nz)
            qv = SPECIFIC_HUMIDITY_BASE + SPECIFIC_HUMIDITY_SPAN * (k / nz)
            write_grib1_message(fout, PARAM_Q, LEVEL_HYBRID, k, nx, ny,
                                uniform(qv), date, time, nz)

        # Surface fields
        write_grib1_message(fout, PARAM_SP, LEVEL_SURFACE, 0, nx, ny,
                            uniform(SURFACE_PRESSURE_PA), date, time, nz)
        write_grib1_message(fout, PARAM_LNSP, LEVEL_SURFACE, 1, nx, ny,
                            uniform(np.log(SURFACE_PRESSURE_PA)), date, time, nz)
        write_grib1_message(fout, PARAM_T2M, LEVEL_SURFACE, 0, nx, ny,
                            uniform(TEMPERATURE_2M_K), date, time, nz)
        write_grib1_message(fout, PARAM_TD2M, LEVEL_SURFACE, 0, nx, ny,
                            uniform(DEWPOINT_2M_K), date, time, nz)
        write_grib1_message(fout, PARAM_U10M, LEVEL_SURFACE, 0, nx, ny,
                            uniform(u_wind), date, time, nz)
        write_grib1_message(fout, PARAM_V10M, LEVEL_SURFACE, 0, nx, ny,
                            uniform(v_wind), date, time, nz)
        write_grib1_message(fout, PARAM_SSHF, LEVEL_SURFACE, 0, nx, ny,
                            uniform(sshf), date, time, nz)
        write_grib1_message(fout, PARAM_SSR, LEVEL_SURFACE, 0, nx, ny,
                            uniform(SOLAR_RADIATION_W_M2), date, time, nz)
        write_grib1_message(fout, PARAM_EWSS, LEVEL_SURFACE, 0, nx, ny,
                            uniform(EASTWARD_SURFACE_STRESS_N_M2), date, time, nz)
        write_grib1_message(fout, PARAM_NSSS, LEVEL_SURFACE, 0, nx, ny,
                            uniform(NORTHWARD_SURFACE_STRESS_N_M2), date, time, nz)
        write_grib1_message(fout, PARAM_LSP, LEVEL_SURFACE, 0, nx, ny,
                            uniform(lsp), date, time, nz)
        write_grib1_message(fout, PARAM_CP, LEVEL_SURFACE, 0, nx, ny,
                            uniform(cp), date, time, nz)
        write_grib1_message(fout, PARAM_BLH, LEVEL_SURFACE, 0, nx, ny,
                            uniform(blh), date, time, nz)


def main():
    pre = argparse.ArgumentParser(add_help=False)
    pre.add_argument("--profile", default=DEFAULT_PROFILE)
    known, _ = pre.parse_known_args()
    profile = load_profile(known.profile)
    grid = profile.get("grid") or {}
    temporal = profile.get("temporal") or {}

    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--profile", default=known.profile)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--nx", type=int, default=int(grid["nx"]))
    parser.add_argument("--ny", type=int, default=int(grid["ny"]))
    parser.add_argument("--nz", type=int, default=int(grid["nz"]))
    parser.add_argument("--u-wind", type=float, default=0.5)
    parser.add_argument("--v-wind", type=float, default=-0.3)
    parser.add_argument("--w-wind", type=float, default=0.0)
    parser.add_argument("--u-shear-per-m", type=float, default=0.0,
                        help="Linear u shear per meter applied to the level-height proxy (corpus shear cases).")
    parser.add_argument("--sshf", type=float, default=40.0,
                        help="Uniform sensible heat flux [W/m2] for corpus PBL cases.")
    parser.add_argument("--blh", type=float, default=800.0,
                        help="Uniform boundary-layer height [m] for corpus PBL cases.")
    parser.add_argument("--lsp", type=float, default=0.0,
                        help="Uniform large-scale precipitation for corpus wet-deposition cases.")
    parser.add_argument("--cp", type=float, default=0.0,
                        help="Uniform convective precipitation for corpus wet-deposition cases.")
    parser.add_argument("--start-date", default="20240101")
    parser.add_argument("--hours", type=int, default=6)
    args = parser.parse_args()

    if (args.nx, args.ny, args.nz) != (int(grid["nx"]), int(grid["ny"]), int(grid["nz"])):
        raise SystemExit(
            "synthetic meteorology grid arguments must match versioned profile "
            f"{grid['nx']}x{grid['ny']}x{grid['nz']}"
        )
    cadence_s = temporal.get("cadence_s")
    if not isinstance(cadence_s, int) or cadence_s <= 0 or cadence_s % 3600 != 0:
        raise SystemExit("profile temporal.cadence_s must be a positive whole hour")
    cadence_hours = cadence_s // 3600
    if args.hours < cadence_hours or args.hours % cadence_hours != 0:
        raise SystemExit(
            f"--hours must be a positive multiple of profile cadence ({cadence_hours} h)"
        )

    os.makedirs(args.output_dir, exist_ok=True)

    date = int(args.start_date)
    for hour in range(0, args.hours + 1, cadence_hours):
        time_hhmmss = hour * 10000
        filename = f"EN{args.start_date}{hour:02d}"
        output_path = os.path.join(args.output_dir, filename)

        print(f"  {filename}  (date={date}, time={time_hhmmss:06d})")
        generate_one_timestep(
            output_path, args.nx, args.ny, args.nz,
            args.u_wind, args.v_wind, args.w_wind,
            date, time_hhmmss,
            u_shear_per_m=args.u_shear_per_m,
            sshf=args.sshf,
            blh=args.blh,
            lsp=args.lsp,
            cp=args.cp,
        )

    available_path = os.path.join(args.output_dir, "AVAILABLE")
    with open(available_path, "w") as f:
        f.write("DATE      TIME     FILENAME     SPECIFICATIONS\n")
        f.write("YYYYMMDD  HHMMSS\n")
        f.write("________ ________ __________ __________\n")
        for hour in range(0, args.hours + 1, cadence_hours):
            fname = f"EN{args.start_date}{hour:02d}"
            f.write(f"{args.start_date} {hour * 10000:06d}      "
                    f"{fname}      ON DISC\n")

    count = args.hours // cadence_hours + 1
    print(f"\nGenerated {count} GRIB1 files + AVAILABLE in {args.output_dir}/")


if __name__ == "__main__":
    main()
