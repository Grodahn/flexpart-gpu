#!/usr/bin/env python3
"""Audit #186 controls from genuine pinned routines; arithmetic is diagnostic only."""

import argparse
import json
import math
import os
import re
from pathlib import Path

from prepare_shared_height_oracle import require, close, f32, sample
from prepare_w_production_oracle import (
    ABSOLUTE_TOLERANCE_M_S, RELATIVE_TOLERANCE, canonical_text_sha256,
    git, sha256, verify_call_edges,
)

VARIANTS = ('full', 'x', 'y', 'zero')
ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / 'fixtures/interpolation/interior-w-v1'


def decode_input(path):
    """Decode the complete finite 3x3/two-time controlled input."""
    rows = [list(map(float, line.split())) for line in path.read_text().splitlines()]
    require(all(math.isfinite(v) for row in rows for v in row), 'non-finite input')
    require(rows[0] == [3] and len(rows[1]) == 4, 'invalid dimensions/geometry')
    require(rows[1][:2] == [0.25, 0.4] and rows[1][3] == 48, 'unexpected physical grid')
    ab = rows[2:6]
    require(ab == [[0, 1], [5000, .7], [10000, .3], [10000, .1]], 'invalid hybrid coefficients')
    terrain = rows[6:9]
    require(all(len(r) == 3 for r in terrain), 'incomplete terrain')
    columns, cursor = [], 9
    for m in (1, 2):
        for y in range(3):
            for x in range(3):
                surface, levels = rows[cursor], rows[cursor+1:cursor+5]
                require(len(surface) == 3 and len(levels) == 4 and all(len(r) == 5 for r in levels),
                        'incomplete column')
                ps, t, td = surface
                require(ps > 80000 and 200 < td <= t < 320, 'invalid surface state')
                pressures = [a+b*ps for a,b in ab]
                require(all(a > b > 0 for a,b in zip(pressures, pressures[1:])), 'invalid pressure ordering')
                require(levels[0][:2] == [t, levels[1][1]], 'wrong artificial surface T/q')
                require(all(200 < r[0] < 320 and 0 <= r[1] < .03 for r in levels), 'invalid T/q')
                require(levels[-1][-1] == 0, 'top interface omega must be zero')
                columns.append({'memory': m, 'x': x, 'y': y, 'surface_pa_t_k_td_k': surface,
                                'center_t_q_u_v_interface_omega': levels})
                cursor += 5
    queries = rows[cursor+1:]
    require(rows[cursor] == [15] and len(queries) == 15 and all(len(q) == 5 for q in queries),
            'incomplete query input')
    return {'grid_dx_dy_lon0_lat0_deg': rows[1], 'half_level_a_pa_b': ab,
            'terrain_m_asl_y_x': terrain, 'columns': columns,
            'queries_time_s_x_y_bracket_fraction': queries}


def decode(path):
    """Reject missing, duplicate, malformed, or nonfinite direct records."""
    lines = path.read_text().splitlines()
    require(lines[0] == 'FLEXPART_INTERIOR_W_ORACLE_V1', 'wrong header')
    native, shared, queries, controls, geometry = {}, {}, {}, {}, None
    for line in lines[1:]:
        t = line.split()
        if t[0] == 'GEOMETRY':
            require(geometry is None and len(t) == 7, 'invalid geometry record')
            geometry = list(map(float, t[1:]))
            continue
        if t[0] == 'CONTROL':
            require(len(t) == 5 and t[1] in VARIANTS, 'wrong control record')
            key, value, dest = (t[1], int(t[2])), list(map(float,t[3:])), controls
        elif t[0] == 'NATIVE':
            require(len(t) == 13, 'wrong native field count')
            key, value, dest = tuple(map(int, t[1:5])), list(map(float, t[5:])), native
        elif t[0] == 'SHARED':
            require(len(t) == 10 and t[1] in VARIANTS, 'wrong shared record')
            key, value, dest = (t[1], *map(int, t[2:6])), list(map(float, t[6:])), shared
        elif t[0] == 'QUERY':
            require(len(t) == 11 and t[1] in VARIANTS, 'wrong query record')
            key, value, dest = (t[1], int(t[2])), [int(t[3]), *map(float, t[4:])], queries
        else:
            raise ValueError('unknown record')
        require(key not in dest, 'duplicate record')
        dest[key] = value
    keys = {(m,x,y,k) for m in (1,2) for x in range(3) for y in range(3) for k in range(1,5)}
    require(set(native) == keys, 'incomplete native evidence')
    require(set(shared) == {(v,*key) for v in VARIANTS for key in keys}, 'incomplete controls')
    require(set(queries) == {(v,q) for v in VARIANTS for q in range(1,16)}, 'incomplete queries')
    require(set(controls) == {(v,m) for v in VARIANTS for m in (1,2)}, 'incomplete control factors')
    for (v,m),factors in controls.items():
        require(factors == [geometry[4] if v in ('full','x') else 0,
                            geometry[5] if v in ('full','y') else 0], 'wrong control factors')
    require(geometry is not None and all(math.isfinite(v) for row in
            [geometry, *native.values(), *shared.values(), *queries.values()] for v in row), 'non-finite output')
    return geometry, native, shared, queries


