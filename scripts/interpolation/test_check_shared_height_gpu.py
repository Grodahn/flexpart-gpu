#!/usr/bin/env python3
"""Adversarial checks on real #184 evidence; requires a device-produced report."""
import copy
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
                             'wrong_target', 'wrong_pin'):
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
                    else:
                        record['gpu_evidence']['oracle']['revision'] = '0' * 40
                    output.write_text(json.dumps(record))
                    with self.assertRaises(ValueError):
                        check(output)


if __name__ == '__main__':
    unittest.main()
