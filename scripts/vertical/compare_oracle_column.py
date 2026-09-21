#!/usr/bin/env python3
"""Compare #30 Rust vertical-column output with pinned FLEXPART evidence."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

PRESSURE_ABS_TOL_PA = 0.05
PRESSURE_REL_TOL = 1.0e-6
HEIGHT_ABS_TOL_M = 0.02
HEIGHT_REL_TOL = 1.0e-5
VERTICAL_VELOCITY_ABS_TOL_MS = 2.0e-5
VERTICAL_VELOCITY_REL_TOL = 1.0e-5

ORACLE_HEADERS = {
    "FLEXPART_VERTICAL_ROUTINE_ORACLE_V1": "pinned_routine",
    "FLEXPART_VERTICAL_CONFORMANCE_HARNESS_V1": "conformance_harness",
}


REQUIRED_VERTTRANSFORM_SNIPPETS = (
    "tvold=tt2_tmp(ix,jy)*(1.+0.378*ew(td2_tmp(ix,jy),ps_tmp(ix,jy))/",
    "pint=akz(kz)+bkz(kz)*ps_tmp(ix,jy)",
    "tv=tth_tmp(ix,jy,kz)*(1.+0.608*qvh_tmp(ix,jy,kz))",
    "if (abs(tv-tvold).gt.0.2) then",
    "log(pold/pint)*(tv-tvold)/log(tv/tvold)",
    "log(pold/pint)*tv",
    "wzlev(ix,jy,1)=0.",
    "wzlev(ix,jy,kz)=(uvzlev(ix,jy,kz+1)+uvzlev(ix,jy,kz))*0.5",
    "wzlev(ix,jy,nwz)=wzlev(ix,jy,nwz-1)+",
    "pinmconv(ix,jy,1)=(uvzlev(ix,jy,2))/",
    "pinmconv(ix,jy,kz)=(uvzlev(ix,jy,kz+1)-uvzlev(ix,jy,kz-1))/",
    "pinmconv(ix,jy,nz)=(uvzlev(ix,jy,nz)-uvzlev(ix,jy,nz-1))/",
    "ww(0:nxlim,0:nylim,1,n)=wwh(0:nxlim,0:nylim,1)*pinmconv(0:nxlim,0:nylim,1)",
)

REQUIRED_WINDFIELDS_SNIPPETS = (
    "akm(nwz-i+1)=zsec2(numskip+i)",
    "bkm(nwz-i+1)=zsec2(nlev_ec+1+numskip+i)",
    "akz(1)=0.",
    "bkz(1)=1.",
    "akz(i+1)=0.5*(akm(i+1)+akm(i))",
    "bkz(i+1)=0.5*(bkm(i+1)+bkm(i))",
    "nuvz=nuvz+1",
    "aknew(i)=akz(i)",
    "bknew(i)=bkz(i)",
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
    if len(lines) < 4 or lines[0] not in ORACLE_HEADERS:
        raise ValueError("invalid oracle output header")
    execution_mode = ORACLE_HEADERS[lines[0]]
    nz = int(lines[1])
    position = 2
    levels = []
    for expected_index in range(nz):
        if position >= len(lines):
            raise ValueError("oracle output ended before all level rows")
        parts = lines[position].split()
        position += 1
        if len(parts) != 4 or int(parts[0]) != expected_index:
            raise ValueError(f"invalid oracle output row {expected_index}")
        levels.append({
            "level": expected_index,
            "pressure_pa": float(parts[1]),
            "height_agl_m": float(parts[2]),
            "height_asl_m": float(parts[3]),
        })

    if position >= len(lines):
        raise ValueError("oracle output lacks INTERFACES section")
    interface_header = lines[position].split()
    position += 1
    if len(interface_header) != 2 or interface_header[0] != "INTERFACES":
        raise ValueError("invalid oracle INTERFACES header")
    interface_count = int(interface_header[1])
    if interface_count != nz + 1:
        raise ValueError("oracle interface height count mismatch")
    interfaces = []
    for expected_index in range(interface_count):
        if position >= len(lines):
            raise ValueError("oracle output ended before all interface rows")
        parts = lines[position].split()
        position += 1
        if len(parts) != 4 or int(parts[0]) != expected_index:
            raise ValueError(f"invalid oracle interface row {expected_index}")
        interfaces.append({
            "interface": expected_index,
            "pressure_pa": float(parts[1]),
            "height_agl_m": float(parts[2]),
            "height_asl_m": float(parts[3]),
        })

    if position >= len(lines):
        raise ValueError("oracle output lacks MOTION section")
    motion_header = lines[position].split()
    position += 1
    if len(motion_header) != 2 or motion_header[0] != "MOTION":
        raise ValueError("invalid oracle MOTION header")
    has_motion = int(motion_header[1])
    if has_motion not in (0, 1):
        raise ValueError("invalid oracle MOTION flag")

    motion = None
    if has_motion:
        if position >= len(lines):
            raise ValueError("oracle output lacks motion level count")
        count = int(lines[position])
        position += 1
        if count != nz + 1:
            raise ValueError("oracle interface-motion level count mismatch")
        motion = []
        for expected_index in range(count):
            if position >= len(lines):
                raise ValueError("oracle output ended before all motion rows")
            parts = lines[position].split()
            position += 1
            if len(parts) != 3 or int(parts[0]) != expected_index:
                raise ValueError(f"invalid oracle motion row {expected_index}")
            motion.append({
                "interface": expected_index,
                "omega_pa_s": float(parts[1]),
                "vertical_velocity_ms": float(parts[2]),
            })

    if position != len(lines):
        raise ValueError("unexpected trailing oracle output")
    return {
        "execution_mode": execution_mode,
        "levels": levels,
        "interfaces": interfaces,
        "motion": motion,
    }


def compare_scalar(actual, expected, abs_tol, rel_tol):
    passed, diff, limit = close(actual, expected, abs_tol, rel_tol)
    return {
        "candidate": actual,
        "oracle": expected,
        "absolute_difference": diff,
        "allowed_difference": limit,
        "pass": passed,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--oracle", type=Path, required=True)
    parser.add_argument("--oracle-checkout", type=Path, required=True)
    parser.add_argument("--reference-manifest", type=Path, required=True)
    parser.add_argument("--source-snapshot", type=Path, required=True)
    parser.add_argument("--source-motion", type=Path)
    parser.add_argument(
        "--expect-execution-mode",
        choices=sorted(set(ORACLE_HEADERS.values())),
        required=True,
    )
    parser.add_argument("--oracle-provenance", type=Path)
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
    if oracle["execution_mode"] != args.expect_execution_mode:
        raise ValueError(
            f"oracle execution mode {oracle['execution_mode']} != "
            f"expected {args.expect_execution_mode}"
        )
    if len(oracle["levels"]) != nz:
        raise ValueError("candidate/oracle vertical level count mismatch")

    rows = []
    overall = True
    for level, expected in enumerate(oracle["levels"]):
        comparisons = {
            "pressure_pa": compare_scalar(
                result["level_pressure_pa"][level],
                expected["pressure_pa"],
                PRESSURE_ABS_TOL_PA,
                PRESSURE_REL_TOL,
            ),
            "height_agl_m": compare_scalar(
                result["height_agl_m"][level],
                expected["height_agl_m"],
                HEIGHT_ABS_TOL_M,
                HEIGHT_REL_TOL,
            ),
            "height_asl_m": compare_scalar(
                result["height_asl_m"][level],
                expected["height_asl_m"],
                HEIGHT_ABS_TOL_M,
                HEIGHT_REL_TOL,
            ),
        }
        overall = overall and all(item["pass"] for item in comparisons.values())
        rows.append({"level": level, "fields": comparisons})

    candidate_interface_pressure = result.get("interface_pressure_pa")
    candidate_interface_agl = result.get("interface_height_agl_m")
    candidate_interface_asl = result.get("interface_height_asl_m")
    if not isinstance(candidate_interface_pressure, list) or len(candidate_interface_pressure) != nz + 1:
        raise ValueError("candidate interface pressure count mismatch")
    if not isinstance(candidate_interface_agl, list) or len(candidate_interface_agl) != nz + 1:
        raise ValueError("candidate interface AGL height count mismatch")
    if not isinstance(candidate_interface_asl, list) or len(candidate_interface_asl) != nz + 1:
        raise ValueError("candidate interface ASL height count mismatch")

    interface_rows = []
    for interface, expected in enumerate(oracle["interfaces"]):
        comparisons = {
            "pressure_pa": compare_scalar(
                candidate_interface_pressure[interface],
                expected["pressure_pa"],
                PRESSURE_ABS_TOL_PA,
                PRESSURE_REL_TOL,
            ),
            "height_agl_m": compare_scalar(
                candidate_interface_agl[interface],
                expected["height_agl_m"],
                HEIGHT_ABS_TOL_M,
                HEIGHT_REL_TOL,
            ),
            "height_asl_m": compare_scalar(
                candidate_interface_asl[interface],
                expected["height_asl_m"],
                HEIGHT_ABS_TOL_M,
                HEIGHT_REL_TOL,
            ),
        }
        overall = overall and all(item["pass"] for item in comparisons.values())
        interface_rows.append({"interface": interface, "fields": comparisons})

    candidate_motion = result.get("vertical_velocity")
    oracle_motion = oracle["motion"]
    if (candidate_motion is None) != (oracle_motion is None):
        raise ValueError("candidate/oracle motion presence differs")

    motion_rows = None
    if candidate_motion is not None:
        if candidate_motion.get("vertical_staggering") != "level_interface":
            raise ValueError("candidate oracle motion must retain level_interface staggering")
        candidate_values = candidate_motion.get("values_ms")
        if not isinstance(candidate_values, list) or len(candidate_values) != nz + 1:
            raise ValueError("candidate interface-motion value count mismatch")
        motion_rows = []
        for interface, expected in enumerate(oracle_motion):
            comparison = compare_scalar(
                candidate_values[interface],
                expected["vertical_velocity_ms"],
                VERTICAL_VELOCITY_ABS_TOL_MS,
                VERTICAL_VELOCITY_REL_TOL,
            )
            overall = overall and comparison["pass"]
            motion_rows.append({
                "interface": interface,
                "omega_pa_s": expected["omega_pa_s"],
                "vertical_velocity_ms": comparison,
            })

    source_motion = None
    if args.source_motion is not None:
        source_motion = {
            "path": str(args.source_motion),
            "sha256": sha256(args.source_motion),
        }
        if candidate_motion is None:
            raise ValueError("--source-motion was supplied but candidate contains no motion")
    elif candidate_motion is not None:
        raise ValueError("candidate contains motion but --source-motion provenance is missing")

    oracle_provenance = None
    if args.oracle_provenance is not None:
        parsed_provenance = json.loads(
            args.oracle_provenance.read_text(encoding="utf-8")
        )
        oracle_provenance = {
            "path": str(args.oracle_provenance),
            "sha256": sha256(args.oracle_provenance),
            "content": parsed_provenance,
        }
        if oracle["execution_mode"] == "pinned_routine":
            if parsed_provenance.get("pinned_commit") != pinned_commit:
                raise ValueError("routine provenance pinned commit mismatch")
            if parsed_provenance.get("checkout_clean") is not True:
                raise ValueError("routine provenance does not record a clean checkout")
            if parsed_provenance.get("source_path") != "src/verttransform_mod.f90":
                raise ValueError("routine provenance source path mismatch")
            if parsed_provenance.get("source_sha256") != sha256(verttransform):
                raise ValueError("routine provenance source hash mismatch")
            if parsed_provenance.get("routine") != "verttransform_ecmwf_heights":
                raise ValueError("routine provenance names the wrong FLEXPART routine")
    elif oracle["execution_mode"] == "pinned_routine":
        raise ValueError("pinned-routine oracle requires --oracle-provenance")

    scientific_scope = (
        "hybrid pressure, FLEXPART-11.1 hypsometric model-level heights"
        + (
            ", and pressure vertical velocity (omega) to geometric m/s via pinmconv"
            if candidate_motion is not None else ""
        )
    )
    report = {
        "schema": "flexpart-gpu.vertical-column-comparison.v1",
        "status": "PASS" if overall else "FAIL",
        "scientific_scope": scientific_scope + ", FLEXPART-11.1 W/interface geometric heights",
        "source_snapshot": {
            "path": str(args.source_snapshot),
            "sha256": sha256(args.source_snapshot),
        },
        "source_motion": source_motion,
        "candidate": {
            "report_path": str(args.candidate),
            "sha256": sha256(args.candidate),
        },
        "oracle": {
            "name": manifest["name"],
            "version": manifest["version"],
            "pinned_commit": pinned_commit,
            "checkout_clean": True,
            "execution_mode": oracle["execution_mode"],
            "verttransform_mod_sha256": sha256(verttransform),
            "windfields_mod_sha256": sha256(windfields),
            "qvsat_mod_sha256": sha256(qvsat),
            "output_path": str(args.oracle),
            "output_sha256": sha256(args.oracle),
            "routine_provenance": oracle_provenance,
            "source_contract_snippets_verified": True,
            "hybrid_level_construction_verified": True,
            "hybrid_interface_pressure_verified": True,
            "pinmconv_contract_verified": True,
            "wzlev_contract_verified": True,
            "note": (
                "Normative: the focused driver is linked against the object files "
                "from the pinned pristine full FLEXPART 11.1 build and calls "
                "verttransform_mod::verttransform_ecmwf_heights directly; "
                "windfields_mod supplies the routine's real module state."
                if oracle["execution_mode"] == "pinned_routine"
                else
                "Secondary conformance harness: independently replays the scalar "
                "column equations while linking pinned par_mod/qvsat_mod. It is not "
                "the normative FLEXPART oracle."
            ),
        },
        "tolerances": {
            "pressure": {"absolute_pa": PRESSURE_ABS_TOL_PA, "relative": PRESSURE_REL_TOL},
            "height": {"absolute_m": HEIGHT_ABS_TOL_M, "relative": HEIGHT_REL_TOL},
            "vertical_velocity": {
                "absolute_m_s": VERTICAL_VELOCITY_ABS_TOL_MS,
                "relative": VERTICAL_VELOCITY_REL_TOL,
            },
        },
        "levels": rows,
        "interfaces": interface_rows,
        "motion_interfaces": motion_rows,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if not overall:
        raise SystemExit("vertical column comparison failed")


if __name__ == "__main__":
    main()
