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
      --out-fixture fixtures/interpolation/contract-v1.json

Pass --offline (with pre-generated oracle-output files) to skip re-running the oracle.
"""

import argparse
import hashlib
import json
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
        "queries": ["2", "1.25 0.5 1", "3.1 2.0 1"],
    },
    "horizontal-periodic-wrap": {
        "mode": "horizontal",
        "grid": ("4 3 1", "0.0 0.0 1.0 1.0", "1"),
        "data": [
            "100.0", "200.0", "300.0", "400.0",
            "110.0", "210.0", "310.0", "410.0",
            "120.0", "220.0", "320.0", "420.0",
        ],
        "queries": ["3", "3.2 1.5 1", "3.9 0.5 1", "0.4 2.3 1"],
    },
    "vertical-model-levels": {
        "mode": "vertical",
        "grid": ("3", "10.0", "100.0", "1000.0", "3", "1.0", "2.0", "3.0"),
        "queries": [
            "6",
            "0 5.0", "0 50.0", "0 100.0", "0 1000.0", "0 2000.0", "1 500.0",
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


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


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
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--emit-inputs-only", action="store_true")
    args = parser.parse_args()

    for name, case in CASES.items():
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
        "com_mod.o", "par_mod.o", "windfields_mod.o", "interpol_mod.o",
    ):
        if not (src / obj).is_file():
            raise ValueError(f"missing pinned object file: {src / obj}")

    cases_out = []
    for name, case in CASES.items():
        case = dict(case, name=name)
        case_file = args.input_dir / name / f"{name}.txt"
        output_file = args.output_dir / f"{name}.out"
        run_oracle(args.binary, case_file, output_file, args.offline)
        golden = parse_output(output_file.read_text(encoding="utf-8"))
        cases_out.append(
            {
                "id": name,
                "mode": case["mode"],
                "input": case_file.read_text(encoding="utf-8").splitlines(),
                "golden": golden,
            }
        )

    fixture = {
        "schema": {"id": "flexpart-gpu.interpolation-contract", "version": 1},
        "pinned_flexpart": manifest,
        "oracle_output_version": ORACLE_OUTPUT_VERSION,
        "cases": cases_out,
    }
    args.out_fixture.parent.mkdir(parents=True, exist_ok=True)
    args.out_fixture.write_text(
        json.dumps(fixture, indent=2) + "\n", encoding="utf-8"
    )

    if args.out_provenance is not None:
        symbols_probe = {
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
            "driver_source": {
                "path": "scripts/interpolation/direct_interpolation_oracle.f90",
                "sha256": sha256(
                    Path(__file__).resolve().parents[1] / "interpolation"
                    / "direct_interpolation_oracle.f90"
                ),
            },
            "linked_flexpart": {
                "objects": [
                    {"file": "src/com_mod.f90", "object": "src/com_mod.o",
                     "source_sha256": sha256(src / "com_mod.f90"),
                     "object_sha256": sha256(src / "com_mod.o")},
                    {"file": "src/par_mod.f90", "object": "src/par_mod.o",
                     "source_sha256": sha256(src / "par_mod.f90"),
                     "object_sha256": sha256(src / "par_mod.o")},
                    {"file": "src/windfields_mod.f90", "object": "src/windfields_mod.o",
                     "source_sha256": sha256(src / "windfields_mod.f90"),
                     "object_sha256": sha256(src / "windfields_mod.o")},
                    {"file": "src/interpol_mod.f90", "object": "src/interpol_mod.o",
                     "source_sha256": sha256(src / "interpol_mod.f90"),
                     "object_sha256": sha256(src / "interpol_mod.o")},
                ],
                "routines": [
                    "find_grid_indices", "find_grid_distances",
                    "find_time_vars", "find_z_level_meters", "find_vert_vars",
                    "hor_interpol_4d", "hor_interpol_2d",
                    "temporal_interpolation", "vert_interpol", "interpol_rain",
                ],
            },
            "cases": {
                name: sha256(args.output_dir / f"{name}.out")
                for name in CASES
            },
            "scope": (
                "The driver links the pristine pinned FLEXPART 11.1 interpolation "
                "modules and calls find_grid_indices/find_grid_distances/"
                "find_z_level_meters/find_vert_vars/hor_interpol_4d/"
                "temporal_interpolation/vert_interpol/interpol_rain directly on "
                "canonical synthetic grids. Golden values in contract-v1.json are "
                "the direct oracle outputs."
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