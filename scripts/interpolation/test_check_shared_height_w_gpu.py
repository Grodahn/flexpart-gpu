#!/usr/bin/env python3
"""Mutation tests for the #188 required device evidence auditor."""
import copy
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from check_shared_height_w_gpu import check


class EvidenceTests(unittest.TestCase):
    """Exercise failure boundaries using real outputs emitted by the device test."""
    def setUp(self):
        directory=Path(os.environ.get('FLEXPART_GPU_INTERIOR_W_EVIDENCE','target/ci-gate/interior-w-gpu'))
        self.record=json.loads((directory/'comparison.json').read_text())
        self.inputs=(directory/'inputs.json').read_bytes()
        self.temp=tempfile.TemporaryDirectory(dir=Path(__file__).resolve().parents[2]/"target");self.addCleanup(self.temp.cleanup)
        self.path=Path(self.temp.name)/'comparison.json'
        self.path.with_name('inputs.json').write_bytes(self.inputs)

    def audit(self, change=None):
        record=copy.deepcopy(self.record)
        if change:change(record)
        self.path.write_text(json.dumps(record))
        return check(self.path)

    def test_real_device_evidence_passes(self):
        self.assertEqual(self.audit()['state'],'PASS')

    def test_missing_or_false_device_claims_fail(self):
        for change in [lambda r:r['gpu_evidence']['execution'].update(adapter=None),
                       lambda r:r['gpu_evidence']['execution'].update(status='skipped'),
                       lambda r:r['gpu_evidence']['execution'].update(calculation_path='cpu_replacement'),
                       lambda r:r.update(submission_count=2), lambda r:r.update(intermediate_d2h_count=1)]:
            with self.subTest(change=change), self.assertRaises((ValueError,TypeError)):self.audit(change)

    def test_missing_duplicate_or_nonfinite_rows_fail(self):
        for change in [lambda r:r['rows'].pop(),lambda r:r['rows'].__setitem__(0,r['rows'][1]),
                       lambda r:r['rows'][0].update(actual_m_s=float('nan')),
                       lambda r:r['rows'][0].update(actual_m_s=99.0)]:
            with self.subTest(change=change), self.assertRaises(ValueError):self.audit(change)

    def test_hash_and_provenance_drift_fail(self):
        for change in [lambda r:r['gpu_evidence']['candidate'].update(shader_sha256='0'*64),
                       lambda r:r['gpu_evidence']['candidate'].update(input_sha256='0'*64),
                       lambda r:r['w_sources'][0]['normalization'].update(algorithm_id='geometric_identity_v1'),
                       lambda r:r['w_sources'][0].update(horizontal_sha256='0'*64),
                       lambda r:r['w_metadata'][0].update(unit='pascal_per_second'),
                       lambda r:r['final_w_status'][0].__setitem__(0,1),
                       lambda r:r['w_sources'][0].update(prepared_uv_sha256='0'*64),
                       lambda r:r['native_interface_lanes'][0][0].__setitem__(1,12.0),
                       lambda r:r['target'].update(height_sha256='0'*64),
                       lambda r:r['gpu_evidence']['oracle'].update(executable_sha256='0'*64)]:
            with self.subTest(change=change), self.assertRaises(ValueError):self.audit(change)

    @staticmethod
    def rebind_target(record, target, serialized):
        """Keep every dependent envelope consistent so the target audit is exercised."""
        record['target']=target
        record['serialized_target_identity']=serialized
        for index in range(2):
            record['uv_metadata'][index]['target']=copy.deepcopy(target)
            metadata=json.loads(record['serialized_uv_metadata'][index])
            metadata['target']=json.loads(serialized)
            metadata['target']['identity_sha256']=target['identity_sha256']
            encoded=json.dumps(metadata,separators=(',',':'))
            record['serialized_uv_metadata'][index]=encoded
            rebound=hashlib.sha256(encoded.encode()).hexdigest()
            record['w_sources'][index]['prepared_uv_sha256']=rebound
            record['w_metadata'][index]['source']['prepared_uv_sha256']=rebound
            record['w_metadata'][index]['prepared_uv']['target']=copy.deepcopy(target)

    def test_consistently_rebound_false_target_identity_fails(self):
        def change(record):
            target=copy.deepcopy(record['target']);target['identity_sha256']='0'*64
            self.rebind_target(record,target,record['serialized_target_identity'])
        with self.assertRaisesRegex(ValueError,'target provenance hash'):self.audit(change)

    def test_consistently_rebound_unsupported_selection_fails(self):
        def change(record):
            target=copy.deepcopy(record['target']);target['selection_rule']='unsupported_selection'
            preimage=json.loads(record['serialized_target_identity'])
            preimage['selection_rule']=target['selection_rule']
            encoded=json.dumps(preimage,separators=(',',':'))
            target['identity_sha256']=hashlib.sha256(encoded.encode()).hexdigest()
            self.rebind_target(record,target,encoded)
        with self.assertRaisesRegex(ValueError,'target selection rule'):self.audit(change)

    def test_missing_or_drifted_target_preimage_fails(self):
        for change in [lambda r:r.pop('serialized_target_identity'),
                       lambda r:r.update(serialized_target_identity=r['serialized_target_identity']+' ')]:
            with self.subTest(change=change), self.assertRaisesRegex(ValueError,'target'):self.audit(change)

    def test_missing_direction_and_unbound_metrics_fail(self):
        for change in [lambda r:r['contributions'].pop(), lambda r:r['contributions'][0].update(x_m_s=0.0),
                       lambda r:r['gpu_evidence']['comparison'].update(max_absolute_error=0.0),
                       lambda r:r['uv_metadata'][0]['source']['time'].update(valid_time_epoch_seconds=42)]:
            with self.subTest(change=change), self.assertRaises(ValueError):self.audit(change)


if __name__=='__main__':unittest.main()
