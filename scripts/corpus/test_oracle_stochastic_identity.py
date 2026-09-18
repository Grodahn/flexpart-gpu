#!/usr/bin/env python3
"""Regression tests for the oracle stochastic identity contract (issue #50).

Covers canonical seed mapping, default-mode preservation, fail-closed
provenance/identity behavior, patch artifact verification, and
algorithm-level initialization distinctness. Standard library only.

Run: python scripts/corpus/test_oracle_stochastic_identity.py
"""

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from oracle_stochastic_identity import (  # noqa: E402
    IDENTITY_MAX,
    IDENTITY_MIN,
    PRISTINE_ORACLE,
    SEEDABLE_ORACLE,
    STRATEGY,
    STRATEGY_VERSION,
    StochasticIdentityError,
    build_identity_record,
    default_state,
    derive_state,
    initial_tables_distinct,
    load_contract,
    parse_requested_identity,
    ran3_initial_table,
    validate_run_record,
    verify_patch_artifact,
)

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
CONTRACT_PATH = REPO_ROOT / "reference" / "oracle-stochastic-identity.json"
PROFILE = {"id": "flexpart-11.1-single-thread", "version": 1}
SEEDABLE_EXE = "s" * 64
PRISTINE_EXE = "p" * 64


def test_contract_loads_and_verifies_patch():
    contract = load_contract(CONTRACT_PATH)
    provenance = verify_patch_artifact(contract, REPO_ROOT)
    assert provenance["touched_files"] == ["src/FLEXPART.f90", "src/random_mod.f90"]
    assert len(provenance["sha256"]) == 64
    print("test_contract_loads_and_verifies_patch: OK")


def test_parse_requested_identity_accepts_canonical_range():
    assert parse_requested_identity("1") == 1
    assert parse_requested_identity("10") == 10
    assert parse_requested_identity("1000000000") == IDENTITY_MAX
    assert parse_requested_identity(7) == 7
    print("test_parse_requested_identity_accepts_canonical_range: OK")


def test_parse_requested_identity_rejects_out_of_range():
    for raw in ("0", 0, "-1", "-320", "1000000001", 2 ** 31 - 1):
        try:
            parse_requested_identity(raw)
        except StochasticIdentityError:
            continue
        raise AssertionError(f"out-of-range identity accepted: {raw!r}")
    print("test_parse_requested_identity_rejects_out_of_range: OK")


def test_parse_requested_identity_rejects_ambiguous_missing():
    for raw in (None, "", "   ", "abc", "5.0", "0x10", " 5", "5 ", "+5",
                "5\n", "5abc", 5.0, True, False, ["5"], {"s": 5}):
        try:
            parse_requested_identity(raw)
        except StochasticIdentityError:
            continue
        raise AssertionError(f"ambiguous identity accepted: {raw!r}")
    print("test_parse_requested_identity_rejects_ambiguous_missing: OK")


def test_parse_requested_identity_rejects_leading_zeros():
    """Canonical decimal representation must not have leading zeros."""
    for raw in ("01", "001", "0001", "0000000001", "010", "007"):
        try:
            parse_requested_identity(raw)
        except StochasticIdentityError:
            continue
        raise AssertionError(f"leading zeros accepted: {raw!r}")
    print("test_parse_requested_identity_rejects_leading_zeros: OK")


def test_parse_requested_identity_rejects_spaces():
    """Leading/trailing spaces must be rejected (not trimmed)."""
    for raw in (" 1", "1 ", " 1 ", "\t1", "1\t", " 1\t"):
        try:
            parse_requested_identity(raw)
        except StochasticIdentityError:
            continue
        raise AssertionError(f"spaces accepted: {raw!r}")
    print("test_parse_requested_identity_rejects_spaces: OK")


def test_parse_requested_identity_rejects_signs():
    """Signs must be rejected."""
    for raw in ("+1", "-1", "+10", "-10"):
        try:
            parse_requested_identity(raw)
        except StochasticIdentityError:
            continue
        raise AssertionError(f"sign accepted: {raw!r}")
    print("test_parse_requested_identity_rejects_signs: OK")


def test_derive_state_mapping_vectors():
    first = derive_state(1)
    assert first["iseed1"] == [-8]
    assert first["iseed2"] == [-89]
    assert first["rannumb_table_idummy"] == -321
    last = derive_state(IDENTITY_MAX)
    assert last["iseed1"] == [-(7 + IDENTITY_MAX)]
    assert last["iseed2"] == [-(88 + IDENTITY_MAX)]
    assert last["rannumb_table_idummy"] == -(320 + IDENTITY_MAX)
    multi = derive_state(42, num_threads=3)
    assert multi["iseed1"] == [-49, -50, -51]
    assert multi["iseed2"] == [-130, -131, -132]
    print("test_derive_state_mapping_vectors: OK")


def test_default_state_matches_pristine_hardcoded_seeds():
    default = default_state()
    assert default["requested_identity"] is None
    assert default["offset"] == 0
    assert default["iseed1"] == [-7]
    assert default["iseed2"] == [-88]
    assert default["rannumb_table_idummy"] == -320
    print("test_default_state_matches_pristine_hardcoded_seeds: OK")


