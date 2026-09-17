#!/usr/bin/env python3
"""Convert ERA5 numpy arrays to FLEXPART-compatible GRIB1 files.

Reads the .npy arrays downloaded by download_era5_gcs.py and writes
FLEXPART-compatible GRIB1 files with 3-hourly timesteps.

Also exports a JSON file with wind/surface field metadata for the
flexpart-gpu binary to consume.

Usage:
    python3 scripts/etex/prepare_flexpart_input_from_npy.py \\
        --era5-dir target/etex/era5_raw \\
        --output-dir target/etex/meteo
"""

import argparse
import json
import math
import os
import sys
from datetime import datetime, timedelta

try:
    import numpy as np
except ImportError:
    print("Missing numpy", file=sys.stderr); sys.exit(1)

try:
    import eccodes
except ImportError:
    print("Missing eccodes. Install: apt install python3-eccodes", file=sys.stderr)
    sys.exit(1)


PRESSURE_LEVELS = [1000, 925, 850, 700, 600, 500, 400, 300, 250, 200, 150, 100, 70, 50]

# Reversed: level 1=top (50hPa), level 14=surface (1000hPa) for FLEXPART
PRESSURE_LEVELS_TOP_FIRST = list(reversed(PRESSURE_LEVELS))

def compute_hybrid_pv(pressure_levels_hpa_top_first):
    """Construct terrain-following half levels for a pressure-level smoke run.

    This is an approximation, not native ERA5 model-level forcing. Interpolate
    each pressure-level field to the matching cell-specific full levels below.
    """
    levels = np.asarray(pressure_levels_hpa_top_first, dtype=np.float64)
    half = np.empty(len(levels) + 1, dtype=np.float64)
    half[0] = max(0.0, levels[0] - (levels[1] - levels[0]) / 2) / 1000.0
    half[1:-1] = (levels[:-1] + levels[1:]) / 2000.0
    half[-1] = 1.0
    return [0.0] * len(half) + half.tolist()


def interpolate_pressure_field(field, surface_pressure_pa, pv):
    """Map (top-to-bottom, y, x) ERA5 fields to the declared sigma levels."""
    n = field.shape[0]
    half = np.asarray(pv[n + 1:], dtype=np.float64)
    sigma = 0.5 * (half[:-1] + half[1:])
    target_hpa = sigma[:, None, None] * surface_pressure_pa[None, :, :] / 100.0
    levels = np.asarray(PRESSURE_LEVELS_TOP_FIRST, dtype=np.float64)
    target_hpa = np.clip(target_hpa, levels[0], levels[-1])
    upper = np.clip(np.searchsorted(levels, target_hpa, side="left"), 1, n - 1)
    lower = upper - 1
    low = np.take_along_axis(field, lower, axis=0)
    high = np.take_along_axis(field, upper, axis=0)
    fraction = (target_hpa - levels[lower]) / (levels[upper] - levels[lower])
    return low + fraction * (high - low)


PARAM_3D = {
    "u_component_of_wind":  131,
    "v_component_of_wind":  132,
    "vertical_velocity":    135,
    "temperature":          130,
    "specific_humidity":    133,
}

PARAM_SFC = {
    "surface_pressure":                         134,
    "10m_u_component_of_wind":                  165,
    "10m_v_component_of_wind":                  166,
    "2m_temperature":                           167,
    "2m_dewpoint_temperature":                  168,
    "boundary_layer_height":                    159,
    "surface_sensible_heat_flux":               146,
    "surface_net_solar_radiation":              176,
    "mean_eastward_turbulent_surface_stress":   180,
    "mean_northward_turbulent_surface_stress":  181,
    "convective_precipitation":                 143,
    "large_scale_precipitation":                142,
    "forecast_surface_roughness":               173,
    "geopotential_at_surface":                    129,
    "land_sea_mask":                             172,
}

TIME_STEP_HOURS = 3


