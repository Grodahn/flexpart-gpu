"""Fail-closed orchestration tests using subprocesses, not scientific substitutes."""
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import agent_tests as runner


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.run = Path(self.temp.name)

    def execute(self, text="test result: ok. 1 passed; 0 failed; 0 ignored; 0 filtered out\n", code=0, kind="counts", stages=None):
        stages = stages or [runner.Stage("fixture", ("fixture",), kind), runner.Stage("later", ("later",))]
        def process(command, **kwargs):
            kwargs["stdout"].write(text.encode())
            return subprocess.CompletedProcess(command, code)
        with mock.patch.object(runner.subprocess, "run", side_effect=process):
            return runner.run_stages("fixture", "focused", stages, self.run, "a" * 40)

    def test_every_domain_profile_has_explicit_commands(self):
        expected = {
            "meteorology-resident": (["resident"], ["resident", "resident-corners"]),
            "transport-advection": (["advection"], ["advection", "paired"]),
            "simulation": (["preflight", "order"], ["preflight", "forward", "forward-validation", "backward"]),
            "validation-provenance": (["wrapper"], ["wrapper", "input-audit", "manifest", "provenance", "case", "case-facade"]),
        }
        self.assertEqual(set(expected), set(runner.selections()))
        for domain, profiles in runner.selections().items():
            for index, level in enumerate(("focused", "domain")):
                with self.subTest(domain=domain, level=level):
                    stages = profiles[level]
                    self.assertEqual([s.name for s in stages], expected[domain][index])
                    for s in stages:
                        self.assertTrue(s.command)
                        self.assertNotIn("clean", s.command)
                        if s.command[:2] == ("cargo", "test"):
                            self.assertIn("--test-threads=1", s.command)
                            self.assertIn("--nocapture", s.command)
                            if "--exact" in s.command:
                                self.assertGreater(s.command.index("--exact"), s.command.index("--"))
        advection = runner.selections()["transport-advection"]["focused"][0]
        self.assertEqual(advection.command, ("cargo", "test", "--test", "integration", "software_advection::test_sw_wgpu_advection_001_constant_wind_displacement", "--", "--exact", "--nocapture", "--test-threads=1"))

    def test_exact_command_registry_snapshot(self):
        expected = {
            "resident": "cargo test --test meteorology_resident -- --nocapture --test-threads=1",
            "resident-corners": "cargo test --lib gpu::meteorology::resident::tests -- --nocapture --test-threads=1",
            "advection": "cargo test --test integration software_advection::test_sw_wgpu_advection_001_constant_wind_displacement -- --exact --nocapture --test-threads=1",
            "preflight": "cargo run --bin gpu-preflight -- --software --json-output {run}/preflight.json",
            "order": "cargo test --test forward_timeloop test_forward_timeloop_transport_precedes_deposition_and_reports_precede_advance -- --exact --nocapture --test-threads=1",
            "forward": "cargo test --test forward_timeloop -- --nocapture --test-threads=1",
            "forward-validation": "cargo test --test forward_timeloop -- --nocapture --test-threads=1",
            "backward": "cargo test --test backward_timeloop -- --nocapture --test-threads=1",
            "wrapper": "PYTHON scripts/test_agent_validation.py",
            "input-audit": "PYTHON scripts/corpus/test_audit_corpus_inputs.py",
            "manifest": "PYTHON scripts/corpus/test_write_corpus_manifest_v1.py",
            "provenance": "PYTHON scripts/provenance/test_run_provenance.py",
            "case": "cargo test --lib validation::case:: -- --nocapture --test-threads=1",
            "case-facade": "cargo test --test validation_case_contract -- --nocapture --test-threads=1",
            "paired": "PYTHON scripts/agent_validation.py --check comparison --case ADV-ANA-001 --output-dir {run}/paired",
        }
        for domain, profiles in runner.selections().items():
            for level, stages in profiles.items():
                for stage in stages:
                    with self.subTest(domain=domain, level=level, stage=stage.name):
                        command = ["PYTHON" if part == sys.executable else part for part in stage.command]
                        self.assertEqual(command, expected[stage.name].split())
                        mode = "1" if stage.name == "forward-validation" else "0"
                        expected_env = (("FLEXPART_GPU_VALIDATION", mode),) if stage.name in ("forward", "forward-validation", "order") else ()
                        self.assertEqual(stage.environment, expected_env)

    def test_list_and_invalid_selection_do_not_run(self):
        with mock.patch.object(sys, "argv", ["agent_tests", "--list"]), mock.patch.object(runner.subprocess, "run") as run, contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(runner.main(), 0)
            self.assertEqual(set(json.loads(output.getvalue())), set(runner.selections()))
            run.assert_not_called()
        for args in (["--domain", "unknown"], ["--domain", "simulation", "--level", "full"]):
            with mock.patch.object(sys, "argv", ["agent_tests", *args]), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                runner.main()
            self.assertEqual(error.exception.code, 2)

    def test_noisy_success_retains_every_byte_and_one_measured_json_line(self):
        text = "noise\n" * 10000 + "test result: ok. 2 passed; 0 failed; 0 ignored; 0 filtered out\n"
        summary = self.execute(text)
        with contextlib.redirect_stdout(io.StringIO()) as output:
            runner.emit(summary)
        self.assertEqual(len(output.getvalue().splitlines()), 1)
        self.assertEqual(summary["terminal_bytes"], len(output.getvalue().encode()))
        self.assertLess(summary["terminal_bytes"], len(text.encode()))
        self.assertEqual((self.run / "fixture.log").read_bytes(), text.encode())
        self.assertEqual(summary["output_bytes"], 2 * len(text.encode()))
        self.assertEqual(json.loads((self.run / "summary.json").read_text())["state"], "PASS")
        self.assertEqual(summary["counts"]["executed"], 4)

    def test_nonzero_exit_preserved_with_bounded_assertions_and_not_run(self):
        text = "assertion failed: retained early\n" + ("x" * 1000 + "\n") * 80
        summary = self.execute(text, 42)
        self.assertEqual((summary["state"], summary["exit_code"]), ("FAIL", 42))
        first, later = summary["stages"]
        self.assertEqual(first["exit_code"], 42)
        self.assertEqual(later["state"], "NOT_RUN")
        self.assertIn("assertion failed", first["assertions"][0])
        self.assertLessEqual(len(first["diagnostic_tail"]), 30)
        self.assertTrue(all(len(line) <= 500 for line in first["diagnostic_tail"]))
        self.assertEqual((self.run / "fixture.log").stat().st_size, len(text.encode()))

    def test_signal_status_retained(self):
        summary = self.execute("terminated", -15)
        self.assertEqual(summary["exit_code"], -15)
        self.assertEqual(summary["stages"][0]["exit_code"], -15)

    def test_missing_executable_blocked(self):
        with mock.patch.object(runner.subprocess, "run", side_effect=FileNotFoundError("no cargo")):
            summary = runner.run_stages("fixture", "focused", [runner.Stage("fixture", ("absent",))], self.run, "revision")
        self.assertEqual((summary["state"], summary["exit_code"]), ("BLOCKED", 127))
        self.assertIsNone(summary["stages"][0]["exit_code"])

    def test_missing_adapter_blocked(self):
        summary = self.execute("required software WGSL adapter not found", 101)
        self.assertEqual((summary["state"], summary["exit_code"]), ("BLOCKED", 101))

    def test_empty_zero_skipped_and_failed_despite_zero_are_rejected(self):
        for text in ("", "test result: ok. 0 passed; 0 failed; 0 ignored", "test result: ok. 1 passed; 1 failed; 0 ignored", "No GPU adapter found â€” skipping test\ntest result: ok. 1 passed; 0 failed; 0 ignored"):
            with self.subTest(text=text):
                summary = self.execute(text)
                self.assertNotEqual(summary["state"], "PASS")
                self.assertNotEqual(summary["exit_code"], 0)
                self.assertEqual(summary["stages"][0]["exit_code"], 0)
        summary = self.execute("test result: ok. 1 passed; 0 failed; 1 ignored")
        self.assertEqual(summary["stages"][0]["state"], "SKIPPED")
        self.assertEqual(summary["state"], "ERROR")

    def test_legacy_backward_cannot_claim_device_execution(self):
        summary = self.execute(kind="legacy-backward")
        self.assertEqual(summary["state"], "BLOCKED")
        self.assertEqual(summary["stages"][0]["exit_code"], 0)
        self.assertEqual(summary["exit_code"], 1)

    def test_unittest_counts_include_failures_and_skips(self):
        self.assertEqual(runner.counts("Ran 7 tests in 0.1s\nFAILED (failures=1, errors=2, skipped=1)"), {"executed": 6, "failed": 3, "skipped": 1})

    def test_gpu_markers_required(self):
        for kind in ("advection", "order", "forward"):
            with self.subTest(kind=kind):
                self.assertEqual(self.execute(kind=kind)["state"], "ERROR")
        text = "SW-WGPU-ADVECTION-001: adapter=WARP backend=Dx12 type=Cpu software=true\nSW-WGPU-ADVECTION-001: east=36000\nSW-WGPU-ADVECTION-001: software_adapter=true\ntest result: ok. 1 passed; 0 failed; 0 ignored"
        self.assertEqual(self.execute(text, kind="advection")["state"], "PASS")

    def test_preflight_missing_malformed_skipped_and_empty_adapter(self):
        path = self.run / "preflight.json"
        for value in ("", "[]", "{", '{"schema":{"id":"flexpart-gpu.gpu-preflight","version":1},"status":"skipped"}'):
            path.write_text(value)
            self.assertEqual(self.execute(kind="preflight")["state"], "ERROR")
        report = {"schema": {"id": "flexpart-gpu.gpu-preflight", "version": 1}, "status": "passed", "report": {"adapter": {}, "smoke_test": {"status": "passed", "actual_value": 3, "expected_value": 3}}}
        path.write_text(json.dumps(report))
        self.assertEqual(self.execute(kind="preflight")["state"], "ERROR")
        report["report"]["adapter"] = {"name": "WARP", "backend": "dx12", "device_type": "cpu", "adapter_class": "software_wgsl"}
        path.write_text(json.dumps(report))
        self.assertEqual(self.execute(kind="preflight")["state"], "PASS")

    def test_stale_resident_report_and_lock_fail_closed(self):
        with mock.patch.object(runner, "REPO", self.run):
            path = self.run / runner.RESIDENT_REPORT
            path.parent.mkdir(parents=True)
            path.write_text("{}")
            self.assertIn("fresh resident", self.execute(kind="resident")["stages"][0]["diagnostic"])
            stage = runner.Stage("resident", ("fixture",), "resident")
            with runner.evidence_lock([stage]):
                with self.assertRaises(FileExistsError):
                    with runner.evidence_lock([stage]):
                        pass
            self.assertFalse((self.run / "target/agent-tests/resident.lock").exists())

    def test_unique_run_directories(self):
        with mock.patch.object(runner, "REPO", self.run), mock.patch.object(sys, "argv", ["agent_tests", "--domain", "validation-provenance"]), mock.patch.object(runner.subprocess, "check_output", return_value="a" * 40), mock.patch.object(runner.subprocess, "run", side_effect=FileNotFoundError("fixture")), contextlib.redirect_stdout(io.StringIO()) as output:
            runner.main()
            runner.main()
        summaries = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertNotEqual(summaries[0]["summary"], summaries[1]["summary"])
        self.assertTrue(all(Path(s["summary"]).exists() for s in summaries))

    def test_resident_report_audits_existing_ci_contract_and_retains_copy(self):
        import hashlib
        with mock.patch.object(runner, "REPO", self.run):
            shader_dir = self.run / "src/shaders"
            shader_dir.mkdir(parents=True)
            for name in runner.RESIDENT_SHADERS:
                (shader_dir / f"{name}.wgsl").write_text(name)
            shader_hash = hashlib.sha256("\n".join(runner.RESIDENT_SHADERS).encode()).hexdigest()
            rows = []
            for name in runner.RESIDENT_CASES:
                lanes = [2, 2, 2, 2]
                if name == "negative-signed-cell-memory-safe":
                    lanes[1] = 3
                if name == "within-stencil-differing-geometry-owned-by-118":
                    lanes[1] = 7
                rows.append({"id": name, "passed": True, "submission_count": 1, "intermediate_d2h_count": 0,
                    "host_prepare_sample": False, "metadata": {"capacity": 4, "component_count": 1, "lane_stride": 1},
                    "status": {"fatal": name == "fatal-across-workgroups", "lanes": lanes}, "query_inputs": [0] * 4,
                    "input_sha256": "a" * 64, "order": ["device_status_reset", "particle_producer", "resident_adapter", "canonical_sampling", "device_result_status", "status_guarded_fixture_consumer"]})
            report = {"schema": {"id": "flexpart-gpu.meteorology-resident", "version": 1}, "passed": True,
                "revision": "revision", "adapter": {"name": "WARP", "backend": "dx12", "device_type": "cpu", "adapter_class": "software_wgsl"},
                "shader_bundle_sha256": shader_hash, "cases": rows, "static_grid_context_time_rejection": True}
            source = self.run / runner.RESIDENT_REPORT
            source.parent.mkdir(parents=True)
            stage = runner.Stage("resident", ("fixture",), "resident")
            source.write_text(json.dumps(report))
            result = runner.validate_evidence(stage, "", self.run, "revision")
            retained = Path(result["paths"][0])
            self.assertEqual(retained.read_bytes(), source.read_bytes())
            for field, value in (("cases", []), ("revision", "stale"), ("adapter", {}), ("shader_bundle_sha256", "wrong"), ("passed", False)):
                malformed = dict(report, **{field: value})
                source.write_text(json.dumps(malformed))
                with self.subTest(field=field), self.assertRaises(ValueError):
                    runner.validate_evidence(stage, "", self.run, "revision")
            source.write_text("{}")
            self.assertNotEqual(retained.read_bytes(), source.read_bytes())

    def paired_fixture(self, dirty=False):
        provenance = runner.provenance
        paired = self.run / "paired"
        paired.mkdir(exist_ok=True)
        (self.run / "reference").mkdir(exist_ok=True)
        pin, revision = "b" * 40, "a" * 40
        (self.run / "reference/flexpart-11.1.json").write_text(json.dumps({"pinned_commit": pin}))
        case_path = paired / "ADV-ANA-001.json"
        case_path.write_text('{"schema_version":2,"case_id":"ADV-ANA-001"}')
        seed = paired / "seed_000.json"
        seed.write_text('{"seed_index":0}')
        header = paired / "header"
        header.write_bytes(b"oracle-output")
        report_path = paired / "comparison-report.json"
        report = {"status": "DIAGNOSTIC_NO_PARITY_VERDICT", "cases": {"ADV-ANA-001": {
            "status": "implemented", "seeds": [{"seed_index": 0}],
            "candidate_files": {str(seed): provenance.digest(seed)},
            "oracle_files": {str(header): provenance.digest(header)},
            "oracle_budget": {"status": "closed"}}}}
        report_path.write_text(json.dumps(report))
        case = provenance.case_manifest_identity(case_path)
        build = provenance.candidate_build_identity(revision, "c" * 64)
        common = {"case": case, "realization": {}, "candidate_revision": revision,
            "candidate_build": build, "candidate_executable_sha256": "c" * 64,
            "oracle_kind": provenance.ORACLE_PRISTINE, "oracle_revision": pin,
            "oracle_executable_sha256": "d" * 64,
            "oracle_profile": {"id": provenance.ORACLE_PROFILE_ID, "version": 1},
            "inputs_sha256": {report_path.name: provenance.digest(report_path), case_path.name: provenance.digest(case_path)}}
        executions = [
            provenance.build_execution_record(role="candidate", runtime_adapter="fixture-adapter",
                outputs_sha256={seed.name: provenance.digest(seed)}, **common),
            provenance.build_execution_record(role="oracle", cpu_runtime="flexpart-11.1-single-thread",
                outputs_sha256={header.name: provenance.digest(header)}, **common),
        ]
        manifest = provenance.create_run_manifest(cases=[case],
            candidate={"revision": revision, "worktree_dirty": dirty, "executable_sha256": "c" * 64, "build": build},
            oracle={"kind": provenance.ORACLE_PRISTINE, "pinned_commit": pin, "worktree_dirty": False,
                "executable_sha256": "d" * 64, "execution_profile": common["oracle_profile"]},
            runtime={"adapter": "fixture-adapter", "cpu_runtime": "flexpart-11.1-single-thread"},
            executions=executions, base=str(paired))
        manifest_path = paired / "run-manifest.json"
        manifest_path.write_text(json.dumps(manifest))
        summary = {"schema": "flexpart-gpu.agent-validation-summary.v1", "state": "PASS", "check": "comparison", "case_id": "ADV-ANA-001",
            "candidate": {"revision": revision}, "oracle": {"pinned_commit": pin}, "scientific_verdict": report["status"],
            "stages": [{"stage": name, "exit_code": 0} for name in ("candidate", "oracle", "input-audit", "comparison", "manifest")],
            "summary": str(paired / "summary.json"), "evidence": [str(report_path), str(manifest_path)]}
        (paired / "summary.json").write_text(json.dumps(summary))
        return summary, report, manifest

    def test_paired_success_consumes_existing_verdict_and_identity(self):
        with mock.patch.object(runner, "REPO", self.run):
            summary, _report, _manifest = self.paired_fixture()
            paired = self.run / "paired"
            result = self.execute(kind="paired")
            self.assertEqual(result["state"], "PASS")
            self.assertEqual(result["scientific_verdict"], summary["scientific_verdict"])
            self.assertEqual(result["stages"][0]["evidence"]["provenance_state"], "VERIFIED")
            for field, value in (("stages", []), ("evidence", []), ("oracle", {"pinned_commit": "wrong"}), ("scientific_verdict", "PARITY")):
                (paired / "summary.json").write_text(json.dumps(dict(summary, **{field: value})))
                self.assertEqual(self.execute(kind="paired")["state"], "ERROR")
            (paired / "summary.json").write_text("{")
            self.assertEqual(self.execute(code=37, kind="paired")["exit_code"], 37)

    def test_paired_rejects_nonempty_malformed_reports_and_manifests(self):
        with mock.patch.object(runner, "REPO", self.run):
            for filename in ("comparison-report.json", "run-manifest.json"):
                self.paired_fixture()
                (self.run / "paired" / filename).write_text('{"existing":"evidence"}')
                with self.subTest(filename=filename):
                    self.assertEqual(self.execute(kind="paired")["state"], "ERROR")
            self.paired_fixture()
            (self.run / "paired/header").write_bytes(b"substituted oracle output")
            self.assertEqual(self.execute(kind="paired")["state"], "ERROR")
            self.paired_fixture()
            (self.run / "paired/seed_000.json").unlink()
            self.assertEqual(self.execute(kind="paired")["state"], "ERROR")

    def test_paired_keeps_dirty_checkout_attribution_explicitly_partial(self):
        with mock.patch.object(runner, "REPO", self.run):
            self.paired_fixture(dirty=True)
            result = self.execute(kind="paired")
            self.assertEqual(result["state"], "PASS")
            self.assertEqual(result["stages"][0]["evidence"]["provenance_state"], "PARTIAL")

    def test_forward_profiles_override_inherited_validation_mode(self):
        stages = runner.selections()["simulation"]["domain"][1:3]
        environments = []
        text = "test result: ok. 1 passed; 0 failed; 0 ignored\n" + runner.ORDER_MARKER + "\nTIMELOOP-DEFERRED-126: WGSL output\nTIMELOOP-ERROR-126: retry"
        def process(command, **kwargs):
            environments.append(kwargs["env"]["FLEXPART_GPU_VALIDATION"])
            kwargs["stdout"].write(text.encode())
            return subprocess.CompletedProcess(command, 0)
        with mock.patch.dict(runner.os.environ, {"FLEXPART_GPU_VALIDATION": "1"}), mock.patch.object(runner.subprocess, "run", side_effect=process):
            result = runner.run_stages("simulation", "domain", stages, self.run, "revision")
        self.assertEqual(result["state"], "PASS")
        self.assertEqual(environments, ["0", "1"])

    def test_unwritable_run_directory_emits_compact_error_without_launch(self):
        with mock.patch.object(runner, "REPO", self.run), mock.patch.object(sys, "argv", ["agent_tests", "--domain", "simulation"]), mock.patch.object(Path, "mkdir", side_effect=PermissionError("denied")), mock.patch.object(runner.subprocess, "run") as process, contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(runner.main(), 1)
            process.assert_not_called()
        summary = json.loads(output.getvalue())
        self.assertEqual(summary["state"], "ERROR")
        self.assertIsNone(summary["summary"])
        self.assertEqual(len(output.getvalue().splitlines()), 1)

    def test_unwritable_stage_log_does_not_escape_error_summary(self):
        with mock.patch.object(Path, "open", side_effect=PermissionError("log denied")):
            summary = self.execute()
        self.assertEqual(summary["state"], "ERROR")
        self.assertEqual(summary["exit_code"], 1)
        self.assertIsNone(summary["stages"][0]["exit_code"])
        self.assertIsNone(summary["stages"][0]["output_bytes"])
        self.assertEqual(summary["stages"][1]["state"], "NOT_RUN")

    def test_unwritable_summary_preserves_nonzero_subprocess_status(self):
        for code in (0, 37, -15):
            summary = self.execute(code=code)
            with mock.patch.object(Path, "write_text", side_effect=PermissionError("summary denied")), contextlib.redirect_stdout(io.StringIO()) as output:
                runner.emit(summary)
            emitted = json.loads(output.getvalue())
            self.assertEqual(emitted["state"], "ERROR")
            self.assertEqual(emitted["exit_code"], code or 1)
            self.assertIsNone(emitted["summary"])
            self.assertEqual(emitted["terminal_bytes"], len(output.getvalue().encode()))

    def test_paired_delegation_and_malformed_evidence(self):
        stage = runner.selections()["transport-advection"]["domain"][1]
        self.assertEqual(stage.command[1:6], ("scripts/agent_validation.py", "--check", "comparison", "--case", "ADV-ANA-001"))
        self.assertIn(self.execute(kind="paired")["state"], ("ERROR", "BLOCKED"))
        path = self.run / "paired/summary.json"
        path.parent.mkdir()
        path.write_text('{"state":"BLOCKED"}')
        self.assertEqual(self.execute(code=1, kind="paired")["state"], "BLOCKED")
        path.write_text('{"state":"PASS"}')
        self.assertIn(self.execute(kind="paired")["state"], ("ERROR", "BLOCKED"))


if __name__ == "__main__":
    unittest.main()
