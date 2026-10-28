#!/usr/bin/env python3
"""Fail-closed #213 original public-advance evidence; no candidate calculations."""
import argparse
import hashlib
import json
import math
import os
import re
import struct
import subprocess
from pathlib import Path

from verify_top_boundary_oracle import (ROOT, ARTIFACTS as V1_ARTIFACTS, TOOLS as V1_TOOLS,
                                       digest, text_digest, git, require)

FIXTURE = ROOT / 'fixtures/transport/top-boundary-v2'
V1 = ROOT / 'fixtures/transport/top-boundary-v1'
TOOLS = V1_TOOLS + [
    'scripts/transport/flagged_top_boundary_oracle.f90',
    'scripts/transport/flagged_top_observer.f90',
    'scripts/transport/flagged_top_boundary_oracle.sh',
    'scripts/transport/flagged_top_link.sh',
    'scripts/transport/run_flagged_top_boundary_oracle.py',
    'scripts/transport/compare_top_boundary_builds.py',
    'scripts/transport/verify_flagged_top_boundary_oracle.py',
]
ARTIFACTS = V1_ARTIFACTS + ['output-unwrapped.txt', 'build/flagged-top-observer.o',
                           'build/unwrapped-control', 'build/unwrapped-control.link-map']
WRAPPED = ['__interpol_mod_MOD_interpol_wind', '__interpol_mod_MOD_interpol_wind_short',
           '__particle_mod_MOD_update_z_sp', '__particle_mod_MOD_set_z_sp']


CALL_GRAPH = {
        'MAIN__': ['__advance_mod_MOD_advance', '__advance_mod_MOD_pushpartdown',
                   '__verttransform_mod_MOD_verttransform_init', '__verttransform_mod_MOD_verttransform_ecmwf_heights'],
        '__advance_mod_MOD_advance': ['__wrap_' + symbol for symbol in WRAPPED[:2]] + ['__settling_mod_MOD_get_settling'],
        **{'__wrap_' + symbol: [symbol] for symbol in WRAPPED},
    }

# Historical bytes, not recomputed expectations. Changing an oracle requires a
# new version and an explicit scientific explanation; v1 is never re-recorded.
V1_HASHES = {
    'output.txt': 'b10df540cc86ebb1afaccba6f9178deabbf4af1135c76adfc582868e2c957be2',
    'report.json': '1354922d5e37c17f9b5b4c57fbb40a4e11401f912bbf180e2f064be3ae0b09c9',
    'reproducibility.json': '6726664dfbe2d21a741346ed62f81ba9b2c44316a15112e8f1a0b182412abbe1',
}
V2_RAW_SHA256 = '427809dfaaea7d57cb8719fed97b3fa2b13c31106b822e8756f4c4e89444369b'


def audit_v1(fixture=V1):
    require({p.name for p in fixture.iterdir() if p.is_file()} == set(V1_HASHES), 'v1 file inventory changed')
    for name, expected in V1_HASHES.items():
        require(digest(fixture/name)==expected, 'frozen v1 fixture changed: '+name)


def science_digest(science):
    return hashlib.sha256(json.dumps(science,sort_keys=True,separators=(',',':')).encode()).hexdigest()


