#!/usr/bin/env python3
"""Prepare a one-column canonical Snapshot for the FLEXPART #30 Fortran harness."""

import argparse
import json
import math
from pathlib import Path


def field(snapshot, field_id):
    matches = [item for item in snapshot["fields"] if item["id"] == field_id]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one {field_id} field")
    return matches[0]["values"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--snapshot", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--motion", type=Path)
    args = parser.parse_args()

    snapshot = json.loads(args.snapshot.read_text(encoding="utf-8"))
    grid = snapshot["horizontal_grid"]
    vertical = snapshot["vertical_coordinate"]
    if grid["nx"] != 1 or grid["ny"] != 1:
        raise ValueError("oracle column fixture must be exactly 1x1 horizontally")
    if vertical["kind"] != "hybrid_sigma_pressure":
        raise ValueError("oracle column fixture must use hybrid_sigma_pressure")
    if vertical["ordering"] != "increasing":
        raise ValueError("oracle harness input is canonical top-to-bottom/increasing-pressure")
    if vertical["reference"] != "model_native":
        raise ValueError("oracle column fixture must use model_native vertical reference")

    levels = vertical["level_values"]
    a = vertical["hybrid_a_interface_pa"]
    b = vertical["hybrid_b_interface"]
    nz = len(levels)
    if len(a) != nz + 1 or len(b) != nz + 1:
        raise ValueError("hybrid interface coefficient length mismatch")

    surface_pressure = field(snapshot, "surface_pressure")
    temperature = field(snapshot, "temperature")
    humidity = field(snapshot, "specific_humidity")
    orography = field(snapshot, "orography")
    temperature_2m = field(snapshot, "temperature2m")
    dewpoint_2m = field(snapshot, "dewpoint2m")
    if not all(len(values) == 1 for values in (
        surface_pressure, orography, temperature_2m, dewpoint_2m
    )):
        raise ValueError("surface fields must contain exactly one value")
    if len(temperature) != nz or len(humidity) != nz:
        raise ValueError("model-level field length mismatch")

    lines = [
        str(nz),
        " ".join(map(str, [
            surface_pressure[0], temperature_2m[0], dewpoint_2m[0], orography[0]
        ])),
    ]
    lines.extend(f"{a_value} {b_value}" for a_value, b_value in zip(a, b))
    lines.extend(
        f"{temperature_value} {humidity_value}"
        for temperature_value, humidity_value in zip(temperature, humidity)
    )

    if args.motion is None:
        lines.append("0")
    else:
        motion = json.loads(args.motion.read_text(encoding="utf-8"))
        expected = {
            "kind": "pressure_velocity_omega",
            "unit": "pascal_per_second",
            "sign": "positive_pressure_increasing",
            "vertical_staggering": "level_interface",
        }
        for key, value in expected.items():
            if motion.get(key) != value:
                raise ValueError(
                    f"oracle motion fixture requires {key}={value!r}, got {motion.get(key)!r}"
                )
        motion_values = motion.get("values")
        if not isinstance(motion_values, list) or len(motion_values) != nz + 1:
            raise ValueError("oracle interface omega must contain nz+1 values")
        if any(not math.isfinite(value) for value in motion_values):
            raise ValueError("oracle interface omega contains non-finite values")
        lines.append("1")
        lines.extend(str(value) for value in motion_values)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
