#!/usr/bin/env python3
"""Mutation regressions for genuine flagged gas-carrier original evidence."""
import copy
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

import verify_flagged_top_boundary_oracle as oracle
from compare_top_boundary_builds import compare


class FlaggedTopBoundaryEvidenceTests(unittest.TestCase):
    def test_complete_fixture_and_v1_bytes(self):
        result = oracle.audit_fixture()
        self.assertEqual(result['advance_cases'], 64)
        self.assertEqual(result['original_builds'], 2)
        self.assertTrue(result['v1_unchanged'])

    def reject_raw(self, mutate):
        rows = (oracle.FIXTURE / 'output.txt').read_text().splitlines()
        mutate(rows)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'output.txt'
            path.write_text('\n'.join(rows) + '\n')
            with self.assertRaises((ValueError, KeyError, IndexError)):
                oracle.decode(path)

    def test_missing_duplicate_and_unresolved_rows(self):
        for tag in ('ADVANCE', 'CARRIER', 'OBS', 'GRID'):
            with self.subTest(tag=tag, fault='missing'):
                self.reject_raw(lambda rows: rows.pop(next(i for i,r in enumerate(rows) if r.startswith(tag+' '))))
            with self.subTest(tag=tag, fault='duplicate'):
                self.reject_raw(lambda rows: rows.append(next(r for r in rows if r.startswith(tag+' '))))

    def test_missing_direction_geometry_epoch_scenario(self):
        for column in (2, 3, 4, 5):
            def mutate(rows):
                index = next(i for i,r in enumerate(rows) if r.startswith('ADVANCE '))
                fields = rows[index].split()
                fields[column] = '99'
                rows[index] = ' '.join(fields)
            with self.subTest(column=column):
                self.reject_raw(mutate)

    def test_disabled_flag_nonzero_velocity_malformed_bits_and_nonfinite(self):
        for column, value in ((9, 'F'), (7, '1.0'), (6, 'NaN'), (11, '0000000000000001')):
            def mutate(rows):
                index = next(i for i,r in enumerate(rows) if r.startswith('OBS '))
                fields = rows[index].split()
                fields[column] = value
                rows[index] = ' '.join(fields)
            with self.subTest(column=column):
                self.reject_raw(mutate)

    def test_consistent_nonzero_velocity_is_rejected(self):
        def mutate(rows):
            index = next(i for i,r in enumerate(rows) if r.startswith('OBS '))
            fields = rows[index].split()
            fields[7] = '1.0'
            fields[14] = '3F800000'
            rows[index] = ' '.join(fields)
        self.reject_raw(mutate)

    def test_decimal_f32_value_must_equal_the_observed_bits_exactly(self):
        def mutate(rows):
            index = next(i for i,r in enumerate(rows) if r.startswith('OBS 1 INPUT '))
            fields = rows[index].split()
            fields[8] = '1e-50'
            rows[index] = ' '.join(fields)
        self.reject_raw(mutate)

    def test_reordered_original_stage_sequence_is_rejected(self):
        def mutate(rows):
            first = next(i for i,r in enumerate(rows) if r.startswith('OBS 1 CURRENT_IN '))
            rows[first], rows[first+1] = rows[first+1], rows[first]
        self.reject_raw(mutate)

    def test_missing_gas_carrier_provenance(self):
        for column, value in ((4, '2'), (6, '2500.0'), (13, 'F')):
            def mutate(rows):
                index = next(i for i,r in enumerate(rows) if r.startswith('CARRIER '))
                fields = rows[index].split()
                fields[column] = value
                rows[index] = ' '.join(fields)
            with self.subTest(column=column):
                self.reject_raw(mutate)

    def test_sticky_stop_cannot_be_cleared(self):
        def mutate(rows):
            index = next(i for i,r in enumerate(rows) if r.startswith('OBS 4 FINAL '))
            fields = rows[index].split()
            fields[10] = 'F'
            rows[index] = ' '.join(fields)
        self.reject_raw(mutate)

    def mutate_fixture(self, callback):
        with tempfile.TemporaryDirectory() as tmp:
            fixture = Path(tmp) / 'v2'
            shutil.copytree(oracle.FIXTURE, fixture)
            callback(fixture)
            manifest = json.loads((fixture/'manifest.json').read_text())
            for name in manifest['files_sha256']:
                if (fixture/name).is_file():
                    manifest['files_sha256'][name] = oracle.digest(fixture/name)
            (fixture/'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises((ValueError, KeyError, OSError, TypeError)):
                oracle.audit_fixture(fixture)

    def test_pair_requires_two_independent_executions_and_correct_provenance(self):
        mutations = [
            lambda pair: pair['runs'].pop(),
            lambda pair: pair['runs'].append(copy.deepcopy(pair['runs'][0])),
            lambda pair: pair['runs'].__setitem__(1, copy.deepcopy(pair['runs'][0])),
            lambda pair: pair['runs'][1].pop('build_origin'),
            lambda pair: pair['runs'][0]['provenance'].update(pinned_commit='0'*40),
            lambda pair: pair['runs'][0]['provenance']['sources_sha256'].update({'src/advance_mod.f90':'0'*64}),
            lambda pair: pair['runs'][0]['provenance'].update(unwrapped_control_exact=False),
            lambda pair: pair['runs'][0]['provenance']['artifacts_sha256'].pop('build/w-production-oracle'),
            lambda pair: pair['runs'][0].update(report_sha256='0'*64),
            lambda pair: pair['runs'][0]['provenance'].update(verified_call_edges=[]),
        ]
        for index, mutate in enumerate(mutations):
            def callback(fixture):
                path = fixture / 'reproducibility.json'
                pair = json.loads(path.read_text())
                mutate(pair)
                path.write_text(json.dumps(pair))
            with self.subTest(index=index):
                self.mutate_fixture(callback)

    def test_changed_expected_data_hashes_and_field_manifest(self):
        self.mutate_fixture(lambda f: (f/'output.txt').write_bytes((f/'output.txt').read_bytes()+b'\n'))
        self.mutate_fixture(lambda f: (f/'input.txt').write_text('invented inputs'))
        self.mutate_fixture(lambda f: (f/'scenarios.json').write_text('{}'))
        self.mutate_fixture(lambda f: (f/'report.json').unlink())
        with tempfile.TemporaryDirectory() as tmp:
            fixture=Path(tmp)/'v2'
            shutil.copytree(oracle.FIXTURE,fixture)
            (fixture/'README.md').write_text('changed without updating digest')
            with self.assertRaisesRegex(ValueError,'mismatched fixture hash'):
                oracle.audit_fixture(fixture)

    def test_missing_versioned_explanation_is_rejected(self):
        def mutate(fixture):
            path = fixture / 'manifest.json'
            manifest = json.loads(path.read_text())
            manifest['version_explanation'] = 'unexplained changed expectations'
            path.write_text(json.dumps(manifest))
        self.mutate_fixture(mutate)

    def test_scenario_declarations_must_match_executed_cases(self):
        mutations = {'declared_combinations': 0, 'directions': [-1], 'geometries': [0],
                     'epochs_1200_s': [0], 'scenario_ids': [1], 'signed_step_s': '600',
                     'source_times_s': 'invented', 'species_selection': 'species 2',
                     'schema': {'id': 'invented', 'version': 2}}
        for key, value in mutations.items():
            def mutate(fixture):
                path = fixture / 'scenarios.json'
                scenarios = json.loads(path.read_text())
                scenarios[key] = value
                path.write_text(json.dumps(scenarios))
            with self.subTest(field=key):
                self.mutate_fixture(mutate)

    def test_v1_cannot_change_even_with_a_recomputed_digest(self):
        with tempfile.TemporaryDirectory() as tmp:
            fixture=Path(tmp)/'v1'
            shutil.copytree(oracle.V1,fixture)
            (fixture/'report.json').write_text('{}')
            with self.assertRaisesRegex(ValueError,'frozen v1 fixture changed'):
                oracle.audit_v1(fixture)

    def test_two_relocated_copies_of_one_build_are_rejected(self):
        report=json.loads((oracle.FIXTURE/'report.json').read_text())
        with patch('compare_top_boundary_builds.retained', return_value=(report,'/tmp/same-build')):
            with self.assertRaisesRegex(ValueError,'relocated copies'):
                compare(Path('first'),Path('second'))


if __name__ == '__main__':
    unittest.main()
