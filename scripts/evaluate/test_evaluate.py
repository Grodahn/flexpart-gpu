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
import evaluate_case
import metrics


def _namespaced(**overrides):
    import argparse
    defaults = {
        "candidate_seeds": None, "case_def": None, "fortran_output": None,
        "thresholds": str(evaluate_case.DEFAULT_THRESHOLDS),
        "oracle_manifest": str(evaluate_case.DEFAULT_ORACLE_MANIFEST),
        "run_manifest": None, "input_equivalence_report": None,
        "oracle_checkout": None, "oracle_executable": None,
        "oracle_seed": None, "oracle_seed_controllable": False,
        "candidate_log": None, "candidate_executable": None,
        "candidate_revision": None, "seed": None,
        "candidate_seed_controllable": False, "note": [],
    }
    defaults.update(overrides)
    return argparse.Namespace(**defaults)


def _demo_case(path, case_id="DEMO-001", base_key=(1, 2), count=2,
               derivation="wrapping_add_key0_v1", candidate_philox=True):
    path = Path(path)
    philox = None
    if candidate_philox:
        philox = {
            "base_key": list(base_key),
            "base_counter": [0, 0, 0, 0],
            "count": count,
            "derivation": derivation,
        }
    path.write_text(json.dumps({
        "schema_version": 2,
        "case_id": case_id,
        "release": {"inventory": {"quantity_kg": 1.0, "unit": "kg"}},
        "physics_switches": {"turbulence": bool(candidate_philox),
                             "convection": False,
                             "dry_deposition": False, "wet_deposition": False,
                             "decay": False},
        "stochastic": {"candidate_philox": philox, "oracle_seed": None},
        "domain": {"nx": 32, "ny": 32, "nz": 8, "dx_deg": 0.1, "dy_deg": 0.1,
                   "xlon0_deg": 9.5, "ylat0_deg": 8.5},
        "integration": {"start": "20240101000000", "dt_s": 300, "steps": 12,
                        "total_s": 3600},
    }), encoding="utf-8")


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
                '{"schema_version": 2, "case_id": "OTHER", "release": {},'
                ' "physics_switches": {}, "stochastic": {"candidate_philox": null, "oracle_seed": null}, "domain": {}}',
                encoding="utf-8")
            case = io_corpus.read_case_definition(str(case_path))
            self.assertNotEqual(case["case_id"], data["case_id"])

    def test_case_reader_requires_explicit_stochastic_namespace_fields(self):
        import io_corpus
        with tempfile.TemporaryDirectory() as directory:
            for missing in ("candidate_philox", "oracle_seed"):
                path = Path(directory) / f"missing-{missing}.json"
                stochastic = {
                    "candidate_philox": None,
                    "oracle_seed": None,
                }
                del stochastic[missing]
                path.write_text(json.dumps({
                    "schema_version": 2,
                    "case_id": "DEMO-001",
                    "release": {},
                    "physics_switches": {},
                    "stochastic": stochastic,
                    "domain": {},
                }), encoding="utf-8")
                with self.subTest(missing=missing):
                    with self.assertRaisesRegex(ValueError, missing):
                        io_corpus.read_case_definition(str(path))

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


class MassGateWorstGovernsTest(unittest.TestCase):
    """P1-1: opposite-sign budget errors must not cancel into a pass."""

    def test_cancelled_mean_still_fails(self):
        low = metrics.mass_budget(1.0, 0.6)   # -40%
        high = metrics.mass_budget(1.0, 1.4)  # +40%
        mean_error = (low["relative_error"] + high["relative_error"]) / 2.0
        self.assertAlmostEqual(mean_error, 0.0)
        worst = max(abs(low["relative_error"]), abs(high["relative_error"]))
        self.assertAlmostEqual(worst, 0.4)
        doc = {"criteria": [{"id": "MASS_BUDGET_CLOSE",
                             "applies_to": ["corpus-seeds"],
                             "metric": "particle_metrics.mass_budget.relative_error",
                             "operator": "abs_lt", "value": 1e-5,
                             "unit": "dimensionless"}]}
        result = report_lib.evaluate_thresholds(
            doc, {"case_id": "corpus-seeds",
                  "particle_metrics.mass_budget.relative_error": worst})
        self.assertEqual(result[0]["verdict"], "FAIL")