def audit_fixture(fixture=FIXTURE):
    """Recheck both retained execution inventories and immutable expected fields."""
    audit_v1()
    manifest=json.loads((fixture/'manifest.json').read_text())
    require(manifest['schema']=={'id':'flexpart-gpu.flagged-top-fixture','version':2}, 'wrong fixture version')
    require(manifest['version_explanation']=='Add genuine flagged gas-carrier public advance scenarios 1-8; v1 is unchanged.', 'missing versioned explanation')
    files=manifest['files_sha256']
    required={'output.txt','output-unwrapped.txt','report.json','reproducibility.json','scenarios.json','README.md','input.txt'}
    require(set(files)==required, 'missing fixture artifacts')
    for name, expected in files.items():
        require(digest(fixture/name)==expected, 'mismatched fixture hash: '+name)
    require(manifest['v1_sha256']==V1_HASHES, 'v1 historical identity changed')
    require(digest(fixture/'output.txt')==V2_RAW_SHA256, 'changed expected results require a new fixture version')
    require(text_digest(fixture/'input.txt')==text_digest(ROOT/'fixtures/interpolation/shared-height-v1/input.txt'), 'changed source scenario input')
    science=decode(fixture/'output.txt')
    require([line for line in (fixture/'output.txt').read_text().splitlines() if not line.startswith('OBS ')] ==
            [line for line in (fixture/'output-unwrapped.txt').read_text().splitlines() if not line.startswith('OBS ')], 'unwrapped control output mismatch')
    report=json.loads((fixture/'report.json').read_text())
    require(report['schema']=={'id':'flexpart-gpu.top-boundary-research','version':2} and report['issue']==213, 'wrong report identity')
    require(science==report['science'], 'decoded/raw mismatch')
    require(science_digest(science)==manifest['decoded_science_sha256'], 'changed decoded output hash')
    scenarios=json.loads((fixture/'scenarios.json').read_text())
    require(scenarios['cases']==[{k:c[k] for k in ('id','geometry','direction','epoch_1200_s','scenario','initial_z_m','old_w_unsigned_m_s','end_w_unsigned_m_s','x_slope_unsigned_m_s')} for c in science['cases']], 'scenario manifest coverage/inputs changed')
    require(scenarios['expected_fields']==list(science['cases'][0]), 'expected field identities changed')
    pair=json.loads((fixture/'reproducibility.json').read_text())
    require(pair['schema']=={'id':'flexpart-gpu.top-boundary-reproducibility','version':2} and pair['state']=='PASS', 'missing verified original build pair')
    require(pair['baseline_report_sha256']==digest(fixture/'report.json') and pair['raw_output_sha256']==V2_RAW_SHA256, 'pair baseline/raw identity changed')
    require(pair['verifier_sha256']==text_digest(ROOT/'scripts/transport/compare_top_boundary_builds.py'), 'stale paired verifier')
    runs=pair['runs']
    require(len(runs)==2, 'two required original builds missing')
    require(len({r['build_origin'] for r in runs})==2 and len({r['report_sha256'] for r in runs})==2, 'reused outputs are not independent execution')
    baseline=json.loads((V1/'report.json').read_text())['provenance']
    pin=json.loads((ROOT/'reference/flexpart-11.1.json').read_text())['pinned_commit']
    for run in runs:
        p=run['provenance']
        require(re.fullmatch(r'/tmp/flexpart-top-research\.[A-Za-z0-9]+/build/upstream',run['build_origin']) and p.get('build_origin')==run['build_origin'], 'missing original build origin')
        require(p['pinned_commit']==pin, 'incorrect original revision')
        require(p['sources_sha256']==baseline['sources_sha256'], 'modified or missing original sources')
        require(p['tools_sha256']=={s:text_digest(ROOT/s) for s in TOOLS}, 'stale driver/launcher/auditor identity')
        expected_edges = [[caller,callee] for caller,callees in CALL_GRAPH.items() for callee in callees]
        expected_edges += [['advance_mod.o (original relocation)','__wrap_'+symbol] for symbol in WRAPPED[2:]]
        require(p['verified_call_edges']==expected_edges, 'missing genuine public/wrapper linkage evidence')
        require(p['profile']==baseline['profile'] and p['driver_flags']==baseline['driver_flags'], 'build configuration changed')
        require(p.get('unwrapped_control_exact') is True, 'missing unwrapped control execution')
        require(set(p['artifacts_sha256'])==set(ARTIFACTS) and len(p['linked_objects_sha256'])>20, 'incomplete build inventory')
        require(p['artifacts_sha256']['output.txt']==V2_RAW_SHA256 and p['artifacts_sha256']['output-unwrapped.txt']==digest(fixture/'output-unwrapped.txt'), 'build raw hash differs')
        require(p['compiler'] and re.fullmatch(r'sha256:[0-9a-f]{64}',p['image_id']), 'missing compiler/image identity')
        for hashes in (p['artifacts_sha256'],p['linked_objects_sha256']):
            require(all(re.fullmatch('[0-9a-f]{64}',value) for value in hashes.values()), 'malformed evidence hash')
        reconstructed={**report, 'state':'PASS','provenance':p}
        raw=(json.dumps(reconstructed,indent=2)+'\n').encode()
        require(hashlib.sha256(raw).hexdigest()==run['report_sha256'], 'independent report identity does not resolve')
    require(runs[0]['provenance']==report['provenance'], 'baseline does not belong to first execution')
    require(runs[0]['provenance']['image_id']==runs[1]['provenance']['image_id'] and runs[0]['provenance']['compiler']==runs[1]['provenance']['compiler'], 'independent build environment mismatch')
    return dict(state='PASS',advance_cases=64,original_builds=2,v1_unchanged=True,
                raw_output_sha256=V2_RAW_SHA256,decoded_report_sha256=digest(fixture/'report.json'),
                scientific_verdict='GENUINE_ORIGINAL_ROUTINE_ONLY_NO_GPU_PARITY')


