#!/usr/bin/env python3
"""Mutation tests over fresh #197 production evidence; missing evidence is an error."""
import copy
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from shared_advection_evidence import device

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

if __name__=='__main__':
    unittest.main()
