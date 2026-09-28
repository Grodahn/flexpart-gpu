#!/usr/bin/env python3
"""Compact agent-facing entry point for the existing corpus validation path.

Successful runs print one JSON object. Complete subprocess output and all
scientific artifacts remain on disk. Use ``--verbose`` to mirror full stage
logs to the terminal, or ``--clean`` to force the pinned Docker/oracle build.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
DEFAULT_CASE = "ADV-ANA-001"
MAX_DIAGNOSTIC_LINES = 30
MAX_DIAGNOSTIC_LINE_CHARS = 500
ORACLE_CALIBRATION_CASES = {
    "DRY-007": "ADV-ANA-001",
    "WET-008": "ADV-ANA-001",
}
BLOCKED_MARKERS = (
    "docker is required",
    "docker daemon",
    "docker api",
    "permission denied while trying to connect",
    "not a git checkout",
    "fortran checkout not found",
    "could not find the pinned oracle",
)


def git_value(*arguments: str) -> str:
    """Read candidate identity while tolerating sandbox ownership boundaries."""
    resolved = REPO.resolve().as_posix()
    result = subprocess.run(
        ["git", "-c", f"safe.directory={resolved}", "-C", resolved, *arguments],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def focused_oracle_cases() -> set[str]:
    """Return cases supported by the existing synthetic paired runner."""
    index = json.loads(
        (REPO / "fixtures" / "corpus" / "corpus.json").read_text(encoding="utf-8")
    )
    cases_dir = REPO / "fixtures" / "corpus" / "cases"
    fortran_dir = REPO / "fixtures" / "corpus" / "fortran"
    return {
        entry["id"]
        for entry in index["cases"]
        if entry["status"] == "implemented"
        and (cases_dir / f"{entry['id']}.json").is_file()
        and (fortran_dir / entry["id"]).is_dir()
    }


def default_output_dir(case_id: str) -> Path:
    """Create a collision-resistant run directory while retaining prior logs."""
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    return REPO / "target" / "agent-validation" / case_id / f"{timestamp}-{os.getpid()}"


def create_output_dir(path: Path) -> Path:
    """Create an empty absolute output directory without overwriting prior logs."""
    resolved = path.resolve()
    if resolved.exists() and (not resolved.is_dir() or any(resolved.iterdir())):
        raise ValueError(f"output directory already exists and is not empty: {resolved}")
    resolved.mkdir(parents=True, exist_ok=True)
    return resolved


def commands_for(check: str, case_id: str, report: Path) -> list[tuple[str, list[str]]]:
    """Build the minimal stage list without duplicating validation semantics."""
    default_bash = "bash"
    windows_git_bash = Path(r"C:\Program Files\Git\bin\bash.exe")
    if os.name == "nt" and windows_git_bash.is_file():
        default_bash = str(windows_git_bash)
    bash = os.environ.get("BASH", default_bash)
    oracle = [bash, str(REPO / "scripts" / "run-corpus.sh"), "oracle", case_id]
    if check == "oracle":
        return [("oracle", oracle)]
    calibration_case = ORACLE_CALIBRATION_CASES.get(case_id)
    manifest = report.with_name("run-manifest.json")
    commands = [
        (
            "candidate",
            [bash, str(REPO / "scripts" / "run-corpus.sh"), "candidate", case_id, "10"],
        ),
    ]
    if calibration_case:
        commands.extend(
            [
                (
                    "oracle-calibration",
                    [
                        bash,
                        str(REPO / "scripts" / "run-corpus.sh"),
                        "oracle",
                        calibration_case,
                    ],
                ),
                (
                    "input-audit-calibration",
                    [
                        sys.executable,
                        str(REPO / "scripts" / "corpus" / "audit_corpus_inputs.py"),
                        "--oracle-dir",
                        str(REPO / "target" / "corpus" / "oracle"),
                        "--require-oracle",
                        "--case",
                        calibration_case,
                    ],
                ),
            ]
        )
    commands.extend(
        [
            ("oracle", oracle),
            (
                "input-audit",
                [
                    sys.executable,
                    str(REPO / "scripts" / "corpus" / "audit_corpus_inputs.py"),
                    "--candidate-dir",
                    str(REPO / "target" / "corpus" / "candidate"),
                    "--oracle-dir",
                    str(REPO / "target" / "corpus" / "oracle"),
                    "--require-oracle",
                    "--case",
                    case_id,
                ],
            ),
        ]
    )
    comparison = [
        sys.executable,
        str(REPO / "scripts" / "corpus" / "compare_corpus.py"),
        "--candidate-dir",
        str(REPO / "target" / "corpus" / "candidate"),
        "--oracle-dir",
        str(REPO / "target" / "corpus" / "oracle"),
        "--output",
        str(report),
        "--case",
        case_id,
    ]
    manifest_command = [
        sys.executable,
        str(REPO / "scripts" / "corpus" / "write_corpus_manifest.py"),
        "--output",
        str(manifest),
        "--corpus-index",
        str(REPO / "fixtures" / "corpus" / "corpus.json"),
        "--oracle-manifest",
        str(REPO / "reference" / "flexpart-11.1.json"),
        "--oracle-checkout",
        os.environ.get("FLEXPART_DIR", str(REPO.parent / "flexpart")),
        "--candidate-checkout",
        str(REPO),
        "--candidate-dir",
        str(REPO / "target" / "corpus" / "candidate"),
        "--oracle-dir",
        str(REPO / "target" / "corpus" / "oracle"),
        "--report",
        str(report),
        "--cases-dir",
        str(REPO / "fixtures" / "corpus" / "cases"),
        "--fortran-fixtures",
        str(REPO / "fixtures" / "corpus" / "fortran"),
        "--thresholds",
        str(REPO / "fixtures" / "corpus" / "thresholds.json"),
        "--meteo-dir",
        str(REPO / "target" / "corpus" / "meteo"),
        "--candidate-exe",
        str(
            REPO
            / "target"
            / "release"
            / ("corpus-run.exe" if os.name == "nt" else "corpus-run")
        ),
        "--oracle-exe",
        str(
            Path(os.environ.get("FLEXPART_DIR", str(REPO.parent / "flexpart")))
            / "src"
            / "FLEXPART"
        ),
        "--case",
        case_id,
    ]
    if calibration_case:
        comparison.extend(["--oracle-calibration-case", calibration_case])
        manifest_command.extend(["--oracle-dependency-case", calibration_case])
    commands.extend([("comparison", comparison), ("manifest", manifest_command)])
    return commands


def bounded_tail(
    output: str,
    lines: int = MAX_DIAGNOSTIC_LINES,
    line_chars: int = MAX_DIAGNOSTIC_LINE_CHARS,
) -> list[str]:
    """Keep failure diagnostics bounded by both line count and line length."""
    tail = []
    for line in output.splitlines()[-lines:]:
        if len(line) > line_chars:
            line = f"...{line[-(line_chars - 3):]}"
        tail.append(line)
    return tail


def classify_failure(output: str) -> str:
    """Distinguish absent prerequisites from an executed validation failure."""
    lowered = output.lower()
    if any(marker in lowered for marker in BLOCKED_MARKERS):
        return "BLOCKED"
    return "FAIL"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", choices=("oracle", "comparison"), default="comparison")
    parser.add_argument("--case", dest="case_id", default=DEFAULT_CASE)
    parser.add_argument("--clean", action="store_true", help="Force a no-cache image/oracle rebuild.")
    parser.add_argument("--verbose", action="store_true", help="Mirror complete stage logs to stderr.")
    parser.add_argument(
        "--output-dir",
        type=Path,
        help="Summary/log directory (default: a new run directory under target/agent-validation/<case>).",
    )
    args = parser.parse_args()

    supported_cases = focused_oracle_cases()
    if args.case_id not in supported_cases:
        parser.error(
            f"case {args.case_id!r} is not supported by the paired synthetic corpus runner; "
            f"choose one of: {', '.join(sorted(supported_cases))}"
        )

    try:
        output_dir = create_output_dir(
            args.output_dir or default_output_dir(args.case_id)
        )
    except (OSError, ValueError) as error:
        parser.error(str(error))
    report = output_dir / "comparison-report.json"
    summary_path = output_dir / "summary.json"
    environment = os.environ.copy()
    if args.clean:
        environment["ORACLE_REBUILD"] = "1"
    if args.verbose:
        environment["ORACLE_VERBOSE"] = "1"

    started = time.monotonic()
    stage_results = []
    oracle_cache_statuses = []
    cache_status_path = REPO / "target" / "corpus" / "oracle-build-status.json"
    state = "PASS"
    diagnostic_tail: list[str] = []
    mirrored_terminal_bytes = 0
    for stage_name, command in commands_for(args.check, args.case_id, report):
        stage_started = time.monotonic()
        try:
            completed = subprocess.run(
                command,
                cwd=REPO,
                env=environment,
                capture_output=True,
                text=True,
                errors="replace",
            )
            exit_code = completed.returncode
            combined = completed.stdout + completed.stderr
        except OSError as error:
            exit_code = 127
            combined = f"could not start {stage_name}: {error}\n"
        log_path = output_dir / f"{stage_name}.log"
        log_path.write_text(combined, encoding="utf-8")
        if args.verbose and combined:
            mirrored = combined if combined.endswith("\n") else f"{combined}\n"
            print(mirrored, file=sys.stderr, end="")
            mirrored_terminal_bytes += len(mirrored.encode("utf-8"))
        stage_result = {
            "stage": stage_name,
            "exit_code": exit_code,
            "elapsed_seconds": round(time.monotonic() - stage_started, 3),
            "log_bytes": len(combined.encode("utf-8")),
            "log": str(log_path.resolve()),
        }
        cache_status_error = None
        if exit_code == 0 and stage_name in ("oracle", "oracle-calibration"):
            try:
                cache_status = json.loads(cache_status_path.read_text(encoding="utf-8"))
                if (
                    not isinstance(cache_status, dict)
                    or cache_status.get("schema")
                    != "flexpart-gpu.oracle-build-status.v1"
                    or cache_status.get("status") not in ("REUSED", "REBUILT")
                ):
                    raise ValueError("unexpected schema or status")
                cache_metadata_source = Path(cache_status["metadata"])
                cache_log_source = Path(cache_status["log"])
                if not cache_metadata_source.is_absolute() or not cache_log_source.is_absolute():
                    raise ValueError("cache artifact paths must be absolute")
                retained_metadata = output_dir / f"{stage_name}-build.json"
                retained_build_log = output_dir / f"{stage_name}-build.log"
                shutil.copy2(cache_metadata_source, retained_metadata)
                shutil.copy2(cache_log_source, retained_build_log)
                cache_status = dict(cache_status)
                cache_status["metadata"] = str(retained_metadata.resolve())
                cache_status["log"] = str(retained_build_log.resolve())
                stage_result["build_cache"] = cache_status
                oracle_cache_statuses.append(cache_status)
            except (KeyError, OSError, ValueError) as error:
                cache_status_error = f"invalid cache status {cache_status_path}: {error}"
        stage_results.append(stage_result)
        if exit_code != 0:
            state = "ERROR" if exit_code == 127 else classify_failure(combined)
            diagnostic_tail = bounded_tail(combined)
            break
        if cache_status_error:
            state = "ERROR"
            diagnostic_tail = [cache_status_error]
            break
        if args.clean and stage_name == "oracle-calibration":
            environment["ORACLE_REBUILD"] = "0"

    reference = json.loads((REPO / "reference" / "flexpart-11.1.json").read_text(encoding="utf-8"))
    cache_status = next(
        (
            status
            for status in oracle_cache_statuses
            if status.get("status") == "REBUILT"
        ),
        oracle_cache_statuses[-1] if oracle_cache_statuses else None,
    )
    candidate_revision = "unknown"
    candidate_dirty = None
    try:
        candidate_revision = git_value("rev-parse", "HEAD")
        candidate_dirty = bool(git_value("status", "--porcelain"))
    except (OSError, subprocess.SubprocessError) as error:
        if state == "PASS":
            state = "ERROR"
            diagnostic_tail = [f"candidate revision unavailable: {error}"]

    successful_stages = {
        stage["stage"] for stage in stage_results if stage["exit_code"] == 0
    }
    evidence = []
    if "oracle" in successful_stages:
        evidence.append(
            str((REPO / "target" / "corpus" / "oracle" / args.case_id).resolve())
        )
    scientific_verdict = "NOT_EVALUATED"
    if args.check == "comparison":
        calibration_case = ORACLE_CALIBRATION_CASES.get(args.case_id)
        if calibration_case and "oracle-calibration" in successful_stages:
            evidence.append(
                str((REPO / "target" / "corpus" / "oracle" / calibration_case).resolve())
            )
        if "candidate" in successful_stages:
            evidence.append(
                str((REPO / "target" / "corpus" / "candidate" / args.case_id).resolve()),
            )
        if "comparison" in successful_stages:
            evidence.append(str(report.resolve()))
            try:
                report_data = json.loads(report.read_text(encoding="utf-8"))
                if not isinstance(report_data, dict):
                    raise ValueError("comparison report must contain a JSON object")
                scientific_verdict = report_data.get("status", "NOT_EVALUATED")
            except (OSError, ValueError) as error:
                state = "ERROR"
                diagnostic_tail = [f"comparison report unavailable: {error}"]
        if "manifest" in successful_stages:
            evidence.append(str(report.with_name("run-manifest.json").resolve()))

    summary = {
        "schema": "flexpart-gpu.agent-validation-summary.v1",
        "state": state,
        "check": args.check,
        "case_id": args.case_id,
        "candidate": {"revision": candidate_revision, "worktree_dirty": candidate_dirty},
        "oracle": {
            "kind": "FLEXPART-11.1-pristine",
            "pinned_commit": reference["pinned_commit"],
            "build_cache": cache_status,
        },
        "scientific_verdict": scientific_verdict,
        "elapsed_seconds": round(time.monotonic() - started, 3),
        "manual_commands": 1,
        "terminal_output": {
            "mode": "verbose-plus-json" if args.verbose else "compact-json",
            "bytes": mirrored_terminal_bytes,
        },
        "stages": stage_results,
        "evidence": evidence,
        "summary": str(summary_path.resolve()),
    }
    if diagnostic_tail:
        summary["diagnostic_tail"] = diagnostic_tail
    while True:
        compact = json.dumps(summary, separators=(",", ":")) + "\n"
        terminal_bytes = mirrored_terminal_bytes + len(compact.encode("utf-8"))
        if summary["terminal_output"]["bytes"] == terminal_bytes:
            break
        summary["terminal_output"]["bytes"] = terminal_bytes
    summary_path.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(compact, end="")
    raise SystemExit(0 if state == "PASS" else 1)


if __name__ == "__main__":
    main()
