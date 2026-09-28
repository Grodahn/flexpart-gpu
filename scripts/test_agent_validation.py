"""Regression tests for the compact validation and oracle-cache interface."""

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import agent_validation
import oracle_build_cache


class OracleBuildCacheTest(unittest.TestCase):
    def create_inputs(self, root: Path) -> tuple[Path, Path]:
        project = root / "project"
        oracle = root / "oracle"
        (project / "docker").mkdir(parents=True)
        (project / "reference").mkdir()
        (oracle / "src").mkdir(parents=True)
        (project / "docker" / "Dockerfile.fortran").write_text("FROM pinned\n")
        (project / "docker" / "docker-compose.fortran.yml").write_text("services: {}\n")
        (project / "reference" / "flexpart-11.1.json").write_text('{"pinned_commit":"abc"}\n')
        (oracle / "src" / "makefile_gfortran").write_text("all:\n\ttrue\n")
        return project, oracle

    @mock.patch.object(oracle_build_cache, "git_head", return_value="a" * 40)
    def test_cache_key_changes_with_relevant_build_input(self, _git_head):
        with tempfile.TemporaryDirectory() as directory:
            project, oracle = self.create_inputs(Path(directory))
            first = oracle_build_cache.cache_identity(project, oracle)
            (project / "docker" / "Dockerfile.fortran").write_text("FROM other-pinned\n")
            second = oracle_build_cache.cache_identity(project, oracle)
            self.assertNotEqual(first["cache_key"], second["cache_key"])

    @mock.patch.object(oracle_build_cache, "git_head", return_value="a" * 40)
    def test_validation_rejects_changed_executable_or_image(self, _git_head):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project, oracle = self.create_inputs(root)
            executable = oracle / "src" / "FLEXPART"
            executable.write_bytes(b"oracle-v1")
            identity = oracle_build_cache.cache_identity(project, oracle)
            metadata = root / "build.json"
            build_log = root / "build.log"
            build_log.write_text("successful build\n")
            metadata.write_text(
                json.dumps(
                    {
                        "schema": oracle_build_cache.SCHEMA,
                        "identity": identity,
                        "docker_image_id": "sha256:image",
                        "oracle_executable_sha256": oracle_build_cache.sha256(executable),
                        "build_log_sha256": oracle_build_cache.sha256(build_log),
                    }
                )
            )
            self.assertTrue(
                oracle_build_cache.validate(metadata, identity, "sha256:image", executable)
            )
            self.assertFalse(
                oracle_build_cache.validate(metadata, identity, "sha256:other", executable)
            )
            executable.write_bytes(b"oracle-v2")
            self.assertFalse(
                oracle_build_cache.validate(metadata, identity, "sha256:image", executable)
            )

    @mock.patch.object(oracle_build_cache, "git_head", return_value="a" * 40)
    def test_validation_rejects_missing_build_log(self, _git_head):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project, oracle = self.create_inputs(root)
            executable = oracle / "src" / "FLEXPART"
            executable.write_bytes(b"oracle-v1")
            identity = oracle_build_cache.cache_identity(project, oracle)
            metadata = root / "build.json"
            metadata.write_text(
                json.dumps(
                    {
                        "schema": oracle_build_cache.SCHEMA,
                        "identity": identity,
                        "docker_image_id": "sha256:image",
                        "oracle_executable_sha256": oracle_build_cache.sha256(executable),
                        "build_log_sha256": "missing",
                    }
                )
            )
            self.assertFalse(
                oracle_build_cache.validate(metadata, identity, "sha256:image", executable)
            )

    @mock.patch.object(oracle_build_cache, "git_head", return_value="a" * 40)
    def test_status_uses_absolute_retained_artifact_paths(self, _git_head):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project, oracle = self.create_inputs(root)
            identity = oracle_build_cache.cache_identity(project, oracle)
            metadata = root / "build.json"
            build_log = root / "build.log"
            metadata.write_text("{}\n")
            build_log.write_text("successful build\n")
            output = root / "status.json"
            oracle_build_cache.write_status(
                output,
                "REUSED",
                identity,
                "sha256:image",
                metadata,
                build_log,
            )
            status = json.loads(output.read_text())
            self.assertEqual(status["metadata"], str(metadata.resolve()))
            self.assertEqual(status["log"], str(build_log.resolve()))


class AgentValidationTest(unittest.TestCase):
    def test_oracle_check_reuses_existing_runner(self):
        commands = agent_validation.commands_for(
            "oracle", "ADV-ANA-001", Path("comparison.json")
        )
        self.assertEqual([name for name, _ in commands], ["oracle"])
        self.assertIn("run-corpus.sh", commands[0][1][1])

    def test_comparison_is_focused_and_uses_existing_audit_and_report(self):
        commands = agent_validation.commands_for(
            "comparison", "WIND-UNI-002", Path("comparison.json")
        )
        self.assertEqual(
            [name for name, _ in commands],
            ["candidate", "oracle", "input-audit", "comparison", "manifest"],
        )
        for name, command in commands[-3:]:
            self.assertIn("--case", command, name)
            self.assertIn("WIND-UNI-002", command, name)

    def test_deposition_comparison_declares_and_runs_oracle_calibration(self):
        commands = agent_validation.commands_for(
            "comparison", "DRY-007", Path("comparison.json")
        )
        by_name = dict(commands)
        self.assertIn("oracle-calibration", by_name)
        self.assertIn("input-audit-calibration", by_name)
        self.assertIn("ADV-ANA-001", by_name["oracle-calibration"])
        self.assertEqual(
            by_name["comparison"][
                by_name["comparison"].index("--oracle-calibration-case") + 1
            ],
            "ADV-ANA-001",
        )
        self.assertEqual(
            by_name["manifest"][
                by_name["manifest"].index("--oracle-dependency-case") + 1
            ],
            "ADV-ANA-001",
        )

    def test_failure_tail_is_bounded(self):
        output = "\n".join(f"line {number}" for number in range(100))
        tail = agent_validation.bounded_tail(output)
        self.assertEqual(len(tail), 30)
        self.assertEqual(tail[-1], "line 99")

    def test_failure_tail_truncates_single_huge_line(self):
        tail = agent_validation.bounded_tail("x" * 10_000)
        self.assertEqual(len(tail), 1)
        self.assertEqual(len(tail[0]), agent_validation.MAX_DIAGNOSTIC_LINE_CHARS)
        self.assertTrue(tail[0].startswith("..."))

    def test_supported_cases_exclude_non_corpus_and_unpaired_cases(self):
        cases = agent_validation.focused_oracle_cases()
        self.assertIn("ADV-ANA-001", cases)
        self.assertNotIn("REPEAT-009", cases)
        self.assertNotIn("RESTART-010", cases)
        self.assertNotIn("ETEX-MINI-013", cases)

    def test_default_output_directory_is_run_scoped(self):
        output = agent_validation.default_output_dir("ADV-ANA-001")
        self.assertEqual(output.parent.name, "ADV-ANA-001")
        self.assertRegex(output.name, r"^\d{8}T\d{6}\.\d{6}Z-\d+$")

    def test_explicit_output_directory_cannot_overwrite_logs(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "run"
            self.assertEqual(agent_validation.create_output_dir(output), output.resolve())
            (output / "oracle.log").write_text("retained\n")
            with self.assertRaisesRegex(ValueError, "not empty"):
                agent_validation.create_output_dir(output)

    def test_missing_docker_is_blocked(self):
        self.assertEqual(
            agent_validation.classify_failure("Docker is required for the oracle build"),
            "BLOCKED",
        )


if __name__ == "__main__":
    unittest.main()
