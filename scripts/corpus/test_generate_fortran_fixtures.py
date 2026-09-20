#!/usr/bin/env python3
"""Regression tests for canonical Oracle override normalization/generation.

No GPU or model execution: parser/unit-level only.
Covers the fail-closed raw-Python path: wind/surface/physics_switches/
integration blocks must not decay to defaults on malformed input.
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


class MeteoPhysicsFailClosedTest(unittest.TestCase):
    """Fail-closed meteo generation: malformed input never decays to defaults.

    Mirrors the canonical Rust contract (ValidationCaseManifest): `wind` is a
    required tagged union, `physics_switches` is a mandatory 5-boolean object,
    and `surface` is null-or-object where null is only permitted when the
    declared physics do not require surface data.
    """

    def test_uniform_wind_uses_declared_components(self):
        case = load_case("WIND-UNI-002")
        args = GEN.meteo_args("WIND-UNI-002", case)
        self.assertIn("--u-wind 5.0 --v-wind -3.0 --w-wind 0.0", args)

    def test_linear_shear_wind_uses_declared_components(self):
        case = load_case("WIND-SHEAR-003")
        args = GEN.meteo_args("WIND-SHEAR-003", case)
        self.assertIn("--u-wind 2.0", args)
        self.assertIn("--u-shear-per-m 0.004", args)

    def test_real_weather_returns_empty_string(self):
        case = load_case("ETEX-MINI-013")
        self.assertEqual(GEN.meteo_args("ETEX-MINI-013", case), "")

    def test_wind_missing_raises(self):
        case = load_case("ADV-ANA-001")
        case = copy.deepcopy(case)
        del case["wind"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.meteo_args("ADV-ANA-001", case)
        self.assertIn("wind", str(ctx.exception))

    def test_wind_non_object_raises(self):
        case = load_case("ADV-ANA-001")
        case = copy.deepcopy(case)
        case["wind"] = False
        with self.assertRaises(SystemExit) as ctx:
            GEN.meteo_args("ADV-ANA-001", case)
        self.assertIn("wind", str(ctx.exception))

    def test_wind_unknown_profile_raises(self):
        case = load_case("ADV-ANA-001")
        case = copy.deepcopy(case)
        case["wind"]["profile"] = "hurricane"
        with self.assertRaises(SystemExit) as ctx:
            GEN.meteo_args("ADV-ANA-001", case)
        self.assertIn("profile", str(ctx.exception))

    def test_wind_missing_component_never_defaulted(self):
        case = load_case("ADV-ANA-001")
        case = copy.deepcopy(case)
        del case["wind"]["u_m_s"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.meteo_args("ADV-ANA-001", case)
        self.assertIn("u_m_s", str(ctx.exception))

    def test_surface_null_permitted_without_turbulence_or_deposition(self):
        case = load_case("ADV-ANA-001")
        self.assertIsNone(case.get("surface"))
        args = GEN.meteo_args("ADV-ANA-001", case)
        self.assertIn("--sshf 40.0 --blh 1500.0 --lsp 0.0 --cp 0.0", args)

    def test_surface_null_rejected_when_physics_requires_surface(self):
        case = load_case("PBL-NEUTRAL-005")
        case = copy.deepcopy(case)
        case["surface"] = None
        with self.assertRaises(SystemExit) as ctx:
            GEN.meteo_args("PBL-NEUTRAL-005", case)
        rendered = str(ctx.exception)
        self.assertIn("surface", rendered)
        self.assertIn("turbulence", rendered)

    def test_surface_non_null_falsey_rejected_not_normalized(self):
        for bad in (False, 0, 0.0, "", []):
            with self.subTest(value=bad):
                case = load_case("ADV-ANA-001")
                case = copy.deepcopy(case)
                case["surface"] = bad
                with self.assertRaises(SystemExit) as ctx:
                    GEN.meteo_args("ADV-ANA-001", case)
                self.assertIn("surface", str(ctx.exception))

    def test_surface_missing_consumed_field_rejected(self):
        case = load_case("PBL-NEUTRAL-005")
        case = copy.deepcopy(case)
        del case["surface"]["mixing_height_m"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.meteo_args("PBL-NEUTRAL-005", case)
        self.assertIn("mixing_height_m", str(ctx.exception))

    def test_surface_non_numeric_field_rejected(self):
        case = load_case("PBL-NEUTRAL-005")
        case = copy.deepcopy(case)
        case["surface"]["sensible_heat_flux_w_m2"] = "warm"
        with self.assertRaises(SystemExit) as ctx:
            GEN.meteo_args("PBL-NEUTRAL-005", case)
        self.assertIn("sensible_heat_flux_w_m2", str(ctx.exception))

    def test_wet_008_uses_declared_surface_precipitation(self):
        case = load_case("WET-008")
        args = GEN.meteo_args("WET-008", case)
        self.assertIn("--lsp 2.0 --cp 1.0", args)

    def test_physics_switches_missing_raises(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        del case["physics_switches"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("physics_switches", str(ctx.exception))

    def test_physics_switches_non_object_raises(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        case["physics_switches"] = False
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("physics_switches", str(ctx.exception))

    def test_physics_switch_missing_key_raises(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        del case["physics_switches"]["dry_deposition"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("dry_deposition", str(ctx.exception))

    def test_physics_switch_non_boolean_raises(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        case["physics_switches"]["turbulence"] = 1
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("turbulence", str(ctx.exception))

    def test_physics_switches_unknown_key_raises(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        case["physics_switches"]["plume_rise"] = True
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("plume_rise", str(ctx.exception))

    def test_malformed_physics_never_skips_agreement_check(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        case["physics_switches"]["turbulence"] = "yes"
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("turbulence", str(ctx.exception))

    def test_integration_missing_start_raises(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        del case["integration"]["start"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("start", str(ctx.exception))

    def test_integration_missing_total_s_raises(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        del case["integration"]["total_s"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.ageclass_text("WIND-UNI-002", case)
        self.assertIn("total_s", str(ctx.exception))

    def test_integration_fractional_total_s_raises(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        case["integration"]["total_s"] = 3600.5
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("second", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
