#!/usr/bin/env python3
"""Regression tests for the authoritative run-provenance path (issue #53).

Covers the required regression and the fail-closed rejections:

1. a valid run manifest verifies successfully;
2. replacing or modifying one output artifact fails verification;
3. representative input mutation fails verification;
4. mixed candidate revisions, mixed oracle builds, mixed run
   directories, stale reuse, missing artifacts and duplicate artifact
   identities are rejected;
5. pristine-oracle and seedable-validation-oracle identities stay
   distinct and are never collapsed;
6. execution identities are deterministic and carry no incidental
   state (timestamps/pids never change the identity).

Run from the repository root with::

    python scripts/provenance/test_run_provenance.py
"""

import json
import tempfile
import unittest
from pathlib import Path

import run_provenance as provenance


CASE_DOC = {
    "schema_version": 2,
    "case_id": "DEMO-001",
    "description": "synthetic provenance fixture",
}


def _write(path: Path, data: bytes) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path


def _case_file(directory: Path, case_id: str = "DEMO-001") -> Path:
    doc = dict(CASE_DOC, case_id=case_id)
    path = directory / f"{case_id}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(doc), encoding="utf-8")
    return path


def _build_manifest(directory: Path, *, oracle_kind="pristine-oracle",
                    seed_index=0, requested_identity=None):
    case_path = _case_file(directory / "cases")
    case = provenance.case_manifest_identity(case_path)
    input_path = _write(directory / "inputs" / "COMMAND", b"command-bytes")
    output_path = _write(directory / "outputs" / "seed_000.json",
                         b'{"seed_index": 0}')
    oracle_output = _write(directory / "outputs" / "header", b"oracle-bytes")
    candidate_exe = _write(directory / "bin" / "candidate", b"candidate-exe")
    oracle_exe = _write(directory / "bin" / "oracle", b"oracle-exe")
    realization: dict = {"seed_index": seed_index}
    oracle_realization: dict = {}
    if requested_identity is not None:
        oracle_realization = {"requested_identity": str(requested_identity)}
    strategy = None
    if oracle_kind == provenance.ORACLE_SEEDABLE:
        strategy = {"strategy": provenance.ORACLE_STRATEGY_ID,
                    "version": provenance.ORACLE_STRATEGY_VERSION,
                    "contract_path": "reference/oracle-stochastic-identity.json"}
    candidate_execution = provenance.build_execution_record(
        role="candidate", case=case, realization=realization,
        candidate_revision="abc123",
        candidate_executable_sha256=provenance.digest(candidate_exe),
        oracle_kind=oracle_kind,
        oracle_revision="c70586c2b7f5258850705325881c61f557ea9bd8",
        oracle_executable_sha256=provenance.digest(oracle_exe),
        oracle_profile={"id": provenance.ORACLE_PROFILE_ID, "version": 1},
        oracle_strategy=strategy,
        runtime_adapter="test-adapter",
        inputs_sha256={input_path.name: provenance.digest(input_path)},
        outputs_sha256={output_path.name: provenance.digest(output_path)},
    )
    oracle_execution = provenance.build_execution_record(
        role="oracle", case=case, realization=oracle_realization,
        candidate_revision="abc123",
        candidate_executable_sha256=provenance.digest(candidate_exe),
        oracle_kind=oracle_kind,
        oracle_revision="c70586c2b7f5258850705325881c61f557ea9bd8",
        oracle_executable_sha256=provenance.digest(oracle_exe),
        oracle_profile={"id": provenance.ORACLE_PROFILE_ID, "version": 1},
        oracle_strategy=strategy,
        cpu_runtime="flexpart-11.1-single-thread",
        inputs_sha256={input_path.name: provenance.digest(input_path)},
        outputs_sha256={oracle_output.name: provenance.digest(oracle_output)},
    )
    manifest = provenance.create_run_manifest(
        cases=[case],
        candidate={"revision": "abc123", "worktree_dirty": False,
                   "executable_sha256": provenance.digest(candidate_exe)},
        oracle={"kind": oracle_kind,
                "pinned_commit": "c70586c2b7f5258850705325881c61f557ea9bd8",
                "worktree_dirty": False,
                "executable_sha256": provenance.digest(oracle_exe),
                "execution_profile": {"id": provenance.ORACLE_PROFILE_ID,
                                      "version": 1},
                "strategy": strategy},
        runtime={"adapter": "test-adapter",
                 "cpu_runtime": "flexpart-11.1-single-thread"},
        executions=[candidate_execution, oracle_execution],
        base=str(directory),
        run_dir=str(directory / "runs" / "demo"),
    )
    manifest_path = directory / "run_manifest.json"
    provenance.write_run_manifest(manifest, manifest_path)
    return manifest, manifest_path, {
        "input": input_path, "output": output_path,
        "oracle_output": oracle_output, "candidate_exe": candidate_exe,
        "oracle_exe": oracle_exe, "case": case_path,
    }