def decode(path):
    """Decode actual observations; stage predicates are checked only on observed states."""
    audit_v1()
    baseline = json.loads((V1 / 'report.json').read_text())['science']
    lines = path.read_text().splitlines()
    require(lines and lines[0] == 'FLEXPART_TOP_BOUNDARY_ORACLE_V2', 'incorrect raw header')
    rows = {'GRID': [], 'PUSH': [], 'ADVANCE': []}
    carriers, observations = {}, {}
    for line in lines[1:]:
        fields = line.split()
        require(bool(fields), 'empty raw row')
        tag, f = fields[0], fields[1:]
        if tag == 'OBS':
            require(len(f) == 15, 'incorrect observation width')
            row_id, stage, clock = int(f[0]), f[1], int(f[2])
            values = list(map(float, f[3:8]))
            require(all(math.isfinite(v) for v in values), 'nonfinite observation')
            require(all(v in ('T', 'F') for v in f[8:10]), 'malformed observation flags')
            for value, bits, form in zip(values, f[10:], ['d','d','d','f','f']):
                require(re.fullmatch('[0-9A-F]{' + ('16' if form=='d' else '8') + '}', bits), 'malformed raw bits')
                require(struct.pack('>' + form, value).hex().upper() == bits, 'decimal/raw bit disagreement')
            table = observations.setdefault(row_id, {})
            require(stage not in table, 'duplicate observation identity')
            table[stage] = dict(zip(['x_cells','y_cells','z_m','settling_m_s','w_m_s'], values))
            table[stage].update(time_s=clock, lsettling=f[8]=='T', nstop=f[9]=='T',
                                bits=dict(zip(['x_f64','y_f64','z_f64','settling_f32','w_f32'], f[10:])))
        elif tag == 'CARRIER':
            require(len(f)==13 and f[-1] in ('T','F'), 'malformed carrier provenance')
            row_id, point, nspec, selected, shape = map(int, f[:5])
            require(row_id not in carriers, 'duplicate carrier')
            density, diameter, cunningham, estimate, mass = map(float, f[5:10])
            require(all(math.isfinite(v) for v in (density,diameter,cunningham,estimate,mass)), 'nonfinite carrier')
            carriers[row_id] = dict(release_point=point, nspec=nspec, selected_species=selected,
                shape=shape, density_kg_m3=density, dquer_original=diameter, cunningham=cunningham,
                vsetaver_m_s=estimate, release_mass=mass, mdomainfill=int(f[10]), ipin=int(f[11]), lsettling=f[12]=='T')
        else:
            require(tag in rows, 'unknown raw row')
            integers, reals, boolean = {'GRID':(3,4,False), 'PUSH':(4,1,True), 'ADVANCE':(5,8,True)}[tag]
            require(len(f)==integers+reals+int(boolean), 'incorrect raw row width')
            row = [*map(int,f[:integers]), *map(float,f[integers:integers+reals])]
            require(all(math.isfinite(v) for v in row), 'nonfinite output')
            if boolean:
                require(f[-1] in ('T','F'), 'malformed stop')
                row.append(f[-1]=='T')
            rows[tag].append(row)
    require(rows['GRID']==baseline['rows']['GRID'] and rows['PUSH']==baseline['rows']['PUSH'], 'frozen geometry/probes changed')
    require(len(rows['ADVANCE'])==64, 'missing scenario rows')
    identities = {tuple(r[1:5]) for r in rows['ADVANCE']}
    expected = {(g,d,e,s) for g in (0,1) for d in (-1,1) for e in (0,1) for s in range(1,9)}
    require(identities==expected, 'missing or duplicate direction/geometry/epoch/scenario')
    require([r[0] for r in rows['ADVANCE']]==list(range(1,65)), 'duplicate row identity')
    require(set(carriers)==set(observations)==set(range(1,65)), 'missing or unresolved carrier/observation')
    frozen = {tuple(r[1:5]):r for r in baseline['rows']['ADVANCE']}
    columns = baseline['advance_columns']
    top, placement = baseline['target_top_m_agl'], baseline['observed_placement_m_agl']
    cases = []
    mandatory = {'INPUT','CURRENT_IN','CURRENT_OUT','PREDICTOR_RAW','ADVANCED_IN',
                 'ADVANCED_OUT','CORRECTOR_RAW','FINAL'}
    for row in rows['ADVANCE']:
        case = dict(zip(columns,row))
        row_id,g,d,e,s = row[:5]
        require(row[5:9]==frozen[g,d,e,s][5:9], 'frozen #180 scientific input changed')
        carrier, stages = carriers[row_id], observations[row_id]
        require(set(stages)>=mandatory and set(stages)<=mandatory|{'PLACE_PREDICTOR','PLACE_CORRECTOR'}, 'missing/invented stage')
        expected_carrier = dict(release_point=1,nspec=1,selected_species=1,shape=0,density_kg_m3=-1.,
                dquer_original=10.,cunningham=1.,vsetaver_m_s=struct.unpack('f',struct.pack('f',-0.01))[0],
                release_mass=1.,mdomainfill=0,ipin=0,lsettling=True)
        require(carrier==expected_carrier, 'incorrect selected-carrier provenance')
        require(all(o['lsettling'] is True and o['settling_m_s']==0 and o['bits']['settling_f32']=='00000000' for o in stages.values()), 'flag disabled or nonzero settling velocity')
        require(case['settling_m_s']==0, 'nonzero returned settling')
        for name,o in stages.items():
            require(o['time_s']==(1200*e*d if name in ('INPUT','CURRENT_IN','CURRENT_OUT','PREDICTOR_RAW','PLACE_PREDICTOR') else (1200*e+600)*d), 'signed stage time changed')
        initial, current, pred, advanced, corr, final = [stages[k] for k in ('INPUT','CURRENT_IN','PREDICTOR_RAW','ADVANCED_IN','CORRECTOR_RAW','FINAL')]
        sequence = ['INPUT','CURRENT_IN','CURRENT_OUT','PREDICTOR_RAW']
        if 'PLACE_PREDICTOR' in stages:
            sequence.append('PLACE_PREDICTOR')
        sequence += ['ADVANCED_IN','ADVANCED_OUT','CORRECTOR_RAW']
        if 'PLACE_CORRECTOR' in stages:
            sequence.append('PLACE_CORRECTOR')
        sequence.append('FINAL')
        require(list(stages)==sequence, 'original observation sequence changed')
        for before,after in [('INPUT','CURRENT_IN'),('CURRENT_IN','CURRENT_OUT'),('ADVANCED_IN','ADVANCED_OUT')]:
            require(all(stages[before][k]==stages[after][k] for k in ('x_cells','y_cells','z_m','settling_m_s','nstop','lsettling')), 'sampling changed particle state')
        require(initial['z_m']==row[5] and initial['x_cells']==0.25 and initial['y_cells']==0, 'initial position changed')
        require(not current['nstop'], 'entry nstop not reset')
        pred_hit, corr_hit = pred['z_m']>=top, corr['z_m']>=top
        pred_stop, corr_stop = d==-1 and pred_hit, d==-1 and corr_hit
        require(advanced['nstop']==pred_stop, 'predictor stop branch disagrees')
        require(corr['nstop']==pred_stop and final['nstop']==(pred_stop or corr_stop), 'nstop not sticky within call')
        for name,hit,raw,after in [('PREDICTOR',pred_hit,pred,advanced), ('CORRECTOR',corr_hit,corr,final)]:
            placed = d==1 and hit
            require(('PLACE_'+name in stages)==placed, 'ordinary placement call missing/invented')
            require(after['z_m']==(placement if placed else raw['z_m']), 'stop retained height/ordinary placement disagrees')
        require(final['z_m']==case['returned_z_m'] and final['x_cells']==case['returned_x_cells'] and final['y_cells']==case['returned_y_cells'] and final['nstop']==case['nstop'], 'returned state differs from observation')
        require(0.5<final['x_cells']<1 and final['y_cells']==0, 'horizontal domain route changed')
        require(pred_hit == (s in (2,3,4,6,7)), 'frozen predictor encounter coverage changed')
        require(corr_hit == (s in (2,3,5,6,7) if d==-1 else s in (5,7)), 'frozen corrector encounter coverage changed')
        case.update(settling_enabled=True, carrier=carrier, stages=stages,
                    predictor_stop=pred_stop, corrector_stop=corr_stop,
                    predictor_encounter=pred_hit, corrector_encounter=corr_hit,
                    normal_placement=any(k.startswith('PLACE_') for k in stages),
                    original_stop_branch=pred_stop or corr_stop,
                    source_identity='pinned advance_mod.f90; original public advance',
                    target_identity='original verttransform_init; run-owned height(nz)',
                    execution='returned_public_advance', scientific_verdict='PASS_ORIGINAL_FLAGGED_ZERO_VELOCITY')
        cases.append(case)
    for c in cases:
        peer=next(p for p in cases if (p['geometry'],p['direction'],p['epoch_1200_s'],p['scenario'])==(1-c['geometry'],c['direction'],c['epoch_1200_s'],c['scenario']))
        require(c['returned_z_m']==peer['returned_z_m'] and c['nstop']==peer['nstop'], 'native top leaked into action')
    return dict(target_top_m_agl=top, observed_placement_m_agl=placement, grid_columns=baseline['grid_columns'],
                push_columns=baseline['push_columns'], advance_columns=columns, rows=rows, cases=cases)


