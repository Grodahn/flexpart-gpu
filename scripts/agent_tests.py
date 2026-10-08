#!/usr/bin/env python3
"""Finite diagnostic test selections; PASS never implies scientific parity."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

import agent_validation

REPO = Path(__file__).resolve().parents[1]
SCHEMA = "flexpart-gpu.agent-tests-summary.v1"
RESIDENT_REPORT = Path("target/ci-gate/meteorology-resident/report.json")
RESIDENT_SHADERS = (
    "particle_query", "resident_status_reset", "resident_query_adapter",
    "horizontal_interpolation", "vertical_sample", "temporal_interpolation",
    "resident_sample_status", "resident_fixture_consumer",
)
RESIDENT_CASES = {
    "surface-active-1", "surface-active-3", "surface-active-4", "inactive-nonfinite-lane",
    "negative-signed-cell-memory-safe", "reuse-after-fatal-status-reset", "nonfinite-fraction",
    "nonfinite-height", "invalid-fraction", "outside-cell", "nonperiodic-east-overshoot",
    "north-overshoot", "nonperiodic-exact-east-north", "split-cell-fraction-survives-endpoint-rounding",
    "fatal-across-workgroups", "reuse-with-empty-active-prefix",
    "model-distinct-compatible-profiles-lower-interior-upper",
    "within-stencil-differing-geometry-owned-by-118",
    "pinned-periodic-edge-0", "pinned-periodic-edge-1", "pinned-periodic-edge-2",
}
ORDER_MARKER = "TIMELOOP-ORDER-126: WGSL transport/deposition/decay executed"


class EvidenceBlocked(ValueError):
    """An existing test lacks the execution proof required by the selected profile."""


@dataclass(frozen=True)
class Stage:
    """Explicit command and existing evidence contract for one diagnostic stage."""
    name: str
    command: tuple[str, ...]
    evidence: str = "counts"
    environment: tuple[tuple[str, str], ...] = ()


def cargo(name, *args, evidence="counts", environment=()):
    """Keep Cargo's ordinary cache and expose captured test markers."""
    selectors = tuple(a for a in args if a != "--exact")
    exact = ("--exact",) if "--exact" in args else ()
    return Stage(name, ("cargo", "test", *selectors, "--", *exact, "--nocapture", "--test-threads=1"), evidence, environment)


def python_test(name, path):
    """Run an existing Python owner without discovering guessed tests."""
    return Stage(name, (sys.executable, path))


def selections():
    """Return the finite v1 registry shared by listing and execution."""
    resident = cargo("resident", "--test", "meteorology_resident", evidence="resident")
    advection = cargo("advection", "--test", "integration",
        "software_advection::test_sw_wgpu_advection_001_constant_wind_displacement", "--exact",
        evidence="advection")
    preflight = Stage("preflight", ("cargo", "run", "--bin", "gpu-preflight", "--",
        "--software", "--json-output", "{run}/preflight.json"), "preflight")
    order = cargo("order", "--test", "forward_timeloop",
        "test_forward_timeloop_transport_precedes_deposition_and_reports_precede_advance", "--exact", evidence="order")
    wrapper = python_test("wrapper", "scripts/test_agent_validation.py")
    paired = Stage("paired", (sys.executable, "scripts/agent_validation.py", "--check", "comparison",
        "--case", "ADV-ANA-001", "--output-dir", "{run}/paired"), "paired")
    return {
        "meteorology-resident": {
            "focused": (resident,),
            "domain": (resident, cargo("resident-corners", "--lib", "gpu::meteorology::resident::tests")),
        },
        "transport-advection": {"focused": (advection,), "domain": (advection, paired)},
        "simulation": {
            "focused": (preflight, order),
            "domain": (preflight, cargo("forward", "--test", "forward_timeloop", evidence="forward"),
                cargo("forward-validation", "--test", "forward_timeloop", evidence="forward",
                    environment=(("FLEXPART_GPU_VALIDATION", "1"),)),
                cargo("backward", "--test", "backward_timeloop", evidence="legacy-backward")),
        },
        "validation-provenance": {
            "focused": (wrapper,),
            "domain": (wrapper,
                python_test("input-audit", "scripts/corpus/test_audit_corpus_inputs.py"),
                python_test("manifest", "scripts/corpus/test_write_corpus_manifest_v1.py"),
                python_test("provenance", "scripts/provenance/test_run_provenance.py"),
                cargo("case", "--lib", "validation::case::"),
                cargo("case-facade", "--test", "validation_case_contract")),
        },
    }