class ValidRunTest(unittest.TestCase):
    def test_valid_manifest_verifies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _manifest, manifest_path, _files = _build_manifest(root)
            result = provenance.verify_run_manifest(
                manifest_path, search_roots=[root])
            self.assertEqual(result["state"], provenance.ATTRIBUTION_VERIFIED)
            self.assertEqual(result["missing"], [])

    def test_consumed_artifact_set_verifies_before_report_use(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _manifest, manifest_path, files = _build_manifest(root)
            result = provenance.verify_artifact_set(
                manifest_path, [("candidate_output", files["output"]),
                                ("oracle_header", files["oracle_output"])])
            self.assertEqual(result["state"], provenance.ATTRIBUTION_VERIFIED)


class OutputSubstitutionTest(unittest.TestCase):
    def test_modified_output_fails_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _manifest, manifest_path, files = _build_manifest(root)
            files["output"].write_bytes(b'{"seed_index": 999}')
            with self.assertRaisesRegex(
                    provenance.ProvenanceError,
                    "output substituted after manifest creation"):
                provenance.verify_run_manifest(
                    manifest_path, search_roots=[root])

    def test_replaced_output_fails_consumed_set_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _manifest, manifest_path, files = _build_manifest(root)
            files["oracle_output"].write_bytes(b"forged-oracle-bytes")
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "differs from run manifest"):
                provenance.verify_artifact_set(
                    manifest_path, [("oracle_header", files["oracle_output"])])


class InputMutationTest(unittest.TestCase):
    def test_changed_input_fails_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _manifest, manifest_path, files = _build_manifest(root)
            files["input"].write_bytes(b"mutated-command")
            with self.assertRaisesRegex(
                    provenance.ProvenanceError,
                    "changed input after manifest creation"):
                provenance.verify_run_manifest(
                    manifest_path, search_roots=[root])


class MixedRevisionTest(unittest.TestCase):
    def test_mixed_candidate_revisions_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case = provenance.case_manifest_identity(_case_file(root / "cases"))
            first = provenance.build_execution_record(
                role="candidate", case=case, realization={"seed_index": 0},
                candidate_revision="rev-a", oracle_kind="pristine-oracle",
                runtime_adapter="adapter",
                inputs_sha256={"COMMAND": "a" * 64},
                outputs_sha256={"seed_000.json": "b" * 64})
            second = provenance.build_execution_record(
                role="candidate", case=case, realization={"seed_index": 1},
                candidate_revision="rev-b", oracle_kind="pristine-oracle",
                runtime_adapter="adapter",
                inputs_sha256={"COMMAND": "a" * 64},
                outputs_sha256={"seed_001.json": "c" * 64})
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "mixed candidate revisions"):
                provenance.create_run_manifest(
                    cases=[case],
                    candidate={"revision": "rev-a",
                               "executable_sha256": "d" * 64},
                    oracle={"kind": "pristine-oracle",
                            "pinned_commit": "c" * 40,
                            "executable_sha256": "e" * 64,
                            "execution_profile": {"id": provenance.ORACLE_PROFILE_ID,
                                                  "version": 1}},
                    runtime={"adapter": "adapter"},
                    executions=[first, second], base=str(root))

    def test_mixed_oracle_builds_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case = provenance.case_manifest_identity(_case_file(root / "cases"))
            first = provenance.build_execution_record(
                role="oracle", case=case, realization={},
                oracle_kind="pristine-oracle",
                oracle_executable_sha256="a" * 64,
                oracle_profile={"id": provenance.ORACLE_PROFILE_ID, "version": 1},
                cpu_runtime="flexpart-11.1-single-thread",
                inputs_sha256={"COMMAND": "b" * 64},
                outputs_sha256={"header": "c" * 64})
            second = provenance.build_execution_record(
                role="oracle", case=case, realization={},
                oracle_kind="pristine-oracle",
                oracle_executable_sha256="d" * 64,
                oracle_profile={"id": provenance.ORACLE_PROFILE_ID, "version": 1},
                cpu_runtime="flexpart-11.1-single-thread",
                inputs_sha256={"COMMAND": "b" * 64},
                outputs_sha256={"header2": "e" * 64})
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "mixed oracle builds"):
                provenance.create_run_manifest(
                    cases=[case],
                    candidate={"revision": "rev",
                               "executable_sha256": "f" * 64},
                    oracle={"kind": "pristine-oracle",
                            "pinned_commit": "c" * 40,
                            "executable_sha256": "a" * 64,
                            "execution_profile": {"id": provenance.ORACLE_PROFILE_ID,
                                                  "version": 1}},
                    runtime={"adapter": "adapter"},
                    executions=[first, second], base=str(root))


