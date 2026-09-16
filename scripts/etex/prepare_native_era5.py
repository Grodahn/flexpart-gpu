#!/usr/bin/env python3
"""Prepare paired FLEXPART 11.1 and GPU inputs from native-level ERA5."""

import argparse
import calendar
import hashlib
import json
from datetime import datetime
from pathlib import Path

import eccodes
import numpy as np

from prepare_flexpart_input_from_npy import PARAM_SFC, write_grib1


LEVEL_FIELDS = {130: "t", 131: "u", 132: "v", 133: "q", 135: "omega"}
HEIGHTS_M = np.asarray(
    [10, 50, 100, 200, 400, 600, 900, 1300, 1800, 2500, 3500, 5000,
     7000, 10000, 14000, 20000], dtype=np.float64)
TIMES = ["1994-10-23T15:00:00", "1994-10-23T18:00:00",
         "1994-10-23T21:00:00", "1994-10-24T00:00:00",
         "1994-10-24T03:00:00", "1994-10-24T06:00:00"]
R_DRY = 287.058
G = 9.80665


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_levels(paths):
    """Read native GRIB messages and retain their original bytes for Fortran."""
    fields = {}
    messages = {}
    pv = None
    for path in paths:
        with path.open("rb") as source:
            while (msg := eccodes.codes_grib_new_from_file(source)) is not None:
                try:
                    date = eccodes.codes_get_long(msg, "dataDate")
                    hhmm = eccodes.codes_get_long(msg, "dataTime")
                    stamp = datetime.strptime(f"{date}{hhmm:04d}", "%Y%m%d%H%M")
                    key = stamp.isoformat()
                    param = eccodes.codes_get_long(msg, "paramId")
                    level = eccodes.codes_get_long(msg, "level")
                    if key not in TIMES or param not in LEVEL_FIELDS or not 1 <= level <= 137:
                        raise ValueError(f"unexpected native ERA5 field: {key}, {param}, {level}")
                    field_key = (key, param, level)
                    if field_key in fields:
                        raise ValueError(f"duplicate native ERA5 field: {field_key}")
                    values = np.asarray(eccodes.codes_get_values(msg)).reshape(41, 65)
                    if not np.isfinite(values).all():
                        raise ValueError(f"nonfinite native ERA5 field: {field_key}")
                    fields[field_key] = values
                    if param != 135:
                        messages.setdefault(key, []).append(eccodes.codes_get_message(msg))
                    this_pv = np.asarray(eccodes.codes_get_array(msg, "pv"))
                    if len(this_pv) != 276:
                        raise ValueError("native ERA5 hybrid coefficients are missing")
                    if pv is None:
                        pv = this_pv
                    elif not np.array_equal(pv, this_pv):
                        raise ValueError("inconsistent hybrid coefficients")
                finally:
                    eccodes.codes_release(msg)
    expected = {(t, p, l) for t in TIMES for p in LEVEL_FIELDS for l in range(1, 138)}
    if fields.keys() != expected:
        raise ValueError(f"native ERA5 coverage: missing {len(expected - fields.keys())}")
    return fields, messages, pv


def read_etadot(paths):
    """Keep ERA5 eta-coordinate velocity as an independent FLEXPART field."""
    found = {}
    for path in paths:
        with path.open("rb") as source:
            while (msg := eccodes.codes_grib_new_from_file(source)) is not None:
                try:
                    date = eccodes.codes_get_long(msg, "dataDate")
                    hhmm = eccodes.codes_get_long(msg, "dataTime")
                    stamp = datetime.strptime(f"{date}{hhmm:04d}", "%Y%m%d%H%M").isoformat()
                    level = eccodes.codes_get_long(msg, "level")
                    if (stamp not in TIMES or not 1 <= level <= 137 or
                            eccodes.codes_get_long(msg, "paramId") != 77 or
                            (stamp, level) in found):
                        raise ValueError(f"unexpected ERA5 etadot: {stamp}, {level}")
                    found[(stamp, level)] = eccodes.codes_get_message(msg)
                finally:
                    eccodes.codes_release(msg)
    expected = {(stamp, level) for stamp in TIMES for level in range(1, 138)}
    if found.keys() != expected:
        raise ValueError(f"ERA5 etadot coverage: missing {len(expected - found.keys())}")
    return found


