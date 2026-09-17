#!/usr/bin/env python3
"""Write the corpus provenance manifest (hashes, revisions, build, adapter, seeds).

Fails closed when candidate outputs are missing or when the oracle checkout
is not the pinned unmodified FLEXPART 11.1 tree. Oracle artifacts are
recorded when present; a candidate-only run records oracle outputs as empty
instead of fabricating parity.

Provenance covers the actually consumed inputs, not just the outputs:
versioned case JSON, the corpus index and thresholds, the Fortran
COMMAND/RELEASES/OUTGRID/AGECLASSES/RECEPTORS/SPECIES fixtures, the
generated GRIB meteorology, both executables and the comparison report, so
a corpus result can be reproduced and audited end to end.

Usage:
    python3 scripts/corpus/write_corpus_manifest.py --output target/corpus/run_manifest.json ...
"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path


def command(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def hash_tree(root: Path):
    result = {}
    if not root.is_dir():
        return result
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        result[str(path.resolve())] = digest(path)
    return result


def git_state(path: Path):
    checkout = path.resolve().as_posix()
    git = ("git", "-c", f"safe.directory={checkout}", "-C", checkout)
    return {
        "commit": command(*git, "rev-parse", "HEAD"),
        "worktree_dirty": bool(command(*git, "status", "--porcelain")),
    }


def hash_existing(path: Path):
    if not path.is_file():
        return None
    return digest(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True)
    parser.add_argument("--corpus-index", required=True)
    parser.add_argument("--oracle-manifest", required=True)
    parser.add_argument("--oracle-checkout", required=True)
    parser.add_argument("--candidate-checkout", required=True)
    parser.add_argument("--candidate-dir", required=True)
    parser.add_argument("--oracle-dir", required=True)
    parser.add_argument("--report", required=True)
    parser.add_argument("--cases-dir", default=None)
    parser.add_argument("--fortran-fixtures", default=None)
    parser.add_argument("--thresholds", default=None)
    parser.add_argument("--meteo-dir", default=None)
    parser.add_argument("--candidate-exe", default=None)
    parser.add_argument("--oracle-exe", default=None)
    args = parser.parse_args()

    reference = json.loads(Path(args.oracle_manifest).read_text(encoding="utf-8"))
    oracle = git_state(Path(args.oracle_checkout))
    if oracle["commit"] != reference["pinned_commit"] or oracle["worktree_dirty"]:
        raise SystemExit("oracle checkout is not the pinned unmodified FLEXPART source")
    candidate = git_state(Path(args.candidate_checkout))
    corpus_index = json.loads(Path(args.corpus_index).read_text(encoding="utf-8"))

    candidate_dir = Path(args.candidate_dir)
    candidate_files = hash_tree(candidate_dir)
    if not candidate_files:
        raise SystemExit(f"missing candidate artifacts under {candidate_dir}")
    oracle_dir = Path(args.oracle_dir)
    oracle_files = hash_tree(oracle_dir)

    # Actually consumed inputs: versioned case definitions, thresholds and
    # Fortran fixtures; generated meteorology; both executables; report.
    inputs_sha256 = {}
    for label, root in (
        ("cases", args.cases_dir),
        ("fortran_fixtures", args.fortran_fixtures),
        ("meteo", args.meteo_dir),
    ):
        if root:
            tree = hash_tree(Path(root))
            if tree:
                inputs_sha256[label] = tree
    if args.thresholds:
        hashed = hash_existing(Path(args.thresholds))
        if hashed:
            inputs_sha256["thresholds"] = hashed
    executables_sha256 = {}
    for label, exe in (
        ("candidate", args.candidate_exe),
        ("oracle", args.oracle_exe),
    ):
        if exe:
            hashed = hash_existing(Path(exe))
            executables_sha256[label] = hashed or f"missing: {exe}"
    report_path = Path(args.report)
    comparison_report_sha256 = hash_existing(report_path)
    if comparison_report_sha256 is None:
        raise SystemExit(f"comparison report missing: {report_path}")

    # Adapter and seeds come from the candidate per-seed outputs themselves.
    adapters = set()
    seeds = {}
    for path in sorted(candidate_dir.rglob("seed_*.json")):
        try:
            seed = json.loads(path.read_text(encoding="utf-8"))
        except Exception:
            continue
        adapters.add(seed.get("adapter", "unknown"))
        seeds.setdefault(seed.get("case_id", "?"), []).append(
            {"seed_index": seed.get("seed_index"), "philox_key": seed.get("philox_key")}
        )

    try:
        compiler = command("docker", "run", "--rm", "flexpart-fortran:latest", "gfortran", "--version").splitlines()[0]
    except Exception:
        compiler = "unavailable (oracle image not built or Docker missing)"
    try:
        makefile_sha = digest(Path(args.oracle_checkout) / "src" / "makefile_gfortran")
    except Exception:
        makefile_sha = None

    manifest = {
        "status": "PROVENANCE_ONLY_NO_PARITY_VERDICT",
        "corpus_version": corpus_index.get("version"),
        "input_audit": "INPUT_EQUIVALENCE_NOT_DEMONSTRATED remains in force for ETEX mini; "
        "no green corpus metric overrides it.",
        "oracle": oracle,
        "candidate": candidate,
        "compiler": {"version": compiler, "makefile_sha256": makefile_sha, "make_arguments": "eta=no arch=x86-64"},
        "adapter": sorted(adapters),
        "seeds": seeds,
        "inputs_sha256": inputs_sha256,
        "executables_sha256": executables_sha256,
        "candidate_sha256": candidate_files,
        "oracle_sha256": oracle_files,
        "comparison_report": str(report_path.resolve()),
        "comparison_report_sha256": comparison_report_sha256,
    }
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"Corpus run manifest: {output}")


if __name__ == "__main__":
    main()
