#!/usr/bin/env python3
"""Fail-closed regression checks for the #118 research evidence consumer."""

import copy
import contextlib
import io
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from prepare_shared_height_oracle import audit_query_inputs, decode, decode_input, investigate
from run_shared_height_oracle import main as launch


FIXTURE = Path(__file__).resolve().parents[2] / 'fixtures/interpolation/shared-height-v1'
SCRATCH = Path(__file__).resolve().parents[2] / 'target/shared-height-test-scratch'


class ResearchEvidenceTests(unittest.TestCase):
    def test_frozen_direct_evidence_distinguishes_alternatives(self):
        native, shared, queries = decode(FIXTURE / 'output.txt')
        self.assertEqual(len(investigate(native, shared, queries)), 9)
        self.assertEqual(len(decode_input(FIXTURE / 'input.txt')['columns_repeated_in_y']), 4)
        audit_query_inputs(decode_input(FIXTURE / 'input.txt'),
                           [shared[1, 0, k][0] for k in range(1, 5)], queries)

    def test_changed_declared_height_fails(self):
        _, shared, queries = decode(FIXTURE / 'output.txt')
        inputs = decode_input(FIXTURE / 'input.txt')
        inputs['query_time_s_x_y_target_bracket_fraction'][1][-1] = 0.55
        with self.assertRaisesRegex(ValueError, 'height/output mismatch'):
            audit_query_inputs(inputs, [shared[1, 0, k][0] for k in range(1, 5)], queries)

    def test_missing_upper_boundary_fails(self):
        _, shared, queries = decode(FIXTURE / 'output.txt')
        inputs = decode_input(FIXTURE / 'input.txt')
        inputs['query_time_s_x_y_target_bracket_fraction'][4][-2] = 0
        queries[4][4] = 0
        with self.assertRaisesRegex(ValueError, 'missing vertical query coverage'):
            audit_query_inputs(inputs, [shared[1, 0, k][0] for k in range(1, 5)], queries)

    def test_launcher_runs_recorded_immutable_image(self):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            checkout = Path(directory) / 'oracle'
            (checkout / '.git').mkdir(parents=True)
            identity = 'sha256:' + 'a' * 64
            calls = [subprocess.CompletedProcess([], 0, stdout=identity + '\n'),
                     subprocess.CompletedProcess([], 0)]
            argv = ['launcher', '--checkout', str(checkout), '--output-dir', str(Path(directory) / 'run')]
            with patch.object(sys, 'argv', argv), patch('subprocess.run', side_effect=calls) as run:
                with contextlib.redirect_stdout(io.StringIO()):
                    launch()
            command = run.call_args_list[1].args[0]
            self.assertEqual(command[command.index('bash') - 1], identity)
            self.assertIn(f'RESEARCH_IMAGE_ID={identity}', command)

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
