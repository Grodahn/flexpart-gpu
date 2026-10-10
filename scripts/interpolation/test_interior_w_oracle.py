#!/usr/bin/env python3
"""Fail-closed tests for #186's independently distinguishing scientific evidence."""

import copy
import json
import tempfile
import unittest
from pathlib import Path

from prepare_interior_w_oracle import (
    FIXTURE, ROOT, VARIANTS, decode, decode_input, investigate, contributions,
    canonical_text_sha256, sha256, ABSOLUTE_TOLERANCE_M_S, RELATIVE_TOLERANCE,
)


class InteriorEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.inputs=decode_input(FIXTURE/'input.txt')
        self.decoded=decode(FIXTURE/'output.txt')

    def test_frozen_scientific_and_source_binding(self):
        report=json.loads((FIXTURE/'report.json').read_text())
        result=investigate(self.inputs,self.decoded)
        for key,value in result.items():
            self.assertEqual(report['scientific'][key],value)
        self.assertEqual(report['scientific']['decoded_inputs'],self.inputs)
        for key,value in report['provenance']['research_sources_sha256'].items():
            self.assertEqual(canonical_text_sha256(ROOT/key),value,key)
        for name in ('input.txt','output.txt'):
            self.assertEqual(sha256(FIXTURE/name),report['provenance']['artifacts_sha256'][name])
        self.assertEqual(report['comparison_policy'],{'owner':'issue #80',
            'absolute_m_s':ABSOLUTE_TOLERANCE_M_S,'relative':RELATIVE_TOLERANCE})
        self.assertEqual(report['provenance']['pinned_commit'],
            json.loads((ROOT/'reference/flexpart-11.1.json').read_text())['pinned_commit'])

    def test_one_missing_direction_fails(self):
        changed=copy.deepcopy(self.decoded)
        for m in (1,2):
            for k in (2,3):
                changed[2]['y',m,1,1,k][-1]=changed[2]['zero',m,1,1,k][-1]
        with self.assertRaises(ValueError):
            investigate(self.inputs,changed)

    def test_opposite_sign_cancellation_fails(self):
        with self.assertRaisesRegex(ValueError,'cancellation'):
            contributions({'zero':0,'x':1,'y':-1,'full':0},'test',True)

    def test_nonadditive_full_fails(self):
        with self.assertRaisesRegex(ValueError,'nonadditive'):
            contributions({'zero':0,'x':1,'y':2,'full':4},'test',True)

    def test_changed_query_height_fails(self):
        changed=copy.deepcopy(self.inputs)
        changed['queries_time_s_x_y_bracket_fraction'][1][-1]=.4
        with self.assertRaisesRegex(ValueError,'query input/output'):
            investigate(changed,self.decoded)

    def test_double_normalized_native_evidence_fails(self):
        changed=copy.deepcopy(self.decoded)
        changed[1][1,1,1,2][7]*=changed[1][1,1,1,2][3]
        with self.assertRaisesRegex(ValueError,'normalization lineage'):
            investigate(self.inputs,changed)

    def test_tampered_public_sample_fails(self):
        changed=copy.deepcopy(self.decoded)
        changed[3]['x',2][-1]+=1
        with self.assertRaises(ValueError):
            investigate(self.inputs,changed)

    def test_missing_nonfinite_or_changed_control_fails(self):
        text=(FIXTURE/'output.txt').read_text()
        scratch=ROOT/'target/interior-w-test'; scratch.mkdir(parents=True,exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as d:
            p=Path(d)/'output.txt'
            changes=[text.replace(next(l for l in text.splitlines() if l.startswith('SHARED y '))+'\n','',1),
                     text.replace('0.0000000000000000E+000','NaN',1),
                     text.replace('CONTROL zero','CONTROL full',1)]
            for changed in changes:
                p.write_text(changed)
                with self.assertRaises(ValueError):
                    decode(p)


if __name__ == '__main__':
    unittest.main()
