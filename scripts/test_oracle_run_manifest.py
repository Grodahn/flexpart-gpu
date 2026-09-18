"""Fail-closed checks for the oracle run provenance writer."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from write_oracle_run_manifest import adapter_line, artifacts, digest, validate_runtime_profile


class OracleRunManifestTest(unittest.TestCase):
    def runtime_contract(self):
        path = Path(__file__).resolve().parents[1] / "reference/flexpart-11.1.json"
        profile = json.loads(path.read_text(encoding="utf-8"))["execution_profile"]
        return path, profile["runtime_environment"].copy()

    def test_runtime_profile_accepts_canonical_settings(self):
        path, environment = self.runtime_contract()
        report = validate_runtime_profile(path, environment)
        self.assertEqual(report["status"], "RUNTIME_SETTINGS_VERIFIED_ONLY")
        self.assertEqual(report["runtime_environment"], environment)
        self.assertEqual(report["reference_manifest_sha256"], digest(path))

    def test_runtime_profile_rejects_every_missing_or_changed_setting(self):
        path, canonical = self.runtime_contract()
        for key in canonical:
            for replacement in (None, "changed"):
                with self.subTest(setting=key, replacement=replacement):
                    environment = canonical.copy()
                    if replacement is None:
                        del environment[key]
                    else:
                        environment[key] = replacement
                    with self.assertRaisesRegex(ValueError, key):
                        validate_runtime_profile(path, environment)

    def test_runtime_profile_rejects_uncontracted_runtime_overrides(self):
        path, environment = self.runtime_contract()
        environment["GOMP_CPU_AFFINITY"] = "0"
        with self.assertRaisesRegex(ValueError, "uncontracted"):
            validate_runtime_profile(path, environment)

    def test_runtime_profile_cli_rejects_changed_threads(self):
        path, canonical = self.runtime_contract()
        environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("OMP_", "GOMP_", "KMP_"))
        }
        environment.update(canonical)
        cli = [sys.executable, str(Path(__file__).with_name("write_oracle_run_manifest.py")),
               "check-runtime-profile", "--oracle-manifest", str(path)]
        accepted = subprocess.run(cli, env=environment, capture_output=True, text=True)
        self.assertEqual(accepted.returncode, 0, accepted.stderr)
        environment["OMP_NUM_THREADS"] = "2"
        rejected = subprocess.run(cli, env=environment, capture_output=True, text=True)
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("OMP_NUM_THREADS", rejected.stderr)
        self.assertEqual(rejected.stdout, "")

    def test_runtime_profile_rejects_incomplete_contract(self):
        path, _ = self.runtime_contract()
        reference = json.loads(path.read_text(encoding="utf-8"))
        del reference["execution_profile"]["runtime_environment"]["OMP_DYNAMIC"]
        with tempfile.TemporaryDirectory() as directory:
            incomplete = Path(directory) / "reference.json"
            incomplete.write_text(json.dumps(reference), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "incomplete"):
                validate_runtime_profile(incomplete, {})

    def test_artifacts_hash_each_file_in_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a").write_bytes(b"first")
            subdir = root / "sub"
            subdir.mkdir()
            (subdir / "b").write_bytes(b"second")
            result = artifacts([root])
            self.assertEqual(result[str((root / "a").resolve())], digest(root / "a"))
            self.assertEqual(len(result), 2)

    def test_missing_artifact_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "missing or empty"):
                artifacts([Path(directory) / "missing"])

    def test_adapter_record_must_be_unambiguous(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "candidate.log"
            path.write_text("[INFO] wgpu adapter: WARP (Dx12, Cpu)\n")
            self.assertIn("WARP", adapter_line(path))
            path.write_text("no adapter\n")
            with self.assertRaisesRegex(ValueError, "unambiguous"):
                adapter_line(path)


if __name__ == "__main__":
    unittest.main()
