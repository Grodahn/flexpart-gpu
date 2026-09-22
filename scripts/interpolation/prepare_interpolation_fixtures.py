#!/usr/bin/env python3
"""Generate the canonical interpolation oracle fixture pack for issue #71.

Writes the FLEXPART direct-oracle input files (`target/interpolation/`), runs the
pinned oracle binary for each sampling case, parses the versioned output, and packs
inputs + golden values into `fixtures/interpolation/contract-v1.json` with a
provenance sibling.

Usage:
  prepare_interpolation_fixtures.py \
      --input-dir target/interpolation/oracle-input \
      --output-dir target/interpolation/oracle-output \
      --binary target/interpolation/oracle-build/interpolation-oracle \
      --oracle-checkout <path to pinned FLEXPART checkout> \
      --reference-manifest reference/flexpart-11.1.json \
      --real-vertical-routine-oracle-output target/ci-gate/vertical-column/real-routine-oracle-output.txt \
      --out-fixture fixtures/interpolation/contract-v1.json

Pass --offline (with pre-generated oracle-output files) to skip re-running the oracle.
"""

import argparse
import hashlib
import json
import math
import re
import subprocess
from pathlib import Path

ORACLE_OUTPUT_VERSION = "FLEXPART_INTERPOLATION_ROUTINE_ORACLE_V1"

# Case -> (mode, grid spec string, field/level data lines, queries lines, query record spec)
# Grid spec string parts: "<canonical_nx> <canonical_ny> <canonical_nz>" and
# "<xlon0> <ylat0> <dx> <dy>" and periodic flag. See the driver input format.
CASES = {
    "horizontal-interior": {
        "mode": "horizontal",
        "grid": ("4 3 1", "0.0 0.0 1.0 1.0", "0"),
        "data": [
            "100.0", "200.0", "300.0", "400.0",
            "110.0", "210.0", "310.0", "410.0",
            "120.0", "220.0", "320.0", "420.0",
        ],
        "queries": ["2", "1.25 0.5 1", "2.9 2.0 1"],
    },
    "horizontal-geographic-interior": {
        "mode": "horizontal_geographic",
        "grid": ("4 3 1", "-2.0 48.0 0.25 0.25", "0"),
        "data": [
            "100.0", "200.0", "300.0", "400.0",
            "110.0", "210.0", "310.0", "410.0",
            "120.0", "220.0", "320.0", "420.0",
        ],
        "queries": [
            "2",
            "-1.6875 48.125 1",
            "-1.375 48.375 1",
        ],
    },
    "horizontal-periodic-wrap": {
        "mode": "horizontal",
        "grid": ("4 3 1", "0.0 0.0 1.0 1.0", "1"),
        "data": [
            "100.0", "200.0", "300.0", "400.0",
            "110.0", "210.0", "310.0", "410.0",
            "120.0", "220.0", "320.0", "420.0",
        ],
        "queries": ["3", "3.2 1.5 1", "3.9 0.5 1", "0.4 2.0 1"],
    },
    "vertical-model-levels": {
        "mode": "vertical",
        "vertical_staggering": "level_center",
        "grid": ("3", "10.0", "100.0", "1000.0", "3", "1.0", "2.0", "3.0"),
        "queries": [
            "6",
            "0 5.0", "0 50.0", "0 100.0", "0 500.0", "0 1000.0", "0 2000.0",
        ],
    },
    "vertical-interface-wzlev": {
        "mode": "vertical",
        "vertical_staggering": "level_interface",
        "source_oracle": {
            "producer_issue": 30,
            "producer_driver": "scripts/vertical/direct_oracle_driver.f90",
            "producer_routine": "verttransform_mod::verttransform_ecmwf_heights",
            "oracle_output_sha256": "5015ea3a9a9e42b1a2b88c60c2867b74a632bffd1b9cfefdc186b005c752b197",
            "geometry_field": "wzlev",
            "value_field": "omega * pinmconv",
            "source_fixture": "fixtures/vertical/synthetic-column-v1.json",
            "source_motion": "fixtures/vertical/synthetic-omega-interface-v1.json",
        },
        "grid": (
            "4",
            "0.0",
            "1954.792236328125",
            "4363.86865234375",
            "7076.8583984375",
            "4",
            "0.13533158600330353",
            "0.10024577379226685",
            "0.060226909816265106",
            "-0.0",
        ),
        "queries": [
            "7",
            "1 -100.0", "1 0.0", "1 1000.0", "1 3000.0",
            "1 6000.0", "1 7076.8583984375", "1 8000.0",
        ],
    },
    "temporal-bilinear": {
        "mode": "temporal",
        "grid": ("0 3600",),
        "data": [],
        "queries": ["3", "0 10.0 20.0", "1800 10.0 20.0", "3600 10.0 20.0"],
    },
    "rain-layer-fields": {
        "mode": "rain",
        "grid": ("4 3", "0.0 0.0 1.0 1.0", "0", "0 3600",),
        "data": [
            # lsp t1, lsp t2
            "0.0", "1.0", "2.0", "3.0", "4.0", "5.0", "6.0", "7.0",
            "8.0", "9.0", "10.0", "11.0",
            "100.0", "101.0", "102.0", "103.0", "104.0", "105.0", "106.0", "107.0",
            "108.0", "109.0", "110.0", "111.0",
            # convprec t1, t2
            "0.5", "0.5", "0.5", "0.5", "0.5", "0.5", "0.5", "0.5",
            "0.5", "0.5", "0.5", "0.5",
            "1.5", "1.5", "1.5", "1.5", "1.5", "1.5", "1.5", "1.5",
            "1.5", "1.5", "1.5", "1.5",
            # tcc t1, t2
            "0.2", "0.2", "0.2", "0.2", "0.2", "0.2", "0.2", "0.2",
            "0.2", "0.2", "0.2", "0.2",
            "0.6", "0.6", "0.6", "0.6", "0.6", "0.6", "0.6", "0.6",
            "0.6", "0.6", "0.6", "0.6",
            # tt t1, t2
            "280.0", "280.0", "280.0", "280.0", "280.0", "280.0", "280.0", "280.0",
            "280.0", "280.0", "280.0", "280.0",
            "290.0", "290.0", "290.0", "290.0", "290.0", "290.0", "290.0", "290.0",
            "290.0", "290.0", "290.0", "290.0",
            # ctwc t1, t2
            "0.001", "0.001", "0.001", "0.001", "0.001", "0.001", "0.001", "0.001",
            "0.001", "0.001", "0.001", "0.001",
            "0.004", "0.004", "0.004", "0.004", "0.004", "0.004", "0.004", "0.004",
            "0.004", "0.004", "0.004", "0.004",
        ],
        "queries": ["1", "1.25 0.5 1800 1"],
    },
}