def test_requested_identities_never_equal_default_state():
    default = default_state()
    for identity in (1, 2, 3, 10, IDENTITY_MAX):
        derived = derive_state(identity)
        assert derived["iseed1"] != default["iseed1"]
        assert derived["iseed2"] != default["iseed2"]
        assert derived["rannumb_table_idummy"] != default["rannumb_table_idummy"]
    print("test_requested_identities_never_equal_default_state: OK")


def test_build_identity_record_seeded_and_default():
    contract = load_contract(CONTRACT_PATH)
    record = build_identity_record(
        requested_env_value="3", oracle_kind=SEEDABLE_ORACLE,
        executable_sha256=SEEDABLE_EXE, patch_sha256=contract["validation_patch"]["sha256"],
        case="WIND-UNI-002", execution_profile=dict(PROFILE))
    assert record["requested_identity"] == 3
    assert record["default_mode"] is False
    assert record["derived_state"] == derive_state(3)
    default = build_identity_record(
        requested_env_value=None, oracle_kind=SEEDABLE_ORACLE,
        executable_sha256=SEEDABLE_EXE, patch_sha256=contract["validation_patch"]["sha256"],
        case="WIND-UNI-002", execution_profile=dict(PROFILE))
    assert default["default_mode"] is True
    assert default["derived_state"] == default_state()
    print("test_build_identity_record_seeded_and_default: OK")


def test_build_identity_record_rejects_pristine_with_requested_seed():
    try:
        build_identity_record(
            requested_env_value="3", oracle_kind=PRISTINE_ORACLE,
            executable_sha256=PRISTINE_EXE, patch_sha256=None,
            case="WIND-UNI-002", execution_profile=dict(PROFILE))
    except StochasticIdentityError:
        print("test_build_identity_record_rejects_pristine_with_requested_seed: OK")
        return
    raise AssertionError("pristine oracle accepted a requested seed")


def test_validate_run_record_accepts_consistent_records():
    contract = load_contract(CONTRACT_PATH)
    patch_sha = contract["validation_patch"]["sha256"]
    seeded = build_identity_record(
        requested_env_value="3", oracle_kind=SEEDABLE_ORACLE,
        executable_sha256=SEEDABLE_EXE, patch_sha256=patch_sha,
        case="WIND-UNI-002", execution_profile=dict(PROFILE))
    assert validate_run_record(
        seeded, contract=contract,
        seedable_executable_sha256=SEEDABLE_EXE,
        pristine_executable_sha256=PRISTINE_EXE) == derive_state(3)
    default = build_identity_record(
        requested_env_value=None, oracle_kind=SEEDABLE_ORACLE,
        executable_sha256=SEEDABLE_EXE, patch_sha256=patch_sha,
        case="WIND-UNI-002", execution_profile=dict(PROFILE))
    assert validate_run_record(
        default, contract=contract,
        seedable_executable_sha256=SEEDABLE_EXE,
        pristine_executable_sha256=PRISTINE_EXE) == default_state()
    print("test_validate_run_record_accepts_consistent_records: OK")


def test_validate_run_record_rejects_mislabeled_pristine():
    contract = load_contract(CONTRACT_PATH)
    record = build_identity_record(
        requested_env_value="3", oracle_kind=SEEDABLE_ORACLE,
        executable_sha256=SEEDABLE_EXE, patch_sha256=contract["validation_patch"]["sha256"],
        case="WIND-UNI-002", execution_profile=dict(PROFILE))
    record["oracle_kind"] = PRISTINE_ORACLE
    try:
        validate_run_record(
            record, contract=contract,
            seedable_executable_sha256=SEEDABLE_EXE,
            pristine_executable_sha256=PRISTINE_EXE)
    except StochasticIdentityError:
        print("test_validate_run_record_rejects_mislabeled_pristine: OK")
        return
    raise AssertionError("patched executable labeled as pristine oracle was accepted")


def test_validate_run_record_rejects_unknown_build():
    contract = load_contract(CONTRACT_PATH)
    record = build_identity_record(
        requested_env_value="3", oracle_kind=SEEDABLE_ORACLE,
        executable_sha256=SEEDABLE_EXE, patch_sha256=contract["validation_patch"]["sha256"],
        case="WIND-UNI-002", execution_profile=dict(PROFILE))
    record["executable_sha256"] = "f" * 64
    try:
        validate_run_record(
            record, contract=contract,
            seedable_executable_sha256=SEEDABLE_EXE,
            pristine_executable_sha256=PRISTINE_EXE)
    except StochasticIdentityError:
        print("test_validate_run_record_rejects_unknown_build: OK")
        return
    raise AssertionError("unknown build identity was accepted")


