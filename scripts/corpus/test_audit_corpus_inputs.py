#!/usr/bin/env python3
"""Regression tests for the Philox-aware corpus input audit.

Parser/unit-level only: no GPU or model execution.
"""

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts" / "corpus"))


def load_audit():
    spec = importlib.util.spec_from_file_location(
        "audit_corpus_inputs",
        REPO / "scripts" / "corpus" / "audit_corpus_inputs.py",
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


AUDIT = load_audit()


def load_case(case_id: str) -> dict:
    path = REPO / "fixtures" / "corpus" / "cases" / f"{case_id}.json"
    return json.loads(path.read_text(encoding="utf-8"))


class PhiloxIdentityTest(unittest.TestCase):
    def test_wind_uni_002_seed_zero_matches_manifest(self):
        case = load_case("WIND-UNI-002")
        base_key, base_counter, count, deterministic, identical, error = (
            AUDIT.candidate_philox_identity("WIND-UNI-002", case)
        )
        self.assertIsNone(error)
        self.assertFalse(deterministic)
        self.assertEqual(base_key, [3737180555, 305419896])
        self.assertEqual(base_counter, [0, 0, 0, 0])
        self.assertEqual(count, 10)
        key, counter = AUDIT.expected_philox_for_seed(
            "WIND-UNI-002", base_key, base_counter, 0
        )
        self.assertEqual(key, [3737180555, 305419896])
        self.assertEqual(counter, [0, 0, 0, 0])

    def test_seed_n_uses_wrapping_derivation(self):
        case = load_case("WIND-UNI-002")
        base_key, base_counter, _, _, _, error = AUDIT.candidate_philox_identity(
            "WIND-UNI-002", case
        )
        self.assertIsNone(error)
        key, _ = AUDIT.expected_philox_for_seed(
            "WIND-UNI-002", base_key, base_counter, 7
        )
        self.assertEqual(key, [(3737180555 + 7) % 2**32, 305419896])
        # Wrapping past u32::MAX.
        key, _ = AUDIT.expected_philox_for_seed(
            "WIND-UNI-002", [2**32 - 1, 1], [0, 0, 0, 0], 1
        )
        self.assertEqual(key, [0, 1])

    def test_stochastic_case_without_identity_fails(self):
        case = load_case("WIND-UNI-002")
        case["stochastic"] = {"candidate_philox": None, "oracle_seed": None}
        _, _, _, _, _, error = AUDIT.candidate_philox_identity("WIND-UNI-002", case)
        self.assertIsNotNone(error)
        self.assertIn("no default key", error)

    def test_legacy_v1_document_rejected(self):
        case = {"version": 1, "case_id": "WIND-UNI-002"}
        _, _, _, _, _, error = AUDIT.candidate_philox_identity("WIND-UNI-002", case)
        self.assertIsNotNone(error)
        self.assertIn("schema_version 2", error)

    def test_repeat_009_reuses_identical_key(self):
        case = load_case("REPEAT-009")
        base_key, base_counter, count, deterministic, identical, error = (
            AUDIT.candidate_philox_identity("REPEAT-009", case)
        )
        self.assertIsNone(error)
        self.assertTrue(identical)
        self.assertEqual(count, 2)
        key0, _ = AUDIT.expected_philox_for_seed(
            "REPEAT-009", base_key, base_counter, 0, identical
        )
        key1, _ = AUDIT.expected_philox_for_seed(
            "REPEAT-009", base_key, base_counter, 1, identical
        )
        self.assertEqual(key0, key1)
        self.assertEqual(key0, [3737180555, 305419896])

    def test_adv_ana_001_valid_without_identity(self):
        case = load_case("ADV-ANA-001")
        base_key, base_counter, count, deterministic, identical, error = (
            AUDIT.candidate_philox_identity("ADV-ANA-001", case)
        )
        self.assertIsNone(error)
        self.assertTrue(deterministic)
        self.assertIsNone(base_key)
        self.assertEqual(count, 1)

    def test_audit_fails_for_wrong_key(self):
        case = load_case("WIND-UNI-002")
        with tempfile.TemporaryDirectory() as tmp:
            case_dir = Path(tmp)
            seed = {
                "particle_count": int(case["release"]["particle_count"]),
                "metrics": {"initial_mass_kg": float(case["release"]["inventory"]["quantity_kg"])},
                "seed_index": 0,
                "philox_key": [1, 2],
                "philox_counter": [0, 0, 0, 0],
                "adapter": "test-adapter",
            }
            (case_dir / "seed_000.json").write_text(json.dumps(seed), encoding="utf-8")
            AUDIT.FAILURES.clear()
            AUDIT.audit_candidate_case("WIND-UNI-002", case, case_dir)
            failures = list(AUDIT.FAILURES)
            AUDIT.FAILURES.clear()
        self.assertTrue(
            any("Philox derivation" in f for f in failures),
            f"expected a Philox derivation failure, got: {failures}",
        )

    def test_audit_fails_for_wrong_counter(self):
        case = load_case("WIND-UNI-002")
        base_key, base_counter, _, _, _, _ = AUDIT.candidate_philox_identity(
            "WIND-UNI-002", case
        )
        with tempfile.TemporaryDirectory() as tmp:
            case_dir = Path(tmp)
            seed = {
                "particle_count": int(case["release"]["particle_count"]),
                "metrics": {"initial_mass_kg": float(case["release"]["inventory"]["quantity_kg"])},
                "seed_index": 0,
                "philox_key": list(base_key),
                "philox_counter": [9, 9, 9, 9],
                "adapter": "test-adapter",
            }
            (case_dir / "seed_000.json").write_text(json.dumps(seed), encoding="utf-8")
            AUDIT.FAILURES.clear()
            AUDIT.audit_candidate_case("WIND-UNI-002", case, case_dir)
            failures = list(AUDIT.FAILURES)
            AUDIT.FAILURES.clear()
        self.assertTrue(
            any("Philox counter" in f for f in failures),
            f"expected a Philox counter failure, got: {failures}",
        )

    def test_audit_rejects_missing_identity(self):
        case = load_case("WIND-UNI-002")
        case["stochastic"] = {"candidate_philox": None, "oracle_seed": None}
        with tempfile.TemporaryDirectory() as tmp:
            case_dir = Path(tmp)
            seed = {
                "particle_count": int(case["release"]["particle_count"]),
                "metrics": {"initial_mass_kg": float(case["release"]["inventory"]["quantity_kg"])},
                "seed_index": 0,
                "philox_key": [3737180555, 305419896],
                "philox_counter": [0, 0, 0, 0],
                "adapter": "test-adapter",
            }
            (case_dir / "seed_000.json").write_text(json.dumps(seed), encoding="utf-8")
            AUDIT.FAILURES.clear()
            AUDIT.audit_candidate_case("WIND-UNI-002", case, case_dir)
            failures = list(AUDIT.FAILURES)
            AUDIT.FAILURES.clear()
        self.assertTrue(
            any("Philox identity declared" in f for f in failures),
            f"expected an identity failure, got: {failures}",
        )


if __name__ == "__main__":
    unittest.main()