def write_grib1(fout, param_id: int, level_type: int, level: int,
                nx: int, ny: int, values, lat_first, lon_first,
                lat_last, lon_last, dx, dy, date: int, time_hhmm: int,
                nz: int = 0, pv_array=None):
    """Write one GRIB1 message."""
    msgid = eccodes.codes_grib_new_from_samples("GRIB1")
    eccodes.codes_set(msgid, "centre", 98)
    eccodes.codes_set(msgid, "editionNumber", 1)
    eccodes.codes_set(msgid, "table2Version", 128)
    eccodes.codes_set(msgid, "dataDate", date)
    eccodes.codes_set(msgid, "dataTime", time_hhmm)
    eccodes.codes_set(msgid, "indicatorOfParameter", param_id)
    eccodes.codes_set(msgid, "indicatorOfTypeOfLevel", level_type)
    eccodes.codes_set(msgid, "level", level)
    eccodes.codes_set(msgid, "gridType", "regular_ll")
    eccodes.codes_set(msgid, "Ni", nx)
    eccodes.codes_set(msgid, "Nj", ny)
    eccodes.codes_set(msgid, "iDirectionIncrementInDegrees", dx)
    eccodes.codes_set(msgid, "jDirectionIncrementInDegrees", dy)
    eccodes.codes_set(msgid, "latitudeOfFirstGridPointInDegrees", lat_first)
    eccodes.codes_set(msgid, "longitudeOfFirstGridPointInDegrees", lon_first)
    eccodes.codes_set(msgid, "latitudeOfLastGridPointInDegrees", lat_last)
    eccodes.codes_set(msgid, "longitudeOfLastGridPointInDegrees", lon_last)
    eccodes.codes_set(msgid, "jScansPositively", 0)

    if pv_array is not None:
        eccodes.codes_set(msgid, "PVPresent", 1)
        eccodes.codes_set_double_array(msgid, "pv", pv_array)

    eccodes.codes_set(msgid, "packingType", "grid_simple")
    eccodes.codes_set(msgid, "bitsPerValue", 16)

    vals = np.asarray(values, dtype=np.float64).flatten()
    eccodes.codes_set_values(msgid, vals)
    eccodes.codes_write(msgid, fout)
    eccodes.codes_release(msgid)


