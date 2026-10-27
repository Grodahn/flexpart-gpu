#!/usr/bin/env python3
"""Fail-closed regression checks for the #120 direct oracle evidence consumer."""

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import prepare_surface_flux_temporal_oracle as oracle
from run_surface_flux_temporal_oracle import main as launch

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / 'fixtures/interpolation/surface-flux-temporal-v1'


def tampered(transform):
    """Write a modified copy of the frozen raw output and decode it."""
    lines = (FIXTURE / 'output.txt').read_text(encoding='utf-8').splitlines()
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / 'output.txt'
        path.write_text('\n'.join(transform(lines)) + '\n', encoding='utf-8')
        return oracle.decode_output(path)


def replace_line(lines, prefix, replacement):
    index = next(i for i, line in enumerate(lines) if line.startswith(prefix))
    return lines[:index] + [replacement] + lines[index + 1:]


class SurfaceFluxOracleEvidenceTests(unittest.TestCase):
    def test_frozen_evidence_reproduces_declared_records(self):
        oracle.validate_declaration()
        self.assertEqual((FIXTURE / 'input.txt').read_text(encoding='utf-8'), oracle.input_text())
        records = oracle.audit(*oracle.decode_output(FIXTURE / 'output.txt'))
        frozen = json.loads((FIXTURE / 'report.json').read_text(encoding='utf-8'))
        self.assertEqual(len(records), 4 * 77)
        self.assertEqual(records, frozen['records'])
        sshf = [r for r in records if r['flexpart_variable'] == 'sshf']
        self.assertTrue(all(r['canonical_value'] == -r['flexpart_value'] for r in sshf))

    def test_changed_slot_value_fails(self):
        pairs, rows = tampered(lambda lines: replace_line(
            lines, 'FIELD 1', 'FIELD 1  4.2000000000000000E+001 -6.0250000000000000E+001  4.2000000000000000E+001'))
        with self.assertRaisesRegex(ValueError, 'memory slot does not store raw input'):
            oracle.audit(pairs, rows)

    def test_changed_time_weights_fail(self):
        pairs, rows = tampered(lambda lines: replace_line(
            lines, 'DTS', 'DTS  1.0000000000000000E+000  1.0800000000000000E+004  9.2592592409346253E-005'))
        with self.assertRaisesRegex(ValueError, 'find_time_vars mismatch'):
            oracle.audit(pairs, rows)

    def test_missing_query_fails(self):
        pairs, rows = oracle.decode_output(FIXTURE / 'output.txt')
        with self.assertRaisesRegex(ValueError, 'incomplete oracle queries'):
            oracle.audit(pairs, rows[:-1])

    def test_nonfinite_output_fails(self):
        with self.assertRaisesRegex(ValueError, 'non-finite'):
            tampered(lambda lines: replace_line(lines, 'FIELD 2', 'FIELD 2 nan nan nan'))

    def test_interval_declaration_without_both_signs_fails(self):
        values = [[abs(v) for v in snapshot] for snapshot in oracle.FIELDS[0]['values']]
        with patch.dict(oracle.FIELDS[0], {'values': values}):
            with self.assertRaisesRegex(ValueError, 'both signs'):
                oracle.validate_declaration()

    def test_missing_call_edge_fails(self):
        with self.assertRaisesRegex(ValueError, 'nm evidence lacks'):
            oracle.verify_call_edges('', '<MAIN__>:\n movabs $0x10,%r11\n lea x # 10 <a>', ['missing'])

    def test_launcher_rejects_output_outside_target(self):
        argv = ['run', '--checkout', str(ROOT), '--flex-extract', str(ROOT), '--output-dir', str(ROOT)]
        with patch.object(sys, 'argv', argv), contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises((SystemExit, ValueError)):
                launch()


if __name__ == '__main__':
    unittest.main()
