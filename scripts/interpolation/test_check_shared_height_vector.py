#!/usr/bin/env python3
"""Reject incomplete or self-consistent corrupted #190 device evidence."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest

from check_shared_height_vector import check, ROOT


class QueryEvidenceTests(unittest.TestCase):
    """Exercise the issue-owned checker against actual retained device output."""

    def setUp(self):
        source = Path(os.environ.get('FLEXPART_GPU_SHARED_VECTOR_EVIDENCE',
                                     ROOT / 'target/ci-gate/shared-height-vector'))
        self.original = json.loads((source / 'report.json').read_text())
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name) / 'report.json'
        self.path.with_name('inputs.json').write_bytes((source / 'inputs.json').read_bytes())

    def write(self, value):
        self.path.write_text(json.dumps(value))

    def test_complete_real_device_evidence(self):
        self.write(self.original)
        check(self.path)

    def test_incomplete_duplicate_and_nonfinite_rows(self):
        for mutation in ('missing', 'duplicate', 'nonfinite', 'expected', 'limit'):
            value = copy.deepcopy(self.original)
            if mutation == 'missing':
                value['rows'].pop()
            elif mutation == 'duplicate':
                value['rows'][1] = value['rows'][0]
            elif mutation == 'nonfinite':
                value['rows'][0]['actual_m_s'] = float('nan')
            elif mutation == 'expected':
                value['rows'][0]['oracle_m_s'] += 1
            else:
                value['rows'][0]['allowed_difference_m_s'] = 100
            self.write(value)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                check(self.path)

    def test_identity_execution_and_fatal_failures(self):
        paths = [('evidence', 'candidate', 'revision'), ('evidence', 'candidate', 'shader_sha256'),
                 ('evidence', 'execution', 'status'), ('evidence', 'oracle', 'output_sha256'),
                 ('sources', 0, 'snapshot_sha256'), ('targets', 1, 'identity_sha256'),
                 ('resident_metadata', 0, 'shared_height', 'preparation'),
                 ('w_sources', 0, 'correction_algorithm'), ('w_sources', 0, 'interface_input_sha256'),
                 ('w_sources', 0, 'horizontal_sha256'), ('w_sources', 0, 'prepared_uv_sha256'),
                 ('w_sources', 0, 'normalization', 'algorithm_id')]
        for path in paths:
            value = copy.deepcopy(self.original)
            selected = value
            for key in path[:-1]:
                selected = selected[key]
            selected[path[-1]] = 'invalid'
            self.write(value)
            with self.subTest(path=path), self.assertRaises(ValueError):
                check(self.path)
        value = copy.deepcopy(self.original)
        next(c for c in value['negative_cases'] if c['case'] == 'failed-w')['result']['output'][0] = [1,2,3,1]
        self.write(value)
        with self.assertRaises(ValueError):
            check(self.path)

    def test_vector_consumer_and_independent_statuses(self):
        for mutation in ('missing-vector','pending-w','inactive-write','skipped','software-class'):
            value=copy.deepcopy(self.original)
            if mutation=='missing-vector':value['vector_rows'].pop()
            elif mutation=='pending-w':value['final_w_status'][0][8]=1
            elif mutation=='inactive-write':
                next(c for c in value['negative_cases'] if c['case']=='inactive-tail')['result']['output'][-1]=[1,2,3,1]
            elif mutation=='skipped':value['evidence']['execution']['status']='skipped'
            else:value['evidence']['execution']['adapter']['adapter_class']='not_executed'
            self.write(value)
            with self.subTest(mutation=mutation),self.assertRaises(ValueError):check(self.path)

    def test_self_consistent_query_sources_must_match_prepared_resources(self):
        for field in ('geometry_identity', 'input_sha256'):
            value = copy.deepcopy(self.original)
            value['sources'][1][field] = 'f'*64
            for name in ('host_metadata', 'resident_metadata'):
                for metadata in value[name]:
                    if field == 'geometry_identity':
                        metadata['geometry_identity'][1] = 'f'*64
                    else:
                        metadata['shared_height']['source_input_sha256'][1] = 'f'*64
            self.write(value)
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'query source differs'):
                check(self.path)

    def test_query_physical_bracket_and_layout_metadata_are_required(self):
        paths = [('host_metadata', 0, 'sign'),
                 ('resident_metadata', 0, 'horizontal_staggering'),
                 ('resident_metadata', 0, 'horizontal_grid', 'dx_deg'),
                 ('host_metadata', 0, 'geometry_provenance', 0, 'height_algorithm_id'),
                 ('resident_metadata', 5, 'temporal_bracket', 'weights', 'dt1_seconds'),
                 ('host_metadata', 1, 'temporal_bracket', 'sampling_basis'),
                 *[('resident_metadata', 0, key) for key in
                   ('capacity', 'active_count', 'component_count', 'lane_stride')],
                 ('host_metadata', 0, 'value_count'),
                 ('host_metadata', 0, 'resolved_heights_agl_m', 0)]
        for path in paths:
            value = copy.deepcopy(self.original)
            selected = value
            for key in path[:-1]:
                selected = selected[key]
            selected[path[-1]] = 2
            self.write(value)
            with self.subTest(path=path), self.assertRaises(ValueError):
                check(self.path)

    def test_native_interface_w_requires_the_specific_unsupported_route(self):
        value = copy.deepcopy(self.original)
        next(c for c in value['negative_cases'] if c['case'] == 'native-interface-w')['rejection'] = 'missing W'
        self.write(value)
        with self.assertRaisesRegex(ValueError, 'unsupported geometry boundary'):
            check(self.path)


if __name__ == '__main__':
    unittest.main()
