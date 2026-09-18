#!/usr/bin/env python3
"""Fail-closed checks for the oracle seed-identity comparator (issue #50).

Hermetic tests (no Docker, git, or FLEXPART needed): host calls are stubbed
and the verification logic runs in-process. Follows the pattern of the #49
``test_compare_oracle_repeatability.py`` suite.

Run: python scripts/corpus/test_compare_oracle_seed_identities.py
"""

import json
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts" / "corpus"))

import compare_oracle_seed_identities as cmp_mod  # noqa: E402
from oracle_stochastic_identity import build_identity_record  # noqa: E402

CASE = "WIND-UNI-002"
PRESCRIBED = [str(s) for s in range(1, 11)]
PROFILE = {"id": "flexpart-11.1-single-thread", "version": 1}


def write_rep(parent, name, raw_bytes, summary, runtime, consumed, exe_sha,
              identity_record=None):
    rep = parent / name
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
    if identity_record is not None:
        (rep / "stochastic_identity.json").write_text(
            json.dumps(identity_record), encoding="utf-8")
    for artifact in ("COMMAND", "RELEASES", "OUTGRID"):
        (rep / artifact).write_text(f"{artifact}\n", encoding="utf-8")
    return rep


class FakeWorld:
    """Hermetic fixture world: Docker and git are stubbed."""

    FIXTURE_FILES = {
        "COMMAND": "command\n",
        "RELEASES": "releases\n",
        "OUTGRID": "outgrid\n",
        "AGECLASSES": "ageclasses\n",
        "RECEPTORS": "receptors\n",
    }
    METEO_FILES = {"AVAILABLE": "available\n", "meteo.grb": b"meteo"}

    def __init__(self, root):
        self.root = Path(root)
        reference = json.loads((REPO / "reference/flexpart-11.1.json").read_text(
            encoding="utf-8"))
        self.profile = reference["execution_profile"]
        self.pinned = reference["pinned_commit"]
        self.manifest = self.root / "reference.json"
        self.manifest.write_text(json.dumps(reference), encoding="utf-8")
        self.runtime = {
            "execution_profile": {"id": self.profile["id"], "version": self.profile["version"]},
            "reference_manifest_sha256": cmp_mod.digest(self.manifest),
            "runtime_environment": self.profile["runtime_environment"],
        }
        self.contract = json.loads(
            (REPO / "reference/oracle-stochastic-identity.json").read_text(encoding="utf-8"))
        self.patch_sha = self.contract["validation_patch"]["sha256"]
        self.pristine_exe = self.root / "pristine-exe"
        self.pristine_exe.write_bytes(b"fake-pristine-executable")
        self.pristine_sha = cmp_mod.digest(self.pristine_exe)
        self.seedable_exe = self.root / "seedable-exe"
        self.seedable_exe.write_bytes(b"fake-seedable-executable")
        self.seedable_sha = cmp_mod.digest(self.seedable_exe)
        self.pristine_checkout = self.root / "pristine-checkout" / "src"
        self.seedable_checkout = self.root / "seedable-checkout" / "src"
        for checkout, marker in ((self.pristine_checkout, False),
                                 (self.seedable_checkout, True)):
            checkout.mkdir(parents=True)
            hook = "validation_seed_offset" if marker else "pristine"
            (checkout / "random_mod.f90").write_text(f"! {hook}\n", encoding="utf-8")
            (checkout / "FLEXPART.f90").write_text(f"! {hook}\n", encoding="utf-8")
        (self.pristine_checkout / "makefile_gfortran").write_text("makefile\n",
                                                                  encoding="utf-8")
        self.fixtures = self.root / "fixtures"
        self.meteo = self.root / "meteo"
        case_fixtures = self.fixtures / CASE
        for rel, content in self.FIXTURE_FILES.items():
            path = case_fixtures / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        case_meteo = self.meteo / CASE
        case_meteo.mkdir(parents=True)
        for rel, content in self.METEO_FILES.items():
            path = case_meteo / rel
            if isinstance(content, bytes):
                path.write_bytes(content)
            else:
                path.write_text(content, encoding="utf-8")
        self.seedable_dir = self.root / "seedable"
        self.pristine_repeat = self.root / "pristine-repeat"

    def consumed_inputs(self):
        options = {f"options/{rel}": cmp_mod.digest(self.fixtures / CASE / rel)
                   for rel in ("COMMAND", "RELEASES", "OUTGRID", "AGECLASSES", "RECEPTORS")}
        meteo = {rel: cmp_mod.digest(self.meteo / CASE / rel)
                 for rel in ("AVAILABLE", "meteo.grb")}
        return {"options": options, "meteo": meteo}

    def identity_record(self, env_value, kind="seedable-validation-oracle"):
        exe = self.seedable_sha if kind == "seedable-validation-oracle" else self.pristine_sha
        return build_identity_record(
            requested_env_value=env_value, oracle_kind=kind,
            executable_sha256=exe,
            patch_sha256=self.patch_sha if kind == "seedable-validation-oracle" else None,
            case=CASE, execution_profile=dict(PROFILE))

    def summary(self, tag):
        return {"last_slice": {"weighted": {"airborne": 1.0}}, "tag": tag}

    def make_pristine_baseline(self, raw=b"pristine-bytes"):
        case_dir = self.pristine_repeat / CASE
        case_dir.mkdir(parents=True, exist_ok=True)
        (case_dir / "experiment.json").write_text(json.dumps({
            "execution_profile": dict(PROFILE),
            "experiment_id": "aaaaaaaa-0000-4000-8000-000000000000",
            "case": CASE,
            "oracle_executable_sha256": self.pristine_sha,
        }), encoding="utf-8")
        write_rep(case_dir, "rep_01", raw, self.summary("baseline"),
                  self.runtime, self.consumed_inputs(), self.pristine_sha)
        return raw

    def make_seedable_experiment(self, repetitions=None):
        if repetitions is None:
            repetitions = {"default": 1, **{s: 1 for s in PRESCRIBED}}
            repetitions["3"] = 5
        self.seedable_dir.mkdir(parents=True, exist_ok=True)
        (self.seedable_dir / "experiment.json").write_text(json.dumps({
            "execution_profile": dict(PROFILE),
            "experiment_id": "bbbbbbbb-0000-4000-8000-000000000000",
            "case": CASE,
            "oracle_kind": "seedable-validation-oracle",
            "patch_sha256": self.patch_sha,
            "oracle_pinned_commit": self.pinned,
            "oracle_executable_sha256": self.seedable_sha,
            "pristine_executable_sha256": self.pristine_sha,
            "seed_enforcement_smoke": {
                "all_rejected": True,
                "attempts": [
                    {"env_value": "0", "rejected": True},
                    {"env_value": "1000000001", "rejected": True},
                ],
            },
            "repetitions": repetitions,
        }), encoding="utf-8")
        return repetitions

    def make_identity(self, label, env_value, raw_by_rep, summary_by_rep=None):
        import shutil
        identity_dir = self.seedable_dir / label
        if identity_dir.exists():
            shutil.rmtree(identity_dir)
        consumed = self.consumed_inputs()
        for index, raw in enumerate(raw_by_rep, start=1):
            summary = (summary_by_rep[index - 1] if summary_by_rep
                       else self.summary(f"{label}-{index}"))
            write_rep(identity_dir, f"rep_{index:02d}", raw, summary,
                      self.runtime, consumed, self.seedable_sha,
                      self.identity_record(env_value))


