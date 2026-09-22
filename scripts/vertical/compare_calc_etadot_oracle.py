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

PINNED_CALC_ETADOT_SHA256 = "160F267F8741F23D13FDBA2F7A88F110BB131AA84AD7894FA43605258E55B0D9"
PINNED_CALC_ETADOT_GIT_BLOB = "741eba91eab049df23a560219d0f2656a6cc9881"
REAL_ERA5_CLASSIFICATION = "real_era5_native_model_level_full_column"

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


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


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

    source_ref = f"{pinned_commit}:Source/Fortran/calc_etadot.f90"
    source_blob = git(checkout, "rev-parse", source_ref)
    if source_blob.lower() != PINNED_CALC_ETADOT_GIT_BLOB.lower():
        raise ValueError(
            f"calc_etadot.f90 git blob {source_blob} != pinned "
            f"{PINNED_CALC_ETADOT_GIT_BLOB}"
        )
    source_bytes = subprocess.run(
        ["git", "-C", str(checkout), "show", source_ref],
        check=True, capture_output=True
    ).stdout
    digest = sha256_bytes(source_bytes)
    if digest.lower() != PINNED_CALC_ETADOT_SHA256.lower():
        raise ValueError(
            f"canonical calc_etadot.f90 sha256 {digest} != pinned "
            f"{PINNED_CALC_ETADOT_SHA256}"
        )
    source_text = source_bytes.decode("utf-8")
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
    return calc_etadot, source_blob, digest


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
    calc_etadot, source_blob, calc_etadot_digest = validate_checkout(
        args.flex_extract_checkout, manifest
    )

    candidate = json.loads(args.candidate.read_text(encoding="utf-8"))
    if candidate.get("schema", {}).get("id") != "flexpart-gpu.eta-dot-column-report":
        raise ValueError("candidate report has the wrong schema")
    source_snapshot_sha = sha256(args.source_snapshot)
    source_motion_sha = sha256(args.source_motion)

    oracle = json.loads(args.oracle.read_text(encoding="utf-8"))
    source_fixture = oracle.get("source_fixture", {})
    classification = source_fixture.get("classification")
    if classification not in {
        "upstream_flex_extract_native_model_level",
        REAL_ERA5_CLASSIFICATION,
    }:
        raise ValueError(
            f"oracle fixture has unsupported classification: {classification!r}"
        )
    raw_metadata = source_fixture.get("raw_grib_metadata", {})
    if raw_metadata.get("paramId") != 77:
        raise ValueError(f"oracle raw GRIB is not parameter 77: {raw_metadata!r}")
    if classification == REAL_ERA5_CLASSIFICATION:
        if raw_metadata.get("typeOfLevel") != "hybrid":
            raise ValueError(
                f"real ERA5 eta-dot is not on native hybrid levels: {raw_metadata!r}"
            )
        coverage = source_fixture.get("level_coverage", {})
        if (
            coverage.get("first") != 1
            or coverage.get("last") != 137
            or coverage.get("count") != 137
            or coverage.get("native_level_count") != 137
            or coverage.get("complete_native_column") is not True
        ):
            raise ValueError(
                f"real ERA5 oracle does not cover the complete 137-level column: {coverage!r}"
            )
        source_provenance = source_fixture.get("source_provenance", {}).get("content", {})
        if source_provenance.get("schema") != "flexpart-gpu.etadot-real-era5-case.v1":
            raise ValueError("real ERA5 oracle lacks pinned source provenance")
        if source_provenance.get("classification") != REAL_ERA5_CLASSIFICATION:
            raise ValueError("real ERA5 source provenance classification mismatch")
        real_column = source_provenance.get("selected_column", {})
        if (
            real_column.get("longitude_deg") != -2.0
            or real_column.get("latitude_deg") != 48.0
            or real_column.get("model_levels") != 137
            or real_column.get("level_coverage") != "1/to/137"
            or not isinstance(real_column.get("surface_pressure_pa"), (int, float))
        ):
            raise ValueError(
                f"real ERA5 selected-column provenance is incomplete: {real_column!r}"
            )
        profiles = real_column.get("profile_values", {})
        if (
            set(profiles) != {"77", "130", "131", "132", "133"}
            or any(len(values) != 137 for values in profiles.values())
        ):
            raise ValueError("real ERA5 selected column does not contain all 137 native levels")

    run_provenance = json.loads(args.run_provenance.read_text(encoding="utf-8"))
    if run_provenance.get("schema") != "flexpart-gpu.etadot-oracle-run-provenance.v1":
        raise ValueError("run provenance has the wrong schema")
    image_id = run_provenance.get("docker_image", {}).get("id")
    compiler_version = run_provenance.get("compiler", {}).get("version")
    executable_hash = run_provenance.get("oracle_executable", {}).get("sha256")
    if not image_id or not compiler_version or not executable_hash:
        raise ValueError("run provenance lacks concrete build identity")

    if classification == REAL_ERA5_CLASSIFICATION:
        embedded_source = source_fixture.get("source_provenance", {})
        run_source = run_provenance.get("source_provenance", {})
        if (
            not embedded_source.get("sha256")
            or embedded_source.get("sha256") != run_source.get("sha256")
        ):
            raise ValueError(
                "real ERA5 source provenance differs between extraction and run identity"
            )

    fixture_hashes = source_fixture.get("source_hashes_sha256", {})
    run_inputs = run_provenance.get("oracle_inputs", {})
    run_outputs = run_provenance.get("oracle_outputs", {})

    if classification == REAL_ERA5_CLASSIFICATION:
        fort4_path = Path(run_inputs.get("fort.4", {}).get("path", ""))
        if not fort4_path.is_file():
            raise ValueError("real ERA5 oracle run provenance lacks fort.4")
        real_namgen = parse_namgen(fort4_path)
        real_expected = {
            "maxl": 6,
            "maxb": 6,
            "mlevel": 137,
            "mlevelist": "1/to/137",
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
        for key, expected in real_expected.items():
            actual_value = real_namgen.get(key)
            if actual_value != expected:
                raise ValueError(
                    f"real ERA5 calc_etadot namelist mismatch: "
                    f"{key}={actual_value!r} expected {expected!r}"
                )
    for name in ("fort.12", "fort.21"):
        if fixture_hashes.get(name) != run_inputs.get(name, {}).get("sha256"):
            raise ValueError(f"{name} hash differs between extracted fixture and run provenance")
    if fixture_hashes.get("fort.15") != run_outputs.get("fort.15", {}).get("sha256"):
        raise ValueError("fort.15 hash differs between extracted oracle and run provenance")

    nlev = oracle["nlev"]
    levels = oracle["levels_present"]
    nx, ny = oracle["nx"], oracle["ny"]
    nxy = nx * ny
    if classification == REAL_ERA5_CLASSIFICATION:
        if nlev != 137 or levels != list(range(1, 138)) or (nx, ny) != (6, 6):
            raise ValueError(
                "real ERA5 oracle coverage changed: "
                f"nlev={nlev}, levels={levels[:3]}..{levels[-3:]}, grid={(nx, ny)}"
            )
        source_provenance = source_fixture["source_provenance"]["content"]
        selected = source_provenance.get("selected_column", {})
        if (
            source_provenance.get("source_grid") != {"nx": 65, "ny": 41}
            or selected.get("longitude_deg") != -2.0
            or selected.get("latitude_deg") != 48.0
            or selected.get("model_levels") != 137
            or selected.get("level_coverage") != "1/to/137"
        ):
            raise ValueError(
                f"real ERA5 selected-column provenance changed: {selected!r}"
            )
        oracle_grid = source_provenance.get("oracle_grid", {})
        if (
            oracle_grid.get("nx") != 6
            or oracle_grid.get("ny") != 6
            or oracle_grid.get("construction")
            != "selected real ERA5 column replicated horizontally"
        ):
            raise ValueError(f"unexpected real-column oracle grid: {oracle_grid!r}")

        selected_profiles = selected.get("profile_values", {})
        eta_profile = selected_profiles.get("77")
        if not isinstance(eta_profile, list) or len(eta_profile) != 137:
            raise ValueError("real ERA5 provenance lacks the complete eta-dot profile")
        motion = json.loads(args.source_motion.read_text(encoding="utf-8"))
        motion_values = motion.get("values", [])
        if len(motion_values) != 137 * nxy:
            raise ValueError("real ERA5 motion does not contain 137 replicated levels")
        for level_index, expected_eta in enumerate(eta_profile):
            row = motion_values[level_index * nxy : (level_index + 1) * nxy]
            if len(row) != nxy or any(abs(value - expected_eta) > 1.0e-12 for value in row):
                raise ValueError(
                    f"real ERA5 eta-dot level {level_index + 1} was not replicated "
                    "exactly from the selected source column"
                )

        expected_ps = float(selected.get("surface_pressure_pa"))
        actual_ps = oracle.get("surface_pressure_pa_xy", [])
        if len(actual_ps) != nxy:
            raise ValueError("real ERA5 oracle surface-pressure field has wrong shape")
        max_ps_error = max(abs(value - expected_ps) for value in actual_ps)
        if max_ps_error > 0.5:
            raise ValueError(
                "real ERA5 spectral ln(ps) does not reconstruct the selected "
                f"surface pressure: max error {max_ps_error} Pa"
            )

    values = candidate["result"]["values_interface_pa_s"]
    if len(values) != (nlev + 1) * nxy:
        raise ValueError(
            f"candidate interface value count {len(values)} != {(nlev + 1) * nxy}"
        )

    per_level = {}
    row_overall = True
    compact_report = classification == REAL_ERA5_CLASSIFICATION
    overall_max_abs = 0.0
    overall_max_rel = 0.0
    failure_count = 0
    failures = []
    for lvl in levels:
        k = lvl
        search = oracle["oracle_etadot_pa_s_xy_by_level"][str(lvl)]
        comparisons = []
        level_overall = True
        level_max_abs = 0.0
        level_max_rel = 0.0
        level_failures = 0
        for y in range(ny):
            for x in range(nx):
                point = y * nx + x
                actual = values[k * nxy + point]
                expected = search[point]
                diff = abs(actual - expected)
                relative = diff / abs(expected) if expected != 0.0 else (0.0 if diff == 0.0 else float("inf"))
                limit = max(
                    PRESSURE_VELOCITY_ABS_TOL_PA_S,
                    PRESSURE_VELOCITY_REL_TOL * abs(expected),
                )
                passed = diff <= limit
                level_overall = level_overall and passed
                level_max_abs = max(level_max_abs, diff)
                level_max_rel = max(level_max_rel, relative)
                overall_max_abs = max(overall_max_abs, diff)
                overall_max_rel = max(overall_max_rel, relative)
                if not passed:
                    level_failures += 1
                    failure_count += 1
                    if len(failures) < 20:
                        failures.append({
                            "level": lvl,
                            "x": x,
                            "y": y,
                            "candidate_pa_s": actual,
                            "oracle_pa_s": expected,
                            "absolute_difference_pa_s": diff,
                            "relative_difference": relative,
                            "allowed_difference_pa_s": limit,
                        })
                if not compact_report:
                    comparisons.append({
                        "x": x,
                        "y": y,
                        "candidate_pa_s": actual,
                        "oracle_pa_s": expected,
                        "absolute_difference_pa_s": diff,
                        "allowed_difference_pa_s": limit,
                        "pass": passed,
                    })
        level_report = {
            "pass": level_overall,
            "comparison_count": nxy,
            "failure_count": level_failures,
            "max_absolute_difference_pa_s": level_max_abs,
            "max_relative_difference": level_max_rel,
        }
        if not compact_report:
            level_report["comparisons"] = comparisons
        per_level[str(lvl)] = level_report
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
            "calc_etadot_f90_git_blob": PINNED_CALC_ETADOT_GIT_BLOB,
            "source_contract_snippets_verified": True,
            "namelist_configuration_verified": True,
        },
        "candidate": {
            "report_path": str(args.candidate),
            "sha256": sha256(args.candidate),
        },
        "source_fixture": source_fixture,
        "coverage": {
            "classification": classification,
            "levels_compared": len(levels),
            "points_per_level": nxy,
            "comparisons": len(levels) * nxy,
            "complete_real_native_column": classification == REAL_ERA5_CLASSIFICATION,
            "selected_real_column": (
                source_fixture.get("source_provenance", {})
                .get("content", {})
                .get("selected_column")
                if classification == REAL_ERA5_CLASSIFICATION
                else None
            ),
            "failure_count": failure_count,
            "max_absolute_difference_pa_s": overall_max_abs,
            "max_relative_difference": overall_max_rel,
            "first_failures": failures,
            "report_mode": "compact" if compact_report else "full",
        },
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
            "calc_etadot_f90": {
                "path": str(calc_etadot),
                "git_blob": source_blob,
                "canonical_sha256": calc_etadot_digest,
                "working_tree_sha256": sha256(calc_etadot),
            },
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