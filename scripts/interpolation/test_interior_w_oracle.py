#!/usr/bin/env python3
"""Fail-closed tests for #186's independently distinguishing scientific evidence."""

import copy
import json
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from prepare_interior_w_oracle import (
    FIXTURE, ROOT, VARIANTS, decode, decode_input, investigate, contributions,
    canonical_text_sha256, sha256, ABSOLUTE_TOLERANCE_M_S, RELATIVE_TOLERANCE,
    linked_execution,
)
from compare_interior_w_builds import compare, verified_frozen, report_identity


class InteriorEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.inputs=decode_input(FIXTURE/'input.txt')
        self.decoded=decode(FIXTURE/'output.txt')

    def test_frozen_scientific_and_source_binding(self):
        report=verified_frozen()
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

    def test_fraction_rounded_to_grid_boundary_fails(self):
        changed=copy.deepcopy(self.inputs)
        changed['queries_time_s_x_y_bracket_fraction'][3][-1]=1e-12
        with self.assertRaisesRegex(ValueError,'rounded query is not strict interior'):
            investigate(changed,self.decoded)

    def test_copied_directory_is_not_an_independent_build(self):
        report=verified_frozen()
        with patch('compare_interior_w_builds.verified_run',return_value=report):
            with self.assertRaisesRegex(ValueError,'copied build origin'):
                compare(ROOT/'target/copied-first',ROOT/'target/copied-second')

    def test_missing_or_inconsistent_frozen_reproducibility_fails(self):
        scratch=ROOT/'target/interior-w-test'; scratch.mkdir(parents=True,exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as d:
            directory=Path(d)
            for name in ('report.json','input.txt','output.txt'):
                shutil.copyfile(FIXTURE/name,directory/name)
            with self.assertRaises(FileNotFoundError):
                verified_frozen(directory)
            original=json.loads((FIXTURE/'reproducibility.json').read_text())
            changes=[]
            for field,value in [('verifier_sha256','0'*64),('raw_output_sha256','0'*64),
                                ('builds',[]),('state','FAIL')]:
                changed=copy.deepcopy(original); changed[field]=value; changes.append(changed)
            changed=copy.deepcopy(original)
            changed['builds'][1]=changed['builds'][0]; changes.append(changed)
            changed=copy.deepcopy(original)
            changed['builds'][0]['report_sha256']='0'*64; changes.append(changed)
            changed=copy.deepcopy(original)
            changed['builds'][1]['provenance']['verified_call_edges'][0]['callee_address']='0x1'; changes.append(changed)
            for changed in changes:
                (directory/'reproducibility.json').write_text(json.dumps(changed))
                with self.assertRaises(ValueError):
                    verified_frozen(directory)

    def test_missing_execution_claims_fail(self):
        original=json.loads((FIXTURE/'report.json').read_text())
        for field,value in [('verified_call_edges',[]),('initializer_binding_verified',False),
                            ('fresh_build_id',''),('linked_objects_sha256',{}),
                            ('artifacts_sha256',{}),('sources_sha256',{})]:
            changed=copy.deepcopy(original); changed['provenance'][field]=value
            with self.assertRaises(ValueError):
                report_identity(changed)

    def test_retained_linkage_requires_actual_edges_initializer_and_objects(self):
        # Small disassembly records exercise the existing #80 large-model edge
        # parser; these are grammar tests, never scientific oracle values.
        symbols={'MAIN__':0x1000,'__verttransform_mod_MOD_verttransform_ecmwf_heights':0x2000,
                 '__verttransform_mod_MOD_verttransform_ecmwf_windfields':0x3000,
                 '__interpol_mod_MOD_interpol_wind':0x4000,'__interpol_mod_MOD_interpol_wind_meter':0x5000}
        nm='\n'.join(f'{address:x} T {name}' for name,address in symbols.items())
        calls='1000 <MAIN__>:\n movabs $0x0,%r11\n lea 0 # 1000 <MAIN__>\n'
        calls+=''.join(f' movabs $0x{offset:x},%rax\n call *%rax\n' for offset in (0x1000,0x2000,0x3000))
        calls+='\n4000 <__interpol_mod_MOD_interpol_wind>:\n movabs $0x0,%r11\n'
        calls+=' lea 0 # 4000 <__interpol_mod_MOD_interpol_wind>\n movabs $0x1000,%rax\n call *%rax\n'
        objects=['interpol_mod.o','par_mod.o','verttransform_mod.o','windfields_mod.o']
        origin='/tmp/flexpart-height-research.example'
        link=''.join(f'LOAD {origin}/build/upstream/{name}\n' for name in objects)
        initializer='__verttransform_mod_MOD_verttransform_init '+origin+'/build/upstream/verttransform_mod.o\n w-production-oracle-driver.o\n'
        scratch=ROOT/'target/interior-w-test'; scratch.mkdir(parents=True,exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as d:
            build=Path(d)
            (build/'w-production-oracle.nm').write_text(nm)
            (build/'linked-objects.txt').write_text('\n'.join(objects)+'\n')
            (build/'w-production-oracle.call-sites').write_text(calls)
            (build/'w-production-oracle.link-map').write_text(link+initializer)
            edges,actual_origin=linked_execution(build)
            self.assertEqual(actual_origin,origin)
            self.assertEqual(len(edges),4)
            for filename,text in [('w-production-oracle.call-sites',''),
                                  ('w-production-oracle.link-map',link),
                                  ('w-production-oracle.link-map',link.replace('LOAD '+origin+'/build/upstream/par_mod.o\n','')+initializer)]:
                (build/filename).write_text(text)
                with self.assertRaises(ValueError):
                    linked_execution(build)
                (build/'w-production-oracle.call-sites').write_text(calls)
                (build/'w-production-oracle.link-map').write_text(link+initializer)

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
