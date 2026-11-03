#!/usr/bin/env python3
"""Self-tests for the #233 ICON-EU surface-flux evidence tooling and decision.

Offline: the tests use the committed #230 step-0 ``ASHFL_S`` message, the
frozen #233 evidence/decision fixtures and synthetic arrays. They need
neither ecCodes nor network access; numpy-dependent tests are skipped when
numpy is absent. Mutated GRIB2 bytes exist only in memory.

Run: python -I scripts/provider/test_icon_eu_flux_audit.py
"""

import datetime
import hashlib
import json
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
import icon_eu_flux_audit as audit  # noqa: E402
import icon_eu_grib2_sections as sections  # noqa: E402

try:
    import numpy
except ImportError:  # pragma: no cover - exercised only without numpy
    numpy = None

FIXTURE = os.path.join(ROOT, "fixtures", "meteorology", "icon-eu-flux-233-v1")
DECISION = os.path.join(FIXTURE, "decision.json")
EVIDENCE = os.path.join(FIXTURE, "flux-evidence.json")
MANIFEST = os.path.join(FIXTURE, "retrieval-manifest.json")
REQUEST = os.path.join(FIXTURE, "sample-request.json")
STEP0_ASHFL = os.path.join(ROOT, "fixtures", "meteorology", "icon-eu-audit-v1", "raw", "2026103100",
                           "icon-eu_europe_regular-lat-lon_single-level_2026103100_000_ASHFL_S.grib2")
STEP0_CYCLE = datetime.datetime(2026, 10, 31, tzinfo=datetime.timezone.utc)
CANONICAL_FLUX_FIELDS = {"sensible_heat_flux", "surface_solar_radiation",
                         "surface_stress_eastward", "surface_stress_northward"}
# 0-based index of WMO octet n inside section 4 is n - 1.
SECTION4_STATISTICAL_PROCESSING = 46
SECTION4_TIME_UNIT = 17
SECTION4_FORECAST_TIME = 18


def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def mutate_section4(message, index, value):
    """Return a copy of ``message`` with one section-4 octet replaced."""
    found = sections.sections(message)
    offset = message.index(found[4])
    data = bytearray(message)
    data[offset + index] = value
    return bytes(data)


class FormulaTests(unittest.TestCase):
    def test_interval_mean_recovers_constant_and_piecewise_flux(self):
        self.assertAlmostEqual(audit.interval_mean(12, 40.0, 13, 40.0), 40.0)
        # 10 W m-2 for 12 h, then 140 W m-2 for 1 h -> averages 10 and 20.
        self.assertAlmostEqual(audit.interval_mean(12, 10.0, 13, 20.0), 140.0)
        # 3-hourly cadence after 78 h.
        self.assertAlmostEqual(audit.interval_mean(78, 1.0, 81, 2.0), (162.0 - 78.0) / 3.0)

    def test_quantization_bound_grows_with_lead_time(self):
        error = 0.5 / 256.0
        self.assertAlmostEqual(audit.quantization_bound(12, error, 13, error), 25 * error)
        self.assertAlmostEqual(audit.quantization_bound(77, error, 78, error), 155 * error)
        self.assertAlmostEqual(audit.quantization_bound(78, error, 81, error), 159 * error / 3)

    @unittest.skipIf(numpy is None, "numpy not installed")
    def test_packing_error_bound_includes_float32_reference_spacing(self):
        representation = {"binary_scale_factor": -8, "decimal_scale_factor": 0,
                          "reference_value": 1.3012830018997192}
        expected = 0.5 / 256.0 + 0.5 * float(numpy.spacing(numpy.float32(1.3012830018997192)))
        self.assertEqual(audit.packing_error_bound(representation), expected)
        self.assertGreater(audit.packing_error_bound(representation), 0.5 / 256.0)

    @unittest.skipIf(numpy is None, "numpy not installed")
    def test_packing_error_bound_widens_for_negative_reference(self):
        positive = {"binary_scale_factor": -7, "decimal_scale_factor": 0, "reference_value": 358.2703}
        negative = dict(positive, reference_value=-358.2703)
        self.assertEqual(audit.packing_error_bound(negative), audit.packing_error_bound(positive))
        self.assertGreater(audit.packing_error_bound(negative), 0.5 / 128.0)

    @unittest.skipIf(numpy is None, "numpy not installed")
    def test_bound_masks_use_only_float_slack(self):
        bound = 0.1015625
        values = numpy.array([-bound, -bound * (1 + 1e-12), -bound * 1.001, bound * 1.001])
        self.assertEqual(audit.below(values, bound).tolist(), [False, False, True, False])
        self.assertEqual(audit.beyond(values, bound).tolist(), [False, False, True, True])

    @unittest.skipIf(numpy is None, "numpy not installed")
    def test_solar_elevation_noon_and_midnight(self):
        moment = datetime.datetime(2026, 11, 3, 12, tzinfo=datetime.timezone.utc)
        # Declination near -15.2 deg on 3 November; noon at 0 deg E, 45 deg N.
        noon = float(audit.solar_elevation_deg(numpy.array(45.0), numpy.array(0.0), moment))
        self.assertAlmostEqual(noon, 90.0 - 45.0 - 15.2, delta=0.5)
        midnight = float(audit.solar_elevation_deg(numpy.array(45.0), numpy.array(180.0), moment))
        self.assertLess(midnight, -50.0)


