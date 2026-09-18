#!/usr/bin/env python3
"""Compare retained oracle repeatability repetitions (#49).

Reads the retained per-repetition oracle artifacts written by
``scripts/run-corpus.sh oracle-repeatability`` under
``target/corpus/oracle_repeatability/<CASE>/rep_XX/`` and writes one
machine-readable repeatability report. This script characterizes execution
repeatability only; it never claims candidate-vs-oracle physics parity.

Each repetition directory must contain:
- ``raw/header``, ``raw/dates``, ``raw/grid_conc_*`` (plus optional partposit)
- ``oracle_summary.json`` decoded by ``scripts/corpus/decode_oracle_output.py``
- ``runtime_profile.json`` written by ``check-runtime-profile`` immediately
  before the FLEXPART invocation
- ``fortran.log`` containing ``CONGRATULATIONS``
- ``COMMAND``, ``RELEASES``, ``OUTGRID`` copies of the consumed inputs

The report records, per case and repetition: execution-profile identity and
version, oracle executable hash, consumed input hashes, raw artifact inventory
with SHA-256, decoded summary hash, and overall byte/hash-identical-to-baseline
result. Non-identical artifacts name the differing files and quantify decoded
numeric differences (max absolute and relative difference). No scientific
PASS/FAIL threshold is applied.

Fails closed (non-zero exit) when the pinned checkout, profile, build
provenance, repetitions, or artifacts are incomplete or inconsistent. Only the
Python standard library is used.

Usage:
    python3 scripts/corpus/compare_oracle_repeatability.py \
        --repeat-dir target/corpus/oracle_repeatability \
        --output target/corpus/oracle_repeatability_report.json \
        --oracle-manifest reference/flexpart-11.1.json \
        --oracle-checkout ../flexpart \
        --oracle-exe ../flexpart/src/FLEXPART \
        --meteo-dir target/corpus/meteo \
        --fixtures-dir fixtures/corpus/fortran
"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def command(*args) -> str:
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()


def hash_tree(root: Path) -> dict:
    result = {}
    if not root.is_dir():
        return result
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        try:
            result[str(path.relative_to(root))] = digest(path)
        except Exception:
            result[str(path.resolve())] = digest(path)
    return result


def git_state(path: Path) -> dict:
    checkout = path.resolve().as_posix()
    git = ("git", "-c", f"safe.directory={checkout}", "-C", checkout)
    return {
        "commit": command(*git, "rev-parse", "HEAD"),
        "worktree_dirty": bool(command(*git, "status", "--porcelain")),
    }


def load_profile(manifest_path: Path) -> dict:
    reference = json.loads(manifest_path.read_text(encoding="utf-8"))
    profile = reference.get("execution_profile", {})
    if profile.get("id") != "flexpart-11.1-single-thread" or profile.get("version") != 1:
        raise ValueError("missing or unsupported oracle execution profile")
    runtime = profile.get("runtime_environment", {})
    required = {
        "OMP_NUM_THREADS", "OMP_THREAD_LIMIT", "OMP_DYNAMIC", "OMP_NESTED",
        "OMP_MAX_ACTIVE_LEVELS", "OMP_SCHEDULE", "OMP_PROC_BIND", "OMP_WAIT_POLICY",
    }
    if set(runtime) != required:
        raise ValueError("incomplete oracle runtime environment contract")
    if runtime.get("OMP_NUM_THREADS") != "1" or runtime.get("OMP_THREAD_LIMIT") != "1":
        raise ValueError("canonical oracle profile requires one thread")
    cases = profile.get("repeatability_cases", {})
    if cases.get("ADV-ANA-001") != "deterministic" or cases.get("WIND-UNI-002") != "stochastic":
        raise ValueError("repeatability cases must name ADV-ANA-001 deterministic and WIND-UNI-002 stochastic")
    if not isinstance(profile.get("minimum_repetitions"), int) or profile["minimum_repetitions"] < 5:
        raise ValueError("minimum_repetitions must be at least 5")
    if profile.get("external_seed_control") != "unavailable":
        raise ValueError("external oracle seed control must be recorded as unavailable")
    build = profile.get("build", {})
    if build.get("make_arguments") != "eta=no arch=x86-64":
        raise ValueError("build contract must pin make_arguments eta=no arch=x86-64")
    return reference, profile


def compare_numbers(baseline, current, path, differences):
    if baseline is None and current is None:
        return
    if isinstance(baseline, bool) or isinstance(current, bool):
        if baseline is not current and baseline != current:
            differences.append({"field": path, "baseline": baseline, "current": current,
                                "abs_diff": None, "rel_diff": None, "kind": "bool"})
        return
    if isinstance(baseline, (int, float)) and isinstance(current, (int, float)):
        b = float(baseline)
        c = float(current)
        if b != c:
            abs_diff = abs(c - b)
            denom = max(abs(b), abs(c))
            rel_diff = abs_diff / denom if denom else None
            differences.append({"field": path, "baseline": baseline, "current": current,
                                "abs_diff": abs_diff, "rel_diff": rel_diff, "kind": "number"})
        return
    if isinstance(baseline, list) and isinstance(current, list):
        if len(baseline) != len(current):
            differences.append({"field": path, "baseline": f"len {len(baseline)}",
                                "current": f"len {len(current)}", "abs_diff": None,
                                "rel_diff": None, "kind": "length"})
            return
        for i, (b, c) in enumerate(zip(baseline, current)):
            compare_numbers(b, c, f"{path}[{i}]", differences)
        return
    if isinstance(baseline, dict) and isinstance(current, dict):
        for key in sorted(set(baseline) | set(current)):
            if key not in baseline:
                differences.append({"field": f"{path}.{key}", "baseline": None,
                                    "current": current[key], "abs_diff": None,
                                    "rel_diff": None, "kind": "added"})
            elif key not in current:
                differences.append({"field": f"{path}.{key}", "baseline": baseline[key],
                                    "current": None, "abs_diff": None,
                                    "rel_diff": None, "kind": "removed"})
            else:
                compare_numbers(baseline[key], current[key], f"{path}.{key}" if path else key, differences)
        return
    if baseline != current:
        differences.append({"field": path, "baseline": baseline, "current": current,
                            "abs_diff": None, "rel_diff": None, "kind": "value"})


def summarize_differences(differences):
    abs_vals = [d["abs_diff"] for d in differences if isinstance(d.get("abs_diff"), (int, float))]
    rel_vals = [d["rel_diff"] for d in differences if isinstance(d.get("rel_diff"), (int, float))]
    return {
        "differing_fields": len(differences),
        "max_abs_diff": max(abs_vals) if abs_vals else None,
        "max_rel_diff": max(rel_vals) if rel_vals else None,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repeat-dir", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--oracle-manifest", required=True)
    parser.add_argument("--oracle-checkout", required=True)
    parser.add_argument("--oracle-exe", required=True)
    parser.add_argument("--meteo-dir", required=True)
    parser.add_argument("--fixtures-dir", required=True)
    parser.add_argument("--image", default="flexpart-fortran:latest")
    args = parser.parse_args()

    manifest_path = Path(args.oracle_manifest)
    reference, profile = load_profile(manifest_path)
    manifest_sha = digest(manifest_path)
    repeat_dir = Path(args.repeat_dir)
    meteo_dir = Path(args.meteo_dir)
    fixtures_dir = Path(args.fixtures_dir)
    oracle_exe = Path(args.oracle_exe)

    oracle = git_state(Path(args.oracle_checkout))
    if oracle["commit"] != reference["pinned_commit"] or oracle["worktree_dirty"]:
        raise SystemExit("oracle checkout is not the pinned unmodified FLEXPART source")
    if not oracle_exe.is_file():
        raise SystemExit(f"oracle executable missing: {oracle_exe}")
    exe_sha = digest(oracle_exe)
    try:
        makefile_sha = digest(Path(args.oracle_checkout) / "src" / "makefile_gfortran")
    except Exception as exc:
        raise SystemExit(f"oracle makefile missing: {exc}")
    try:
        image_id = command("docker", "image", "inspect", args.image, "--format", "{{.Id}}")
    except Exception as exc:
        raise SystemExit(f"oracle image not available: {exc}")
    try:
        compiler = command("docker", "run", "--rm", args.image, "gfortran", "--version").splitlines()[0]
    except Exception as exc:
        raise SystemExit(f"oracle compiler not available: {exc}")
    try:
        docker_version = command("docker", "--version")
    except Exception:
        docker_version = "unavailable"

    cases_report = {}
    for case_id, classification in profile["repeatability_cases"].items():
        case_dir = repeat_dir / case_id
        if not case_dir.is_dir():
            raise SystemExit(f"missing repeatability case directory: {case_dir}")
        reps = sorted(p for p in case_dir.iterdir() if p.is_dir() and p.name.startswith("rep_"))
        if len(reps) < profile["minimum_repetitions"]:
            raise SystemExit(f"{case_id}: need at least {profile['minimum_repetitions']} repetitions, found {len(reps)}")
        fixture_case = fixtures_dir / case_id
        if not fixture_case.is_dir():
            raise SystemExit(f"missing oracle fixture: {fixture_case}")
        fixture_hashes = hash_tree(fixture_case)
        if not fixture_hashes:
            raise SystemExit(f"empty oracle fixture: {fixture_case}")
        meteo_case = meteo_dir / case_id
        meteo_hashes = hash_tree(meteo_case)
        if not meteo_hashes or not (meteo_case / "AVAILABLE").is_file():
            raise SystemExit(f"missing generated meteorology for {case_id}: {meteo_case}")

        rep_reports = []
        for rep in reps:
            raw_dir = rep / "raw"
            summary_path = rep / "oracle_summary.json"
            runtime_path = rep / "runtime_profile.json"
            log_path = rep / "fortran.log"
            for required in (raw_dir / "header", raw_dir / "dates", summary_path, runtime_path, log_path,
                             rep / "COMMAND", rep / "RELEASES", rep / "OUTGRID"):
                if not required.is_file():
                    raise SystemExit(f"{case_id} {rep.name}: missing required artifact {required.name}")
            if not list(raw_dir.glob("grid_conc_*")):
                raise SystemExit(f"{case_id} {rep.name}: missing grid_conc_* in raw/")
            log_text = log_path.read_text(encoding="utf-8", errors="replace")
            if "CONGRATULATIONS" not in log_text:
                raise SystemExit(f"{case_id} {rep.name}: fortran.log lacks successful completion marker")
            runtime = json.loads(runtime_path.read_text(encoding="utf-8"))
            if runtime.get("execution_profile", {}).get("id") != profile["id"]:
                raise SystemExit(f"{case_id} {rep.name}: runtime profile id mismatch")
            if runtime.get("execution_profile", {}).get("version") != profile["version"]:
                raise SystemExit(f"{case_id} {rep.name}: runtime profile version mismatch")
            if runtime.get("runtime_environment") != profile["runtime_environment"]:
                raise SystemExit(f"{case_id} {rep.name}: runtime environment does not match frozen profile")
            if runtime.get("reference_manifest_sha256") != manifest_sha:
                raise SystemExit(f"{case_id} {rep.name}: reference manifest changed during experiment")
            raw_hashes = hash_tree(raw_dir)
            summary_sha = digest(summary_path)
            summary = json.loads(summary_path.read_text(encoding="utf-8"))
            rep_reports.append({
                "rep": rep.name,
                "raw_sha256": raw_hashes,
                "decoded_summary_sha256": summary_sha,
                "decoded_summary": summary,
                "runtime_profile": runtime,
                "input_copies_sha256": {
                    name: digest(rep / name) for name in ("COMMAND", "RELEASES", "OUTGRID")
                },
            })

        baseline = rep_reports[0]
        baseline_summary = baseline["decoded_summary"]
        for rep in rep_reports:
            baseline_raw = baseline["raw_sha256"]
            current_raw = rep["raw_sha256"]
            differing_raw = sorted(
                key for key in set(baseline_raw) | set(current_raw)
                if baseline_raw.get(key) != current_raw.get(key)
            )
            rep["raw_byte_identical_to_baseline"] = not differing_raw
            rep["differing_raw_files"] = differing_raw
            rep["decoded_hash_identical_to_baseline"] = (
                rep["decoded_summary_sha256"] == baseline["decoded_summary_sha256"]
            )
            diffs = []
            compare_numbers(baseline_summary, rep["decoded_summary"], "", diffs)
            # Drop the empty-path wrapper entry when both are dicts; field paths carry the location.
            diffs = [d for d in diffs if d["field"]]
            rep["differing_decoded_fields"] = diffs
            rep["decoded_numeric_summary"] = summarize_differences(diffs)
            rep["byte_identical_to_baseline"] = bool(
                rep["raw_byte_identical_to_baseline"] and rep["decoded_hash_identical_to_baseline"]
            )
            # Keep the full decoded payload out of the per-rep inventory; the
            # hashed summary file remains the attributable decoded artifact.
            del rep["decoded_summary"]

        all_identical = all(r["byte_identical_to_baseline"] for r in rep_reports)
        any_decoded_diff = any(r["differing_decoded_fields"] for r in rep_reports)
        if all_identical:
            observation = "bitwise repeatable under the frozen profile"
        elif not any_decoded_diff:
            observation = "decoded summaries identical but raw bytes differ"
        else:
            observation = "not byte-identical; see differing fields and numeric summaries"
        cases_report[case_id] = {
            "classification": classification,
            "baseline_rep": baseline["rep"],
            "repetitions": rep_reports,
            "all_byte_identical_to_baseline": all_identical,
            "repeatability_observation": observation,
            "consumed_input_hashes": {
                "fixture": fixture_hashes,
                "generated_meteorology": meteo_hashes,
            },
        }

    report = {
        "status": "ORACLE_REPEATABILITY_CHARACTERIZED_NO_PARITY_VERDICT",
        "status_note": "Execution repeatability only; no candidate-vs-oracle physics parity is claimed.",
        "execution_profile": {"id": profile["id"], "version": profile["version"]},
        "reference_manifest_sha256": manifest_sha,
        "oracle": oracle,
        "oracle_executable_sha256": exe_sha,
        "container": {"image": args.image, "image_id": image_id, "docker_version": docker_version},
        "compiler": {
            "version": compiler,
            "make_arguments": profile["build"]["make_arguments"],
            "makefile_sha256": makefile_sha,
        },
        "runtime_environment": profile["runtime_environment"],
        "external_seed_control": "unavailable",
        "seed_note": "FLEXPART 11.1 exposes no approved external seed control in this project; "
                     "repeated runs reuse the pristine default RNG initialization and must not be "
                     "presented as independent stochastic realizations.",
        "cases": cases_report,
    }
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Oracle repeatability report: {output}")


if __name__ == "__main__":
    main()
