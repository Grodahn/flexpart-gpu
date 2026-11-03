#!/usr/bin/env python3
"""Mutate real #227 device evidence; absent execution is a test failure."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest

from check_genuine_production_replay import ROOT, check_resident, check_resident_both


class ResidentEvidenceTests(unittest.TestCase):
    def setUp(self):
        directory = ROOT / 'target/ci-gate/precipitation-resident'
        path = Path(os.environ.get('FLEXPART_GPU_RESIDENT_REPORT', directory / 'report-software_wgsl.json'))
        self.source = json.loads(path.read_text())
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name) / 'report.json'

    def write(self, value):
        self.path.write_text(json.dumps(value))

    def test_complete_actual_device_execution(self):
        self.write(self.source)
        check_resident(self.path)

    def test_missing_or_wrong_adapter_execution(self):
        with self.assertRaises(ValueError):
            check_resident_both(Path(self.temporary.name))
        with self.assertRaises(ValueError):
            check_resident(self.path)
        self.write(self.source)
        software = self.source['adapter']['adapter_class'] == 'software_wgsl'
        with self.assertRaises(ValueError):
            check_resident(self.path, software=not software, hardware=software)

    def test_fail_closed_matrix(self):
        def first_row(r):
            return r['rows'][0]

        def small(r):
            return r['allocation_audit']['small']

        mutations = {
            'stale candidate': lambda r: r['candidate'].update(revision='0'*40),
            'uncommitted': lambda r: r['candidate'].update(worktree_clean=False),
            'missing execution': lambda r: r.pop('adapter'),
            'wrong class': lambda r: r['adapter'].update(adapter_class='not_executed'),
            'CPU fallback': lambda r: r['gpu_evidence']['execution'].update(calculation_path='cpu_replacement'),
            'missing evidence': lambda r: r.pop('gpu_evidence'),
            'two submissions': lambda r: r.update(submissions=2),
            'per-step #207': lambda r: r.update(member_reconstructions=49),
            'intermediate readback': lambda r: r.update(host_transfers_between_stages=1),
            'host coordinates': lambda r: r.update(host_coordinate_uploads=1),
            'wrong rate shader': lambda r: r['shaders'].update(precipitation_resident_rates='0'*64),
            'wrong adapter shader': lambda r: r['shaders'].update(resident_query_adapter='0'*64),
            'fatal producer': lambda r: r.update(status=2),
            'fatal step': lambda r: r['steps'][0].update(fatal_status=1),
            'failed lane': lambda r: r['steps'][0]['lane_status'].__setitem__(0, 9),
            'published inactive lane': lambda r: r['steps'][0]['inactive_or_tail_bits'][0].__setitem__(0, '00000000'),
            'active tail': lambda r: r['steps'][0]['lane_status'].__setitem__(15, 2),
            'missing step': lambda r: r['steps'].pop(),
            'wrong pair': lambda r: r['steps'][1].update(memory_pair_slots=[1, 2]),
            'shifted time': lambda r: r['steps'][0].update(itime_epoch_seconds=r['steps'][0]['itime_epoch_seconds']+1),
            'time arithmetic': lambda r: r['steps'][-1].update(time_differences_seconds=[0, 0, 1]),
            'lane layout': lambda r: r['lanes'].update(active_count=16),
            'per-particle planes': lambda r: small(r).update(device_bytes=small(r)['bound_bytes']+4),
            'particle-scaled dispatches': lambda r: r['allocation_audit']['large'].update(dispatches=4096),
            'per-lane bind groups': lambda r: r['allocation_audit'].update(per_lane_bind_groups=16),
            'missing stage': lambda r: small(r)['stages'].pop(),
            'bitwise drift': lambda r: first_row(r).update(batch_bits=['00000000', '00000000']),
            'bitwise claim': lambda r: r['bitwise_equal']['final'].update(bitwise_equal=False),
            'missing final': lambda r: r['bitwise_equal'].pop('final'),
            'wrong lane': lambda r: first_row(r).update(lane=3),
            'missing output': lambda r: r['rows'].pop(),
            'wrong output': lambda r: first_row(r).update(candidate=[1000.0, 1000.0]),
            'NaN': lambda r: first_row(r).update(candidate=[float('nan'), 0.0]),
            'changed original': lambda r: first_row(r).update(original=[1000.0, 1000.0]),
            'vacuous comparison': lambda r: first_row(r).update(absolute_differences=[]),
            'RRINT1': lambda r: r['metadata']['preparation'].update(rrint=1),
            'numpf3': lambda r: r['metadata']['preparation'].update(numpf=3),
            'grid': lambda r: r['metadata']['horizontal_grid'].update(nx=4),
            'no #216': lambda r: r['metadata']['convective']['quantization_admissions'].clear(),
            'interval mean': lambda r: r['metadata'].update(representation='interval_mean'),
            'changed manifest': lambda r: r.update(input_manifest=r['input_manifest'].replace('"capacity":16', '"capacity":17')),
            'false pass': lambda r: r.update(verdict='failed'),
        }
        for label, mutation in mutations.items():
            with self.subTest(label=label):
                value = copy.deepcopy(self.source)
                mutation(value)
                self.write(value)
                with self.assertRaises((ValueError, KeyError, IndexError)):
                    check_resident(self.path)


if __name__ == '__main__':
    unittest.main()
