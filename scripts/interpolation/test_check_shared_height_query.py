#!/usr/bin/env python3
"""Reject incomplete or self-consistent corrupted #185 device evidence."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest

from check_shared_height_query import check, ROOT


class QueryEvidenceTests(unittest.TestCase):
    """Exercise the issue-owned checker against actual retained device output."""

    def setUp(self):
        source = Path(os.environ.get('FLEXPART_GPU_SHARED_QUERY_EVIDENCE',
                                     ROOT / 'target/ci-gate/shared-height-query'))
        self.original = json.loads((source / 'report.json').read_text())
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name) / 'report.json'
        self.path.with_name('inputs.json').write_bytes((source / 'inputs.json').read_bytes())

    def write(self, value):
        self.path.write_text(json.dumps(value))

    def test_complete_real_device_evidence(self):
        self.write(self.original)
        check(self.path)

    def test_incomplete_duplicate_and_nonfinite_rows(self):
        for mutation in ('missing', 'duplicate', 'nonfinite', 'expected', 'limit'):
            value = copy.deepcopy(self.original)
            if mutation == 'missing':
                value['rows'].pop()
            elif mutation == 'duplicate':
                value['rows'][1] = value['rows'][0]
            elif mutation == 'nonfinite':
                value['rows'][0]['actual_m_s'] = float('nan')
            elif mutation == 'expected':
                value['rows'][0]['oracle_m_s'] += 1
            else:
                value['rows'][0]['allowed_difference_m_s'] = 100
            self.write(value)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                check(self.path)

    def test_identity_execution_and_fatal_failures(self):
        paths = [('evidence', 'candidate', 'revision'), ('evidence', 'candidate', 'shader_sha256'),
                 ('evidence', 'execution', 'status'), ('evidence', 'oracle', 'output_sha256'),
                 ('sources', 0, 'snapshot_sha256'), ('targets', 1, 'identity_sha256'),
                 ('resident_metadata', 0, 'shared_height', 'preparation')]
        for path in paths:
            value = copy.deepcopy(self.original)
            selected = value
            for key in path[:-1]:
                selected = selected[key]
            selected[path[-1]] = 'invalid'
            self.write(value)
            with self.subTest(path=path), self.assertRaises(ValueError):
                check(self.path)
        value = copy.deepcopy(self.original)
        next(c for c in value['negative_cases'] if c['case'] == 'incomplete-lane')['status']['fatal'] = False
        self.write(value)
        with self.assertRaises(ValueError):
            check(self.path)


if __name__ == '__main__':
    unittest.main()
