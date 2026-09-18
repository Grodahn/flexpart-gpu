#!/usr/bin/env python3
"""Fail-closed checks for the oracle repeatability comparator (#49).

Standard library unittest only. Covers numeric difference quantification,
contract validation, and retained-repetition completeness without running
FLEXPART or claiming physics parity.
"""

import json
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts" / "corpus"))

import compare_oracle_repeatability as cmp_mod


class CompareNumbersTest(unittest.TestCase):
    def test_identical_payload_has_no_differences(self):
        payload = {"a": 1.0, "b": [1.0, 2.0], "c": {"d": "x"}}
        diffs = []
        cmp_mod.compare_numbers(payload, json.loads(json.dumps(payload)), "", diffs)
        self.assertEqual([d for d in diffs if d["field"]], [])

    def test_numeric_difference_reports_abs_and_rel(self):
        diffs = []
        cmp_mod.compare_numbers({"v": 100.0}, {"v": 110.0}, "", diffs)
        self.assertEqual(len(diffs), 1)
        self.assertEqual(diffs[0]["field"], "v")
        self.assertAlmostEqual(diffs[0]["abs_diff"], 10.0)
        self.assertAlmostEqual(diffs[0]["rel_diff"], 10.0 / 110.0)

    def test_list_difference_names_index(self):
        diffs = []
        cmp_mod.compare_numbers([1.0, 2.0], [1.0, 3.0], "levels", diffs)
        self.assertEqual(diffs[0]["field"], "levels[1]")
        self.assertAlmostEqual(diffs[0]["abs_diff"], 1.0)

    def test_summary_reports_maximums(self):
        diffs = [
            {"field": "a", "baseline": 1.0, "current": 2.0, "abs_diff": 1.0, "rel_diff": 0.5, "kind": "number"},
            {"field": "b", "baseline": 10.0, "current": 13.0, "abs_diff": 3.0, "rel_diff": 0.3, "kind": "number"},
        ]
        summary = cmp_mod.summarize_differences(diffs)
        self.assertEqual(summary["differing_fields"], 2)
        self.assertAlmostEqual(summary["max_abs_diff"], 3.0)
        self.assertAlmostEqual(summary["max_rel_diff"], 0.5)