def vertical_geometry(t, q, sp, pv):
    """Compute pressure and approximate hydrostatic height above local ground."""
    a, b = pv[:138], pv[138:]
    half = a[:, None, None] + b[:, None, None] * sp[None]
    pressure = 0.5 * (half[:-1] + half[1:])
    if (pressure <= 0).any() or (np.diff(pressure, axis=0) <= 0).any():
        raise ValueError("invalid ERA5 hybrid pressure coordinate")
    tv = t * (1.0 + 0.608 * q)
    height = np.empty_like(pressure)
    height[-1] = R_DRY * tv[-1] / G * np.log(sp / pressure[-1])
    for k in range(135, -1, -1):
        height[k] = height[k + 1] + R_DRY * (tv[k] + tv[k + 1]) / (2 * G) * \
            np.log(pressure[k + 1] / pressure[k])
    if (np.diff(height[::-1], axis=0) <= 0).any():
        raise ValueError("nonmonotonic native ERA5 heights")
    return pressure, height


def interpolate_heights(field, native_height):
    """Interpolate each ERA5 column to the GPU's fixed AGL height grid."""
    z = native_height[::-1]
    f = field[::-1]
    result = np.empty((len(HEIGHTS_M), 41, 65), dtype=np.float32)
    for k, target in enumerate(HEIGHTS_M):
        high = np.clip(np.sum(z < target, axis=0), 1, 136)
        low = high - 1
        z0 = np.take_along_axis(z, low[None], axis=0)[0]
        z1 = np.take_along_axis(z, high[None], axis=0)[0]
        v0 = np.take_along_axis(f, low[None], axis=0)[0]
        v1 = np.take_along_axis(f, high[None], axis=0)[0]
        weight = np.clip((target - z0) / (z1 - z0), 0, 1)
        result[k] = v0 + weight * (v1 - v0)
    return result


def gpu_3d(field):
    return np.ascontiguousarray(field[:, ::-1, :].transpose(2, 1, 0), dtype="<f4")


def gpu_2d(field):
    return np.ascontiguousarray(field[::-1].T, dtype="<f4")


