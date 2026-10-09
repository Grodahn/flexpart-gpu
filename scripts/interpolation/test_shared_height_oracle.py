#!/usr/bin/env python3
"""Fail-closed regression checks for the #118 research evidence consumer."""

import copy
import tempfile
import unittest
from pathlib import Path

from prepare_shared_height_oracle import decode, decode_input, investigate


FIXTURE = Path(__file__).resolve().parents[2] / 'fixtures/interpolation/shared-height-v1'
SCRATCH = Path(__file__).resolve().parents[2] / 'target/shared-height-test-scratch'


class ResearchEvidenceTests(unittest.TestCase):
    def test_frozen_direct_evidence_distinguishes_alternatives(self):
        native, shared, queries = decode(FIXTURE / 'output.txt')
        self.assertEqual(len(investigate(native, shared, queries)), 9)
        self.assertEqual(len(decode_input(FIXTURE / 'input.txt')['columns_repeated_in_y']), 4)

    def test_missing_native_row_fails(self):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            path = Path(directory) / 'output.txt'
            lines = (FIXTURE / 'output.txt').read_text().splitlines()
            path.write_text('\n'.join(lines[:1] + lines[2:]) + '\n')
            with self.assertRaisesRegex(ValueError, 'incomplete'):
                decode(path)

    def test_nonfinite_output_fails(self):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            path = Path(directory) / 'output.txt'
            text = (FIXTURE / 'output.txt').read_text()
            path.write_text(text.replace('0.0000000000000000E+000', 'NaN', 1))
            with self.assertRaisesRegex(ValueError, 'non-finite'):
                decode(path)

    def test_tampered_actual_sample_fails(self):
        native, shared, queries = decode(FIXTURE / 'output.txt')
        changed = copy.deepcopy(queries)
        changed[2][-3] += 1
        with self.assertRaisesRegex(ValueError, 'disagrees'):
            investigate(native, shared, changed)

    def test_identical_time_members_do_not_prove_temporal_path(self):
        native, shared, queries = decode(FIXTURE / 'output.txt')
        changed = copy.deepcopy(queries)
        changed[5][-3:] = changed[6][-3:]
        changed[7][-3:] = changed[6][-3:]
        with self.assertRaises(ValueError):
            investigate(native, shared, changed)


if __name__ == '__main__':
    unittest.main()
