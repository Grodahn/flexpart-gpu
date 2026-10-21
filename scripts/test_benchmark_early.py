"""Fail-closed reporting and input derivation regressions for issue #199."""
import copy
import json
import tempfile
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
        phases = dict.fromkeys(["surface_interpolation_ms", "pbl_preparation_upload_ms", "forcing_ms",
            "deposition_upload_ms", "previous_wait_ms", "encode_submit_ms", "submit_to_poll_ms", "reported_total_ms"], 0.0)
        data = {"schema":"flexpart-gpu.canonical-benchmark-run.v1",
                "regime":"canonical-production", "calculation_path":"wgsl_device",
                "adapter":{"adapter_class":"hardware_gpu"}, "particles":100000,
                "measured_steps":12,"dt_seconds":300,"manifest_sha256":"abc", "canonical_identity":{"source":"x"},
                "configuration":self.manifest["candidate"],"warmup_steps":2,"warmup_wall_seconds":3,
                "measured_start_epoch_seconds":1704067800,"measured_end_epoch_seconds":1704071400,
                "invariants":{"active_particles":100000,"no_boundary_contact":True,"finite":True,
                              "mass_kg":100000,"min_height_m":10,"max_height_m":100,"top_m":1000},
                "steps":[{"wall_seconds":1,"phases":phases} for _ in range(12)],
                "particle_output_sha256":"def","particle_output_bytes":6400000, "measured_wall_seconds":12,
                "particle_steps_per_second":100000}
        bench.validate_candidate(data, 100000, "abc", self.manifest)
        for field in list(data):
            changed = copy.deepcopy(data)
            changed.pop(field)
            with self.assertRaises((ValueError, KeyError)):
                bench.validate_candidate(changed, 100000, "abc", self.manifest)
        for field, value in [("warmup_wall_seconds", 0), ("warmup_steps", 0),
                             ("measured_end_epoch_seconds", 1704067800), ("dt_seconds", 1),
                             ("particle_output_bytes", 0), ("configuration", {})]:
            changed = copy.deepcopy(data)
            changed[field] = value
            with self.assertRaises(ValueError):
                bench.validate_candidate(changed, 100000, "abc", self.manifest)
        for section, field, value in [("invariants", "mass_kg", 0), ("invariants", "finite", False),
                                       ("invariants", "min_height_m", float("nan")),
                                       ("invariants", "top_m", 100)]:
            changed = copy.deepcopy(data)
            changed[section][field] = value
            with self.assertRaises(ValueError):
                bench.validate_candidate(changed, 100000, "abc", self.manifest)
        for value in [None, {}, {**phases, "submit_to_poll_ms":float("nan")}]:
            changed = copy.deepcopy(data)
            changed["steps"][0]["phases"] = value
            with self.assertRaises((ValueError, KeyError, TypeError)):
                bench.validate_candidate(changed, 100000, "abc", self.manifest)
        data["adapter"]["adapter_class"] = "software_wgsl"
        with self.assertRaises(ValueError):
            bench.validate_candidate(data, 100000, "abc", self.manifest)

    def test_cargo_receipt_selects_configured_binary_and_rejects_missing_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "custom-target" / "bench-canonical.exe"
            binary.parent.mkdir()
            binary.write_bytes(b"current binary")
            log = root / "build.log"
            artifact = {"reason":"compiler-artifact", "target":{"name":"bench-canonical"},
                        "executable":str(binary)}
            log.write_text(json.dumps(artifact) + "\nFinished release build\n")
            record = {"returncode":0,"log":str(log)}
            self.assertEqual(bench.build_binary(record), binary.resolve())
            for payload in ["", json.dumps({**artifact, "executable":None}),
                            json.dumps({**artifact, "executable":str(root / "missing")})]:
                log.write_text(payload)
                with self.assertRaises(ValueError):
                    bench.build_binary(record)

    def test_manifest_snapshot_survives_external_source_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, frozen = root / "external.json", root / "scenario.json"
            source.write_text(json.dumps(self.manifest))
            manifest, identity = bench.freeze_manifest(source, frozen)
            source.write_text("{}")
            self.assertEqual(manifest, self.manifest)
            self.assertEqual(json.loads(frozen.read_text()), self.manifest)
            self.assertEqual(bench.sha(frozen), identity)

    def test_reference_meteorology_mutation_before_or_during_execution_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture, meteo = root / "inputs/options", root / "inputs/meteo"
            fixture.mkdir(parents=True)
            meteo.mkdir()
            weather = meteo / "GRIB"
            weather.write_text("original")
            executable = root / "src/FLEXPART"
            executable.parent.mkdir()
            executable.write_bytes(b"oracle")
            inputs = {"fixture":fixture,"meteo":meteo,"input_sha256":bench.tree_hashes(fixture.parent),
                      "case":{"integration":{"total_s":300}}}
            prepared = {"inputs":{100000:inputs},"build":{"oracle_executable_sha256":bench.sha(executable)},"docker":[]}

            def mutate(args, log):
                weather.write_text("changed")
                Path(log).write_text("CONGRATULATIONS\nTime: 300 seconds. Total spawned: 100000 alive: 100000 terminated: 0\n")
                return {"returncode":0,"log":str(log)}

            for during in [False, True]:
                weather.write_text("original" if during else "changed")
                folder = root / str(during)
                folder.mkdir()
                with patch.object(bench.oracle, "require_pristine"), \
                     patch.object(bench, "container_path", return_value="/inputs"), \
                     patch.object(bench, "command", side_effect=mutate) as command:
                    record = bench.fortran_run(prepared, root, 100000, folder)
                self.assertEqual(record["state"], "FAIL")
                self.assertIn("inputs changed", record["error"])
                self.assertEqual(command.call_count, int(during))

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
