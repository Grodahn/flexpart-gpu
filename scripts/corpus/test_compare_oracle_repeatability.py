#!/usr/bin/env python3
"""Fail-closed checks for the oracle repeatability comparator (#49).

Standard library unittest only. Covers numeric difference quantification,
contract validation, and retained-repetition completeness without running
FLEXPART or claiming physics parity.
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts" / "corpus"))

import compare_oracle_repeatability as cmp_mod


class CompareNumbersTest(unittest.TestCase):
    def test_identical_payload_has_no_differences(self):
        payload = {"a": 1.0, "b": [1.0, 2.0], "c": {"d": "x"}}
        diffs = []
        cmp_mod.compare_numbers(payload, json.loads(json.dumps(payload)), "", diffs)
        self.assertEqual([d for d in diffs if d["field"]], [])

    def test_numeric_difference_reports_abs_and_rel(self):
        diffs = []
        cmp_mod.compare_numbers({"v": 100.0}, {"v": 110.0}, "", diffs)
        self.assertEqual(len(diffs), 1)
        self.assertEqual(diffs[0]["field"], "v")
        self.assertAlmostEqual(diffs[0]["abs_diff"], 10.0)
        self.assertAlmostEqual(diffs[0]["rel_diff"], 10.0 / 110.0)

    def test_list_difference_names_index(self):
        diffs = []
        cmp_mod.compare_numbers([1.0, 2.0], [1.0, 3.0], "levels", diffs)
        self.assertEqual(diffs[0]["field"], "levels[1]")
        self.assertAlmostEqual(diffs[0]["abs_diff"], 1.0)

    def test_summary_reports_maximums(self):
        diffs = [
            {"field": "a", "baseline": 1.0, "current": 2.0, "abs_diff": 1.0, "rel_diff": 0.5, "kind": "number"},
            {"field": "b", "baseline": 10.0, "current": 13.0, "abs_diff": 3.0, "rel_diff": 0.3, "kind": "number"},
        ]
        summary = cmp_mod.summarize_differences(diffs)
        self.assertEqual(summary["differing_fields"], 2)
        self.assertAlmostEqual(summary["max_abs_diff"], 3.0)
        self.assertAlmostEqual(summary["max_rel_diff"], 0.5)


class ProfileContractTest(unittest.TestCase):
    def test_canonical_profile_loads(self):
        reference, profile = cmp_mod.load_profile(REPO / "reference/flexpart-11.1.json")
        self.assertEqual(profile["id"], "flexpart-11.1-single-thread")
        self.assertEqual(profile["repeatability_cases"]["ADV-ANA-001"], "deterministic")
        self.assertEqual(profile["external_seed_control"], "unavailable")
        self.assertIn("pinned_commit", reference)

    def test_profile_rejects_substituted_case(self):
        reference = json.loads((REPO / "reference/flexpart-11.1.json").read_text(encoding="utf-8"))
        reference["execution_profile"]["repeatability_cases"] = {"OTHER": "deterministic"}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ref.json"
            path.write_text(json.dumps(reference), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "repeatability cases"):
                cmp_mod.load_profile(path)

    def test_profile_rejects_seed_control_claim(self):
        reference = json.loads((REPO / "reference/flexpart-11.1.json").read_text(encoding="utf-8"))
        reference["execution_profile"]["external_seed_control"] = "seed-42"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ref.json"
            path.write_text(json.dumps(reference), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unavailable"):
                cmp_mod.load_profile(path)


def write_rep(case_dir, name, raw_bytes, summary, runtime):
    rep = case_dir / name
    raw = rep / "raw"
    raw.mkdir(parents=True)
    (raw / "header").write_bytes(raw_bytes)
    (raw / "dates").write_bytes(b"dates")
    (raw / "grid_conc_20240101010000").write_bytes(raw_bytes)
    (rep / "oracle_summary.json").write_text(json.dumps(summary), encoding="utf-8")
    (rep / "runtime_profile.json").write_text(json.dumps(runtime), encoding="utf-8")
    (rep / "fortran.log").write_text("FLEXPART ... CONGRATULATIONS\n", encoding="utf-8")
    for artifact in ("COMMAND", "RELEASES", "OUTGRID"):
        (rep / artifact).write_text(f"{artifact}\n", encoding="utf-8")
    return rep


class RepeatabilityReportTest(unittest.TestCase):
    def make_layout(self, root, summary_b, raw_b=b"same"):
        manifest = REPO / "reference/flexpart-11.1.json"
        reference = json.loads(manifest.read_text(encoding="utf-8"))
        profile = reference["execution_profile"]
        runtime = {
            "execution_profile": {"id": profile["id"], "version": profile["version"]},
            "reference_manifest_sha256": cmp_mod.digest(manifest),
            "runtime_environment": profile["runtime_environment"],
        }
        repeat = root / "repeat"
        for case in ("ADV-ANA-001", "WIND-UNI-002"):
            case_dir = repeat / case
            summary_a = {"last_slice": {"weighted": {"airborne": 1.0}}, "center_of_mass": {"z_m": 50.0}}
            write_rep(case_dir, "rep_01", b"same", summary_a, runtime)
            write_rep(case_dir, "rep_02", raw_b, summary_b, runtime)
            for n in ("rep_03", "rep_04", "rep_05"):
                write_rep(case_dir, n, b"same", summary_a, runtime)
        meteo = root / "meteo"
        for case in ("ADV-ANA-001", "WIND-UNI-002"):
            d = meteo / case
            d.mkdir(parents=True)
            (d / "AVAILABLE").write_text("x\n", encoding="utf-8")
            (d / "meteo.grb").write_bytes(b"meteo")
        return repeat, meteo

    def run_comparator(self, root, repeat, meteo):
        # Resolve a pinned clean oracle checkout: prefer FLEXPART_DIR, then sibling.
        import os
        candidates = []
        if os.environ.get("FLEXPART_DIR"):
            candidates.append(Path(os.environ["FLEXPART_DIR"]))
        candidates += [REPO.parent / "flexpart", Path("/workspace/flexpart")]
        checkout = next((c for c in candidates if (c / "src/FLEXPART").is_file()), None)
        if checkout is None:
            self.skipTest("pinned oracle checkout with built executable not available")
        exe = checkout / "src/FLEXPART"
        out = root / "report.json"
        cmd = [sys.executable, str(REPO / "scripts/corpus/compare_oracle_repeatability.py"),
               "--repeat-dir", str(repeat), "--output", str(out),
               "--oracle-manifest", str(REPO / "reference/flexpart-11.1.json"),
               "--oracle-checkout", str(checkout), "--oracle-exe", str(exe),
               "--meteo-dir", str(meteo),
               "--fixtures-dir", str(REPO / "fixtures/corpus/fortran")]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        return proc, out

    def test_byte_identical_repetitions_characterized_without_parity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline = {"last_slice": {"weighted": {"airborne": 1.0}}, "center_of_mass": {"z_m": 50.0}}
            repeat, meteo = self.make_layout(root, baseline, b"same")
            proc, out = self.run_comparator(root, repeat, meteo)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            report = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(report["status"], "ORACLE_REPEATABILITY_CHARACTERIZED_NO_PARITY_VERDICT")
            self.assertEqual(report["external_seed_control"], "unavailable")
            for case in ("ADV-ANA-001", "WIND-UNI-002"):
                self.assertTrue(report["cases"][case]["all_byte_identical_to_baseline"])
                self.assertIn("bitwise repeatable", report["cases"][case]["repeatability_observation"])

    def test_differing_repetition_names_files_and_quantifies_numbers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            changed = {"last_slice": {"weighted": {"airborne": 1.1}}, "center_of_mass": {"z_m": 55.0}}
            repeat, meteo = self.make_layout(root, changed, b"changed")
            proc, out = self.run_comparator(root, repeat, meteo)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            report = json.loads(out.read_text(encoding="utf-8"))
            rep2 = next(r for r in report["cases"]["ADV-ANA-001"]["repetitions"] if r["rep"] == "rep_02")
            self.assertFalse(rep2["byte_identical_to_baseline"])
            self.assertIn("grid_conc_20240101010000", rep2["differing_raw_files"])
            fields = {d["field"]: d for d in rep2["differing_decoded_fields"]}
            self.assertIn("last_slice.weighted.airborne", fields)
            self.assertAlmostEqual(fields["last_slice.weighted.airborne"]["abs_diff"], 0.1)
            self.assertIsNotNone(rep2["decoded_numeric_summary"]["max_abs_diff"])

    def test_missing_repetition_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline = {"v": 1.0}
            repeat, meteo = self.make_layout(root, baseline, b"same")
            # Remove four reps so only one remains per case.
            for case in ("ADV-ANA-001", "WIND-UNI-002"):
                for name in ("rep_02", "rep_03", "rep_04", "rep_05"):
                    for path in (repeat / case / name).rglob("*"):
                        pass
                    import shutil
                    shutil.rmtree(repeat / case / name)
            proc, _ = self.run_comparator(root, repeat, meteo)
            self.assertNotEqual(proc.returncode, 0)


if __name__ == "__main__":
    unittest.main()