def contributions(values, label, distinguish):
    """Compare independent executed controls under #80's unchanged velocity policy."""
    full, x, y, zero = [values[v] for v in VARIANTS]
    dx, dy, total = x-zero, y-zero, full-zero
    require(close(total, dx+dy), f'nonadditive correction: {label}')
    if distinguish:
        require(not close(x, zero) and not close(y, zero), f'missing independent direction: {label}')
        require(dx*dy > 0, f'opposite-sign cancellation: {label}')
    return {'case': label, 'x_m_s': dx, 'y_m_s': dy, 'full_minus_zero_m_s': total,
            'sum_m_s': dx+dy, 'additive_error_m_s': abs(total-dx-dy),
            'independently_distinguished': distinguish}


def investigate(inputs, decoded):
    """Bind raw inputs, native normalization, controls, and public sampler outputs."""
    geometry, native, shared, queries = decoded
    require(geometry[:4] == list(map(f32, inputs['grid_dx_dy_lon0_lat0_deg'])), 'geometry mismatch')
    require(all(v > 0 for v in geometry[4:]), 'missing physical spacing')
    require(geometry[4:] == [f32(180/f32(f32(d*6371000)*f32(math.pi))) for d in geometry[:2]],
            'physical spacing differs from pinned gridcheck')
    target = [shared['full',1,0,0,k][0] for k in range(1,5)]
    require(target == [native[1,0,0,k][0] for k in range(1,5)], 'wrong initial target owner')
    for col in inputs['columns']:
        m,x,y = col['memory'],col['x'],col['y']
        require(native[m,x,y,1][2] == f32(col['surface_pa_t_k_td_k'][0]), 'surface pressure mismatch')
        for k,raw in enumerate(col['center_t_q_u_v_interface_omega'],1):
            row = native[m,x,y,k]
            require(row[4:7] == list(map(f32,raw[2:])), 'native input mismatch')
            require(row[7] == f32(row[6]*row[3]) and row[3] < 0, 'normalization lineage mismatch')
        for coordinate in (0,1):
            h = [native[m,x,y,k][coordinate] for k in range(1,5)]
            require(all(a < b for a,b in zip(h,h[1:])), 'unordered native geometry')
    column_checks, query_checks, normalization_checks = [], [], []
    for m in (1,2):
        for k in (2,3):
            require(native[m,0,1,k][0] != native[m,2,1,k][0] and
                    native[m,1,0,k][0] != native[m,1,2,k][0], 'missing two-direction geometry')
        for x in range(3):
            for y in range(3):
                for k in range(1,5):
                    rows = {v:shared[v,m,x,y,k] for v in VARIANTS}
                    require(all(r[:3] == rows['zero'][:3] and r[0] == target[k-1] for r in rows.values()),
                            'control changed U/V or target')
                    interior = x == y == 1 and k in (2,3)
                    check = contributions({v:r[3] for v,r in rows.items()}, [m,x,y,k], interior)
                    if not interior:
                        require(all(r[3] == rows['zero'][3] for r in rows.values()), 'boundary correction not skipped')
                    if interior:
                        # Source interpretation diagnostic: actual four-control
                        # differences above remain the authoritative observations.
                        centers=[native[m,x,y,j][0] for j in range(1,5)]
                        j=next(j for j in range(2,5) if centers[j-2] < target[k-1] <= centers[j-1])
                        f=(target[k-1]-centers[j-2])/(centers[j-1]-centers[j-2])
                        sx=sum(w*(native[m,2,1,l][0]-native[m,0,1,l][0])*.5 for l,w in [(j-1,1-f),(j,f)])
                        sy=sum(w*(native[m,1,2,l][0]-native[m,1,0,l][0])*.5 for l,w in [(j-1,1-f),(j,f)])
                        predicted_x=sx*rows['zero'][1]*geometry[4]/math.cos(math.radians(geometry[3]+geometry[1]))
                        predicted_y=sy*rows['zero'][2]*geometry[5]
                        require(close(check['x_m_s'],predicted_x) and close(check['y_m_s'],predicted_y),
                                'source interpretation differs from executed controls')
                        native_x=sx*native[m,x,y,j][4]*geometry[4]/math.cos(math.radians(geometry[3]+geometry[1]))
                        native_y=sy*native[m,x,y,j][5]*geometry[5]
                        require(not close(predicted_x,native_x) and not close(predicted_y,native_y),
                                'native/remapped UV multiplication not distinguished')
                        check.update({'native_center_bracket_upper':j,'center_upper_weight':f,
                            'source_diagnostic_x_m_s':predicted_x,'source_diagnostic_y_m_s':predicted_y,
                            'wrong_native_upper_uv_x_m_s':native_x,'wrong_native_upper_uv_y_m_s':native_y})
                    column_checks.append(check)
                    # A companion diagnostic checks existing #30/#80 lineage only;
                    # it never supplies the normative correction value.
                    profile = [native[m,x,y,j] for j in range(1,5)]
                    if k in (1,4):
                        expected = profile[k-1][7]
                    else:
                        expected = sample([r[1] for r in profile], [r[7] for r in profile], target[k-1])
                    require(close(rows['zero'][3],expected), 'zero control violates normalized interface remap')
                    if interior:
                        omitted = sample([r[1] for r in profile], [r[6] for r in profile], target[k-1])
                        doubled = sample([r[1] for r in profile], [r[7]*r[3] for r in profile], target[k-1])
                        require(not close(expected, omitted) and not close(expected,doubled), 'normalization not distinguished')
                        normalization_checks.append({'memory':m,'level':k,'direct_zero_m_s':rows['zero'][3],
                            'normalized_interface_diagnostic_m_s':expected,'omitted_diagnostic':omitted,
                            'double_normalization_diagnostic_m_s':doubled})
    for number,raw in enumerate(inputs['queries_time_s_x_y_bracket_fraction'],1):
        time,x,y,k,fraction = raw
        require(time in (0,1800,3600) and k == int(k) and 0<x<1 and 0<y<1, 'unsupported query')
        k=int(k)
        require((k in (0,4) and fraction == 0) or (1<=k<=3 and 0<fraction<1), 'invalid height case')
        z = target[0] if k==0 else target[-1] if k==4 else f32(target[k-1]+f32(f32(fraction)*f32(target[k]-target[k-1])))
        if k not in (0,4):
            require(target[k-1] < z < target[k], 'rounded query is not strict interior')
        for v in VARIANTS:
            q=queries[v,number]
            require(q[:4] == [time,f32(x),f32(y),z], 'query input/output mismatch')
            terrain=inputs['terrain_m_asl_y_x']
            terrain_at_query=sum(w*terrain[cy][cx] for cx,cy,w in
                [(0,0,(1-f32(x))*(1-f32(y))), (1,0,f32(x)*(1-f32(y))),
                 (0,1,(1-f32(x))*f32(y)), (1,1,f32(x)*f32(y))])
            require(abs(q[4]-z-terrain_at_query)<=.02, 'AGL/ASL diagnostic mismatch (#30 height policy)')
            require(q[:7] == queries['zero',number][:7], 'query control changed U/V/coordinates')
            for component in range(3):
                values=[]
                for m in (1,2):
                    horizontal=[sum(weight*shared[v,m,cx,cy,j][component+1] for cx,cy,weight in
                        [(0,0,(1-f32(x))*(1-f32(y))), (1,0,f32(x)*(1-f32(y))),
                         (0,1,(1-f32(x))*f32(y)), (1,1,f32(x)*f32(y))]) for j in range(1,5)]
                    values.append(sample(target,horizontal,z))
                expected=values[0]*(1-time/3600)+values[1]*time/3600
                require(close(q[5+component],expected), 'public sampler differs from executed shared values')
        query_checks.append(contributions({v:queries[v,number][-1] for v in VARIANTS},
                                           [number,time,k], k not in (0,4)))
    for time in (0,1800,3600):
        require({int(r[3]) for r in inputs['queries_time_s_x_y_bracket_fraction'] if r[0]==time} == set(range(5)),
                'missing source time/height coverage')
    require(any(native[1,1,1,k][0] != native[2,1,1,k][0] for k in (2,3)), 'unchanged time geometry')
    return {'target_height_m_agl':target, 'column_contributions':column_checks,
            'query_contributions':query_checks, 'normalization_diagnostics':normalization_checks}


