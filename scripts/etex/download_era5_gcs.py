#!/usr/bin/env python3
"""Download ERA5 data for ETEX-1 from Google ARCO-ERA5 (no CDS account needed).

Uses the Analysis-Ready, Cloud Optimized (ARCO) ERA5 dataset hosted on
Google Cloud Storage. This is freely accessible without authentication.

Downloads only the variables needed for FLEXPART: 3D wind/T/q on pressure
levels + surface fields, for Oct 23-26, 1994 over Europe.

Prerequisites:
    uv pip install xarray gcsfs zarr numpy

Usage:
    python3 scripts/etex/download_era5_gcs.py --output-dir target/etex/era5_raw
"""

import argparse
import json
import os
import sys
import time as time_mod

try:
    import numpy as np
except ImportError:
    print("Missing numpy", file=sys.stderr); sys.exit(1)
try:
    import xarray as xr
except ImportError:
    print("Missing xarray. Install: uv pip install xarray", file=sys.stderr); sys.exit(1)
try:
    import gcsfs
except ImportError:
    print("Missing gcsfs. Install: uv pip install gcsfs", file=sys.stderr); sys.exit(1)


ARCO_ERA5_BUCKET = "gs://gcp-public-data-arco-era5/ar/full_37-1h-0p25deg-chunk-1.zarr-v3"

LON_RANGE = slice(-15, 35)
LAT_RANGE = slice(65, 35)
TIME_START = "1994-10-23T00:00"
TIME_END = "1994-10-26T23:00"

PRESSURE_LEVELS = [1000, 925, 850, 700, 600, 500, 400, 300, 250, 200, 150, 100, 70, 50]

VARS_3D = [
    "u_component_of_wind",
    "v_component_of_wind",
    "vertical_velocity",
    "temperature",
    "specific_humidity",
]

VARS_SFC = [
    "surface_pressure",
    "10m_u_component_of_wind",
    "10m_v_component_of_wind",
    "2m_temperature",
    "2m_dewpoint_temperature",
    "boundary_layer_height",
    "surface_sensible_heat_flux",
    "surface_net_solar_radiation",
    "mean_eastward_turbulent_surface_stress",
    "mean_northward_turbulent_surface_stress",
    "convective_precipitation",
    "large_scale_precipitation",
    "forecast_surface_roughness",
    "geopotential_at_surface",
    "land_sea_mask",
]


def open_arco_era5():
    fs = gcsfs.GCSFileSystem(token="anon")
    store = gcsfs.GCSMap(ARCO_ERA5_BUCKET, gcs=fs)
    return xr.open_zarr(store, consolidated=True)


def select_longitudes(dataset, west: float, east: float):
    """Return increasing signed longitudes even when ARCO uses 0..360."""
    if west < 0 <= east and float(dataset.longitude.min()) >= 0:
        negative = dataset.sel(longitude=slice(360 + west, None))
        negative = negative.assign_coords(longitude=negative.longitude - 360)
        positive = dataset.sel(longitude=slice(0, east))
        return xr.concat([negative, positive], dim="longitude")
    return dataset.sel(longitude=slice(west, east))


def download_variable(ds, var_name: str, output_dir: str, is_3d: bool,
                      time_start: str, time_end: str,
                      lat_range: slice, lon_range: slice) -> dict:
    """Download a single variable and save as .npy."""
    if var_name not in ds:
        return None

    var = ds[var_name]
    t0 = time_mod.time()

    out_path = os.path.join(output_dir, f"{var_name}.npy")
    if os.path.isfile(out_path):
        cached = np.load(out_path, mmap_mode="r")
        print(f"  {var_name:45s} {str(cached.shape):25s} cached")
        return {"name": var_name, "dims": list(var.dims),
                "shape": list(cached.shape), "dtype": str(cached.dtype),
                "file": f"{var_name}.npy", "size_mb": round(cached.nbytes / 1e6, 1)}

    try:
        if is_3d and "level" in var.dims:
            subset = var.sel(
                time=slice(time_start, time_end),
                level=PRESSURE_LEVELS,
                latitude=lat_range,
            )
        else:
            subset = var.sel(
                time=slice(time_start, time_end),
                latitude=lat_range,
            )
    except Exception as e:
        print(f"  [SKIP] {var_name}: {e}")
        return None

    if lon_range.start < 0 and float(subset.longitude.min()) >= 0:
        west = subset.sel(longitude=slice(360 + lon_range.start, None))
        east = subset.sel(longitude=slice(0, lon_range.stop))
        shape = subset.shape[:-1] + (west.shape[-1] + east.shape[-1],)
        print(f"  {var_name:45s} {str(shape):25s}", end="", flush=True)
        data = np.concatenate([west.values, east.values], axis=-1)
    else:
        subset = subset.sel(longitude=lon_range)
        print(f"  {var_name:45s} {str(subset.shape):25s}", end="", flush=True)
        data = subset.values
    dt = time_mod.time() - t0
    size_mb = data.nbytes / 1e6

    np.save(out_path, data)
    print(f" {size_mb:6.1f} MB  ({dt:.1f}s)")

    return {
        "name": var_name,
        "dims": list(subset.dims),
        "shape": list(data.shape),
        "dtype": str(data.dtype),
        "file": f"{var_name}.npy",
        "size_mb": round(size_mb, 1),
    }


