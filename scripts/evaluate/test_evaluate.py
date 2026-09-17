"""Alignment, schema and failure-mode tests for the evaluation CLI.

Run from the repository root with:

    python scripts/evaluate/test_evaluate.py
"""

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import report as report_lib


def _demo_seed(path, seed_index, offset):
    import json as _json
    particles = [{"lon_deg": 10.0 + offset + 0.01 * k, "lat_deg": 10.0,
                  "z_m": 50.0 + k, "mass_kg": 0.25} for k in range(4)]
    total = sum(p["mass_kg"] for p in particles)
    lon_mean = sum(p["lon_deg"] * p["mass_kg"] for p in particles) / total
    z_mean = sum(p["z_m"] * p["mass_kg"] for p in particles) / total
    seed = {
        "case_id": "DEMO-001",
        "seed_index": seed_index,
        "philox_key": [1 + seed_index, 2],
        "philox_counter": [0, 0, 0, 0],
        "adapter": "demo-adapter",
        "candidate_revision": "demo",
        "particle_count": 4,
        "active_particles": 4,
        "particles": particles,
        "metrics": {
            "total_mass_kg": total,
            "initial_mass_kg": 1.0,
            "mass_conservation_rel_error": 0.0,
            "com_lon_deg": lon_mean,
            "com_lat_deg": 10.0,
            "com_z_m": z_mean,
            "cov_east_m2": 1.0,
            "cov_north_m2": 0.0,
            "cov_z_m2": 1.0,
            "cov_east_north_m2": 0.0,
            "cov_east_z_m2": 0.0,
            "cov_north_z_m2": 0.0,
            "horizontal_eigenvalues_m2": [0.0, 1.0],
            "z_min_m": 50.0,
            "z_p10_m": 50.0,
            "z_p50_m": 51.0,
            "z_p90_m": 53.0,
            "z_max_m": 53.0,
            "z_mean_m": z_mean,
            "z_std_m": 1.0,
        },
    }
    Path(path).write_text(_json.dumps(seed), encoding="utf-8")


class CorpusReaderTest(unittest.TestCase):
    def test_seed_roundtrip_and_case_mismatch(self):
        import io_corpus
        with tempfile.TemporaryDirectory() as directory:
            seed_path = Path(directory) / "seed_000.json"
            _demo_seed(seed_path, 0, 0.0)
            data = io_corpus.read_seed_file(str(seed_path))
            lons, _lats, _zs, masses = io_corpus.seed_particles(data)
            self.assertEqual(len(lons), 4)
            self.assertAlmostEqual(sum(masses), 1.0)
            case_path = Path(directory) / "case.json"
            case_path.write_text(
                '{"case_id": "OTHER", "release": {}, "physics_switches": {},'
                ' "seeds": {}, "domain": {}}', encoding="utf-8")
            case = io_corpus.read_case_definition(str(case_path))
            self.assertNotEqual(case["case_id"], data["case_id"])

    def test_missing_particle_keys_rejected(self):
        import io_corpus
        with tempfile.TemporaryDirectory() as directory:
            bad = Path(directory) / "bad.json"
            bad.write_text('{"case_id": "x"}', encoding="utf-8")
            with self.assertRaises(ValueError):
                io_corpus.read_seed_file(str(bad))


class GridAlignmentTest(unittest.TestCase):
    def test_matching_grids(self):
        oracle = {"outlon0": 9.5, "outlat0": 8.5, "nx": 32, "ny": 32, "nz": 2,
                  "dx": 0.1, "dy": 0.1, "outheights": [100.0, 250.0]}
        candidate = {"xlon0": 9.5, "ylat0": 8.5, "nx": 32, "ny": 32, "nz": 2,
                     "dx": 0.1, "dy": 0.1, "heights_m": [100.0, 250.0]}
        match, _ = report_lib.check_grid_alignment(oracle, candidate)
        self.assertTrue(match)

    def test_mismatched_origin_fails(self):
        oracle = {"outlon0": 9.5, "outlat0": 8.5, "nx": 32, "ny": 32, "nz": 1,
                  "dx": 0.1, "dy": 0.1, "outheights": [100.0]}
        candidate = {"xlon0": 0.0, "ylat0": 8.5, "nx": 32, "ny": 32, "nz": 1,
                     "dx": 0.1, "dy": 0.1, "heights_m": [100.0]}
        match, checks = report_lib.check_grid_alignment(oracle, candidate)
        self.assertFalse(match)
        self.assertTrue(any(not c["match"] for c in checks))

    def test_mismatched_heights_fail(self):
        oracle = {"outlon0": 0.0, "outlat0": 0.0, "nx": 2, "ny": 2, "nz": 1,
                  "dx": 1.0, "dy": 1.0, "outheights": [100.0]}
        candidate = {"xlon0": 0.0, "ylat0": 0.0, "nx": 2, "ny": 2, "nz": 1,
                     "dx": 1.0, "dy": 1.0, "heights_m": [200.0]}
        match, _ = report_lib.check_grid_alignment(oracle, candidate)
        self.assertFalse(match)


