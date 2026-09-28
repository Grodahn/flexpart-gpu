"""Unit tests for the oracle run-manifest writer helpers (issue #53)."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "provenance"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import run_provenance as provenance
from write_oracle_run_manifest import (
    _partition_output_artifacts,
    resolve_oracle_strategy,
    synthetic_case_binding,
)


class SyntheticCaseBindingTest(unittest.TestCase):
    """The no---case-manifest fallback binds input content, not paths."""

    def test_same_paths_different_bytes_produce_different_bindings(self):
        first = synthetic_case_binding("DEMO-001", {"COMMAND": "a" * 64})
        second = synthetic_case_binding("DEMO-001", {"COMMAND": "b" * 64})
        self.assertNotEqual(first["case_manifest_sha256"],
                            second["case_manifest_sha256"])

    def test_same_content_produces_identical_binding(self):
        first = synthetic_case_binding("DEMO-001", {"COMMAND": "a" * 64})
        second = synthetic_case_binding("DEMO-001", {"COMMAND": "a" * 64})
        self.assertEqual(first["case_manifest_sha256"],
                         second["case_manifest_sha256"])


class ResolveOracleStrategyTest(unittest.TestCase):
    def test_pristine_without_identity_is_accepted(self):
        self.assertIsNone(
            resolve_oracle_strategy(provenance.ORACLE_PRISTINE, None))

    def test_pristine_with_identity_rejected(self):
        with self.assertRaisesRegex(ValueError, "must not carry"):
            resolve_oracle_strategy(provenance.ORACLE_PRISTINE, 5)

    def test_seedable_with_identity_returns_strategy(self):
        strategy = resolve_oracle_strategy(provenance.ORACLE_SEEDABLE, 5)
        self.assertEqual(strategy["strategy"], provenance.ORACLE_STRATEGY_ID)
        self.assertEqual(strategy["version"],
                         provenance.ORACLE_STRATEGY_VERSION)

    def test_seedable_without_identity_rejected_fail_closed(self):
        with self.assertRaisesRegex(ValueError, "require"):
            resolve_oracle_strategy(provenance.ORACLE_SEEDABLE, None)

    def test_unknown_kind_rejected(self):
        with self.assertRaisesRegex(ValueError, "unknown oracle kind"):
            resolve_oracle_strategy("generic-oracle", None)


class PartitionOutputArtifactsTest(unittest.TestCase):
    def _artifacts(self):
        return {
            "target/etex/mini/fortran_run/output": "a" * 64,
            "target/etex/mini/gpu_output.json": "b" * 64,
            "results/evaluation/report.json": "c" * 64,
        }

    def test_explicit_lists_win_over_path_markers(self):
        candidate, oracle = _partition_output_artifacts(
            self._artifacts(),
            candidate_artifacts=["target/etex/mini/fortran_run/output"],
            oracle_artifacts=["target/etex/mini/gpu_output.json"],
        )
        self.assertIn("target/etex/mini/fortran_run/output", candidate)
        self.assertNotIn("target/etex/mini/fortran_run/output", oracle)
        self.assertIn("target/etex/mini/gpu_output.json", oracle)
        self.assertNotIn("target/etex/mini/gpu_output.json", candidate)

    def test_path_markers_classify_role_outputs(self):
        candidate, oracle = _partition_output_artifacts(
            self._artifacts(), [], [])
        self.assertIn("target/etex/mini/gpu_output.json", candidate)
        self.assertIn("target/etex/mini/fortran_run/output", oracle)

    def test_unclassifiable_artifact_bound_to_both_roles(self):
        candidate, oracle = _partition_output_artifacts(
            self._artifacts(), [], [])
        self.assertIn("results/evaluation/report.json", candidate)
        self.assertIn("results/evaluation/report.json", oracle)

    def test_same_artifact_in_both_lists_rejected(self):
        with self.assertRaisesRegex(ValueError, "both candidate and oracle"):
            _partition_output_artifacts(
                self._artifacts(),
                candidate_artifacts=["target/etex/mini/gpu_output.json"],
                oracle_artifacts=["target/etex/mini/gpu_output.json"],
            )

    def test_empty_partition_yields_empty_maps(self):
        candidate, oracle = _partition_output_artifacts({}, [], [])
        self.assertEqual(candidate, {})
        self.assertEqual(oracle, {})


if __name__ == "__main__":
    unittest.main()
