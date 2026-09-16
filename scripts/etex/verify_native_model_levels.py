#!/usr/bin/env python3
"""Verify the checked-in native ERA5 mini GRIB and its declared coverage."""

import argparse
import hashlib
import json
from pathlib import Path

import eccodes
import numpy as np


def verify(path, manifest_path):
    record = json.loads(manifest_path.read_text(encoding="utf-8"))
    if path.stat().st_size != record["bytes"]:
        raise ValueError("ERA5 native GRIB byte count differs from manifest")
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if digest != record["sha256"]:
        raise ValueError("ERA5 native GRIB SHA-256 differs from manifest")

    seen = set()
    expected = {(param, level, time)
                for param in (130, 131, 132, 133, 135)
                for level in range(1, 138)
                for time in (1500, 1800, 2100)}
    count = 0
    with path.open("rb") as source:
        while (message := eccodes.codes_grib_new_from_file(source)) is not None:
            try:
                key = tuple(eccodes.codes_get_long(message, name)
                            for name in ("paramId", "level", "dataTime"))
                if key not in expected:
                    raise ValueError(f"unexpected GRIB field {key}")
                if (eccodes.codes_get_long(message, "dataDate") != 19941023
                        or eccodes.codes_get_long(message, "Ni") != 65
                        or eccodes.codes_get_long(message, "Nj") != 41
                        or eccodes.codes_get_long(message, "NV") != 276
                        or eccodes.codes_get_long(message, "PVPresent") != 1):
                    raise ValueError(f"wrong grid, date, or hybrid coordinate in {key}")
                if (eccodes.codes_get(message, "gridType") != "regular_ll"
                        or eccodes.codes_get_double(message, "latitudeOfFirstGridPointInDegrees") != 53.0
                        or eccodes.codes_get_double(message, "latitudeOfLastGridPointInDegrees") != 43.0
                        or eccodes.codes_get_double(message, "longitudeOfFirstGridPointInDegrees") != 352.0
                        or eccodes.codes_get_double(message, "longitudeOfLastGridPointInDegrees") != 8.0):
                    raise ValueError(f"unexpected GRIB geometry in {key}")
                if not np.all(np.isfinite(eccodes.codes_get_values(message))):
                    raise ValueError(f"non-finite values in {key}")
                seen.add(key)
                count += 1
            finally:
                eccodes.codes_release(message)
    if seen != expected or count != len(expected):
        raise ValueError(f"GRIB field coverage mismatch: {count} messages, {len(seen)} unique")
    print(f"Verified {count} ERA5 model-level fields; SHA-256 {digest}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    base = Path(__file__).resolve().parents[2] / "fixtures/etex/native-mini"
    parser.add_argument("--file", type=Path, default=base / "era5-19941023-151821-ml.grib")
    parser.add_argument("--manifest", type=Path, default=base / "request.json")
    args = parser.parse_args()
    verify(args.file, args.manifest)


if __name__ == "__main__":
    main()