class WindowAlignmentTest(unittest.TestCase):
    def test_matching_windows(self):
        window = {"window_start_epoch_seconds": 1, "window_end_epoch_seconds": 2,
                  "averaging_seconds": 1, "sampling_seconds": 1, "samples": 2,
                  "endpoint_weight": 0.5}
        match, _ = report_lib.check_window_alignment(window, dict(window))
        self.assertTrue(match)

    def test_mismatched_samples_fail(self):
        oracle = {"window_start_epoch_seconds": 1, "window_end_epoch_seconds": 2,
                  "averaging_seconds": 1, "sampling_seconds": 1, "samples": 2,
                  "endpoint_weight": 0.5}
        candidate = dict(oracle, samples=3)
        match, _ = report_lib.check_window_alignment(oracle, candidate)
        self.assertFalse(match)


class ThresholdEvaluationTest(unittest.TestCase):
    def test_mass_budget_gate(self):
        doc = {"criteria": [{"id": "MASS_BUDGET_CLOSE",
                             "applies_to": ["synthetic-uniform-wind"],
                             "metric": "particle_metrics.mass_budget.relative_error",
                             "operator": "abs_lt", "value": 1e-5, "unit": "dimensionless"}]}
        passing = report_lib.evaluate_thresholds(
            doc, {"case_id": "synthetic-uniform-wind",
                  "particle_metrics.mass_budget.relative_error": 1e-7})
        self.assertEqual(passing[0]["verdict"], "PASS")
        failing = report_lib.evaluate_thresholds(
            doc, {"case_id": "synthetic-uniform-wind",
                  "particle_metrics.mass_budget.relative_error": 0.5})
        self.assertEqual(failing[0]["verdict"], "FAIL")

    def test_out_of_scope_is_skipped(self):
        doc = {"criteria": [{"id": "X", "applies_to": ["other"],
                             "metric": "m", "operator": "equals", "value": True}]}
        result = report_lib.evaluate_thresholds(doc, {"case_id": "synthetic-uniform-wind"})
        self.assertEqual(result[0]["verdict"], "SKIP")


class ReportSchemaTest(unittest.TestCase):
    def test_report_has_required_keys(self):
        built = report_lib.make_report(
            case_id="synthetic-uniform-wind", time_window=None, grid=None,
            oracle={"pinned_commit": "x"}, candidate={}, input_hashes={},
            alignment={}, particle_metrics=None, grid_metrics=None, etex=None,
            multiseed=None, threshold_evaluations=[], thresholds_version="v1",
            missing_metrics=[{"name": "grid_metrics.field", "reason": "missing"}],
            notes=[], overall_status="DIAGNOSTIC", status_detail="test")
        self.assertEqual(built["schema_version"], report_lib.SCHEMA_VERSION)
        for key in ("case_id", "overall_status", "oracle", "candidate",
                    "input_sha256", "missing_metrics", "thresholds"):
            self.assertIn(key, built)
        summary = report_lib.render_summary(built)
        self.assertIn("synthetic-uniform-wind", summary)

    def test_report_json_is_serializable(self):
        built = report_lib.make_report(
            case_id="x", time_window={"start_iso": "2024-01-01T00:00:00Z"},
            grid=None, oracle={}, candidate={}, input_hashes={},
            alignment={}, particle_metrics=None, grid_metrics=None, etex=None,
            multiseed=None, threshold_evaluations=[], thresholds_version="v1",
            missing_metrics=[], notes=[], overall_status="DIAGNOSTIC",
            status_detail="test")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.json"
            report_lib.write_report_json(built, path)
            loaded = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(loaded["case_id"], "x")


if __name__ == "__main__":
    unittest.main()
