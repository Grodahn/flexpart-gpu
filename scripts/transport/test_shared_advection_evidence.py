#!/usr/bin/env python3
"""Mutation tests over fresh #197 production evidence; missing evidence is an error."""
import copy
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from shared_advection_evidence import device, control_input, verify_control_sources
from prepare_interior_w_oracle import decode_input

class EvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = Path('target/ci-gate/shared-advection/validation-0-compaction-0')
        cls.report = json.loads((cls.source/'report.json').read_text())

    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix='shared-advection-audit-',dir='target/ci-gate')
        self.addCleanup(directory.cleanup)
        self.path = Path(directory.name)/'report.json'
        for name in ('report.json','inputs.json','negative.json'):
            shutil.copyfile(self.source/name, self.path.with_name(name))

    def reject(self, mutation):
        report = copy.deepcopy(self.report)
        mutation(report)
        self.path.write_text(json.dumps(report))
        with self.assertRaises((ValueError,KeyError)):
            device(self.path)

    def test_complete_device_evidence(self):
        device(self.path)

    def test_substituted_source_inventory(self):
        def substitute(report):
            import hashlib
            report['source_hashes'].pop('src/gpu/advection_resident.rs')
            report['source_hashes']['Cargo.toml']=hashlib.sha256(Path('Cargo.toml').read_bytes()).hexdigest()
        self.reject(substitute)

    def test_missing_candidate_executable(self):
        self.reject(lambda r:r.pop('candidate_executable_sha256'))

    def test_missing_driver_step(self):
        self.reject(lambda r:r['rows'].pop())

    def test_incomplete_status(self):
        self.reject(lambda r:r['rows'][0]['status'].__setitem__(1,1))

    def test_wrong_signed_time(self):
        self.reject(lambda r:r['rows'][0]['metadata'][3]['time']['instantaneous'].__setitem__('epoch_seconds',-1))

    def test_missing_corrected_w(self):
        self.reject(lambda r:r['rows'][0]['metadata'][0]['shared_height'].__setitem__('corrected_w',[]))

    def test_wrong_target(self):
        self.reject(lambda r:r['rows'][0]['metadata'][0]['shared_height'].__setitem__('target_identity_sha256','0'*64))

    def test_wrong_normalization(self):
        self.reject(lambda r:r['rows'][0]['preparations'][0]['source']['normalization'].__setitem__('source_id','foreign'))

    def test_false_reuse(self):
        self.reject(lambda r:r['rows'][0].__setitem__('reused',True))

    def test_missing_negative_evidence(self):
        self.path.with_name('negative.json').unlink()
        with self.assertRaises(FileNotFoundError):
            device(self.path)

    def test_partial_publication(self):
        path = self.path.with_name('negative.json')
        evidence=json.loads(path.read_text())
        evidence['rows'][0]['final_bytes'][0] ^= 1
        path.write_text(json.dumps(evidence))
        with self.assertRaises(ValueError):
            device(self.path)

    def control(self):
        inputs = json.loads(self.path.with_name('inputs.json').read_text())
        parent = Path('fixtures/interpolation/interior-w-v1')
        raw, _ = control_input(parent, self.report)
        path = self.path.with_name('control.txt')
        path.write_bytes(b'\n'.join(raw.splitlines()[:99]) + b'\n' +
                         b'\n'.join((parent/'input.txt').read_bytes().splitlines()[99:]) + b'\n')
        return inputs, decode_input(path)

    def test_complete_independent_control_inputs(self):
        verify_control_sources(*self.control())

    def test_changed_surface_winds_or_terrain(self):
        for field in ('wind_u10m','wind_v10m','orography'):
            with self.subTest(field=field):
                inputs, decoded = self.control()
                snapshot = json.loads(inputs['snapshot_json'][1])
                next(f for f in snapshot['fields'] if f['id'] == field)['values'][4] += 1
                inputs['snapshot_json'][1] = json.dumps(snapshot)
                with self.assertRaisesRegex(ValueError, 'canonical control surface'):
                    verify_control_sources(inputs, decoded)

    def test_changed_grid_hybrid_or_time(self):
        changes = (
            lambda s:s['horizontal_grid'].__setitem__('dx_deg',0.5),
            lambda s:s['vertical_coordinate']['hybrid_b_interface'].__setitem__(1,0.5),
            lambda s:s['fields'][0]['time'].__setitem__('valid_time_epoch_seconds',0),
            lambda s:s['fields'][0].__setitem__('storage_order','z_fastest'),
        )
        for change in changes:
            inputs, decoded = self.control()
            snapshot = json.loads(inputs['snapshot_json'][1])
            change(snapshot)
            inputs['snapshot_json'][1] = json.dumps(snapshot)
            with self.assertRaises(ValueError):
                verify_control_sources(inputs, decoded)

    def test_changed_native_motion_convention(self):
        inputs, decoded = self.control()
        motion = json.loads(inputs['native_motion_json'][0])
        motion['sign'] = 'positive_upward'
        inputs['native_motion_json'][0] = json.dumps(motion)
        with self.assertRaisesRegex(ValueError, 'native control motion convention'):
            verify_control_sources(inputs, decoded)

if __name__=='__main__':
    unittest.main()
