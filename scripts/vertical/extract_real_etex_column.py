#!/usr/bin/env python3
"""Build a 1x1x137 #30 vertical Snapshot from the checked-in ETEX/ERA5 fixtures.

This is validation-fixture plumbing only, not a production provider adapter (#32).
The #29 canonical native-level fixture remains unchanged; this script selects one
of its columns and adds the FLEXPART surface thermodynamic boundary from the
already checked-in ERA5 surface archive.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np

GA = 9.81


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def select_field(source, field_id, x, y):
    matches = [field for field in source["fields"] if field["id"] == field_id]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one {field_id} field")
    field = matches[0]
    nx = source["horizontal_grid"]["nx"]
    ny = source["horizontal_grid"]["ny"]

    if field["shape"] == [nx, ny]:
        values = [field["values"][x + nx * y]]
        shape = [1, 1]
        axes = ["x", "y"]
        staggering = "not_applicable"
    elif len(field["shape"]) == 3 and field["shape"][:2] == [nx, ny]:
        nz = field["shape"][2]
        values = [
            field["values"][x + nx * (y + ny * z)]
            for z in range(nz)
        ]
        shape = [1, 1, nz]
        axes = ["x", "y", "z"]
        staggering = field["vertical_staggering"]
    else:
        raise ValueError(f"unsupported source field shape for {field_id}: {field['shape']}")

    result = dict(field)
    result["shape"] = shape
    result["axis_order"] = axes
    result["vertical_staggering"] = staggering
    result["values"] = values
    return result


def surface_field(field_id, unit, sign, temporal_kind, timestamp, value):
    return {
        "id": field_id,
        "shape": [1, 1],
        "axis_order": ["x", "y"],
        "storage_order": "x_fastest",
        "unit": unit,
        "sign": sign,
        "horizontal_staggering": "cell_center",
        "vertical_staggering": "not_applicable",
        "time": {
            "calendar": "gregorian",
            "kind": temporal_kind,
            "valid_time_epoch_seconds": timestamp,
        },
        "values": [float(value)],
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--canonical", type=Path, required=True)
    parser.add_argument("--canonical-provenance", type=Path, required=True)
    parser.add_argument("--surface-archive", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--provenance-output", type=Path, required=True)
    parser.add_argument("--x", type=int, default=0)
    parser.add_argument("--y", type=int, default=0)
    args = parser.parse_args()

    source = json.loads(args.canonical.read_text(encoding="utf-8"))
    source_provenance = json.loads(
        args.canonical_provenance.read_text(encoding="utf-8")
    )
    nx = source["horizontal_grid"]["nx"]
    ny = source["horizontal_grid"]["ny"]
    if not (0 <= args.x < nx and 0 <= args.y < ny):
        raise ValueError("selected canonical column is out of bounds")
    if source["vertical_coordinate"]["kind"] != "hybrid_sigma_pressure":
        raise ValueError("source is not the expected hybrid native-level fixture")

    horizontal = source_provenance["slice"]["horizontal"]
    lon_indices = horizontal["source_lon_indices"]
    lat_indices = horizontal["source_lat_desc_indices"]
    source_row = lat_indices[args.y]
    source_col = lon_indices[args.x]

    with np.load(args.surface_archive) as archive:
        timestamp = archive["times"][0].decode()
        expected_timestamp = source_provenance["slice"]["timestamp"].removesuffix("Z")
        if timestamp != expected_timestamp:
            raise ValueError(
                f"surface/canonical timestamp mismatch: {timestamp} != {expected_timestamp}"
            )
        t2 = float(archive["2m_temperature"][0, source_row, source_col])
        td2 = float(archive["2m_dewpoint_temperature"][0, source_row, source_col])
        geopotential = float(archive["geopotential_at_surface"][0, source_row, source_col])
        ps_archive = float(archive["surface_pressure"][0, source_row, source_col])

    ps = select_field(source, "surface_pressure", args.x, args.y)
    if abs(ps["values"][0] - ps_archive) > max(0.05, abs(ps_archive) * 1.0e-6):
        raise ValueError("canonical and surface-archive pressure differ")
    timestamp_epoch = ps["time"]["valid_time_epoch_seconds"]

    temperature = select_field(source, "temperature", args.x, args.y)
    humidity = select_field(source, "specific_humidity", args.x, args.y)
    terrain_m = geopotential / GA

    output = {
        "schema": source["schema"],
        "horizontal_grid": {
            "nx": 1,
            "ny": 1,
            "xlon0_deg": horizontal["longitudes_deg"][args.x],
            "ylat0_deg": horizontal["latitudes_deg"][args.y],
            "dx_deg": source["horizontal_grid"]["dx_deg"],
            "dy_deg": source["horizontal_grid"]["dy_deg"],
            "longitude_domain": source["horizontal_grid"]["longitude_domain"],
        },
        "vertical_coordinate": source["vertical_coordinate"],
        "fields": [
            ps,
            temperature,
            humidity,
            surface_field(
                "orography", "meter", "signed_scalar", "static", 0, terrain_m
            ),
            surface_field(
                "temperature2m", "kelvin", "signed_scalar", "instantaneous",
                timestamp_epoch, t2
            ),
            surface_field(
                "dewpoint2m", "kelvin", "signed_scalar", "instantaneous",
                timestamp_epoch, td2
            ),
        ],
    }

    args.output.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(output, indent=2) + "\n"
    args.output.write_text(encoded, encoding="utf-8")

    provenance = {
        "schema": "flexpart-gpu.vertical-real-column-fixture.v1",
        "output_sha256": hashlib.sha256(encoded.encode("utf-8")).hexdigest(),
        "canonical_source": {
            "path": str(args.canonical),
            "sha256": sha256(args.canonical),
        },
        "canonical_provenance": {
            "path": str(args.canonical_provenance),
            "sha256": sha256(args.canonical_provenance),
        },
        "surface_archive": {
            "path": str(args.surface_archive),
            "sha256": sha256(args.surface_archive),
        },
        "selection": {
            "canonical_x": args.x,
            "canonical_y": args.y,
            "source_lat_desc_index": source_row,
            "source_lon_index": source_col,
            "longitude_deg": horizontal["longitudes_deg"][args.x],
            "latitude_deg": horizontal["latitudes_deg"][args.y],
            "timestamp": source_provenance["slice"]["timestamp"],
            "native_levels": len(source["vertical_coordinate"]["level_values"]),
        },
        "surface_boundary": {
            "surface_pressure_pa": ps["values"][0],
            "temperature_2m_k": t2,
            "dewpoint_2m_k": td2,
            "surface_geopotential_m2_s2": geopotential,
            "orography_asl_m": terrain_m,
            "orography_conversion": "geopotential_at_surface / 9.81 m/s^2",
        },
        "scope": (
            "Validation-only #30 column assembled from already checked-in inputs; "
            "not a production provider decoder."
        ),
    }
    args.provenance_output.write_text(
        json.dumps(provenance, indent=2) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()
