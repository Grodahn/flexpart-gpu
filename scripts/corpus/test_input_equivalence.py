#!/usr/bin/env python3
"""Focused tests for the canonical #52 input-equivalence verdict.

Parser/unit-level only: no GPU, no model execution, no broad oracle workflow.
Covers the required #52 proof surface:

- fully equivalent synthetic case;
- timestep mismatch;
- output-window mismatch;
- vertical-reference mismatch;
- meteorology-identity mismatch;
- physics-switch/formulation mismatch;
- declared representation difference with insufficient evidence;
- malformed/missing prerequisite producing INTEGRITY_ERROR;
- ETEX mini through the same contract remains NOT_DEMONSTRATED;
- downstream paired scoring refuses every state except INPUT_EQUIVALENT.
"""

import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts" / "corpus"))

import input_equivalence as IE


def load_case(case_id):
    path = REPO / "fixtures" / "corpus" / "cases" / f"{case_id}.json"
    return json.loads(path.read_text(encoding="utf-8"))


def oracle_dir_for(case_id):
    if case_id == "ETEX-MINI-013":
        return REPO / "fixtures" / "etex" / "mini" / "config"
    return REPO / "fixtures" / "corpus" / "fortran" / case_id


def field_status(report, field_id):
    for field in report["fields"]:
        if field["field_id"] == field_id:
            return field["status"]
    raise AssertionError(f"field {field_id} missing from report")


class EquivalentSyntheticTest(unittest.TestCase):
    def test_adv_ana_001_is_input_equivalent(self):
        case = load_case("ADV-ANA-001")
        report = IE.build_report("ADV-ANA-001", case, oracle_dir_for("ADV-ANA-001"))
        self.assertEqual(report["verdict"], IE.VERDICT_EQUIVALENT)
        self.assertEqual(report["schema_version"], 1)
        self.assertEqual(report["schema_id"], IE.REPORT_SCHEMA_ID)
        # Every required field contributes explicit evidence.
        ids = [f["field_id"] for f in report["fields"]]
        for required in IE.REQUIRED_FIELD_IDS:
            self.assertIn(required, ids)
        for field in report["fields"]:
            self.assertEqual(field["status"], "equivalent", field)
            self.assertTrue(field["detail"])
        # Conversions are named and recorded; #53 is referenced, not implemented.
        self.assertTrue(report["conversions"])
        self.assertEqual(report["provenance_dependency"]["owner"], "#53")
        # Downstream gate allows this report.
        IE.require_input_equivalent(report)

    def test_wind_uni_002_is_input_equivalent(self):
        case = load_case("WIND-UNI-002")
        report = IE.build_report("WIND-UNI-002", case, oracle_dir_for("WIND-UNI-002"))
        self.assertEqual(report["verdict"], IE.VERDICT_EQUIVALENT)


