#!/usr/bin/env python3
"""Regression tests for canonical Oracle override normalization/generation.

No GPU or model execution: parser/unit-level only.
Covers the fail-closed raw-Python path: wind/surface/physics_switches/
integration blocks must not decay to defaults on malformed input.
"""

import copy
import importlib.util
import json
import re
import shutil
import sys
import tempfile
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

    def test_unknown_candidate_philox_derivation_is_rejected(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        case["stochastic"]["candidate_philox"]["derivation"] = "typo_or_future_mode"
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("unsupported", str(ctx.exception))
        self.assertIn("derivation", str(ctx.exception))

    def test_legacy_identical_repeats_flag_is_rejected(self):
        case = load_case("REPEAT-009")
        case = copy.deepcopy(case)
        case["stochastic"]["candidate_philox"]["identical_repeats"] = True
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("REPEAT-009", case)
        self.assertIn("identical_repeats", str(ctx.exception))
    def test_oracle_seed_cannot_substitute_for_candidate_philox(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        physics = GEN.mandatory_physics_switches("WIND-UNI-002", case)
        case["stochastic"]["candidate_philox"] = None
        self.assertIsNotNone(case["stochastic"]["oracle_seed"])
        with self.assertRaises(SystemExit) as ctx:
            GEN._validate_stochastic_contract("WIND-UNI-002", case, physics)
        rendered = str(ctx.exception)
        self.assertIn("candidate_philox", rendered)
        self.assertIn("separate RNG namespace", rendered)

    def test_stochastic_namespace_fields_are_required_explicitly(self):
        for field in ("candidate_philox", "oracle_seed"):
            case = copy.deepcopy(load_case("WIND-UNI-002"))
            physics = GEN.mandatory_physics_switches("WIND-UNI-002", case)
            del case["stochastic"][field]
            with self.subTest(field=field):
                with self.assertRaises(SystemExit) as ctx:
                    GEN._validate_stochastic_contract("WIND-UNI-002", case, physics)
                self.assertIn(f"stochastic.{field}", str(ctx.exception))
                self.assertIn("required explicitly", str(ctx.exception))

    def test_oracle_repetitions_have_no_default(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        physics = GEN.mandatory_physics_switches("WIND-UNI-002", case)
        del case["stochastic"]["oracle_seed"]["repetitions"]
        with self.assertRaises(SystemExit) as ctx:
            GEN._validate_stochastic_contract("WIND-UNI-002", case, physics)
        self.assertIn("repetitions", str(ctx.exception))
        self.assertIn("required explicitly", str(ctx.exception))

    def test_candidate_philox_key_and_counter_are_strict_u32_arrays(self):
        mutations = (
            ("base_key", [1], "exactly 2"),
            ("base_key", [0, 2**32], "unsigned 32-bit"),
            ("base_key", [0, True], "unsigned 32-bit"),
            ("base_counter", [0, 0, 0], "exactly 4"),
            ("base_counter", [0, 0, 0, -1], "unsigned 32-bit"),
        )
        for field, value, expected in mutations:
            case = copy.deepcopy(load_case("WIND-UNI-002"))
            physics = GEN.mandatory_physics_switches("WIND-UNI-002", case)
            case["stochastic"]["candidate_philox"][field] = value
            with self.subTest(field=field, value=value):
                with self.assertRaises(SystemExit) as ctx:
                    GEN._validate_stochastic_contract("WIND-UNI-002", case, physics)
                rendered = str(ctx.exception)
                self.assertIn(field, rendered)
                self.assertIn(expected, rendered)

    def test_species_physics_contract_hash_mismatch_is_rejected(self):
        case = copy.deepcopy(load_case("DRY-007"))
        physics = GEN.mandatory_physics_switches("DRY-007", case)
        case["release"]["species"]["physics_contract"]["git_blob_sha"] = "deadbeef"
        with self.assertRaises(SystemExit) as ctx:
            GEN._validate_species_physics_contract("DRY-007", case, physics)
        self.assertIn("git_blob_sha", str(ctx.exception))
        self.assertIn("canonical", str(ctx.exception))

    def test_active_deposition_without_forcing_block_is_rejected(self):
        case = copy.deepcopy(load_case("DRY-007"))
        physics = GEN.mandatory_physics_switches("DRY-007", case)
        del case["deposition"]
        with self.assertRaises(SystemExit) as ctx:
            GEN._validate_deposition_contract("DRY-007", case, physics)
        self.assertIn("deposition block is required", str(ctx.exception))

    def test_etex_manifest_declares_inert_no_removal_species(self):
        case = load_case("ETEX-MINI-013")
        physics = GEN.mandatory_physics_switches("ETEX-MINI-013", case)
        profile = GEN._validate_species_physics_contract("ETEX-MINI-013", case, physics)
        GEN._validate_deposition_contract("ETEX-MINI-013", case, physics)
        self.assertEqual(profile, "species_024_inert_v1")
        self.assertFalse(physics["dry_deposition"])
        self.assertFalse(physics["wet_deposition"])
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

    def test_nonexistent_deposition_decay_command_pseudo_flags_rejected(self):
        for field in ("ldrydep", "lwetdep", "ldecay"):
            with self.subTest(field=field):
                case = copy.deepcopy(load_case("WIND-UNI-002"))
                case["oracle_command_overrides"][field] = 1
                with self.assertRaises(SystemExit) as ctx:
                    GEN.command_text("WIND-UNI-002", case)
                self.assertIn("unknown oracle override", str(ctx.exception))
                self.assertIn(field, str(ctx.exception))

    def test_ctl_five_point_zero_is_accepted(self):
        case = load_case("WIND-UNI-002")
        self.assertEqual(
            case["oracle_command_overrides"]["turbulence_formulation"],
            "adaptive_w_sigw",
        )
        text = GEN.command_text("WIND-UNI-002", case)
        self.assertAlmostEqual(float(GEN.namelist_value(text, "CTL")), 5.0, places=6)

    def test_etex_manifest_matches_real_mini_pipeline_contract(self):
        case = load_case("ETEX-MINI-013")

        # Candidate meteorology/run contract from prepare_native_era5.py / etex-run.
        self.assertEqual(
            (case["domain"]["nx"], case["domain"]["ny"], case["domain"]["nz"]),
            (65, 41, 16),
        )
        self.assertEqual(
            (case["domain"]["xlon0_deg"], case["domain"]["ylat0_deg"]),
            (-8, 43),
        )
        self.assertEqual(
            case["domain"]["wind_heights_m"],
            [10, 50, 100, 200, 400, 600, 900, 1300, 1800, 2500, 3500,
             5000, 7000, 10000, 14000, 20000],
        )
        self.assertIsNone(case["surface"])
        self.assertEqual(case["integration"], {
            "start": "19941023160000",
            "dt_s": 900,
            "steps": 48,
            "total_s": 43200,
        })
        self.assertEqual(case["release"]["particle_count"], 10000)
        self.assertAlmostEqual(case["release"]["mass_kg_per_particle"], 0.034)
        self.assertEqual(case["release"]["timing"], {
            "kind": "window",
            "start": "19941023160000",
            "end": "19941024034000",
        })

        # etex-run currently inherits this exact ForwardTimeLoopConfig default.
        self.assertEqual(
            case["stochastic"]["candidate_philox"]["base_key"],
            [0xDECAFBAD, 0x12345678],
        )
        self.assertEqual(case["stochastic"]["candidate_philox"]["base_counter"], [0, 0, 0, 0])
        self.assertEqual(case["stochastic"]["candidate_philox"]["count"], 1)
        timeloop = (REPO / "src" / "simulation" / "timeloop.rs").read_text(encoding="utf-8")
        self.assertIn("philox_key: [0xDECA_FBAD, 0x1234_5678]", timeloop)

        oracle_seed = case["stochastic"]["oracle_seed"]
        self.assertEqual(oracle_seed["kind"], "pristine-oracle")
        self.assertIsNone(oracle_seed["strategy"])
        self.assertEqual(oracle_seed["mode"], "default")
        self.assertIsNone(oracle_seed["seed"])
        self.assertEqual(oracle_seed["repetitions"], 1)

        # The source preparation code is the existing ETEX GPU run authority.
        prepare = (REPO / "scripts" / "etex" / "prepare_native_era5.py").read_text(
            encoding="utf-8"
        )
        for fragment in (
            '"nx": 65, "ny": 41, "nz": 16',
            '"xlon0_deg": -8.0, "ylat0_deg": 43.0',
            '"particle_count": 10000',
            '"dt_seconds": 900',
            '"output": {"nx": 64, "ny": 40, "nz": 5',
        ):
            self.assertIn(fragment, prepare)

        heights_match = re.search(
            r"HEIGHTS_M\s*=\s*np\.asarray\(\s*\[([^\]]+)\]",
            prepare,
            re.DOTALL,
        )
        self.assertIsNotNone(heights_match)
        source_heights = [
            float(value)
            for value in re.findall(r"-?\d+(?:\.\d+)?", heights_match.group(1))
        ]
        self.assertEqual(source_heights, case["domain"]["wind_heights_m"])

        # RELEASES: every scientific release value mirrors the existing config.
        generated_releases = GEN.releases_text("ETEX-MINI-013", case, 24)
        actual_releases = (
            REPO / "fixtures" / "etex" / "mini" / "config" / "RELEASES"
        ).read_text(encoding="utf-8")
        for key in (
            "SPECNUM_REL", "IDATE1", "ITIME1", "IDATE2", "ITIME2",
            "LON1", "LON2", "LAT1", "LAT2", "Z1", "Z2", "ZKIND", "MASS", "PARTS",
        ):
            generated = float(GEN.namelist_value(generated_releases, key).replace("D", "E"))
            actual = float(GEN.namelist_value(actual_releases, key).replace("D", "E"))
            self.assertAlmostEqual(generated, actual, places=6, msg=f"ETEX RELEASES {key}")

        # OUTGRID is distinct from the 65x41 meteorology domain.
        self.assertEqual(case["output_grid"], {
            "nx": 64,
            "ny": 40,
            "nz": 5,
            "dx_deg": 0.25,
            "dy_deg": 0.25,
            "xlon0_deg": -8,
            "ylat0_deg": 43,
            "horizontal_ref": "geographic_lon_lat_degrees",
            "heights_m": [100, 500, 1000, 2000, 5000],
            "heights_ref": "agl",
        })
        generated_outgrid = GEN.outgrid_text(case)
        actual_outgrid = (
            REPO / "fixtures" / "etex" / "mini" / "config" / "OUTGRID"
        ).read_text(encoding="utf-8")
        for key in ("OUTLON0", "OUTLAT0", "NUMXGRID", "NUMYGRID", "DXOUT", "DYOUT"):
            self.assertAlmostEqual(
                float(GEN.namelist_value(generated_outgrid, key)),
                float(GEN.namelist_value(actual_outgrid, key)),
                places=6,
                msg=f"ETEX OUTGRID {key}",
            )
        def outheights(text):
            match = re.search(r"\bOUTHEIGHTS\s*=\s*([^/]+)", text, re.DOTALL)
            self.assertIsNotNone(match)
            return [float(v) for v in re.findall(r"-?\d+(?:\.\d+)?", match.group(1))]
        self.assertEqual(outheights(generated_outgrid), outheights(actual_outgrid))
        self.assertEqual(outheights(actual_outgrid), case["output_grid"]["heights_m"])

        # COMMAND: preserve the actual pristine ETEX fixed-sync semantics.
        overrides = case["oracle_command_overrides"]
        self.assertEqual(overrides["turbulence_formulation"], "fixed_sync_w")
        self.assertEqual(overrides["ctl"], -5)
        self.assertEqual(overrides["ifine"], 4)
        self.assertEqual(overrides["lturbulence"], 1)
        self.assertEqual(overrides["lsynctime_s"], 900)
        generated_command = GEN.command_text("ETEX-MINI-013", case)
        actual_command = (
            REPO / "fixtures" / "etex" / "mini" / "config" / "COMMAND"
        ).read_text(encoding="utf-8")
        for key in (
            "LTURBULENCE", "CTL", "IFINE", "LSYNCTIME",
            "LOUTSTEP", "LOUTAVER", "LOUTSAMPLE",
        ):
            self.assertAlmostEqual(
                float(GEN.namelist_value(generated_command, key)),
                float(GEN.namelist_value(actual_command, key)),
                places=6,
                msg=f"ETEX COMMAND {key}",
            )

    def test_real_weather_requires_explicit_output_grid(self):
        case = copy.deepcopy(load_case("ETEX-MINI-013"))
        del case["output_grid"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.validate_and_normalize_case_for_generation(
                "ETEX-MINI-013",
                case,
                case_file="fixtures/corpus/cases/ETEX-MINI-013.json",
                tracer=Path(__file__),
                aerosol=None,
            )
        self.assertIn("output_grid", str(ctx.exception))

    def test_missing_lsynctime_rejected_without_default(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        del case["oracle_command_overrides"]["lsynctime_s"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("lsynctime_s", str(ctx.exception))

    def test_zero_ctl_rejected_for_nonzero_timestep_division(self):
        case = load_case("ADV-ANA-001")
        case = copy.deepcopy(case)
        case["oracle_command_overrides"]["ctl"] = 0
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("ADV-ANA-001", case)
        self.assertIn("non-zero", str(ctx.exception))
        self.assertIn("readoptions_mod.f90:653", str(ctx.exception))

    def test_negative_ctl_requires_fixed_sync_w_formulation(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        case["oracle_command_overrides"]["ctl"] = -5.0
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("adaptive_w_sigw", str(ctx.exception))

        case["oracle_command_overrides"]["turbulence_formulation"] = "fixed_sync_w"
        text = GEN.command_text("WIND-UNI-002", case)
        self.assertAlmostEqual(float(GEN.namelist_value(text, "CTL")), -5.0, places=6)
        self.assertEqual(int(GEN.namelist_value(text, "LSYNCTIME")), 300)

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
        case["oracle_command_overrides"]["turbulence_formulation"] = "fixed_sync_unknown"
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        rendered = str(ctx.exception)
        self.assertIn("adaptive_w_sigw", rendered)
        self.assertIn("fixed_sync_w", rendered)

    def test_missing_turbulence_formulation_rejected(self):
        case = load_case("WIND-UNI-002")
        case = copy.deepcopy(case)
        del case["oracle_command_overrides"]["turbulence_formulation"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("turbulence_formulation", str(ctx.exception))


class ChronologyFailClosedTest(unittest.TestCase):
    def test_invalid_calendar_timestamp_rejected_and_leap_day_accepted(self):
        self.assertEqual(
            GEN._timestamp14("20240229010203", "TEST", "integration.start"),
            "20240229010203",
        )
        for bad in (
            "20230229010203",
            "20241301000000",
            "20240431000000",
            "20240101240000",
            "20240101006000",
            "00000101000000",
        ):
            with self.subTest(value=bad):
                with self.assertRaises(SystemExit):
                    GEN._timestamp14(bad, "TEST", "integration.start")

    def test_integration_dt_steps_total_must_agree(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        case["integration"]["steps"] -= 1
        with self.assertRaises(SystemExit) as ctx:
            GEN._required_integration("WIND-UNI-002", case)
        self.assertIn("dt_s * steps", str(ctx.exception))

    def test_release_must_stay_inside_simulation_window(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        case["release"]["timing"] = {
            "kind": "window",
            "start": "20231231235959",
            "end": "20240101000000",
        }
        with self.assertRaises(SystemExit) as ctx:
            GEN.release_window_datetimes("WIND-UNI-002", case)
        self.assertIn("outside simulation window", str(ctx.exception))

        case = copy.deepcopy(load_case("WIND-UNI-002"))
        case["release"]["timing"] = {
            "kind": "window",
            "start": "20240101003000",
            "end": "20240101010001",
        }
        with self.assertRaises(SystemExit) as ctx:
            GEN.release_window_datetimes("WIND-UNI-002", case)
        self.assertIn("outside simulation window", str(ctx.exception))

    def test_real_weather_coverage_must_cover_full_simulation(self):
        case = copy.deepcopy(load_case("ETEX-MINI-013"))
        integration = GEN._required_integration("ETEX-MINI-013", case)
        case["wind"]["meteorology"]["temporal_coverage"][0] = "19941023160001"
        with self.assertRaises(SystemExit) as ctx:
            GEN._required_meteorology("ETEX-MINI-013", case["wind"], integration)
        self.assertIn("cover the full simulation", str(ctx.exception))

        case = copy.deepcopy(load_case("ETEX-MINI-013"))
        integration = GEN._required_integration("ETEX-MINI-013", case)
        case["wind"]["meteorology"]["temporal_coverage"][1] = "19941024035959"
        with self.assertRaises(SystemExit) as ctx:
            GEN._required_meteorology("ETEX-MINI-013", case["wind"], integration)
        self.assertIn("cover the full simulation", str(ctx.exception))

    def test_real_weather_manifest_digest_path_must_be_normalized(self):
        for digest in (
            "manifest:",
            "manifest:/absolute/DIGESTS.json",
            "manifest:../DIGESTS.json",
            "manifest:fixtures//DIGESTS.json",
            "manifest:fixtures\\DIGESTS.json",
        ):
            case = copy.deepcopy(load_case("ETEX-MINI-013"))
            integration = GEN._required_integration("ETEX-MINI-013", case)
            case["wind"]["meteorology"]["digest"] = digest
            with self.subTest(digest=digest):
                with self.assertRaises(SystemExit) as ctx:
                    GEN._required_meteorology(
                        "ETEX-MINI-013", case["wind"], integration
                    )
                self.assertIn("digest", str(ctx.exception))

    def test_command_dates_are_derived_from_integration_start(self):
        case = copy.deepcopy(load_case("WIND-UNI-002"))
        case["integration"]["start"] = "20240229010203"
        text = GEN.command_text("WIND-UNI-002", case)
        self.assertEqual(int(GEN.namelist_value(text, "IBDATE")), 20240229)
        self.assertEqual(int(GEN.namelist_value(text, "IBTIME")), 10203)
        self.assertEqual(int(GEN.namelist_value(text, "IEDATE")), 20240229)
        self.assertEqual(int(GEN.namelist_value(text, "IETIME")), 20203)

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


REAL_CASES = GEN.CASES
REAL_FORTRAN_OUT = GEN.FORTRAN_OUT


def snapshot(root: Path) -> dict:
    """Map relative file path -> bytes beneath root (sorted, stable)."""
    return {
        p.relative_to(root).as_posix(): p.read_bytes()
        for p in sorted(root.rglob("*"), key=lambda p: p.as_posix())
        if p.is_file()
    }


def make_upstream_species(root: Path):
    """Minimal upstream examples tree: tracer with PDRYVEL, aerosol with PNDIA.

    The DRY-007 derivation replaces exactly one PDRYVEL assignment and the
    WET-008 derivation needs exactly one PNDIA line to strip.
    """
    tracer = root / "examples" / "Tracer" / "SPECIES" / "SPECIES_024"
    aerosol = root / "examples" / "Aerosol" / "SPECIES" / "SPECIES_040"
    tracer.parent.mkdir(parents=True, exist_ok=True)
    aerosol.parent.mkdir(parents=True, exist_ok=True)
    tracer.write_text(
        "&SPECIES_PARAMS\n"
        " PSPECIES='SO2',\n"
        " PDRYVEL=1.0,\n"
        "/\n",
        encoding="utf-8",
    )
    aerosol.write_text(
        "&SPECIES_PARAMS\n"
        " PSPECIES='SO2',\n"
        " PNDIA=11,\n"
        "/\n",
        encoding="utf-8",
    )
    return tracer, aerosol


class PreflightFailClosedTest(unittest.TestCase):
    """The generator must preflight before it writes; a malformed schema-v2
    input must never leave a partially updated fixture tree."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.cases_dir = self.tmp / "cases"
        self.cases_dir.mkdir()
        self.out_root = self.tmp / "fortran"
        self.out_root.mkdir()
        self.flexpart_root = self.tmp / "flexpart"
        self.flexpart_root.mkdir()
        self.tracer, self.aerosol = make_upstream_species(self.flexpart_root)
        for json_path in sorted(REAL_CASES.glob("*.json")):
            if json_path.name == "RESTART-010.json":
                # Generated from the PBL-NEUTRAL-005 shape; kept absent here
                # to exercise the same _load_case fallback main() uses.
                continue
            shutil.copyfile(json_path, self.cases_dir / json_path.name)
        self._saved = (GEN.CASES, GEN.FORTRAN_OUT)

    def tearDown(self):
        GEN.CASES, GEN.FORTRAN_OUT = self._saved
        self._tmp.cleanup()

    def _run_main(self):
        argv = ["generate_fortran_fixtures.py", "--flexpart-dir", str(self.flexpart_root)]
        old_argv = sys.argv[:]
        sys.argv = argv
        try:
            return GEN.main()
        finally:
            sys.argv = old_argv

    def _assert_aborts_before_writes(self, mutate_case, fragment):
        GEN.CASES = self.cases_dir
        GEN.FORTRAN_OUT = self.out_root
        before = snapshot(self.out_root)
        # Stale fixture files the generation would overwrite on success: both
        # an earlier-ordered valid case and the malformed case itself.
        stale = {
            "ADV-ANA-001/RELEASES": b"STALE-VALID",
            "WIND-UNI-002/COMMAND": b"STALE-MALFORMED",
        }
        for path, content in stale.items():
            write = self.out_root / path
            write.parent.mkdir(parents=True, exist_ok=True)
            write.write_bytes(content)
            before[path] = content
        case_path = self.cases_dir / "WIND-UNI-002.json"
        case = json.loads(case_path.read_text(encoding="utf-8"))
        mutate_case(case)
        case_path.write_text(json.dumps(case), encoding="utf-8")
        with self.assertRaises(SystemExit) as ctx:
            self._run_main()
        self.assertIn(fragment, str(ctx.exception))
        self.assertEqual(snapshot(self.out_root), before)

    def test_malformed_surface_aborts_before_any_write(self):
        def mutate(case):
            case["surface"] = "not-an-object"

        self._assert_aborts_before_writes(mutate, "surface")

    def test_malformed_wind_aborts_before_any_write(self):
        def mutate(case):
            case["wind"] = {"profile": "uniform"}

        self._assert_aborts_before_writes(mutate, "wind.u_m_s")

    def test_malformed_release_aborts_before_any_write(self):
        def mutate(case):
            case["release"]["particle_count"] = "many"

        self._assert_aborts_before_writes(mutate, "particle_count")

    def test_release_zero_mass_aborts_before_any_write(self):
        def mutate(case):
            case["release"]["inventory"]["quantity_kg"] = 0
        self._assert_aborts_before_writes(mutate, "quantity_kg")

    def test_release_per_particle_mass_mismatch_aborts_before_any_write(self):
        def mutate(case):
            case["release"]["mass_kg_per_particle"] = 0.123
        self._assert_aborts_before_writes(mutate, "mass_kg_per_particle")

    def test_release_bad_timestamp_aborts_before_any_write(self):
        def mutate(case):
            case["release"]["timing"]["at"] = "20240101"
        self._assert_aborts_before_writes(mutate, "YYYYMMDDHHMMSS")

    def test_release_negative_agl_aborts_before_any_write(self):
        def mutate(case):
            case["release"]["geometry"]["z_m"] = -1
        self._assert_aborts_before_writes(mutate, ">= 0")

    def test_release_outside_domain_aborts_before_any_write(self):
        def mutate(case):
            case["release"]["geometry"]["lon_deg"] = 20.0
        self._assert_aborts_before_writes(mutate, "outside domain")

    def test_malformed_physics_switches_aborts_before_any_write(self):
        def mutate(case):
            case["physics_switches"] = {"turbulence": True}

        self._assert_aborts_before_writes(mutate, "physics_switches")

    def test_missing_source_containment_policy_aborts_before_any_write(self):
        def mutate(case):
            case.pop("require_source_containment", None)
        self._assert_aborts_before_writes(mutate, "require_source_containment")

    def test_execution_profile_drift_aborts_before_any_write(self):
        def mutate(case):
            case["execution_profile"]["manifest_path"] = "reference/other.json"
        self._assert_aborts_before_writes(mutate, "frozen #49 profile")

    def test_malformed_oracle_override_aborts_before_any_write(self):
        def mutate(case):
            case["oracle_command_overrides"]["ctl"] = 0

        self._assert_aborts_before_writes(mutate, "ctl")

    def test_oracle_state_fields_are_required_explicitly(self):
        for field in ("strategy", "mode", "seed"):
            with self.subTest(field=field):
                def mutate(case, field=field):
                    case["stochastic"]["oracle_seed"].pop(field, None)
                self._assert_aborts_before_writes(
                    mutate, f"stochastic.oracle_seed.{field}"
                )

    def test_pristine_oracle_with_seed_aborts_before_any_write(self):
        def mutate(case):
            case["stochastic"]["oracle_seed"] = {
                "kind": "pristine-oracle",
                "strategy": None,
                "mode": "default",
                "seed": 1,
                "repetitions": 2,
            }
        self._assert_aborts_before_writes(mutate, "seed=null")

    def test_oracle_mode_seed_consistency_aborts_before_any_write(self):
        def missing_requested_seed(case):
            oracle = case["stochastic"]["oracle_seed"]
            oracle["mode"] = "requested_identity"
            oracle["seed"] = None
        self._assert_aborts_before_writes(missing_requested_seed, "requested_identity")

        def seeded_default(case):
            oracle = case["stochastic"]["oracle_seed"]
            oracle["mode"] = "default"
            oracle["seed"] = 3
        self._assert_aborts_before_writes(seeded_default, "requires seed=null")

    def test_missing_context_unit_aborts_before_any_write(self):
        def mutate(case):
            case["units"].pop("mass", None)
        self._assert_aborts_before_writes(mutate, "units.mass")

    def test_wrong_concentration_unit_aborts_before_any_write(self):
        def mutate(case):
            case["units"]["concentration"] = "pg/m3"
        self._assert_aborts_before_writes(mutate, "units.concentration")

    def test_missing_metric_definition_refs_aborts_before_any_write(self):
        def mutate(case):
            case["validation_definition_refs"]["metric_contracts"] = []
        self._assert_aborts_before_writes(mutate, "metric_contracts")

    def test_outgrid_rounding_mismatch_aborts_before_any_write(self):
        def mutate(case):
            case["domain"]["dx_deg"] = 0.251

        self._assert_aborts_before_writes(mutate, "OUTGRID DXOUT")

    def test_release_longitude_rounding_mismatch_aborts_before_any_write(self):
        def mutate(case):
            case["release"]["geometry"]["lon_deg"] = 10.0004

        self._assert_aborts_before_writes(mutate, "RELEASES LON1")

    def test_release_latitude_rounding_mismatch_aborts_before_any_write(self):
        def mutate(case):
            case["release"]["geometry"]["lat_deg"] = 10.0004

        self._assert_aborts_before_writes(mutate, "RELEASES LAT1")

    def test_release_height_rounding_mismatch_aborts_before_any_write(self):
        def mutate(case):
            case["release"]["geometry"]["z_m"] = 50.0004

        self._assert_aborts_before_writes(mutate, "RELEASES Z1")

    def test_real_weather_validates_meteorology_metadata_in_preflight(self):
        GEN.CASES = self.cases_dir
        GEN.FORTRAN_OUT = self.out_root
        case = load_case("ETEX-MINI-013")
        case = copy.deepcopy(case)
        case["integration"]["start"] = "20240101000000"
        case["integration"]["total_s"] = 3600
        case_file = str(REAL_CASES / "ETEX-MINI-013.json")
        normalized = GEN.validate_and_normalize_case_for_generation(
            "RW-PROBE", case, case_file, self.tracer, self.aerosol
        )
        self.assertEqual(normalized["wind_profile"], "real_weather")
        self.assertEqual(normalized["files"]["METEO_ARGS.txt"], "\n")
        self.assertIn("# Real-weather case", normalized["files"]["METEO.txt"])
        self.assertIn("era5", normalized["files"]["METEO.txt"])
        # Removing the metadata must abort the preflight, before any write.
        missing = copy.deepcopy(case)
        del missing["wind"]["meteorology"]
        before = snapshot(self.out_root)
        with self.assertRaises(SystemExit) as ctx:
            GEN.validate_and_normalize_case_for_generation(
                "RW-PROBE", missing, case_file, self.tracer, self.aerosol
            )
        self.assertIn("meteorology", str(ctx.exception))
        self.assertEqual(snapshot(self.out_root), before)

    def test_single_valid_case_produces_expected_files_via_write_case_fixtures(self):
        GEN.CASES = self.cases_dir
        GEN.FORTRAN_OUT = self.out_root
        prepared = GEN.prepare_cases(["ADV-ANA-001"], self.tracer, self.aerosol)
        self.assertEqual(len(prepared), 1)
        original_verify = GEN.verify_case
        try:
            GEN.verify_case = lambda *args, **kwargs: (_ for _ in ()).throw(
                AssertionError("writer must not perform scientific verification")
            )
            GEN.write_case_fixtures(prepared[0], self.out_root)
        finally:
            GEN.verify_case = original_verify
        case_dir = self.out_root / "ADV-ANA-001"
        for name in (
            "COMMAND",
            "RELEASES",
            "OUTGRID",
            "AGECLASSES",
            "RECEPTORS",
            "METEO_ARGS.txt",
            "METEO.txt",
            "INPUT_DERIVATION.json",
        ):
            self.assertTrue((case_dir / name).is_file(), name)
        self.assertTrue((case_dir / "SPECIES" / "SPECIES_024").is_file())
        provenance = json.loads(
            (case_dir / "INPUT_DERIVATION.json").read_text(encoding="utf-8")
        )
        self.assertEqual(
            provenance["case_file"], "fixtures/corpus/cases/ADV-ANA-001.json"
        )
        self.assertIn("&COMMAND", (case_dir / "COMMAND").read_text(encoding="utf-8"))

    def test_full_generator_run_writes_all_fixtures(self):
        GEN.CASES = self.cases_dir
        GEN.FORTRAN_OUT = self.out_root
        self._run_main()
        expected = {
            "ADV-ANA-001",
            "WIND-UNI-002",
            "WIND-SHEAR-003",
            "PBL-STABLE-004",
            "PBL-NEUTRAL-005",
            "PBL-UNSTABLE-006",
            "DRY-007",
            "WET-008",
            "RESTART-010",
        }
        self.assertEqual(
            {d.name for d in self.out_root.iterdir() if d.is_dir()}, expected
        )
        for case_id in expected:
            case_dir = self.out_root / case_id
            self.assertTrue((case_dir / "COMMAND").is_file(), case_id)
            self.assertTrue((case_dir / "RELEASES").is_file(), case_id)
            self.assertTrue((case_dir / "OUTGRID").is_file(), case_id)
            self.assertTrue((case_dir / "AGECLASSES").is_file(), case_id)
            self.assertTrue((case_dir / "RECEPTORS").is_file(), case_id)
            self.assertTrue((case_dir / "METEO_ARGS.txt").is_file(), case_id)
            self.assertTrue((case_dir / "METEO.txt").is_file(), case_id)
            self.assertTrue((case_dir / "INPUT_DERIVATION.json").is_file(), case_id)
        self.assertTrue((self.out_root / "RESTART-010" / "RESTART-NOTE.txt").is_file())
        restart_provenance = json.loads(
            (self.out_root / "RESTART-010" / "INPUT_DERIVATION.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(
            restart_provenance["case_file"],
            "fixtures/corpus/cases/RESTART-010.json",
        )


class DirectionOutputSemanticsTest(unittest.TestCase):
    """COMMAND direction/output timing must come from the manifest only.

    The FLEXPART namelist keys LDIRECT/LOUTSTEP/LOUTAVER/LOUTSAMPLE are
    derived artifacts of the required `simulation_direction` and `output`
    manifest blocks (Issue #51 / #57): no hard-coded value may substitute for
    a missing or inconsistent declaration.
    """

    def _case(self, case_id="WIND-UNI-002"):
        return copy.deepcopy(load_case(case_id))

    def test_missing_simulation_direction_is_rejected(self):
        case = self._case()
        del case["simulation_direction"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("simulation_direction", str(ctx.exception))

    def test_unknown_simulation_direction_is_rejected(self):
        case = self._case()
        case["simulation_direction"] = "sideways"
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("simulation_direction", str(ctx.exception))

    def test_missing_output_spec_is_rejected(self):
        case = self._case()
        del case["output"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("output", str(ctx.exception))

    def test_missing_individual_output_timing_field_is_rejected(self):
        case = self._case()
        del case["output"]["interval_s"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("interval_s", str(ctx.exception))

    def test_missing_output_quantity_is_rejected(self):
        case = self._case()
        del case["output"]["quantity"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("quantity", str(ctx.exception))

    def test_zero_negative_and_fractional_timing_values_are_rejected(self):
        for field in ("interval_s", "averaging_window_s", "sampling_interval_s"):
            for value in (0, -1, 1500.5):
                case = self._case()
                case["output"][field] = value
                with self.assertRaises(SystemExit) as ctx:
                    GEN.command_text("WIND-UNI-002", case)
                self.assertIn(field, str(ctx.exception))

    def test_sampling_interval_exceeding_averaging_window_is_rejected(self):
        case = self._case()
        case["output"]["sampling_interval_s"] = 3600
        case["output"]["averaging_window_s"] = 1800
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("sampling_interval_s", str(ctx.exception))

    def test_averaging_window_exceeding_output_interval_is_rejected(self):
        case = self._case()
        case["output"]["averaging_window_s"] = 3600
        case["output"]["interval_s"] = 1800
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("averaging_window_s", str(ctx.exception))

    def test_command_output_uses_manifest_values_not_constants(self):
        for case_id in (
            "ADV-ANA-001",
            "WIND-UNI-002",
            "WIND-SHEAR-003",
            "PBL-STABLE-004",
            "PBL-NEUTRAL-005",
            "PBL-UNSTABLE-006",
            "DRY-007",
            "WET-008",
            "REPEAT-009",
        ):
            text = GEN.command_text(case_id, load_case(case_id))
            self.assertEqual(int(GEN.namelist_value(text, "LDIRECT")), 1, case_id)
            self.assertEqual(int(GEN.namelist_value(text, "LOUTSTEP")), 1800, case_id)
            self.assertEqual(int(GEN.namelist_value(text, "LOUTAVER")), 1800, case_id)
            self.assertEqual(int(GEN.namelist_value(text, "LOUTSAMPLE")), 300, case_id)

    def test_changing_output_interval_changes_generated_command(self):
        case = self._case()
        case["output"]["interval_s"] = 3600
        case["output"]["averaging_window_s"] = 3600
        text = GEN.command_text("WIND-UNI-002", case)
        self.assertEqual(int(GEN.namelist_value(text, "LOUTSTEP")), 3600)
        self.assertEqual(int(GEN.namelist_value(text, "LOUTAVER")), 3600)
        self.assertEqual(int(GEN.namelist_value(text, "LOUTSAMPLE")), 300)
        self.assertIn("LOUTSTEP=           3600,\n", text)
        self.assertNotEqual(text, GEN.command_text("WIND-UNI-002", self._case()))

    def test_backward_direction_rejected_until_semantics_are_modeled(self):
        case = self._case()
        case["simulation_direction"] = "backward"
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        rendered = str(ctx.exception)
        self.assertIn("deliberately unsupported", rendered)
        self.assertIn("source-receptor", rendered)

    def test_output_quantity_is_closed_like_rust_enum(self):
        case = self._case()
        case["output"]["quantity"] = "banana"
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("output.quantity", str(ctx.exception))

    def test_output_timings_must_match_declared_lsynctime(self):
        case = self._case()
        case["output"]["sampling_interval_s"] = 301
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("LSYNCTIME", str(ctx.exception))

        case = self._case()
        case["output"]["interval_s"] = case["oracle_command_overrides"]["lsynctime_s"]
        case["output"]["averaging_window_s"] = case["oracle_command_overrides"]["lsynctime_s"]
        case["output"]["sampling_interval_s"] = case["oracle_command_overrides"]["lsynctime_s"]
        with self.assertRaises(SystemExit) as ctx:
            GEN.command_text("WIND-UNI-002", case)
        self.assertIn("2*LSYNCTIME", str(ctx.exception))

    def test_checked_in_fixtures_preserve_effective_configuration(self):
        for case_id in (
            "ADV-ANA-001",
            "WIND-UNI-002",
            "WIND-SHEAR-003",
            "PBL-STABLE-004",
            "PBL-NEUTRAL-005",
            "PBL-UNSTABLE-006",
            "DRY-007",
            "WET-008",
            "RESTART-010",
        ):
            case = load_case("PBL-NEUTRAL-005" if case_id == "RESTART-010" else case_id)
            text = GEN.command_text(case_id, case)
            expected = (
                REPO / "fixtures" / "corpus" / "fortran" / case_id / "COMMAND"
            ).read_text(encoding="utf-8")
            self.assertEqual(
                text, expected, f"{case_id} COMMAND drifted after migration"
            )

    def test_no_hard_coded_direction_or_output_defaults_remain_in_generator(self):
        source = (REPO / "scripts" / "corpus" / "generate_fortran_fixtures.py").read_text(
            encoding="utf-8"
        )
        code = [
            line for line in source.splitlines() if not line.lstrip().startswith("#")
        ]
        for key in ("LDIRECT", "LOUTSTEP", "LOUTAVER", "LOUTSAMPLE", "LSYNCTIME"):
            self.assertIsNone(
                re.search(rf"{key}\s*=\s*[+-]?\d", "\n".join(code)),
                f"hard-coded {key} default remains in the generator path",
            )

    def test_verify_case_detects_direction_drift(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            flex = tmp / "flexpart"
            flex.mkdir()
            tracer, aerosol = make_upstream_species(flex)
            case = load_case("WIND-UNI-002")
            out_root = tmp / "fortran"
            out_root.mkdir()
            prepared = GEN.validate_and_normalize_case_for_generation(
                "WIND-UNI-002",
                case,
                case_file=str(REAL_CASES / "WIND-UNI-002.json"),
                tracer=tracer,
                aerosol=aerosol,
            )
            GEN.write_case_fixtures(prepared, out_root)
            command_path = out_root / "WIND-UNI-002" / "COMMAND"
            text = command_path.read_text(encoding="utf-8")
            tampered = text.replace(" LDIRECT=               1,", " LDIRECT=              -1,")
            self.assertNotEqual(text, tampered)
            command_path.write_text(tampered, encoding="utf-8")
            with self.assertRaises(SystemExit) as ctx:
                GEN.verify_case("WIND-UNI-002", case, out_root / "WIND-UNI-002", 24)
            self.assertIn("LDIRECT", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
