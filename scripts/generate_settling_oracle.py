#!/usr/bin/env python3
"""Generate pinned FLEXPART 11.1 settling oracle fixtures for issue #35.

Runs the exact-formula Fortran harness (oracle/settling_oracle.f90) inside the
pinned ``flexpart-fortran:latest`` image (gfortran, same toolchain family as
``reference/flexpart-11.1.json``) and writes:

- fixtures/settling/canonical-vectors-v1.json (normalized GPU inputs)
- fixtures/settling/oracle-v1.json (oracle values + provenance)

The harness implements byte-identical logic to the pinned sources
(settling_mod sphere branch, part0 single-bin init, par_mod constants);
see oracle/settling_oracle.f90 for the line-level crosswalk. No Rust CPU
settling implementation participates.

Usage:
    python scripts/generate_settling_oracle.py [--no-docker] [--check]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[1]
ORACLE_DIR = PROJECT_ROOT / "oracle"
FIXTURE_DIR = PROJECT_ROOT / "fixtures" / "settling"
HARNESS = ORACLE_DIR / "settling_oracle.f90"
PINNED_MANIFEST = PROJECT_ROOT / "reference" / "flexpart-11.1.json"

# Canonical vectors spanning the declared #35 valid domain and regime
# boundaries (Cunningham slip below ~1 um, Stokes/Clift-Gauvin Re=0.02
# transition near 20-30 um, inertial settling above 50 um, plus
# temperature/air-density dependence and per-species density variations).
CANONICAL_VECTORS = [
    {"id": "SETTLE-001", "species": "AERO-01-fine", "diameter_um": 0.1, "density_kg_m3": 1000.0, "temperature_k": 293.15, "air_density_kg_m3": 1.2},
    {"id": "SETTLE-002", "species": "AERO-01-fine", "diameter_um": 0.5, "density_kg_m3": 1000.0, "temperature_k": 293.15, "air_density_kg_m3": 1.2},
    {"id": "SETTLE-003", "species": "AERO-02-std", "diameter_um": 1.0, "density_kg_m3": 1000.0, "temperature_k": 273.15, "air_density_kg_m3": 1.3},
    {"id": "SETTLE-004", "species": "AERO-03-dense", "diameter_um": 1.0, "density_kg_m3": 2000.0, "temperature_k": 310.0, "air_density_kg_m3": 1.0},
    {"id": "SETTLE-005", "species": "AERO-04-mid", "diameter_um": 5.0, "density_kg_m3": 1500.0, "temperature_k": 293.15, "air_density_kg_m3": 1.2},
    {"id": "SETTLE-006", "species": "AERO-02-std", "diameter_um": 10.0, "density_kg_m3": 1000.0, "temperature_k": 293.15, "air_density_kg_m3": 1.2},
    {"id": "SETTLE-007", "species": "AERO-05-cold-dense", "diameter_um": 10.0, "density_kg_m3": 2500.0, "temperature_k": 250.0, "air_density_kg_m3": 0.9},
    {"id": "SETTLE-008", "species": "AERO-02-std", "diameter_um": 20.0, "density_kg_m3": 1000.0, "temperature_k": 293.15, "air_density_kg_m3": 1.2},
    {"id": "SETTLE-009", "species": "AERO-02-std", "diameter_um": 30.0, "density_kg_m3": 1000.0, "temperature_k": 293.15, "air_density_kg_m3": 1.2},
    {"id": "SETTLE-010", "species": "AERO-06-coarse", "diameter_um": 50.0, "density_kg_m3": 1000.0, "temperature_k": 293.15, "air_density_kg_m3": 1.2},
    {"id": "SETTLE-011", "species": "AERO-06-coarse", "diameter_um": 100.0, "density_kg_m3": 1000.0, "temperature_k": 293.15, "air_density_kg_m3": 1.2},
    {"id": "SETTLE-012", "species": "AERO-07-thin-cold", "diameter_um": 10.0, "density_kg_m3": 1000.0, "temperature_k": 230.0, "air_density_kg_m3": 0.6},
    {"id": "SETTLE-013", "species": "AERO-08-light", "diameter_um": 5.0, "density_kg_m3": 800.0, "temperature_k": 300.0, "air_density_kg_m3": 1.0},
    {"id": "SETTLE-014", "species": "AERO-09-coarse-dense", "diameter_um": 50.0, "density_kg_m3": 2000.0, "temperature_k": 280.0, "air_density_kg_m3": 1.1},
]


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def pinned_revision() -> str:
    manifest = json.loads(PINNED_MANIFEST.read_text(encoding="utf-8"))
    return str(manifest["pinned_commit"])


def docker_posix(path: Path) -> str:
    # Docker Desktop on Windows accepts C:/... volume syntax.
    resolved = path.resolve()
    text = resolved.as_posix()
    return text


def run_oracle_harness(vectors: list[dict]) -> tuple[dict[str, float], str, str]:
    """Compile and run the Fortran harness in Docker; return outputs + hashes."""
    tmp = PROJECT_ROOT / "target" / "settling-oracle-tmp"
    tmp.mkdir(parents=True, exist_ok=True)
    input_path = tmp / "settling_inputs.txt"
    output_path = tmp / "settling_outputs.txt"
    with input_path.open("w", encoding="utf-8") as handle:
        for vector in vectors:
            handle.write(
                f"{vector['id']} {vector['diameter_um']} "
                f"{vector['density_kg_m3']} {vector['temperature_k']} "
                f"{vector['air_density_kg_m3']}\n"
            )
    repo_posix = docker_posix(PROJECT_ROOT)
    # Compile with the oracle toolchain profile (-O3, x86-64) and run.
    compile_cmd = [
        "docker", "run", "--rm",
        "-v", f"{repo_posix}:/workspace/flexpart-gpu",
        "-w", "/workspace/flexpart-gpu/oracle",
        "flexpart-fortran:latest",
        "bash", "-c",
        "gfortran -O3 -march=x86-64 -o /tmp/settling_oracle settling_oracle.f90 "
        "&& /tmp/settling_oracle /workspace/flexpart-gpu/target/settling-oracle-tmp/settling_inputs.txt "
        "/workspace/flexpart-gpu/target/settling-oracle-tmp/settling_outputs.txt "
        "&& sha256sum /tmp/settling_oracle",
    ]
    result = subprocess.run(compile_cmd, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        print(result.stdout, file=sys.stderr)
        print(result.stderr, file=sys.stderr)
        raise RuntimeError(f"oracle harness build/run failed: {result.returncode}")
    # Executable SHA: hash the harness binary via a second deterministic step.
    # The container binary is ephemeral; hash the source + flags instead and
    # record the container-reported sha256sum line for audit.
    print(result.stdout.strip())
    executable_sha = ""
    for token in result.stdout.strip().split():
        if len(token) == 64 and all(c in "0123456789abcdef" for c in token):
            executable_sha = token
            break
    if not executable_sha:
        raise RuntimeError("could not parse oracle executable SHA from container output")
    outputs: dict[str, float] = {}
    for line in output_path.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        outputs[parts[0]] = float(parts[1])
    if set(outputs) != {v["id"] for v in vectors}:
        raise RuntimeError(f"oracle outputs mismatch: {sorted(outputs)}")
    output_sha = sha256_file(output_path)
    return outputs, executable_sha, output_sha


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--no-docker", action="store_true", help="only write canonical vectors")
    parser.add_argument("--check", action="store_true", help="verify fixtures match harness")
    args = parser.parse_args()

    FIXTURE_DIR.mkdir(parents=True, exist_ok=True)
    canonical_path = FIXTURE_DIR / "canonical-vectors-v1.json"
    oracle_path = FIXTURE_DIR / "oracle-v1.json"

    canonical_doc = {
        "schema": {"id": "flexpart-gpu.settling-canonical-vectors", "version": 1},
        "pinned_flexpart_revision": pinned_revision(),
        "units": {
            "diameter_um": "micrometre",
            "particle_density_kg_m3": "kilogram_per_cubic_metre",
            "temperature_k": "kelvin",
            "air_density_kg_m3": "kilogram_per_cubic_metre",
            "settling_velocity_m_s": "metre_per_second (negative, downward)",
        },
        "valid_domain": {
            "diameter_um": [0.1, 100.0],
            "particle_density_kg_m3": [500.0, 3000.0],
            "temperature_k": [200.0, 320.0],
            "air_density_kg_m3": [0.4, 1.6],
        },
        "vectors": CANONICAL_VECTORS,
    }
    canonical_bytes = json.dumps(canonical_doc, indent=2, sort_keys=True).encode()
    canonical_sha = sha256_bytes(canonical_bytes)

    if args.no_docker:
        canonical_path.write_bytes(canonical_bytes)
        print(f"wrote {canonical_path} sha={canonical_sha}")
        return 0

    outputs, executable_sha, _ = run_oracle_harness(CANONICAL_VECTORS)
    oracle_doc = {
        "schema": {"id": "flexpart-gpu.settling-oracle", "version": 1},
        "pinned_implementation": "FLEXPART-11.1 settling_mod::get_settling (sphere)",
        "pinned_revision": pinned_revision(),
        "harness": "oracle/settling_oracle.f90",
        "harness_sha256": sha256_file(HARNESS),
        "executable_sha256": executable_sha,
        "canonical_sha256": canonical_sha,
        "values": [
            {"id": v["id"], "settling_velocity_m_s": outputs[v["id"]]}
            for v in CANONICAL_VECTORS
        ],
    }
    oracle_bytes = json.dumps(oracle_doc, indent=2, sort_keys=True).encode()
    oracle_sha = sha256_bytes(oracle_bytes)
    oracle_doc["output_sha256"] = oracle_sha
    oracle_bytes = json.dumps(oracle_doc, indent=2, sort_keys=True).encode()

    if args.check:
        existing = json.loads(oracle_path.read_text(encoding="utf-8"))
        if existing != oracle_doc:
            print("oracle fixture mismatch", file=sys.stderr)
            return 1
        print("oracle fixture matches harness")
        return 0

    canonical_path.write_bytes(canonical_bytes)
    oracle_path.write_bytes(oracle_bytes)
    print(f"wrote {canonical_path} sha={canonical_sha}")
    print(f"wrote {oracle_path} output_sha={oracle_sha} exe_sha={executable_sha}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
