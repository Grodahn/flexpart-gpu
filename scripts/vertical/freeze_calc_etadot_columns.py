#!/usr/bin/env python3
"""Freeze compact columns from passing fresh calc_etadot gate artifacts."""

import argparse
import json
from pathlib import Path

from compare_calc_etadot_oracle import sha256, verify_artifact

CASES = ("real-era5-137", "synthetic-72000", "synthetic-101325")


def read(path):
    return json.loads(path.read_bytes())


def freeze_case(root, name):
    run = root / name
    report = read(run / "comparison-report.json")
    provenance = read(run / "run-provenance.json")
    if (report["status"] != "PASS" or report["coverage"]["comparisons"] != 137 * 36
            or report["run_provenance"]["sha256"] != sha256(run / "run-provenance.json")):
        raise ValueError(f"missing passing full-column comparison: {name}")
    for record in (*provenance["oracle_inputs"].values(), *provenance["oracle_outputs"].values()):
        verify_artifact(record)
    snapshot_path = run / "oracle-json/snapshot.json"
    motion_path = run / "oracle-json/motion.json"
    if any(report["inputs"][key]["sha256"] != sha256(path)
           for key, path in (("snapshot", snapshot_path), ("motion", motion_path))):
        raise ValueError(f"column input hashes changed: {name}")
    coordinate = read(snapshot_path)["vertical_coordinate"]
    oracle_path = run / "oracle-json/oracle.json"
    if report["inputs"]["decoded_oracle"]["sha256"] != sha256(oracle_path):
        raise ValueError(f"decoded oracle hash changed: {name}")
    oracle = read(oracle_path)
    source = oracle["source_fixture"]["source_provenance"]["content"]
    values = oracle["oracle_etadot_pa_s_xy_by_level"]
    return coordinate, {
        "id": name,
        "pressure_source": source["pressure_source"],
        "t0_unpacked_precision_bits": source["oracle_grid"]["t0_unpacked_precision_bits"],
        "source_pressure_pa": source["selected_column"]["surface_pressure_pa"],
        "surface_pressure_pa": oracle["candidate_surface_pressure_pa_xy"][0],
        "fort15_surface_pressure_pa": oracle["surface_pressure_pa_xy"][0],
        "eta_dot_per_s": read(motion_path)["values"][::36],
        "oracle_below_level_pa_s": [values[str(level)][0] for level in range(1, 138)],
        "max_absolute_difference_pa_s": report["coverage"]["max_absolute_difference_pa_s"],
        "max_relative_difference_for_nonzero_reference": report["coverage"]["max_relative_difference"],
        "max_tolerance_fraction": report["coverage"]["max_tolerance_fraction"],
        "compared_values": report["coverage"]["comparisons"],
        "sources_sha256": {key: value["sha256"] for key, value in source["sources"].items()},
        "oracle_inputs_sha256": {key: value["sha256"] for key, value in provenance["oracle_inputs"].items()},
        "fort15_sha256": provenance["oracle_outputs"]["fort.15"]["sha256"],
        "decoded_oracle_sha256": sha256(oracle_path),
    }, report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_root", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    coordinate, _, report = freeze_case(args.run_root, CASES[0])
    keys = ("hybrid_a_interface_pa", "hybrid_b_interface")
    columns = []
    for name in CASES:
        case_coordinate, column, case_report = freeze_case(args.run_root, name)
        if (any(case_coordinate[key] != coordinate[key] for key in keys)
                or case_report["oracle"] != report["oracle"]
                or case_report["run_provenance"]["build_identity"] != report["run_provenance"]["build_identity"]):
            raise ValueError(f"oracle identity or L137 coordinate differs: {name}")
        columns.append(column)
    build = report["run_provenance"]["build_identity"]
    fixture = {
        "schema": "flexpart-gpu.calc-etadot-l137-columns.v1",
        "oracle": report["oracle"],
        "build": {
            "image": build["docker_image"],
            "compiler": build["compiler"],
            "executable_sha256": build["oracle_executable"]["sha256"],
        },
        "tolerances": report["tolerances"]["pressure_velocity"],
        **{key: coordinate[key] for key in keys},
        "columns": columns,
    }
    args.output.write_text(json.dumps(fixture, indent=2, allow_nan=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
