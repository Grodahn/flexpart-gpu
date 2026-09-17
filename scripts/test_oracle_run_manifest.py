"""Fail-closed checks for the oracle run provenance writer."""

import tempfile
import unittest
from pathlib import Path

from write_oracle_run_manifest import adapter_line, artifacts, digest


class OracleRunManifestTest(unittest.TestCase):
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
