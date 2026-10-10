#!/usr/bin/env python3
"""Adversarial checks on real #184 evidence; requires a device-produced report."""
import copy
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from check_shared_height_gpu import check


class EvidenceTests(unittest.TestCase):
    """Reject false success from incomplete or tampered recorded device execution."""

    def test_reject_incomplete_evidence(self):
        """Remove independent proof surfaces from an actual passing record."""
        import os
        directory = Path(os.environ.get('FLEXPART_GPU_SHARED_HEIGHT_EVIDENCE',
                                        'target/ci-gate/shared-height-gpu'))
        original = json.loads((directory / 'comparison.json').read_text())
        check(directory / 'comparison.json')
        with tempfile.TemporaryDirectory(dir=directory) as temporary:
            output = Path(temporary) / 'comparison.json'
            shutil.copyfile(directory / 'inputs.json', output.with_name('inputs.json'))
            for mutation in ('missing_row', 'duplicate', 'nonfinite', 'wrong_value',
                             'missing_adapter', 'skipped', 'wrong_shader', 'wrong_source',
                             'wrong_target', 'wrong_pin', 'wrong_adapter_class',
                             'wrong_backend', 'wrong_device_type', 'missing_candidate',
                             'wrong_case', 'wrong_count', 'missing_metrics', 'wrong_metrics',
                             'reported_failure', 'wrong_policy', 'wrong_target_hash',
                             'wrong_selection', 'wrong_sign', 'wrong_height_reference',
                             'wrong_layout', 'wrong_consumer', 'wrong_geometry', 'wrong_algorithm'):
                with self.subTest(mutation=mutation):
                    record = copy.deepcopy(original)
                    if mutation == 'missing_row':
                        record['rows'].pop()
                    elif mutation == 'duplicate':
                        record['rows'][-1] = record['rows'][0]
                    elif mutation == 'nonfinite':
                        record['rows'][0]['actual_m_s'] = float('nan')
                    elif mutation == 'wrong_value':
                        record['rows'][0]['actual_m_s'] += 1
                    elif mutation == 'missing_adapter':
                        record['gpu_evidence']['execution']['adapter'] = None
                    elif mutation == 'skipped':
                        record['gpu_evidence']['execution']['status'] = 'skipped'
                    elif mutation == 'wrong_shader':
                        record['gpu_evidence']['candidate']['shader_sha256'] = '0' * 64
                    elif mutation == 'wrong_source':
                        record['sources'][1]['snapshot_sha256'] = '0' * 64
                    elif mutation == 'wrong_target':
                        record['actual_target_height_m_agl'][1] += 1
                    elif mutation == 'wrong_pin':
                        record['gpu_evidence']['oracle']['revision'] = '0' * 40
                    elif mutation == 'wrong_adapter_class':
                        adapter = record['gpu_evidence']['execution']['adapter']
                        adapter['adapter_class'] = ('hardware_gpu' if adapter['device_type'] == 'cpu' else 'software_wgsl')
                    elif mutation == 'wrong_backend':
                        record['gpu_evidence']['execution']['adapter']['backend'] = 'empty'
                    elif mutation == 'wrong_device_type':
                        record['gpu_evidence']['execution']['adapter']['device_type'] = 'unknown'
                    elif mutation == 'missing_candidate':
                        record['gpu_evidence']['candidate']['implementation_id'] = ''
                    elif mutation == 'wrong_case':
                        record['gpu_evidence']['case_id'] = 'another_case'
                    elif mutation == 'wrong_count':
                        record['gpu_evidence']['comparison']['candidate_value_count'] = 0
                    elif mutation == 'missing_metrics':
                        record['gpu_evidence']['comparison']['max_absolute_error'] = None
                    elif mutation == 'wrong_metrics':
                        record['gpu_evidence']['comparison']['max_relative_error'] = 1.0
                    elif mutation == 'reported_failure':
                        record['gpu_evidence']['comparison']['first_failure'] = {'kind': 'tolerance_exceeded'}
                    elif mutation == 'wrong_policy':
                        record['gpu_evidence']['comparison']['policy']['absolute_tolerance'] = 1.0
                    elif mutation == 'wrong_consumer':
                        record['consumer_shader_sha256'] = '0' * 64
                    elif mutation in ('wrong_geometry', 'wrong_algorithm'):
                        source = record['sources'][1]
                        if mutation == 'wrong_geometry':
                            source['geometry_identity'] = 'another geometry'
                        else:
                            source['geometry_provenance']['height_algorithm_id'] = 'another algorithm'
                        record['resource_metadata'][1]['source'] = copy.deepcopy(source)
                    elif mutation in ('wrong_target_hash', 'wrong_selection'):
                        key = 'identity_sha256' if mutation == 'wrong_target_hash' else 'selection_rule'
                        record['target'][key] = 'wrong'
                        for metadata in record['resource_metadata']:
                            metadata['target'][key] = 'wrong'
                    else:
                        key, value = {
                            'wrong_sign': ('signs', ['positive_northward', 'positive_eastward']),
                            'wrong_height_reference': ('height_reference', 'above_mean_sea_level'),
                            'wrong_layout': ('storage_layout', 'y fastest'),
                        }[mutation]
                        record['resource_metadata'][1][key] = value
                    output.write_text(json.dumps(record))
                    with self.assertRaises(ValueError):
                        check(output)

    def test_reject_different_oracle_inputs_with_consistent_hashes(self):
        """Self-consistent candidate hashes cannot substitute for oracle input equality."""
        import os
        directory = Path(os.environ.get('FLEXPART_GPU_SHARED_HEIGHT_EVIDENCE',
                                        'target/ci-gate/shared-height-gpu'))
        original = json.loads((directory / 'comparison.json').read_text())
        original_inputs = json.loads((directory / 'inputs.json').read_text())
        mutations = ('wind_u', 'wind_v', 'wind_u10m', 'wind_v10m', 'surface_pressure',
                     'temperature', 'specific_humidity', 'temperature2m', 'dewpoint2m',
                     'orography', 'coefficients', 'time')
        with tempfile.TemporaryDirectory(dir=directory) as temporary:
            output = Path(temporary) / 'comparison.json'
            for mutation in mutations:
                with self.subTest(mutation=mutation):
                    record = copy.deepcopy(original)
                    inputs = copy.deepcopy(original_inputs)
                    snapshot = inputs[1]
                    if mutation == 'coefficients':
                        snapshot['vertical_coordinate']['hybrid_a_interface_pa'][0] += 1
                    elif mutation == 'time':
                        for field in snapshot['fields']:
                            field['time']['valid_time_epoch_seconds'] += 1
                    else:
                        field = next(f for f in snapshot['fields'] if f['id'] == mutation)
                        field['values'][0] += 1
                    serialized = json.dumps(snapshot, separators=(',', ':'))
                    record['serialized_sources'][1] = serialized
                    digest = hashlib.sha256(serialized.encode()).hexdigest()
                    source = record['sources'][1]
                    old_digest = source['snapshot_sha256']
                    source['snapshot_sha256'] = digest
                    source['geometry_provenance']['source_snapshot_sha256'] = digest
                    source['geometry_identity'] = source['geometry_identity'].replace(old_digest, digest)
                    source['time'] = next(f['time'] for f in snapshot['fields'] if f['id'] == 'wind_u')
                    record['resource_metadata'][1]['source'] = copy.deepcopy(source)
                    input_bytes = json.dumps(inputs, separators=(',', ':')).encode()
                    output.with_name('inputs.json').write_bytes(input_bytes)
                    record['gpu_evidence']['candidate']['input_sha256'] = hashlib.sha256(input_bytes).hexdigest()
                    output.write_text(json.dumps(record))
                    with self.assertRaisesRegex(ValueError, 'oracle'):
                        check(output)


if __name__ == '__main__':
    unittest.main()