def verify_linkage(build):
    """Require actual call instructions and the original defining object's binding."""
    disassembly = (build / 'executable.disassembly').read_text()
    symbols = {fields[2]: int(fields[0],16) for line in (build / 'w-production-oracle.nm').read_text().splitlines()
               if len(fields := line.split()) == 3}
    edges = []
    for caller, callees in CALL_GRAPH.items():
        body = re.search(r'^[0-9a-f]+ <' + re.escape(caller) + r'>:\n(.*?)(?=\n[0-9a-f]+ <|\Z)',
                         disassembly, re.M | re.S)
        require(body is not None, f'missing executable routine {caller}')
        for callee in callees:
            direct = re.search(r'\bcall\s+[0-9a-f]+ <' + re.escape(callee) + r'>', body[1])
            # Same GNU large-model GOT-relative resolution used by #80's verifier.
            immediate = re.search(r'movabs \$0x([0-9a-f]+),%r11', body[1])
            anchor = re.search(r'lea\s+[^#]+# ([0-9a-f]+) <', body[1])
            indirect = None
            if immediate and anchor and callee in symbols:
                base = int(immediate[1],16) + int(anchor[1],16)
                displacement = (symbols[callee] - base) % (1 << 64)
                indirect = re.search(rf'movabs \$0x{displacement:x},%rax(?:(?!movabs).){{0,1800}}?call\s+\*%rax',
                                     body[1], re.S)
            require(direct is not None or indirect is not None, f'no genuine call {caller} -> {callee}')
            edges.append([caller, callee])
    link_map = (build / 'w-production-oracle.link-map').read_text()
    require(re.search(r'^__advance_mod_MOD_advance\s+[^\n]*/upstream/advance_mod.o\n(?:[ \t]+[^\n]+\n)*?[ \t]+w-production-oracle-driver.o$',
                      link_map, re.M) is not None, 'public advance not bound to pristine compiled object')
    # -O3 keeps some private actions separate and hoists movement call addresses.
    # Original object relocations plus the real wrapper observations establish
    # those edges without inventing direct public-advance call instructions.
    original = (build / 'advance-original.disassembly').read_text()
    for symbol in WRAPPED[2:]:
        require(re.search(r'R_X86_64_PLTOFF64\s+' + symbol + r'\b', original),
                'missing original movement/placement relocation: ' + symbol)
        require(re.search(r'^__wrap_' + symbol + r'\s+flagged-top-observer.o$', link_map, re.M),
                'observer not bound to separately compiled object')
        edges.append(['advance_mod.o (original relocation)', '__wrap_' + symbol])
    return edges