def linked_execution(build):
    """Reconstruct actual routine edges and fresh-build origin from retained linkage."""
    edges=verify_call_edges((build/'w-production-oracle.nm').read_text(),
                            (build/'w-production-oracle.call-sites').read_text())
    link_map=(build/'w-production-oracle.link-map').read_text()
    require(re.search(r'^__verttransform_mod_MOD_verttransform_init\s+[^\n]*verttransform_mod.o\n'
                      r'\s+w-production-oracle-driver.o$', link_map, re.M),
            'missing initializer binding')
    linked=(build/'linked-objects.txt').read_text().splitlines()
    require(linked==sorted(set(linked)) and len(linked)>3 and 'FLEXPART.o' not in linked,
            'invalid object inventory')
    # Absolute LOAD entries survive artifact relocation and distinguish an
    # independently compiled scratch tree from a copy of an earlier run.
    loads=re.findall(r'^LOAD (/tmp/flexpart-height-research\.[^/\s]+)/build/upstream/([^/\s]+\.o)$',
                     link_map, re.M)
    require(sorted(name for _,name in loads)==linked, 'link map/object inventory mismatch')
    origins={origin for origin,_ in loads}
    require(len(origins)==1, 'missing/ambiguous fresh-build origin')
    return edges, origins.pop()


