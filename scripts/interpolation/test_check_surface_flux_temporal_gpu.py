#!/usr/bin/env python3
"""Adversarial checks on real #120 evidence; requires a device-produced report."""
import copy
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest

from check_surface_flux_temporal_gpu import check

ROOT = Path(__file__).resolve().parents[2]


def mutate(record, mutation):
    field = record['fields'][0]
    handoff = field['handoffs'][0]
    if mutation == 'missing_row':
        field['rows'].pop()
    elif mutation == 'duplicate_row':
        field['rows'][-1] = field['rows'][0]
    elif mutation == 'wrong_value':
        field['rows'][0]['gpu_value'] += 1
    elif mutation == 'nonfinite':
        field['rows'][0]['gpu_value'] = float('nan')
    elif mutation == 'missing_adapter':
        field['gpu_evidence']['execution']['adapter'] = None
    elif mutation == 'skipped':
        field['gpu_evidence']['execution']['status'] = 'skipped'
    elif mutation == 'cpu_replacement':
        field['gpu_evidence']['execution']['calculation_path'] = 'cpu_replacement'
    elif mutation == 'wrong_shader':
        record['shader_sha256']['temporal'] = '0' * 64
    elif mutation == 'wrong_pin':
        field['gpu_evidence']['oracle']['revision'] = '0' * 40
    elif mutation == 'wrong_verdict':
        record['verdicts'][field['canonical_field']]['interval_mean_through_instantaneous_path'] = 'PASS'
    elif mutation == 'missing_field':
        record['fields'].pop()
    elif mutation == 'wrong_basis':
        handoff['metadata']['temporal_bracket']['sampling_basis'] = 'instantaneous_state'
    elif mutation == 'wrong_sign':
        handoff['metadata']['sign'] = 'signed_scalar'
    elif mutation == 'wrong_unit':
        handoff['metadata']['unit'] = 'newton_per_square_meter'
    elif mutation == 'wrong_stage_order':
        handoff['stages'].reverse()
    elif mutation == 'wrong_source_hash':
        handoff['metadata']['source_snapshot_sha256'][0] = '0' * 64
    elif mutation == 'interval_source':
        handoff['metadata']['source_times'][0]['kind'] = 'interval_mean'
    elif mutation == 'host_readback':
        record['intermediate_host_readbacks'] = 1
    elif mutation == 'wrong_policy':
        field['gpu_evidence']['comparison']['policy']['relative_tolerance'] = 1e-3
    elif mutation == 'wrong_revision':
        record['candidate_revision'] = '0' * 40
    elif mutation == 'wrong_input':
        sources = [json.loads(s) for s in record['serialized_sources']]
        sources[0]['fields'][-1]['values'][0] += 1
        record['serialized_sources'][0] = json.dumps(sources[0], separators=(',', ':'))
    else:
        raise AssertionError(mutation)


MUTATIONS = ['missing_row', 'duplicate_row', 'wrong_value', 'nonfinite', 'missing_adapter',
             'skipped', 'cpu_replacement', 'wrong_shader', 'wrong_pin', 'wrong_verdict',
             'missing_field', 'wrong_basis', 'wrong_sign', 'wrong_unit', 'wrong_stage_order',
             'wrong_source_hash', 'interval_source', 'host_readback', 'wrong_policy',
             'wrong_revision', 'wrong_input']


class EvidenceTests(unittest.TestCase):
    """Reject false success from incomplete or tampered recorded device execution."""

    def test_reject_incomplete_evidence(self):
        directory = ROOT / os.environ.get('FLEXPART_GPU_SURFACE_FLUX_EVIDENCE',
                                          'target/ci-gate/surface-flux-temporal-gpu')
        oracle = Path(os.environ.get('FLEXPART_GPU_SURFACE_FLUX_ORACLE',
                                     ROOT / 'fixtures/interpolation/surface-flux-temporal-v1/report.json'))
        original = json.loads((directory / 'report.json').read_text(encoding='utf-8'))
        check(directory / 'report.json', oracle)
        with tempfile.TemporaryDirectory(dir=directory) as temporary:
            output = Path(temporary) / 'report.json'
            shutil.copyfile(directory / 'inputs.json', output.with_name('inputs.json'))
            for mutation in MUTATIONS:
                with self.subTest(mutation=mutation):
                    record = copy.deepcopy(original)
                    mutate(record, mutation)
                    output.write_text(json.dumps(record), encoding='utf-8')
                    with self.assertRaises((ValueError, KeyError, StopIteration)):
                        check(output, oracle)


if __name__ == '__main__':
    unittest.main()