def audit(checkout, run):
    """Reconstruct linkage and source identity instead of trusting stored pass flags."""
    manifest = json.loads((ROOT / 'reference/flexpart-11.1.json').read_text())
    pin = manifest['pinned_commit']
    require(git(checkout, 'rev-parse', 'HEAD') == pin and not git(checkout, 'status', '--porcelain'),
            'reference must be the clean pin')
    for name in ('checkout-before.txt', 'checkout-after.txt'):
        require((run / name).read_text().splitlines() == [pin], 'missing clean-checkout record')
    science = decode(run / 'output.txt')
    require(text_digest(run / 'input.txt') == text_digest(ROOT / 'fixtures/interpolation/shared-height-v1/input.txt'),
            'input is not the retained #118 profile fixture')
    build = run / 'build'
    exported = build / 'advance-reexported.o'
    try:
        subprocess.run(['objcopy', '--globalize-symbol=__advance_mod_MOD_pushpartdown',
                        str(build / 'advance_mod.original.o'), str(exported)], check=True,
                       capture_output=True)
        require(digest(exported) == digest(build / 'upstream/advance_mod.o'), 'object differs from visibility-only export')
    finally:
        exported.unlink(missing_ok=True)
    require((build / 'advance-original.text').read_bytes() == (build / 'advance-exported.text').read_bytes(),
            'reference code bytes modified')
    edges = verify_linkage(build)
    sources = {}
    for source in git(checkout, 'ls-files', 'src').splitlines():
        # Upstream make stamps the normal model main; this main is never linked.
        if source == 'src/FLEXPART.f90':
            continue
        relative = Path(source).relative_to('src')
        require(text_digest(checkout / source) == text_digest(build / 'upstream' / relative),
                f'archived source changed: {source}')
        sources[source] = text_digest(checkout / source)
    objects = (build / 'linked-objects.txt').read_text().splitlines()
    require(objects == sorted(set(objects)) and 'advance_mod.o' in objects and 'FLEXPART.o' not in objects,
            'incorrect object inventory')
    image = os.environ.get('RESEARCH_IMAGE_ID', '')
    require(re.fullmatch(r'sha256:[0-9a-f]{64}', image) is not None, 'missing immutable image ID')
    return {'schema': {'id': 'flexpart-gpu.top-boundary-research', 'version': 2}, 'issue': 213,
            'validation_level': 'direct_pinned_routines_synthetic_research_only',
            'comparison_policy': 'Exact raw output and decoded observations in fixed immutable image; no GPU tolerance established.',
            'science': science, 'provenance': {
                'pinned_commit': pin, 'image_id': image, 'profile': manifest['execution_profile'],
                'compiler': (build / 'compiler-identity.txt').read_text().strip(),
                'driver_flags': ['-O0', '-fopenmp', '-mcmodel=large'],
                'sources_sha256': sources, 'tools_sha256': {s: text_digest(ROOT / s) for s in TOOLS},
                'linked_objects_sha256': {s: digest(build / 'upstream' / s) for s in objects},
                'artifacts_sha256': {s: digest(run / s) for s in ARTIFACTS}, 'verified_call_edges': edges,
                'build_origin': re.search(r'(/tmp/flexpart-top-research\.[^/\s]+/build/upstream)/advance_mod.o', (build/'w-production-oracle.link-map').read_text())[1],
                'symbol_export': 'pushpartdown visibility only; repeating objcopy on original reproduces the entire linked object',
            }}