def test_validate_run_record_rejects_unprovable_seed():
    contract = load_contract(CONTRACT_PATH)
    record = build_identity_record(
        requested_env_value="3", oracle_kind=SEEDABLE_ORACLE,
        executable_sha256=SEEDABLE_EXE, patch_sha256=contract["validation_patch"]["sha256"],
        case="WIND-UNI-002", execution_profile=dict(PROFILE))
    record["derived_state"] = default_state()
    try:
        validate_run_record(
            record, contract=contract,
            seedable_executable_sha256=SEEDABLE_EXE,
            pristine_executable_sha256=PRISTINE_EXE)
    except StochasticIdentityError:
        print("test_validate_run_record_rejects_unprovable_seed: OK")
        return
    raise AssertionError("unprovable seed/state was accepted")


def test_validate_run_record_rejects_profile_mismatch():
    contract = load_contract(CONTRACT_PATH)
    record = build_identity_record(
        requested_env_value="3", oracle_kind=SEEDABLE_ORACLE,
        executable_sha256=SEEDABLE_EXE, patch_sha256=contract["validation_patch"]["sha256"],
        case="WIND-UNI-002", execution_profile=dict(PROFILE))
    record["execution_profile"] = {"id": "flexpart-11.1-single-thread", "version": 999}
    try:
        validate_run_record(
            record, contract=contract,
            seedable_executable_sha256=SEEDABLE_EXE,
            pristine_executable_sha256=PRISTINE_EXE)
    except StochasticIdentityError:
        print("test_validate_run_record_rejects_profile_mismatch: OK")
        return
    raise AssertionError("profile mismatch was accepted")


def test_validate_run_record_rejects_indistinguishable_executables():
    contract = load_contract(CONTRACT_PATH)
    record = build_identity_record(
        requested_env_value="3", oracle_kind=SEEDABLE_ORACLE,
        executable_sha256=SEEDABLE_EXE, patch_sha256=contract["validation_patch"]["sha256"],
        case="WIND-UNI-002", execution_profile=dict(PROFILE))
    try:
        validate_run_record(
            record, contract=contract,
            seedable_executable_sha256=SEEDABLE_EXE,
            pristine_executable_sha256=SEEDABLE_EXE)
    except StochasticIdentityError:
        print("test_validate_run_record_rejects_indistinguishable_executables: OK")
        return
    raise AssertionError("indistinguishable executables were accepted")


def test_ran3_replica_pristine_initial_value():
    table = ran3_initial_table(-7)
    assert len(table) == 55
    # mj = mseed - 7 seeds table entry 55 before warmup; after the exact
    # warmup the table is fully determined: check determinism, not magic.
    assert table == ran3_initial_table(-7)
    assert table != ran3_initial_table(-8)
    print("test_ran3_replica_pristine_initial_value: OK")


def test_initial_tables_distinct_for_prescribed_identities():
    evidence = initial_tables_distinct([1, 2, 3, 4, 5, 6, 7, 8, 9, 10])
    assert evidence["pairwise_distinct"] is True
    assert len(evidence["table_digests"]) == 22  # 11 states x 2 families
    assert len(set(evidence["table_digests"].values())) == 22
    print("test_initial_tables_distinct_for_prescribed_identities: OK")


def test_initial_tables_distinct_rejects_duplicate_identities():
    try:
        initial_tables_distinct([3, 3])
    except StochasticIdentityError:
        print("test_initial_tables_distinct_rejects_duplicate_identities: OK")
        return
    raise AssertionError("duplicate identities were accepted")


def test_patch_scope_tripwire_detects_broadened_patch():
    contract = load_contract(CONTRACT_PATH)
    with tempfile.TemporaryDirectory() as tmp:
        patched = Path(tmp) / "flexpart-11.1-seedable.patch"
        text = (REPO_ROOT / "reference" / "flexpart-11.1-seedable.patch").read_text(
            encoding="utf-8")
        patched.write_text(
            text + "\n+      part(ipart)%turbvel%u = 0.0\n", encoding="utf-8")
        tampered = dict(contract)
        tampered["validation_patch"] = dict(contract["validation_patch"])
        import hashlib
        tampered["validation_patch"]["sha256"] = hashlib.sha256(
            patched.read_bytes()).hexdigest()
        fake_root = Path(tmp)
        (fake_root / "reference").mkdir()
        patched.rename(fake_root / "reference" / "flexpart-11.1-seedable.patch")
        try:
            verify_patch_artifact(tampered, fake_root)
        except StochasticIdentityError:
            print("test_patch_scope_tripwire_detects_broadened_patch: OK")
            return
    raise AssertionError("broadened patch passed the scope tripwire")


def test_contract_rng_namespaces_are_separate():
    contract = load_contract(CONTRACT_PATH)
    namespaces = contract.get("rng_namespaces", "")
    assert "Philox" in namespaces and "must never be equated" in namespaces
    assert contract["seed_control"] == "validation-patch"
    assert contract["native_seed_control"] == "unavailable"
    print("test_contract_rng_namespaces_are_separate: OK")


TESTS = [value for name, value in sorted(globals().items())
         if name.startswith("test_") and callable(value)]


def main() -> int:
    failures = 0
    for test in TESTS:
        try:
            test()
        except Exception as exc:  # noqa: BLE001 - report every failure, fail closed
            failures += 1
            print(f"{test.__name__}: FAIL: {exc}")
    print(f"{len(TESTS) - failures}/{len(TESTS)} stochastic identity tests passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