def provenance(checkout, run):
    """Reuse #80 symbol/call verification and #118 pristine scratch-build provenance."""
    manifest=json.loads((ROOT/'reference/flexpart-11.1.json').read_text())
    require(git(checkout,'rev-parse','HEAD') == manifest['pinned_commit'], 'wrong pinned revision')
    require(not git(checkout,'status','--porcelain'), 'dirty pinned checkout')
    for name in ('checkout-before.txt','checkout-after.txt'):
        require((run/name).read_text() == manifest['pinned_commit']+'\n', 'failed retained pristine check')
    build=run/'build'
    edges, fresh_build_id=linked_execution(build)
    image=os.environ.get('RESEARCH_IMAGE_ID','')
    require(re.fullmatch(r'sha256:[0-9a-f]{64}',image), 'missing immutable image')
    sources=['verttransform_mod.f90','windfields_mod.f90','interpol_mod.f90','par_mod.f90',
             'getfields_mod.f90','makefile_gfortran']
    for source in sources:
        require(sha256(checkout/'src'/source) == sha256(build/'upstream'/source), 'changed pinned source')
    linked=(build/'linked-objects.txt').read_text().splitlines()
    require(linked==sorted(set(linked)) and len(linked)>3 and 'FLEXPART.o' not in linked, 'invalid object inventory')
    artifacts=['input.txt','output.txt','full-build.log','driver-build-run.log','checkout-before.txt','checkout-after.txt',
               *['build/'+p for p in ('w-production-oracle','w-production-oracle-driver.o','w-production-oracle.nm',
                  'w-production-oracle.call-sites','w-production-oracle.link-map','linked-objects.txt',
                  'compiler-identity.txt')]]
    research=['interior_w_oracle.f90','prepare_interior_w_oracle.py','test_interior_w_oracle.py','run_shared_height_oracle.py',
              'shared_height_oracle.sh','prepare_shared_height_oracle.py','w_production_oracle.sh',
              'prepare_w_production_oracle.py']
    return {'pinned_commit':manifest['pinned_commit'],'checkout_clean_before_after':True,
            'docker_image_id':image,'compiler':(build/'compiler-identity.txt').read_text().strip(),
            'fresh_build_id':fresh_build_id,
            'build_profile':manifest['execution_profile'],'driver_flags':['-O0','-fopenmp','-mcmodel=large'],
            'sources_sha256':{'src/'+p:sha256(checkout/'src'/p) for p in sources},
            'research_sources_sha256':{p:canonical_text_sha256(ROOT/p) for p in
                [*['scripts/interpolation/'+r for r in research], 'scripts/oracle_build_cache.py',
                 'reference/flexpart-11.1.json']},
            'linked_objects_sha256':{p:sha256(build/'upstream'/p) for p in linked},
            'artifacts_sha256':{p:sha256(run/p) for p in artifacts},'verified_call_edges':edges,
            'initializer_binding_verified':True}


