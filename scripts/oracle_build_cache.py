#!/usr/bin/env python3
"""Deterministically identify and validate reusable FLEXPART oracle builds.

The corpus and technical gate share this build-preparation owner. A cache hit
skips compilation only; scientific execution and validation stay with their
existing callers and always run freshly.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path


SCHEMA = "flexpart-gpu.oracle-build-cache.v2"
MAKE_ARGUMENTS = "FC=gfortran eta=no arch=x86-64 -j4"


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
    # Hash the source bytes too: a matching HEAD alone cannot identify a dirty
    # checkout. Generated objects/modules are checked separately on every hit.
    for path in sorted((oracle_checkout / "src").rglob("*")):
        if path.is_file() and (path.suffix.lower() in (".f90", ".f", ".h", ".inc")
                               or path.name.startswith("makefile")):
            inputs[path.relative_to(oracle_checkout).as_posix()] = sha256(path)
    inputs["cache_owner"] = sha256(Path(__file__))
    identity = {
        "oracle_commit": git_head(oracle_checkout),
        "make_arguments": MAKE_ARGUMENTS,
        "inputs_sha256": inputs,
    }
    encoded = json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
    identity["cache_key"] = hashlib.sha256(encoded).hexdigest()
    return identity


def atomic_json(path: Path, record: dict) -> None:
    """Publish only a complete record, even if a build or writer is interrupted."""
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(dir=path.parent, prefix=path.name + ".")
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            json.dump(record, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def build_artifacts(executable: Path) -> dict:
    """Identify the complete retained link/module set used by direct drivers."""
    paths = sorted((*executable.parent.glob("*.o"), *executable.parent.glob("*.mod")))
    if not any(p.suffix == ".o" for p in paths) or not any(p.suffix == ".mod" for p in paths):
        raise ValueError("oracle build requires both objects and compiler modules")
    return {path.name: sha256(path) for path in paths}


def validate(metadata_path: Path, identity: dict, image_id: str, executable: Path) -> bool:
    """Return whether retained image and executable exactly match the cache record."""
    try:
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        artifacts = build_artifacts(executable)
    except (OSError, ValueError):
        return False
    build_log = metadata_path.with_name("build.log")
    return (
        isinstance(metadata, dict)
        and metadata.get("schema") == SCHEMA
        and metadata.get("identity") == identity
        and metadata.get("docker_image_id") == image_id
        and bool(image_id)
        and metadata.get("artifacts_sha256") == artifacts
        and executable.is_file()
        and metadata.get("oracle_executable_sha256") == sha256(executable)
        and build_log.is_file()
        and metadata.get("build_log_sha256") == sha256(build_log)
    )


def write_status(
    output_path: Path,
    status: str,
    identity: dict,
    image_id: str,
    metadata_path: Path,
    build_log: Path,
) -> None:
    """Write native absolute cache-artifact paths for the agent summary."""
    if status not in ("REUSED", "REBUILT"):
        raise ValueError(f"unsupported cache status: {status}")
    if not metadata_path.is_file() or not build_log.is_file():
        raise FileNotFoundError("cache status requires retained metadata and build log")
    record = {
        "schema": "flexpart-gpu.oracle-build-status.v1",
        "status": status,
        "cache_key": identity["cache_key"],
        "docker_image_id": image_id,
        "metadata": str(metadata_path.resolve()),
        "log": str(build_log.resolve()),
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    atomic_json(output_path, record)


def require_pristine(checkout: Path, project: Path) -> None:
    """Reject an unpinned or dirty oracle before consuming any retained build."""
    pin = json.loads((project / "reference/flexpart-11.1.json").read_text())["pinned_commit"]
    status = subprocess.check_output(
        ["git", "-c", f"safe.directory={checkout.resolve().as_posix()}",
         "-C", str(checkout), "status", "--porcelain"], text=True)
    if git_head(checkout) != pin or status.strip():
        raise ValueError("oracle checkout must be pinned and pristine")


def image_identity() -> tuple[str, str]:
    """Resolve an immutable image and its concrete compiler/linker/package identity."""
    image = subprocess.check_output(
        ["docker", "image", "inspect", "flexpart-fortran:latest", "--format", "{{.Id}}"],
        text=True).strip()
    toolchain = subprocess.check_output(
        ["docker", "run", "--rm", image, "bash", "-c",
         "set -euo pipefail; gfortran -v 2>&1; ld --version; "
         "sha256sum /usr/bin/gfortran /usr/bin/ld; dpkg-query -W"], text=True)
    if not image or not toolchain:
        raise ValueError("missing image/toolchain identity")
    return image, toolchain


def resolved_identity(project: Path, checkout: Path, image: str, toolchain: str) -> dict:
    """Use the same complete identity for every build-record caller."""
    identity = cache_identity(project, checkout)
    identity.pop("cache_key")
    identity["toolchain_sha256"] = hashlib.sha256(toolchain.encode()).hexdigest()
    identity["docker_image_id"] = image
    identity["cache_key"] = hashlib.sha256(
        json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return identity


def prepare(project: Path, checkout: Path, status_output: Path, clean: bool) -> None:
    """Prepare #92's pinned build for both corpus and technical-gate consumers."""
    project, checkout = project.resolve(), checkout.resolve()
    cache = project / "target/oracle-cache"
    cache.mkdir(parents=True, exist_ok=True)
    # A concurrent caller must fail explicitly rather than hash a half-built
    # executable. This lock covers this owner's build and publication only.
    lock = cache / "prepare.lock"
    lock.mkdir()
    try:
        status_output.unlink(missing_ok=True)
        require_pristine(checkout, project)
        metadata = cache / "build.json"
        log = cache / "build.log"
        executable = checkout / "src/FLEXPART"
        try:
            image, toolchain = image_identity()
        except subprocess.CalledProcessError:
            image, toolchain = "", ""
        identity = resolved_identity(project, checkout, image, toolchain)
        if not clean and image and validate(metadata, identity, image, executable):
            disposition = "REUSED"
            print("Oracle build cache: VERIFIED_REUSE", flush=True)
        else:
            metadata.unlink(missing_ok=True)
            environment = dict(os.environ, FLEXPART_DIR=checkout.as_posix())
            compose = ["docker", "compose", "-f", str(project / "docker/docker-compose.fortran.yml")]
            with log.open("w", encoding="utf-8") as stream:
                command = compose + ["build"] + (["--no-cache"] if clean else []) + ["flexpart-fortran"]
                stream.write("=== docker compose build ===\n")
                stream.flush()
                subprocess.run(command, env=environment, stdout=stream, stderr=subprocess.STDOUT, check=True)
                stream.write("=== make clean; full Fortran compilation ===\n")
                stream.flush()
                try:
                    user = ["--user", f"{os.getuid()}:{os.getgid()}"] if hasattr(os, "getuid") else []
                    subprocess.run(compose + ["run", "--rm", *user,
                        "-e", "GIT_CONFIG_COUNT=1", "-e", "GIT_CONFIG_KEY_0=safe.directory",
                        "-e", "GIT_CONFIG_VALUE_0=/workspace/flexpart", "flexpart-fortran", "bash", "-c",
                        "set -euo pipefail; cd /workspace/flexpart/src; "
                        "make -f makefile_gfortran clean; "
                        "FC=gfortran make -f makefile_gfortran eta=no arch=x86-64 -j4; test -x FLEXPART"],
                        env=environment, stdout=stream, stderr=subprocess.STDOUT, check=True)
                finally:
                    subprocess.run(["git", "-c", f"safe.directory={checkout.as_posix()}",
                                    "-C", str(checkout), "checkout", "--", "src/FLEXPART.f90"], check=True)
                    (checkout / "src/gitversion.txt").unlink(missing_ok=True)
            require_pristine(checkout, project)
            image, toolchain = image_identity()
            identity = resolved_identity(project, checkout, image, toolchain)
            with log.open("a", encoding="utf-8") as stream:
                stream.write("\n=== resolved image and toolchain ===\n" + image + "\n" + toolchain)
            atomic_json(metadata, {
                "schema": SCHEMA, "identity": identity, "docker_image_id": image,
                "oracle_executable_sha256": sha256(executable),
                "artifacts_sha256": build_artifacts(executable), "build_log_sha256": sha256(log),
            })
            disposition = "REBUILT"
            print("Oracle build cache: REBUILT", flush=True)
        require_pristine(checkout, project)
        write_status(status_output, disposition, identity, image, metadata, log)
    finally:
        lock.rmdir()


