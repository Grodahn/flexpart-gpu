#!/usr/bin/env python3
"""Require two complete fresh #186 builds and exact frozen scientific reproduction."""

import argparse
import json
import re
from pathlib import Path

from prepare_interior_w_oracle import (
    ROOT, decode, decode_input, investigate, require, sha256, canonical_text_sha256,
    VARIANTS, ABSOLUTE_TOLERANCE_M_S, RELATIVE_TOLERANCE,
)


def verified_run(directory):
    """Bind each accepted report to all retained executable, source and raw bytes."""
    result=json.loads((directory/'report.json').read_text())
    require(result['schema']=={'id':'flexpart-gpu.interior-w-research','version':1}, 'wrong research schema')
    require(result['verdict']=='independent_x_y_interior_correction_verified', 'missing scientific verdict')
    require(result['comparison_policy']=={'owner':'issue #80','absolute_m_s':ABSOLUTE_TOLERANCE_M_S,
                                         'relative':RELATIVE_TOLERANCE}, 'changed velocity policy')
    provenance=result['provenance']
    require(set(provenance['research_sources_sha256'])=={
        *['scripts/interpolation/'+p for p in ('interior_w_oracle.f90','prepare_interior_w_oracle.py',
          'test_interior_w_oracle.py','run_shared_height_oracle.py','shared_height_oracle.sh',
          'prepare_shared_height_oracle.py','w_production_oracle.sh','prepare_w_production_oracle.py')],
        'scripts/oracle_build_cache.py','reference/flexpart-11.1.json'}, 'incomplete research source inventory')
    require(re.fullmatch(r'sha256:[0-9a-f]{64}',provenance['docker_image_id']), 'invalid immutable image identity')
    require(provenance['compiler'].startswith('GNU Fortran') and provenance['initializer_binding_verified'] and
            provenance['driver_flags']==['-O0','-fopenmp','-mcmodel=large'], 'incomplete compiler/driver evidence')
    require(provenance['build_profile']==json.loads((ROOT/'reference/flexpart-11.1.json').read_text())['execution_profile'],
            'wrong pinned build profile')
    expected_artifacts={'input.txt','output.txt','full-build.log','driver-build-run.log',
                        'checkout-before.txt','checkout-after.txt',
                        *['build/'+p for p in ('w-production-oracle','w-production-oracle-driver.o',
                          'w-production-oracle.nm','w-production-oracle.call-sites',
                          'w-production-oracle.link-map','linked-objects.txt','compiler-identity.txt')]}
    require(set(provenance['artifacts_sha256'])==expected_artifacts, 'incomplete retained artifact inventory')
    linked=(directory/'build/linked-objects.txt').read_text().splitlines()
    require(linked==sorted(set(linked)) and len(linked)>3 and 'FLEXPART.o' not in linked, 'wrong object inventory')
    require(set(provenance['linked_objects_sha256'])==set(linked), 'incomplete linked object hashes')
    require(set(provenance['sources_sha256'])=={'src/'+p for p in
        ('verttransform_mod.f90','windfields_mod.f90','interpol_mod.f90','par_mod.f90',
         'getfields_mod.f90','makefile_gfortran')}, 'incomplete source hashes')
    for group,base in [('artifacts_sha256',directory),
                       ('linked_objects_sha256',directory/'build/upstream'),
                       ('sources_sha256',directory/'build/upstream')]:
        require(bool(provenance[group]), 'empty artifact set')
        for key,expected in provenance[group].items():
            relative=Path(key).name if group=='sources_sha256' else key
            path=(base/relative).resolve()
            require(path.is_relative_to(base.resolve()) and path.is_file(), 'missing/unsafe artifact '+key)
            require(sha256(path)==expected, 'corrupt artifact '+key)
    for path,expected in provenance['research_sources_sha256'].items():
        require(canonical_text_sha256(ROOT/path)==expected, 'changed research source '+path)
    pin=json.loads((ROOT/'reference/flexpart-11.1.json').read_text())['pinned_commit']
    require(provenance['pinned_commit']==pin and provenance['checkout_clean_before_after'], 'wrong pristine identity')
    for name in ('checkout-before.txt','checkout-after.txt'):
        require((directory/name).read_text()==pin+'\n', 'incomplete pristine check')
    inputs=decode_input(directory/'input.txt')
    decoded=decode(directory/'output.txt')
    for key,value in investigate(inputs,decoded).items():
        require(result['scientific'][key]==value, 'changed decoded result '+key)
    require(result['scientific']['decoded_inputs']==inputs, 'changed decoded input')
    require(result['scientific']['geometry']==decoded[0] and
            result['scientific']['native_rows']==[{'key':list(k),'values':v} for k,v in sorted(decoded[1].items())] and
            result['scientific']['shared_rows']==[{'key':list(k),'height_uv_w':v} for k,v in sorted(decoded[2].items())] and
            result['scientific']['queries']==[{'key':list(k),'time_x_y_agl_asl_uv_w':v} for k,v in sorted(decoded[3].items())],
            'changed raw/decoded records')
    return result


def compare(first, second):
    """Independent build directories must reproduce complete raw and decoded science."""
    require(first.resolve()!=second.resolve(), 'two distinct fresh build directories required')
    runs=[verified_run(p) for p in (first,second)]
    require(runs[0]['scientific']==runs[1]['scientific'], 'scientific values do not reproduce')
    require(sha256(first/'output.txt')==sha256(second/'output.txt'), 'raw values do not reproduce')
    for key in ('pinned_commit','sources_sha256','research_sources_sha256','compiler','docker_image_id','build_profile'):
        require(runs[0]['provenance'][key]==runs[1]['provenance'][key], 'different scientific build identity '+key)
    require(runs[0]['comparison_policy']==runs[1]['comparison_policy'], 'different numerical policy')
    return {'schema':{'id':'flexpart-gpu.interior-w-reproducibility','version':1},
            'state':'PASS','validation_level':'direct_pinned_routines_synthetic_research_only',
            'exact_raw_and_decoded_reproduction':True,
            'raw_output_sha256':sha256(first/'output.txt'),
            'comparison_policy':runs[0]['comparison_policy'],
            'verifier_sha256':canonical_text_sha256(Path(__file__)),
            'builds':[{'report_sha256':sha256(p/'report.json'), 'provenance':r['provenance']}
                     for p,r in zip((first,second),runs)]}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--first',type=Path,required=True)
    parser.add_argument('--second',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    result=compare(args.first,args.second)
    require(not args.output.exists(), 'reproducibility output must be new')
    args.output.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({'state':'PASS','report':str(args.output)}))


if __name__=='__main__':
    main()
