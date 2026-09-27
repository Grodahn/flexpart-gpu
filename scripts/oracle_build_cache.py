#!/usr/bin/env python3
"""Deterministically identify and validate reusable FLEXPART oracle builds.

This helper does not build or run an oracle.  It only records whether the
existing executable and Docker image match the inputs consumed by the existing
``run-corpus.sh`` build path.  A cache miss therefore always falls back to that
same pinned build, rather than introducing an alternate oracle path.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path


SCHEMA = "flexpart-gpu.oracle-build-cache.v1"


def sha256(path: Path) -> str:
    """Return the SHA-256 identity used for one cache input or artifact."""
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_head(checkout: Path) -> str:
    """Resolve the oracle revision without depending on global safe-directory state."""
    resolved = checkout.resolve().as_posix()
    return subprocess.run(
        ["git", "-c", f"safe.directory={resolved}", "-C", resolved, "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def cache_identity(project_root: Path, oracle_checkout: Path) -> dict:
    """Describe every build input whose change requires an oracle rebuild."""
    inputs = {
        "dockerfile": sha256(project_root / "docker" / "Dockerfile.fortran"),
        "compose": sha256(project_root / "docker" / "docker-compose.fortran.yml"),
        "oracle_manifest": sha256(project_root / "reference" / "flexpart-11.1.json"),
        "oracle_makefile": sha256(oracle_checkout / "src" / "makefile_gfortran"),
    }
    identity = {
        "oracle_commit": git_head(oracle_checkout),
        "make_arguments": "FC=gfortran eta=no arch=x86-64 -j4",
        "inputs_sha256": inputs,
    }
    encoded = json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
    identity["cache_key"] = hashlib.sha256(encoded).hexdigest()
    return identity


def validate(metadata_path: Path, identity: dict, image_id: str, executable: Path) -> bool:
    """Return whether retained image and executable exactly match the cache record."""
    try:
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    return (
        metadata.get("schema") == SCHEMA
        and metadata.get("identity") == identity
        and metadata.get("docker_image_id") == image_id
        and executable.is_file()
        and metadata.get("oracle_executable_sha256") == sha256(executable)
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("key", "validate", "record"))
    parser.add_argument("--project-root", type=Path, required=True)
    parser.add_argument("--oracle-checkout", type=Path, required=True)
    parser.add_argument("--metadata", type=Path)
    parser.add_argument("--image-id")
    parser.add_argument("--executable", type=Path)
    args = parser.parse_args()

    identity = cache_identity(args.project_root, args.oracle_checkout)
    if args.action == "key":
        print(json.dumps(identity, sort_keys=True))
        return
    if args.metadata is None or args.image_id is None or args.executable is None:
        parser.error("validate/record require --metadata, --image-id, and --executable")
    if args.action == "validate":
        if validate(args.metadata, identity, args.image_id, args.executable):
            print("REUSED")
            return
        print("REBUILD")
        raise SystemExit(1)

    record = {
        "schema": SCHEMA,
        "identity": identity,
        "docker_image": "flexpart-fortran:latest",
        "docker_image_id": args.image_id,
        "oracle_executable_sha256": sha256(args.executable),
    }
    args.metadata.parent.mkdir(parents=True, exist_ok=True)
    args.metadata.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    print(args.metadata)


if __name__ == "__main__":
    main()
