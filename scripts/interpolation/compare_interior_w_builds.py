#!/usr/bin/env python3
"""Require two complete fresh #186 builds and exact frozen scientific reproduction."""

import argparse
import hashlib
import json
import re
from pathlib import Path

from prepare_interior_w_oracle import (
    ROOT, FIXTURE, decode, decode_input, investigate, linked_execution,
    require, sha256, canonical_text_sha256, ABSOLUTE_TOLERANCE_M_S, RELATIVE_TOLERANCE,
)

VALIDATION_LEVEL = 'direct_pinned_routines_synthetic_research_only'
POLICY = {'owner':'issue #80','absolute_m_s':ABSOLUTE_TOLERANCE_M_S,'relative':RELATIVE_TOLERANCE}
STABLE_BUILD_KEYS = ('pinned_commit','sources_sha256','research_sources_sha256','compiler',
                     'docker_image_id','build_profile','driver_flags')


def report_identity(result):
    """Require the finite, complete claim and provenance inventory even in frozen records."""
    require(result['schema']=={'id':'flexpart-gpu.interior-w-research','version':1}, 'wrong research schema')
    require(result['issue']==186 and result['validation_level']==VALIDATION_LEVEL, 'wrong scientific scope')
    require(result['verdict']=='independent_x_y_interior_correction_verified', 'missing scientific verdict')
    require(result['comparison_policy']==POLICY, 'changed velocity policy')
    provenance=result['provenance']
    require(set(provenance['research_sources_sha256'])=={
        *['scripts/interpolation/'+p for p in ('interior_w_oracle.f90','prepare_interior_w_oracle.py',
          'test_interior_w_oracle.py','run_shared_height_oracle.py','shared_height_oracle.sh',
          'prepare_shared_height_oracle.py','w_production_oracle.sh','prepare_w_production_oracle.py')],
        'scripts/oracle_build_cache.py','reference/flexpart-11.1.json'}, 'incomplete research source inventory')
    require(re.fullmatch(r'sha256:[0-9a-f]{64}',provenance['docker_image_id']), 'invalid immutable image identity')
    require(provenance['compiler'].startswith('GNU Fortran') and provenance['initializer_binding_verified'] is True and
            provenance['driver_flags']==['-O0','-fopenmp','-mcmodel=large'], 'incomplete compiler/driver evidence')
    require(provenance['build_profile']==json.loads((ROOT/'reference/flexpart-11.1.json').read_text())['execution_profile'],
            'wrong pinned build profile')
    require(re.fullmatch(r'/tmp/flexpart-height-research\.[^/\s]+', provenance['fresh_build_id']),
            'missing fresh-build identity')
    expected_artifacts={'input.txt','output.txt','full-build.log','driver-build-run.log',
                        'checkout-before.txt','checkout-after.txt',
                        *['build/'+p for p in ('w-production-oracle','w-production-oracle-driver.o',
                          'w-production-oracle.nm','w-production-oracle.call-sites',
                          'w-production-oracle.link-map','linked-objects.txt','compiler-identity.txt')]}
    require(set(provenance['artifacts_sha256'])==expected_artifacts, 'incomplete retained artifact inventory')
    linked=list(provenance['linked_objects_sha256'])
    require(linked==sorted(set(linked)) and len(linked)>3 and 'FLEXPART.o' not in linked, 'wrong object inventory')
    require(set(provenance['sources_sha256'])=={'src/'+p for p in
        ('verttransform_mod.f90','windfields_mod.f90','interpol_mod.f90','par_mod.f90',
         'getfields_mod.f90','makefile_gfortran')}, 'incomplete source hashes')
    for group in ('artifacts_sha256','linked_objects_sha256','sources_sha256','research_sources_sha256'):
        require(all(isinstance(h,str) and re.fullmatch(r'[0-9a-f]{64}',h)
                    for h in provenance[group].values()), 'malformed hashes '+group)
    expected_edges=[('MAIN__','__verttransform_mod_MOD_verttransform_ecmwf_heights'),
                    ('MAIN__','__verttransform_mod_MOD_verttransform_ecmwf_windfields'),
                    ('MAIN__','__interpol_mod_MOD_interpol_wind'),
                    ('__interpol_mod_MOD_interpol_wind','__interpol_mod_MOD_interpol_wind_meter')]
    edges=provenance['verified_call_edges']
    require([(e['caller'],e['callee']) for e in edges]==expected_edges and
            all(e['verified_in_linked_executable'] is True for e in edges), 'incomplete execution claims')
    pin=json.loads((ROOT/'reference/flexpart-11.1.json').read_text())['pinned_commit']
    require(provenance['pinned_commit']==pin and provenance['checkout_clean_before_after'] is True,
            'wrong pristine identity')
    for path,expected in provenance['research_sources_sha256'].items():
        require(canonical_text_sha256(ROOT/path)==expected, 'changed research source '+path)
    return provenance


def scientific_records(result, directory):
    """Bind every decoded and derived value to the complete raw evidence."""
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


