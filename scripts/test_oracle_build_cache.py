"""Fail-closed build reuse checks; these tests never substitute scientific evidence."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

import oracle_build_cache as cache
import test_agent_validation


class VerifiedCacheTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.project, self.oracle = test_agent_validation.OracleBuildCacheTest().create_inputs(self.root)
        self.executable = self.oracle / "src/FLEXPART"
        self.executable.write_bytes(b"executable")
        self.metadata = self.root / "build.json"
        self.log = self.root / "build.log"
        self.log.write_text("complete build log")
        self.head = mock.patch.object(cache, "git_head", return_value="a" * 40)
        self.head.start()
        self.addCleanup(self.head.stop)
        self.identity = cache.cache_identity(self.project, self.oracle)
        self.record()

    def record(self):
        cache.atomic_json(self.metadata, {
            "schema": cache.SCHEMA, "identity": self.identity,
            "docker_image_id": "sha256:image", "artifacts_sha256": cache.build_artifacts(self.executable),
            "oracle_executable_sha256": cache.sha256(self.executable),
            "build_log_sha256": cache.sha256(self.log),
        })

    def valid(self):
        return cache.validate(self.metadata, self.identity, "sha256:image", self.executable)

    def test_each_retained_artifact_is_required_and_content_checked(self):
        for path in (self.executable, self.oracle / "src/reference.o",
                     self.oracle / "src/reference.mod", self.log, self.metadata):
            original = path.read_bytes()
            for mutation in ("corrupt", "missing"):
                with self.subTest(path=path.name, mutation=mutation):
                    path.write_bytes(b"corrupt") if mutation == "corrupt" else path.unlink()
                    self.assertFalse(self.valid())
                    path.write_bytes(original)
                    self.assertTrue(self.valid())

    def test_each_source_recipe_pin_and_flag_invalidates_identity(self):
        paths = [self.project / "docker/Dockerfile.fortran",
                 self.project / "docker/docker-compose.fortran.yml",
                 self.project / "reference/flexpart-11.1.json",
                 self.oracle / "src/makefile_gfortran", self.oracle / "src/physics.f90"]
        paths[-1].write_text("original source")
        original_identity = cache.cache_identity(self.project, self.oracle)
        for path in paths:
            with self.subTest(path=path.name):
                original = path.read_bytes()
                path.write_bytes(original + b"changed")
                self.assertNotEqual(original_identity, cache.cache_identity(self.project, self.oracle))
                path.write_bytes(original)
        with mock.patch.object(cache, "MAKE_ARGUMENTS", "changed flags"):
            self.assertNotEqual(original_identity, cache.cache_identity(self.project, self.oracle))
        for field in ("oracle_commit", "toolchain_sha256"):
            changed = dict(self.identity, **{field: "changed"})
            self.assertFalse(cache.validate(self.metadata, changed, "sha256:image", self.executable))

    def test_interrupted_publication_preserves_previous_record(self):
        original = self.metadata.read_bytes()
        with mock.patch.object(cache.os, "replace", side_effect=OSError("interrupted")):
            with self.assertRaises(OSError):
                cache.atomic_json(self.metadata, {"incomplete": True})
        self.assertEqual(original, self.metadata.read_bytes())
        self.assertEqual([self.metadata], list(self.root.glob("build.json*")))

    def test_direct_driver_reuses_only_complete_validated_builds(self):
        driver = self.root / "driver.f90"
        driver.write_text("source")
        binary = self.root / "driver"
        metadata = self.root / "driver-build.json"
        command = [sys.executable, "-c", "from pathlib import Path; "
                   f"Path({str(binary)!r}).write_bytes(b'compiled')"]
        arguments = dict(parent=self.metadata, project=self.project, checkout=self.oracle)
        with mock.patch.object(cache, "require_pristine"), contextlib.redirect_stdout(io.StringIO()):
            cache.cached_command(metadata, [driver], [binary], command, **arguments)
            with mock.patch.object(cache.subprocess, "run", side_effect=AssertionError("rebuilt on hit")):
                cache.cached_command(metadata, [driver], [binary], command, **arguments)
            for path in (driver, binary, metadata.with_suffix(".log")):
                with self.subTest(path=path.name):
                    path.write_bytes(b"changed")
                    cache.cached_command(metadata, [driver], [binary], command, **arguments)
                    self.assertEqual(b"compiled", binary.read_bytes())
            binary.unlink()
            cache.cached_command(metadata, [driver], [binary], command, **arguments)
            (self.oracle / "src/reference.o").write_bytes(b"corrupt parent")
            with self.assertRaises(ValueError):
                cache.cached_command(metadata, [driver], [binary], command, **arguments)

    def test_failed_direct_build_does_not_publish_success(self):
        driver = self.root / "driver.f90"
        driver.write_text("source")
        metadata = self.root / "driver-build.json"
        with mock.patch.object(cache, "require_pristine"):
            with self.assertRaises(cache.subprocess.CalledProcessError):
                cache.cached_command(metadata, [driver], [self.root / "missing"],
                                     [sys.executable, "-c", "raise SystemExit(1)"],
                                     parent=self.metadata, project=self.project, checkout=self.oracle)
        self.assertFalse(metadata.exists())

    def test_dirty_or_unpinned_checkout_is_rejected_before_reuse(self):
        (self.project / "reference/flexpart-11.1.json").write_text(
            json.dumps({"pinned_commit": "a" * 40}))
        with mock.patch.object(cache.subprocess, "check_output", return_value=" M src/physics.f90"):
            with self.assertRaises(ValueError):
                cache.require_pristine(self.oracle, self.project)
        with mock.patch.object(cache.subprocess, "check_output", return_value=""):
            cache.require_pristine(self.oracle, self.project)
            with mock.patch.object(cache, "git_head", return_value="b" * 40):
                with self.assertRaises(ValueError):
                    cache.require_pristine(self.oracle, self.project)


class CargoCachePolicyTest(unittest.TestCase):
    def test_only_compilation_artifacts_are_cached_and_prs_cannot_publish(self):
        root = Path(__file__).resolve().parents[1]
        for name, prefix in (("software-wgpu.yml", ""), ("validation-gate.yml", "flexpart-gpu/")):
            text = (root / ".github/workflows" / name).read_text()
            block = text.split("      - name: Restore trusted Cargo compilation cache\n")[1].split("      - ")[0]
            self.assertIn("cache-targets: \"false\"", block)
            self.assertIn("cache-workspace-crates: \"false\"", block)
            self.assertIn("save-if: ${{ github.event_name != 'pull_request' }}", block)
            directories = block.split("          cache-directories: |\n")[1].split("          cache-workspace-crates:")[0]
            self.assertEqual(set(directories.split()), {
                prefix + "target/" + profile + "/" + folder
                for profile in ("debug", "release") for folder in ("deps", "build", ".fingerprint")} | {prefix + ".cargo-cache-integrity"})


if __name__ == "__main__":
    unittest.main()
