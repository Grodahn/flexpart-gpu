#!/usr/bin/env python3
"""Select the six ETEX mini surface snapshots from independent ERA5 arrays."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np


FIELDS = (
    "surface_pressure", "10m_u_component_of_wind", "10m_v_component_of_wind",
    "2m_temperature", "2m_dewpoint_temperature", "boundary_layer_height",
    "surface_sensible_heat_flux", "surface_net_solar_radiation",
    "mean_eastward_turbulent_surface_stress",
    "mean_northward_turbulent_surface_stress", "convective_precipitation",
    "large_scale_precipitation", "forecast_surface_roughness",
    "geopotential_at_surface", "land_sea_mask",
)
TIMES = (
    "1994-10-23T15:00:00", "1994-10-23T18:00:00",
    "1994-10-23T21:00:00", "1994-10-24T00:00:00",
    "1994-10-24T03:00:00", "1994-10-24T06:00:00",
)


def build(source: Path, output: Path):
    source_times = np.load(source / "times.npy").astype("datetime64[s]")
    wanted = np.asarray(TIMES, dtype="datetime64[s]")
    indices = []
    for stamp in wanted:
        matches = np.flatnonzero(source_times == stamp)
        if len(matches) != 1:
            raise ValueError(f"expected exactly one ERA5 surface snapshot at {stamp}")
        indices.append(int(matches[0]))
    arrays = {name: np.load(source / f"{name}.npy")[indices]
              for name in FIELDS}
    arrays["times"] = wanted.astype("S19")
    arrays["latitudes"] = np.load(source / "latitudes.npy")
    arrays["longitudes"] = np.load(source / "longitudes.npy")
    for name in FIELDS:
        if arrays[name].shape != (6, 41, 65) or not np.isfinite(arrays[name]).all():
            raise ValueError(f"unexpected ERA5 surface field: {name}")
    if not np.array_equal(arrays["latitudes"], np.arange(53, 42.99, -0.25)):
        raise ValueError("unexpected latitude grid")
    if not np.array_equal(arrays["longitudes"], np.arange(-8, 8.01, 0.25)):
        raise ValueError("unexpected longitude grid")
    output.parent.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(output, **arrays)
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    print(json.dumps({"file": str(output), "bytes": output.stat().st_size,
                      "sha256": digest, "times": TIMES}, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    build(args.source, args.output)


if __name__ == "__main__":
    main()
