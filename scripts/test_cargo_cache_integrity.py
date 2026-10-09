"""Reject damaged compiler caches while retaining fresh scientific output trees."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import cargo_cache_integrity as integrity
from oracle_build_cache import sha256


class CargoIntegrityTest(unittest.TestCase):
    def test_valid_cache_is_retained_and_corruption_requires_rebuild(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            library = root / "target/debug/deps/libdependency-abc.rlib"
            library.parent.mkdir(parents=True)
            library.write_bytes(b"trusted dependency")
            report = root / "target/ci-gate/evidence.json"
            report.parent.mkdir()
            report.write_text("fresh result outside cache")
            manifest = root / integrity.MANIFEST
            manifest.parent.mkdir()
            manifest.write_text(json.dumps({"schema": "flexpart-gpu.cargo-cache-integrity.v1",
                                           "files": {library.relative_to(root).as_posix(): sha256(library)}}))
            metadata = {"workspace_members": ["candidate"], "packages": [
                {"id": "candidate", "name": "candidate", "targets": []},
                {"id": "dependency", "name": "dependency", "targets": [{"name": "dependency", "kind": ["lib"]}]}]}
            with mock.patch.object(integrity.subprocess, "check_output", return_value=json.dumps(metadata)):
                self.assertTrue(integrity.verify(root))
                library.write_bytes(b"corrupt")
                self.assertFalse(integrity.verify(root))
            self.assertFalse(library.exists())
            self.assertFalse(manifest.exists())
            self.assertEqual("fresh result outside cache", report.read_text())

    def test_missing_or_malformed_manifest_discards_partial_build_cache(self):
        for content in (None, "{", "null", '{"schema":"old","files":{}}'):
            with self.subTest(content=content), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                artifact = root / "target/release/build/partial"
                artifact.parent.mkdir(parents=True)
                artifact.write_text("incomplete")
                if content is not None:
                    manifest = root / integrity.MANIFEST
                    manifest.parent.mkdir(parents=True)
                    manifest.write_text(content)
                self.assertFalse(integrity.verify(root))
                self.assertFalse(artifact.exists())


if __name__ == "__main__":
    unittest.main()
