#!/usr/bin/env python3
"""Verify trusted dependency compilation bytes; discard a damaged cache before tests."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import subprocess

from oracle_build_cache import atomic_json, sha256

DIRECTORIES = tuple(Path("target") / profile / kind
                    for profile in ("debug", "release") for kind in ("deps", "build", ".fingerprint"))
MANIFEST = Path(".cargo-cache-integrity/manifest.json")


def dependency_files(root: Path) -> dict[str, str]:
    """Hash exactly the dependency packages retained by the pinned cache action.

    Workspace crates and their tests are excluded. The action removes their
    compilation artifacts on save, and every required test runs after restore.
    """
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--all-features", "--format-version", "1"], cwd=root, text=True))
    dependencies = [p for p in metadata["packages"] if p["id"] not in metadata["workspace_members"]]
    package_names = {p["name"] for p in dependencies}
    target_names = {t["name"].replace("-", "_") for p in dependencies for t in p["targets"]
                    if set(t["kind"]) & {"lib", "cdylib", "dylib", "rlib", "staticlib", "proc-macro"}}
    files = {}
    for directory in DIRECTORIES:
        allowed = (package_names | target_names) if directory.name != "deps" else (
            {p.replace("-", "_") for p in package_names} | target_names)
        if directory.name == "deps":
            allowed |= {"lib" + name for name in allowed}
        if not (root / directory).is_dir():
            continue
        for child in (root / directory).iterdir():
            if child.name not in allowed and child.name.rsplit("-", 1)[0] not in allowed:
                continue
            for path in sorted(child.rglob("*")) if child.is_dir() else [child]:
                if path.is_file():
                    files[path.relative_to(root).as_posix()] = sha256(path)
    return files


def verify(root: Path) -> bool:
    """Require a complete checksum record or remove compilation caches for a rebuild."""
    try:
        record = json.loads((root / MANIFEST).read_text())
        valid = (record.get("schema") == "flexpart-gpu.cargo-cache-integrity.v1"
                 and bool(record.get("files")) and record["files"] == dependency_files(root))
    except (OSError, ValueError, AttributeError):
        valid = False
    if not valid:
        # All targets are fixed compilation-only paths under this workspace.
        # Scientific output trees are never read, restored or deleted here.
        for directory in DIRECTORIES:
            path = root / directory
            if not path.resolve().is_relative_to(root.resolve()):
                raise ValueError(f"cache directory escapes workspace: {path}")
            if path.is_dir():
                shutil.rmtree(path)
        (root / MANIFEST).unlink(missing_ok=True)
    return valid


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("verify", "record"))
    args = parser.parse_args()
    root = Path.cwd().resolve()
    if args.action == "record":
        files = dependency_files(root)
        if not files:
            raise ValueError("no dependency artifacts to record")
        atomic_json(root / MANIFEST, {"schema": "flexpart-gpu.cargo-cache-integrity.v1", "files": files})
        print(json.dumps({"dependency_files": len(files), "uncompressed_bytes": sum(
            (root / path).stat().st_size for path in files)}))
    else:
        print("Cargo cache: VERIFIED_REUSE" if verify(root) else "Cargo cache: MISS_OR_INVALID; rebuilding")
