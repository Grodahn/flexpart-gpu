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

REPO_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(REPO_ROOT / "provenance"))

import run_provenance as provenance


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


def meteorology_input_provenance(paths, contract):
    """Bind run provenance to the actual canonical meteorology snapshots used.

    Callers that do not consume canonical meteorology leave paths empty; the
    manifest records that explicitly instead of implying that the checked-in
    schema fixture was a runtime input.
    """
    if not paths:
        return {
            "status": "NOT_BOUND_TO_RUN",
            "inputs": {},
        }

    inputs = {}
    for name in paths:
        path = Path(name)
        if not path.is_file():
            raise ValueError(f"canonical meteorology runtime input is missing: {path}")
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ValueError(f"canonical meteorology runtime input is malformed: {path}") from error
        schema = payload.get("schema")
        if not isinstance(schema, dict):
            raise ValueError(f"canonical meteorology runtime input lacks schema identity: {path}")
        if (schema.get("id") != contract["schema_id"]
                or schema.get("version") != contract["schema_version"]):
            raise ValueError(f"canonical meteorology runtime input schema mismatch: {path}")
        inputs[str(path.resolve())] = {
            "sha256": digest(path),
            "schema_id": schema["id"],
            "schema_version": schema["version"],
        }

    return {
        "status": "BOUND_TO_CANONICAL_INPUTS",
        "inputs": inputs,
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


def _partition_output_artifacts(output_sha256, candidate_artifacts, oracle_artifacts):
    """Split recorded output hashes into candidate/oracle ownership.

    Explicit ``--candidate-artifact`` / ``--oracle-artifact`` lists win.
    Remaining artifacts are classified by role markers in their path
    (``gpu`` -> candidate, ``fortran`` -> oracle). Artifacts that cannot
    be classified are conservatively bound to BOTH executions so neither
    role can deny consuming them; ownership is never guessed silently
    toward one side only.
    """
    candidate_set = set(candidate_artifacts)
    oracle_set = set(oracle_artifacts)
    overlap = candidate_set & oracle_set
    if overlap:
        raise ValueError(
            f"artifact listed as both candidate and oracle output: "
            f"{sorted(overlap)[0]}")
    candidate_outputs = {}
    oracle_outputs = {}
    for path, value in output_sha256.items():
        if path in candidate_set:
            candidate_outputs[path] = value
        elif path in oracle_set:
            oracle_outputs[path] = value
        elif "gpu" in path.lower():
            candidate_outputs[path] = value
        elif "fortran" in path.lower():
            oracle_outputs[path] = value
        else:
            candidate_outputs[path] = value
            oracle_outputs[path] = value
    return candidate_outputs, oracle_outputs


def synthetic_case_binding(case_label, input_sha256):
    """Fallback case binding when no versioned case file is supplied.

    Hashes the content of the consumed inputs, not just their path names:
    identical paths with different bytes must not produce the same
    binding.
    """
    return {
        "case_id": case_label,
        "case_manifest_sha256": provenance.hash_bytes(
            provenance.canonical_json(sorted(input_sha256.items()))),
        "case_schema_version": provenance.CASE_SCHEMA_VERSION,
    }


def resolve_oracle_strategy(oracle_kind, oracle_requested_identity):
    """Resolve the #50 oracle strategy reference, failing closed."""
    if oracle_kind == provenance.ORACLE_PRISTINE:
        if oracle_requested_identity is not None:
            raise ValueError(
                "pristine-oracle runs must not carry "
                "--oracle-requested-identity")
        return None
    if oracle_kind == provenance.ORACLE_SEEDABLE:
        if oracle_requested_identity is None:
            raise ValueError(
                "seedable-validation-oracle runs require "
                "--oracle-requested-identity; a seedable run without a "
                "requested identity is rejected instead of silently "
                "falling back to default mode")
        return {
            "strategy": provenance.ORACLE_STRATEGY_ID,
            "version": provenance.ORACLE_STRATEGY_VERSION,
            "contract_path": "reference/oracle-stochastic-identity.json",
        }
    raise ValueError(f"unknown oracle kind: {oracle_kind!r}")


def make_manifest(args):
    reference = json.loads(Path(args.oracle_manifest).read_text(encoding="utf-8"))
    oracle = git_state(args.oracle_checkout)
    if oracle["commit"] != reference["pinned_commit"] or oracle["worktree_dirty"]:
        raise ValueError("oracle checkout is not the pinned unmodified FLEXPART source")
    image = json.loads(command("docker", "image", "inspect", args.image))[0]
    packages = command("docker", "run", "--rm", args.image, "dpkg-query", "-W")
    compiler = command("docker", "run", "--rm", args.image, "gfortran", "--version").splitlines()[0]
    meteorology_contract = meteorology_contract_identity(args.candidate_checkout)
    report = {
        "status": "PROVENANCE_ONLY_NO_PARITY_VERDICT",
        "scenario": args.scenario,
        "oracle": oracle,
        "candidate": git_state(args.candidate_checkout),
        "meteorology_contract": meteorology_contract,
        "meteorology_input": meteorology_input_provenance(
            args.meteorology_input, meteorology_contract),
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
    # --- Issue #53 authoritative v1 overlay --------------------------------
    # Legacy keys above are preserved for compatibility; the v1 envelope
    # below is the authoritative path. Oracle kind reuses the #50
    # vocabulary and is never collapsed into a generic identity.
    oracle_exe_sha = report["oracle_executable_sha256"]
    candidate_exe_sha = report["candidate_executable_sha256"]
    candidate_state = report["candidate"]
    case_label = args.case or args.scenario
    if args.case_manifest:
        case_binding = provenance.case_manifest_identity(args.case_manifest)
        if case_binding["case_id"] != case_label:
            raise ValueError(
                f"case manifest declares {case_binding['case_id']}, "
                f"expected {case_label}")
        v1_cases = [case_binding]
    else:
        v1_cases = [{
            "case_id": case_label,
            "case_manifest_sha256": "0" * 64,
            "case_schema_version": provenance.CASE_SCHEMA_VERSION,
            "manifest_path": "",
        }]
    realization: dict = {}
    if args.seed is not None:
        realization["candidate_seed"] = args.seed
    if args.oracle_requested_identity is not None:
        realization["requested_identity"] = str(args.oracle_requested_identity)
    oracle_strategy = resolve_oracle_strategy(
        args.oracle_kind, args.oracle_requested_identity)
    has_case_binding = v1_cases[0]["case_manifest_sha256"] != "0" * 64
    effective_case = v1_cases[0] if has_case_binding else synthetic_case_binding(
        case_label, report["input_sha256"])
    candidate_outputs, oracle_outputs = _partition_output_artifacts(
        report["output_sha256"], args.candidate_artifact, args.oracle_artifact)
    candidate_execution = provenance.build_execution_record(
        role="candidate",
        case=effective_case,
        realization=dict(realization),
        candidate_revision=candidate_state.get("commit"),
        candidate_executable_sha256=candidate_exe_sha,
        oracle_kind=args.oracle_kind,
        oracle_executable_sha256=oracle_exe_sha,
        oracle_profile={"id": provenance.ORACLE_PROFILE_ID,
                        "version": provenance.ORACLE_PROFILE_VERSION},
        oracle_strategy=oracle_strategy,
        runtime_adapter=report["adapter"],
        inputs_sha256=dict(report["input_sha256"]),
        outputs_sha256=candidate_outputs,
    )
    oracle_execution = provenance.build_execution_record(
        role="oracle",
        case=effective_case,
        realization=dict(realization) if args.oracle_kind == provenance.ORACLE_SEEDABLE else {},
        candidate_revision=candidate_state.get("commit"),
        candidate_executable_sha256=candidate_exe_sha,
        oracle_kind=args.oracle_kind,
        oracle_executable_sha256=oracle_exe_sha,
        oracle_profile={"id": provenance.ORACLE_PROFILE_ID,
                        "version": provenance.ORACLE_PROFILE_VERSION},
        oracle_strategy=oracle_strategy,
        runtime_adapter=None,
        cpu_runtime="flexpart-11.1-single-thread",
        inputs_sha256=dict(report["input_sha256"]),
        outputs_sha256=oracle_outputs,
    )
    # Without an explicit --case-manifest the case binding is a placeholder
    # and the manifest must stay PARTIAL with a machine-readable gap.
    v1_notes = ["v1 authoritative provenance overlay (issue #53); "
                "legacy top-level keys preserved for compatibility."]
    if v1_cases[0]["case_manifest_sha256"] == "0" * 64:
        v1_notes.append("case-manifest hash is a placeholder; pass "
                        "--case-manifest for VERIFIED case binding")
    v1_manifest = provenance.create_run_manifest(
        cases=v1_cases if v1_cases[0]["case_manifest_sha256"] != "0" * 64
        else [{"case_id": case_label,
               "case_manifest_sha256": candidate_execution["case_manifest_sha256"],
               "case_schema_version": provenance.CASE_SCHEMA_VERSION,
               "manifest_path": args.case_manifest or ""}],
        candidate={
            "revision": candidate_state.get("commit", "unknown"),
            "worktree_dirty": candidate_state.get("worktree_dirty"),
            "executable_sha256": candidate_exe_sha,
            "build": None,
        },
        oracle={
            "kind": args.oracle_kind,
            "pinned_commit": reference.get("pinned_commit", ""),
            "worktree_dirty": oracle.get("worktree_dirty"),
            "executable_sha256": oracle_exe_sha,
            "execution_profile": {"id": provenance.ORACLE_PROFILE_ID,
                                  "version": provenance.ORACLE_PROFILE_VERSION},
            "strategy": oracle_strategy,
            "requested_identity": (int(args.oracle_requested_identity)
                                   if args.oracle_requested_identity is not None
                                   else None),
            "patch_sha256": None,
        },
        runtime={"adapter": report["adapter"],
                 "cpu_runtime": "flexpart-11.1-single-thread"},
        executions=[candidate_execution, oracle_execution],
        base="target/etex" if case_label.startswith("ETEX") else "target/corpus",
        notes=v1_notes,
    )
    # A placeholder case binding can never be VERIFIED: surface the gap
    # machine-readably instead of promoting partial attribution.
    if not args.case_manifest:
        v1_manifest["attribution"]["state"] = provenance.ATTRIBUTION_PARTIAL
        v1_manifest["attribution"]["missing"].append({
            "name": "case.case_manifest_sha256",
            "reason": "no --case-manifest supplied; case binding is unverified",
        })
    for key, value in v1_manifest.items():
        report[key] = value
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
    parser.add_argument(
        "--meteorology-input", action="append", default=[],
        help="actual canonical meteorology snapshot used by this run; may be repeated")
    parser.add_argument("--image", default="flexpart-fortran:latest")
    parser.add_argument("--seed", type=int, default=None)
    parser.add_argument("--input", action="append", default=[])
    parser.add_argument("--artifact", action="append", default=[])
    parser.add_argument(
        "--case", default=None,
        help="Validation case id this run belongs to (e.g. ETEX-MINI-013). "
             "When omitted, --scenario is used as the case label.")
    parser.add_argument(
        "--case-manifest", default=None,
        help="Versioned #51 case file for --case; enables the v1 "
             "case-manifest hash binding. When omitted the v1 overlay "
             "records an explicit PARTIAL gap instead of inventing one.")
    parser.add_argument(
        "--oracle-kind", default=provenance.ORACLE_PRISTINE,
        choices=[provenance.ORACLE_PRISTINE, provenance.ORACLE_SEEDABLE],
        help="Oracle execution kind from the #50 contract (pristine-oracle "
             "vs seedable-validation-oracle; never collapsed).")
    parser.add_argument(
        "--oracle-requested-identity", default=None,
        help="Requested seedable identity (FLEXPART_VALIDATION_SEED value) "
             "for seedable-validation-oracle runs; must be provided for "
             "seedable-validation-oracle runs and omitted for pristine runs.")
    parser.add_argument(
        "--candidate-artifact", action="append", default=[],
        help="Output artifact produced by the candidate execution; may be repeated")
    parser.add_argument(
        "--oracle-artifact", action="append", default=[],
        help="Output artifact produced by the oracle execution; may be repeated")
    args = parser.parse_args()
    if not args.input or not args.artifact:
        parser.error("at least one --input and --artifact are required")
    resolve_oracle_strategy(args.oracle_kind, args.oracle_requested_identity)
    report = make_manifest(args)
    output = Path(args.output)
    payload = (json.dumps(report, indent=2) + "\n").encode("utf-8")
    # Non-overwriting for every existing file: a v1 manifest with a
    # different run_id, a legacy manifest, or any other prior evidence at
    # this path is never silently replaced.
    provenance.ensure_non_overwriting_write(output, payload)
    print(f"Oracle run manifest: {output} (v1 run {report.get('run_id', '?')[:16]})")


if __name__ == "__main__":
    main()
