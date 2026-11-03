#!/usr/bin/env python3
"""Offline self-tests for the #231 vertical-evidence tooling (no download, no ecCodes).

Run: ``python -I scripts/provider/test_icon_eu_vertical_231.py``. The tests use
the committed decoded evidence and synthetic columns. They check that the
summarizer refuses disagreeing decoders, that the request expands to the exact
audited file set, and that the geometry helpers match their analytical forms.
"""

import copy
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import icon_eu_audit as audit  # noqa: E402
import icon_eu_vertical_231 as vertical  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
FIXTURE = os.path.join(ROOT, "fixtures", "meteorology", "icon-eu-vertical-231-v1")
EXPECTED_FILES = 301


def load(name):
    with open(os.path.join(FIXTURE, name), encoding="utf-8") as handle:
        return json.load(handle)


class SummarizeTests(unittest.TestCase):
    def setUp(self):
        evidence = load("decoded-evidence.json")
        self.primary = {key: evidence[key] for key in ("decoder", "messages", "science", "columns")}
        self.secondary = copy.deepcopy(self.primary)
        self.secondary["decoder"] = {"label": "secondary", "eccodes": "x", "independent_values": None}
        for record in self.secondary["messages"]:
            record["independent_values_max_abs_difference"] = None

    def test_identical_decoders_are_accepted(self):
        result = vertical.summarize(self.primary, self.secondary)
        self.assertEqual(result["secondary_max_relative_difference"], 0.0)

    def test_primary_without_independent_decode_is_refused(self):
        self.primary["decoder"]["independent_values"] = None
        with self.assertRaises(audit.AuditError):
            vertical.summarize(self.primary, self.secondary)

    def test_value_hash_drift_is_refused(self):
        self.secondary["messages"][0]["values_sha256_f8le"] = "0" * 64
        with self.assertRaises(audit.AuditError):
            vertical.summarize(self.primary, self.secondary)

    def test_raw_column_drift_is_refused(self):
        self.secondary["columns"][0]["hhl_m"][10] += 1.0e-9
        with self.assertRaises(audit.AuditError):
            vertical.summarize(self.primary, self.secondary)

    def test_derived_drift_beyond_summation_order_is_refused(self):
        stats = self.secondary["science"]["flexpart_reconstruction_minus_hhl_full_levels"]["all_levels"]
        stats["max_abs"] *= 1.0 + 1.0e-9
        with self.assertRaises(audit.AuditError):
            vertical.summarize(self.primary, self.secondary)


class GeometryTests(unittest.TestCase):
    def test_request_expands_to_the_audited_file_set(self):
        with open(os.path.join(FIXTURE, "sample-request.json"), encoding="utf-8") as handle:
            identities = vertical.expected_identities(json.load(handle))
        self.assertEqual(len(identities), EXPECTED_FILES)
        self.assertIn(("time-invariant", "HHL", None, 75), identities)
        self.assertIn(("model-level", "QV", 0, 74), identities)

    def test_wzlev_style_displacement_is_quarter_second_difference(self):
        numpy = vertical.numpy_module()
        hhl = numpy.array([[1000.0], [600.0], [300.0], [100.0], [0.0]])
        expected = 0.25 * (hhl[:-2] - 2.0 * hhl[1:-1] + hhl[2:])
        numpy.testing.assert_allclose(vertical.wzlev_style_minus_hhl(hhl, hhl[-1]), expected, rtol=0, atol=1e-12)

    def test_isothermal_dry_column_matches_analytical_hypsometric_height(self):
        numpy = vertical.numpy_module()
        temperature = 260.0
        pressure = numpy.array([[50000.0], [70000.0], [90000.0]])
        surface = numpy.array([100000.0])
        dew_point = numpy.array([150.0])  # negligible vapour pressure at the surface boundary
        heights = vertical.flexpart_heights(surface, numpy.array([temperature]), dew_point, pressure,
                                            numpy.full(pressure.shape, temperature), numpy.zeros(pressure.shape))
        scale = vertical.FLEXPART_R_AIR / vertical.FLEXPART_GA * temperature
        numpy.testing.assert_allclose(heights[:, 0], scale * numpy.log(surface[0] / pressure[:, 0]), rtol=1e-6)


if __name__ == "__main__":
    unittest.main()
