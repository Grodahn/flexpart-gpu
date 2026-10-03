#!/usr/bin/env python3
"""Generate pinned FLEXPART 11.1 settling oracle fixtures for issue #35.

Links a state-only driver to pristine compiled FLEXPART routines inside the
existing ``flexpart-fortran:latest`` image and writes:

- fixtures/settling/canonical-vectors-v1.json (normalized GPU inputs)
- fixtures/settling/oracle-v1.json (oracle values + provenance)

The pinned clean checkout is verified before and after execution. A Git archive
is built in retained scratch; the reference checkout is never modified. Raw
outputs, source/object/build identities and the executable are retained.

Usage:
    python scripts/generate_settling_oracle.py --oracle-checkout ../flexpart [--check]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import subprocess
import sys
import tarfile
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
    {"id": "SETTLE-004", "species": "AERO-03-dense", "diameter_um": 1.0, "density_kg_m3": 2000.0, "temperature_k": 273.15, "air_density_kg_m3": 1.3},
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
# All corners exercise simultaneous extremes, rather than untested bounds.
for diameter in (0.1, 100.0):
    for density in (500.0, 3000.0):
        for temperature in (200.0, 320.0):
            for air_density in (0.4, 1.6):
                CANONICAL_VECTORS.append({
                    "id": f"SETTLE-{len(CANONICAL_VECTORS) + 1:03}",
                    "species": f"corner-{diameter}-{density}",
                    "diameter_um": diameter, "density_kg_m3": density,
                    "temperature_k": temperature, "air_density_kg_m3": air_density,
                })
# Tight bracket around the initial Re=0.02 switch at standard air conditions.
for diameter in (21.60, 21.62):
    CANONICAL_VECTORS.append({
        "id": f"SETTLE-{len(CANONICAL_VECTORS) + 1:03}", "species": "transition",
        "diameter_um": diameter, "density_kg_m3": 1000.0,
        "temperature_k": 293.15, "air_density_kg_m3": 1.2,
    })


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


def git(checkout: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-c", f"safe.directory={checkout.resolve().as_posix()}",
         "-C", str(checkout), *args], capture_output=True, text=True, check=True,
    ).stdout.strip()


def verify_checkout(checkout: Path) -> None:
    if git(checkout, "rev-parse", "HEAD") != pinned_revision():
        raise ValueError("oracle checkout is not at the pinned revision")
    if git(checkout, "status", "--porcelain", "--untracked-files=all"):
        raise ValueError("oracle checkout must be pristine")


def verify_scratch_sources(archive: Path, source: Path) -> None:
    """Require every archived source byte to remain pristine in the build tree."""
    with tarfile.open(archive) as snapshot:
        for member in snapshot.getmembers():
            if member.isfile():
                stream = snapshot.extractfile(member)
                if stream is None or (source / member.name).read_bytes() != stream.read():
                    raise ValueError(f"modified scratch oracle source: {member.name}")


def decode_outputs(data: bytes, vectors: list[dict]) -> dict[str, float]:
    outputs = {}
    for line in data.decode("utf-8").splitlines():
        parts = line.split()
        if len(parts) != 2 or parts[0] in outputs:
            raise ValueError("malformed or duplicate oracle output row")
        value = float(parts[1])
        if not math.isfinite(value) or value >= 0:
            raise ValueError("oracle velocity must be finite and downward")
        outputs[parts[0]] = value
    if set(outputs) != {v["id"] for v in vectors}:
        raise ValueError("oracle output ids do not exactly cover canonical vectors")
    return outputs


def audit_fixtures() -> None:
    """Fail closed on stale driver, raw output, inputs, or decoded velocities."""
    canonical_path = FIXTURE_DIR / "canonical-vectors-v1.json"
    canonical = json.loads(canonical_path.read_bytes())
    oracle = json.loads((FIXTURE_DIR / "oracle-v1.json").read_bytes())
    raw = (FIXTURE_DIR / "oracle-output-v1.txt").read_bytes()
    if canonical["schema"] != {"id": "flexpart-gpu.settling-canonical-vectors", "version": 1}:
        raise ValueError("unsupported canonical schema")
    if oracle["schema"] != {"id": "flexpart-gpu.settling-oracle", "version": 1}:
        raise ValueError("unsupported oracle schema")
    if canonical["vectors"] != CANONICAL_VECTORS:
        raise ValueError("canonical vectors differ from the declared coverage")
    if canonical["pinned_flexpart_revision"] != pinned_revision() or oracle["pinned_revision"] != pinned_revision():
        raise ValueError("unpinned fixture revision")
    if oracle["canonical_sha256"] != sha256_file(canonical_path) or oracle["output_sha256"] != sha256_bytes(raw):
        raise ValueError("stale canonical or raw output hash")
    if oracle["harness_sha256"] != sha256_bytes(HARNESS.read_bytes().replace(b"\r\n", b"\n")):
        raise ValueError("stale direct oracle driver")
    provenance = oracle["provenance"]
    inputs = "".join(
        f"{v['id']} {v['diameter_um']} {v['density_kg_m3']} "
        f"{v['temperature_k']} {v['air_density_kg_m3']}\n" for v in CANONICAL_VECTORS
    ).encode()
    if provenance["input_sha256"] != sha256_bytes(inputs):
        raise ValueError("oracle input hash differs from the canonical vectors")
    if provenance["recipe_sha256"] != sha256_bytes((ORACLE_DIR / "run_settling_oracle.sh").read_bytes().replace(b"\r\n", b"\n")):
        raise ValueError("stale build recipe")
    if provenance["checkout_clean"] is not True or provenance["executable_sha256"] != oracle["executable_sha256"]:
        raise ValueError("inconsistent source/executable provenance")
    for name in ("settling_mod.o", "drydepo_mod.o", "par_mod.o", "erf_mod.o"):
        if name not in provenance["linked_objects_sha256"]:
            raise ValueError(f"missing linked pinned object: {name}")
    decoded = decode_outputs(raw, CANONICAL_VECTORS)
    expected = [{"id": v["id"], "settling_velocity_m_s": decoded[v["id"]]} for v in CANONICAL_VECTORS]
    if oracle["values"] != expected:
        raise ValueError("decoded velocities differ from the hashed raw output")


def run_oracle_harness(vectors: list[dict], checkout: Path) -> tuple[dict, bytes, dict]:
    """Build/run the pinned routines without changing the reference checkout."""
    verify_checkout(checkout)
    tmp = PROJECT_ROOT / "target" / "settling-oracle"
    tmp.mkdir(parents=True, exist_ok=True)
    archive = tmp / "source.tar"
    git(checkout, "archive", "--format=tar", f"--output={archive.resolve()}", pinned_revision())
    inputs = "".join(
        f"{v['id']} {v['diameter_um']} {v['density_kg_m3']} "
        f"{v['temperature_k']} {v['air_density_kg_m3']}\n" for v in vectors
    ).encode()
    (tmp / "inputs.txt").write_bytes(inputs)
    image = subprocess.check_output(
        ["docker", "image", "inspect", "flexpart-fortran:latest", "--format", "{{.Id}}"],
        text=True,
    ).strip()
    recipe = ORACLE_DIR / "run_settling_oracle.sh"
    identity = {
        "source_archive_sha256": sha256_file(archive), "docker_image_id": image,
        "recipe_sha256": sha256_bytes(recipe.read_bytes().replace(b"\r\n", b"\n")),
        "dockerfile_sha256": sha256_bytes((PROJECT_ROOT / "docker/Dockerfile.fortran").read_bytes().replace(b"\r\n", b"\n")),
    }
    build_id = sha256_bytes(json.dumps(identity, sort_keys=True).encode())
    build = tmp / build_id
    build.mkdir(exist_ok=True)
    if (build / "source/src/FLEXPART").exists():
        verify_scratch_sources(archive, build / "source")
    command = [
        "docker", "run", "--rm", "-v", f"{docker_posix(PROJECT_ROOT)}:/workspace/flexpart-gpu",
        "-e", "OMP_NUM_THREADS=1", "-e", "OMP_THREAD_LIMIT=1", image,
        "bash", "/workspace/flexpart-gpu/oracle/run_settling_oracle.sh", build_id,
    ]
    try:
        with (build / "build-run.log").open("wb") as log:
            result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=False)
        if result.returncode:
            raise RuntimeError(f"oracle build/run failed ({result.returncode}); see {build / 'build-run.log'}")
    finally:
        verify_checkout(checkout)
    verify_scratch_sources(archive, build / "source")
    raw = (build / "outputs.txt").read_bytes()
    objects = {}
    for line in (build / "linked-objects.txt").read_text().splitlines():
        digest, name = line.split(maxsplit=1)
        objects[Path(name).name] = digest
    identity.update({
        "compiler": (build / "compiler.txt").read_text().splitlines()[0],
        "linked_objects_sha256": objects,
        "executable_sha256": sha256_file(build / "settling_oracle"),
        "input_sha256": sha256_bytes(inputs), "checkout_clean": True,
        "source_files_sha256": {
            name: sha256_bytes(subprocess.check_output([
                "git", "-c", f"safe.directory={checkout.resolve().as_posix()}",
                "-C", str(checkout), "show", f"{pinned_revision()}:src/{name}",
            ])) for name in ("settling_mod.f90", "drydepo_mod.f90", "par_mod.f90", "readoptions_mod.f90")
        },
    })
    return decode_outputs(raw, vectors), raw, identity


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--oracle-checkout", type=Path, default=Path(os.environ.get("FLEXPART_DIR", PROJECT_ROOT.parent / "flexpart")))
    parser.add_argument("--check", action="store_true", help="verify fixtures match harness")
    parser.add_argument("--audit", action="store_true", help="audit frozen fixtures without Docker")
    args = parser.parse_args()
    if args.audit:
        audit_fixtures()
        print("settling fixture audit passed")
        return 0

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

    outputs, raw_output, provenance = run_oracle_harness(CANONICAL_VECTORS, args.oracle_checkout)
    executable_sha = provenance["executable_sha256"]
    oracle_doc = {
        "schema": {"id": "flexpart-gpu.settling-oracle", "version": 1},
        "pinned_implementation": "FLEXPART-11.1 settling_mod::get_settling (sphere)",
        "pinned_revision": pinned_revision(),
        "harness": "oracle/settling_oracle.f90",
        "harness_sha256": sha256_bytes(HARNESS.read_bytes().replace(b"\r\n", b"\n")),
        "provenance": provenance,
        "raw_output": "fixtures/settling/oracle-output-v1.txt",
        "executable_sha256": executable_sha,
        "canonical_sha256": canonical_sha,
        "values": [
            {"id": v["id"], "settling_velocity_m_s": outputs[v["id"]]}
            for v in CANONICAL_VECTORS
        ],
    }
    oracle_sha = sha256_bytes(raw_output)
    oracle_doc["output_sha256"] = oracle_sha
    oracle_bytes = json.dumps(oracle_doc, indent=2, sort_keys=True).encode()

    if args.check:
        existing = json.loads(oracle_path.read_text(encoding="utf-8"))
        if (existing != oracle_doc or canonical_path.read_bytes() != canonical_bytes
                or (FIXTURE_DIR / "oracle-output-v1.txt").read_bytes() != raw_output):
            print("oracle fixture mismatch", file=sys.stderr)
            return 1
        print("oracle fixture matches harness")
        return 0

    canonical_path.write_bytes(canonical_bytes)
    oracle_path.write_bytes(oracle_bytes)
    (FIXTURE_DIR / "oracle-output-v1.txt").write_bytes(raw_output)
    print(f"wrote {canonical_path} sha={canonical_sha}")
    print(f"wrote {oracle_path} output_sha={oracle_sha} exe_sha={executable_sha}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