class TimestepMismatchTest(unittest.TestCase):
    def test_lsynctime_mismatch_yields_mismatch(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        # Keep the case internally valid (multiples still hold) but contradict
        # the resolved COMMAND LSYNCTIME=300.
        case["oracle_command_overrides"]["lsynctime_s"] = 600
        case["output"]["sampling_interval_s"] = 600
        report = IE.build_report("WIND-UNI-002", case, oracle_dir_for("WIND-UNI-002"))
        self.assertEqual(report["verdict"], IE.VERDICT_MISMATCH)
        self.assertEqual(field_status(report, IE.FIELD_TIMESTEP), "mismatch")


class OutputWindowMismatchTest(unittest.TestCase):
    def test_interval_mismatch_yields_mismatch(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        # 3600 stays a multiple of LSYNCTIME=300 and keeps averaging<=interval.
        case["output"]["interval_s"] = 3600
        report = IE.build_report("WIND-UNI-002", case, oracle_dir_for("WIND-UNI-002"))
        self.assertEqual(report["verdict"], IE.VERDICT_MISMATCH)
        self.assertEqual(field_status(report, IE.FIELD_OUTPUT_WINDOWS), "mismatch")


class VerticalReferenceMismatchTest(unittest.TestCase):
    def test_release_height_mismatch_yields_mismatch(self):
        case = copy.deepcopy(load_case("ADV-ANA-001"))
        case["release"]["geometry"]["z_m"] = 200.0
        report = IE.build_report("ADV-ANA-001", case, oracle_dir_for("ADV-ANA-001"))
        self.assertEqual(report["verdict"], IE.VERDICT_MISMATCH)
        self.assertEqual(field_status(report, IE.FIELD_RELEASE_VERTICAL_REF), "mismatch")


class MeteorologyIdentityMismatchTest(unittest.TestCase):
    def test_wind_component_mismatch_yields_mismatch(self):
        case = copy.deepcopy(load_case("ADV-ANA-001"))
        case["wind"]["u_m_s"] = 20.0
        report = IE.build_report("ADV-ANA-001", case, oracle_dir_for("ADV-ANA-001"))
        self.assertEqual(report["verdict"], IE.VERDICT_MISMATCH)
        self.assertEqual(field_status(report, IE.FIELD_METEOROLOGY_IDENTITY), "mismatch")


class PhysicsSwitchMismatchTest(unittest.TestCase):
    def test_switch_mismatch_yields_mismatch(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        # Keep the case internally consistent (switches agree) but contradict
        # the resolved COMMAND LTURBULENCE=1.
        case["physics_switches"]["turbulence"] = False
        case["oracle_command_overrides"]["lturbulence"] = 0
        report = IE.build_report("WIND-UNI-002", case, oracle_dir_for("WIND-UNI-002"))
        self.assertEqual(report["verdict"], IE.VERDICT_MISMATCH)
        self.assertEqual(field_status(report, IE.FIELD_PHYSICS_SWITCHES), "mismatch")

    def test_formulation_mismatch_yields_mismatch(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        case["oracle_command_overrides"]["ctl"] = 10.0
        report = IE.build_report("WIND-UNI-002", case, oracle_dir_for("WIND-UNI-002"))
        self.assertEqual(report["verdict"], IE.VERDICT_MISMATCH)
        self.assertEqual(field_status(report, IE.FIELD_PHYSICS_FORMULATION), "mismatch")


class RepresentationDifferenceTest(unittest.TestCase):
    def test_declared_limitation_without_evidence_is_not_demonstrated(self):
        case = copy.deepcopy(load_case("ADV-ANA-001"))
        case["representation_differences"]["known_input_equivalence_limitations"] = [
            {
                "code": "INPUT_EQUIVALENCE_NOT_DEMONSTRATED",
                "description": "Synthetic test limitation without independent evidence.",
            }
        ]
        report = IE.build_report("ADV-ANA-001", case, oracle_dir_for("ADV-ANA-001"))
        self.assertEqual(report["verdict"], IE.VERDICT_NOT_DEMONSTRATED)
        self.assertEqual(
            field_status(report, IE.FIELD_REPRESENTATION_DIFFERENCES), "not_demonstrated"
        )


class IntegrityErrorTest(unittest.TestCase):
    def test_missing_oracle_dir_yields_integrity_error(self):
        case = load_case("ADV-ANA-001")
        with tempfile.TemporaryDirectory() as tmp:
            report = IE.build_report("ADV-ANA-001", case, Path(tmp) / "missing")
        self.assertEqual(report["verdict"], IE.VERDICT_INTEGRITY_ERROR)

    def test_missing_command_yields_integrity_error(self):
        case = load_case("ADV-ANA-001")
        with tempfile.TemporaryDirectory() as tmp:
            oracle = Path(tmp)
            (oracle / "RELEASES").write_text(
                (oracle_dir_for("ADV-ANA-001") / "RELEASES").read_text(encoding="utf-8"),
                encoding="utf-8",
            )
            (oracle / "OUTGRID").write_text(
                (oracle_dir_for("ADV-ANA-001") / "OUTGRID").read_text(encoding="utf-8"),
                encoding="utf-8",
            )
            report = IE.build_report("ADV-ANA-001", case, oracle)
        self.assertEqual(report["verdict"], IE.VERDICT_INTEGRITY_ERROR)

    def test_malformed_manifest_yields_integrity_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            bad = Path(tmp) / "BAD-001.json"
            bad.write_text(json.dumps({"case_id": "BAD-001"}), encoding="utf-8")
            report = IE.evaluate_case_file(bad, oracle_dir_for("ADV-ANA-001"))
        self.assertEqual(report["verdict"], IE.VERDICT_INTEGRITY_ERROR)

    def test_case_id_mismatch_yields_integrity_error(self):
        case = load_case("ADV-ANA-001")
        report = IE.build_report("WIND-UNI-002", case, oracle_dir_for("WIND-UNI-002"))
        # Geometry/timing/etc. come from ADV-ANA-001 but are evaluated as
        # WIND-UNI-002: resolved inputs contradict the declared case.
        # Either MISMATCH or INTEGRITY_ERROR is fail-closed here; the
        # manifest-identity path via evaluate_case_file must be INTEGRITY_ERROR.
        self.assertIn(report["verdict"], (IE.VERDICT_MISMATCH, IE.VERDICT_INTEGRITY_ERROR))
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "WIND-UNI-002.json"
            path.write_text(json.dumps(case), encoding="utf-8")
            ident = IE.evaluate_case_file(path, oracle_dir_for("WIND-UNI-002"))
        self.assertEqual(ident["verdict"], IE.VERDICT_INTEGRITY_ERROR)


class EtexMiniTest(unittest.TestCase):
    def test_etex_mini_stays_not_demonstrated(self):
        case_path = REPO / "fixtures" / "corpus" / "cases" / "ETEX-MINI-013.json"
        report = IE.evaluate_case_file(case_path, oracle_dir_for("ETEX-MINI-013"))
        self.assertEqual(report["verdict"], IE.VERDICT_NOT_DEMONSTRATED)
        # Same canonical field surface as synthetic cases.
        ids = [f["field_id"] for f in report["fields"]]
        for required in IE.REQUIRED_FIELD_IDS:
            self.assertIn(required, ids)
        # Known blockers are explicit, not silently normalized.
        self.assertEqual(
            field_status(report, IE.FIELD_REPRESENTATION_DIFFERENCES), "not_demonstrated"
        )
        self.assertEqual(
            field_status(report, IE.FIELD_VERTICAL_COORDINATE), "not_demonstrated"
        )
        # Downstream scoring must refuse this report.
        with self.assertRaises(IE.InputEquivalenceError):
            IE.require_input_equivalent(report)


class DownstreamGateTest(unittest.TestCase):
    def test_gate_refuses_every_non_equivalent_state(self):
        base = {
            "schema_version": 1,
            "schema_id": IE.REPORT_SCHEMA_ID,
            "case_id": "TEST-001",
            "case_file": "fixtures/corpus/cases/TEST-001.json",
            "oracle_dir": "fixtures/corpus/fortran/TEST-001",
            "fields": [],
            "conversions": [],
            "provenance_dependency": IE.PROVENANCE_DEPENDENCY,
        }
        for verdict in (
            IE.VERDICT_NOT_DEMONSTRATED,
            IE.VERDICT_MISMATCH,
            IE.VERDICT_INTEGRITY_ERROR,
        ):
            with self.subTest(verdict=verdict):
                report = dict(base, verdict=verdict)
                with self.assertRaises(IE.InputEquivalenceError) as ctx:
                    IE.require_input_equivalent(report)
                self.assertIn(verdict, str(ctx.exception))
        allowed = dict(base, verdict=IE.VERDICT_EQUIVALENT)
        IE.require_input_equivalent(allowed)

    def test_gate_rejects_malformed_report(self):
        with self.assertRaises(IE.InputEquivalenceError):
            IE.require_input_equivalent({"verdict": "PASS"})
        with self.assertRaises(IE.InputEquivalenceError):
            IE.require_input_equivalent({})


if __name__ == "__main__":
    unittest.main()
