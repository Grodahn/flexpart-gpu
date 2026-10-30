#!/usr/bin/env python3
"""Reject incomplete or self-consistently corrupted #219 replay evidence.

Runs against the actual retained device reports in
``target/ci-gate/precipitation-members`` (override with
``FLEXPART_GPU_PRECIPITATION_EVIDENCE``); a missing report fails.
"""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest

from check_genuine_production_replay import REPORT_NAMES, ROOT, check, check_both


def evidence_dir():
    return Path(os.environ.get('FLEXPART_GPU_PRECIPITATION_EVIDENCE',
                               ROOT / 'target/ci-gate/precipitation-members'))


def available_report():
    for name in REPORT_NAMES.values():
        path = evidence_dir() / name
        if path.is_file():
            return path
    raise FileNotFoundError('no retained #219 replay report in ' + str(evidence_dir()))


class ReplayEvidenceTests(unittest.TestCase):
    """Mutate real device output; every mutation must fail closed."""

    def setUp(self):
        self.source = available_report()
        self.original = json.loads(self.source.read_text(encoding='utf-8'))
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name) / self.source.name

    def write(self, value):
        self.path.write_text(json.dumps(value), encoding='utf-8')

    def rejects(self, mutate, label):
        value = copy.deepcopy(self.original)
        mutate(value)
        self.write(value)
        with self.subTest(label), self.assertRaises(ValueError):
            check(self.path)

    def test_complete_real_device_evidence(self):
        self.write(self.original)
        check(self.path)

    def test_missing_report_and_missing_adapter_fail(self):
        with self.assertRaises(ValueError):
            check(Path(self.temporary.name) / 'absent.json')
        with self.assertRaises(ValueError):
            check_both(Path(self.temporary.name))

    def test_wrong_adapter_class_fails(self):
        is_software = self.original['adapter']['adapter_class'] == 'software_wgsl'
        self.write(self.original)
        with self.assertRaises(ValueError):
            check(self.path, software=not is_software, hardware=is_software)
        self.rejects(lambda v: v['adapter'].update(
            adapter_class='hardware_gpu' if is_software else 'software_wgsl'), 'class relabel')

    def test_lost_admission_and_invalid_projection_fail(self):
        def first_admitted(value):
            lane = value['lanes'][0]
            return next(r for r in lane['interval_rates_90'] if r['admitted_216'])

        self.rejects(lambda v: v['admissions_216'].pop(), 'lost admission')
        self.rejects(lambda v: v['lanes'][0]['admissions_216'].clear(), 'lost lane admission')
        self.rejects(lambda v: v['metadata']['large_scale']['quantization_admissions'].clear(),
                     'lost #207 metadata admission')
        self.rejects(lambda v: first_admitted(v).update(gpu_rate_bits='80000000'), '-0.0 projection')
        self.rejects(lambda v: first_admitted(v).update(gpu_rate_bits='b3a6f1ab'), 'signed rate')
        self.rejects(lambda v: v['admissions_216'][0].update(bound_exact='1/2'), 'wrong bound')
        self.rejects(lambda v: v['admissions_216'][0]['current_source']['packing'].update(
            message_sha256='0' * 64), 'wrong message identity')

    def test_comparison_and_identity_mutations_fail(self):
        mutations = {
            'stale revision': lambda v: v['candidate'].update(revision='0' * 40),
            'dirty worktree': lambda v: v['candidate'].update(worktree_clean=False),
            'shader': lambda v: v['shaders'].update(darain='0' * 64),
            'grib input': lambda v: v['inputs']['grib_files_sha256'].update(
                {next(iter(v['inputs']['grib_files_sha256'])): '0' * 64}),
            'two submissions': lambda v: v.update(submissions=2),
            'host transfer': lambda v: v.update(host_transfers_between_stages=1),
            'cpu stage order': lambda v: v['stages'].reverse(),
            'missing member': lambda v: v['lanes'][1]['members']['rows'].pop(),
            'member value': lambda v: v['lanes'][1]['members']['rows'][5].update(gpu=1e3),
            'missing interval row': lambda v: v['lanes'][0]['interval_rates_90'].pop(),
            'interval value': lambda v: v['lanes'][0]['interval_rates_90'][3].update(
                gpu_rate_bits='447a0000'),  # 1000 mm/h
            'member timestamp': lambda v: v['metadata']['members'][0].update(
                valid_time_epoch_seconds=0),
            'missing metadata interval': lambda v: v['metadata']['intervals'].pop(),
            'verdict': lambda v: v.update(verdict='failed'),
        }
        for label, mutate in mutations.items():
            self.rejects(mutate, label)


if __name__ == '__main__':
    unittest.main()