class SeedIdentityReportTest(unittest.TestCase):
    def setUp(self):
        self._git_state = cmp_mod.git_state
        self._command = cmp_mod.command

    def tearDown(self):
        cmp_mod.git_state = self._git_state
        cmp_mod.command = self._command

    def patch_host(self, world):
        cmp_mod.git_state = lambda path: {"commit": world.pinned, "worktree_dirty": False}

        def fake_command(*args):
            if args[:3] == ("docker", "image", "inspect"):
                return "sha256:test-image-id"
            if args[:3] == ("docker", "run", "--rm"):
                return "GNU Fortran (test) 1.0"
            if "diff" in args and "--name-only" in args:
                return "src/FLEXPART.f90\nsrc/random_mod.f90"
            return self._command(*args)
        cmp_mod.command = fake_command

    def run_comparator(self, world):
        out = world.root / "report.json"
        argv = ["compare_oracle_seed_identities.py",
                "--seedable-dir", str(world.seedable_dir),
                "--pristine-repeat-dir", str(world.pristine_repeat),
                "--output", str(out),
                "--oracle-manifest", str(world.manifest),
                "--identity-contract", str(REPO / "reference/oracle-stochastic-identity.json"),
                "--pristine-checkout", str(world.pristine_checkout.parent),
                "--pristine-exe", str(world.pristine_exe),
                "--seedable-checkout", str(world.seedable_checkout.parent),
                "--seedable-exe", str(world.seedable_exe),
                "--meteo-dir", str(world.meteo),
                "--fixtures-dir", str(world.fixtures),
                "--repo-root", str(REPO)]
        old_argv = sys.argv
        sys.argv = argv
        try:
            cmp_mod.main()
        except SystemExit as exc:
            code = exc.code
            message = code if isinstance(code, str) else ""
            # Read report if it was written before exit
            report = None
            if out.is_file():
                report = json.loads(out.read_text(encoding="utf-8"))
            return (0 if code in (None, 0) else 1), report, message
        finally:
            sys.argv = old_argv
        return 0, json.loads(out.read_text(encoding="utf-8")), ""

    def make_full_world(self, root):
        world = FakeWorld(root)
        self.patch_host(world)
        baseline_raw = world.make_pristine_baseline()
        world.make_seedable_experiment()
        world.make_identity("default", None, [baseline_raw],
                            [world.summary("baseline")])
        for label in PRESCRIBED:
            if label == "3":
                world.make_identity(label, label, [b"seed-3-repeat"] * 5,
                                    [world.summary("3-repeat")] * 5)
            else:
                world.make_identity(label, label, [f"seed-{label}-0".encode()])
        return world

    def test_full_evidence_report_characterized_without_parity(self):
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            code, report, message = self.run_comparator(world)
            self.assertEqual(code, 0, message)
            self.assertEqual(
                report["status"],
                "ORACLE_STOCHASTIC_IDENTITY_CHARACTERIZED_NO_PARITY_VERDICT")
            self.assertTrue(report["default_equivalence"]["raw_byte_identical"])
            self.assertTrue(report["default_equivalence"]["decoded_hash_identical"])
            self.assertTrue(report["distinct_identities"]["pairwise_distinct"])
            self.assertTrue(
                report["same_seed_repeatability"]["all_byte_identical_to_baseline"])
            self.assertEqual(report["seedable_executable_sha256"], world.seedable_sha)
            self.assertEqual(report["pristine_executable_sha256"], world.pristine_sha)
            self.assertNotEqual(world.seedable_sha, world.pristine_sha)

    def test_missing_identity_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            import shutil
            world = self.make_full_world(directory)
            shutil.rmtree(world.seedable_dir / "7")
            code, _, message = self.run_comparator(world)
            self.assertEqual(code, 1)
            self.assertIn("7", message)

    def test_colliding_identities_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            code, _, _ = self.run_comparator(world)
            self.assertEqual(code, 0)
            # Rewrite identity 8 with identity 7's bytes: the mechanism failed.
            world.make_identity("8", "8", [b"seed-7-0"])
            (world.seedable_dir / "8" / "rep_01" / "oracle_summary.json").write_text(
                json.dumps(world.summary("7-1")), encoding="utf-8")
            code, _, message = self.run_comparator(world)
            self.assertEqual(code, 1)
            self.assertIn("distinct", message)

    def test_default_difference_fails_closed(self):
        """Any default-mode difference (raw or decoded) must fail the run."""
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            # Different raw bytes, same summary structure -> raw differs
            world.make_identity("default", None, [b"other-bytes"],
                                [world.summary("other")])
            code, report, message = self.run_comparator(world)
            self.assertEqual(code, 1, message)
            self.assertIn("differs", message.lower())

    def test_default_raw_only_difference_fails_closed(self):
        """Raw difference with identical decoded summary must fail."""
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            # Same decoded summary content but different raw bytes
            # (simulating e.g. timestamp differences in header that don't affect decoded)
            world.make_identity("default", None, [b"other-raw-bytes"],
                                [world.summary("baseline")])  # same tag -> same summary
            code, report, message = self.run_comparator(world)
            self.assertEqual(code, 1, message)
            self.assertIn("differs", message.lower())
            self.assertIsNotNone(report)
            self.assertEqual(report["status"], "SEEDABLE_DEFAULT_EQUIVALENCE_FAILED")
            self.assertFalse(report["default_equivalence"]["raw_byte_identical"])
            self.assertTrue(report["default_equivalence"]["decoded_hash_identical"])

    def test_default_decoded_only_difference_fails_closed(self):
        """Decoded summary difference with identical raw bytes must fail."""
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            # Same raw bytes but different decoded summary
            world.make_identity("default", None, [b"pristine-bytes"],
                                [world.summary("different-decoded")])
            code, report, message = self.run_comparator(world)
            self.assertEqual(code, 1, message)
            self.assertIn("differs", message.lower())
            self.assertIsNotNone(report)
            self.assertEqual(report["status"], "SEEDABLE_DEFAULT_EQUIVALENCE_FAILED")
            self.assertTrue(report["default_equivalence"]["raw_byte_identical"])
            self.assertFalse(report["default_equivalence"]["decoded_hash_identical"])

    def test_unrepeatable_seed_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            world.make_identity("3", "3", [f"seed-3-{i}".encode() for i in range(5)])
            code, _, message = self.run_comparator(world)
            self.assertEqual(code, 1)
            self.assertIn("repeatability", message)

    def test_mislabeled_identity_record_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            bad = world.identity_record("4")
            bad["oracle_kind"] = "pristine-oracle"
            (world.seedable_dir / "4" / "rep_01" / "stochastic_identity.json").write_text(
                json.dumps(bad), encoding="utf-8")
            code, _, message = self.run_comparator(world)
            self.assertEqual(code, 1)
            self.assertIn("pristine", message)

    def test_swapped_identity_fails_closed(self):
        """Identity record in directory 7 claiming identity 8 must fail."""
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            # Overwrite directory 7's identity record to claim identity 8
            bad = world.identity_record("8")  # env_value="8", requested_identity=8
            (world.seedable_dir / "7" / "rep_01" / "stochastic_identity.json").write_text(
                json.dumps(bad), encoding="utf-8")
            code, _, message = self.run_comparator(world)
            self.assertEqual(code, 1, message)
            self.assertIn("does not match", message)
            self.assertIn("7", message)

    def test_mislabeled_default_repetition_fails_closed(self):
        """Default directory with non-default identity record must fail."""
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            # Overwrite default's identity record to claim identity 1
            bad = world.identity_record("1")  # env_value="1", requested_identity=1
            (world.seedable_dir / "default" / "rep_01" / "stochastic_identity.json").write_text(
                json.dumps(bad), encoding="utf-8")
            code, _, message = self.run_comparator(world)
            self.assertEqual(code, 1, message)
            self.assertIn("non-default", message)
            self.assertIn("default", message)

    def test_unknown_build_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            bad = world.identity_record("4")
            bad["executable_sha256"] = "f" * 64
            (world.seedable_dir / "4" / "rep_01" / "stochastic_identity.json").write_text(
                json.dumps(bad), encoding="utf-8")
            code, _, message = self.run_comparator(world)
            self.assertEqual(code, 1)
            self.assertIn("unknown", message)

    def test_missing_experiment_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            (world.seedable_dir / "experiment.json").unlink()
            code, _, message = self.run_comparator(world)
            self.assertEqual(code, 1)
            self.assertIn("experiment", message)

    def test_missing_pristine_baseline_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            (world.pristine_repeat / CASE / "rep_01" / "raw" / "header").unlink()
            code, _, message = self.run_comparator(world)
            self.assertEqual(code, 1)

    def test_profile_mismatch_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            world = self.make_full_world(directory)
            runtime = json.loads(
                (world.seedable_dir / "4" / "rep_01" / "runtime_profile.json").read_text(
                    encoding="utf-8"))
            runtime["runtime_environment"] = dict(runtime["runtime_environment"])
            runtime["runtime_environment"]["OMP_NUM_THREADS"] = "2"
            (world.seedable_dir / "4" / "rep_01" / "runtime_profile.json").write_text(
                json.dumps(runtime), encoding="utf-8")
            code, _, message = self.run_comparator(world)
            self.assertEqual(code, 1)
            self.assertIn("profile", message)


if __name__ == "__main__":
    unittest.main()
