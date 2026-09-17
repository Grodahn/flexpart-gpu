#!/usr/bin/env python3
"""Record the exact oracle environment and hashed inputs/outputs of a run.

This is provenance, not a scientific validation verdict. Missing artifacts or
an unpinned/modified FLEXPART checkout are errors.
"""

import argparse
import hashlib
import json
import subprocess
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


def adapter_line(path):
    with open(path, encoding="utf-8", errors="replace") as source:
        matches = [line.strip() for line in source
                   if "wgpu adapter:" in line or
                   "wgpu adapter (software fallback requested):" in line]
    if len(matches) != 1:
        raise ValueError("candidate log lacks one unambiguous wgpu adapter record")
    return matches[0]


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