def verified_frozen(directory=FIXTURE):
    """Fail closed on absent, stale or inconsistent frozen two-build evidence."""
    result=json.loads((directory/'report.json').read_text())
    provenance=report_identity(result)
    scientific_records(result,directory)
    for name in ('input.txt','output.txt'):
        require(sha256(directory/name)==provenance['artifacts_sha256'][name], 'changed frozen raw '+name)
    paired=json.loads((directory/'reproducibility.json').read_text())
    require(paired['schema']=={'id':'flexpart-gpu.interior-w-reproducibility','version':1} and
            paired['state']=='PASS' and paired['validation_level']==VALIDATION_LEVEL and
            paired['exact_raw_and_decoded_reproduction'] is True, 'missing frozen reproducibility verdict')
    require(paired['comparison_policy']==POLICY and
            paired['verifier_sha256']==canonical_text_sha256(Path(__file__)), 'stale frozen reproducibility policy/verifier')
    require(paired['raw_output_sha256']==sha256(directory/'output.txt'), 'changed frozen paired raw output')
    builds=paired['builds']
    require(len(builds)==2, 'two frozen builds required')
    for build in builds:
        require(re.fullmatch(r'[0-9a-f]{64}',build['report_sha256']), 'invalid frozen report hash')
        build_report={**result,'provenance':build['provenance']}
        report_identity(build_report)
        expected_hash=hashlib.sha256((json.dumps(build_report,indent=2)+'\n').encode('utf-8')).hexdigest()
        require(build['report_sha256']==expected_hash, 'inconsistent frozen report/provenance hash')
        for name in ('input.txt','output.txt'):
            require(build['provenance']['artifacts_sha256'][name]==provenance['artifacts_sha256'][name],
                    'inconsistent frozen build raw '+name)
    require(builds[0]['provenance']==provenance and builds[0]['report_sha256']==sha256(directory/'report.json'),
            'frozen baseline/report mismatch')
    require(builds[0]['provenance']['fresh_build_id']!=builds[1]['provenance']['fresh_build_id'] and
            builds[0]['report_sha256']!=builds[1]['report_sha256'], 'copied frozen build evidence')
    for key in STABLE_BUILD_KEYS:
        require(builds[0]['provenance'][key]==builds[1]['provenance'][key], 'inconsistent frozen build identity '+key)
    return result


def verified_run(directory):
    """Bind each accepted report to all retained executable, source and raw bytes."""
    result=json.loads((directory/'report.json').read_text())
    provenance=report_identity(result)
    linked=(directory/'build/linked-objects.txt').read_text().splitlines()
    require(list(provenance['linked_objects_sha256'])==linked, 'incomplete linked object hashes')
    for group,base in [('artifacts_sha256',directory),
                       ('linked_objects_sha256',directory/'build/upstream'),
                       ('sources_sha256',directory/'build/upstream')]:
        require(bool(provenance[group]), 'empty artifact set')
        for key,expected in provenance[group].items():
            relative=Path(key).name if group=='sources_sha256' else key
            path=(base/relative).resolve()
            require(path.is_relative_to(base.resolve()) and path.is_file(), 'missing/unsafe artifact '+key)
            require(sha256(path)==expected, 'corrupt artifact '+key)
    edges,origin=linked_execution(directory/'build')
    require(provenance['verified_call_edges']==edges and provenance['fresh_build_id']==origin,
            'execution/build identity differs from retained linkage')
    require(provenance['compiler']==(directory/'build/compiler-identity.txt').read_text().strip(),
            'compiler identity differs from retained evidence')
    pin=json.loads((ROOT/'reference/flexpart-11.1.json').read_text())['pinned_commit']
    for name in ('checkout-before.txt','checkout-after.txt'):
        require((directory/name).read_text()==pin+'\n', 'incomplete pristine check')
    scientific_records(result,directory)
    frozen=json.loads((FIXTURE/'report.json').read_text())
    for key in ('pinned_commit','sources_sha256','research_sources_sha256','compiler','build_profile','driver_flags'):
        require(provenance[key]==frozen['provenance'][key], 'changed frozen pinned build identity '+key)
    require(result['scientific']==frozen['scientific'], 'changed frozen science')
    require(sha256(directory/'output.txt')==sha256(FIXTURE/'output.txt') and
            canonical_text_sha256(directory/'input.txt')==canonical_text_sha256(FIXTURE/'input.txt'),
            'changed frozen raw evidence')
    return result


def compare(first, second):
    """Independent build directories must reproduce complete raw and decoded science."""
    require(first.resolve()!=second.resolve(), 'two distinct fresh build directories required')
    verified_frozen()
    runs=[verified_run(p) for p in (first,second)]
    require(runs[0]['provenance']['fresh_build_id']!=runs[1]['provenance']['fresh_build_id'],
            'two independent fresh builds required; copied build origin')
    require(runs[0]['scientific']==runs[1]['scientific'], 'scientific values do not reproduce')
    require(sha256(first/'output.txt')==sha256(second/'output.txt'), 'raw values do not reproduce')
    for key in STABLE_BUILD_KEYS:
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
