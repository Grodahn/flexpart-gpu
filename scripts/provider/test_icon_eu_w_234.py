#!/usr/bin/env python3
"""Offline lock and analytical checks for the ICON-EU W research decision."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import icon_eu_w_234 as research  # noqa: E402


class ResearchDecisionTests(unittest.TestCase):
    def test_versioned_verdict_and_artifact_digests(self):
        path = research.DECISION / "decision.json"
        self.assertEqual(research.digest(path),
                         "61eae71722f488cfbba9742d1febbb2f0efe524de49a500a78c5084f9da73f17")
        decision = research.load(path)
        self.assertEqual((decision["issue"], decision["version"], decision["verdict"]),
                         (234, 1, "UNSUPPORTED"))
        self.assertIsNone(decision["admitted_motion_kind"])
        self.assertFalse(decision["real_data_coverage"]["pointwise_paired_motion_comparison"])
        root = research.METEO.parents[1]
        for artifact, expected in decision["artifact_sha256"].items():
            with self.subTest(artifact=artifact):
                self.assertEqual(research.digest(root / artifact), expected)
        reference = research.load(root / "reference" / "flexpart-11.1.json")
        self.assertEqual(decision["references"]["flexpart_commit"], reference["pinned_commit"])

    def test_exact_analytical_and_independent_particle_reference(self):
        cases = research.analytical()
        self.assertEqual(cases, research.load(research.DECISION / "evidence.json")["analytic_exact"])
        self.assertEqual(len(cases), 5)
        self.assertEqual(cases["tilted_surface"],
                         {"interface_relative_m_s": 0, "agl_rate_m_s": 0})

    def test_interface_wind_requires_positive_bracket(self):
        for upper, lower, interface in ((12, 4, 13), (12, 4, 4), (4, 12, 6), (4, 4, 4)):
            with self.subTest(upper=upper, lower=lower, interface=interface):
                with self.assertRaises(ValueError):
                    research.half_wind(1, 0, upper, lower, interface)

    def test_pinned_wind_and_geometry_handoff(self):
        evidence = research.load(research.DECISION / "evidence.json")
        wind = research.retained_wind()
        self.assertEqual(evidence["wind_230_retained"], wind)
        measured = evidence["geometry_redecoded"]
        _, audit = research.audit_rows()
        archive = research.load(research.VERTICAL / "decoded-evidence.json")
        rows = {str(row["level"]): row for row in archive["messages"] if row["variable"] == "HHL"}
        self.assertEqual(set(measured["hhl"]), {str(level) for level in research.LEVELS})
        for level, row in measured["hhl"].items():
            with self.subTest(level=level):
                original = audit[f"{level}_HHL"]
                self.assertEqual(row["values_sha256_f8le"], original["values"]["values_sha256_f8le"])
                self.assertEqual(row["values_sha256_f8le"], rows[level]["values_sha256_f8le"])
                self.assertEqual(row["archive_grib2_sha256"], rows[level]["grib2_sha256"])
                self.assertEqual(row["audit_230_grib2_sha256"], original["grib2_sha256"])
        self.assertEqual(measured["centered_interior_count"], 900625)
        self.assertEqual(measured["slopes"]["1"]["max_abs_asl_slope"], [0.0, 0.0])
        self.assertEqual(measured["slopes"]["75"]["max_abs_agl_slope"], [0.0, 0.0])
        self.assertEqual(evidence["interior_bounds_from_extrema"], research.wind_bounds(wind, measured))


if __name__ == "__main__":
    unittest.main()