class SeedIdentityTest(unittest.TestCase):
    """P1-2: seed files must be distinct, correctly derived identities."""

    def _seeds(self, directory, keys):
        import io_corpus
        paths = []
        for index, key in enumerate(keys):
            path = Path(directory) / f"seed_{index:03d}.json"
            _demo_seed(path, index, 0.0)
            data = json.loads(path.read_text(encoding="utf-8"))
            data["philox_key"] = list(key)
            path.write_text(json.dumps(data), encoding="utf-8")
            paths.append((str(path), io_corpus.read_seed_file(str(path))))
        return paths

    def test_duplicate_seed_index_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            case = Path(directory) / "case.json"
            _demo_case(case)
            case_def = json.loads(case.read_text(encoding="utf-8"))
            seeds = self._seeds(directory, [(1, 2), (1, 2)])
            # Same index twice is rejected before identity comparison.
            dup = [(seeds[0][0], dict(seeds[0][1], seed_index=0)),
                   (seeds[1][0], dict(seeds[1][1], seed_index=0))]
            with self.assertRaisesRegex(ValueError, "duplicate seed_index"):
                evaluate_case._validate_seed_identities(dup, case_def, "DEMO-001")

    def test_duplicate_philox_identity_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            case = Path(directory) / "case.json"
            _demo_case(case)
            case_def = json.loads(case.read_text(encoding="utf-8"))
            seeds = self._seeds(directory, [(1, 2), (1, 2)])
            # Indices 0 and 1, but index 1 must derive key [2, 2].
            with self.assertRaisesRegex(ValueError, "derivation|duplicate"):
                evaluate_case._validate_seed_identities(seeds, case_def, "DEMO-001")

    def test_derivation_mismatch_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            case = Path(directory) / "case.json"
            _demo_case(case, base_key=(100, 200))
            case_def = json.loads(case.read_text(encoding="utf-8"))
            seeds = self._seeds(directory, [(1, 2), (2, 2)])
            with self.assertRaisesRegex(ValueError, "derivation"):
                evaluate_case._validate_seed_identities(seeds, case_def, "DEMO-001")

    def test_reuse_derivation_allows_repeat_without_case_id_special_case(self):
        import io_corpus
        with tempfile.TemporaryDirectory() as directory:
            case = Path(directory) / "case.json"
            _demo_case(
                case,
                case_id="ARBITRARY-REPEAT-123",
                base_key=(5, 6),
                derivation="reuse_base_identity_v1",
            )
            case_def = json.loads(case.read_text(encoding="utf-8"))
            seeds = []
            for index in (0, 1):
                path = Path(directory) / f"seed_{index:03d}.json"
                _demo_seed(path, index, 0.0)
                data = json.loads(path.read_text(encoding="utf-8"))
                data["case_id"] = "ARBITRARY-REPEAT-123"
                data["philox_key"] = [5, 6]
                path.write_text(json.dumps(data), encoding="utf-8")
                seeds.append((str(path), io_corpus.read_seed_file(str(path))))
            evaluate_case._validate_seed_identities(
                seeds, case_def, "ARBITRARY-REPEAT-123"
            )

    def test_checked_in_repeat009_uses_executable_reuse_derivation(self):
        import io_corpus
        case_path = (
            evaluate_case.REPO_ROOT
            / "fixtures"
            / "corpus"
            / "cases"
            / "REPEAT-009.json"
        )
        case_def = json.loads(case_path.read_text(encoding="utf-8"))
        self.assertEqual(
            case_def["stochastic"]["candidate_philox"]["derivation"],
            "reuse_base_identity_v1",
        )
        with tempfile.TemporaryDirectory() as directory:
            seeds = []
            for index in (0, 1):
                path = Path(directory) / f"seed_{index:03d}.json"
                _demo_seed(path, index, 0.0)
                data = json.loads(path.read_text(encoding="utf-8"))
                data["case_id"] = "REPEAT-009"
                data["philox_key"] = list(
                    case_def["stochastic"]["candidate_philox"]["base_key"]
                )
                data["philox_counter"] = list(
                    case_def["stochastic"]["candidate_philox"]["base_counter"]
                )
                path.write_text(json.dumps(data), encoding="utf-8")
                seeds.append((str(path), io_corpus.read_seed_file(str(path))))
            evaluate_case._validate_seed_identities(
                seeds, case_def, "REPEAT-009"
            )

    def test_unknown_derivation_rejected_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            case = Path(directory) / "case.json"
            _demo_case(case, derivation="future_or_typo_policy")
            case_def = json.loads(case.read_text(encoding="utf-8"))
            with self.assertRaisesRegex(ValueError, "unsupported.*derivation"):
                evaluate_case._candidate_philox_block(case_def, "DEMO-001")

    def test_legacy_identical_repeats_flag_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            case = Path(directory) / "case.json"
            _demo_case(case, derivation="reuse_base_identity_v1")
            case_def = json.loads(case.read_text(encoding="utf-8"))
            case_def["stochastic"]["candidate_philox"]["identical_repeats"] = True
            with self.assertRaisesRegex(ValueError, "identical_repeats"):
                evaluate_case._candidate_philox_block(case_def, "DEMO-001")

    def test_seed_index_beyond_declared_count_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            case = Path(directory) / "case.json"
            _demo_case(case, count=2)
            case_def = json.loads(case.read_text(encoding="utf-8"))
            seeds = self._seeds(directory, [(1, 2)])
            seeds[0][1]["seed_index"] = 2
            with self.assertRaisesRegex(ValueError, "exceeds declared candidate count"):
                evaluate_case._validate_seed_identities(
                    seeds, case_def, "DEMO-001"
                )

    def test_wrapping_derivation_wraps_u32_key(self):
        with tempfile.TemporaryDirectory() as directory:
            case = Path(directory) / "case.json"
            _demo_case(case, base_key=(2**32 - 1, 7), count=2)
            case_def = json.loads(case.read_text(encoding="utf-8"))
            self.assertEqual(
                evaluate_case._expected_philox_identity(case_def, "DEMO-001", 1),
                ([0, 7], [0, 0, 0, 0]),
            )

    def test_deterministic_case_requires_exactly_one_candidate_artifact(self):
        import io_corpus
        with tempfile.TemporaryDirectory() as directory:
            case = Path(directory) / "case.json"
            _demo_case(case, candidate_philox=False)
            case_def = json.loads(case.read_text(encoding="utf-8"))
            seeds = self._seeds(directory, [(0, 0), (0, 0)])
            with self.assertRaisesRegex(ValueError, "exactly one"):
                evaluate_case._validate_seed_identities(
                    seeds, case_def, "DETERMINISTIC-001"
                )

    def test_nonexistent_oracle_directory_rejected(self):
        oracle_manifest = evaluate_case.load_oracle_manifest(
            str(evaluate_case.DEFAULT_ORACLE_MANIFEST))
        with tempfile.TemporaryDirectory() as directory:
            case = Path(directory) / "case.json"
            _demo_case(case)
            seed = Path(directory) / "seed_000.json"
            _demo_seed(seed, 0, 0.0)
            args = _namespaced(case_def=str(case),
                               candidate_seeds=[str(seed)],
                               fortran_output=str(Path(directory) / "missing"))
            with self.assertRaisesRegex(FileNotFoundError, "does not exist"):
                evaluate_case.run_corpus_seeds(args, oracle_manifest)