def approximate_heights_m(pressure_levels_hpa):
    """Pressure to approximate geometric height (meters) via barometric formula."""
    P0 = 1013.25
    T0 = 288.15
    L = 0.0065
    g = 9.80665
    R = 287.05
    return [round((T0 / L) * (1.0 - (p / P0) ** (R * L / g)), 1) for p in pressure_levels_hpa]


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--era5-dir", default="target/etex/era5_raw")
    parser.add_argument("--output-dir", default="target/etex/meteo")
    parser.add_argument("--gpu-json", default=None)
    parser.add_argument("--time-step", type=int, default=TIME_STEP_HOURS,
                        help="Output time step in hours (default: 3)")
    args = parser.parse_args()

    os.makedirs(args.output_dir, exist_ok=True)

    with open(os.path.join(args.era5_dir, "metadata.json")) as f:
        meta = json.load(f)

    lats = np.load(os.path.join(args.era5_dir, "latitudes.npy"))
    lons = np.load(os.path.join(args.era5_dir, "longitudes.npy"))
    times_str = np.load(os.path.join(args.era5_dir, "times.npy"))

    ny, nx = len(lats), len(lons)
    lat_first = float(lats[0])  # 65°N (scan from N to S)
    lat_last = float(lats[-1])  # 35°N
    lon_first = float(lons[0])  # -15°
    lon_last = float(lons[-1])  # 35°
    dx = abs(float(lons[1] - lons[0]))
    dy = abs(float(lats[1] - lats[0]))

    print(f"Grid: {nx}x{ny}, dx={dx}°, dy={dy}°")
    print(f"Lat: {lat_first} to {lat_last}")
    print(f"Lon: {lon_first} to {lon_last}")

    data_3d = {}
    for name in PARAM_3D:
        path = os.path.join(args.era5_dir, f"{name}.npy")
        if os.path.exists(path):
            data_3d[name] = np.load(path)
            print(f"  Loaded {name}: {data_3d[name].shape}")
        else:
            print(f"  [WARN] Missing {name}")

    data_sfc = {}
    for name in PARAM_SFC:
        path = os.path.join(args.era5_dir, f"{name}.npy")
        if os.path.exists(path):
            data_sfc[name] = np.load(path)
            print(f"  Loaded {name}: {data_sfc[name].shape}")
        else:
            print(f"  [WARN] Missing {name}")

    # Also compute LNSP from surface pressure
    if "surface_pressure" in data_sfc:
        sp = data_sfc["surface_pressure"]
        data_sfc["_lnsp"] = np.log(np.maximum(sp, 1.0))

    t0_dt = datetime.fromisoformat(str(times_str[0])[:19])
    available_lines = []
    step = args.time_step
    gpu_timesteps = []

    print(f"\nWriting GRIB1 files (every {step}h):")
    for t_idx in range(0, len(times_str), step):
        dt = t0_dt + timedelta(hours=t_idx)
        date_int = int(dt.strftime("%Y%m%d"))
        time_hhmm = dt.hour * 100     # GRIB dataTime: HHMM
        time_hhmmss = dt.hour * 10000  # AVAILABLE file: HHMMSS
        fname = f"EN{dt.strftime('%Y%m%d')}{dt.hour:02d}"
        out_path = os.path.join(args.output_dir, fname)

        pv = compute_hybrid_pv(PRESSURE_LEVELS_TOP_FIRST)
        surface_pressure = data_sfc["surface_pressure"][t_idx, :, :]

        with open(out_path, "wb") as fout:
            for name, param_id in PARAM_3D.items():
                if name not in data_3d:
                    continue
                arr = data_3d[name]
                full_levels = interpolate_pressure_field(
                    arr[t_idx, ::-1, :, :], surface_pressure, pv)
                # ERA5 numpy: index 0 = 1000hPa (surface), index 13 = 50hPa (top)
                # FLEXPART: level 1 = top (50hPa), level 14 = surface (1000hPa)
                for model_level in range(1, len(PRESSURE_LEVELS) + 1):
                    # model_level 1 → top (50hPa) → numpy index 13
                    vals = full_levels[model_level - 1, :, :]
                    write_grib1(fout, param_id, 109, model_level, nx, ny, vals,
                                lat_first, lon_first, lat_last, lon_last,
                                dx, dy, date_int, time_hhmm,
                                nz=len(PRESSURE_LEVELS), pv_array=pv)

            for name, param_id in PARAM_SFC.items():
                if name not in data_sfc:
                    continue
                vals = data_sfc[name][t_idx, :, :]
                if name in ("surface_sensible_heat_flux", "surface_net_solar_radiation"):
                    # ERA5 stores hourly accumulated J/m2; FLEXPART expects W/m2.
                    vals = vals / 3600.0
                elif name in ("convective_precipitation", "large_scale_precipitation"):
                    # ERA5 accumulated metres per hour -> FLEXPART mm/h.
                    vals = vals * 1000.0
                write_grib1(fout, param_id, 1, 0, nx, ny, vals,
                            lat_first, lon_first, lat_last, lon_last,
                            dx, dy, date_int, time_hhmm)

            if "_lnsp" in data_sfc:
                vals = data_sfc["_lnsp"][t_idx, :, :]
                write_grib1(fout, 152, 1, 1, nx, ny, vals,
                            lat_first, lon_first, lat_last, lon_last,
                            dx, dy, date_int, time_hhmm,
                            nz=len(PRESSURE_LEVELS), pv_array=pv)

        size_kb = os.path.getsize(out_path) / 1024
        available_lines.append(f"{date_int} {time_hhmmss:06d}      {fname}      ON DISC")
        gpu_timesteps.append({
            "index": t_idx,
            "date": date_int,
            "time": time_hhmmss,
            "file": fname,
            "datetime_utc": dt.strftime("%Y-%m-%dT%H:%M:00"),
        })
        print(f"  {fname}  ({size_kb:.0f} KB)")

    available_path = os.path.join(args.output_dir, "AVAILABLE")
    with open(available_path, "w") as f:
        f.write("DATE      TIME     FILENAME     SPECIFICATIONS\n")
        f.write("YYYYMMDD  HHMMSS\n")
        f.write("________ ________ __________ __________\n")
        for line in available_lines:
            f.write(line + "\n")

    print(f"\nWrote {len(available_lines)} files + AVAILABLE to {args.output_dir}/")

    gpu_json_path = args.gpu_json or os.path.join(args.output_dir, "gpu_meteo.json")
    heights_m = approximate_heights_m(PRESSURE_LEVELS)
    gpu_data = {
        "grid": {
            "nx": nx, "ny": ny, "nz": len(PRESSURE_LEVELS),
            "dx": dx, "dy": dy,
            "lon_first": lon_first, "lat_first": lat_first,
            "lon_last": lon_last, "lat_last": lat_last,
        },
        "pressure_levels_hpa": PRESSURE_LEVELS,
        "heights_m": heights_m,
        "era5_dir": args.era5_dir,
        "timesteps": gpu_timesteps,
        "release": {
            "name": "ETEX-1",
            "lon": -2.0,
            "lat": 48.058,
            "z_m": 10.0,
            "mass_g": 340000,
            "start": "1994-10-23T16:00:00",
            "end": "1994-10-24T03:40:00",
        },
    }
    with open(gpu_json_path, "w") as f:
        json.dump(gpu_data, f, indent=2)
    print(f"GPU metadata: {gpu_json_path}")


if __name__ == "__main__":
    main()
