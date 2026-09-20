#!/usr/bin/env python3
"""Compare #30 Rust vertical-column output with a pinned FLEXPART 11.1 harness."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

PRESSURE_ABS_TOL_PA = 0.05
PRESSURE_REL_TOL = 1.0e-6
HEIGHT_ABS_TOL_M = 0.02
HEIGHT_REL_TOL = 1.0e-5

REQUIRED_VERTTRANSFORM_SNIPPETS = (
    "tvold=tt2_tmp(ix,jy)*(1.+0.378*ew(td2_tmp(ix,jy),ps_tmp(ix,jy))/",
    "pint=akz(kz)+bkz(kz)*ps_tmp(ix,jy)",
    "tv=tth_tmp(ix,jy,kz)*(1.+0.608*qvh_tmp(ix,jy,kz))",
    "if (abs(tv-tvold).gt.0.2) then",
    "log(pold/pint)*(tv-tvold)/log(tv/tvold)",
    "log(pold/pint)*tv",
)

REQUIRED_WINDFIELDS_SNIPPETS = (
    "akz(1)=0.",
    "bkz(1)=1.",
    "akz(i+1)=0.5*(akm(i+1)+akm(i))",
    "bkz(i+1)=0.5*(bkm(i+1)+bkm(i))",
    "nuvz=nuvz+1",
)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(checkout, *args):
    return subprocess.run(
        ["git", "-C", str(checkout), *args],
        check=True, capture_output=True, text=True
    ).stdout.strip()


def close(actual, expected, abs_tol, rel_tol):
    diff = abs(actual - expected)
    limit = max(abs_tol, rel_tol * abs(expected))
    return diff <= limit, diff, limit


def read_oracle(path):
    lines = path.read_text(encoding="utf-8").splitlines()
    if len(lines) < 3 or lines[0] != "FLEXPART_VERTICAL_COLUMN_ORACLE_V1":
        raise ValueError("invalid oracle output header")
    nz = int(lines[1])
    if len(lines) != nz + 2:
        raise ValueError("oracle output level count mismatch")
    result = []
    for expected_index, line in enumerate(lines[2:]):
        parts = line.split()
        if len(parts) != 4 or int(parts[0]) != expected_index:
            raise ValueError(f"invalid oracle output row {expected_index}")
        result.append({
            "level": expected_index,
            "pressure_pa": float(parts[1]),
            "height_agl_m": float(parts[2]),
            "height_asl_m": float(parts[3]),
        })
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--oracle", type=Path, required=True)
    parser.add_argument("--oracle-checkout", type=Path, required=True)
    parser.add_argument("--reference-manifest", type=Path, required=True)
    parser.add_argument("--source-snapshot", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    manifest = json.loads(args.reference_manifest.read_text(encoding="utf-8"))
    pinned_commit = manifest["pinned_commit"]
    actual_commit = git(args.oracle_checkout, "rev-parse", "HEAD")
    if actual_commit != pinned_commit:
        raise ValueError(f"oracle checkout {actual_commit} != pinned {pinned_commit}")
    if git(args.oracle_checkout, "status", "--porcelain"):
        raise ValueError("oracle checkout is dirty")

    verttransform = args.oracle_checkout / "src" / "verttransform_mod.f90"
    windfields = args.oracle_checkout / "src" / "windfields_mod.f90"
    qvsat = args.oracle_checkout / "src" / "qvsat_mod.f90"
    for path in (verttransform, windfields, qvsat):
        if not path.is_file():
            raise ValueError(f"missing oracle source {path}")
    source_text = verttransform.read_text(encoding="utf-8")
    missing = [snippet for snippet in REQUIRED_VERTTRANSFORM_SNIPPETS
               if snippet not in source_text]
    if missing:
        raise ValueError(
            "pinned verttransform source no longer matches the harness contract: "
            + repr(missing)
        )
    windfields_text = windfields.read_text(encoding="utf-8")
    missing = [snippet for snippet in REQUIRED_WINDFIELDS_SNIPPETS
               if snippet not in windfields_text]
    if missing:
        raise ValueError(
            "pinned windfields source no longer matches the hybrid-level contract: "
            + repr(missing)
        )

    candidate = json.loads(args.candidate.read_text(encoding="utf-8"))
    result = candidate["result"]
    nx, ny, nz = result["nx"], result["ny"], result["nz"]
    if (nx, ny) != (1, 1):
        raise ValueError("candidate report is not a 1x1 column")
    oracle = read_oracle(args.oracle)
    if len(oracle) != nz:
        raise ValueError("candidate/oracle vertical level count mismatch")

    rows = []
    overall = True
    for level, expected in enumerate(oracle):
        actual_values = {
            "pressure_pa": result["level_pressure_pa"][level],
            "height_agl_m": result["height_agl_m"][level],
            "height_asl_m": result["height_asl_m"][level],
        }
        comparisons = {}
        for field_name, actual in actual_values.items():
            expected_value = expected[field_name]
            if field_name == "pressure_pa":
                abs_tol, rel_tol = PRESSURE_ABS_TOL_PA, PRESSURE_REL_TOL
            else:
                abs_tol, rel_tol = HEIGHT_ABS_TOL_M, HEIGHT_REL_TOL
            passed, diff, limit = close(actual, expected_value, abs_tol, rel_tol)
            overall = overall and passed
            comparisons[field_name] = {
                "candidate": actual,
                "oracle": expected_value,
                "absolute_difference": diff,
                "allowed_difference": limit,
                "pass": passed,
            }
        rows.append({"level": level, "fields": comparisons})

    report = {
        "schema": "flexpart-gpu.vertical-column-comparison.v1",
        "status": "PASS" if overall else "FAIL",
        "scientific_scope": "hybrid pressure and FLEXPART-11.1 hypsometric model-level heights",
        "source_snapshot": {
            "path": str(args.source_snapshot),
            "sha256": sha256(args.source_snapshot),
        },
        "candidate": {
            "report_path": str(args.candidate),
            "sha256": sha256(args.candidate),
        },
        "oracle": {
            "name": manifest["name"],
            "version": manifest["version"],
            "pinned_commit": pinned_commit,
            "checkout_clean": True,
            "verttransform_mod_sha256": sha256(verttransform),
            "windfields_mod_sha256": sha256(windfields),
            "qvsat_mod_sha256": sha256(qvsat),
            "harness_output_path": str(args.oracle),
            "harness_output_sha256": sha256(args.oracle),
            "source_contract_snippets_verified": True,
            "hybrid_level_construction_verified": True,
            "note": (
                "The harness links the pinned oracle par_mod/qvsat_mod directly and "
                "replays the scalar column loop from verttransform_ecmwf_heights; "
                "the pristine oracle checkout is not modified."
            ),
        },
        "tolerances": {
            "pressure": {"absolute_pa": PRESSURE_ABS_TOL_PA, "relative": PRESSURE_REL_TOL},
            "height": {"absolute_m": HEIGHT_ABS_TOL_M, "relative": HEIGHT_REL_TOL},
        },
        "levels": rows,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if not overall:
        raise SystemExit("vertical column comparison failed")


if __name__ == "__main__":
    main()
