#!/usr/bin/env python3
"""Compare seedable-oracle stochastic identity evidence (issue #50).

Reads the retained per-identity oracle artifacts written by
``scripts/run-corpus.sh oracle-seed-identities`` under
``target/corpus/oracle_seedable/<IDENTITY|default>/rep_XX/`` together with the
pristine ``WIND-UNI-002`` baseline retained by the #49
``oracle-repeatability`` runner, and writes one machine-readable evidence
report. This script characterizes stochastic-identity behavior only; it never
claims candidate-vs-oracle physics parity and never performs ensemble
statistics (owned by later #57/#58/#59 work).

Each identity directory must contain per repetition:
- ``raw/header``, ``raw/dates``, ``raw/grid_conc_*``
- ``oracle_summary.json`` decoded by ``scripts/corpus/decode_oracle_output.py``
- ``runtime_profile.json`` written by ``check-runtime-profile`` immediately
  before the FLEXPART invocation (must match the frozen #49 profile)
- ``fortran.log`` containing ``CONGRATULATIONS``
- ``oracle_executable.sha256`` tying the repetition to the seedable build
- ``consumed_inputs.json`` with SHA-256 of the prepared run-directory options
  and shared meteorology, recorded BEFORE the FLEXPART invocation
- ``stochastic_identity.json`` with the canonical requested identity and its
  derivation into FLEXPART seed/state fields
- ``COMMAND``, ``RELEASES``, ``OUTGRID`` post-run copies (traceability only)

The seedable case directory must contain ``experiment.json`` tying every
identity to one seedable experiment (executable, patch, profile, smoke
proof). The pristine baseline comes from the #49 repeatability layout and
its own experiment record.

Fails closed (non-zero exit) when the contract, patch provenance, build
identity, profile, repetitions, or artifacts are incomplete or inconsistent,
when the seedable default does not reproduce the pristine baseline, when the
requested identities are not pairwise distinct, or when the repeated
identity is not repeatable. Only the Python standard library is used.

Usage:
    python3 scripts/corpus/compare_oracle_seed_identities.py \
        --seedable-dir target/corpus/oracle_seedable \
        --pristine-repeat-dir target/corpus/oracle_repeatability \
        --output target/corpus/oracle_seed_identity_report.json \
        --oracle-manifest reference/flexpart-11.1.json \
        --identity-contract reference/oracle-stochastic-identity.json \
        --pristine-checkout ../flexpart \
        --pristine-exe ../flexpart/src/FLEXPART \
        --seedable-checkout target/flexpart-seedable \
        --seedable-exe target/flexpart-seedable/src/FLEXPART \
        --meteo-dir target/corpus/meteo \
        --fixtures-dir fixtures/corpus/fortran
"""

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from compare_oracle_repeatability import (  # noqa: E402
    compare_numbers,
    hash_tree,
    summarize_differences,
)
from oracle_stochastic_identity import (  # noqa: E402
    PRISTINE_ORACLE,
    PROFILE_ID,
    PROFILE_VERSION,
    SEEDABLE_ORACLE,
    STRATEGY,
    STRATEGY_VERSION,
    StochasticIdentityError,
    digest,
    initial_tables_distinct,
    load_contract,
    validate_run_record,
    verify_patch_artifact,
)


def command(*args) -> str:
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()


def git_state(path: Path) -> dict:
    checkout = path.resolve().as_posix()
    git = ("git", "-c", f"safe.directory={checkout}", "-C", checkout)
    return {
        "commit": command(*git, "rev-parse", "HEAD"),
        "worktree_dirty": bool(command(*git, "status", "--porcelain")),
    }


