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
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
if str(REPO_ROOT / "scripts" / "provenance") not in sys.path:
    sys.path.insert(0, str(REPO_ROOT / "scripts" / "provenance"))
if str(REPO_ROOT / "scripts") not in sys.path:
    sys.path.insert(0, str(REPO_ROOT / "scripts"))

import run_provenance as provenance


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
    parser.add_argument(
        "--oracle-kind",
        default=provenance.ORACLE_PRISTINE,
        choices=[provenance.ORACLE_PRISTINE, provenance.ORACLE_SEEDABLE],
        help="Oracle execution kind from the #50 contract "
        "(pristine-oracle vs seedable-validation-oracle; never collapsed).",
    )
    parser.add_argument(
        "--case",
        dest="case_id",
        help="Hash and report only this canonical case id for a focused run.",
    )
    parser.add_argument(
        "--oracle-dependency-case",
        action="append",
        default=[],
        help="Additional oracle case consumed by a focused comparison.",
    )
    args = parser.parse_args()

    reference = json.loads(Path(args.oracle_manifest).read_text(encoding="utf-8"))
    oracle = git_state(Path(args.oracle_checkout))
    if oracle["commit"] != reference["pinned_commit"] or oracle["worktree_dirty"]:
        raise SystemExit("oracle checkout is not the pinned unmodified FLEXPART source")
    candidate = git_state(Path(args.candidate_checkout))
    corpus_index = json.loads(Path(args.corpus_index).read_text(encoding="utf-8"))
    known_case_ids = {entry["id"] for entry in corpus_index["cases"]}
    if args.case_id and args.case_id not in known_case_ids:
        raise SystemExit(f"unknown corpus case: {args.case_id}")
    if args.oracle_dependency_case and not args.case_id:
        raise SystemExit("--oracle-dependency-case requires --case")
    unknown_dependencies = set(args.oracle_dependency_case) - known_case_ids
    if unknown_dependencies:
        raise SystemExit(
            f"unknown oracle dependency case: {sorted(unknown_dependencies)[0]}"
        )
    focused_input_cases = [args.case_id, *args.oracle_dependency_case] if args.case_id else []

    candidate_dir = Path(args.candidate_dir)
    candidate_files = hash_tree(
        candidate_dir / args.case_id if args.case_id else candidate_dir
    )
    if not candidate_files:
        raise SystemExit(f"missing candidate artifacts under {candidate_dir}")
    oracle_dir = Path(args.oracle_dir)
    if args.case_id:
        oracle_files = {}
        for case_id in focused_input_cases:
            case_files = hash_tree(oracle_dir / case_id)
            if not case_files:
                raise SystemExit(f"missing oracle artifacts under {oracle_dir / case_id}")
            oracle_files.update(case_files)
    else:
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
            root_path = Path(root)
            if args.case_id and label == "cases":
                tree = {}
                for case_id in focused_input_cases:
                    case_file = root_path / f"{case_id}.json"
                    if not case_file.is_file():
                        raise SystemExit(f"missing focused {label} input: {case_file}")
                    tree[str(case_file.resolve())] = digest(case_file)
            elif args.case_id:
                tree = {}
                for case_id in focused_input_cases:
                    case_root = root_path / case_id
                    case_tree = hash_tree(case_root)
                    if not case_tree:
                        raise SystemExit(f"missing focused {label} inputs under {case_root}")
                    tree.update(case_tree)
            else:
                tree = hash_tree(root_path)
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
    seed_root = candidate_dir / args.case_id if args.case_id else candidate_dir
    for path in sorted(seed_root.rglob("seed_*.json")):
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

    # --- Issue #53 authoritative v1 overlay --------------------------------
    # The legacy keys above are preserved byte-for-byte for compatibility;
    # the v1 envelope below is the authoritative provenance path. Case
    # identities are referenced from the #51 case files (never copied),
    # oracle kind reuses the #50 contract vocabulary, and each candidate
    # realization receives its own execution identity.
    v1_cases = []
    cases_root = Path(args.cases_dir) if args.cases_dir else None
    case_ids = focused_input_cases or sorted(
        {entry["id"] for entry in corpus_index.get("cases", [])})
    for case_id in case_ids:
        case_file = (cases_root / f"{case_id}.json") if cases_root else None
        if case_file is not None and case_file.is_file():
            v1_cases.append(provenance.case_manifest_identity(case_file))
        else:
            # Focused runs always hash the case file; full-corpus runs
            # without --cases-dir record an explicit gap instead of
            # inventing an identity.
            v1_cases.append({
                "case_id": case_id,
                "case_manifest_sha256": "0" * 64,
                "case_schema_version": provenance.CASE_SCHEMA_VERSION,
                "manifest_path": str(case_file) if case_file else "",
            })
    # Real case identities only: placeholder zero-hashes never verify.
    v1_cases = [c for c in v1_cases
                if c["case_manifest_sha256"] != "0" * 64]
    if not v1_cases:
        raise SystemExit(
            "cannot build v1 provenance without case-manifest identities "
            "(pass --cases-dir with the versioned case files)")

    candidate_exe_sha = executables_sha256.get("candidate")
    if isinstance(candidate_exe_sha, str) and len(candidate_exe_sha) != 64:
        candidate_exe_sha = None
    oracle_exe_sha = executables_sha256.get("oracle")
    if isinstance(oracle_exe_sha, str) and len(oracle_exe_sha) != 64:
        oracle_exe_sha = None
    adapter_identity = sorted(adapters)[0] if len(adapters) == 1 else None

    # Bind every file consumed by the validation/provenance workflow into
    # the authoritative v1 identity. The legacy nested inputs_sha256 field is
    # informational only; verification reads execution input maps.
    workflow_inputs: dict[str, str] = {}
    for value in inputs_sha256.values():
        entries = value if isinstance(value, dict) else {}
        for name, hashed in provenance.normalize_artifact_map(entries).items():
            if name in workflow_inputs and workflow_inputs[name] != hashed:
                raise SystemExit(
                    f"workflow input identity {name!r} maps to two hashes")
            workflow_inputs[name] = hashed
    for path in (Path(args.corpus_index), Path(args.oracle_manifest), report_path):
        for name, hashed in provenance.normalize_artifact_map(
                {str(path.resolve()): digest(path)}).items():
            if name in workflow_inputs and workflow_inputs[name] != hashed:
                raise SystemExit(
                    f"workflow input identity {name!r} maps to two hashes")
            workflow_inputs[name] = hashed
    if args.thresholds:
        threshold_path = Path(args.thresholds)
        for name, hashed in provenance.normalize_artifact_map(
                {str(threshold_path.resolve()): digest(threshold_path)}).items():
            if name in workflow_inputs and workflow_inputs[name] != hashed:
                raise SystemExit(
                    f"workflow input identity {name!r} maps to two hashes")
            workflow_inputs[name] = hashed

    v1_executions = []
    seed_files = sorted(seed_root.rglob("seed_*.json"))
    for seed_file in seed_files:
        try:
            seed_doc = json.loads(seed_file.read_text(encoding="utf-8"))
        except Exception:
            continue
        seed_case = seed_doc.get("case_id", args.case_id or "?")
        case_binding = next((c for c in v1_cases if c["case_id"] == seed_case), None)
        if case_binding is None:
            raise SystemExit(
                f"seed file {seed_file} belongs to case {seed_case!r} which is "
                "not part of this run; misattributed provenance is rejected")
        # Scope artifact keys by case so identical basenames from
        # different cases stay distinct identities.
        scoped_seed_key = f"{seed_case}/{seed_file.name}"
        v1_executions.append(provenance.build_execution_record(
            role="candidate",
            case=case_binding,
            realization={
                "seed_index": seed_doc.get("seed_index"),
                "philox_key": seed_doc.get("philox_key"),
                "philox_counter": seed_doc.get("philox_counter"),
                "seed_file": seed_file.name,
            },
            candidate_revision=candidate.get("commit"),
            candidate_executable_sha256=candidate_exe_sha,
            oracle_kind=args.oracle_kind,
            oracle_revision=reference.get("pinned_commit"),
            oracle_executable_sha256=oracle_exe_sha,
            oracle_profile={"id": provenance.ORACLE_PROFILE_ID,
                            "version": provenance.ORACLE_PROFILE_VERSION},
            runtime_adapter=seed_doc.get("adapter"),
            inputs_sha256=workflow_inputs,
            outputs_sha256={scoped_seed_key: digest(seed_file)},
        ))
    # Oracle side: one execution per case binding over that case's
    # recorded oracle output bytes so mixed oracle builds across cases
    # are detectable in one manifest. Case attribution matches whole
    # path components only (DEMO-1 must not capture DEMO-10 outputs),
    # and unattributable files fail closed instead of being dropped.
    oracle_by_case: dict[str, dict[str, str]] = {c["case_id"]: {} for c in v1_cases}
    for abs_key, value in oracle_files.items():
        normalized = abs_key.replace("\\", "/")
        parts = normalized.split("/")
        owner = next((c["case_id"] for c in v1_cases if c["case_id"] in parts), None)
        if owner is None and len(v1_cases) == 1:
            owner = v1_cases[0]["case_id"]
        if owner is None:
            raise SystemExit(
                f"oracle artifact {abs_key} cannot be attributed to any "
                f"case of this run {sorted(c['case_id'] for c in v1_cases)}; "
                "unattributable provenance is rejected")
        oracle_by_case[owner][f"{owner}/{Path(abs_key).name}"] = value
    for case_binding in v1_cases:
        case_outputs = oracle_by_case.get(case_binding["case_id"], {})
        if not case_outputs:
            continue
        v1_executions.append(provenance.build_execution_record(
            role="oracle",
            case=case_binding,
            realization={},
            candidate_revision=candidate.get("commit"),
            candidate_executable_sha256=candidate_exe_sha,
            oracle_kind=args.oracle_kind,
            oracle_revision=reference.get("pinned_commit"),
            oracle_executable_sha256=oracle_exe_sha,
            oracle_profile={"id": provenance.ORACLE_PROFILE_ID,
                            "version": provenance.ORACLE_PROFILE_VERSION},
            runtime_adapter=None,
            cpu_runtime="flexpart-11.1-single-thread",
            inputs_sha256=workflow_inputs,
            outputs_sha256=case_outputs,
        ))
    if not v1_executions:
        raise SystemExit("cannot build v1 provenance without execution artifacts")

    v1_manifest = provenance.create_run_manifest(
        cases=v1_cases,
        candidate={
            "revision": candidate.get("commit", "unknown"),
            "worktree_dirty": candidate.get("worktree_dirty"),
            "executable_sha256": candidate_exe_sha,
            "build": {"compiler": compiler, "makefile_sha256": makefile_sha},
        },
        oracle={
            "kind": args.oracle_kind,
            "pinned_commit": reference.get("pinned_commit", ""),
            "worktree_dirty": oracle.get("worktree_dirty"),
            "executable_sha256": oracle_exe_sha,
            "execution_profile": {"id": provenance.ORACLE_PROFILE_ID,
                                  "version": provenance.ORACLE_PROFILE_VERSION},
            "strategy": None,
            "requested_identity": None,
            "patch_sha256": None,
        },
        runtime={"adapter": adapter_identity,
                 "cpu_runtime": "flexpart-11.1-single-thread"},
        executions=v1_executions,
        base="target/corpus",
        notes=["v1 authoritative provenance overlay (issue #53); "
               "legacy top-level keys preserved for compatibility."],
    )
    for key, value in v1_manifest.items():
        manifest[key] = value

    output = Path(args.output)
    payload = (json.dumps(manifest, indent=2) + "\n").encode("utf-8")
    # Non-overwriting for every existing file: a v1 manifest with a
    # different run_id, a legacy manifest, or any other prior evidence at
    # this path is never silently replaced.
    provenance.ensure_non_overwriting_write(output, payload)
    print(f"Corpus run manifest: {output} (v1 run {manifest['run_id'][:16]})")


if __name__ == "__main__":
    main()