class StaleMixedRunTest(unittest.TestCase):
    def test_stale_run_id_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, manifest_path, _files = _build_manifest(root)
            manifest["run_id"] = "0" * 64
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "stale artifact reused"):
                provenance.verify_run_manifest(
                    manifest_path, search_roots=[root])

    def test_missing_artifact_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _manifest, manifest_path, files = _build_manifest(root)
            files["output"].unlink()
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "missing required artifact"):
                provenance.verify_run_manifest(
                    manifest_path, search_roots=[root])

    def test_duplicate_artifact_identity_rejected(self):
        with self.assertRaisesRegex(
                provenance.ProvenanceError, "duplicate artifact identity"):
            provenance._normalize_artifact_map(
                {"/a/case/header": "a" * 64, "/b/case/header": "b" * 64})

    def test_non_overwriting_write_refuses_replacement(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "evidence.bin"
            provenance.ensure_non_overwriting_write(target, b"first")
            provenance.ensure_non_overwriting_write(target, b"first")
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "refusing to overwrite"):
                provenance.ensure_non_overwriting_write(target, b"second")


class OracleKindTest(unittest.TestCase):
    def test_pristine_and_seedable_stay_distinct(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pristine, _, _ = _build_manifest(
                root / "pristine", oracle_kind="pristine-oracle")
            seedable, _, _ = _build_manifest(
                root / "seedable", oracle_kind="seedable-validation-oracle",
                requested_identity=3)
            self.assertEqual(pristine["oracle"]["kind"], "pristine-oracle")
            self.assertEqual(seedable["oracle"]["kind"],
                             "seedable-validation-oracle")
            self.assertNotEqual(pristine["run_id"], seedable["run_id"])
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "unknown oracle kind"):
                provenance.build_execution_record(
                    role="oracle",
                    case=provenance.case_manifest_identity(
                        _case_file(root / "cases")),
                    oracle_kind="generic-oracle")

    def test_partial_attribution_never_promoted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case = provenance.case_manifest_identity(_case_file(root / "cases"))
            execution = provenance.build_execution_record(
                role="candidate", case=case, realization={"seed_index": 0},
                candidate_revision="unknown",
                oracle_kind="pristine-oracle",
                runtime_adapter=None, cpu_runtime=None,
                inputs_sha256={"COMMAND": "a" * 64},
                outputs_sha256={"seed_000.json": "b" * 64})
            manifest = provenance.create_run_manifest(
                cases=[case],
                candidate={"revision": "unknown",
                           "executable_sha256": None},
                oracle={"kind": "pristine-oracle",
                        "pinned_commit": "c" * 40,
                        "executable_sha256": None,
                        "execution_profile": {"id": provenance.ORACLE_PROFILE_ID,
                                              "version": 1}},
                runtime={"adapter": None},
                executions=[execution], base=str(root))
            self.assertEqual(manifest["attribution"]["state"],
                             provenance.ATTRIBUTION_PARTIAL)
            self.assertTrue(manifest["attribution"]["missing"])


