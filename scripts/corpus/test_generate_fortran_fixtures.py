#!/usr/bin/env python3
"""Regression tests for canonical Oracle override normalization/generation.

No GPU or model execution: parser/unit-level only.
"""

import copy
import importlib.util
import json
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts" / "corpus"))


def load_generator():
    spec = importlib.util.spec_from_file_location(
        "generate_fortran_fixtures",
        REPO / "scripts" / "corpus" / "generate_fortran_fixtures.py",
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


GEN = load_generator()


def load_case(case_id: str) -> dict:
    path = REPO / "fixtures" / "corpus" / "cases" / f"{case_id}.json"
    return json.loads(path.read_text(encoding="utf-8"))


class OracleOverrideTest(unittest.TestCase):
    def test_adv_ana_001_generates_turbulence_off(self):
        case = load_case("ADV-ANA-001")
        text = GEN.command_text("ADV-ANA-001", case)
        self.assertEqual(int(GEN.namelist_value(text, "LTURBULENCE")), 0)
        self.assertEqual(int(GEN.namelist_value(text, "LCONVECTION")), 0)

    def test_wind_uni_002_generates_declared_values(self):
        case = load_case("WIND-UNI-002")
        text = GEN.command_text("WIND-UNI-002", case)
        self.assertEqual(int(GEN.namelist_value(text, "LTURBULENCE")), 1)
        self.assertEqual(int(GEN.namelist_value(text, "LCONVECTION")), 0)
        self.assertAlmostEqual(
            float(GEN.namelist_value(text, "CTL")), 5.0, places=6
        )
        self.assertEqual(int(GEN.namelist_value(text, "IFINE")), 4)

    def test_migrated_shear_case_generates_declared_values(self):
        case = load_case("WIND-SHEAR-003")
        self.assertEqual(case.get("schema_version"), 2)
        text = GEN.command_text("WIND-SHEAR-003", case)
        self.assertEqual(int(GEN.namelist_value(text, "LTURBULENCE")), 1)
        self.assertEqual(int(GEN.namelist_value(text, "LCONVECTION")), 0)
        self.assertEqual(int(GEN.namelist_value(text, "IFINE")), 4)

    def test_legacy_uppercase_document_is_rejected(self):
        legacy = {
            "integration": {"start": "20240101000000", "total_s": 3600},
            "oracle_command_overrides": {
                "LTURBULENCE": 1,
                "LCONVECTION": 0,
                "CTL": 5.0,
                "IFINE": 4,
            },
            "physics_switches": {"turbulence": True, "convection": False},
        }
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("LEGACY-001", legacy)
        self.assertIn("legacy", str(ctx.exception))

    def test_missing_required_override_fails_without_default(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        del case["oracle_command_overrides"]["lturbulence"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("lturbulence", str(ctx.exception))

    def test_both_forms_rejected_as_ambiguous(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        case["oracle_command_overrides"]["LTURBULENCE"] = 1
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("ambiguous", str(ctx.exception))
        self.assertIn("LTURBULENCE", str(ctx.exception))
        self.assertIn("lturbulence", str(ctx.exception))

    def test_flag_other_than_zero_or_one_rejected(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        case["oracle_command_overrides"]["lturbulence"] = 2
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("0 or 1", str(ctx.exception))

    def test_conflicting_physics_switch_rejected(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        # physics says turbulence on, oracle says off.
        case["oracle_command_overrides"]["lturbulence"] = 0
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("conflicts", str(ctx.exception))

    def test_optional_flag_conflicting_physics_switch_rejected(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        # physics says no dry deposition, oracle declares dry deposition on.
        case["oracle_command_overrides"]["ldrydep"] = 1
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("dry_deposition", str(ctx.exception))
        self.assertIn("conflicts", str(ctx.exception))

    def test_ctl_five_point_zero_is_accepted(self):
        case = load_case("WIND-UNI-002")
        self.assertEqual(
            case["oracle_command_overrides"]["turbulence_formulation"],
            "adaptive_w_sigw",
        )
        text = GEN.command_text("WIND-UNI-002", case)
        self.assertAlmostEqual(float(GEN.namelist_value(text, "CTL")), 5.0, places=6)

    def test_zero_ctl_rejected_for_nonzero_timestep_division(self):
        case = load_case("ADV-ANA-001")
        case = copy.deepcopy(case)
        case["oracle_command_overrides"]["ctl"] = 0
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("ADV-ANA-001", case)
        self.assertIn("non-zero", str(ctx.exception))
        self.assertIn("readoptions_mod.f90:653", str(ctx.exception))

    def test_negative_ctl_rejected_as_deliberately_unsupported_fixed_mode(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        # CTL=-5.0 is the pinned oracle's own default; in FLEXPART it selects
        # the fixed-timestep mode (method=0, mintime=lsynctime,
        # readoptions_mod.f90:786-795), which the contract deliberately freezes
        # out in favour of the adaptive w/sigw mode.
        case["oracle_command_overrides"]["ctl"] = -5.0
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        rendered = str(ctx.exception)
        self.assertIn("deliberately", rendered)
        self.assertIn("unsupported", rendered)
        self.assertIn("fixed", rendered)

    def test_small_positive_ctl_rejected_for_turbulence_formulation(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        # readoptions_mod.f90:645-650: ctl < 0.1 silently rewrites the Markov
        # chain to the w formulation and forces ifine=1, so the generator
        # refuses it as inconsistent with the declared formulation.
        case["oracle_command_overrides"]["ctl"] = 0.05
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("readoptions_mod.f90", str(ctx.exception))

    def test_small_positive_ctl_rejected_even_with_turbulence_disabled(self):
        case = load_case("ADV-ANA-001")
        case = copy.deepcopy(case)
        # The declared `adaptive_w_sigw` formulation pins the w/sigw semantics,
        # so a sub-threshold CTL is inconsistent regardless of the LTURBULENCE
        # flag (the oracle still interprets CTL < 0.1 as the w formulation).
        case["oracle_command_overrides"]["ctl"] = 0.05
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("ADV-ANA-001", case)
        self.assertIn("turbulence_formulation", str(ctx.exception))

    def test_unknown_turbulence_formulation_rejected(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        case["oracle_command_overrides"]["turbulence_formulation"] = "fixed_sync"
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("adaptive_w_sigw", str(ctx.exception))

    def test_missing_turbulence_formulation_rejected(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        del case["oracle_command_overrides"]["turbulence_formulation"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("turbulence_formulation", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
