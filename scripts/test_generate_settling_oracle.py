"""Fail-closed regressions for the direct settling oracle artifact boundary."""

import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import generate_settling_oracle as oracle


class SettlingOracleTests(unittest.TestCase):
    def test_decoder_rejects_missing_duplicate_malformed_and_nonfinite_rows(self):
        vectors = [{"id": "A"}, {"id": "B"}]
        for raw in (
            b"A -1\n", b"A -1\nA -2\nB -3\n", b"A -1 extra\nB -2\n",
            b"A NaN\nB -2\n", b"A -1\nB 0\n", b"A -1\nB -2\nC -3\n",
        ):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                oracle.decode_outputs(raw, vectors)

    def test_frozen_fixture_audit_rejects_stale_artifacts(self):
        original = oracle.FIXTURE_DIR
        with tempfile.TemporaryDirectory() as tmp:
            fixtures = Path(tmp)
            for name in ("canonical-vectors-v1.json", "oracle-v1.json", "oracle-output-v1.txt"):
                (fixtures / name).write_bytes((original / name).read_bytes())
            document = json.loads((fixtures / "oracle-v1.json").read_bytes())
            with patch.object(oracle, "FIXTURE_DIR", fixtures):
                oracle.audit_fixtures()
                for key in ("harness_sha256", "canonical_sha256", "output_sha256"):
                    mutated = copy.deepcopy(document)
                    mutated[key] = "0" * 64
                    (fixtures / "oracle-v1.json").write_text(json.dumps(mutated))
                    with self.subTest(key=key), self.assertRaises(ValueError):
                        oracle.audit_fixtures()
                mutated = copy.deepcopy(document)
                mutated["values"][0]["settling_velocity_m_s"] *= 2
                (fixtures / "oracle-v1.json").write_text(json.dumps(mutated))
                with self.assertRaises(ValueError):
                    oracle.audit_fixtures()

    def test_audit_rejects_rehashed_unit_and_domain_changes(self):
        original = oracle.FIXTURE_DIR
        with tempfile.TemporaryDirectory() as tmp:
            fixtures = Path(tmp)
            for name in ("canonical-vectors-v1.json", "oracle-v1.json", "oracle-output-v1.txt"):
                (fixtures / name).write_bytes((original / name).read_bytes())
            canonical = json.loads((fixtures / "canonical-vectors-v1.json").read_bytes())
            document = json.loads((fixtures / "oracle-v1.json").read_bytes())
            with patch.object(oracle, "FIXTURE_DIR", fixtures):
                for section, value in (("units", "metre"), ("valid_domain", [0.1, 1000.0])):
                    mutated = copy.deepcopy(canonical)
                    mutated[section]["diameter_um"] = value
                    path = fixtures / "canonical-vectors-v1.json"
                    path.write_text(json.dumps(mutated), encoding="utf-8")
                    document["canonical_sha256"] = oracle.sha256_file(path)
                    (fixtures / "oracle-v1.json").write_text(json.dumps(document), encoding="utf-8")
                    with self.subTest(section=section), self.assertRaises(ValueError):
                        oracle.audit_fixtures()

    def test_unpinned_and_dirty_checkouts_are_rejected(self):
        for results in (["0" * 40], [oracle.pinned_revision(), " M src/settling_mod.f90"]):
            with patch.object(oracle, "git", side_effect=results), self.assertRaises(ValueError):
                oracle.verify_checkout(Path("unused"))


if __name__ == "__main__":
    unittest.main()