CASE_SEMANTICS = {
    "horizontal-interior": {
        "coordinates": {
            "horizontal": "canonical cell-center grid-index coordinates xt/yt",
            "vertical": "kz is a 1-based FLEXPART model-level index",
        },
        "staggering": {"horizontal": "cell_center", "vertical": "level_center"},
        "ordering": {"horizontal_storage": "x_fastest_then_y", "vertical": "single_level"},
        "units": {"horizontal_query": "grid_cell", "vertical_query": "index", "value": "arbitrary_scalar"},
        "time": {
            "kind": "instantaneous_static_for_fixture",
            "memory_slots": "same field copied to both FLEXPART memory slots",
        },
        "supported_query_domain": {
            "x": "0 <= xt <= nx-1 for non-periodic canonical grids",
            "y": "0 <= yt <= ny-1",
            "out_of_domain": "not frozen by #71; downstream #72 fails closed",
        },
    },
    "horizontal-geographic-interior": {
        "coordinates": {
            "horizontal": (
                "geographic longitude/latitude cell-center coordinates converted "
                "by point_mod::coordtrafo before interpolation"
            ),
            "vertical": "kz is a 1-based FLEXPART model-level index",
        },
        "staggering": {"horizontal": "cell_center", "vertical": "level_center"},
        "ordering": {"horizontal_storage": "x_fastest_then_y", "vertical": "single_level"},
        "units": {
            "horizontal_query": "degrees_east/degrees_north",
            "internal_horizontal": "grid_cell",
            "vertical_query": "index",
            "value": "arbitrary_scalar",
        },
        "time": {
            "kind": "instantaneous_static_for_fixture",
            "memory_slots": "same field copied to both FLEXPART memory slots",
        },
        # The direct oracle intentionally composes coordtrafo with the
        # interpolation primitives to prove the geographic->grid mapping. This is
        # an oracle exercise path, not a pristine production call chain:
        # coordtrafo is used during release-point initialization, while runtime
        # meteorology sampling already operates in FLEXPART grid coordinates.
        "oracle_exercise_path": [
            "point_mod::coordtrafo",
            "interpol_mod::find_grid_indices",
            "interpol_mod::find_grid_distances",
            "interpol_mod::hor_interpol_4d",
        ],
        "production_direct_call_edges": {
            "release_coordinate_initialization": [
                [
                    "FLEXPART::read_options_and_initialise_flexpart",
                    "point_mod::coordtrafo",
                ],
            ],
            "above_pbl_wind_sampling": [
                ["advance_mod::advance", "interpol_mod::init_interpol"],
                ["interpol_mod::init_interpol", "interpol_mod::find_ngrid"],
                ["interpol_mod::init_interpol", "interpol_mod::find_grid_indices"],
                ["interpol_mod::init_interpol", "interpol_mod::find_grid_distances"],
                ["interpol_mod::init_interpol", "interpol_mod::find_time_vars"],
                ["interpol_mod::init_interpol", "interpol_mod::find_z_level"],
                ["advance_mod::advance", "advance_mod::adv_above_pbl"],
                ["advance_mod::adv_above_pbl", "interpol_mod::interpol_wind"],
                ["interpol_mod::interpol_wind", "interpol_mod::find_ngrid"],
                ["interpol_mod::interpol_wind", "interpol_mod::find_grid_indices"],
                ["interpol_mod::interpol_wind", "interpol_mod::find_grid_distances"],
                ["interpol_mod::interpol_wind", "interpol_mod::find_time_vars"],
                ["interpol_mod::interpol_wind", "interpol_mod::find_z_level_meters"],
                ["interpol_mod::interpol_wind", "interpol_mod::interpol_wind_meter"],
                ["interpol_mod::interpol_wind_meter", "interpol_mod::find_vert_vars"],
                ["interpol_mod::interpol_wind_meter", "interpol_mod::hor_interpol"],
                ["interpol_mod::interpol_wind_meter", "interpol_mod::vert_interpol"],
                [
                    "interpol_mod::interpol_wind_meter",
                    "interpol_mod::temporal_interpolation",
                ],
            ],
        },
        "generic_interface_resolution": {
            "interpol_mod::hor_interpol(4d_field,...)": "interpol_mod::hor_interpol_4d",
        },
        "production_note": (
            "All production edges above are direct CALL statements in the pinned "
            "source. init_interpol setup and adv_above_pbl/interpol_wind are sibling "
            "branches of advance, not one synthetic linear stack."
        ),
        "mapping": {
            "xlon0_deg": -2.0,
            "ylat0_deg": 48.0,
            "dx_deg": 0.25,
            "dy_deg": 0.25,
        },
    },
    "horizontal-periodic-wrap": {
        "coordinates": {
            "horizontal": "canonical cell-center grid-index coordinates xt/yt; periodic X adds a wrapped duplicate column",
            "vertical": "kz is a 1-based FLEXPART model-level index",
        },
        "staggering": {"horizontal": "cell_center", "vertical": "level_center"},
        "ordering": {"horizontal_storage": "x_fastest_then_y", "vertical": "single_level"},
        "units": {"horizontal_query": "grid_cell", "vertical_query": "index", "value": "arbitrary_scalar"},
        "time": {
            "kind": "instantaneous_static_for_fixture",
            "memory_slots": "same field copied to both FLEXPART memory slots",
        },
        "supported_query_domain": {
            "x": "0 <= xt < nx for periodic canonical grids; nx is the duplicate endpoint",
            "y": "0 <= yt <= ny-1",
            "out_of_domain": "not frozen by #71; downstream #72 fails closed",
        },
    },
    "vertical-model-levels": {
        "coordinates": {"horizontal": "not_applicable", "vertical": "metric height AGL"},
        "staggering": {"horizontal": "not_applicable", "vertical": "level_center"},
        "ordering": {"vertical": "bottom_to_top_increasing_height"},
        "units": {"vertical_query": "meter", "value": "arbitrary_scalar"},
        "time": {"kind": "not_applicable"},
    },
    "vertical-interface-wzlev": {
        "coordinates": {"horizontal": "not_applicable", "vertical": "FLEXPART wzlev metric height AGL"},
        "staggering": {"horizontal": "not_applicable", "vertical": "level_interface"},
        "ordering": {"vertical": "bottom_to_top_increasing_height"},
        "units": {"vertical_query": "meter", "value": "meter_per_second"},
        "time": {"kind": "not_applicable"},
    },
    "temporal-bilinear": {
        "coordinates": {"horizontal": "not_applicable", "vertical": "not_applicable"},
        "staggering": {"horizontal": "not_applicable", "vertical": "not_applicable"},
        "ordering": {"time": "memtime_1_then_memtime_2"},
        "units": {"time": "second", "value": "arbitrary_scalar"},
        "time": {
            "kind": "instantaneous_two_member_linear",
            "members": [0, 3600],
            "endpoints_inclusive": True,
            "primitive_outside_memory_window": "linear_extrapolation_no_range_guard",
            "production_lifecycle_direct_call_edges": [
                ["timemanager_mod::timemanager", "getfields_mod::getfields"],
                ["timemanager_mod::timemanager", "advance_mod::advance"],
            ],
            "production_temporal_direct_call_edges": [
                ["advance_mod::advance", "interpol_mod::init_interpol"],
                ["interpol_mod::init_interpol", "interpol_mod::find_time_vars"],
                ["advance_mod::advance", "advance_mod::adv_above_pbl"],
                ["advance_mod::adv_above_pbl", "interpol_mod::interpol_wind"],
                ["interpol_mod::interpol_wind", "interpol_mod::find_time_vars"],
                ["interpol_mod::interpol_wind", "interpol_mod::interpol_wind_meter"],
                [
                    "interpol_mod::interpol_wind_meter",
                    "interpol_mod::temporal_interpolation",
                ],
                ["advance_mod::advance", "advance_mod::petterssen_corr"],
                [
                    "advance_mod::petterssen_corr",
                    "interpol_mod::interpol_wind_short",
                ],
                [
                    "interpol_mod::interpol_wind_short",
                    "interpol_mod::find_time_vars",
                ],
                [
                    "interpol_mod::interpol_wind_short",
                    "interpol_mod::interpol_wind_meter",
                ],
            ],
            "range_policy_owner": (
                "caller/canonical API; Petterssen end-step guard is outside "
                "find_time_vars/temporal_interpolation"
            ),
        },
    },
    "rain-layer-fields": {
        "coordinates": {
            "horizontal": "canonical cell-center grid-index coordinates xt/yt",
            "vertical": "kz is the 1-based layer selector consumed by interpol_rain",
        },
        "staggering": {"horizontal": "cell_center", "vertical": "surface_or_layer_field"},
        "ordering": {
            "horizontal_storage": "x_fastest_then_y",
            "time": "field_time1_then_field_time2",
        },
        "units": {
            "time": "second",
            "lsprec": "millimeter_per_hour",
            "convprec": "millimeter_per_hour",
            "tcc": "dimensionless_fraction",
            "tt": "kelvin",
            "ctwc": "kilogram_per_kilogram",
            "cloud_bounds": "model_level_index_or_icmv",
        },
        "time": {
            "kind": "two_FLEXPART_memory_members",
            "members": [0, 3600],
            "precipitation_input_representation": "already_normalized_rate",
            "reset_deaccumulation": "not_performed_here; owned_by_issue_75",
        },
        "production_ingest_direct_call_edges": {
            "ecmwf": [
                ["timemanager_mod::timemanager", "getfields_mod::getfields"],
                ["getfields_mod::getfields", "windfields_mod::readwind_ecmwf"],
            ],
            "gfs": [
                ["timemanager_mod::timemanager", "getfields_mod::getfields"],
                ["getfields_mod::getfields", "windfields_mod::readwind_gfs"],
            ],
        },
        "ingest_output_fields": [
            "windfields_mod::lsprec",
            "windfields_mod::convprec",
        ],
        "production_sampling_direct_call_edges": [
            ["timemanager_mod::timemanager", "wetdepo_mod::wetdepo"],
            ["wetdepo_mod::wetdepo", "wetdepo_mod::get_wetscav"],
            ["wetdepo_mod::get_wetscav", "interpol_mod::find_ngrid"],
            ["wetdepo_mod::get_wetscav", "interpol_mod::find_grid_indices"],
            ["wetdepo_mod::get_wetscav", "interpol_mod::find_grid_distances"],
            ["wetdepo_mod::get_wetscav", "interpol_mod::find_z_level_meters"],
            ["wetdepo_mod::get_wetscav", "interpol_mod::interpol_rain"],
        ],
        "production_boundary_note": (
            "FLEXPART samples lsprec/convprec already stored in windfields_mod; "
            "source accumulation deaccumulation/reset normalization is upstream "
            "of this sampling boundary and owned by issue #75"
        ),
    },
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def normalize_json(value):
    """Normalize insignificant JSON representation differences for semantic hashing."""
    if isinstance(value, dict):
        return {key: normalize_json(item) for key, item in value.items()}
    if isinstance(value, list):
        return [normalize_json(item) for item in value]
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError("non-finite number cannot be part of the contract")
        if value.is_integer():
            return int(value)
    return value


def canonical_json_sha256(path: Path) -> str:
    """Hash normalized JSON so key order, whitespace and 0-vs-0.0 cannot drift provenance."""
    value = normalize_json(json.loads(path.read_text(encoding="utf-8")))
    canonical = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()


def load_oracle_build_metadata(binary: Path, manifest: dict) -> dict:
    """Capture the actual driver compiler plus the complete link-input set."""
    compiler_path = Path(str(binary) + ".compiler-version.txt")
    linked_objects_path = Path(str(binary) + ".linked-objects.txt")
    if not compiler_path.is_file():
        raise ValueError(f"missing oracle compiler provenance: {compiler_path}")
    if not linked_objects_path.is_file():
        raise ValueError(f"missing oracle linked-object provenance: {linked_objects_path}")

    compiler_version = compiler_path.read_text(encoding="utf-8").strip()
    linked_objects = [
        line.strip()
        for line in linked_objects_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    if not compiler_version:
        raise ValueError("empty oracle compiler provenance")
    if not linked_objects:
        raise ValueError("empty oracle linked-object provenance")
    if linked_objects != sorted(linked_objects) or len(linked_objects) != len(set(linked_objects)):
        raise ValueError("oracle linked-object provenance must be sorted and unique")
    if "FLEXPART.o" in linked_objects or not all(name.endswith(".o") for name in linked_objects):
        raise ValueError("invalid oracle linked-object set")

    object_set_bytes = ("\n".join(linked_objects) + "\n").encode("utf-8")
    profile = manifest["execution_profile"]
    return {
        "compiler_version": compiler_version,
        "full_flexpart_object_build": profile["build"],
        "container": profile["docker"],
        "driver_compile_link": {
            "compiler": "gfortran",
            "compile_flags": ["-O0", "-I<oracle-src>", "-fopenmp", "-mcmodel=large"],
            "driver": "scripts/interpolation/direct_interpolation_oracle.f90",
            "objects": "sorted src/*.o excluding FLEXPART.o",
            "link_flags": [
                "-L/usr/lib/x86_64-linux-gnu",
                "-Wl,-rpath=/usr/lib/x86_64-linux-gnu",
                "-leccodes",
                "-leccodes_f90",
                "-lm",
                "-lnetcdff",
            ],
        },
        "linked_objects": linked_objects,
        "linked_object_count": len(linked_objects),
        "linked_object_set_sha256": hashlib.sha256(object_set_bytes).hexdigest(),
    }


def validate_interface_oracle_source(path: Path) -> None:
    """Verify the #30 direct FLEXPART W/interface evidence used by #71."""
    case = CASES["vertical-interface-wzlev"]
    source = case["source_oracle"]
    if not path.is_file():
        raise ValueError(f"missing #30 vertical routine oracle output: {path}")
    actual_sha = sha256(path)
    if actual_sha != source["oracle_output_sha256"]:
        raise ValueError(
            f"#30 vertical routine oracle output hash {actual_sha} != "
            f"frozen {source['oracle_output_sha256']}"
        )

    lines = path.read_text(encoding="utf-8").splitlines()
    if len(lines) < 4 or lines[0] != "FLEXPART_VERTICAL_ROUTINE_ORACLE_V1":
        raise ValueError("invalid #30 vertical routine oracle header")
    nz = int(lines[1])
    pos = 2 + nz
    header = lines[pos].split()
    pos += 1
    if header != ["INTERFACES", str(nz + 1)]:
        raise ValueError("invalid #30 INTERFACES section")
    interface_heights_top_to_surface = []
    for expected in range(nz + 1):
        parts = lines[pos].split()
        pos += 1
        if len(parts) != 4 or int(parts[0]) != expected:
            raise ValueError("invalid #30 interface row")
        interface_heights_top_to_surface.append(float(parts[2]))

    motion_header = lines[pos].split()
    pos += 1
    if motion_header != ["MOTION", "1"]:
        raise ValueError("#30 interface fixture must contain normalized W motion")
    count = int(lines[pos])
    pos += 1
    if count != nz + 1:
        raise ValueError("#30 W-motion count mismatch")
    motion_top_to_surface = []
    for expected in range(count):
        parts = lines[pos].split()
        pos += 1
        if len(parts) != 3 or int(parts[0]) != expected:
            raise ValueError("invalid #30 W-motion row")
        motion_top_to_surface.append(float(parts[2]))
    if pos != len(lines):
        raise ValueError("unexpected trailing #30 oracle output")

    grid = list(case["grid"])
    nlevel = int(grid[0])
    heights = [float(v) for v in grid[1:1 + nlevel]]
    nvalues_pos = 1 + nlevel
    nvalues = int(grid[nvalues_pos])
    values = [float(v) for v in grid[nvalues_pos + 1:nvalues_pos + 1 + nvalues]]
    if nlevel != nz + 1 or nvalues != nlevel:
        raise ValueError("#71 interface fixture shape does not match #30 W geometry")

    oracle_heights = list(reversed(interface_heights_top_to_surface))
    oracle_values = list(reversed(motion_top_to_surface))
    if heights != oracle_heights:
        raise ValueError("#71 W/interface heights drifted from #30 FLEXPART wzlev output")
    if values != oracle_values:
        raise ValueError("#71 W/interface values drifted from #30 FLEXPART pinmconv output")


def build_real_vertical_sampling_case(path: Path) -> dict:
    """Build a real #29/#30 temperature-column case sampled by the #71 oracle."""
    repo_root = Path(__file__).resolve().parents[2]
    canonical_path = repo_root / "fixtures/meteorology/era5-etex-native-v1.json"
    provenance_path = (
        repo_root / "fixtures/meteorology/era5-etex-native-v1.provenance.json"
    )
    canonical = json.loads(canonical_path.read_text(encoding="utf-8"))
    source_provenance = json.loads(provenance_path.read_text(encoding="utf-8"))

    if not path.is_file():
        raise ValueError(f"missing real #30 vertical routine oracle output: {path}")
    lines = path.read_text(encoding="utf-8").splitlines()
    if len(lines) < 3 or lines[0] != "FLEXPART_VERTICAL_ROUTINE_ORACLE_V1":
        raise ValueError("invalid real #30 vertical routine oracle header")
    oracle_nz = int(lines[1])

    nx = canonical["horizontal_grid"]["nx"]
    ny = canonical["horizontal_grid"]["ny"]
    nz = len(canonical["vertical_coordinate"]["level_values"])
    if oracle_nz != nz:
        raise ValueError(
            f"real #30 oracle level count {oracle_nz} != canonical level count {nz}"
        )

    level_heights_top_to_bottom = []
    for expected in range(nz):
        parts = lines[2 + expected].split()
        if len(parts) != 4 or int(parts[0]) != expected:
            raise ValueError(f"invalid real #30 level row {expected}")
        level_heights_top_to_bottom.append(float(parts[2]))

    matches = [field for field in canonical["fields"] if field["id"] == "temperature"]
    if len(matches) != 1:
        raise ValueError("real canonical fixture must contain exactly one temperature field")
    temperature = matches[0]
    if temperature["shape"] != [nx, ny, nz]:
        raise ValueError("real canonical temperature shape drifted")

    x = 0
    y = 0
    values_top_to_bottom = [
        float(temperature["values"][x + nx * (y + ny * z)])
        for z in range(nz)
    ]
    heights = list(reversed(level_heights_top_to_bottom))
    values = list(reversed(values_top_to_bottom))
    if not all(math.isfinite(value) for value in heights + values):
        raise ValueError("real vertical sampling source contains non-finite values")
    if not all(upper > lower for lower, upper in zip(heights, heights[1:])):
        raise ValueError("real #30 AGL heights must increase bottom-to-top")

    # Freeze two boundaries, two strict interior interpolations and one exact
    # interior model level. The query heights are derived from the real #30
    # geometry rather than synthetic constants.
    query_heights = [
        heights[0],
        0.5 * (heights[20] + heights[21]),
        heights[nz // 2],
        0.5 * (heights[nz - 22] + heights[nz - 21]),
        heights[-1],
    ]
    fmt = lambda value: format(value, ".17g")
    horizontal = source_provenance["slice"]["horizontal"]
    timestamp = source_provenance["slice"]["timestamp"]
    epoch_seconds = source_provenance["slice"]["epoch_seconds"]

    return {
        "mode": "vertical",
        "vertical_staggering": "level_center",
        "source_oracle": {
            "producer_issue": 30,
            "producer_driver": "scripts/vertical/direct_oracle_driver.f90",
            "producer_routine": "verttransform_mod::verttransform_ecmwf_heights",
            "oracle_output_sha256": sha256(path),
            "geometry_field": "level height_agl_m",
            "value_field": "temperature",
            "value_source": "fixtures/meteorology/era5-etex-native-v1.json",
            "canonical_x": x,
            "canonical_y": y,
            "longitude_deg": horizontal["longitudes_deg"][x],
            "latitude_deg": horizontal["latitudes_deg"][y],
            "timestamp": timestamp,
            "epoch_seconds": epoch_seconds,
        },
        "grid": tuple(
            [str(nz)]
            + [fmt(value) for value in heights]
            + [str(nz)]
            + [fmt(value) for value in values]
        ),
        "queries": [str(len(query_heights))]
        + [f"0 {fmt(value)}" for value in query_heights],
        "semantics": {
            "coordinates": {
                "horizontal": (
                    "fixed real ERA5/ETEX cell-center column at canonical x=0,y=0"
                ),
                "vertical": "metric height AGL from pinned #30 FLEXPART oracle",
            },
            "staggering": {
                "horizontal": "cell_center",
                "vertical": "level_center",
            },
            "ordering": {
                "vertical_geometry": "bottom_to_top_increasing_height",
                "source_native_levels": "top_to_bottom_increasing_pressure",
            },
            "units": {
                "vertical_query": "meter",
                "value": temperature["unit"],
            },
            "time": {
                "kind": "instantaneous_valid_time",
                "timestamp": timestamp,
                "epoch_seconds": epoch_seconds,
            },
        },
    }


def build_real_data_sample(real_sampling_case: dict) -> dict:
    """Describe and verify the checked-in real #29/#30 ERA5/ETEX column source."""
    repo_root = Path(__file__).resolve().parents[2]
    canonical_path = repo_root / "fixtures/meteorology/era5-etex-native-v1.json"
    provenance_path = (
        repo_root / "fixtures/meteorology/era5-etex-native-v1.provenance.json"
    )
    surface_path = (
        repo_root / "fixtures/etex/native-mini/era5-surface-19941023-24.npz"
    )
    extraction_path = "scripts/vertical/extract_real_etex_column.py"

    canonical = json.loads(canonical_path.read_text(encoding="utf-8"))
    source_provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    expected_canonical_sha = source_provenance["artifact"]["sha256"]
    actual_canonical_sha = sha256(canonical_path)
    if actual_canonical_sha != expected_canonical_sha:
        raise ValueError(
            "real ERA5/ETEX canonical fixture hash does not match its #29 provenance"
        )

    expected_surface_sha = source_provenance["source"]["surface_archive"]["sha256"]
    actual_surface_sha = sha256(surface_path)
    if actual_surface_sha != expected_surface_sha:
        raise ValueError(
            "real ERA5/ETEX surface archive hash does not match its #29 provenance"
        )

    slice_meta = source_provenance["slice"]
    horizontal = slice_meta["horizontal"]
    native_levels = slice_meta["vertical"]["native_levels"]
    if canonical["horizontal_grid"]["nx"] < 1 or canonical["horizontal_grid"]["ny"] < 1:
        raise ValueError("real ERA5/ETEX fixture has no selectable column")
    if len(canonical["vertical_coordinate"]["level_values"]) != native_levels:
        raise ValueError("real ERA5/ETEX native-level count drifted")

    return {
        "id": "era5-etex-real-column-v1",
        "kind": "era5_etex_vertical_column",
        "source": {
            "canonical_fixture": "fixtures/meteorology/era5-etex-native-v1.json",
            "canonical_fixture_sha256": actual_canonical_sha,
            "canonical_provenance": (
                "fixtures/meteorology/era5-etex-native-v1.provenance.json"
            ),
            "surface_archive": (
                "fixtures/etex/native-mini/era5-surface-19941023-24.npz"
            ),
            "surface_archive_sha256": actual_surface_sha,
        },
        "selection": {
            "canonical_x": 0,
            "canonical_y": 0,
            "source_lon_index": horizontal["source_lon_indices"][0],
            "source_lat_desc_index": horizontal["source_lat_desc_indices"][0],
            "longitude_deg": horizontal["longitudes_deg"][0],
            "latitude_deg": horizontal["latitudes_deg"][0],
            "timestamp": slice_meta["timestamp"],
            "epoch_seconds": slice_meta["epoch_seconds"],
            "native_levels": native_levels,
        },
        "represented_fields": source_provenance["represented_fields"],
        "semantics": {
            "coordinates": {
                "horizontal": "longitude/latitude cell centers in degrees",
                "vertical": canonical["vertical_coordinate"]["kind"],
                "vertical_reference": canonical["vertical_coordinate"]["reference"],
            },
            "staggering": {
                field["id"]: {
                    "horizontal": field["horizontal_staggering"],
                    "vertical": field["vertical_staggering"],
                }
                for field in canonical["fields"]
                if field["id"] in source_provenance["represented_fields"]
            },
            "ordering": {
                "vertical": canonical["vertical_coordinate"]["ordering"],
                "storage": "x_fastest",
            },
            "units": {
                field["id"]: field["unit"]
                for field in canonical["fields"]
                if field["id"] in source_provenance["represented_fields"]
            },
            "time": {
                "kind": "instantaneous_valid_time",
                "timestamp": slice_meta["timestamp"],
                "epoch_seconds": slice_meta["epoch_seconds"],
            },
        },
        "compatibility": {
            "canonical_contract_issue": 29,
            "vertical_transform_issue": 30,
            "interpolation_contract_issue": 71,
            "interpolation_sampling_case": "real-era5-etex-temperature-column",
            "interpolation_geometry_oracle_sha256": (
                real_sampling_case["source_oracle"]["oracle_output_sha256"]
            ),
            "extraction_path": extraction_path,
            "extraction_source_sha256": sha256(repo_root / extraction_path),
            "ci_gate_step": "2b",
            "pinned_oracle_evidence": (
                "target/ci-gate/vertical-column/real-comparison-report.json"
            ),
            "fixture_provenance_evidence": (
                "target/ci-gate/vertical-column/real-column-fixture-provenance.json"
            ),
        },
        "scope": (
            "Checked-in real ERA5/ETEX source column used by #29 and transformed/"
            "validated by #30 against the pinned FLEXPART 11.1 direct routine oracle. "
            "#71 samples the real temperature profile on #30's pinned FLEXPART AGL "
            "geometry through find_z_level_meters/find_vert_vars/vert_interpol. "
            "Provider decoding and vertical-transform ownership remain in #29/#30."
        ),
    }


def write_input(case: dict, input_dir: Path) -> Path:
    lines = [case["mode"]]
    lines.extend(case["grid"])
    lines.extend(case.get("data", []))
    lines.extend(case["queries"])
    case_file = input_dir / f"{case['name']}.txt"
    case_file.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return case_file


def parse_output(text: str) -> dict:
    lines = text.splitlines()
    if not lines or lines[0] != ORACLE_OUTPUT_VERSION:
        raise ValueError(f"unexpected oracle output header: {lines[0]!r}")
    parsed: dict = {}
    block: dict = {}
    current_key: str | None = None

    def flush():
        nonlocal block, current_key
        if current_key is not None and block:
            parsed.setdefault("queries", []).append(block)
        block = {}
        current_key = None

    for line in lines[1:]:
        if line == "QUERY":
            flush()
            current_key = "query"
            continue
        parts = line.split(None, 1)
        if len(parts) == 2:
            key, rest = parts
            values: list = []
            for token in re.split(r"\s+", rest.strip()):
                if not token:
                    continue
                try:
                    values.append(float(token))
                except ValueError:
                    values.append(token)
            if current_key is None:
                parsed[key] = values[0] if len(values) == 1 else values
            elif key == "CLOUD":
                block[key] = values
            else:
                block[key] = values
    flush()
    return parsed


def run_oracle(binary: Path, case_file: Path, output_file: Path, offline: bool) -> None:
    if offline:
        if not output_file.is_file():
            raise ValueError(f"offline mode requires {output_file}")
        return
    output_file.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [str(binary), str(case_file), str(output_file)],
        check=True,
        capture_output=True,
        text=True,
    )
    if not output_file.is_file():
        raise ValueError(f"oracle did not produce {output_file}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--oracle-checkout", type=Path, required=True)
    parser.add_argument("--reference-manifest", type=Path, required=True)
    parser.add_argument("--out-fixture", type=Path, required=True)
    parser.add_argument("--out-provenance", type=Path, required=False)
    parser.add_argument("--vertical-routine-oracle-output", type=Path, required=False)
    parser.add_argument(
        "--real-vertical-routine-oracle-output", type=Path, required=True
    )
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--emit-inputs-only", action="store_true")
    args = parser.parse_args()

    if args.vertical_routine_oracle_output is not None:
        validate_interface_oracle_source(args.vertical_routine_oracle_output)

    cases = dict(CASES)
    real_sampling_case = build_real_vertical_sampling_case(
        args.real_vertical_routine_oracle_output
    )
    cases["real-era5-etex-temperature-column"] = real_sampling_case

    for name, case in cases.items():
        case = dict(case, name=name)
        input_dir = args.input_dir / name
        input_dir.mkdir(parents=True, exist_ok=True)
        write_input(case, input_dir)
    if args.emit_inputs_only:
        print(f"wrote case inputs to {args.input_dir}")
        return

    manifest = json.loads(args.reference_manifest.read_text(encoding="utf-8"))
    pinned = manifest["pinned_commit"]
    actual = subprocess.run(
        ["git", "-C", str(args.oracle_checkout), "rev-parse", "HEAD"],
        check=True, capture_output=True, text=True,
    ).stdout.strip()
    if actual != pinned:
        raise ValueError(f"oracle checkout {actual} != pinned {pinned}")
    if subprocess.run(
        ["git", "-C", str(args.oracle_checkout), "status", "--porcelain"],
        check=True, capture_output=True, text=True,
    ).stdout.strip():
        raise ValueError("oracle checkout is dirty")

    src = args.oracle_checkout / "src"
    for obj in (
        "com_mod.o", "par_mod.o", "point_mod.o", "windfields_mod.o", "interpol_mod.o",
    ):
        if not (src / obj).is_file():
            raise ValueError(f"missing pinned object file: {src / obj}")

    cases_out = []
    for name, case in cases.items():
        case = dict(case, name=name)
        case_file = args.input_dir / name / f"{name}.txt"
        output_file = args.output_dir / f"{name}.out"
        run_oracle(args.binary, case_file, output_file, args.offline)
        golden = parse_output(output_file.read_text(encoding="utf-8"))
        fixture_case = {
            "id": name,
            "mode": case["mode"],
            "input": case_file.read_text(encoding="utf-8").splitlines(),
            "golden": golden,
        }
        fixture_case["semantics"] = case.get("semantics", CASE_SEMANTICS.get(name))
        if fixture_case["semantics"] is None:
            raise ValueError(f"missing semantics for interpolation case {name}")
        for key in ("vertical_staggering", "source_oracle"):
            if key in case:
                fixture_case[key] = case[key]
        cases_out.append(fixture_case)

    real_data_samples = [build_real_data_sample(real_sampling_case)]
    fixture = {
        "schema": {"id": "flexpart-gpu.interpolation-contract", "version": 1},
        "pinned_flexpart": manifest,
        "oracle_output_version": ORACLE_OUTPUT_VERSION,
        "real_data_samples": real_data_samples,
        "cases": cases_out,
    }
    args.out_fixture.parent.mkdir(parents=True, exist_ok=True)
    args.out_fixture.write_text(
        json.dumps(fixture, indent=2) + "\n", encoding="utf-8"
    )

    if args.out_provenance is not None:
        symbols_probe = {
            "coordtrafo": "__point_mod_MOD_coordtrafo",
            "interpol_rain": "__interpol_mod_MOD_interpol_rain",
            "find_grid_indices": "__interpol_mod_MOD_find_grid_indices",
            "find_grid_distances": "__interpol_mod_MOD_find_grid_distances",
            "find_time_vars": "__interpol_mod_MOD_find_time_vars",
            "find_z_level_meters": "__interpol_mod_MOD_find_z_level_meters",
            "find_vert_vars": "__interpol_mod_MOD_find_vert_vars",
            "vert_interpol": "__interpol_mod_MOD_vert_interpol",
            "temporal_interpolation": "__interpol_mod_MOD_temporal_interpolation",
            "hor_interpol_4d": "__interpol_mod_MOD_hor_interpol_4d",
            "hor_interpol_2d": "__interpol_mod_MOD_hor_interpol_2d",
        }
        nm = subprocess.run(
            ["nm", str(args.binary)], check=True, capture_output=True, text=True
        ).stdout
        missing = [name for name, sym in symbols_probe.items() if sym not in nm]
        if missing:
            raise ValueError(f"oracle binary missing symbols: {missing}")
        repo_root = Path(__file__).resolve().parents[2]
        build_metadata = load_oracle_build_metadata(args.binary, manifest)
        provenance = {
            "schema": "flexpart-gpu.interpolation-contract-provenance.v1",
            "pinned_commit": pinned,
            "checkout_clean": True,
            "entrypoint_present": True,
            "oracle": {
                "name": manifest["name"],
                "version": manifest["version"],
                "pinned_commit": pinned,
            },
            "binary": {
                "path": str(args.binary),
                "sha256": sha256(args.binary),
            },
            "build": {
                "compiler_version": build_metadata["compiler_version"],
                "full_flexpart_object_build": build_metadata["full_flexpart_object_build"],
                "container": build_metadata["container"],
                "driver_compile_link": build_metadata["driver_compile_link"],
            },
            "fixture_artifact": {
                "path": "fixtures/interpolation/contract-v1.json",
                "hash_kind": "normalized_canonical_json_sha256",
                "sha256": canonical_json_sha256(args.out_fixture),
            },
            "generator_source": {
                "path": "scripts/interpolation/prepare_interpolation_fixtures.py",
                "sha256": sha256(Path(__file__).resolve()),
            },
            "driver_source": {
                "path": "scripts/interpolation/direct_interpolation_oracle.f90",
                "sha256": sha256(repo_root / "scripts/interpolation/direct_interpolation_oracle.f90"),
            },
            "oracle_harness_source": {
                "path": "scripts/interpolation/direct_oracle.sh",
                "sha256": sha256(repo_root / "scripts/interpolation/direct_oracle.sh"),
            },
            "real_extraction_source": {
                "path": "scripts/vertical/extract_real_etex_column.py",
                "sha256": sha256(repo_root / "scripts/vertical/extract_real_etex_column.py"),
            },
            "reference_manifest": {
                "path": "reference/flexpart-11.1.json",
                "sha256": sha256(args.reference_manifest),
            },
            "linked_flexpart": {
                "link_strategy": "all src/*.o except FLEXPART.o",
                "linked_objects": build_metadata["linked_objects"],
                "linked_object_count": build_metadata["linked_object_count"],
                "linked_object_set_sha256": build_metadata["linked_object_set_sha256"],
                "direct_routine_objects": [
                    {"file": "src/com_mod.f90", "object": "src/com_mod.o",
                     "source_sha256": sha256(src / "com_mod.f90"),
                     "object_sha256": sha256(src / "com_mod.o")},
                    {"file": "src/par_mod.f90", "object": "src/par_mod.o",
                     "source_sha256": sha256(src / "par_mod.f90"),
                     "object_sha256": sha256(src / "par_mod.o")},
                    {"file": "src/point_mod.f90", "object": "src/point_mod.o",
                     "source_sha256": sha256(src / "point_mod.f90"),
                     "object_sha256": sha256(src / "point_mod.o")},
                    {"file": "src/windfields_mod.f90", "object": "src/windfields_mod.o",
                     "source_sha256": sha256(src / "windfields_mod.f90"),
                     "object_sha256": sha256(src / "windfields_mod.o")},
                    {"file": "src/interpol_mod.f90", "object": "src/interpol_mod.o",
                     "source_sha256": sha256(src / "interpol_mod.f90"),
                     "object_sha256": sha256(src / "interpol_mod.o")},
                ],
                "routines": [
                    "coordtrafo", "find_grid_indices", "find_grid_distances",
                    "find_time_vars", "find_z_level_meters", "find_vert_vars",
                    "hor_interpol_4d", "hor_interpol_2d",
                    "temporal_interpolation", "vert_interpol", "interpol_rain",
                ],
            },
            "cases": {
                name: sha256(args.output_dir / f"{name}.out")
                for name in cases
            },
            "interface_vertical_source": CASES["vertical-interface-wzlev"]["source_oracle"],
            "real_vertical_source": real_sampling_case["source_oracle"],
            "real_data_samples": real_data_samples,
            "scope": (
                "The driver links the pristine pinned FLEXPART 11.1 interpolation "
                "modules. The geographic case intentionally composes "
                "point_mod::coordtrafo with the horizontal interpolation primitives "
                "as an oracle exercise path. Production behavior is crosswalked as "
                "direct CALL edges rather than synthetic linear stacks: release "
                "coordtrafo is called from read_options_and_initialise_flexpart; "
                "timemanager calls getfields and advance as siblings; advance calls "
                "init_interpol and adv_above_pbl; adv_above_pbl calls interpol_wind; "
                "interpol_wind performs its own grid/time/vertical setup and dispatches "
                "to interpol_wind_meter, which calls horizontal, vertical and temporal "
                "interpolation. Precipitation ingestion and wet-deposition sampling are "
                "likewise recorded as direct CALL edges, with lsprec/convprec named "
                "separately as data fields rather than pseudo-routines. "
                "W/interface sampling uses #30 direct FLEXPART wzlev/pinmconv output "
                "as its vertical geometry/value source. The real #29 ERA5/ETEX "
                "temperature column is paired with #30's pinned FLEXPART level-height "
                "output and sampled by the same #71 vertical routines. Golden values "
                "in contract-v1.json are direct interpolation-oracle outputs."
            ),
        }
        args.out_provenance.parent.mkdir(parents=True, exist_ok=True)
        args.out_provenance.write_text(
            json.dumps(provenance, indent=2) + "\n", encoding="utf-8"
        )

    print(f"wrote {args.out_fixture}")
    if args.out_provenance is not None:
        print(f"wrote {args.out_provenance}")


if __name__ == "__main__":
    main()