def cached_command(metadata: Path, inputs: list[Path], artifacts: list[Path], command: list[str], *,
                   parent: Path = Path("/workspace/target/oracle-cache/build.json"),
                   project: Path = Path("/workspace/flexpart-gpu"),
                   checkout: Path = Path("/workspace/flexpart")) -> None:
    """Reuse compilation only; callers always run and check scientific outputs afterwards.

    Direct drivers inherit the verified #92 parent build identity and supply
    their complete recipe/source and retained binary/provenance artifact set.
    These records are local build records, never restored from untrusted CI.
    """
    if not parent.is_file():
        # Standalone legacy callers may have built the pristine objects without
        # #92. Preserve their full compilation path; never infer a cache hit.
        subprocess.run(command, check=True)
        print("Direct oracle build: UNCACHED (no verified parent build)", flush=True)
        return
    parent_record = json.loads(parent.read_text())
    if parent_record.get("schema") != SCHEMA or not parent_record.get("artifacts_sha256"):
        raise ValueError("direct driver requires the current verified parent build")
    require_pristine(checkout, project)
    current = cache_identity(project, checkout)
    if (current["inputs_sha256"] != parent_record["identity"]["inputs_sha256"]
            or not validate(parent, parent_record["identity"], parent_record["docker_image_id"],
                            checkout / "src/FLEXPART")):
        raise ValueError("direct driver parent build is missing, changed or corrupt")
    source = Path(os.environ.get("ORACLE_SRC", str(checkout / "src")))
    if source.resolve() != (checkout / "src").resolve():
        # Standalone callers can select a different link tree. The verified
        # parent cannot identify that tree, so retain their compilation path.
        subprocess.run(command, check=True)
        print("Direct oracle build: UNCACHED (different link tree)", flush=True)
        return
    identity = {"parent_sha256": sha256(parent),
                "inputs_sha256": {str(p): sha256(p) for p in inputs},
                "command": command, "cache_owner_sha256": sha256(Path(__file__)),
                "build_environment": {key: os.environ.get(key, "") for key in
                    ("ORACLE_SRC", "objects", "FC", "FFLAGS", "LDFLAGS", "LIBRARY_PATH", "CPATH",
                     "LD_LIBRARY_PATH", "COMPILER_PATH", "GCC_EXEC_PREFIX")}}
    log = metadata.with_suffix(".log")
    try:
        record = json.loads(metadata.read_text())
        hit = (record.get("schema") == SCHEMA and record.get("identity") == identity
               and record.get("artifacts_sha256") == {str(p): sha256(p) for p in artifacts}
               and record.get("build_log_sha256") == sha256(log))
    except (OSError, ValueError, AttributeError):
        hit = False
    if hit:
        print(f"Direct oracle build: VERIFIED_REUSE ({metadata})", flush=True)
        return
    metadata.unlink(missing_ok=True)
    metadata.parent.mkdir(parents=True, exist_ok=True)
    with log.open("w") as stream:
        subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT, check=True)
    atomic_json(metadata, {"schema": SCHEMA, "identity": identity,
                          "artifacts_sha256": {str(p): sha256(p) for p in artifacts},
                          "build_log_sha256": sha256(log)})
    print(f"Direct oracle build: REBUILT ({metadata})", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "cached-command"))
    parser.add_argument("--project-root", type=Path)
    parser.add_argument("--oracle-checkout", type=Path)
    parser.add_argument("--metadata", type=Path)
    parser.add_argument("--status-output", type=Path)
    parser.add_argument("--clean", action="store_true")
    parser.add_argument("--input", type=Path, action="append", default=[])
    parser.add_argument("--artifact", type=Path, action="append", default=[])
    parser.add_argument("--command", nargs=argparse.REMAINDER, default=[])
    args = parser.parse_args()
    if args.action == "cached-command":
        if not args.metadata or not args.input or not args.artifact or not args.command:
            parser.error("cached-command requires metadata, inputs, artifacts and --command")
        cached_command(args.metadata, args.input, args.artifact, args.command)
    else:
        if args.project_root is None or args.oracle_checkout is None or args.status_output is None:
            parser.error("prepare requires project-root, oracle-checkout and status-output")
        prepare(args.project_root, args.oracle_checkout, args.status_output, args.clean)


if __name__ == "__main__":
    main()