def load_frozen_profile(manifest_path: Path) -> dict:
    reference = json.loads(manifest_path.read_text(encoding="utf-8"))
    profile = reference.get("execution_profile", {})
    if profile.get("id") != PROFILE_ID or profile.get("version") != PROFILE_VERSION:
        raise SystemExit("missing or unsupported oracle execution profile")
    runtime = profile.get("runtime_environment", {})
    required = {
        "OMP_NUM_THREADS", "OMP_THREAD_LIMIT", "OMP_DYNAMIC", "OMP_NESTED",
        "OMP_MAX_ACTIVE_LEVELS", "OMP_SCHEDULE", "OMP_PROC_BIND", "OMP_WAIT_POLICY",
    }
    if set(runtime) != required:
        raise SystemExit("incomplete oracle runtime environment contract")
    if runtime.get("OMP_NUM_THREADS") != "1" or runtime.get("OMP_THREAD_LIMIT") != "1":
        raise SystemExit("canonical oracle profile requires one thread")
    if profile.get("external_seed_control") != "unavailable":
        raise SystemExit("pristine profile must record seed control as unavailable")
    return reference, profile


def check_runtime_record(runtime_path: Path, profile: dict, manifest_sha: str, label: str) -> dict:
    try:
        runtime = json.loads(runtime_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise SystemExit(f"{label}: unreadable runtime_profile.json: {exc}")
    if runtime.get("execution_profile", {}) != {"id": profile["id"], "version": profile["version"]}:
        raise SystemExit(f"{label}: runtime profile does not match the frozen contract")
    if runtime.get("runtime_environment") != profile["runtime_environment"]:
        raise SystemExit(f"{label}: runtime environment does not match frozen profile")
    if runtime.get("reference_manifest_sha256") != manifest_sha:
        raise SystemExit(f"{label}: reference manifest changed during experiment")
    return runtime


def load_rep(rep: Path, case_label: str, profile: dict, manifest_sha: str,
             expected_exe_sha: str, require_identity: bool = True) -> dict:
    raw_dir = rep / "raw"
    summary_path = rep / "oracle_summary.json"
    runtime_path = rep / "runtime_profile.json"
    log_path = rep / "fortran.log"
    exe_path = rep / "oracle_executable.sha256"
    consumed_path = rep / "consumed_inputs.json"
    identity_path = rep / "stochastic_identity.json"
    required = [raw_dir / "header", raw_dir / "dates", summary_path, runtime_path,
                log_path, exe_path, consumed_path,
                rep / "COMMAND", rep / "RELEASES", rep / "OUTGRID"]
    if require_identity:
        required.append(identity_path)
    for path in required:
        if not path.is_file():
            raise SystemExit(f"{case_label} {rep.name}: missing required artifact {path.name}")
    if not list(raw_dir.glob("grid_conc_*")):
        raise SystemExit(f"{case_label} {rep.name}: missing grid_conc_* in raw/")
    log_text = log_path.read_text(encoding="utf-8", errors="replace")
    if "CONGRATULATIONS" not in log_text:
        raise SystemExit(f"{case_label} {rep.name}: fortran.log lacks successful completion marker")
    rep_exe_sha = exe_path.read_text(encoding="utf-8").strip()
    if rep_exe_sha != expected_exe_sha:
        raise SystemExit(
            f"{case_label} {rep.name}: repetition is tied to executable {rep_exe_sha}, "
            f"not this experiment's {expected_exe_sha}")
    runtime = check_runtime_record(runtime_path, profile, manifest_sha, f"{case_label} {rep.name}")
    try:
        consumed = json.loads(consumed_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise SystemExit(f"{case_label} {rep.name}: unreadable consumed_inputs.json: {exc}")
    for required_option in ("options/COMMAND", "options/RELEASES", "options/OUTGRID"):
        if required_option not in consumed.get("options", {}):
            raise SystemExit(f"{case_label} {rep.name}: consumed inputs lack {required_option}")
    if not consumed.get("meteo"):
        raise SystemExit(f"{case_label} {rep.name}: consumed inputs lack meteorology hashes")
    try:
        identity_record = (json.loads(identity_path.read_text(encoding="utf-8"))
                           if require_identity or identity_path.is_file() else None)
    except (OSError, json.JSONDecodeError) as exc:
        raise SystemExit(f"{case_label} {rep.name}: unreadable stochastic_identity.json: {exc}")
    return {
        "rep": rep.name,
        "oracle_executable_sha256": rep_exe_sha,
        "consumed_inputs": consumed,
        "raw_sha256": hash_tree(raw_dir),
        "decoded_summary_sha256": digest(summary_path),
        "decoded_summary": json.loads(summary_path.read_text(encoding="utf-8")),
        "runtime_profile": runtime,
        "identity_record": identity_record,
    }


def verify_consumed_against_tree(label: str, consumed: dict, meteo_dir: Path,
                                 fixtures_dir: Path, case_id: str) -> None:
    meteo_case = meteo_dir / case_id
    if not (meteo_case / "AVAILABLE").is_file():
        raise SystemExit(f"missing generated meteorology for {case_id}: {meteo_case}")
    if hash_tree(meteo_case) != consumed["meteo"]:
        raise SystemExit(
            f"{label}: generated meteorology changed since the repetitions ran; "
            "rerun the experiment instead of reporting stale inputs")
    fixture_case = fixtures_dir / case_id
    if not fixture_case.is_dir():
        raise SystemExit(f"missing oracle fixture: {fixture_case}")
    fixture_hashes = hash_tree(fixture_case)
    if not fixture_hashes:
        raise SystemExit(f"empty oracle fixture: {fixture_case}")
    consumed_options = consumed["options"]
    for fixture_rel in ("COMMAND", "RELEASES", "OUTGRID", "AGECLASSES", "RECEPTORS"):
        if fixture_hashes.get(fixture_rel) != consumed_options.get(f"options/{fixture_rel}"):
            raise SystemExit(
                f"{label}: versioned fixture {fixture_rel} differs from what the "
                "repetitions consumed; rerun the experiment")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seedable-dir", required=True)
    parser.add_argument("--pristine-repeat-dir", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--oracle-manifest", required=True)
    parser.add_argument("--identity-contract", required=True)
    parser.add_argument("--pristine-checkout", required=True)
    parser.add_argument("--pristine-exe", required=True)
    parser.add_argument("--seedable-checkout", required=True)
    parser.add_argument("--seedable-exe", required=True)
    parser.add_argument("--meteo-dir", required=True)
    parser.add_argument("--fixtures-dir", required=True)
    parser.add_argument("--image", default="flexpart-fortran:latest")
    parser.add_argument("--repo-root", default=None)
    args = parser.parse_args()

    manifest_path = Path(args.oracle_manifest)
    reference, profile = load_frozen_profile(manifest_path)
    manifest_sha = digest(manifest_path)
    repo_root = Path(args.repo_root) if args.repo_root else manifest_path.parent.parent

    try:
        contract = load_contract(Path(args.identity_contract))
        patch_provenance = verify_patch_artifact(contract, repo_root)
    except StochasticIdentityError as exc:
        raise SystemExit(f"stochastic identity contract invalid: {exc}")

    # --- Build provenance: pristine stays normative, seedable is distinct. ---
    pristine = git_state(Path(args.pristine_checkout))
    if pristine["commit"] != reference["pinned_commit"] or pristine["worktree_dirty"]:
        raise SystemExit("pristine oracle checkout is not the pinned unmodified FLEXPART source")
    pristine_exe = Path(args.pristine_exe)
    if not pristine_exe.is_file():
        raise SystemExit(f"pristine oracle executable missing: {pristine_exe}")
    pristine_exe_sha = digest(pristine_exe)
    for probed in (Path(args.pristine_checkout) / "src" / "random_mod.f90",
                   Path(args.pristine_checkout) / "src" / "FLEXPART.f90"):
        if "validation_seed_offset" in probed.read_text(encoding="utf-8", errors="replace"):
            raise SystemExit(
                f"pristine oracle source {probed.name} carries validation seed "
                "handling; the normative checkout must stay unmodified")

    seedable_checkout = Path(args.seedable_checkout)
    seedable = git_state(seedable_checkout)
    if seedable["commit"] != reference["pinned_commit"]:
        raise SystemExit("seedable checkout is not at the pinned commit")
    try:
        changed = command("git", "-c",
                          f"safe.directory={seedable_checkout.resolve().as_posix()}",
                          "-C", seedable_checkout.resolve().as_posix(),
                          "diff", "--name-only").split()
    except subprocess.CalledProcessError as exc:
        raise SystemExit(f"cannot inspect seedable checkout diff: {exc}")
    if sorted(changed) != ["src/FLEXPART.f90", "src/random_mod.f90"]:
        raise SystemExit(
            f"seedable checkout differs from pristine by {sorted(changed)}; only the "
            "versioned validation patch may be applied")
    for probed in (seedable_checkout / "src" / "random_mod.f90",
                   seedable_checkout / "src" / "FLEXPART.f90"):
        if "validation_seed_offset" not in probed.read_text(encoding="utf-8", errors="replace"):
            raise SystemExit(
                f"seedable source {probed.name} lacks the validation seed handling")
    seedable_exe = Path(args.seedable_exe)
    if not seedable_exe.is_file():
        raise SystemExit(f"seedable oracle executable missing: {seedable_exe}")
    seedable_exe_sha = digest(seedable_exe)
    if seedable_exe_sha == pristine_exe_sha:
        raise SystemExit(
            "seedable executable is identical to the pristine executable; the "
            "validation instrument is not distinguished from the normative oracle")

    try:
        makefile_sha = digest(Path(args.pristine_checkout) / "src" / "makefile_gfortran")
    except Exception as exc:
        raise SystemExit(f"oracle makefile missing: {exc}")
    try:
        image_id = command("docker", "image", "inspect", args.image, "--format", "{{.Id}}")
    except Exception as exc:
        raise SystemExit(f"oracle image not available: {exc}")
    try:
        compiler = command("docker", "run", "--rm", args.image, "gfortran", "--version").splitlines()[0]
    except Exception as exc:
        raise SystemExit(f"oracle compiler not available: {exc}")

    # --- Seedable experiment record. ---
    seedable_dir = Path(args.seedable_dir)
    experiment_path = seedable_dir / "experiment.json"
    if not experiment_path.is_file():
        raise SystemExit(f"missing seedable experiment record: {experiment_path}")
    experiment = json.loads(experiment_path.read_text(encoding="utf-8"))
    if experiment.get("execution_profile") != {"id": profile["id"], "version": profile["version"]}:
        raise SystemExit("seedable experiment profile does not match the frozen contract")
    if experiment.get("oracle_kind") != SEEDABLE_ORACLE:
        raise SystemExit("seedable experiment must identify as seedable-validation-oracle")
    if experiment.get("patch_sha256") != patch_provenance["sha256"]:
        raise SystemExit("seedable experiment cites an unknown patch build")
    if experiment.get("oracle_executable_sha256") != seedable_exe_sha:
        raise SystemExit("seedable experiment executable does not match the binary on disk")
    if experiment.get("pristine_executable_sha256") != pristine_exe_sha:
        raise SystemExit("seedable experiment pristine reference does not match the binary on disk")
    if experiment.get("oracle_pinned_commit") != reference["pinned_commit"]:
        raise SystemExit("seedable experiment pinned commit mismatch")
    if experiment.get("case") != contract.get("prescribed_case", "WIND-UNI-002"):
        raise SystemExit("seedable experiment case mismatch")
    smoke = experiment.get("seed_enforcement_smoke", {})
    if not smoke.get("all_rejected") or not smoke.get("attempts"):
        raise SystemExit(
            "seedable experiment lacks proof that invalid seeds fail instead of "
            "falling back silently")
    repetitions = experiment.get("repetitions", {})
    prescribed = [str(s) for s in contract.get("prescribed_identities", [])]
    if not prescribed:
        raise SystemExit("contract prescribes no stochastic identities")
    repeat_identity = str(contract.get("repeatability_identity"))
    repeat_reps = int(contract.get("repeatability_repetitions", 5))

    # --- Pristine baseline from the #49 layout. ---
    pristine_case_dir = Path(args.pristine_repeat_dir) / experiment["case"]
    pristine_experiment_path = pristine_case_dir / "experiment.json"
    if not pristine_experiment_path.is_file():
        raise SystemExit(
            f"missing pristine experiment record: run the #49 oracle-repeatability "
            f"runner for {experiment['case']} first")
    pristine_experiment = json.loads(pristine_experiment_path.read_text(encoding="utf-8"))
    if pristine_experiment.get("oracle_executable_sha256") != pristine_exe_sha:
        raise SystemExit(
            "pristine baseline ties to a different executable; rebuild and rerun "
            "both experiments instead of mixing builds")
    baseline_rep = pristine_case_dir / "rep_01"
    baseline = load_rep(baseline_rep, f"pristine-baseline {experiment['case']}",
                        profile, manifest_sha, pristine_exe_sha, require_identity=False)
    if baseline["identity_record"] is not None and \
            baseline["identity_record"].get("oracle_kind") != PRISTINE_ORACLE:
        raise SystemExit("pristine baseline carries a seedable identity record")
    verify_consumed_against_tree("pristine-baseline", baseline["consumed_inputs"],
                                 Path(args.meteo_dir), Path(args.fixtures_dir),
                                 experiment["case"])

    # --- Seedable identities. ---
    identity_reports = {}
    for label in ["default"] + prescribed:
        identity_dir = seedable_dir / label
        if not identity_dir.is_dir():
            raise SystemExit(f"missing seedable identity directory: {identity_dir}")
        reps = sorted(p for p in identity_dir.iterdir() if p.is_dir() and p.name.startswith("rep_"))
        expected_reps = repetitions.get(label)
        if expected_reps is None:
            raise SystemExit(f"experiment does not declare repetitions for identity {label}")
        if len(reps) != expected_reps:
            raise SystemExit(
                f"identity {label}: found {len(reps)} repetitions but the experiment "
                f"declared {expected_reps}")
        loaded = [load_rep(rep, f"seedable {label}", profile, manifest_sha, seedable_exe_sha)
                  for rep in reps]
        for entry in loaded:
            try:
                applied = validate_run_record(
                    entry["identity_record"], contract=contract,
                    seedable_executable_sha256=seedable_exe_sha,
                    pristine_executable_sha256=pristine_exe_sha)
            except StochasticIdentityError as exc:
                raise SystemExit(f"seedable {label} {entry['rep']}: {exc}")
            entry["applied_state"] = applied
            del entry["identity_record"]
        baseline_consumed = json.dumps(loaded[0]["consumed_inputs"], sort_keys=True)
        for entry in loaded[1:]:
            if json.dumps(entry["consumed_inputs"], sort_keys=True) != baseline_consumed:
                raise SystemExit(
                    f"seedable {label} {entry['rep']}: consumed inputs differ within "
                    "one identity; repetitions with different inputs must not be compared")
        verify_consumed_against_tree(f"seedable {label}", loaded[0]["consumed_inputs"],
                                     Path(args.meteo_dir), Path(args.fixtures_dir),
                                     experiment["case"])
        identity_reports[label] = {
            "repetitions": loaded,
            "consumed_inputs_identical_across_reps": True,
        }

    # --- B. Default-behavior equivalence (seedable default vs pristine). ---
    default_rep = identity_reports["default"]["repetitions"][0]
    differing_raw = sorted(
        key for key in set(baseline["raw_sha256"]) | set(default_rep["raw_sha256"])
        if baseline["raw_sha256"].get(key) != default_rep["raw_sha256"].get(key))
    diffs = []
    compare_numbers(baseline["decoded_summary"], default_rep["decoded_summary"], "", diffs)
    diffs = [d for d in diffs if d["field"]]
    default_equivalence = {
        "pristine_baseline_rep": "rep_01",
        "raw_byte_identical": not differing_raw,
        "differing_raw_files": differing_raw,
        "decoded_hash_identical": (
            default_rep["decoded_summary_sha256"] == baseline["decoded_summary_sha256"]),
        "differing_decoded_fields": diffs,
        "decoded_numeric_summary": summarize_differences(diffs),
    }
    default_equivalence["observation"] = (
        "seedable default reproduces pristine bit-exactly"
        if default_equivalence["raw_byte_identical"]
        and default_equivalence["decoded_hash_identical"]
        else "SEEDABLE DEFAULT DIFFERS FROM PRISTINE; the patch is not acceptable")

    # --- Fail closed: default mode must reproduce pristine bit-exactly. ---
    default_failed = (not default_equivalence["raw_byte_identical"]
                      or not default_equivalence["decoded_hash_identical"])
    if default_failed:
        # Write report with failure status for diagnostics, then exit non-zero.
        report = {
            "status": "SEEDABLE_DEFAULT_EQUIVALENCE_FAILED",
            "status_note": "Seedable default mode does not reproduce pristine oracle; "
                           "the validation-only patch is not acceptable.",
            "strategy": STRATEGY,
            "strategy_version": STRATEGY_VERSION,
            "case": experiment["case"],
            "experiment_id": experiment.get("experiment_id"),
            "execution_profile": {"id": profile["id"], "version": profile["version"]},
            "reference_manifest_sha256": manifest_sha,
            "oracle": pristine,
            "pristine_executable_sha256": pristine_exe_sha,
            "seedable_executable_sha256": seedable_exe_sha,
            "seedable_checkout": {
                "commit": seedable["commit"],
                "diff_name_only": sorted(changed),
            },
            "validation_patch": patch_provenance,
            "seed_enforcement_smoke": smoke,
            "container": {"image": args.image, "image_id": image_id},
            "compiler": {
                "version": compiler,
                "make_arguments": profile["build"]["make_arguments"],
                "makefile_sha256": makefile_sha,
            },
            "runtime_environment": profile["runtime_environment"],
            "rng_namespaces": "Candidate Philox seeds and oracle identities are separate "
                              "RNG namespaces and are never equated.",
            "default_equivalence": default_equivalence,
        }
        output = Path(args.output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(f"Oracle seed identity report: {output}")
        raise SystemExit(
            "seedable default differs from pristine (raw_byte_identical="
            f"{default_equivalence['raw_byte_identical']}, "
            f"decoded_hash_identical={default_equivalence['decoded_hash_identical']}); "
            "the validation-only patch is not acceptable")

    # --- C. Distinct requested stochastic identities. ---
    summaries = {}
    raws = {}
    for label in prescribed:
        rep = identity_reports[label]["repetitions"][0]
        summaries[label] = rep["decoded_summary_sha256"]
        raws[label] = hashlib.sha256(
            json.dumps(rep["raw_sha256"], sort_keys=True).encode("utf-8")).hexdigest()
    if len(set(summaries.values())) != len(prescribed):
        raise SystemExit(
            f"requested identities did not produce distinct stochastic evidence "
            f"(decoded summaries collide: {summaries}); the seed mechanism failed")
    if len(set(raws.values())) != len(prescribed):
        raise SystemExit(
            "requested identities did not produce distinct raw outputs; "
            "the seed mechanism failed")
    try:
        algorithm_evidence = initial_tables_distinct([int(s) for s in prescribed])
    except StochasticIdentityError as exc:
        raise SystemExit(f"algorithm-level distinctness failed: {exc}")

    # --- D. Same-seed repeatability. ---
    repeat_entries = identity_reports[repeat_identity]["repetitions"]
    if len(repeat_entries) < repeat_reps:
        raise SystemExit(
            f"repeatability identity {repeat_identity} has {len(repeat_entries)} "
            f"repetitions, need at least {repeat_reps}")
    repeat_base = repeat_entries[0]
    repeat_base_summary = repeat_base["decoded_summary"]
    for entry in repeat_entries:
        differing = sorted(
            key for key in set(repeat_base["raw_sha256"]) | set(entry["raw_sha256"])
            if repeat_base["raw_sha256"].get(key) != entry["raw_sha256"].get(key))
        entry["raw_byte_identical_to_baseline"] = not differing
        entry["differing_raw_files"] = differing
        entry["decoded_hash_identical_to_baseline"] = (
            entry["decoded_summary_sha256"] == repeat_base["decoded_summary_sha256"])
        entry_diffs = []
        compare_numbers(repeat_base_summary, entry["decoded_summary"], "", entry_diffs)
        entry["differing_decoded_fields"] = [d for d in entry_diffs if d["field"]]
        entry["decoded_numeric_summary"] = summarize_differences(entry["differing_decoded_fields"])
        entry["byte_identical_to_baseline"] = bool(
            entry["raw_byte_identical_to_baseline"]
            and entry["decoded_hash_identical_to_baseline"])
        del entry["decoded_summary"]
        del entry["consumed_inputs"]
    all_repeat_identical = all(e["byte_identical_to_baseline"] for e in repeat_entries)
    if not all_repeat_identical:
        raise SystemExit(
            f"same-seed repeatability failed for identity {repeat_identity}: not all "
            "repetitions reproduce to the #49 bitwise repeatability level; the seed "
            "mechanism does not control all relevant stochastic state")

    for label in prescribed:
        for entry in identity_reports[label]["repetitions"]:
            entry.pop("decoded_summary", None)
            entry.pop("consumed_inputs", None)
    for entry in identity_reports["default"]["repetitions"]:
        entry.pop("decoded_summary", None)
        entry.pop("consumed_inputs", None)

    report = {
        "status": "ORACLE_STOCHASTIC_IDENTITY_CHARACTERIZED_NO_PARITY_VERDICT",
        "status_note": "Stochastic identity behavior only; no candidate-vs-oracle "
                       "physics parity and no ensemble statistics are claimed.",
        "strategy": STRATEGY,
        "strategy_version": STRATEGY_VERSION,
        "case": experiment["case"],
        "experiment_id": experiment.get("experiment_id"),
        "execution_profile": {"id": profile["id"], "version": profile["version"]},
        "reference_manifest_sha256": manifest_sha,
        "oracle": pristine,
        "pristine_executable_sha256": pristine_exe_sha,
        "seedable_executable_sha256": seedable_exe_sha,
        "seedable_checkout": {
            "commit": seedable["commit"],
            "diff_name_only": sorted(changed),
        },
        "validation_patch": patch_provenance,
        "seed_enforcement_smoke": smoke,
        "container": {"image": args.image, "image_id": image_id},
        "compiler": {
            "version": compiler,
            "make_arguments": profile["build"]["make_arguments"],
            "makefile_sha256": makefile_sha,
        },
        "runtime_environment": profile["runtime_environment"],
        "rng_namespaces": "Candidate Philox seeds and oracle identities are separate "
                          "RNG namespaces and are never equated.",
        "default_equivalence": default_equivalence,
        "distinct_identities": {
            "identities": prescribed,
            "decoded_summary_sha256": summaries,
            "raw_inventory_sha256": raws,
            "pairwise_distinct": True,
            "algorithm_level": algorithm_evidence,
        },
        "same_seed_repeatability": {
            "identity": repeat_identity,
            "repetitions": repeat_entries,
            "all_byte_identical_to_baseline": all_repeat_identical,
            "observation": "requested identity reproduces bitwise under the frozen profile",
        },
        "identities": identity_reports,
        "pristine_baseline": {
            "rep": "rep_01",
            "oracle_executable_sha256": pristine_exe_sha,
            "raw_sha256": baseline["raw_sha256"],
            "decoded_summary_sha256": baseline["decoded_summary_sha256"],
        },
    }
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Oracle seed identity report: {output}")


if __name__ == "__main__":
    main()
