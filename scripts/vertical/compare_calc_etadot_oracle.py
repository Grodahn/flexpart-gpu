#!/usr/bin/env python3
"""Compare candidate eta-dot->Pa/s output with the pinned flex_extract oracle.

Host/container-agnostic script for the calc_etadot oracle tier (#70). It
consumes:

  * the oracle extracted by extract_calc_etadot_oracle.py (oracle.json),
  * the candidate report produced by the eta-dot-column-report binary,
  * the pinned, clean flex_extract checkout that produced the oracle,

cross-checks that the checkout is exactly the pinned commit, that
calc_etadot.f90 hashes to the pinned fingerprint, that the ETAR transform
block still matches the harness contract and that the example namelist still
selects the oracle configuration (META=1, METADIFF=0, MOMEGA=0, MDPDETA=1),
then compares every point x level value with f32-vs-f64 pilot tolerances and
writes a machine-readable comparison report.

Candidate indexing contract (validated against the oracle):
  * candidate interface index K == calc_etadot level K (both top-first);
  * values_interface_pa_s is interface-major, point minor with x-fastest
    (interface index 0 = top of atmosphere);
  * the W-interface value at level K is read in [K-1].-(..)-[K] order from the
    canonical increasing-pressure hybrid description, i.e. value[K].
"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

PRESSURE_VELOCITY_ABS_TOL_PA_S = 1.0e-7
PRESSURE_VELOCITY_REL_TOL = 3.0e-5
MAX_ATTRIBUTED_REL_ERROR = 3.0e-5

PINNED_CALC_ETADOT_SHA256 = "07ED3522F8C1B35065965D01AF828F7532605A3AA9BE44D48FB9CA3F2ED976FF"

REQUIRED_TRANSFORM_SNIPPETS = (
    "P00=101325.",
    "CALL READLATLON(FILENAME,ETAR,MAXL,MAXB,MLEVEL,(/77/))",
    "DAK=AK(K+1)-AK(K)",
    "DBK=BK(K+1)-BK(K)",
    "ETAR(I,J,K)=2*ETAR(I,J,K)*PS(I,J,1)*(DAK/PS(I,J,1)+DBK)/",
    "(DAK/P00+DBK)",
    "IF (K .GT. 1) ETAR(I,J,K)=ETAR(I,J,K)-ETAR(I,J,K-1)",
)

EXPECTED_NAMGEN = {
    "maxl": 6,
    "maxb": 6,
    "mlevel": 91,
    "mnauf": 106,
    "metapar": 77,
    "momega": 0,
    "momegadiff": 0,
    "mgauss": 0,
    "msmooth": 0,
    "meta": 1,
    "metadiff": 0,
    "mdpdeta": 1,
}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(checkout, *args):
    return subprocess.run(
        ["git", "-C", str(checkout), *args],
        check=True, capture_output=True, text=True
    ).stdout.strip()


def parse_namgen(path):
    text = path.read_text(encoding="utf-8")
    result = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("&") or line.startswith("/") or line.startswith("!"):
            continue
        if "=" not in line:
            continue
        key, _, value = line.partition("=")
        key = key.strip()
        value = value.strip().rstrip(",").strip()
        if value.startswith('"'):
            result[key] = value.strip('"')
        elif "." in value:
            result[key] = float(value)
        else:
            result[key] = int(value)
    return result


def validate_checkout(checkout, manifest):
    pinned_commit = manifest["pinned_commit"]
    actual = git(checkout, "rev-parse", "HEAD")
    if actual != pinned_commit:
        raise ValueError(f"oracle checkout {actual} != pinned {pinned_commit}")
    if git(checkout, "status", "--porcelain"):
        raise ValueError("oracle checkout is dirty")

    calc_etadot = checkout / "Source" / "Fortran" / "calc_etadot.f90"
    if not calc_etadot.is_file():
        raise ValueError(f"missing oracle source {calc_etadot}")
    digest = sha256(calc_etadot)
    if digest.lower() != PINNED_CALC_ETADOT_SHA256.lower():
        raise ValueError(
            f"calc_etadot.f90 sha256 {digest} != pinned "
            f"{PINNED_CALC_ETADOT_SHA256}"
        )
    source_text = calc_etadot.read_text(encoding="utf-8")
    missing = [
        snippet for snippet in REQUIRED_TRANSFORM_SNIPPETS
        if snippet not in source_text
    ]
    if missing:
        raise ValueError(
            "pinned calc_etadot source no longer matches the ETAR transform "
            "contract: " + repr(missing)
        )

    fort4 = checkout / "Testing" / "Installation" / "Calc_etadot" / "fort.4"
    if not fort4.is_file():
        raise ValueError(f"missing example namelist {fort4}")
    namgen = parse_namgen(fort4)
    for key, expected in EXPECTED_NAMGEN.items():
        actual_value = namgen.get(key)
        if isinstance(expected, str):
            match = actual_value == expected
        else:
            match = (actual_value is not None
                     and abs(float(actual_value) - expected) < 1e-9)
        if not match:
            raise ValueError(
                f"example namelist no longer selects the oracle configuration: "
                f"{key}={actual_value!r} expected {expected!r}"
            )
    return calc_etadot


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--oracle", type=Path, required=True)
    parser.add_argument("--flex-extract-checkout", type=Path, required=True)
    parser.add_argument("--reference-manifest", type=Path, required=True)
    parser.add_argument("--source-snapshot", type=Path, required=True)
    parser.add_argument("--source-motion", type=Path, required=True)
    parser.add_argument("--run-provenance", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    manifest = json.loads(args.reference_manifest.read_text(encoding="utf-8"))
    calc_etadot = validate_checkout(args.flex_extract_checkout, manifest)

    candidate = json.loads(args.candidate.read_text(encoding="utf-8"))
    if candidate.get("schema", {}).get("id") != "flexpart-gpu.eta-dot-column-report":
        raise ValueError("candidate report has the wrong schema")
    source_snapshot_sha = sha256(args.source_snapshot)
    source_motion_sha = sha256(args.source_motion)

    oracle = json.loads(args.oracle.read_text(encoding="utf-8"))
    source_fixture = oracle.get("source_fixture", {})
    if source_fixture.get("classification") != "upstream_flex_extract_native_model_level":
        raise ValueError("oracle fixture is not identified as the pinned native-model-level source")
    raw_metadata = source_fixture.get("raw_grib_metadata", {})
    if raw_metadata.get("paramId") != 77:
        raise ValueError(f"oracle raw GRIB is not parameter 77: {raw_metadata!r}")

    run_provenance = json.loads(args.run_provenance.read_text(encoding="utf-8"))
    if run_provenance.get("schema") != "flexpart-gpu.etadot-oracle-run-provenance.v1":
        raise ValueError("run provenance has the wrong schema")
    image_id = run_provenance.get("docker_image", {}).get("id")
    compiler_version = run_provenance.get("compiler", {}).get("version")
    executable_hash = run_provenance.get("oracle_executable", {}).get("sha256")
    if not image_id or not compiler_version or not executable_hash:
        raise ValueError("run provenance lacks concrete build identity")

    fixture_hashes = source_fixture.get("source_hashes_sha256", {})
    run_inputs = run_provenance.get("oracle_inputs", {})
    run_outputs = run_provenance.get("oracle_outputs", {})
    for name in ("fort.12", "fort.21"):
        if fixture_hashes.get(name) != run_inputs.get(name, {}).get("sha256"):
            raise ValueError(f"{name} hash differs between extracted fixture and run provenance")
    if fixture_hashes.get("fort.15") != run_outputs.get("fort.15", {}).get("sha256"):
        raise ValueError("fort.15 hash differs between extracted oracle and run provenance")

    nlev = oracle["nlev"]
    levels = oracle["levels_present"]
    nx, ny = oracle["nx"], oracle["ny"]
    nxy = nx * ny

    values = candidate["result"]["values_interface_pa_s"]
    if len(values) != (nlev + 1) * nxy:
        raise ValueError(
            f"candidate interface value count {len(values)} != {(nlev + 1) * nxy}"
        )

    per_level = {}
    row_overall = True
    for lvl in levels:
        k = lvl
        search = oracle["oracle_etadot_pa_s_xy_by_level"][str(lvl)]
        comparisons = []
        level_overall = True
        for y in range(ny):
            for x in range(nx):
                point = y * nx + x
                actual = values[k * nxy + point]
                expected = search[point]
                diff = abs(actual - expected)
                limit = max(
                    PRESSURE_VELOCITY_ABS_TOL_PA_S,
                    PRESSURE_VELOCITY_REL_TOL * abs(expected),
                )
                passed = diff <= limit
                level_overall = level_overall and passed
                comparisons.append({
                    "x": x,
                    "y": y,
                    "candidate_pa_s": actual,
                    "oracle_pa_s": expected,
                    "absolute_difference_pa_s": diff,
                    "allowed_difference_pa_s": limit,
                    "pass": passed,
                })
        per_level[str(lvl)] = {"comparisons": comparisons, "pass": level_overall}
        row_overall = row_overall and level_overall

    report = {
        "schema": "flexpart-gpu.etadot-oracle-field-comparison.v1",
        "status": "PASS" if row_overall else "FAIL",
        "oracle": {
            "name": manifest["name"],
            "version": manifest["version"],
            "pinned_commit": manifest["pinned_commit"],
            "checkout_clean": True,
            "calc_etadot_f90_sha256": PINNED_CALC_ETADOT_SHA256,
            "source_contract_snippets_verified": True,
            "namelist_configuration_verified": True,
        },
        "candidate": {
            "report_path": str(args.candidate),
            "sha256": sha256(args.candidate),
        },
        "source_fixture": source_fixture,
        "run_provenance": {
            "path": str(args.run_provenance),
            "sha256": sha256(args.run_provenance),
            "build_identity": {
                "docker_image": run_provenance["docker_image"],
                "compiler": run_provenance["compiler"],
                "oracle_executable": run_provenance["oracle_executable"],
            },
            "oracle_inputs": run_provenance["oracle_inputs"],
            "oracle_outputs": run_provenance["oracle_outputs"],
        },
        "inputs": {
            "snapshot": {"path": str(args.source_snapshot), "sha256": source_snapshot_sha},
            "motion": {"path": str(args.source_motion), "sha256": source_motion_sha},
            "calc_etadot_f90": {"path": str(calc_etadot), "sha256": sha256(calc_etadot)},
        },
        "tolerances": {
            "pressure_velocity": {
                "absolute_pa_s": PRESSURE_VELOCITY_ABS_TOL_PA_S,
                "relative": PRESSURE_VELOCITY_REL_TOL,
            },
            "note": (
                "Candidate arithmetic is f32; the pinned calc_etadot builds "
                f"with -fdefault-real-8 (f64). Worst observed relative error "
                f"is the hybrid-coordinate rounding floor; the attributable "
                f"cap of {MAX_ATTRIBUTED_REL_ERROR} admits 3x headroom over it."
            ),
        },
        "grid": {"nx": nx, "ny": ny, "nlev": nlev, "levels": levels},
        "levels": per_level,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if not row_overall:
        raise SystemExit("eta-dot oracle field comparison failed")


if __name__ == "__main__":
    main()