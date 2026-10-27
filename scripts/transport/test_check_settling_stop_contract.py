#!/usr/bin/env python3
"""Reject incomplete, invented or inconsistent #203 contract matrices."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import check_settling_stop_contract as contract


class SettlingStopContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.matrix = json.loads(contract.MATRIX.read_text(encoding='utf-8'))

    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='settling-stop-contract-')
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.path = self.root / 'matrix.json'

    def write(self, matrix):
        self.path.write_text(json.dumps(matrix), encoding='utf-8')
        return self.path

    def mutate(self, callback):
        matrix = copy.deepcopy(self.matrix)
        callback(matrix)
        with self.assertRaises((ValueError, KeyError, TypeError)):
            contract.check(self.write(matrix))

    def case(self, matrix, case_id):
        return next(case for case in matrix['cases'] if case['id'] == case_id)

    def test_frozen_matrix_is_consistent_without_claiming_sources(self):
        result = contract.check(self.write(self.matrix))
        self.assertEqual(result['state'], 'PASS_SOURCE_ANCHORS_NOT_CHECKED')
        self.assertEqual(result['required_cases'], 19)
        self.assertIn('NO_SETTLING_OR_TRANSPORT_PARITY', result['scientific_verdict'])

    def test_missing_required_case(self):
        self.mutate(lambda m: m['cases'].remove(self.case(m, 'C18')))

    def test_duplicate_case(self):
        self.mutate(lambda m: m['cases'].append(copy.deepcopy(self.case(m, 'C03'))))

    def test_parity_claim(self):
        self.mutate(lambda m: m['claims'].update(numerical_settling_parity=True))

    def test_invented_oracle_value(self):
        self.mutate(lambda m: self.case(m, 'C04')['evidence']['oracle_rows'][0]['expect'].update(returned_z_m=9526.0))

    def test_wrong_row_count(self):
        self.mutate(lambda m: self.case(m, 'C04')['evidence']['oracle_rows'][0]['expect'].update(count=3))

    def test_wrong_stop_classification(self):
        self.mutate(lambda m: self.case(m, 'C04')['evidence']['oracle_rows'][0]['expect'].update(nstop=False))

    def test_wrong_representability(self):
        self.mutate(lambda m: self.case(m, 'C25')['evidence']['oracle_rows'][0]['expect'].update(f32_representable=True))

    def test_forward_stop(self):
        self.mutate(lambda m: self.case(m, 'C01')['nstop'].update(final=True))

    def test_stop_without_downstream_exclusion(self):
        self.mutate(lambda m: self.case(m, 'C13').update(downstream='eligible'))

    def test_fatal_step_publishing_stop(self):
        self.mutate(lambda m: self.case(m, 'C18')['nstop'].update(final=True))

    def test_oracle_blocked_without_gap(self):
        self.mutate(lambda m: self.case(m, 'C07')['evidence'].update(oracle_gap=None))

    def test_unknown_status(self):
        self.mutate(lambda m: self.case(m, 'C12').update(status='done'))

    def test_unknown_anchor(self):
        self.mutate(lambda m: self.case(m, 'C10')['original']['anchors'].append('advance.invented'))

    def test_stale_owner_symbol(self):
        self.mutate(lambda m: m['owners'][0].update(symbol='pub struct RemovedSpecies'))

    def test_changed_frozen_raw_output_hash(self):
        self.mutate(lambda m: m['authority'].update(frozen_raw_output_sha256='0' * 64))

    def test_source_anchor_token_mismatch(self):
        matrix = copy.deepcopy(self.matrix)
        checkout = self.root / 'flexpart'
        (checkout / 'src').mkdir(parents=True)
        for anchor in matrix['source_anchors']:
            path = checkout / anchor['file']
            if not path.exists():
                path.write_bytes(b'\n' * 4000)
        advance = (checkout / 'src/advance_mod.f90').read_bytes()
        matrix['authority']['advance_mod_sha256'] = contract.digest(advance)
        pinned = matrix['authority']['pinned_commit']
        with patch.object(contract, 'git', side_effect=lambda _, *args: pinned if args[0] == 'rev-parse' else ''):
            with self.assertRaisesRegex(ValueError, 'not in'):
                contract.check(self.write(matrix), checkout)


if __name__ == '__main__':
    unittest.main()