def build_report(checkout, run):
    """Reaudit retained genuine execution without changing any scientific values."""
    inputs=decode_input(run/'input.txt')
    decoded=decode(run/'output.txt')
    scientific={'decoded_inputs':inputs, 'geometry':decoded[0],
                'native_rows':[{'key':list(k),'values':v} for k,v in sorted(decoded[1].items())],
                'shared_rows':[{'key':list(k),'height_uv_w':v} for k,v in sorted(decoded[2].items())],
                'queries':[{'key':list(k),'time_x_y_agl_asl_uv_w':v} for k,v in sorted(decoded[3].items())],
                **investigate(inputs,decoded)}
    return {'schema':{'id':'flexpart-gpu.interior-w-research','version':1},'issue':186,
            'verdict':'independent_x_y_interior_correction_verified',
            'validation_level':'direct_pinned_routines_synthetic_research_only',
            'comparison_policy':{'owner':'issue #80','absolute_m_s':ABSOLUTE_TOLERANCE_M_S,
                                 'relative':RELATIVE_TOLERANCE},
            'scientific':scientific,'provenance':provenance(checkout,run)}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkout',type=Path,required=True)
    parser.add_argument('--run',type=Path,required=True)
    parser.add_argument('--expected',type=Path,required=True)
    args=parser.parse_args()
    result=build_report(args.checkout,args.run)
    frozen=args.expected/'report.json'
    require(frozen.exists(), 'frozen evidence absent; inspect and freeze explicitly')
    expected=json.loads(frozen.read_text())
    for key in ('scientific','comparison_policy','verdict'):
        require(result[key]==expected[key], 'changed frozen '+key)
    # Each run binds and executes its immutable image. Independently built CI
    # images may have different layer hashes; paired builds must use one image.
    for key in ('pinned_commit','sources_sha256','research_sources_sha256','compiler','build_profile'):
        require(result['provenance'][key]==expected['provenance'][key], 'changed provenance '+key)
    require(sha256(args.run/'output.txt')==sha256(args.expected/'output.txt'), 'changed raw output')
    require(canonical_text_sha256(args.run/'input.txt')==canonical_text_sha256(args.expected/'input.txt'), 'changed raw input')
    # A failed frozen comparison must not leave a success-shaped report that a
    # subsequent standalone paired verifier could mistake for accepted evidence.
    (args.run/'report.json').write_text(json.dumps(result,indent=2)+'\n', encoding='utf-8')
    print(json.dumps({'state':'PASS','verdict':result['verdict'],'queries':15}))


if __name__ == '__main__':
    main()
