"""Fail-closed checks for the oracle run provenance writer."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from write_oracle_run_manifest import (
    adapter_line,
    artifacts,
    digest,
    meteorology_contract_identity,
    meteorology_input_provenance,
    validate_runtime_profile,
)


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

    def test_meteorology_contract_identity_reads_checked_in_schema(self):
        checkout = Path(__file__).resolve().parents[1]
        identity = meteorology_contract_identity(checkout)
        self.assertEqual(identity["schema_id"], "flexpart-gpu.canonical-meteorology")
        self.assertEqual(identity["schema_version"], 1)
        source = checkout / "fixtures/meteorology/synthetic-v1.json"
        self.assertEqual(identity["identity_source_sha256"], digest(source))

    def test_meteorology_contract_identity_fails_closed_when_missing_or_malformed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "fixtures/meteorology/synthetic-v1.json"
            with self.assertRaisesRegex(ValueError, "missing"):
                meteorology_contract_identity(root)

            fixture.parent.mkdir(parents=True)
            fixture.write_text("{not json", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "malformed"):
                meteorology_contract_identity(root)

            fixture.write_text(json.dumps({"schema": {"id": "", "version": 0}}),
                               encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "id"):
                meteorology_contract_identity(root)

    def test_meteorology_input_provenance_distinguishes_runtime_binding(self):
        checkout = Path(__file__).resolve().parents[1]
        contract = meteorology_contract_identity(checkout)

        unbound = meteorology_input_provenance([], contract)
        self.assertEqual(unbound["status"], "NOT_BOUND_TO_RUN")
        self.assertEqual(unbound["inputs"], {})

        source = checkout / "fixtures/meteorology/synthetic-v1.json"
        bound = meteorology_input_provenance([source], contract)
        self.assertEqual(bound["status"], "BOUND_TO_CANONICAL_INPUTS")
        record = bound["inputs"][str(source.resolve())]
        self.assertEqual(record["sha256"], digest(source))
        self.assertEqual(record["schema_id"], contract["schema_id"])
        self.assertEqual(record["schema_version"], contract["schema_version"])

    def test_meteorology_input_provenance_rejects_schema_mismatch(self):
        contract = {
            "schema_id": "flexpart-gpu.canonical-meteorology",
            "schema_version": 1,
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "snapshot.json"
            path.write_text(json.dumps({
                "schema": {
                    "id": "flexpart-gpu.canonical-meteorology",
                    "version": 2,
                }
            }), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "schema mismatch"):
                meteorology_input_provenance([path], contract)

    def test_meteorology_input_provenance_rejects_missing_or_malformed_input(self):
        contract = {
            "schema_id": "flexpart-gpu.canonical-meteorology",
            "schema_version": 1,
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            missing = root / "missing.json"
            with self.assertRaisesRegex(ValueError, "missing"):
                meteorology_input_provenance([missing], contract)

            malformed = root / "malformed.json"
            malformed.write_text("{broken", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "malformed"):
                meteorology_input_provenance([malformed], contract)

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