class CoverageTests(unittest.TestCase):
    def store(self, steps_by_variable):
        store = audit.Store.__new__(audit.Store)
        store.fields = {(variable, step): None
                        for variable, steps in steps_by_variable.items() for step in steps}
        return store

    def test_missing_required_message_fails_closed(self):
        with self.assertRaises(audit.AuditError):
            self.store({"ASHFL_S": [0, 1]}).field("T_G", 13)

    def test_flux_step_lists_must_match_and_start_at_zero(self):
        complete = {variable: [0, 1, 2] for variable in audit.AVERAGED_FLUXES}
        self.store(complete).check_flux_coverage()
        with self.assertRaises(audit.AuditError):
            self.store(dict(complete, ASOB_S=[0, 2])).check_flux_coverage()
        with self.assertRaises(audit.AuditError):
            self.store({variable: [1, 2] for variable in audit.AVERAGED_FLUXES}).check_flux_coverage()


class IdentityTests(unittest.TestCase):
    def setUp(self):
        with open(STEP0_ASHFL, "rb") as handle:
            self.message = handle.read()

    def problems(self, message, cycle=STEP0_CYCLE, variable="ASHFL_S"):
        return audit.identity_problems(sections.parse(message), variable, cycle, 0)

    def test_real_step0_average_has_expected_identity(self):
        self.assertEqual(self.problems(self.message), [])

    def test_accumulation_instead_of_average_is_rejected(self):
        mutated = mutate_section4(self.message, SECTION4_STATISTICAL_PROCESSING, 1)
        self.assertTrue(any("statistical processing" in item for item in self.problems(mutated)))

    def test_hours_instead_of_minutes_is_rejected(self):
        mutated = mutate_section4(self.message, SECTION4_TIME_UNIT, 1)
        self.assertTrue(any("unit of time range" in item for item in self.problems(mutated)))

    def test_averaging_start_after_run_start_is_rejected(self):
        mutated = mutate_section4(self.message, SECTION4_FORECAST_TIME + 3, 60)
        self.assertTrue(any("averaging start" in item for item in self.problems(mutated)))

    def test_wrong_cycle_and_wrong_parameter_are_rejected(self):
        other_cycle = STEP0_CYCLE + datetime.timedelta(hours=3)
        self.assertTrue(any("reference time" in item for item in self.problems(self.message, other_cycle)))
        self.assertTrue(any("parameter" in item for item in self.problems(self.message, variable="ASOB_S")))

    def test_unknown_variable_fails_closed(self):
        with self.assertRaises(audit.AuditError):
            self.problems(self.message, variable="ALHFL_S")

    def test_compare_reports_value_and_shape_differences(self):
        left = {"decoder": {"label": "a"}, "x": [1.0, 2], "y": "s"}
        self.assertEqual(list(audit.differences(left, {"decoder": {"label": "b"}, "x": [1.0, 2], "y": "s"})), [])
        self.assertEqual(len(list(audit.differences(left, {"decoder": {}, "x": [1.1, 2], "y": "s"}))), 1)
        self.assertEqual(len(list(audit.differences(left, {"decoder": {}, "x": [1.0], "y": "s"}))), 1)


class FrozenEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.decision = load(DECISION)
        cls.evidence = load(EVIDENCE)

    def test_decision_binds_exact_evidence_bytes(self):
        bound = self.decision["evidence"]
        self.assertEqual(audit.canonical_sha256(EVIDENCE), bound["evidence_sha256"])
        self.assertEqual(audit.canonical_sha256(MANIFEST), bound["retrieval_manifest_sha256"])
        self.assertEqual(self.evidence["retrieval_manifest_sha256"], bound["retrieval_manifest_sha256"])
        with open(REQUEST, "rb") as handle:
            request_sha = hashlib.sha256(handle.read().replace(b"\r\n", b"\n")).hexdigest()
        self.assertEqual(self.evidence["request_sha256"], request_sha)
        self.assertEqual(self.evidence["identity_problem_count"], 0)
        self.assertEqual(self.evidence["decoder"]["label"], bound["primary_decoder"])

    def test_every_published_flux_step_has_one_message_and_interval(self):
        request = load(REQUEST)
        flux_group = next(group for group in request["groups"] if "ASHFL_S" in group["variables"])
        steps = flux_group["steps"]
        for variable in audit.AVERAGED_FLUXES:
            rows = self.evidence["interval_bounds"][variable]
            self.assertEqual([(row["start_h"], row["end_h"]) for row in rows], list(zip(steps[:-1], steps[1:])))
            self.assertTrue(self.evidence["step_zero"][variable]["all_zero"])

    def test_interval_bounds_recompute_from_message_records(self):
        messages = {(row["variable"], row["step_h"]): row for row in self.evidence["messages"]}
        for variable, rows in self.evidence["interval_bounds"].items():
            for row in rows:
                first = messages[(variable, row["start_h"])]
                last = messages[(variable, row["end_h"])]
                self.assertEqual(row["start_grib2_sha256"], first["grib2_sha256"])
                self.assertEqual(row["end_grib2_sha256"], last["grib2_sha256"])
                expected = audit.quantization_bound(row["start_h"], first["packing_error_bound"],
                                                    row["end_h"], last["packing_error_bound"])
                self.assertAlmostEqual(row["quantization_bound"], expected, delta=1e-15)

    def test_averaged_messages_cover_run_start_to_valid_time_in_minutes(self):
        for row in self.evidence["messages"]:
            if row["product_template"] != audit.PRODUCT_TEMPLATE_STATISTICAL:
                continue
            self.assertEqual(row["statistical_processing"], audit.STATISTICAL_AVERAGE, row["path"])
            self.assertEqual(row["averaging_start_minutes"], 0, row["path"])
            self.assertEqual(row["averaging_length_minutes"], 60 * row["step_h"], row["path"])

    def test_decision_quantization_table_matches_evidence(self):
        table = self.decision["quantization"]["bounds_by_field"]
        for variable, rows in self.evidence["interval_bounds"].items():
            hourly = [row for row in rows if row["length_h"] == 1]
            longer = [row for row in rows if row["length_h"] > 1]
            entry = table[variable]
            self.assertEqual(entry["first_hourly_interval_bound"], hourly[0]["quantization_bound"])
            self.assertEqual(entry["last_hourly_interval"], [hourly[-1]["start_h"], hourly[-1]["end_h"]])
            self.assertEqual(entry["last_hourly_interval_bound"], hourly[-1]["quantization_bound"])
            self.assertEqual(entry["max_hourly_bound"], max(row["quantization_bound"] for row in hourly))
            self.assertEqual(entry["max_three_hourly_bound"], max(row["quantization_bound"] for row in longer))

    def test_decision_signs_follow_decoded_checks(self):
        signs = {item["source"]: item for item in self.decision["fields"]}
        for row in self.evidence["sensible_heat_sign"]:
            self.assertGreater(row["fraction_minus_ashfl_sign_equals_tg_minus_t2m_sign"],
                               self.decision["sign_evidence_thresholds"]["minimum_agreement_fraction"])
        self.assertEqual(signs["ASHFL_S"]["sign_transformation"], "negate")
        for row in self.evidence["stress_sign"]:
            self.assertGreater(row["fraction_same_sign_as_wind"],
                               self.decision["sign_evidence_thresholds"]["minimum_agreement_fraction"])
            self.assertGreater(row["correlation_with_mean_wind"], 0.0)
        self.assertEqual(signs["AUMFL_S"]["sign_transformation"], "identity")
        self.assertEqual(signs["AVMFL_S"]["sign_transformation"], "identity")
        for row in self.evidence["shortwave_budget"]:
            self.assertEqual(row["net_above_downward_cells"], 0)

    def test_decision_shortwave_findings_match_evidence(self):
        summary = self.evidence["summary"]
        finding = self.decision["shortwave_consistency"]
        for key in ("asob_negative_beyond_bound_cell_intervals", "asob_night_beyond_bound_cell_intervals",
                    "asob_violation_columns_from_east_edge", "asob_violation_latitude_range"):
            self.assertEqual(finding[key], summary[key], key)
        for row in self.evidence["shortwave_interval_consistency"]:
            for cell in row["below_minus_bound"] + row["night_beyond_bound"]:
                self.assertLessEqual(cell["columns_from_east_edge"], finding["edge_band_columns"] - 1)

    def test_decision_verdicts_are_explicit_and_never_instantaneous(self):
        vocabulary = set(self.decision["verdict_vocabulary"])
        self.assertEqual({item["canonical_id"] for item in self.decision["fields"]}, CANONICAL_FLUX_FIELDS)
        for item in self.decision["fields"]:
            for key in ("interval_mean_verdict", "instantaneous_verdict", "calcpar_consumer_verdict"):
                self.assertIn(item[key], vocabulary, f"{item['canonical_id']} {key}")
            self.assertEqual(item["canonical_temporal_kind"], "interval_mean")
            self.assertEqual(item["instantaneous_verdict"], "NOT_ADMISSIBLE")
            self.assertEqual(item["calcpar_consumer_verdict"], "REQUIRES_NEW_TEMPORAL_ORACLE")
        self.assertTrue(self.decision["follow_up_issues"])


if __name__ == "__main__":
    unittest.main(verbosity=1)