def retained(run):
    """Verify retained bytes and executable call evidence, not just report status."""
    report = json.loads((run / 'report.json').read_text())
    require(report['state'] == 'PASS', 'record-only execution is not regression verification')
    require(report['science'] == decode(run / 'output.txt'), 'decoded report/raw disagreement')
    require(digest(run / 'output.txt') == digest(FIXTURE / 'output.txt'), 'frozen raw output changed')
    baseline = json.loads((FIXTURE / 'report.json').read_text())
    require(report['science'] == baseline['science'], 'scientific baseline disagreement')
    p = report['provenance']
    for key in ('pinned_commit', 'sources_sha256', 'tools_sha256', 'profile', 'driver_flags', 'verified_call_edges'):
        require(p[key] == baseline['provenance'][key], f'baseline provenance changed: {key}')
    require(p['tools_sha256'] == {s: text_digest(ROOT / s) for s in TOOLS}, 'stale verifier/tools')
    require(set(p['artifacts_sha256']) == set(ARTIFACTS) and len(p['linked_objects_sha256']) > 20,
            'incomplete provenance inventories')
    linked = (run / 'build/linked-objects.txt').read_text().splitlines()
    require(linked == sorted(set(linked)) and 'FLEXPART.o' not in linked and
            set(linked) == set(p['linked_objects_sha256']), 'incomplete linked-object hash inventory')
    for artifact, expected in p['artifacts_sha256'].items():
        require(digest(run / artifact) == expected, f'changed artifact: {artifact}')
    for obj, expected in p['linked_objects_sha256'].items():
        require(digest(run / 'build/upstream' / obj) == expected, f'changed linked object: {obj}')
    for source, expected in p['sources_sha256'].items():
        require(text_digest(run / 'build/upstream' / Path(source).relative_to('src')) == expected,
                f'changed source: {source}')
    for name in ('checkout-before.txt', 'checkout-after.txt'):
        require((run / name).read_text().splitlines() == [p['pinned_commit']], 'checkout record changed')
    require(re.fullmatch(r'sha256:[0-9a-f]{64}', p['image_id']), 'missing image identity')
    require((run / 'build/advance-original.text').read_bytes() == (run / 'build/advance-exported.text').read_bytes(),
            'reference instruction bytes changed')
    require(verify_linkage(run / 'build') == p['verified_call_edges'], 'call linkage changed')
    require((run / 'build/compiler-identity.txt').read_text().strip() == p['compiler'], 'compiler record changed')
    link_map = (run / 'build/w-production-oracle.link-map').read_text()
    origins = set(re.findall(r'(/tmp/flexpart-top-research\.[^/\s]+/build/upstream)/advance_mod.o', link_map))
    require(len(origins) == 1 and p['build_origin'] in origins, 'missing or mixed fresh build origin')
    require(report['provenance'].get('unwrapped_control_exact') is True, 'missing unwrapped control execution')
    require([x for x in (run/'output.txt').read_text().splitlines() if not x.startswith('OBS ')] ==
            [x for x in (run/'output-unwrapped.txt').read_text().splitlines() if not x.startswith('OBS ')], 'unwrapped control mismatch')
    return report, origins.pop()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkout',type=Path)
    parser.add_argument('--fixture',type=Path,help='audit a complete frozen v2 fixture without rebuilding')
    parser.add_argument('--run',type=Path)
    parser.add_argument('--record',action='store_true')
    args=parser.parse_args()
    if args.fixture is not None:
        print(json.dumps(audit_fixture(args.fixture)))
        return
    if args.checkout is None or args.run is None:
        parser.error('--checkout and --run are required for execution auditing')
    result=audit(args.checkout,args.run)
    control=[line for line in (args.run/'output-unwrapped.txt').read_text().splitlines() if not line.startswith('OBS ')]
    observed=[line for line in (args.run/'output.txt').read_text().splitlines() if not line.startswith('OBS ')]
    require(control==observed, 'observer changed genuine routine outputs')
    result['provenance']['unwrapped_control_exact']=True
    if not args.record:
        expected=json.loads((FIXTURE/'report.json').read_text())
        require(result['science']==expected['science'], 'versioned scientific expectations changed')
        require(digest(args.run/'output.txt')==digest(FIXTURE/'output.txt'), 'versioned raw output changed')
        for key in ('pinned_commit','sources_sha256','tools_sha256','profile','driver_flags','verified_call_edges'):
            require(result['provenance'][key]==expected['provenance'][key], 'provenance changed: '+key)
    result['state']='RECORDED' if args.record else 'PASS'
    (args.run/'report.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(dict(state=result['state'],advance_cases=64,report=str(args.run/'report.json'))))


if __name__=='__main__':
    main()