def prepare(native_dir, meteo_dir, gpu_dir):
    native = [native_dir / "era5-19941023-151821-ml.grib",
              native_dir / "era5-19941024-000306-ml.grib"]
    etadot = [native_dir / "era5-19941023-151821-etadot.grib",
              native_dir / "era5-19941024-000306-etadot.grib"]
    surface_path = native_dir / "era5-surface-19941023-24.npz"
    for path in [*native, *etadot, surface_path]:
        if not path.is_file():
            raise FileNotFoundError(path)
    with np.load(surface_path) as archive:
        if [x.decode() for x in archive["times"]] != TIMES:
            raise ValueError("ERA5 surface times do not match native levels")
        if not np.array_equal(archive["latitudes"], np.arange(53, 42.99, -0.25)):
            raise ValueError("ERA5 surface latitude grid mismatch")
        if not np.array_equal(archive["longitudes"], np.arange(-8, 8.01, 0.25)):
            raise ValueError("ERA5 surface longitude grid mismatch")
        surface = {name: archive[name] for name in PARAM_SFC}
    fields, messages, pv = read_levels(native)
    eta_messages = read_etadot(etadot)
    meteo_dir.mkdir(parents=True, exist_ok=True)
    gpu_dir.mkdir(parents=True, exist_ok=True)
    available = ["DATE      TIME     FILENAME     SPECIFICATIONS",
                 "YYYYMMDD  HHMMSS", "________ ________ __________ __________"]
    entries = []
    checks = {}
    for idx, stamp in enumerate(TIMES):
        dt = datetime.fromisoformat(stamp)
        fname = f"EN{dt:%Y%m%d%H}"
        date, time = int(dt.strftime("%Y%m%d")), dt.hour * 100
        sp = surface["surface_pressure"][idx]
        if np.any(sp <= 0) or not np.isfinite(sp).all():
            raise ValueError(f"invalid surface pressure: {stamp}")
        meteo_file = meteo_dir / fname
        with meteo_file.open("wb") as out:
            for raw in messages[stamp]:
                out.write(raw)
            for level in range(1, 138):
                out.write(eta_messages[(stamp, level)])
            for name, param in PARAM_SFC.items():
                values = surface[name][idx]
                if name in ("surface_sensible_heat_flux", "surface_net_solar_radiation"):
                    values = values / 3600.0
                elif name in ("convective_precipitation", "large_scale_precipitation"):
                    values = values * 1000.0
                write_grib1(out, param, 1, 0, 65, 41, values, 53, 352, 43, 8,
                            0.25, 0.25, date, time)
            write_grib1(out, 152, 1, 1, 65, 41, np.log(sp), 53, 352, 43, 8,
                        0.25, 0.25, date, time)
        available.append(f"{date} {dt.hour:02d}0000      {fname}      ON DISC")

        native_fields = {name: np.stack([fields[(stamp, param, level)]
                                         for level in range(1, 138)])
                         for param, name in LEVEL_FIELDS.items()}
        pressure, height = vertical_geometry(native_fields["t"], native_fields["q"], sp, pv)
        sampled = {name: interpolate_heights(values, height)
                   for name, values in native_fields.items()}
        p = interpolate_heights(pressure, height)
        tv = sampled["t"] * (1.0 + 0.608 * sampled["q"])
        rho = p / (R_DRY * tv)
        w = -sampled["omega"] / (rho * G)
        grad = np.gradient(rho, HEIGHTS_M, axis=0)
        sfc = {name: gpu_2d(surface[name][idx]) for name in PARAM_SFC}
        rho_sfc = sfc["surface_pressure"] / (R_DRY * sfc["2m_temperature"])
        stress = np.hypot(sfc["mean_eastward_turbulent_surface_stress"],
                          sfc["mean_northward_turbulent_surface_stress"])
        zero = np.zeros((65, 41), dtype=np.float32)
        gpu_fields = [sampled["u"], sampled["v"], w, sampled["t"],
                      sampled["q"], p, rho, grad]
        gpu_surface = [sfc["surface_pressure"], sfc["10m_u_component_of_wind"],
                       sfc["10m_v_component_of_wind"], sfc["2m_temperature"],
                       sfc["2m_dewpoint_temperature"],
                       sfc["large_scale_precipitation"] * 1000,
                       sfc["convective_precipitation"] * 1000,
                       sfc["surface_sensible_heat_flux"] / 3600,
                       sfc["surface_net_solar_radiation"] / 3600, stress,
                       np.sqrt(np.maximum(stress, 0) / np.maximum(rho_sfc, 0.5)),
                       zero, np.maximum(sfc["boundary_layer_height"], 50),
                       np.full((65, 41), 10000, dtype=np.float32), zero]
        gpu_file = gpu_dir / f"met_{idx:03d}.bin"
        with gpu_file.open("wb") as out:
            for value in gpu_fields:
                out.write(gpu_3d(value).tobytes())
            for value in gpu_surface:
                out.write(np.ascontiguousarray(value, dtype="<f4").tobytes())
        entries.append({"index": idx, "datetime": stamp,
                        "epoch_seconds": calendar.timegm(dt.timetuple()),
                        "file": gpu_file.name})
        checks[fname] = digest(meteo_file)
        checks[gpu_file.name] = digest(gpu_file)
    (meteo_dir / "AVAILABLE").write_text("\n".join(available) + "\n")
    manifest = {
        "nx": 65, "ny": 41, "nz": 16, "dx_deg": 0.25, "dy_deg": 0.25,
        "xlon0_deg": -8.0, "ylat0_deg": 43.0,
        "heights_m": HEIGHTS_M.tolist(), "timesteps": entries,
        "release": {"name": "ETEX-1", "start": "19941023160000",
                    "end": "19941024034000", "lon": -2.0, "lat": 48.058,
                    "z_min": 5.0, "z_max": 15.0, "mass_kg": 340.0,
                    "particle_count": 10000},
        "simulation": {"start": "19941023160000", "end": "19941024040000",
                       "dt_seconds": 900},
        "output": {"nx": 64, "ny": 40, "nz": 5,
                   "heights_m": [100, 500, 1000, 2000, 5000],
                   "interval_seconds": 10800, "sampling_seconds": 900},
        "meteorology": {"native_grib_sha256": {p.name: digest(p) for p in native + etadot},
                        "surface_archive_sha256": digest(surface_path),
                        "fortran_input_sha256": checks,
                        "gpu_vertical_grid": "16 fixed AGL heights, hydrostatic interpolation"},
    }
    (gpu_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Prepared {len(TIMES)} paired ERA5 snapshots")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-dir", type=Path, required=True)
    parser.add_argument("--meteo-dir", type=Path, required=True)
    parser.add_argument("--gpu-dir", type=Path, required=True)
    args = parser.parse_args()
    prepare(args.native_dir, args.meteo_dir, args.gpu_dir)


if __name__ == "__main__":
    main()
