#!/usr/bin/env python3
"""Fail-closed regression tests for the finite direct transport evidence contract."""

import json
import tempfile
import unittest
from pathlib import Path

from verify_top_boundary_oracle import FIXTURE, ROOT, TOOLS, decode, digest, text_digest
from compare_top_boundary_builds import retained


class TopBoundaryEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = ROOT / 'target/top-boundary-test-scratch'
        cls.scratch.mkdir(parents=True, exist_ok=True)

    def test_frozen_output_and_provenance(self):
        report = json.loads((FIXTURE / 'report.json').read_text())
        self.assertEqual(decode(FIXTURE / 'output.txt'), report['science'])
        self.assertEqual(digest(FIXTURE / 'output.txt'), report['provenance']['artifacts_sha256']['output.txt'])
        self.assertEqual({s: text_digest(ROOT / s) for s in TOOLS}, report['provenance']['tools_sha256'])
        pair = json.loads((FIXTURE / 'reproducibility.json').read_text())
        self.assertEqual(pair['state'], 'PASS')
        self.assertEqual(pair['baseline_report_sha256'], digest(FIXTURE / 'report.json'))
        self.assertEqual(pair['verifier_sha256'], text_digest(ROOT / 'scripts/transport/compare_top_boundary_builds.py'))
        self.assertEqual(len(pair['runs']), 2)
        self.assertNotEqual(pair['runs'][0]['build_origin'], pair['runs'][1]['build_origin'])

    def reject(self, transform):
        with tempfile.TemporaryDirectory(dir=self.scratch) as tmp:
            path = Path(tmp) / 'output.txt'
            path.write_text(transform((FIXTURE / 'output.txt').read_text()))
            with self.assertRaises((ValueError, IndexError)):
                decode(path)

    def test_missing_duplicate_nonfinite_unknown_rows(self):
        self.reject(lambda s: '\n'.join(s.splitlines()[:-1]) + '\n')
        self.reject(lambda s: s + s.splitlines()[1] + '\n')
        self.reject(lambda s: s.replace('9.52184375000000000E+003', 'NaN'))
        self.reject(lambda s: s + 'UNKNOWN 1\n')

    def test_false_stop_and_native_target_leak(self):
        def bad_stop(s):
            rows = s.splitlines()
            for i,r in enumerate(rows):
                if r.startswith('PUSH 0 -1 1 0 '):
                    rows[i] = r[:-1] + 'F'
                    break
            return '\n'.join(rows) + '\n'
        self.reject(bad_stop)
        self.reject(lambda s: s.replace('GRID 1 2 1 ', 'GRID 1 2 0 ', 1))

    def test_record_only_and_missing_artifacts_cannot_prove_reproducibility(self):
        with tempfile.TemporaryDirectory(dir=self.scratch) as tmp:
            run = Path(tmp)
            report = json.loads((FIXTURE / 'report.json').read_text())
            (run / 'report.json').write_text(json.dumps(report))
            with self.assertRaisesRegex(ValueError, 'record-only'):
                retained(run)
            report['state'] = 'PASS'
            (run / 'report.json').write_text(json.dumps(report))
            with self.assertRaises(FileNotFoundError):
                retained(run)


if __name__ == '__main__':
    unittest.main()
