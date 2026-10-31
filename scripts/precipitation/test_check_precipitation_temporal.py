#!/usr/bin/env python3
"""Mutate real #119 device evidence; absent execution is a test failure."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest

from check_genuine_production_replay import ROOT, check_temporal, check_temporal_both, harness_digests


class TemporalEvidenceTests(unittest.TestCase):
    def setUp(self):
        directory = ROOT / 'target/ci-gate/precipitation-temporal'
        path = Path(os.environ.get('FLEXPART_GPU_TEMPORAL_REPORT', directory / 'software_wgsl.json'))
        self.source = json.loads(path.read_text())
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name) / 'report.json'

    def write(self, value):
        self.path.write_text(json.dumps(value))

    def test_complete_actual_device_execution(self):
        self.write(self.source)
        check_temporal(self.path)

    def test_harness_encoding_preserves_content_identity(self):
        path = Path(self.temporary.name) / 'driver.f90'
        path.write_bytes(b'program oracle\nend program oracle\n')
        expected = harness_digests(path)
        path.write_bytes(b'program oracle\r\nend program oracle\r\n')
        self.assertEqual(harness_digests(path), expected)
        path.write_bytes(b'program modified\nend program modified\n')
        self.assertTrue(harness_digests(path).isdisjoint(expected))

    def test_missing_or_wrong_adapter_execution(self):
        with self.assertRaises(ValueError):
            check_temporal_both(Path(self.temporary.name))
        with self.assertRaises(ValueError):
            check_temporal(self.path)
        self.write(self.source)
        software = self.source['adapter']['adapter_class'] == 'software_wgsl'
        with self.assertRaises(ValueError):
            check_temporal(self.path, software=not software, hardware=software)

    def test_fail_closed_matrix(self):
        mutations = {
            'stale candidate': lambda r: r['candidate'].update(revision='0'*40),
            'uncommitted': lambda r: r['candidate'].update(worktree_clean=False),
            'missing execution': lambda r: r.pop('adapter'),
            'wrong class': lambda r: r['adapter'].update(adapter_class='not_executed'),
            'CPU fallback': lambda r: r['gpu_evidence']['execution'].update(calculation_path='cpu_replacement'),
            'missing evidence': lambda r: r.pop('gpu_evidence'),
            'intermediate readback': lambda r: r.update(host_transfers_between_stages=1),
            'two submissions': lambda r: r.update(submissions=2),
            'bad stage order': lambda r: r['stage_order'].reverse(),
            'wrong shader': lambda r: r['shaders'].update(precipitation_temporal='0'*64),
            'fatal producer': lambda r: r.update(status=2),
            'fatal sampler': lambda r: r.update(status=4),
            'missing pair': lambda r: r['metadata']['memory_pairs'].pop(),
            'changed time': lambda r: r['metadata']['queries'][0].update(itime_epoch_seconds=0),
            'outside support': lambda r: r['metadata']['members']['members'][0].update(support_end_epoch_seconds=0),
            'missing leading': lambda r: r['metadata']['members']['members'].pop(0),
            'missing trailing': lambda r: r['metadata']['members']['intervals'].pop(),
            'grid': lambda r: r['metadata']['members']['horizontal_grid'].update(nx=4),
            'calendar': lambda r: r['metadata']['members'].update(calendar='no_leap'),
            'duplicate interior': lambda r: r['metadata']['memory_pairs'][0].update(right=r['metadata']['memory_pairs'][0]['left']),
            'swapped pair': lambda r: r['metadata']['memory_pairs'][0].update(left=r['metadata']['memory_pairs'][0]['right']),
            'interval amount': lambda r: r.update(unit='kilogram_per_square_meter'),
            'interval mean': lambda r: r['metadata']['members'].update(representation='interval_mean'),
            'RRINT1': lambda r: r['metadata']['members']['preparation'].update(rrint=1),
            'numpf3': lambda r: r['metadata']['members']['preparation'].update(numpf=3),
            'PUREFC': lambda r: r['metadata']['members']['preparation'].update(purefc=1),
            'BASETIME': lambda r: r['metadata']['members']['preparation'].update(basetime_hour=0),
            'no #216': lambda r: r['metadata']['members']['convective']['quantization_admissions'].clear(),
            'invalid #216': lambda r: r['metadata']['members']['large_scale']['quantization_admissions'][0].update(bound_exact='1/2'),
            'missing output': lambda r: r['rows'].pop(),
            'wrong output': lambda r: r['rows'][0].update(candidate=[1000.0, 1000.0]),
            'NaN': lambda r: r['rows'][0].update(candidate=[float('nan'), 0.0]),
            'infinity': lambda r: r['rows'][0].update(candidate=[float('inf'), 0.0]),
            'changed original': lambda r: r['rows'][0].update(original=[1000.0, 1000.0]),
            'false pass': lambda r: r.update(verdict='failed'),
        }
        for label, mutation in mutations.items():
            with self.subTest(label=label):
                value = copy.deepcopy(self.source)
                mutation(value)
                self.write(value)
                with self.assertRaises((ValueError, KeyError)):
                    check_temporal(self.path)


if __name__ == '__main__':
    unittest.main()
