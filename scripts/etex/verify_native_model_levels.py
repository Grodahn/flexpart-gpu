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
    wanted_date = int(record["request"]["date"].replace("-", ""))
    wanted_times = {int(value) * 100 for value in record["request"]["time"].split("/")}
    expected = {(param, level, time)
                for param in (130, 131, 132, 133, 135)
                for level in range(1, 138)
                for time in wanted_times}
    count = 0
    with path.open("rb") as source:
        while (message := eccodes.codes_grib_new_from_file(source)) is not None:
            try:
                key = tuple(eccodes.codes_get_long(message, name)
                            for name in ("paramId", "level", "dataTime"))
                if key not in expected:
                    raise ValueError(f"unexpected GRIB field {key}")
                if (eccodes.codes_get_long(message, "dataDate") != wanted_date
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


def verify_etadot(base):
    record = json.loads((base / "request-etadot.json").read_text(encoding="utf-8"))
    for entry in record["files"]:
        path = base / entry["file"]
        if path.stat().st_size != entry["bytes"] or \
                hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]:
            raise ValueError(f"ERA5 etadot differs from manifest: {path.name}")
        expected = {(level, int(hour) * 100)
                    for level in range(1, 138)
                    for hour in entry["time"].split("/")}
        seen = set()
        with path.open("rb") as source:
            while (message := eccodes.codes_grib_new_from_file(source)) is not None:
                try:
                    key = (eccodes.codes_get_long(message, "level"),
                           eccodes.codes_get_long(message, "dataTime"))
                    if (key not in expected or
                            eccodes.codes_get_long(message, "paramId") != 77 or
                            eccodes.codes_get_long(message, "Ni") != 65 or
                            eccodes.codes_get_long(message, "Nj") != 41 or
                            eccodes.codes_get_long(message, "NV") != 276 or
                            eccodes.codes_get_long(message, "dataDate") !=
                            int(entry["date"].replace("-", "")) or
                            not np.isfinite(eccodes.codes_get_values(message)).all()):
                        raise ValueError(f"unexpected ERA5 etadot field: {key}")
                    seen.add(key)
                finally:
                    eccodes.codes_release(message)
        if seen != expected:
            raise ValueError(f"ERA5 etadot coverage mismatch: {path.name}")
        print(f"Verified {len(seen)} ERA5 etadot fields; SHA-256 {entry['sha256']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    base = Path(__file__).resolve().parents[2] / "fixtures/etex/native-mini"
    parser.add_argument("--file", type=Path, default=base / "era5-19941023-151821-ml.grib")
    parser.add_argument("--manifest", type=Path, default=base / "request.json")
    args = parser.parse_args()
    verify(args.file, args.manifest)
    next_day = base / "era5-19941024-000306-ml.grib"
    next_manifest = base / "request-next-day.json"
    verify(next_day, next_manifest)
    verify_etadot(base)
    surface = base / "era5-surface-19941023-24.npz"
    surface_record = json.loads((base / "surface-request.json").read_text(encoding="utf-8"))
    if surface.stat().st_size != surface_record["bytes"] or \
            hashlib.sha256(surface.read_bytes()).hexdigest() != surface_record["sha256"]:
        raise ValueError("ERA5 surface fixture differs from manifest")
    with np.load(surface) as data:
        if len(data["times"]) != 6 or not all(
                np.isfinite(data[name]).all() and data[name].shape == (6, 41, 65)
                for name in surface_record["fields"]):
            raise ValueError("ERA5 surface coverage is incomplete")
    print("Verified six ERA5 surface snapshots")


if __name__ == "__main__":
    main()
