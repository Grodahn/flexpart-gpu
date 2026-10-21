"""Fail-closed reporting and input derivation regressions for issue #199."""
import copy
import json
import unittest
from pathlib import Path
from unittest.mock import patch

import benchmark_early as bench


class BenchmarkTests(unittest.TestCase):
    def setUp(self):
        self.manifest = json.loads(bench.MANIFEST.read_text())

    def test_empty_scaled_and_unwarmed_campaigns_rejected(self):
        bench.validate_manifest(self.manifest)
        for field, invalid in [("particle_counts", []), ("particle_counts", [1000]),
                               ("repetitions", 0), ("warmup_seconds", 0),
                               ("warmup_seconds", float("nan"))]:
            changed = copy.deepcopy(self.manifest)
            changed[field] = invalid
            with self.assertRaises(ValueError):
                bench.validate_manifest(changed)

    def test_hardware_and_complete_evidence_required(self):
        data = {"regime":"canonical-production", "calculation_path":"wgsl_device",
                "adapter":{"adapter_class":"hardware_gpu"}, "particles":100000,
                "measured_steps":1,"manifest_sha256":"abc", "canonical_identity":{"source":"x"},
                "invariants":{"active_particles":100000,"no_boundary_contact":True},
                "steps":[{}],"particle_output_sha256":"def", "measured_wall_seconds":1,
                "particle_steps_per_second":100000}
        bench.validate_candidate(data, 100000, "abc", 1)
        for field in list(data):
            changed = copy.deepcopy(data)
            changed.pop(field)
            with self.assertRaises((ValueError, KeyError)):
                bench.validate_candidate(changed, 100000, "abc", 1)
        data["adapter"]["adapter_class"] = "software_wgsl"
        with self.assertRaises(ValueError):
            bench.validate_candidate(data, 100000, "abc", 1)

    def test_missing_and_failed_samples_never_pass_aggregate(self):
        rows = bench.aggregate([], [100000])
        self.assertTrue(all(row["state"] == "FAIL" for row in rows))
        records = [{"regime":"canonical-production","particles":100000,"state":"PASS",
                    "result":{"measured_wall_seconds":2}},
                   {"regime":"canonical-production","particles":100000,"state":"FAIL"}]
        rows = bench.aggregate(records, [100000])
        self.assertEqual(rows[0]["state"], "FAIL")
        self.assertEqual(rows[0]["passed"], 1)
        self.assertEqual(rows[0]["wall_seconds"]["mean"], 2)
        self.assertFalse(any("speedup" in row for row in rows))

    def test_invalid_elapsed_values_rejected(self):
        for value in [0, -1, float("inf"), float("nan")]:
            with self.assertRaises(ValueError):
                bench.distribution([value])

    def test_fortran_actual_count_and_full_duration_required(self):
        text = "Time: 300 seconds. Total spawned: 100000 alive: 100000 terminated: 0\n"
        self.assertEqual(bench.fortran_counts(text, 100000, 300)[0]["alive"], 100000)
        for invalid, count, duration in [("", 100000, 300), (text, 1000000, 300),
                                         (text, 100000, 3600),
                                         (text.replace("terminated: 0", "terminated: 1"), 100000, 300)]:
            with self.assertRaises(ValueError):
                bench.fortran_counts(invalid, count, duration)

    def test_future_production_changes_rejected(self):
        with patch.object(bench.subprocess, "check_output", side_effect=["a"*40, "", "src/shaders/advection_corrector.wgsl"]), \
             patch.object(bench.subprocess, "run"):
            with self.assertRaisesRegex(ValueError, "production sources changed"):
                bench.require_candidate(self.manifest)

    def test_reference_timer_diagnostics_require_finite_complete_fields(self):
        text = "\n".join(label + ": 1.25D+00 seconds" for label in
            ["Read wind fields", "Timemanager", "first timestep", "Write particle files", "Total running time"])
        self.assertEqual(bench.reference_timers(text)["seconds"]["Timemanager"], 1.25)
        for invalid in ["", text.replace("Timemanager: 1.25D+00", "Timemanager: -1")]:
            with self.assertRaises(ValueError):
                bench.reference_timers(invalid)

    def test_existing_generator_preserves_count_mass_and_physics(self):
        case = json.loads((bench.REPO / self.manifest["fortran"]["case"]).read_text())
        tracer = bench.REPO / "fixtures/corpus/fortran/ADV-ANA-001/SPECIES/SPECIES_024"
        for count in self.manifest["particle_counts"]:
            case["release"]["particle_count"] = count
            case["release"]["inventory"]["quantity_kg"] = count
            case["release"]["mass_kg_per_particle"] = 1
            result = bench.fixtures.validate_and_normalize_case_for_generation(
                case["case_id"], case, case_file="benchmark-test", tracer=tracer, aerosol=None)
            self.assertRegex(result["files"]["RELEASES"], rf"PARTS\s*=\s*{count}")
            self.assertRegex(result["files"]["COMMAND"], r"LTURBULENCE\s*=\s*0")
            self.assertEqual(result["case"]["release"]["particle_count"], count)
            self.assertEqual(json.loads(result["files"]["INPUT_DERIVATION.json"])["oracle_mass_g"], count * 1000)


if __name__ == "__main__":
    unittest.main()
