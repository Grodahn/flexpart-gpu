#!/usr/bin/env python3
"""Negative audit regressions over fresh required #112 real-driver evidence.

Run after gpu::meteorology::advection_production_tests; absent evidence fails.
"""
import contextlib
import copy
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest

from check_resident_advection_evidence import audit


class ResidentAdvectionEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = Path("target/ci-gate/resident-advection-production")
        cls.report = json.loads((cls.source / "report.json").read_text(encoding="utf-8"))
        cls.supplement = json.loads((cls.source / "interior-deferred-boundary.json").read_text(encoding="utf-8"))
        cls.inputs = {r["input_file"] for r in cls.report["rows"] + cls.supplement["rows"]}

    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="resident-advection-audit-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        for name in self.inputs | {"report.json", "interior-deferred-boundary.json"}:
            shutil.copyfile(self.source / name, self.root / name)

    @staticmethod
    def successful(report):
        return next(r for r in report["rows"] if r["failure"] is None and r["active_count"] == 1)

    def reject(self, mutation):
        report = copy.deepcopy(self.report)
        mutation(report)
        (self.root / "report.json").write_text(json.dumps(report), encoding="utf-8")
        with self.assertRaises(AssertionError), contextlib.redirect_stdout(io.StringIO()):
            audit(self.root)

    def test_complete_real_driver_evidence_passes(self):
        with contextlib.redirect_stdout(io.StringIO()):
            audit(self.root)

    def test_pending_active_status_rejected(self):
        self.reject(lambda r: self.successful(r)["status"].__setitem__(1, 1))

    def test_missing_active_status_rejected(self):
        self.reject(lambda r: self.successful(r)["status"].__setitem__(1, 0))

    def test_unknown_status_rejected(self):
        self.reject(lambda r: self.successful(r)["status"].__setitem__(1, 10))

    def test_lane_failure_without_fatal_rejected(self):
        self.reject(lambda r: self.successful(r)["status"].__setitem__(1, 8))

    def test_inactive_tail_status_write_rejected(self):
        self.reject(lambda r: self.successful(r)["status"].__setitem__(2, 2))

    def test_missing_case_rejected(self):
        self.reject(lambda r: r["rows"].pop())

    def test_stale_revision_rejected(self):
        self.reject(lambda r: r.__setitem__("candidate_revision", "0" * 40))

    def test_null_sample_for_valid_lane_rejected(self):
        self.reject(lambda r: self.successful(r)["sampled_values"].__setitem__(0, None))

    def test_nonfinite_sample_for_valid_lane_rejected(self):
        self.reject(lambda r: self.successful(r)["sampled_values"].__setitem__(0, float("inf")))

    def test_arithmetic_failure_in_success_report_rejected(self):
        self.reject(lambda r: self.successful(r)["status"].__setitem__(-1, 1))

    def test_all_failed_lanes_cannot_replace_mixed_failure_proof(self):
        def replace(report):
            row = next(r for r in report["rows"] if r["failure"] == [1, 1] and r["active_count"] == 3)
            row["status"][4 * 131 + 2] = 8
        self.reject(replace)

    def test_missing_deferred_evidence_rejected(self):
        (self.root / "interior-deferred-boundary.json").unlink()
        with self.assertRaises(FileNotFoundError), contextlib.redirect_stdout(io.StringIO()):
            audit(self.root)


if __name__ == "__main__":
    unittest.main()