def require(condition, message):
    """Reject missing or malformed mandatory execution evidence."""
    if not condition:
        raise ValueError(message)


def read_json(path):
    """Read an object; empty or non-object evidence cannot establish success."""
    value = json.loads(path.read_text(encoding="utf-8"))
    require(isinstance(value, dict) and value, f"invalid evidence object: {path}")
    return value


def adapter(value):
    """Require the configured software WGSL identity, never a CPU replacement."""
    require(isinstance(value, dict), "missing adapter")
    require(value.get("adapter_class") == "software_wgsl", "required software adapter missing")
    require(all(isinstance(value.get(k), str) and value[k].strip() for k in ("name", "backend", "device_type")), "incomplete adapter identity")
    return value


def counts(output):
    """Parse stable libtest/unittest summaries without counting filtered tests."""
    rust = re.findall(r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored", output)
    if rust:
        values = [tuple(map(int, row)) for row in rust]
        return {"executed": sum(p + f for p, f, _ in values), "failed": sum(f for _, f, _ in values), "skipped": sum(s for _, _, s in values)}
    python = re.search(r"Ran (\d+) tests? in [\d.]+s", output)
    if python:
        def number(name):
            match = re.search(rf"\b{name}=(\d+)", output)
            return int(match[1]) if match else 0
        skipped = number("skipped")
        return {"executed": int(python[1]) - skipped, "failed": number("failures") + number("errors") + number("unexpected successes"), "skipped": skipped}
    return {"executed": None, "failed": None, "skipped": None}


def validate_evidence(stage, output, run, revision):
    """Audit existing reports/markers; numerical comparisons stay with their owners."""
    kind = stage.evidence
    if kind == "resident":
        source = REPO / RESIDENT_REPORT
        report = read_json(source)
        require(report.get("schema") == {"id": "flexpart-gpu.meteorology-resident", "version": 1}, "resident schema")
        require(report.get("passed") is True and report.get("revision") == revision, "resident verdict/revision")
        identity = adapter(report.get("adapter"))
        shaders = "\n".join((REPO / f"src/shaders/{p}.wgsl").read_text(encoding="utf-8") for p in RESIDENT_SHADERS)
        require(report.get("shader_bundle_sha256") == hashlib.sha256(shaders.encode()).hexdigest(), "resident shader identity")
        rows = report.get("cases", [])
        require(isinstance(rows, list) and len(rows) == len(RESIDENT_CASES), "resident case coverage")
        require({r["id"] for r in rows} == RESIDENT_CASES, "resident case identities")
        require(report.get("static_grid_context_time_rejection") is True, "resident rejection evidence")
        for row in rows:
            require(row["passed"] is True and row["submission_count"] == 1 and row["intermediate_d2h_count"] == 0 and row["host_prepare_sample"] is False, "resident execution/residency")
            metadata = row["metadata"]
            require(metadata["component_count"] == metadata["lane_stride"] == 1, "resident layout")
            require(len(row["status"]["lanes"]) == len(row["query_inputs"]) == metadata["capacity"], "resident lane evidence")
            require(re.fullmatch(r"[0-9a-f]{64}", row["input_sha256"]) is not None, "resident input identity")
            require(row["order"] == ["device_status_reset", "particle_producer", "resident_adapter", "canonical_sampling", "device_result_status", "status_guarded_fixture_consumer"], "resident order")
        cases = {row["id"]: row for row in rows}
        require(cases["negative-signed-cell-memory-safe"]["status"]["lanes"][1] == 3, "resident negative cell status")
        require(cases["within-stencil-differing-geometry-owned-by-118"]["status"]["lanes"][1] == 7, "resident incompatible geometry status")
        require(cases["fatal-across-workgroups"]["status"]["fatal"] is True, "resident fatal aggregation")
        require(cases["reuse-after-fatal-status-reset"]["status"] == {"fatal": False, "lanes": [2, 2, 2, 2]}, "resident status reset")
        retained = run / "resident-report.json"
        retained.write_bytes(source.read_bytes())
        return {"adapter": identity, "paths": [str(retained)]}
    if kind == "preflight":
        path = run / "preflight.json"
        report = read_json(path)
        require(report.get("schema") == {"id": "flexpart-gpu.gpu-preflight", "version": 1}, "preflight schema")
        require(report.get("status") == "passed" and report.get("failure") is None and report.get("skip_reason") is None, "preflight skipped/failed")
        inner = report["report"]
        identity = adapter(inner["adapter"])
        smoke = inner["smoke_test"]
        require(smoke["status"] == "passed" and smoke["actual_value"] == smoke["expected_value"], "preflight smoke")
        return {"adapter": identity, "paths": [str(path)]}
    if kind == "advection":
        match = re.search(r"SW-WGPU-ADVECTION-001: adapter=(.+) backend=(\w+) type=Cpu software=true", output)
        require(match is not None and match[1].strip(), "missing advection adapter identity")
        require("SW-WGPU-ADVECTION-001: software_adapter=true" in output and "SW-WGPU-ADVECTION-001: east=" in output, "missing WGSL displacement markers")
        return {"adapter": {"name": match[1], "backend": match[2], "adapter_class": "software_wgsl"}, "paths": []}
    if kind in ("order", "forward"):
        require(ORDER_MARKER in output, "missing required WGSL order regression")
        if kind == "forward":
            require("TIMELOOP-DEFERRED-126: WGSL output" in output and "TIMELOOP-ERROR-126:" in output, "missing forward device regressions")
    if kind == "legacy-backward":
        raise EvidenceBlocked("backward_timeloop can return early on NoAdapter without an execution marker; prerequisite: owner-provided backward device evidence (tests unchanged)")
    if kind == "paired":
        summary = read_json(run / "paired/summary.json")
        require(summary.get("schema") == "flexpart-gpu.agent-validation-summary.v1", "paired schema")
        require(summary.get("state") == "PASS" and summary.get("check") == "comparison" and summary.get("case_id") == "ADV-ANA-001", "paired execution")
        pin = read_json(REPO / "reference/flexpart-11.1.json")["pinned_commit"]
        require(summary["candidate"]["revision"] == revision and summary["oracle"]["pinned_commit"] == pin, "paired revision identity")
        require(summary.get("scientific_verdict") == "DIAGNOSTIC_NO_PARITY_VERDICT", "unexpected paired scientific verdict")
        require(summary.get("stages") and all(s["exit_code"] == 0 for s in summary["stages"]), "incomplete paired stages")
        required = {"candidate", "oracle", "input-audit", "comparison", "manifest"}
        require(required <= {s["stage"] for s in summary["stages"]}, "missing paired stages")
        require(summary.get("evidence"), "absent paired evidence")
        for path in summary["evidence"]:
            p = Path(path)
            require(p.exists() and (any(p.iterdir()) if p.is_dir() else p.stat().st_size > 0), f"empty paired evidence: {path}")
        read_json(run / "paired/comparison-report.json")
        read_json(run / "paired/run-manifest.json")
        return {"scientific_verdict": summary["scientific_verdict"], "paths": [summary["summary"], *summary["evidence"]]}
    return {"paths": []}


@contextmanager
def evidence_lock(stages):
    """Protect legacy fixed report writers without deleting any retained evidence."""
    kind = next((s.evidence for s in stages if s.evidence in ("resident", "paired")), None)
    path = REPO / "target/agent-tests" / f"{kind}.lock"
    descriptor = None
    if kind:
        path.parent.mkdir(parents=True, exist_ok=True)
        descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
    try:
        yield
    finally:
        if descriptor is not None:
            os.close(descriptor)
            path.unlink()


def file_stamp(path):
    """Detect unchanged legacy evidence after a skipped or empty invocation."""
    return (path.stat().st_mtime_ns, path.stat().st_size, hashlib.sha256(path.read_bytes()).hexdigest()) if path.exists() else None


def stage_result(stage, run, revision):
    """Describe even unstarted stages with the same versioned audit fields."""
    command = [part.replace("{run}", str(run)) for part in stage.command]
    return {"stage": stage.name, "state": "NOT_RUN", "command": command,
        "environment": {"FLEXPART_GPU_SOFTWARE": "1", "FLEXPART_GPU_CANDIDATE_REVISION": revision, **dict(stage.environment)},
        "exit_code": None, "elapsed_seconds": 0, "output_bytes": 0,
        "counts": {"executed": None, "failed": None, "skipped": None}}


def run_stages(domain, level, stages, run, revision):
    """Retain complete bytes and stop at the first failed command/evidence check."""
    started = time.monotonic()
    results = []
    final = "PASS"
    exit_code = 0
    environment = os.environ.copy()
    environment["FLEXPART_GPU_SOFTWARE"] = "1"
    environment["FLEXPART_GPU_CANDIDATE_REVISION"] = revision
    for stage in stages:
        command = [part.replace("{run}", str(run)) for part in stage.command]
        result = stage_result(stage, run, revision)
        results.append(result)
        if final != "PASS":
            continue
        log = run / f"{stage.name}.log"
        result["log"] = str(log)
        clock = time.monotonic()
        output = ""
        try:
            previous = file_stamp(REPO / RESIDENT_REPORT) if stage.evidence == "resident" else None
            with log.open("wb") as stream:
                process = subprocess.run(command, cwd=REPO, env={**environment, **dict(stage.environment)}, stdout=stream, stderr=subprocess.STDOUT, check=False)
            code = process.returncode
            result["exit_code"] = code
            output = log.read_bytes().decode("utf-8", errors="replace")
            result["counts"] = counts(output)
            state = "PASS"
            diagnostic = None
            if code:
                state = agent_validation.classify_failure(output)
                if re.search(r"(?:no suitable GPU adapter|required software WGSL adapter not found|NoAdapter|GPU adapter.*not found)", output, re.I):
                    state = "BLOCKED"
                # Preserve a delegated blocked/error state, even though its CLI returns 1.
                if stage.evidence == "paired" and (run / "paired/summary.json").exists():
                    delegated = read_json(run / "paired/summary.json").get("state")
                    if delegated in ("FAIL", "BLOCKED", "ERROR"):
                        state = delegated
            else:
                require(not re.search(r"skipping test|skipped.*(?:GPU|WGSL|adapter)|no GPU adapter", output, re.I), "required execution was skipped")
                observed = result["counts"]
                if stage.evidence not in ("preflight", "paired"):
                    if observed["skipped"]:
                        result["state"] = "SKIPPED"
                        raise ValueError("selected tests were skipped")
                    require(observed["executed"] is not None and observed["executed"] > 0, "zero tests or missing test summary")
                    require(observed["failed"] == 0, "failed tests despite exit zero")
                if stage.evidence == "resident":
                    require(file_stamp(REPO / RESIDENT_REPORT) != previous, "missing fresh resident evidence")
                result["evidence"] = validate_evidence(stage, output, run, revision)
        except OSError as error:
            not_started = result["exit_code"] is None
            code = (127 if isinstance(error, FileNotFoundError) else 1) if not_started else (result["exit_code"] or 1)
            state = "BLOCKED" if not_started and isinstance(error, FileNotFoundError) else "ERROR"
            diagnostic = str(error)
            with log.open("ab") as stream:
                stream.write((diagnostic + "\n").encode())
            output = log.read_bytes().decode("utf-8", errors="replace")
        except EvidenceBlocked as error:
            code, state, diagnostic = result["exit_code"] or 1, "BLOCKED", str(error)
        except (ValueError, KeyError, TypeError, AttributeError, IndexError) as error:
            code, state, diagnostic = result["exit_code"] or 1, "ERROR", str(error)
        result["elapsed_seconds"] = round(time.monotonic() - clock, 3)
        result["output_bytes"] = log.stat().st_size
        if result["state"] != "SKIPPED":
            result["state"] = state
        if state != "PASS":
            final, exit_code = state, code
            result["diagnostic"] = diagnostic or "subprocess failed"
            lines = output.splitlines()
            assertions = [line[:500] for line in lines if re.search(r"assert|panicked|FAILED|Error|error:", line)][:8]
            result["assertions"] = assertions
            result["diagnostic_tail"] = agent_validation.bounded_tail(output)
    return {"schema": SCHEMA, "domain": domain, "profile": level, "state": final,
        "exit_code": exit_code, "revision": revision, "scientific_verdict": next((r.get("evidence", {}).get("scientific_verdict") for r in results if r.get("evidence", {}).get("scientific_verdict")), "NOT_EVALUATED"),
        "elapsed_seconds": round(time.monotonic() - started, 3),
        "counts": {key: sum(r["counts"][key] or 0 for r in results) for key in ("executed", "failed", "skipped")},
        "output_bytes": sum(r["output_bytes"] for r in results), "stages": results,
        "summary": str(run / "summary.json")}


def emit(summary):
    """Persist the full summary and print exactly one measured JSON line."""
    summary["terminal_bytes"] = 0
    while True:
        line = json.dumps(summary, separators=(",", ":"), ensure_ascii=True) + "\n"
        size = len(line.encode("utf-8"))
        if size == summary["terminal_bytes"]:
            break
        summary["terminal_bytes"] = size
    Path(summary["summary"]).write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    if hasattr(sys.stdout, "buffer"):
        sys.stdout.buffer.write(line.encode("utf-8"))
        sys.stdout.buffer.flush()
    else:
        sys.stdout.write(line)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--domain", choices=selections())
    parser.add_argument("--level", choices=("focused", "domain"), default="focused")
    parser.add_argument("--list", action="store_true", help="List exact commands; {run} is a fresh run directory.")
    args = parser.parse_args()
    if args.list:
        print(json.dumps({d: {p: [{"stage": s.name, "command": s.command, "evidence": s.evidence, "environment": dict(s.environment)} for s in stages] for p, stages in profiles.items()} for d, profiles in selections().items()}, separators=(",", ":")))
        return 0
    if not args.domain:
        parser.error("--domain is required unless --list is used")
    root = REPO / "target/agent-tests" / args.domain
    root.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix=time.strftime("%Y%m%dT%H%M%S-"), dir=root)).resolve()
    stages = selections()[args.domain][args.level]
    started = time.monotonic()
    revision = "unknown"
    try:
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True, stderr=subprocess.PIPE).strip()
        with evidence_lock(stages):
            summary = run_stages(args.domain, args.level, stages, run, revision)
    except (OSError, subprocess.SubprocessError) as error:
        summary = {"schema": SCHEMA, "domain": args.domain, "profile": args.level, "state": "BLOCKED", "exit_code": 1,
            "diagnostic_tail": agent_validation.bounded_tail(str(error)), "failed_stage": "setup",
            "revision": revision, "elapsed_seconds": round(time.monotonic() - started, 3),
            "counts": {"executed": 0, "failed": 0, "skipped": 0}, "output_bytes": 0,
            "scientific_verdict": "NOT_EVALUATED",
            "stages": [stage_result(s, run, revision) for s in stages], "summary": str(run / "summary.json")}
    emit(summary)
    code = summary["exit_code"]
    if code < 0 and os.name != "nt":
        import signal
        signal.signal(-code, signal.SIG_DFL)
        os.kill(os.getpid(), -code)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
