"""End-to-end tests for the corpus run-manifest writer v1 overlay.

These exercise the writer through its CLI with synthetic run artifacts,
covering the review fixes: unknown seed cases, exact case-component
attribution, unattributable oracle files and non-overwriting output.

Skipped when the pinned FLEXPART checkout is not available next to this
repository (the writer fails closed on an unpinned oracle checkout).
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
FLEXPART_DIR = REPO_ROOT.parent / "flexpart"
sys.path.insert(0, str(REPO_ROOT / "scripts" / "provenance"))

import run_provenance as provenance  # noqa: E402

PINNED = "c70586c2b7f5258850705325881c61f557ea9bd8"


def _git_output(*args, cwd):
    return subprocess.run(
        ["git", "-c", f"safe.directory={cwd}", "-C", cwd, *args],
        check=True, capture_output=True, text=True).stdout.strip()


def _seed(case_id, seed_index, directory):
    seed = {
        "case_id": case_id,
        "seed_index": seed_index,
        "philox_key": [1 + seed_index, 2],
        "philox_counter": [0, 0, 0, 0],
        "adapter": "writer-test-adapter",
        "candidate_revision": "writer-test",
        "particle_count": 1,
        "active_particles": 1,
        "particles": [{"lon_deg": 10.0, "lat_deg": 10.0, "z_m": 50.0,
                       "mass_kg": 1.0}],
        "metrics": {"total_mass_kg": 1.0},
    }
    path = directory / "seed_000.json"
    path.write_text(json.dumps(seed), encoding="utf-8")
    return path


def _write_tree(root):
    candidate = root / "candidate" / "ADV-ANA-001"
    candidate.mkdir(parents=True, exist_ok=True)
    for path in candidate.glob("seed_*.json"):
        path.unlink()
    _seed("ADV-ANA-001", 0, candidate)
    oracle = root / "oracle" / "ADV-ANA-001"
    oracle.mkdir(parents=True, exist_ok=True)
    (oracle / "header").write_bytes(b"header-bytes")
    (oracle / "dates").write_bytes(b"dates-bytes")
    meteo = root / "meteo" / "ADV-ANA-001"
    meteo.mkdir(parents=True, exist_ok=True)
    (meteo / "AVAILABLE").write_bytes(b"meteo-bytes")
    fixtures = root / "fortran" / "ADV-ANA-001"
    fixtures.mkdir(parents=True, exist_ok=True)
    (fixtures / "COMMAND").write_bytes(b"command-bytes")
    report = root / "comparison_report.json"
    report.write_text("{}", encoding="utf-8")
    cand_exe = root / "candidate-exe"
    cand_exe.write_bytes(b"candidate-binary")
    oracle_exe = root / "oracle-exe"
    oracle_exe.write_bytes(b"oracle-binary")
    return report, cand_exe, oracle_exe


def _run_writer(root, output, extra_args=(), case_id="ADV-ANA-001",
                build_tree=True):
    if build_tree:
        report, cand_exe, oracle_exe = _write_tree(root)
    else:
        report = root / "comparison_report.json"
        cand_exe = root / "candidate-exe"
        oracle_exe = root / "oracle-exe"
    cmd = [
        sys.executable,
        str(REPO_ROOT / "scripts" / "corpus" / "write_corpus_manifest.py"),
        "--output", str(output),
        "--corpus-index", str(REPO_ROOT / "fixtures" / "corpus" / "corpus.json"),
        "--oracle-manifest", str(REPO_ROOT / "reference" / "flexpart-11.1.json"),
        "--oracle-checkout", str(FLEXPART_DIR),
        "--candidate-checkout", str(REPO_ROOT),
        "--candidate-dir", str(root / "candidate"),
        "--oracle-dir", str(root / "oracle"),
        "--report", str(report),
        "--cases-dir", str(REPO_ROOT / "fixtures" / "corpus" / "cases"),
        "--fortran-fixtures", str(root / "fortran"),
        "--thresholds", str(REPO_ROOT / "fixtures" / "corpus" / "thresholds.json"),
        "--meteo-dir", str(root / "meteo"),
        "--candidate-exe", str(cand_exe),
        "--oracle-exe", str(oracle_exe),
        *extra_args,
    ]
    if case_id is not None:
        cmd.extend(["--case", case_id])
    return subprocess.run(cmd, capture_output=True, text=True)


@unittest.skipUnless(
    FLEXPART_DIR.is_dir()
    and _git_output("rev-parse", "HEAD", cwd=FLEXPART_DIR.resolve().as_posix()) == PINNED,
    "pinned FLEXPART checkout not available")
class CorpusWriterV1Test(unittest.TestCase):
    def test_valid_focused_manifest_verifies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "run_manifest.json"
            result = _run_writer(root, output)
            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(manifest["schema"]["id"], provenance.SCHEMA_ID)
            self.assertEqual(manifest["attribution"]["state"],
                             provenance.ATTRIBUTION_VERIFIED)
            verified = provenance.verify_run_manifest(
                output,
                search_roots=[root / "candidate" / "ADV-ANA-001",
                              root / "oracle" / "ADV-ANA-001",
                              root / "meteo" / "ADV-ANA-001",
                              REPO_ROOT / "fixtures" / "corpus" / "cases",
                              root])
            self.assertEqual(verified["state"], provenance.ATTRIBUTION_VERIFIED)

    def test_unknown_seed_case_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _write_tree(root)
            for path in (root / "candidate" / "ADV-ANA-001").glob("seed_*.json"):
                path.unlink()
            _seed("OTHER-CASE", 0, root / "candidate" / "ADV-ANA-001")
            output = root / "run_manifest.json"
            result = _run_writer(root, output, build_tree=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("not part of this run", result.stderr)

    def test_oracle_artifact_of_similar_case_name_not_captured(self):
        # DEMO-1 must not capture DEMO-10 outputs: attribution matches
        # whole path components only.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report, cand_exe, oracle_exe = _write_tree(root)
            decoy = root / "oracle" / "DEMO-10"
            decoy.mkdir(parents=True)
            (decoy / "header").write_bytes(b"decoy-header")
            output = root / "run_manifest.json"
            result = _run_writer(root, output)
            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads(output.read_text(encoding="utf-8"))
            oracle_outputs = [
                record["outputs_sha256"] for record in manifest["executions"]
                if record["role"] == "oracle"]
            self.assertTrue(oracle_outputs)
            for outputs in oracle_outputs:
                self.assertNotIn("DEMO-10", json.dumps(outputs))

    def test_unattributable_oracle_file_rejected(self):
        # Without --case the writer covers every declared corpus case, so
        # an oracle file outside any case directory cannot be attributed
        # and must fail closed instead of being dropped.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _write_tree(root)
            decoy = root / "oracle" / "unattributable"
            decoy.mkdir(parents=True)
            (decoy / "header").write_bytes(b"orphan-header")
            output = root / "run_manifest.json"
            result = _run_writer(root, output, case_id=None)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("cannot be attributed", result.stderr)

    def test_legacy_manifest_at_output_not_silently_replaced(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "run_manifest.json"
            output.write_text(json.dumps({"status": "legacy-manifest"}),
                             encoding="utf-8")
            result = _run_writer(root, output)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("refusing to overwrite", result.stderr)
            self.assertEqual(json.loads(output.read_text(encoding="utf-8")),
                             {"status": "legacy-manifest"})

    def test_rerun_produces_identical_run_id_at_new_path(self):
        # Execution identity is deterministic: identical evidence yields
        # the same run_id, while a same-path rerun refuses to overwrite
        # because the informational created_utc timestamp differs.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first_output = root / "run_manifest.json"
            first = _run_writer(root, first_output)
            self.assertEqual(first.returncode, 0, first.stderr)
            # Recreate byte-identical run artifacts and rerun to a new path.
            candidate = root / "candidate" / "ADV-ANA-001"
            for path in candidate.glob("seed_*.json"):
                path.unlink()
            _seed("ADV-ANA-001", 0, candidate)
            second_output = root / "run_manifest-2.json"
            second = _run_writer(root, second_output)
            self.assertEqual(second.returncode, 0, second.stderr)
            first_manifest = json.loads(first_output.read_text(encoding="utf-8"))
            second_manifest = json.loads(second_output.read_text(encoding="utf-8"))
            self.assertEqual(first_manifest["run_id"], second_manifest["run_id"])
            for record_a, record_b in zip(first_manifest["executions"],
                                          second_manifest["executions"]):
                self.assertEqual(record_a["execution_id"],
                                 record_b["execution_id"])

    def test_same_path_rerun_refuses_to_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "run_manifest.json"
            first = _run_writer(root, output)
            self.assertEqual(first.returncode, 0, first.stderr)
            second = _run_writer(root, output)
            self.assertNotEqual(second.returncode, 0)
            self.assertIn("refusing to overwrite", second.stderr)


if __name__ == "__main__":
    unittest.main()
