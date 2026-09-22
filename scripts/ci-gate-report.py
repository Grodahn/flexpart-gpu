#!/usr/bin/env python3
"""Generate the machine-readable CI gate report (schema v1).

Technical gate only. The report never claims scientific parity:
``scientific_verdict`` is always ``NOT_EVALUATED``. Versioned scientific
thresholds are owned by the scientific-metrics track and are not applied here.

The extended test corpus (runner) is owned by a parallel track. This writer
only records the stable allow-list and reports every other known corpus
category as NOT_WIRED, never as PASS.
"""

import argparse
import datetime
import hashlib
import json
import re
import subprocess
from pathlib import Path

SCHEMA_VERSION = "1.0"
GATE_ID = "ci-technical-gate"

# Required corpus from Issue #6 (RISK-03.3G-01). None of these are wired into
# the per-PR technical gate yet; they stay local/manual until their runner
# and metric format are stable (owned by parallel tracks:
# scripts/run-corpus.sh + docs/corpus-matrix.md and
# scripts/evaluate/evaluate_case.py + docs/evaluation.md).
KNOWN_FUTURE_CORPUS = [
    "ADV-ANA-001",
    "WIND-UNI-002",
    "WIND-SHEAR-003",
    "PBL-STABLE-004",
    "PBL-NEUTRAL-005",
    "PBL-UNSTABLE-006",
    "DRY-007",
    "WET-008",
    "REPEAT-009",
    "RESTART-010",
    "DECAY-011",
    "CONV-012",
    "ETEX-MINI-013",
]


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def hash_if_exists(path):
    path = Path(path)
    if path.is_file():
        return sha256_file(path)
    return None


def git_revision(checkout):
    try:
        commit = subprocess.run(
            ["git", "-C", str(checkout), "rev-parse", "HEAD"],
            check=True, capture_output=True, text=True,
        ).stdout.strip()
    except Exception:
        return {"commit": "unknown", "dirty": None}
    try:
        status = subprocess.run(
            ["git", "-C", str(checkout), "status", "--porcelain"],
            check=True, capture_output=True, text=True,
        ).stdout.strip()
    except Exception:
        status = ""
    return {"commit": commit, "dirty": bool(status)}