class ExecutionIdentityTest(unittest.TestCase):
    def test_ensemble_realizations_receive_distinct_identities(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case = provenance.case_manifest_identity(_case_file(root / "cases"))
            first = provenance.build_execution_record(
                role="candidate", case=case, realization={"seed_index": 0},
                candidate_revision="rev", oracle_kind="pristine-oracle",
                runtime_adapter="adapter",
                inputs_sha256={"COMMAND": "a" * 64},
                outputs_sha256={"seed_000.json": "b" * 64})
            second = provenance.build_execution_record(
                role="candidate", case=case, realization={"seed_index": 1},
                candidate_revision="rev", oracle_kind="pristine-oracle",
                runtime_adapter="adapter",
                inputs_sha256={"COMMAND": "a" * 64},
                outputs_sha256={"seed_001.json": "c" * 64})
            self.assertNotEqual(first["execution_id"], second["execution_id"])

    def test_identity_ignores_incidental_state(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case = provenance.case_manifest_identity(_case_file(root / "cases"))
            kwargs = dict(role="candidate", case=case,
                          realization={"seed_index": 0},
                          candidate_revision="rev",
                          oracle_kind="pristine-oracle",
                          runtime_adapter="adapter",
                          inputs_sha256={"COMMAND": "a" * 64},
                          outputs_sha256={"seed_000.json": "b" * 64})
            self.assertEqual(
                provenance.build_execution_record(**kwargs)["execution_id"],
                provenance.build_execution_record(**kwargs)["execution_id"])
            run_dir = provenance.execution_run_dir(root, "DEMO-001",
                                                   "a" * 64)
            self.assertEqual(run_dir, root / "DEMO-001" / ("a" * 16))

    def test_legacy_manifest_never_silently_reinterpreted(self):
        with self.assertRaisesRegex(
                provenance.ProvenanceError, "cannot be silently reinterpreted"):
            provenance.migrate_legacy_manifest({"status": "old"})


class HardeningTest(unittest.TestCase):
    """Review fixes: traversal, fail-closed maps, integrity cross-checks."""

    def test_run_dir_rejects_parent_traversal_case_id(self):
        with self.assertRaisesRegex(provenance.ProvenanceError, "invalid case_id"):
            provenance.execution_run_dir(Path("/tmp"), "../escape", "a" * 64)
        with self.assertRaisesRegex(provenance.ProvenanceError, "invalid case_id"):
            provenance.execution_run_dir(Path("/tmp"), "a/../b", "a" * 64)

    def test_artifact_map_rejects_non_string_hash(self):
        with self.assertRaisesRegex(provenance.ProvenanceError, "hex string"):
            provenance._normalize_artifact_map({"header": 12345})

    def test_artifact_map_rejects_non_object(self):
        with self.assertRaisesRegex(provenance.ProvenanceError, "must be an object"):
            provenance._normalize_artifact_map(["not", "a", "dict"])

    def test_consumed_set_disambiguates_case_scoped_keys(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "case-a" / "header"
            second = root / "case-b" / "header"
            first.parent.mkdir(parents=True)
            second.parent.mkdir(parents=True)
            first.write_bytes(b"first")
            second.write_bytes(b"second")
            recorded = {
                "case-a/header": provenance.digest(first),
                "case-b/header": provenance.digest(second),
            }
            self.assertEqual(
                provenance._resolve_consumed_artifact(recorded, first, [root]),
                provenance.digest(first))
            self.assertEqual(
                provenance._resolve_consumed_artifact(recorded, second, [root]),
                provenance.digest(second))

    def test_consumed_set_rejects_truly_ambiguous_basename(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "case-a" / "header"
            second = root / "case-b" / "header"
            first.parent.mkdir(parents=True)
            second.parent.mkdir(parents=True)
            first.write_bytes(b"first")
            second.write_bytes(b"second")
            ambiguous = root / "elsewhere" / "header"
            ambiguous.parent.mkdir(parents=True)
            ambiguous.write_bytes(b"first")
            recorded = {
                "case-a/header": provenance.digest(first),
                "case-b/header": provenance.digest(second),
            }
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "duplicate artifact identity"):
                provenance._resolve_consumed_artifact(recorded, ambiguous, [root])

    def test_non_dict_json_document_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "run_manifest.json"
            path.write_text("[1, 2, 3]", encoding="utf-8")
            with self.assertRaisesRegex(provenance.ProvenanceError, "not a JSON object"):
                provenance.load_manifest(path)

    def test_non_dict_executions_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _manifest, manifest_path, _files = _build_manifest(root)
            document = json.loads(manifest_path.read_text(encoding="utf-8"))
            document["executions"] = "not-a-list"
            manifest_path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "non-empty array"):
                provenance.verify_run_manifest(
                    manifest_path, search_roots=[root])

    def test_pruned_artifact_map_entry_rejected(self):
        # An execution-recorded hash missing from the merged artifacts
        # map must not verify: pruning must not leave evidence unverified.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, manifest_path, files = _build_manifest(root)
            manifest["artifacts"]["outputs"].pop("seed_000.json")
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "execution-owned provenance"):
                provenance.verify_run_manifest(
                    manifest_path, search_roots=[root])

    def test_execution_map_disagreeing_with_merged_map_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, manifest_path, _files = _build_manifest(root)
            manifest["artifacts"]["outputs"]["seed_000.json"] = "0" * 64
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "execution-owned provenance"):
                provenance.verify_run_manifest(
                    manifest_path, search_roots=[root])

    def test_unowned_merged_artifact_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, manifest_path, _files = _build_manifest(root)
            manifest["artifacts"]["outputs"]["unowned.bin"] = "0" * 64
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "execution-owned provenance"):
                provenance.verify_run_manifest(
                    manifest_path, search_roots=[root])

    def test_consumed_set_does_not_cross_execution_roles(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _manifest, manifest_path, files = _build_manifest(root)
            result = provenance.verify_artifact_set(
                manifest_path,
                [("candidate-output", files["oracle_output"], "candidate")])
            self.assertEqual(result["state"], provenance.ATTRIBUTION_PARTIAL)
            self.assertEqual(result["verified"], [])

    def test_mutated_binding_cannot_reuse_execution_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, manifest_path, files = _build_manifest(root)
            replacement_hash = provenance.hash_bytes(b"replacement")
            candidate = next(
                record for record in manifest["executions"]
                if record["role"] == "candidate")
            candidate["outputs_sha256"][files["output"].name] = replacement_hash
            manifest["artifacts"]["outputs"][files["output"].name] = replacement_hash
            files["output"].write_bytes(b"replacement")
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(
                    provenance.ProvenanceError, "execution identity mismatch"):
                provenance.verify_run_manifest(
                    manifest_path, search_roots=[root])


class ManifestIntegrityTest(unittest.TestCase):
    """verify_manifest_integrity: disk-independent consistency checks."""

    def _document(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, _path, _files = _build_manifest(root)
        return manifest

    def test_consistent_manifest_passes(self):
        provenance.verify_manifest_integrity(self._document())

    def test_stale_run_id_rejected(self):
        document = self._document()
        document["run_id"] = "0" * 64
        with self.assertRaisesRegex(provenance.ProvenanceError, "stale artifact"):
            provenance.verify_manifest_integrity(document)

    def test_mixed_candidate_revision_rejected(self):
        document = self._document()
        extra = json.loads(json.dumps(
            next(r for r in document["executions"] if r["role"] == "candidate")))
        extra["candidate_revision"] = "other-revision"
        extra["execution_id"] = provenance.derive_execution_id(
            provenance._execution_binding(extra))
        document["executions"].append(extra)
        document["run_id"] = provenance.derive_run_id(
            [r["execution_id"] for r in document["executions"]])
        with self.assertRaisesRegex(
                provenance.ProvenanceError, "mismatched candidate revision"):
            provenance.verify_manifest_integrity(document)

    def test_mixed_oracle_build_rejected(self):
        document = self._document()
        extra = json.loads(json.dumps(
            next(r for r in document["executions"] if r["role"] == "oracle")))
        extra["oracle_executable_sha256"] = "f" * 64
        extra["execution_id"] = provenance.derive_execution_id(
            provenance._execution_binding(extra))
        document["executions"].append(extra)
        document["run_id"] = provenance.derive_run_id(
            [r["execution_id"] for r in document["executions"]])
        with self.assertRaisesRegex(
                provenance.ProvenanceError, "mismatched oracle identity/build"):
            provenance.verify_manifest_integrity(document)

    def test_invalid_state_rejected(self):
        document = self._document()
        document["attribution"]["state"] = provenance.ATTRIBUTION_INVALID
        with self.assertRaisesRegex(provenance.ProvenanceError, "INVALID"):
            provenance.verify_manifest_integrity(document)

    def test_mutated_candidate_summary_rejected(self):
        document = self._document()
        document["candidate"]["revision"] = "unbound-revision"
        with self.assertRaisesRegex(
                provenance.ProvenanceError, "summary revision disagrees"):
            provenance.verify_manifest_integrity(document)

    def test_mutated_oracle_summary_rejected(self):
        document = self._document()
        document["oracle"]["pinned_commit"] = "f" * 40
        with self.assertRaisesRegex(
                provenance.ProvenanceError, "summary revision disagrees"):
            provenance.verify_manifest_integrity(document)

    def test_non_v1_document_rejected(self):
        with self.assertRaisesRegex(provenance.ProvenanceError, "not a "):
            provenance.verify_manifest_integrity({"status": "legacy"})


class PartialPromotionTest(unittest.TestCase):
    """A manifest with gaps must verify as PARTIAL, never VERIFIED."""

    def test_missing_gaps_stay_partial_at_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, manifest_path, files = _build_manifest(root)
            manifest["attribution"]["state"] = provenance.ATTRIBUTION_PARTIAL
            manifest["attribution"]["missing"].append({
                "name": "candidate.executable_sha256",
                "reason": "no executable hash supplied",
            })
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            result = provenance.verify_run_manifest(
                manifest_path, search_roots=[root])
            self.assertEqual(result["state"], provenance.ATTRIBUTION_PARTIAL)
            self.assertTrue(result["missing"])


if __name__ == "__main__":
    unittest.main()
