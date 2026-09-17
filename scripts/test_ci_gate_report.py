"""Fail-closed checks for the CI gate report writer (Issue #6 point 4).

Technical gate only. These checks enforce that unwired corpus cases are
never reported as passed and that no scientific parity verdict is claimed.
"""

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
REPORT_SCRIPT = REPO_ROOT / "scripts" / "ci-gate-report.py"


def run_report(output_dir, status="TECHNICAL_FAIL", failure="unit-test"):
    output = Path(output_dir) / "ci-gate-report.json"
    subprocess.run(
        ["python", str(REPORT_SCRIPT),
         "--output", str(output),
         "--project-root", str(REPO_ROOT),
         "--output-dir", str(output_dir),
         "--oracle-checkout", str(REPO_ROOT.parent / "flexpart"),
         "--status", status,
         "--particles", "1000",
         "--allowlist", "SW-WGPU-ADVECTION-001 SYNTHETIC-UNIFORM-WIND-SMOKE",
         "--failure", failure],
        check=True, capture_output=True, text=True,
    )
    return json.loads(output.read_text(encoding="utf-8"))


class CiGateReportTest(unittest.TestCase):
    def test_report_never_claims_scientific_parity(self):
        with tempfile.TemporaryDirectory() as directory:
            report = run_report(directory)
            self.assertEqual(report["schema_version"], "1.0")
            self.assertEqual(report["gate_id"], "ci-technical-gate")
            self.assertEqual(report["scientific_verdict"], "NOT_EVALUATED")
            self.assertIn(report["status"], ("TECHNICAL_PASS", "TECHNICAL_FAIL"))
            self.assertEqual(
                report["scientific_thresholds"]["status"], "NOT_APPLIED")

    def test_pending_corpus_cases_are_never_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            report = run_report(directory)
            self.assertGreater(len(report["pending_corpus_cases"]), 0)
            for entry in report["pending_corpus_cases"]:
                self.assertEqual(entry["status"], "NOT_WIRED")
                self.assertNotIn("PASS", entry["status"])
            # Known corpus IDs from the parallel track stay listed.
            ids = {entry["case_id"] for entry in report["pending_corpus_cases"]}
            for expected in ("ADV-ANA-001", "PBL-NEUTRAL-005", "ETEX-MINI-013"):
                self.assertIn(expected, ids)

    def test_allowlisted_cases_do_not_claim_science(self):
        with tempfile.TemporaryDirectory() as directory:
            report = run_report(directory, status="TECHNICAL_PASS",
                                failure="none")
            self.assertEqual(report["status"], "TECHNICAL_PASS")
            for case in report["cases"]:
                self.assertIn(case["status"], ("PASS", "FAIL"))
                self.assertIn("no scientific parity", case["note"].lower())


if __name__ == "__main__":
    unittest.main()