class ProfileContractTest(unittest.TestCase):
    def test_canonical_profile_loads(self):
        reference, profile = cmp_mod.load_profile(REPO / "reference/flexpart-11.1.json")
        self.assertEqual(profile["id"], "flexpart-11.1-single-thread")
        self.assertEqual(profile["repeatability_cases"]["ADV-ANA-001"], "deterministic")
        self.assertEqual(profile["external_seed_control"], "unavailable")
        self.assertIn("pinned_commit", reference)

    def test_profile_rejects_substituted_case(self):
        reference = json.loads((REPO / "reference/flexpart-11.1.json").read_text(encoding="utf-8"))
        reference["execution_profile"]["repeatability_cases"] = {"OTHER": "deterministic"}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ref.json"
            path.write_text(json.dumps(reference), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "repeatability cases"):
                cmp_mod.load_profile(path)

    def test_profile_rejects_seed_control_claim(self):
        reference = json.loads((REPO / "reference/flexpart-11.1.json").read_text(encoding="utf-8"))
        reference["execution_profile"]["external_seed_control"] = "seed-42"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ref.json"
            path.write_text(json.dumps(reference), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unavailable"):
                cmp_mod.load_profile(path)


EXE_SHA_FOR_TESTS = "0" * 64


def write_rep(case_dir, name, raw_bytes, summary, runtime, consumed, exe_sha=EXE_SHA_FOR_TESTS):
    rep = case_dir / name
    raw = rep / "raw"
    raw.mkdir(parents=True)
    (raw / "header").write_bytes(raw_bytes)
    (raw / "dates").write_bytes(b"dates")
    (raw / "grid_conc_20240101010000").write_bytes(raw_bytes)
    (rep / "oracle_summary.json").write_text(json.dumps(summary), encoding="utf-8")
    (rep / "runtime_profile.json").write_text(json.dumps(runtime), encoding="utf-8")
    (rep / "fortran.log").write_text("FLEXPART ... CONGRATULATIONS\n", encoding="utf-8")
    (rep / "oracle_executable.sha256").write_text(exe_sha, encoding="utf-8")
    (rep / "consumed_inputs.json").write_text(json.dumps(consumed), encoding="utf-8")
    for artifact in ("COMMAND", "RELEASES", "OUTGRID"):
        (rep / artifact).write_text(f"{artifact}\n", encoding="utf-8")
    return rep


SHARED_TEST_EXPERIMENT_ID = "11111111-2222-4333-8444-555555555555"


def write_experiment(case_dir, profile, exe_sha=EXE_SHA_FOR_TESTS, reps=5, commit=None,
                     exp_id=SHARED_TEST_EXPERIMENT_ID):
    if commit is None:
        commit = json.loads((REPO / "reference/flexpart-11.1.json").read_text(encoding="utf-8"))["pinned_commit"]
    experiment = {
        "execution_profile": {"id": profile["id"], "version": profile["version"]},
        "experiment_id": exp_id,
        "case": case_dir.name,
        "classification": profile["repeatability_cases"][case_dir.name],
        "repetitions_requested": reps,
        "experiment_started_utc": "2026-01-01T00:00:00Z",
        "oracle_pinned_commit": commit,
        "oracle_executable_sha256": exe_sha,
        "docker_image": profile["docker"]["image"],
        "docker_image_id": "test-image",
        "compiler": "test-compiler",
        "make_arguments": profile["build"]["make_arguments"],
        "makefile_sha256": "test-makefile",
        "external_seed_control": "unavailable",
    }
    case_dir.mkdir(parents=True, exist_ok=True)
    (case_dir / "experiment.json").write_text(json.dumps(experiment), encoding="utf-8")
    return experiment


class FakeWorld:
    """Hermetic fixture world: no Docker, git, or real oracle checkout needed."""

    FIXTURE_FILES = {
        "COMMAND": "command\n",
        "RELEASES": "releases\n",
        "OUTGRID": "outgrid\n",
        "AGECLASSES": "ageclasses\n",
        "RECEPTORS": "receptors\n",
        "SPECIES/SPECIES_024": "species\n",
        "METEO_ARGS.txt": "args\n",
    }
    METEO_FILES = {"AVAILABLE": "available\n", "meteo.grb": b"meteo"}

    def __init__(self, root, cases=("ADV-ANA-001", "WIND-UNI-002")):
        self.root = Path(root)
        self.cases = cases
        reference = json.loads((REPO / "reference/flexpart-11.1.json").read_text(encoding="utf-8"))
        self.profile = reference["execution_profile"]
        self.manifest = self.root / "reference.json"
        self.manifest.write_text(json.dumps(reference), encoding="utf-8")
        self.runtime = {
            "execution_profile": {"id": self.profile["id"], "version": self.profile["version"]},
            "reference_manifest_sha256": cmp_mod.digest(self.manifest),
            "runtime_environment": self.profile["runtime_environment"],
        }
        self.exe = self.root / "fake-oracle-exe"
        self.exe.write_bytes(b"fake-oracle-executable")
        self.exe_sha = cmp_mod.digest(self.exe)
        self.checkout = self.root / "fake-checkout"
        (self.checkout / "src").mkdir(parents=True)
        (self.checkout / "src" / "makefile_gfortran").write_text("makefile\n", encoding="utf-8")
        self.fixtures = self.root / "fixtures"
        self.meteo = self.root / "meteo"
        for case in cases:
            case_fixtures = self.fixtures / case
            for rel, content in self.FIXTURE_FILES.items():
                path = case_fixtures / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
            case_meteo = self.meteo / case
            case_meteo.mkdir(parents=True)
            for rel, content in self.METEO_FILES.items():
                path = case_meteo / rel
                if isinstance(content, bytes):
                    path.write_bytes(content)
                else:
                    path.write_text(content, encoding="utf-8")
        self.repeat = self.root / "repeat"

    def consumed_inputs(self, case):
        options = {}
        for rel in ("COMMAND", "RELEASES", "OUTGRID", "AGECLASSES", "RECEPTORS", "SPECIES/SPECIES_024"):
            options[f"options/{rel}"] = cmp_mod.digest(self.fixtures / case / rel)
        meteo = {rel: cmp_mod.digest(self.meteo / case / rel)
                 for rel in ("AVAILABLE", "meteo.grb")}
        return {"options": options, "pathnames_sha256": "p" * 64,
                "pathnames_text": "text", "meteo": meteo}

    def make_case(self, case, summaries, raw_blobs=None, exe_sha=None, exp_id=None):
        case_dir = self.repeat / case
        write_experiment(case_dir, self.profile, exe_sha or self.exe_sha,
                         exp_id=exp_id or SHARED_TEST_EXPERIMENT_ID)
        consumed = self.consumed_inputs(case)
        raw_blobs = raw_blobs or [b"same"] * len(summaries)
        for i, (summary, blob) in enumerate(zip(summaries, raw_blobs), start=1):
            write_rep(case_dir, f"rep_{i:02d}", blob, summary, self.runtime, consumed,
                      exe_sha or self.exe_sha)
        return case_dir


class RepeatabilityReportTest(unittest.TestCase):
    def setUp(self):
        self._git_state = cmp_mod.git_state
        self._command = cmp_mod.command

    def tearDown(self):
        cmp_mod.git_state = self._git_state
        cmp_mod.command = self._command

    def patch_host(self, world):
        pinned = json.loads(world.manifest.read_text(encoding="utf-8"))["pinned_commit"]
        cmp_mod.git_state = lambda path: {"commit": pinned, "worktree_dirty": False}

        def fake_command(*args):
            if args[:3] == ("docker", "image", "inspect"):
                return "sha256:test-image-id"
            if args[:3] == ("docker", "run", "--rm"):
                return "GNU Fortran (test) 1.0"
            if args[:2] == ("docker", "--version"):
                return "Docker version test"
            return self._command(*args)
        cmp_mod.command = fake_command

    def run_comparator(self, world, cases=None):
        # Run the comparator in-process with Docker/git stubbed so the tests
        # prove the verification logic without needing an oracle container.
        out = world.root / "report.json"
        argv = ["compare_oracle_repeatability.py",
                "--repeat-dir", str(world.repeat), "--output", str(out),
                "--oracle-manifest", str(world.manifest),
                "--oracle-checkout", str(world.checkout),
                "--oracle-exe", str(world.exe),
                "--meteo-dir", str(world.meteo),
                "--fixtures-dir", str(world.fixtures)]
        if cases is not None:
            argv += ["--cases", " ".join(cases)]
        old_argv = sys.argv
        sys.argv = argv
        try:
            cmp_mod.main()
        except SystemExit as exc:
            code = exc.code
            message = code if isinstance(code, str) else ""
            return (0 if code in (None, 0) else 1), None, message
        finally:
            sys.argv = old_argv
        return 0, json.loads(out.read_text(encoding="utf-8")), ""

    def baseline_summary(self):
        return {"last_slice": {"weighted": {"airborne": 1.0}}, "center_of_mass": {"z_m": 50.0}}

    def test_byte_identical_repetitions_characterized_without_parity(self):
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory)
            self.patch_host(world)
            for case in world.cases:
                world.make_case(case, [self.baseline_summary()] * 5)
            code, report, message = self.run_comparator(world)
            self.assertEqual(code, 0, message)
            self.assertEqual(report["status"], "ORACLE_REPEATABILITY_CHARACTERIZED_NO_PARITY_VERDICT")
            self.assertEqual(report["external_seed_control"], "unavailable")
            self.assertEqual(report["oracle_executable_sha256"], world.exe_sha)
            for case in world.cases:
                entry = report["cases"][case]
                self.assertTrue(entry["all_byte_identical_to_baseline"])
                self.assertIn("bitwise repeatable", entry["repeatability_observation"])
                self.assertTrue(entry["consumed_inputs_identical_across_reps"])
                self.assertEqual(entry["executable_sha256"], world.exe_sha)
                self.assertIn("options/COMMAND", entry["consumed_input_hashes"]["options"])

    def test_differing_repetition_names_files_and_quantifies_numbers(self):
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory)
            self.patch_host(world)
            changed = {"last_slice": {"weighted": {"airborne": 1.1}}, "center_of_mass": {"z_m": 55.0}}
            for case in world.cases:
                world.make_case(case, [self.baseline_summary(), changed] + [self.baseline_summary()] * 3,
                                raw_blobs=[b"same", b"changed"] + [b"same"] * 3)
            code, report, message = self.run_comparator(world)
            self.assertEqual(code, 0, message)
            rep2 = next(r for r in report["cases"]["ADV-ANA-001"]["repetitions"] if r["rep"] == "rep_02")
            self.assertFalse(rep2["byte_identical_to_baseline"])
            self.assertIn("grid_conc_20240101010000", rep2["differing_raw_files"])
            fields = {d["field"]: d for d in rep2["differing_decoded_fields"]}
            self.assertIn("last_slice.weighted.airborne", fields)
            self.assertAlmostEqual(fields["last_slice.weighted.airborne"]["abs_diff"], 0.1)
            self.assertIsNotNone(rep2["decoded_numeric_summary"]["max_abs_diff"])

    def test_missing_repetition_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory)
            self.patch_host(world)
            for case in world.cases:
                world.make_case(case, [self.baseline_summary()])
            code, _, _ = self.run_comparator(world)
            self.assertNotEqual(code, 0)

    def test_stale_repetition_with_foreign_executable_fails_closed(self):
        # A leftover rep_* directory from an older experiment (different
        # executable record, no fresh experiment cover) must be rejected.
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory)
            self.patch_host(world)
            for case in world.cases:
                world.make_case(case, [self.baseline_summary()] * 5)
            stale = write_rep(world.repeat / "ADV-ANA-001", "rep_06", b"same",
                              self.baseline_summary(), world.runtime,
                              world.consumed_inputs("ADV-ANA-001"), exe_sha="f" * 64)
            self.assertTrue(stale.is_dir())
            code, _, message = self.run_comparator(world)
            self.assertNotEqual(code, 0)
            self.assertIn("stale", message)

    def test_changed_consumed_inputs_fail_closed(self):
        # One repetition consuming different inputs (e.g. regenerated
        # meteorology or edited COMMAND) voids the repeatability claim.
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory)
            self.patch_host(world)
            for case in world.cases:
                world.make_case(case, [self.baseline_summary()] * 5)
            rep2 = world.repeat / "WIND-UNI-002" / "rep_02" / "consumed_inputs.json"
            consumed = json.loads(rep2.read_text(encoding="utf-8"))
            consumed["meteo"]["meteo.grb"] = "0" * 64
            rep2.write_text(json.dumps(consumed), encoding="utf-8")
            code, _, message = self.run_comparator(world)
            self.assertNotEqual(code, 0)
            self.assertIn("differ", message)

    def test_mixed_experiment_ids_fail_closed(self):
        # Two otherwise valid cases sharing one executable hash but produced
        # by separate invocations must never be combined into one report, let
        # alone labeled canonical.
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory)
            self.patch_host(world)
            world.make_case("ADV-ANA-001", [self.baseline_summary()] * 5,
                            exp_id="aaaaaaaa-0000-4000-8000-000000000001")
            world.make_case("WIND-UNI-002", [self.baseline_summary()] * 5,
                            exp_id="bbbbbbbb-0000-4000-8000-000000000002")
            code, report, message = self.run_comparator(world)
            self.assertNotEqual(code, 0)
            self.assertIsNone(report)
            self.assertIn("different experiments", message)

    def test_shared_experiment_id_two_case_run_succeeds(self):
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory)
            self.patch_host(world)
            for case in world.cases:
                world.make_case(case, [self.baseline_summary()] * 5,
                                exp_id="aaaaaaaa-0000-4000-8000-000000000001")
            code, report, message = self.run_comparator(world)
            self.assertEqual(code, 0, message)
            self.assertEqual(report["experiment_id"], "aaaaaaaa-0000-4000-8000-000000000001")
            self.assertEqual(report["cases_evaluated"], ["ADV-ANA-001", "WIND-UNI-002"])

    def test_missing_experiment_id_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory)
            self.patch_host(world)
            for case in world.cases:
                world.make_case(case, [self.baseline_summary()] * 5)
            experiment_path = world.repeat / "ADV-ANA-001" / "experiment.json"
            experiment = json.loads(experiment_path.read_text(encoding="utf-8"))
            del experiment["experiment_id"]
            experiment_path.write_text(json.dumps(experiment), encoding="utf-8")
            code, _, message = self.run_comparator(world)
            self.assertNotEqual(code, 0)
            self.assertIn("experiment ID", message)

    def test_missing_experiment_record_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory)
            self.patch_host(world)
            for case in world.cases:
                world.make_case(case, [self.baseline_summary()] * 5)
            (world.repeat / "ADV-ANA-001" / "experiment.json").unlink()
            code, _, message = self.run_comparator(world)
            self.assertNotEqual(code, 0)
            self.assertIn("experiment.json", message)

    def test_single_case_scope_produces_valid_single_case_report(self):
        # The documented single-case command must yield a valid report for
        # that case instead of failing on (or mixing in) the other case.
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory, cases=("ADV-ANA-001",))
            self.patch_host(world)
            world.make_case("ADV-ANA-001", [self.baseline_summary()] * 5)
            code, report, message = self.run_comparator(world, cases=["ADV-ANA-001"])
            self.assertEqual(code, 0, message)
            self.assertEqual(sorted(report["cases"]), ["ADV-ANA-001"])
            self.assertEqual(report["cases_evaluated"], ["ADV-ANA-001"])
            self.assertEqual(report["experiment_id"], SHARED_TEST_EXPERIMENT_ID)
            self.assertTrue(report["cases"]["ADV-ANA-001"]["all_byte_identical_to_baseline"])

    def test_unscoped_run_ignores_no_case_silently(self):
        # Without --cases, a missing profile case fails with a clear error
        # rather than producing a silently partial report.
        with tempfile.TemporaryDirectory() as directory:
            world = FakeWorld(directory, cases=("ADV-ANA-001",))
            self.patch_host(world)
            world.make_case("ADV-ANA-001", [self.baseline_summary()] * 5)
            code, _, message = self.run_comparator(world)
            self.assertNotEqual(code, 0)
            self.assertIn("WIND-UNI-002", message)


if __name__ == "__main__":
    unittest.main()
