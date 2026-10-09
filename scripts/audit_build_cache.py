#!/usr/bin/env python3
"""Measure full cold/warm/clean technical gates without reusing scientific results."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import time

from oracle_build_cache import atomic_json


def audit(root: Path, output: Path) -> None:
    """Retain each complete gate invocation and compare its authoritative fixtures."""
    output.mkdir(parents=True, exist_ok=False)
    gate = root / "target/ci-gate"
    # Only build records are invalidated to measure a compilation-cache miss.
    # Docker layers and unrelated Cargo caches remain available.
    (root / "target/oracle-cache/build.json").unlink(missing_ok=True)
    for name in ("vertical-column", "interpolation", "w-production-oracle"):
        (gate / name / "oracle-build/build.json").unlink(missing_ok=True)
    rows = []
    fixtures = {}
    for name, extra in (("cold", []), ("warm", []), ("clean", ["--clean"])):
        log = output / f"{name}.log"
        started = time.monotonic()
        with log.open("wb") as stream:
            result = subprocess.run(["bash", "scripts/ci-gate.sh", "--particles", "1000",
                                     "--require-flex-extract-oracle", *extra], cwd=root,
                                    stdout=stream, stderr=subprocess.STDOUT)
        row = {"mode": name, "elapsed_seconds": time.monotonic() - started,
               "exit_code": result.returncode, "terminal_bytes": log.stat().st_size}
        rows.append(row)
        atomic_json(output / "measurements.json", {"runs": rows})
        # Preserve oracle-run symlinks, including intentionally dangling build links.
        shutil.copytree(gate, output / name, symlinks=True)
        if result.returncode:
            raise RuntimeError(f"{name} technical gate failed; see {log}")
        status = json.loads((gate / "oracle-build-status.json").read_text())
        expected = "REUSED" if name == "warm" else "REBUILT"
        if status["status"] != expected:
            raise ValueError(f"{name}: expected {expected}, got {status['status']}")
        row["oracle_build_status"] = status["status"]
        build = json.loads((gate / "oracle-build.json").read_text())
        row["executable_sha256"] = build["oracle_executable_sha256"]
        row["cache_key"] = status["cache_key"]
        # The gate itself checks all comparisons, provenance, symbols, pristine
        # checkout, GPU adapter and test markers freshly. These exact normative
        # payloads add cross-run equivalence rather than replace those checks.
        current = {}
        for relative in ("interpolation/contract-v1.json", "w-production-oracle/w-production-oracle-v1.json",
                         "vertical-column/routine-oracle-output.txt",
                         "vertical-column/real-routine-oracle-output.txt",
                         "vertical-column/conformance-output.txt",
                         "vertical-column/real-conformance-output.txt"):
            payload = (gate / relative).read_bytes()
            if not payload:
                raise ValueError(f"missing scientific payload: {relative}")
            current[relative] = hashlib.sha256(payload).hexdigest()
        if fixtures and current != fixtures:
            raise ValueError(f"{name}: scientific payload drift")
        fixtures = current
        row["scientific_payload_sha256"] = current
        row["direct_build_hits"] = log.read_text(errors="replace").count("Direct oracle build: VERIFIED_REUSE")
        row["direct_build_rebuilds"] = log.read_text(errors="replace").count("Direct oracle build: REBUILT")
        if row["direct_build_rebuilds"] != (0 if name == "warm" else 3):
            raise ValueError(f"{name}: unexpected direct-driver build count")
        if row["direct_build_hits"] != (10 if name == "warm" else 7):
            raise ValueError(f"{name}: missing direct-driver cache dispositions")
        row["docker_image_builds"] = 0 if name == "warm" else 1
        row["make_clean_invocations"] = 0 if name == "warm" else 1
        build_log = (gate / "oracle-build.log").read_text(errors="replace")
        row["full_fortran_compiler_commands"] = 0 if name == "warm" else sum(
            line.startswith("gfortran ") for line in build_log.splitlines())
        row["retained_build_log_bytes"] = (gate / "oracle-build.log").stat().st_size
        atomic_json(output / "measurements.json", {"runs": rows})
        print(json.dumps(row, separators=(",", ":")), flush=True)
    if rows[0]["cache_key"] != rows[1]["cache_key"]:
        raise ValueError("unchanged warm run did not reuse the exact cold identity")
    if len({row["executable_sha256"] for row in rows}) != 1:
        raise ValueError("cold/warm/clean executable drift")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("target/build-cache-audit"))
    args = parser.parse_args()
    audit(Path(__file__).resolve().parents[1], args.output.resolve())