def parse_adapter(preflight_log):
    info = {
        "name": None,
        "backend": None,
        "type": None,
        "is_software": None,
        "fallback_requested": None,
        "preflight_log": str(preflight_log) if preflight_log else None,
    }
    path = Path(preflight_log) if preflight_log else None
    if not path or not path.is_file():
        return info
    text = path.read_text(encoding="utf-8", errors="replace")
    # Example: "adapter: llvmpipe (Vulkan, Cpu)"
    match = re.search(r"adapter:\s*(.+?)\s*\((\w+),\s*(\w+)\)", text)
    if match:
        info["name"] = match.group(1).strip()
        info["backend"] = match.group(2).strip()
        info["type"] = match.group(3).strip()
    if "software adapter: true" in text:
        info["is_software"] = True
    elif "software adapter: false" in text:
        info["is_software"] = False
    if "software fallback requested: true" in text:
        info["fallback_requested"] = True
    elif "software fallback requested: false" in text:
        info["fallback_requested"] = False
    return info


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True)
    parser.add_argument("--project-root", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--oracle-checkout", required=True)
    parser.add_argument("--flex-extract-checkout", default=None)
    parser.add_argument("--status", required=True,
                        choices=["TECHNICAL_PASS", "TECHNICAL_FAIL"])
    parser.add_argument("--particles", default="1000")
    parser.add_argument("--allowlist", default="")
    parser.add_argument("--failure", default=None)
    args = parser.parse_args()

    project_root = Path(args.project_root)
    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    oracle_manifest_path = project_root / "reference" / "flexpart-11.1.json"
    oracle_manifest = json.loads(oracle_manifest_path.read_text(encoding="utf-8"))
    pinned_commit = oracle_manifest["pinned_commit"]

    oracle_git = git_revision(args.oracle_checkout)
    candidate_git = git_revision(project_root)

    try:
        import os
        github_run_id = os.environ.get("GITHUB_RUN_ID", "local")
        github_sha = os.environ.get("GITHUB_SHA", candidate_git["commit"])
    except Exception:
        github_run_id, github_sha = "local", candidate_git["commit"]

    build_env = {
        "github_run_id": github_run_id,
        "github_sha": github_sha,
        "build_env_txt": None,
    }
    build_env_path = output_dir / "build-env.txt"
    if build_env_path.is_file():
        build_env["build_env_txt"] = build_env_path.read_text(
            encoding="utf-8", errors="replace")

    adapter = parse_adapter(output_dir / "gpu-preflight.log")

    # Inputs that determine the small gate (hashes for provenance).
    input_files = [
        project_root / "reference" / "flexpart-11.1.json",
        project_root / "src" / "bin" / "fortran-validation.rs",
        project_root / "tests" / "integration" / "software_advection.rs",
        project_root / "src" / "bin" / "gpu-preflight.rs",
    ]
    input_sha256 = {}
    for path in input_files:
        digest = hash_if_exists(path)
        if digest:
            try:
                key = str(path.relative_to(project_root))
            except ValueError:
                key = str(path)
            input_sha256[key] = digest

    output_files = [
        output_dir / "candidate-output.json",
        output_dir / "candidate-run.log",
        output_dir / "sw-wgpu-advection.log",
        output_dir / "gpu-preflight.log",
        output_dir / "oracle-verify.log",
        output_dir / "oracle-build.log",
    ]
    output_sha256 = {}
    for path in output_files:
        digest = hash_if_exists(path)
        if digest:
            output_sha256[path.name] = digest

    oracle_executable = Path(args.oracle_checkout) / "src" / "FLEXPART"
    oracle_executable_sha = hash_if_exists(oracle_executable)

    etadot_oracle = None
    if args.flex_extract_checkout:
        flex_extract_manifest = project_root / "reference" / "flex-extract.json"
        flex_extract_commit = None
        flex_extract_dirty = None
        if flex_extract_manifest.is_file():
            pinned = json.loads(
                flex_extract_manifest.read_text(encoding="utf-8")
            )["pinned_commit"]
        else:
            pinned = None
        if Path(args.flex_extract_checkout).is_dir():
            revision = git_revision(args.flex_extract_checkout)
            flex_extract_commit = revision["commit"]
            flex_extract_dirty = revision["dirty"]
        status_path = Path(args.output_dir, "flex-extract-oracle-status.txt")
        etadot_status = "NOT_WIRED"
        if status_path.is_file():
            etadot_status = status_path.read_text(encoding="utf-8").strip()
        if etadot_status not in {"PASS", "FAIL", "NOT_WIRED", "NOT_RUN", "RUNNING"}:
            etadot_status = "FAIL"
        if etadot_status == "RUNNING":
            etadot_status = "FAIL"
        comparison_path = Path(
            args.output_dir, "flex-extract-oracle", "comparison-report.json"
        )
        if etadot_status == "PASS":
            try:
                comparison = json.loads(comparison_path.read_text(encoding="utf-8"))
                if comparison.get("status") != "PASS":
                    etadot_status = "FAIL"
            except Exception:
                etadot_status = "FAIL"
        etadot_oracle = {
            "manifest": "reference/flex-extract.json",
            "pinned_commit": pinned,
            "actual_commit": flex_extract_commit,
            "worktree_clean": (not flex_extract_dirty
                               if flex_extract_dirty is not None else None),
            "status": etadot_status,
        }

    candidate_binary = project_root / "target" / "release" / "fortran-validation"
    # Windows executable has .exe suffix; accept either.
    candidate_executable_sha = hash_if_exists(candidate_binary)
    if candidate_executable_sha is None:
        candidate_executable_sha = hash_if_exists(
            project_root / "target" / "release" / "fortran-validation.exe")

    # Candidate output summary (structural only, no scientific thresholds).
    candidate_summary = {}
    candidate_output = output_dir / "candidate-output.json"
    if candidate_output.is_file():
        try:
            data = json.loads(candidate_output.read_text(encoding="utf-8"))
            candidate_summary = {
                "total_particles_active": data.get("total_particles_active"),
                "total_steps": data.get("total_steps"),
                "window_start_epoch_seconds": data.get("window_start_epoch_seconds"),
                "window_end_epoch_seconds": data.get("window_end_epoch_seconds"),
                "averaging_seconds": data.get("averaging_seconds"),
                "sampling_seconds": data.get("sampling_seconds"),
                "samples": data.get("samples"),
            }
        except Exception as error:
            candidate_summary = {"parse_error": str(error)}

    allowlist = [token for token in args.allowlist.split() if token]
    cases = []
    for case_id in allowlist:
        log_name = {
            "SW-WGPU-ADVECTION-001": "sw-wgpu-advection.log",
            "SYNTHETIC-UNIFORM-WIND-SMOKE": "candidate-run.log",
        }.get(case_id, None)
        log_path = output_dir / log_name if log_name else None
        cases.append({
            "case_id": case_id,
            "status": "PASS" if args.status == "TECHNICAL_PASS" else "FAIL",
            "log": log_name,
            "log_present": bool(log_path and log_path.is_file()),
            "note": "Technical execution only; no scientific parity verdict.",
        })

    # Corpus runner coordination: owned by a parallel track
    # (scripts/run-corpus.sh, docs/corpus-matrix.md). Record its presence
    # without depending on it; never report unwired cases as PASS. The
    # scientific metric format is owned by scripts/evaluate/evaluate_case.py
    # (docs/evaluation.md) and is not applied here.
    corpus_runner = project_root / "scripts" / "run-corpus.sh"
    evaluate_runner = project_root / "scripts" / "evaluate" / "evaluate_case.py"
    if corpus_runner.is_file():
        corpus_runner_status = "PRESENT_NOT_WIRED_IN_THIS_GATE"
    else:
        corpus_runner_status = "NOT_FOUND_EXPECTED_FROM_PARALLEL_TRACK"
    if evaluate_runner.is_file():
        evaluate_runner_status = "PRESENT_NOT_APPLIED_IN_THIS_GATE"
    else:
        evaluate_runner_status = "NOT_FOUND_EXPECTED_FROM_PARALLEL_TRACK"
    pending = [
        {"case_id": case_id, "status": "NOT_WIRED",
         "note": "Bound only once its runner and metric format are stable."}
        for case_id in KNOWN_FUTURE_CORPUS
        if case_id not in allowlist
    ]

    report = {
        "schema_version": SCHEMA_VERSION,
        "gate_id": GATE_ID,
        "generated_at_utc": datetime.datetime.now(
            datetime.timezone.utc).isoformat(),
        "status": args.status,
        "scientific_verdict": "NOT_EVALUATED",
        "failure": args.failure,
        "oracle": {
            "pinned_commit": pinned_commit,
            "actual_commit": oracle_git["commit"],
            "worktree_clean": (not oracle_git["dirty"]
                               if oracle_git["dirty"] is not None else None),
            "executable_sha256": oracle_executable_sha,
            "manifest": "reference/flexpart-11.1.json",
        },
        "etadot_oracle": etadot_oracle,
        "candidate": {
            "revision": candidate_git["commit"],
            "worktree_dirty": candidate_git["dirty"],
            "executable_sha256": candidate_executable_sha,
        },
        "build_env": build_env,
        "adapter": adapter,
        "cases": cases,
        "corpus_runner": corpus_runner_status,
        "evaluate_runner": evaluate_runner_status,
        "pending_corpus_cases": pending,
        "input_sha256": input_sha256,
        "output_sha256": output_sha256,
        "candidate_summary": candidate_summary,
        "particles": args.particles,
        "seeds": {
            "philox_key": ["0xDECAFBAD", "0x12345678"],
            "initial_philox_counter": [0, 0, 0, 0],
            "note": ("Fixed default key/counter for the small smoke only; "
                     "stochastic comparisons require at least 10 independent "
                     "seeds per Issue #6 and are NOT covered by this gate."),
        },
        "scientific_thresholds": {
            "status": "NOT_APPLIED",
            "note": ("Versioned scientific thresholds are owned by the "
                     "scientific-metrics track and are only applied when "
                     "inputs and metrics are demonstrably suitable."),
        },
        "notes": ("Technical gate only. TECHNICAL_PASS proves a pinned clean "
                  "oracle build, a real software-WGPU adapter, an analytical "
                  "displacement check, and a small candidate smoke with "
                  "recorded provenance. It does not establish scientific "
                  "parity and does not close Issue #6."),
    }

    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"CI gate report: {output} ({args.status})")


if __name__ == "__main__":
    main()
