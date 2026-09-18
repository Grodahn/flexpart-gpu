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
-   before the FLEXPART invocation
- ``fortran.log`` containing ``CONGRATULATIONS``
- ``oracle_executable.sha256`` tying the repetition to the experiment build
- ``consumed_inputs.json`` with SHA-256 of the prepared run-directory options
  and shared meteorology, recorded BEFORE the FLEXPART invocation
- ``COMMAND``, ``RELEASES``, ``OUTGRID`` post-run copies (traceability only;
  the pre-invocation ``consumed_inputs.json`` is the input evidence)

Each case directory must contain ``experiment.json`` written by the runner
before its repetitions, carrying the unique ``experiment_id`` of the
invocation that produced it. Only the cases named with ``--cases`` are
evaluated, and a multi-case report requires all of them to share one
experiment ID, so repetitions from separate experiments can never be combined
silently (even when they used an identical executable).

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
        --fixtures-dir fixtures/corpus/fortran \
        [--cases "ADV-ANA-001 WIND-UNI-002"]
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
    parser.add_argument("--cases", default=None,
                        help="Space-separated subset of repeatability cases to evaluate "
                             "(default: all profile cases). Only the named cases enter "
                             "the report, so a single-case run yields a valid single-case "
                             "report instead of mixing in older repetitions.")
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

    if args.cases and args.cases.strip() and args.cases.strip() != "all":
        requested = args.cases.split()
    else:
        requested = list(profile["repeatability_cases"])
    for case_id in requested:
        if case_id not in profile["repeatability_cases"]:
            raise SystemExit(f"case {case_id} is not in the frozen repeatability set")
    if not requested:
        raise SystemExit("no repeatability cases requested")

    cases_report = {}
    experiment_executables = {}
    for case_id in requested:
        classification = profile["repeatability_cases"][case_id]
        case_dir = repeat_dir / case_id
        if not case_dir.is_dir():
            raise SystemExit(f"missing repeatability case directory: {case_dir}")
        experiment_path = case_dir / "experiment.json"
        if not experiment_path.is_file():
            raise SystemExit(
                f"{case_id}: missing experiment.json; the case was not produced by "
                "the current oracle-repeatability runner")
        experiment = json.loads(experiment_path.read_text(encoding="utf-8"))
        if experiment.get("execution_profile", {}) != {"id": profile["id"], "version": profile["version"]}:
            raise SystemExit(f"{case_id}: experiment profile does not match the frozen contract")
        if experiment.get("case") != case_id:
            raise SystemExit(f"{case_id}: experiment record belongs to {experiment.get('case')}")
        if not isinstance(experiment.get("experiment_id"), str) or not experiment["experiment_id"]:
            raise SystemExit(
                f"{case_id}: experiment record lacks an experiment ID; the case was not "
                "produced by the current oracle-repeatability runner")
        if experiment.get("classification") != classification:
            raise SystemExit(f"{case_id}: experiment classification mismatch")
        if experiment.get("oracle_pinned_commit") != reference["pinned_commit"]:
            raise SystemExit(f"{case_id}: experiment pinned commit mismatch")
        if experiment.get("external_seed_control") != "unavailable":
            raise SystemExit(f"{case_id}: experiment must record seed control as unavailable")
        reps = sorted(p for p in case_dir.iterdir() if p.is_dir() and p.name.startswith("rep_"))
        if len(reps) < profile["minimum_repetitions"]:
            raise SystemExit(f"{case_id}: need at least {profile['minimum_repetitions']} repetitions, found {len(reps)}")
        if len(reps) != experiment.get("repetitions_requested"):
            raise SystemExit(
                f"{case_id}: found {len(reps)} repetition directories but the experiment "
                f"requested {experiment.get('repetitions_requested')}; stale directories "
                "from another experiment must not be evaluated")
        experiment_executables[case_id] = experiment["oracle_executable_sha256"]

        rep_reports = []
        for rep in reps:
            raw_dir = rep / "raw"
            summary_path = rep / "oracle_summary.json"
            runtime_path = rep / "runtime_profile.json"
            log_path = rep / "fortran.log"
            exe_path = rep / "oracle_executable.sha256"
            consumed_path = rep / "consumed_inputs.json"
            for required in (raw_dir / "header", raw_dir / "dates", summary_path, runtime_path, log_path,
                             exe_path, consumed_path,
                             rep / "COMMAND", rep / "RELEASES", rep / "OUTGRID"):
                if not required.is_file():
                    raise SystemExit(f"{case_id} {rep.name}: missing required artifact {required.name}")
            if not list(raw_dir.glob("grid_conc_*")):
                raise SystemExit(f"{case_id} {rep.name}: missing grid_conc_* in raw/")
            log_text = log_path.read_text(encoding="utf-8", errors="replace")
            if "CONGRATULATIONS" not in log_text:
                raise SystemExit(f"{case_id} {rep.name}: fortran.log lacks successful completion marker")
            rep_exe_sha = exe_path.read_text(encoding="utf-8").strip()
            if rep_exe_sha != experiment["oracle_executable_sha256"]:
                raise SystemExit(
                    f"{case_id} {rep.name}: repetition is tied to executable {rep_exe_sha}, "
                    f"not this experiment's {experiment['oracle_executable_sha256']}")
            runtime = json.loads(runtime_path.read_text(encoding="utf-8"))
            if runtime.get("execution_profile", {}).get("id") != profile["id"]:
                raise SystemExit(f"{case_id} {rep.name}: runtime profile id mismatch")
            if runtime.get("execution_profile", {}).get("version") != profile["version"]:
                raise SystemExit(f"{case_id} {rep.name}: runtime profile version mismatch")
            if runtime.get("runtime_environment") != profile["runtime_environment"]:
                raise SystemExit(f"{case_id} {rep.name}: runtime environment does not match frozen profile")
            if runtime.get("reference_manifest_sha256") != manifest_sha:
                raise SystemExit(f"{case_id} {rep.name}: reference manifest changed during experiment")
            consumed = json.loads(consumed_path.read_text(encoding="utf-8"))
            for required_option in ("options/COMMAND", "options/RELEASES", "options/OUTGRID"):
                if required_option not in consumed.get("options", {}):
                    raise SystemExit(f"{case_id} {rep.name}: consumed inputs lack {required_option}")
            if not consumed.get("meteo"):
                raise SystemExit(f"{case_id} {rep.name}: consumed inputs lack meteorology hashes")
            raw_hashes = hash_tree(raw_dir)
            summary_sha = digest(summary_path)
            summary = json.loads(summary_path.read_text(encoding="utf-8"))
            consumed_digest = hashlib.sha256(
                json.dumps(consumed, sort_keys=True).encode("utf-8")).hexdigest()
            rep_reports.append({
                "rep": rep.name,
                "oracle_executable_sha256": rep_exe_sha,
                "consumed_inputs_digest": consumed_digest,
                "consumed_inputs": consumed,
                "raw_sha256": raw_hashes,
                "decoded_summary_sha256": summary_sha,
                "decoded_summary": summary,
                "runtime_profile": runtime,
            })

        # Every repetition of a case must have consumed identical inputs,
        # including the generated meteorology; otherwise the repeatability
        # claim is void.
        baseline_inputs = json.dumps(rep_reports[0]["consumed_inputs"], sort_keys=True)
        for rep in rep_reports[1:]:
            if json.dumps(rep["consumed_inputs"], sort_keys=True) != baseline_inputs:
                current_keys = set(rep["consumed_inputs"].get("options", {})) | set(rep["consumed_inputs"].get("meteo", {}))
                baseline_keys = set(rep_reports[0]["consumed_inputs"].get("options", {})) | set(rep_reports[0]["consumed_inputs"].get("meteo", {}))
                raise SystemExit(
                    f"{case_id} {rep['rep']}: consumed inputs differ from {rep_reports[0]['rep']} "
                    f"(differing keys: {sorted(current_keys ^ baseline_keys) or 'same keys, different hashes'}); "
                    "repetitions with different inputs must not be compared")
        baseline_consumed = rep_reports[0]["consumed_inputs"]

        # The shared meteorology and versioned fixtures on disk must still be
        # what the repetitions consumed; otherwise the report would attribute
        # results to inputs that were never used.
        meteo_case = meteo_dir / case_id
        if not (meteo_case / "AVAILABLE").is_file():
            raise SystemExit(f"missing generated meteorology for {case_id}: {meteo_case}")
        if hash_tree(meteo_case) != rep_reports[0]["consumed_inputs"]["meteo"]:
            raise SystemExit(
                f"{case_id}: generated meteorology changed since the repetitions ran; "
                "rerun the experiment instead of reporting stale inputs")
        fixture_case = fixtures_dir / case_id
        if not fixture_case.is_dir():
            raise SystemExit(f"missing oracle fixture: {fixture_case}")
        fixture_hashes = hash_tree(fixture_case)
        if not fixture_hashes:
            raise SystemExit(f"empty oracle fixture: {fixture_case}")
        consumed_options = rep_reports[0]["consumed_inputs"]["options"]
        for fixture_rel in ("COMMAND", "RELEASES", "OUTGRID", "AGECLASSES", "RECEPTORS"):
            if fixture_hashes.get(fixture_rel) != consumed_options.get(f"options/{fixture_rel}"):
                raise SystemExit(
                    f"{case_id}: versioned fixture {fixture_rel} differs from what the "
                    "repetitions consumed; rerun the experiment")
        for fixture_rel, sha in fixture_hashes.items():
            if fixture_rel.startswith("SPECIES/") and consumed_options.get(f"options/{fixture_rel}") != sha:
                raise SystemExit(
                    f"{case_id}: versioned fixture {fixture_rel} differs from what the "
                    "repetitions consumed; rerun the experiment")

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
            # Keep full decoded and consumed-input payloads out of the per-rep
            # inventory; the hashed summary file and the case-level verified
            # input map remain the attributable artifacts.
            del rep["decoded_summary"]
            del rep["consumed_inputs"]

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
            "experiment": experiment,
            "executable_sha256": experiment["oracle_executable_sha256"],
            "repetitions": rep_reports,
            "all_byte_identical_to_baseline": all_identical,
            "repeatability_observation": observation,
            "consumed_inputs_identical_across_reps": True,
            "consumed_input_hashes": baseline_consumed,
            "consumed_inputs_note": "SHA-256 of the prepared run-directory options and shared "
                                    "meteorology, recorded before each invocation and verified "
                                    "identical across all repetitions of this case.",
            "versioned_inputs_at_report": {
                "fixture": fixture_hashes,
                "generated_meteorology": hash_tree(meteo_case),
                "note": "Report-time state, cross-checked against the pre-run consumed "
                        "inputs above; not standalone evidence.",
            },
        }

    experiment_shas = {case: experiment_executables[case] for case in requested}
    if len(set(experiment_shas.values())) != 1:
        raise SystemExit(
            f"evaluated cases tie to different executables {experiment_shas}; "
            "one report must cover a single experiment only")
    experiment_ids = {case: cases_report[case]["experiment"]["experiment_id"] for case in requested}
    if len(set(experiment_ids.values())) != 1:
        raise SystemExit(
            f"evaluated cases belong to different experiments {experiment_ids}; "
            "rerun all cases in one oracle-repeatability invocation instead of "
            "combining repetitions from separate experiments")
    experiment_id = next(iter(set(experiment_ids.values())))

    verified_exe_sha = next(iter(set(experiment_shas.values())))
    if exe_sha != verified_exe_sha:
        raise SystemExit(
            f"oracle executable on disk ({exe_sha}) differs from the experiment build "
            f"({verified_exe_sha}); rebuild and rerun the experiment instead of "
            "reporting repetitions tied to another binary")
    scope_note = (
        "Canonical evidence evaluates all profile cases."
        if set(requested) == set(profile["repeatability_cases"]) else
        f"Single-scope report evaluating only {sorted(requested)}; canonical evidence "
        "requires all profile cases.")
    report = {
        "status": "ORACLE_REPEATABILITY_CHARACTERIZED_NO_PARITY_VERDICT",
        "status_note": "Execution repeatability only; no candidate-vs-oracle physics parity is claimed.",
        "scope_note": scope_note,
        "cases_evaluated": sorted(requested),
        "experiment_id": experiment_id,
        "execution_profile": {"id": profile["id"], "version": profile["version"]},
        "reference_manifest_sha256": manifest_sha,
        "oracle": oracle,
        "oracle_executable_sha256": verified_exe_sha,
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
