#!/usr/bin/env python3
"""Record the exact oracle environment and hashed inputs/outputs of a run.

This is provenance, not a scientific validation verdict. Missing artifacts or
an unpinned/modified FLEXPART checkout are errors.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path


def command(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()


def digest(path):
    sha = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            sha.update(chunk)
    return sha.hexdigest()


def artifacts(paths):
    result = {}
    for name in paths:
        path = Path(name)
        files = sorted(p for p in path.rglob("*") if p.is_file()) if path.is_dir() else [path]
        if not files or any(not p.is_file() for p in files):
            raise ValueError(f"missing or empty artifact path: {path}")
        for file in files:
            result[str(file.resolve())] = digest(file)
    return result


def git_state(path):
    checkout = Path(path).resolve().as_posix()
    git = ("git", "-c", f"safe.directory={checkout}", "-C", checkout)
    return {
        "commit": command(*git, "rev-parse", "HEAD"),
        "worktree_dirty": bool(command(*git, "status", "--porcelain")),
    }


def meteorology_contract_identity(candidate_checkout):
    """Read the canonical meteorology schema identity from the checked-in fixture.

    The synthetic fixture is validated by the Rust meteorology-contract CI tests
    before this writer runs, so it is the machine-readable bridge to the Rust
    schema constants without duplicating them in Python.
    """
    fixture = Path(candidate_checkout) / "fixtures" / "meteorology" / "synthetic-v1.json"
    if not fixture.is_file():
        raise ValueError(f"canonical meteorology schema fixture is missing: {fixture}")
    try:
        payload = json.loads(fixture.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError("canonical meteorology schema fixture is malformed") from error
    schema = payload.get("schema")
    if not isinstance(schema, dict):
        raise ValueError("canonical meteorology schema identity is missing")
    schema_id = schema.get("id")
    schema_version = schema.get("version")
    if not isinstance(schema_id, str) or not schema_id:
        raise ValueError("canonical meteorology schema id is missing or invalid")
    if not isinstance(schema_version, int) or isinstance(schema_version, bool) or schema_version <= 0:
        raise ValueError("canonical meteorology schema version is missing or invalid")
    return {
        "schema_id": schema_id,
        "schema_version": schema_version,
        "identity_source": str(fixture.resolve()),
        "identity_source_sha256": digest(fixture),
    }


def adapter_line(path):
    with open(path, encoding="utf-8", errors="replace") as source:
        matches = [line.strip() for line in source
                   if "wgpu adapter:" in line or
                   "wgpu adapter (software fallback requested):" in line]
    if len(matches) != 1:
        raise ValueError("candidate log lacks one unambiguous wgpu adapter record")
    return matches[0]


def validate_runtime_profile(manifest_path, environment):
    reference = json.loads(Path(manifest_path).read_text(encoding="utf-8"))
    profile = reference.get("execution_profile", {})
    if profile.get("id") != "flexpart-11.1-single-thread" or profile.get("version") != 1:
        raise ValueError("missing or unsupported oracle execution profile")
    expected = profile.get("runtime_environment", {})
    required = {
        "OMP_NUM_THREADS", "OMP_THREAD_LIMIT", "OMP_DYNAMIC", "OMP_NESTED",
        "OMP_MAX_ACTIVE_LEVELS", "OMP_SCHEDULE", "OMP_PROC_BIND", "OMP_WAIT_POLICY",
    }
    if set(expected) != required or any(not isinstance(v, str) or not v for v in expected.values()):
        raise ValueError("incomplete oracle runtime environment contract")
    if expected["OMP_NUM_THREADS"] != "1" or expected["OMP_THREAD_LIMIT"] != "1":
        raise ValueError("canonical oracle profile requires one thread")
    unexpected = sorted(
        key for key in environment
        if key.startswith(("OMP_", "GOMP_", "KMP_")) and key not in expected
    )
    if unexpected:
        raise ValueError(f"uncontracted OpenMP settings: {', '.join(unexpected)}")
    for key, value in expected.items():
        if environment.get(key) != value:
            raise ValueError(f"oracle runtime setting {key}: expected {value!r}, got {environment.get(key)!r}")
    return {
        "status": "RUNTIME_SETTINGS_VERIFIED_ONLY",
        "execution_profile": {"id": profile["id"], "version": profile["version"]},
        "reference_manifest_sha256": digest(manifest_path),
        "runtime_environment": {key: environment[key] for key in sorted(expected)},
    }


def make_manifest(args):
    reference = json.loads(Path(args.oracle_manifest).read_text(encoding="utf-8"))
    oracle = git_state(args.oracle_checkout)
    if oracle["commit"] != reference["pinned_commit"] or oracle["worktree_dirty"]:
        raise ValueError("oracle checkout is not the pinned unmodified FLEXPART source")
    image = json.loads(command("docker", "image", "inspect", args.image))[0]
    packages = command("docker", "run", "--rm", args.image, "dpkg-query", "-W")
    compiler = command("docker", "run", "--rm", args.image, "gfortran", "--version").splitlines()[0]
    report = {
        "status": "PROVENANCE_ONLY_NO_PARITY_VERDICT",
        "scenario": args.scenario,
        "oracle": oracle,
        "candidate": git_state(args.candidate_checkout),
        "meteorology_contract": meteorology_contract_identity(args.candidate_checkout),
        "container": {
            "image": args.image,
            "image_id": image["Id"],
            "ubuntu_snapshot": image.get("Config", {}).get("Labels", {}).get(
                "org.opencontainers.image.ubuntu.snapshot"),
            "packages": packages.splitlines(),
        },
        "compiler": {"version": compiler,
                     "make_arguments": "eta=no arch=x86-64",
                     "makefile_sha256": digest(Path(args.oracle_checkout) / "src/makefile_gfortran")},
        "random_seed": args.seed,
        "random_seed_note": ("candidate seed is not exposed by this runner"
                             if args.seed is None else None),
        "input_sha256": artifacts(args.input),
        "output_sha256": artifacts(args.artifact),
        "oracle_executable_sha256": digest(args.oracle_executable),
        "candidate_executable_sha256": (digest(args.candidate_executable)
                                        if args.candidate_executable else None),
        "adapter": adapter_line(args.candidate_log),
    }
    return report


def main():
    if sys.argv[1:2] == ["check-runtime-profile"]:
        parser = argparse.ArgumentParser()
        parser.add_argument("command")
        parser.add_argument("--oracle-manifest", required=True)
        args = parser.parse_args()
        print(json.dumps(validate_runtime_profile(args.oracle_manifest, os.environ), indent=2))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True)
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--oracle-manifest", required=True)
    parser.add_argument("--oracle-checkout", required=True)
    parser.add_argument("--oracle-executable", required=True)
    parser.add_argument("--candidate-checkout", required=True)
    parser.add_argument("--candidate-executable")
    parser.add_argument("--candidate-log", required=True)
    parser.add_argument("--image", default="flexpart-fortran:latest")
    parser.add_argument("--seed", type=int, default=None)
    parser.add_argument("--input", action="append", default=[])
    parser.add_argument("--artifact", action="append", default=[])
    args = parser.parse_args()
    if not args.input or not args.artifact:
        parser.error("at least one --input and --artifact are required")
    report = make_manifest(args)
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Oracle run manifest: {output}")


if __name__ == "__main__":
    main()