class ProvenanceTest(unittest.TestCase):
    """P1-3: no checkout-HEAD attribution for previously generated outputs."""

    def test_unknown_revision_is_explicit(self):
        oracle_manifest = evaluate_case.load_oracle_manifest(
            str(evaluate_case.DEFAULT_ORACLE_MANIFEST))
        args = _namespaced()
        _oracle, candidate, missing, _notes = evaluate_case.build_provenance(
            args, oracle_manifest)
        self.assertIsNone(candidate["revision"])
        self.assertIsNone(candidate["revision_source"])
        self.assertTrue(any(m["name"] == "candidate.revision" for m in missing))

    def test_manifest_hash_mismatch_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "output.json"
            artifact.write_text("{}", encoding="utf-8")
            manifest = {"oracle": {"commit": "x", "worktree_dirty": False},
                        "candidate": {"commit": "y", "worktree_dirty": False},
                        "output_sha256": {str(artifact.resolve()): "0" * 64}}
            with self.assertRaisesRegex(ValueError, "differs from run manifest"):
                evaluate_case._verify_artifacts_against_manifest(
                    manifest, [("candidate_output", str(artifact), "candidate")])

    def test_manifest_hash_match_verified(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "output.json"
            artifact.write_text("{}", encoding="utf-8")
            digest = report_lib.sha256_file(artifact)
            manifest = {"oracle": {"commit": "x", "worktree_dirty": False},
                        "candidate": {"commit": "y", "worktree_dirty": False},
                        "output_sha256": {str(artifact.resolve()): digest}}
            verified, uncovered = evaluate_case._verify_artifacts_against_manifest(
                manifest, [("candidate_output", str(artifact), "candidate")])
            self.assertEqual(verified, ["candidate_output"])
            self.assertEqual(uncovered, [])

    def test_mixed_unknown_embedded_revision_stays_unverified(self):
        # P2: ["abc123", "unknown"] must not attribute abc123 to the set.
        oracle_manifest = evaluate_case.load_oracle_manifest(
            str(evaluate_case.DEFAULT_ORACLE_MANIFEST))
        args = _namespaced()
        _oracle, candidate, missing, _notes = evaluate_case.build_provenance(
            args, oracle_manifest, embedded_revisions=["abc123", "unknown"])
        self.assertIsNone(candidate["revision"])
        self.assertIsNone(candidate["revision_source"])
        gaps = [m for m in missing if m["name"] == "candidate.revision"]
        self.assertEqual(len(gaps), 1)
        self.assertIn("1 of 2", gaps[0]["reason"])

    def test_partial_manifest_coverage_withholds_attribution(self):
        # P1: a manifest covering only some consumed oracle outputs must not
        # attribute the evaluated output to the pinned run.
        oracle_manifest = evaluate_case.load_oracle_manifest(
            str(evaluate_case.DEFAULT_ORACLE_MANIFEST))
        pinned = oracle_manifest["pinned_commit"]
        with tempfile.TemporaryDirectory() as directory:
            covered = Path(directory) / "grid_conc_000"
            covered.write_bytes(b"covered")
            extra = Path(directory) / "grid_conc_001"
            extra.write_bytes(b"from another run")
            manifest = {"oracle": {"commit": pinned, "worktree_dirty": False},
                        "candidate": {"commit": "y", "worktree_dirty": False},
                        "output_sha256": {
                            str(covered.resolve()): report_lib.sha256_file(covered)}}
            manifest_path = Path(directory) / "manifest.json"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            args = _namespaced(run_manifest=str(manifest_path))
            oracle, _candidate, missing, _notes = evaluate_case.build_provenance(
                args, oracle_manifest,
                artifact_paths=[("oracle_covered", str(covered), "oracle"),
                                ("oracle_extra", str(extra), "oracle")])
            self.assertEqual(oracle["attribution"], "unverified")
            self.assertTrue(any("oracle_extra" in m["name"] for m in missing))

    def test_checkout_alone_leaves_output_attribution_unverified(self):
        # P1: a verified source checkout does not link supplied outputs to a
        # run, so output attribution stays unverified.
        import subprocess
        try:
            subprocess.run(["git", "--version"], check=True, capture_output=True)
        except (subprocess.CalledProcessError, FileNotFoundError):
            self.skipTest("git is not available")
        with tempfile.TemporaryDirectory() as directory:
            subprocess.run(["git", "init"], cwd=directory, check=True,
                           capture_output=True)
            subprocess.run(["git", "config", "user.email", "t@t"], cwd=directory,
                           check=True, capture_output=True)
            subprocess.run(["git", "config", "user.name", "t"], cwd=directory,
                           check=True, capture_output=True)
            Path(directory, "f.txt").write_text("x", encoding="utf-8")
            subprocess.run(["git", "add", "."], cwd=directory, check=True,
                           capture_output=True)
            subprocess.run(["git", "commit", "-m", "x"], cwd=directory, check=True,
                           capture_output=True)
            head = subprocess.run(
                ["git", "rev-parse", "HEAD"], cwd=directory, check=True,
                capture_output=True, text=True).stdout.strip()
            oracle_manifest = {"pinned_commit": head, "name": "FLEXPART",
                               "version": "11.1"}
            args = _namespaced(oracle_checkout=directory)
            oracle, _candidate, _missing, _notes = evaluate_case.build_provenance(
                args, oracle_manifest)
            self.assertTrue(oracle["checkout_verified"])
            self.assertEqual(oracle["attribution"], "unverified")


class CrossCheckVerdictTest(unittest.TestCase):
    """P2-2: implementation divergence gets an explicit integrity verdict."""

    def _basis(self):
        lons = [10.0, 10.1, 9.9, 10.0]
        lats = [10.0, 10.0, 10.0, 10.1]
        heights = [50.0, 51.0, 52.0, 53.0]
        uniform = [1.0] * 4
        com = metrics.center_of_mass_particles(lons, lats, heights, uniform)
        cov = metrics.horizontal_covariance(lons, lats, uniform)
        quantiles = metrics.unweighted_quantiles_linear(heights, (0.1, 0.5, 0.9))
        mean_z = sum(heights) / 4
        std_z = (sum((z - mean_z) ** 2 for z in heights) / 4) ** 0.5
        runner = {
            "com_lon_deg": com["lon_deg"], "com_lat_deg": com["lat_deg"],
            "com_z_m": com["z_m"],
            "cov_east_m2": cov["covariance_km2"][0][0] * 1e6,
            "cov_north_m2": cov["covariance_km2"][1][1] * 1e6,
            "horizontal_eigenvalues_m2": [v * 1e6 for v in cov["eigenvalues_km2"]],
            "z_p10_m": quantiles["quantiles"]["0.1"],
            "z_p50_m": quantiles["quantiles"]["0.5"],
            "z_p90_m": quantiles["quantiles"]["0.9"],
            "z_mean_m": mean_z, "z_std_m": std_z,
            "total_mass_kg": 4.0,
        }
        return runner, com, cov, quantiles, mean_z, std_z

    def test_consistent_implementations(self):
        runner, com, cov, quantiles, mean_z, std_z = self._basis()
        result = evaluate_case._cross_check_runner_metrics(
            runner, com, cov, quantiles, mean_z, std_z, 4.0)
        self.assertEqual(result["verdict"], "CONSISTENT")
        self.assertEqual(result["violations"], [])

    def test_wide_spread_matches_runner_formula(self):
        # Runner block recomputed inline from the Rust formula in
        # src/bin/corpus-run.rs (unweighted means, R*cos/same metres per
        # degree, population variance/covariance, linear-index quantiles),
        # independently of metrics.horizontal_covariance. A ~0.1 degree
        # spread must stay CONSISTENT under the shared conversion.
        import math as _math
        earth = 6_371_000.0
        lons = [10.0, 10.1, 9.9, 10.05]
        lats = [10.0, 10.0, 10.05, 9.95]
        heights = [50.0, 60.0, 55.0, 65.0]
        n = 4
        com_lon = sum(lons) / n
        com_lat = sum(lats) / n
        com_z = sum(heights) / n
        m_lon = earth * _math.cos(_math.radians(com_lat)) * _math.pi / 180.0
        m_lat = earth * _math.pi / 180.0
        east = [(lon - com_lon) * m_lon for lon in lons]
        north = [(lat - com_lat) * m_lat for lat in lats]
        var_ee = sum(x * x for x in east) / n
        var_nn = sum(y * y for y in north) / n
        cov_en = sum(x * y for x, y in zip(east, north)) / n
        trace = var_ee + var_nn
        disc = max(trace * trace - 4.0 * (var_ee * var_nn - cov_en ** 2), 0.0) ** 0.5
        ordered = sorted(heights)

        def quantile(p):
            pos = p * (n - 1)
            low = int(_math.floor(pos))
            high = int(_math.ceil(pos))
            return ordered[low] + (ordered[high] - ordered[low]) * (pos - low)

        std_z = (sum((z - com_z) ** 2 for z in heights) / n) ** 0.5
        runner = {
            "com_lon_deg": com_lon, "com_lat_deg": com_lat, "com_z_m": com_z,
            "cov_east_m2": var_ee, "cov_north_m2": var_nn,
            "horizontal_eigenvalues_m2": [(trace + disc) / 2.0,
                                          (trace - disc) / 2.0],
            "z_p10_m": quantile(0.1), "z_p50_m": quantile(0.5),
            "z_p90_m": quantile(0.9),
            "z_mean_m": com_z, "z_std_m": std_z, "total_mass_kg": 4.0,
        }
        uniform = [1.0] * n
        uw_com = metrics.center_of_mass_particles(lons, lats, heights, uniform)
        uw_cov = metrics.horizontal_covariance(lons, lats, uniform)
        uw_quantiles = metrics.unweighted_quantiles_linear(heights, (0.1, 0.5, 0.9))
        result = evaluate_case._cross_check_runner_metrics(
            runner, uw_com, uw_cov, uw_quantiles, com_z, std_z, 4.0)
        self.assertEqual(result["verdict"], "CONSISTENT")
        self.assertEqual(result["violations"], [])

    def test_large_eigenvalue_gap_is_divergent(self):
        runner, com, cov, quantiles, mean_z, std_z = self._basis()
        runner["horizontal_eigenvalues_m2"] = [
            v + 1.5e6 for v in runner["horizontal_eigenvalues_m2"]]
        result = evaluate_case._cross_check_runner_metrics(
            runner, com, cov, quantiles, mean_z, std_z, 4.0)
        self.assertEqual(result["verdict"], "DIVERGENT")
        self.assertTrue(any(v["metric"] == "eigenvalue_large_m2"
                            for v in result["violations"]))


class InputAuditTest(unittest.TestCase):
    """P2-3: the ETEX audit artifact is consumed, not asserted."""

    def test_failed_audit_recorded(self):
        with tempfile.TemporaryDirectory() as directory:
            audit = Path(directory) / "audit.json"
            audit.write_text(json.dumps({"schema_version": 1,
                                         "status": "FAIL"}), encoding="utf-8")
            args = _namespaced(input_equivalence_report=str(audit))
            missing, notes, hashes = [], [], {}
            record = evaluate_case._consume_input_audit(
                args, "etex-mini", missing, notes, hashes)
            self.assertEqual(record["status"], "FAIL")
            self.assertIn("input_equivalence_report", hashes)
            self.assertFalse(any(m["name"] == "etex.input_equivalence_audit"
                                 for m in missing))

    def test_missing_audit_file_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            args = _namespaced(
                input_equivalence_report=str(Path(directory) / "absent.json"))
            with self.assertRaises(FileNotFoundError):
                evaluate_case._consume_input_audit(
                    args, "etex-mini", [], [], {})

    def test_absent_flag_leaves_status_unasserted(self):
        args = _namespaced()
        missing, notes, hashes = [], [], {}
        record = evaluate_case._consume_input_audit(
            args, "etex-mini", missing, notes, hashes)
        self.assertIsNone(record)
        self.assertTrue(any(m["name"] == "etex.input_equivalence_audit"
                            for m in missing))
        self.assertNotIn("INPUT_EQUIVALENCE_NOT_DEMONSTRATED",
                         " ".join(notes))


if __name__ == "__main__":
    unittest.main()