def main():
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--output-dir", default="target/etex/era5_raw")
    parser.add_argument("--time-start", default=TIME_START)
    parser.add_argument("--time-end", default=TIME_END)
    parser.add_argument("--lon-west", type=float, default=LON_RANGE.start)
    parser.add_argument("--lon-east", type=float, default=LON_RANGE.stop)
    parser.add_argument("--lat-north", type=float, default=LAT_RANGE.start)
    parser.add_argument("--lat-south", type=float, default=LAT_RANGE.stop)
    args = parser.parse_args()
    lat_range = slice(args.lat_north, args.lat_south)
    lon_range = slice(args.lon_west, args.lon_east)

    os.makedirs(args.output_dir, exist_ok=True)
    request = {"lat_north": args.lat_north, "lat_south": args.lat_south,
               "lon_west": args.lon_west, "lon_east": args.lon_east}
    metadata_path = os.path.join(args.output_dir, "metadata.json")
    if any(name.endswith(".npy") for name in os.listdir(args.output_dir)):
        if not os.path.isfile(metadata_path):
            raise ValueError("cached ERA5 arrays lack metadata; use a fresh output directory")
        with open(metadata_path, encoding="utf-8") as source:
            previous = json.load(source)
        if (previous.get("source") != ARCO_ERA5_BUCKET
                or previous.get("time_start") != args.time_start
                or previous.get("time_end") != args.time_end
                or previous.get("requested_domain") != request
                or previous.get("pressure_levels_hpa") != PRESSURE_LEVELS):
            raise ValueError("cached ERA5 request differs; use a fresh output directory")

    print("=" * 70)
    print("  ERA5 for ETEX-1 via Google ARCO-ERA5 (no CDS account)")
    print("=" * 70)
    print(f"  Period:  {args.time_start} to {args.time_end}")
    print(f"  Domain:  {lat_range.start}°N-{lat_range.stop}°N, "
          f"{lon_range.start}°E-{lon_range.stop}°E")
    print(f"  Levels:  {len(PRESSURE_LEVELS)} pressure levels")
    print(f"  3D vars: {len(VARS_3D)}")
    print(f"  Sfc vars: {len(VARS_SFC)}")
    print()

    print("Opening ARCO-ERA5 store...")
    ds = open_arco_era5()
    print(f"  {len(ds.data_vars)} variables available")

    time_ds = ds.sel(time=slice(args.time_start, args.time_end))
    n_times = len(time_ds.time)
    print(f"  {n_times} hourly timesteps selected")

    lat_sub = ds.sel(latitude=lat_range).latitude.values
    lon_sub = select_longitudes(ds.longitude, lon_range.start, lon_range.stop).longitude.values
    print(f"  Grid: {len(lat_sub)} lat x {len(lon_sub)} lon")

    metadata = {
        "source": ARCO_ERA5_BUCKET,
        "time_start": args.time_start,
        "time_end": args.time_end,
        "n_timesteps": n_times,
        "timesteps": [str(t) for t in time_ds.time.values[:4]] + ["..."],
        "latitudes_range": [float(lat_sub[-1]), float(lat_sub[0])],
        "longitudes_range": [float(lon_sub[0]), float(lon_sub[-1])],
        "n_lat": len(lat_sub),
        "n_lon": len(lon_sub),
        "pressure_levels_hpa": PRESSURE_LEVELS,
        "requested_domain": request,
        "variables": [],
    }

    print("\nDownloading 3D fields (pressure levels):")
    for var in VARS_3D:
        info = download_variable(ds, var, args.output_dir, is_3d=True,
                                 time_start=args.time_start, time_end=args.time_end,
                                 lat_range=lat_range, lon_range=lon_range)
        if info:
            metadata["variables"].append(info)

    print("\nDownloading surface fields:")
    for var in VARS_SFC:
        info = download_variable(ds, var, args.output_dir, is_3d=False,
                                 time_start=args.time_start, time_end=args.time_end,
                                 lat_range=lat_range, lon_range=lon_range)
        if info:
            metadata["variables"].append(info)

    np.save(os.path.join(args.output_dir, "latitudes.npy"), lat_sub)
    np.save(os.path.join(args.output_dir, "longitudes.npy"), lon_sub)
    np.save(os.path.join(args.output_dir, "times.npy"),
            np.array([str(t) for t in time_ds.time.values]))

    meta_path = os.path.join(args.output_dir, "metadata.json")
    with open(meta_path, "w") as f:
        json.dump(metadata, f, indent=2)

    total_mb = sum(v.get("size_mb", 0) for v in metadata["variables"])
    print(f"\nDone: {len(metadata['variables'])} variables, {total_mb:.0f} MB total")
    print(f"Metadata: {meta_path}")
    print(f"\nNext: python3 scripts/etex/prepare_flexpart_input_from_npy.py \\")
    print(f"        --era5-dir {args.output_dir} --output-dir target/etex/meteo")


if __name__ == "__main__":
    main()
