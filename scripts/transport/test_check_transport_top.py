#!/usr/bin/env python3
"""Reject incomplete or self-consistently corrupted #198 evidence after real-driver execution."""
import copy
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

from check_transport_top import check, digest


class TransportTopEvidence(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = Path(os.environ.get('FLEXPART_GPU_TRANSPORT_TOP_EVIDENCE',
                                        'target/ci-gate/transport-top/validation-0-compaction-0'))
        cls.report = json.loads((cls.source / 'report.json').read_text())

    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='transport-top-audit-')
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name) / 'evidence'
        shutil.copytree(self.source, self.root)
        self.path = self.root / 'report.json'

    def mutate(self, callback):
        report = copy.deepcopy(self.report)
        callback(report)
        self.path.write_text(json.dumps(report))
        with self.assertRaises((ValueError, KeyError)):
            check(self.path)

    def device_mutation(self, callback):
        def change(report):
            row = next(r for r in report['rows'] if not r['settling_rejected'])
            path = self.root / row['device_file']
            device = json.loads(path.read_text())
            callback(device)
            raw = json.dumps(device).encode()
            path.write_bytes(raw)
            row['device_sha256'] = digest(raw)
        self.mutate(change)

    def test_complete_device_evidence(self):
        result = check(self.path)
        self.assertEqual(result['exact_projection_rows'], 56)
        self.assertEqual(result['trajectory_diagnostic_rows'], 8)
        self.assertEqual(result['rejected_settling_rows'], 8)
        self.assertIn('PARITY_NOT_ESTABLISHED', result['scientific_verdict'])

    def test_explicit_ci_head_revision_accepts_merge_checkout(self):
        with patch.dict(os.environ, {'FLEXPART_GPU_CANDIDATE_REVISION': self.report['candidate_revision']}), \
             patch('check_transport_top.subprocess.check_output', return_value='c'*40+'\n'):
            self.assertEqual(check(self.path)['state'], 'PASS')

    def test_missing_case(self):
        self.mutate(lambda report: report['rows'].pop())

    def test_missing_source_inventory(self):
        self.mutate(lambda report: report['source_hashes'].pop('src/gpu/meteorology/transport_boundary.rs'))

    def test_stale_revision(self):
        self.mutate(lambda report: report.update(candidate_revision='0'*40))

    def test_pending_status(self):
        self.device_mutation(lambda device: device['status'].__setitem__(1, 1))

    def test_advanced_coordinate(self):
        self.device_mutation(lambda device: device['queries'][131].update(fraction_x=0.25))

    def test_advanced_height(self):
        self.device_mutation(lambda device: device['queries'][131].update(height_agl_m=9521.84375))

    def test_mixed_target(self):
        self.device_mutation(lambda device: device['metadata'][3]['shared_height'].update(target_identity_sha256='0'*64))

    def test_ordinary_trajectory_corruption(self):
        self.device_mutation(lambda device: device['corrected_bytes'].__setitem__(8, device['corrected_bytes'][8]^1))

    def test_tail_publication_corruption(self):
        self.device_mutation(lambda device: device['published_bytes'].__setitem__(-1, device['published_bytes'][-1]^1))

    def test_settling_rejection_atomicity(self):
        def change(report):
            row = next(r for r in report['rows'] if r['settling_rejected'])
            row['final_bytes'][0] ^= 1
        self.mutate(change)

    def flagged_mutation(self, callback):
        def change(report):
            row=report['flagged_rows'][3]
            path=self.root/row['device_file']
            device=json.loads(path.read_text())
            callback(device)
            raw=json.dumps(device).encode();path.write_bytes(raw);row['device_sha256']=digest(raw)
        self.mutate(change)

    def test_missing_flagged_row(self):
        self.mutate(lambda r:r['flagged_rows'].pop())

    def test_wrong_flagged_oracle_hash(self):
        self.mutate(lambda r:r.update(flagged_oracle_fixture_sha256='0'*64))

    def test_wrong_stop_bit(self):
        self.flagged_mutation(lambda d:d['stop_words'].__setitem__(0,0))

    def test_missing_route_provenance(self):
        self.flagged_mutation(lambda d:d.pop('settling_provenance_sha256'))

    def test_missing_two_step(self):
        self.flagged_mutation(lambda d:d.update(second_step=None))

    def test_missing_flagged_software_execution(self):
        def change(r):r['flagged_rows'][0]['adapter'].update(device_type='cpu',adapter_class='hardware_gpu')
        self.mutate(change)

    def test_flagged_mass_loss(self):
        self.flagged_mutation(lambda d:d['published_bytes'].__setitem__(24,d['published_bytes'][24]^1))

    def test_partial_fatal_termination(self):
        self.mutate(lambda r:r['lifecycle'][1]['published_bytes'].__setitem__(66*96+20,0))

    def test_missing_mixed_batch(self):
        self.mutate(lambda r:r['lifecycle'].pop())

    def test_second_step_changed_stopped_lane(self):
        self.mutate(lambda r:r['lifecycle'][0]['second_step']['published_bytes'].__setitem__(2*96+8,0))

    def test_inactive_tail_stop(self):
        self.mutate(lambda r:r['lifecycle'][0]['stop_words'].__setitem__(129,1))

    def test_unsupported_marked_pass(self):
        self.mutate(lambda r:r['rows'][-1].update(settling_rejected=False))


if __name__ == '__main__':
    unittest.main()
