#!/usr/bin/env python3
"""Audit #197 real drivers and replay their exact queries through #186's genuine binary."""
import argparse
import hashlib
import json
import math
import re
from pathlib import Path
import struct
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/interpolation'))
from compare_interior_w_builds import verified_run
from check_shared_height_gpu import require
from check_shared_height_w_gpu import equivalent


SOURCE_FILES = {'src/simulation/timeloop/forward.rs', 'src/simulation/timeloop/meteorology.rs', 'src/gpu/meteorology/shared_w.rs', 'src/gpu/meteorology/resident.rs', 'src/gpu/shared_height.rs', 'src/simulation/timeloop/backward.rs', 'src/gpu/shared_height_w.rs', 'src/gpu/meteorology.rs', 'tests/support/interior_w.rs', 'src/simulation/timeloop/forward/timestep.rs', 'src/gpu/meteorology/shared_uv.rs', 'src/gpu/advection_resident.rs', 'src/simulation/timeloop/forward/operators.rs', 'src/gpu/meteorology/advection_production_tests.rs'}
SHADERS = {'advection_step_guard', 'shared_height_w_remap', 'vertical_sample', 'resident_sample_status', 'advection_predictor_query', 'resident_status_reset', 'shared_height_uv', 'advection_step_commit', 'temporal_interpolation', 'advection_corrector', 'resident_query_adapter', 'particle_query', 'horizontal_interpolation', 'shared_height_query_status', 'interior_w_slope'}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def f32(x):
    return struct.unpack('<f', struct.pack('<f', x))[0]


def device(path, software=False, hardware=False):
    """Require complete real-driver status, identity, reuse and immutable input evidence."""
    report = json.loads(path.read_text())
    require(report['schema'] == 'flexpart-gpu.shared-advection.v1', 'schema')
    require(report['execution'] == 'wgsl_device', 'no device execution')
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    require(report['candidate_revision'] == revision, 'stale candidate')
    require(set(report['source_hashes']) == SOURCE_FILES and set(report['shader_hashes']) == SHADERS, 'incomplete source/shader inventory')
    require(re.fullmatch('[0-9a-f]{64}',report['candidate_executable_sha256']) is not None, 'candidate executable hash')
    for name, expected in report['source_hashes'].items():
        require(digest((ROOT / name).read_bytes()) == expected, 'source bytes ' + name)
    for name, expected in report['shader_hashes'].items():
        require(digest((ROOT / 'src/shaders' / (name + '.wgsl')).read_bytes()) == expected, 'shader bytes ' + name)
    raw = path.with_name('inputs.json').read_bytes()
    require(digest(raw) == report['input_sha256'], 'source input bytes')
    inputs = json.loads(raw)
    snapshots = [json.loads(s) for s in inputs['snapshot_json']]
    require(len(snapshots) == len(inputs['native_motion_json']) == len(inputs['runtimes']) == 2, 'source count')
    expected = {(False, 0, s) for s in (0, 1)} | {(False, 1800, s) for s in (0, 1)} | {(True, t, s) for t in (1800, 3600) for s in (0, 1)}
    require(len(report['rows']) == 8 and {(r['backward'], r['start_epoch'], r['step']) for r in report['rows']} == expected, 'signed stages/epochs/reuse coverage')
    for row in report['rows']:
        adapter = row['adapter']
        require(adapter['name'] and adapter['backend'], 'adapter')
        require(adapter['adapter_class'] == ('software_wgsl' if adapter['device_type'] == 'cpu' else 'hardware_gpu'), 'adapter classification')
        require(not software or adapter['adapter_class'] == 'software_wgsl', 'software execution required')
        require(not hardware or adapter['adapter_class'] == 'hardware_gpu', 'hardware execution required')
        require(row['capacity'] == 3 and row['active_count'] == 2 and row['reused'] == bool(row['step']), 'capacity/reuse')
        require(row['status'] == [0, 2, 2, 0] * 6 + [0], 'incomplete/fatal/lane status')
        require(len(row['queries']) == 6 and len(row['sampled_values']) == 18 and len(row['metadata']) == 6, 'raw samples')
        target = row['target']
        heights = target['height_m_agl']
        require(len(heights) == 4 and heights[0] == 0 and all(a < b for a, b in zip(heights, heights[1:])), 'target heights')
        preimage = dict(target, identity_sha256='')
        require(digest(row['target_identity_json'].encode()) == target['identity_sha256'], 'target identity preimage')
        require(equivalent(json.loads(row['target_identity_json']), preimage), 'target identity contents')
        require(digest(struct.pack('<4f', *heights)) == target['height_sha256'], 'target byte hash')
        require(len(row['preparations']) == 2, 'prepared sources')
        for i, p in enumerate(row['preparations']):
            uv = p['prepared_uv']
            require(uv['target'] == target and uv['source']['snapshot_sha256'] == digest(inputs['snapshot_json'][i].encode()), 'prepared source/target')
            require(p['source']['normalization'] == inputs['runtimes'][i]['vertical_velocity']['provenance'], 'normalization lineage')
        for stage in (0, 1):
            epoch = row['start_epoch'] + (-1 if row['backward'] else 1) * (row['step'] + stage)
            for component, field in enumerate(('wind_u', 'wind_v', 'vertical_velocity')):
                m = row['metadata'][stage * 3 + component]
                require(m['field_id'] == field and m['time']['instantaneous']['epoch_seconds'] == epoch, 'signed sample time')
                shared = m['shared_height']
                require(shared['preparation'] == 'shared_height_uvw_190' and len(shared['corrected_w']) == 2, 'complete prepared vector')
                require(shared['target_identity_sha256'] == target['identity_sha256'] and shared['target_height_sha256'] == target['height_sha256'], 'query target identity')
                for lane in (0, 1):
                    value = row['sampled_values'][(stage * 3 + component) * 3 + lane]
                    require(type(value) in (int, float) and math.isfinite(value), 'nonfinite sample')
            require(row['queries'][stage * 3 + 2]['active'] == 0, 'inactive query tail')
            for lane in (0, 1):
                q = row['queries'][stage * 3 + lane]
                require(q['active'] == 1 and 0 <= q['height_agl_m'] < heights[-1], 'unsupported top route')
    negatives = json.loads(path.with_name('negative.json').read_text())
    require(negatives['candidate_revision'] == revision, 'stale negative evidence')
    variants = {'target-unexecuted','target-failed','uv-pending','uv-invalid-status','w-unexecuted','w-remapped-only','w-failed','w-invalid-status','nonfinite-vector','at-top','above-top','predictor-crossing','corrector-crossing'}
    require(len(negatives['rows']) == 26 and {(r['backward'],r['variant']) for r in negatives['rows']} == {(b,v) for b in (False,True) for v in variants}, 'complete negative driver coverage')
    for row in negatives['rows']:
        require(all((row['initial_bytes'][i] & 1) == expected for i, expected in ((20,1),(116,1),(212,0))), 'negative active/inactive particle state')
        require(row['initial_bytes'] == row['final_bytes'] and len(row['final_bytes']) == 288, 'fatal scientific-state atomicity')
        require(len(row['status']) == 25 and all(row['status'][i] == 0 for i in range(3,24,4)), 'negative inactive status/tail')
        require(any(row['status'][i] for i in range(0,24,4)) or row['status'][-1] != 0, 'negative route accepted')
        require(row['adapter']['adapter_class'] == ('software_wgsl' if software else 'hardware_gpu') if (software or hardware) else row['adapter']['adapter_class'] in ('hardware_gpu','software_wgsl'), 'negative adapter')
    return report, inputs


def control_input(parent, report):
    """Only test input plumbing: tile initial thermodynamics, retain original U/V/omega."""
    lines = (parent / 'input.txt').read_text().splitlines()
    result = lines[:9]
    for member in range(2):
        for column in range(9):
            start = 9 + (member * 9 + column) * 5
            result.append(lines[9])
            for level in range(4):
                values = lines[start + 1 + level].split()
                values[:2] = lines[10 + level].split()[:2]
                result.append(' '.join(values))
    heights = list(map(f32, report['rows'][0]['target']['height_m_agl']))
    requests = []
    for index, row in enumerate(report['rows']):
        for stage in (0, 1):
            epoch = row['metadata'][stage * 3]['time']['instantaneous']['epoch_seconds']
            for lane in (0, 1):
                q = row['queries'][stage * 3 + lane]
                z = f32(q['height_agl_m'])
                x, y = q['cell_x'] + f32(q['fraction_x']), q['cell_y'] + f32(q['fraction_y'])
                require(f32(x) == x and f32(y) == y, 'query coordinates cannot be represented by genuine driver')
                requests.append(dict(row=index, stage=stage, lane=lane, time=epoch, x=x, y=y, z=z, bracket=-1, fraction=z))
    result.append(str(len(requests)))
    result.extend(f"{q['time']} {q['x']:.17g} {q['y']:.17g} {q['bracket']} {q['fraction']:.17g}" for q in requests)
    return ('\n'.join(result) + '\n').encode(), requests


def verify_control_sources(inputs, decoded):
    """Bind every native scientific input to the independent genuine control."""
    for i, source in enumerate([json.loads(s) for s in inputs['snapshot_json']]):
        grid = source['horizontal_grid']
        require(grid['nx'] == grid['ny'] == 3 and grid['longitude_domain'] == 'minus180_to180', 'canonical control domain')
        require(list(map(f32, [grid[k] for k in ('dx_deg','dy_deg','xlon0_deg','ylat0_deg')])) ==
                list(map(f32, decoded['grid_dx_dy_lon0_lat0_deg'])), 'canonical control grid')
        coordinate = source['vertical_coordinate']
        require(coordinate['kind'] == 'hybrid_sigma_pressure' and coordinate['reference'] == 'model_native'
                and coordinate['ordering'] == 'increasing', 'canonical control vertical coordinate')
        require(list(map(f32, coordinate['hybrid_a_interface_pa'])) ==
                [f32(ab[0]) for ab in reversed(decoded['half_level_a_pa_b'])] and
                list(map(f32, coordinate['hybrid_b_interface'])) ==
                [f32(ab[1]) for ab in reversed(decoded['half_level_a_pa_b'])], 'canonical control hybrid coefficients')
        fields = {f['id']: f for f in source['fields']}
        surface_fields = ('surface_pressure','temperature2m','dewpoint2m','wind_u10m','wind_v10m','orography')
        center_fields = ('temperature','specific_humidity','wind_u','wind_v')
        require(len(fields) == len(source['fields']) == 10 and set(fields) == set(surface_fields + center_fields), 'canonical control fields')
        for field, record in fields.items():
            require(record['shape'] == ([3,3] if field in surface_fields else [3,3,3]) and
                    record['axis_order'] == (['x','y'] if field in surface_fields else ['x','y','z']) and
                    record['storage_order'] == 'x_fastest' and
                    len(record['values']) == (9 if field in surface_fields else 27), 'canonical control layout')
            require(record['time']['calendar'] == 'gregorian' and
                    record['time']['kind'] == ('static' if field == 'orography' else 'instantaneous') and
                    record['time']['valid_time_epoch_seconds'] == i * 3600, 'canonical control source time')
        motion = json.loads(inputs['native_motion_json'][i])
        require(motion['kind'] == 'pressure_velocity_omega' and motion['unit'] == 'pascal_per_second' and
                motion['sign'] == 'positive_pressure_increasing' and motion['vertical_staggering'] == 'level_interface'
                and len(motion['values']) == 36, 'native control motion convention')
        for c in [c for c in decoded['columns'] if c['memory'] == i+1]:
            column = c['y'] * 3 + c['x']
            surface = c['surface_pa_t_k_td_k'] + c['center_t_q_u_v_interface_omega'][0][2:4] + [decoded['terrain_m_asl_y_x'][c['y']][c['x']]]
            for field, value in zip(surface_fields, surface):
                require(f32(fields[field]['values'][column]) == f32(value), 'canonical control surface ' + field)
            for z in range(3):
                for lane, field in enumerate(center_fields):
                    require(f32(fields[field]['values'][z*9+column]) == f32(c['center_t_q_u_v_interface_omega'][3-z][lane]), 'canonical control center')
            for z in range(4):
                require(f32(motion['values'][z*9+column]) == f32(c['center_t_q_u_v_interface_omega'][3-z][4]), 'native omega control')


def replay(path, parent, software=False, hardware=False, execute=True):
    """Execute/revalidate #186's existing linked binary without a new sampler or build."""
    report, inputs = device(path, software, hardware)
    original = verified_run(parent)
    raw, requests = control_input(parent, report)
    directory = path.parent / 'oracle-replay'
    directory.mkdir(exist_ok=True)
    input_path, output_path = directory / 'input.txt', directory / 'output.txt'
    harness = (ROOT / 'scripts/interpolation/interior_w_oracle.f90').read_text()
    needle = '    if (k==0) then'
    require(harness.count(needle) == 1, 'unrecognized genuine #186 harness')
    adapted = harness.replace(needle, '    if (k==-1) then\n      z=fraction\n    else if (k==0) then')
    harness_path = directory / 'interior_w_driver.f90'
    if execute:
        harness_path.write_text(adapted)
        input_path.write_bytes(raw)
        image = original['provenance']['docker_image_id']
        command = ['docker', 'run', '--rm', '-v', f'{parent.resolve().as_posix()}:/oracle:ro',
                   '-v', f'{ROOT.as_posix()}:/workspace/flexpart-gpu:ro',
                   '-v', f'{directory.resolve().as_posix()}:/replay', '-w', '/replay',
                   '-e','ORACLE_SRC=/oracle/build/upstream','-e','DRIVER=/replay/interior_w_driver.f90']
        for key, value in original['provenance']['build_profile']['runtime_environment'].items():
            command += ['-e', key + '=' + str(value)]
        command += [image, 'bash', '/workspace/flexpart-gpu/scripts/interpolation/w_production_oracle.sh',
                    '/replay/build','/replay/input.txt','/replay/output.txt']
        with (directory / 'run.log').open('wb') as stream:
            subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT, check=True)
    require(harness_path.read_text() == adapted, 'changed query-plumbing adapter')
    from prepare_w_production_oracle import verify_call_edges
    build = directory / 'build'
    edges = verify_call_edges((build/'w-production-oracle.nm').read_text(), (build/'w-production-oracle.call-sites').read_text())
    require([(e['caller'],e['callee']) for e in edges] == [(e['caller'],e['callee']) for e in original['provenance']['verified_call_edges']], 'genuine sampler/transform call edges')
    links = (build/'w-production-oracle.link-map').read_text()
    require(re.search(r'^__verttransform_mod_MOD_verttransform_init\s+[^\n]*verttransform_mod.o\n'
                      r'\s+w-production-oracle-driver.o$', links, re.M), 'initializer binding')
    linked = sorted(re.findall(r'^LOAD /oracle/build/upstream/([^/\s]+\.o)$', links, re.M))
    require(linked == list(original['provenance']['linked_objects_sha256']), 'exact unchanged pinned object linkage')
    require((build/'compiler-identity.txt').read_text().strip() == original['provenance']['compiler'], 'compiler identity')
    require(input_path.read_bytes() == raw, 'replay input bytes')
    # Bind all candidate canonical lanes to the independent genuine input, not just hashes.
    from prepare_interior_w_oracle import decode_input
    temp = directory / 'source-control.txt'
    temp.write_bytes(b'\n'.join(raw.splitlines()[:99]) + b'\n' + b'\n'.join((parent / 'input.txt').read_bytes().splitlines()[99:]) + b'\n')
    decoded = decode_input(temp)
    verify_control_sources(inputs, decoded)
    rows = [line.split() for line in output_path.read_text().splitlines() if line.startswith('QUERY full ')]
    require(len(rows) == len(requests) == 32, 'complete genuine query replay')
    comparisons = []
    for number, (tokens, request) in enumerate(zip(rows, requests), 1):
        require(int(tokens[2]) == number and int(tokens[3]) == request['time'], 'oracle query order/time')
        require(list(map(float, tokens[4:7])) == [request['x'], request['y'], request['z']], 'exact oracle query coordinates/height')
        row, stage, lane = report['rows'][request['row']], request['stage'], request['lane']
        for component, expected in enumerate(map(float, tokens[8:11])):
            actual = row['sampled_values'][(stage * 3 + component) * 3 + lane]
            error = abs(actual - expected)
            limit = 1e-6 + 1e-5 * max(abs(actual), abs(expected))
            require(math.isfinite(expected) and error <= limit, 'genuine pinned velocity mismatch')
            comparisons.append(dict(**request, component=component, actual_m_s=actual, oracle_m_s=expected,
                                    absolute_difference_m_s=error, allowed_difference_m_s=limit))
    result = dict(schema='flexpart-gpu.shared-advection-oracle.v1', verdict='passed_supported_query_handoff',
                  replay_artifacts_sha256={str(p.relative_to(directory)):digest(p.read_bytes()) for p in [harness_path, input_path, output_path, *[build/n for n in ('w-production-oracle','w-production-oracle-driver.o','w-production-oracle.nm','w-production-oracle.call-sites','w-production-oracle.link-map','compiler-identity.txt','linked-objects.txt')]]}, candidate_report_sha256=digest(path.read_bytes()), candidate_input_sha256=report['input_sha256'],
                  pinned_provenance=original['provenance'], replay_call_edges=edges, replay_tool_sha256=digest(Path(__file__).read_bytes()), input_sha256=digest(raw), output_sha256=digest(output_path.read_bytes()),
                  comparison_policy=original['comparison_policy'], rows=comparisons,
                  maximum_absolute_difference_m_s=max(r['absolute_difference_m_s'] for r in comparisons))
    policy = {'absolute_tolerance':1e-6, 'relative_tolerance':1e-5}
    require(all(r['absolute_difference_m_s'] <= policy['absolute_tolerance'] or
                r['absolute_difference_m_s'] <= policy['relative_tolerance'] * max(abs(r['actual_m_s']),abs(r['oracle_m_s']))
                for r in comparisons), 'generic finite comparison')
    adapters = [r['adapter'] for r in report['rows']]
    require(all(a == adapters[0] for a in adapters), 'mixed adapter evidence')
    result['gpu_evidence'] = {
        'schema':{'id':'flexpart-gpu.gpu-execution-evidence','version':1},
        'case_id':'ADV-GPU-SHARED-197',
        'candidate':{'implementation_id':'production_shared_height_petterssen_queries',
                     'revision':report['candidate_revision'], 'input_sha256':report['input_sha256'],
                     'shader_sha256':digest(json.dumps(report['shader_hashes'],sort_keys=True,separators=(',',':')).encode())},
        'execution':{'status':'passed','calculation_path':'wgsl_device','adapter':adapters[0],'failure':None,'skip_reason':None},
        'oracle':{'implementation_id':'FLEXPART-11.1 genuine #186 invariant-top QUERY control',
                  'revision':original['provenance']['pinned_commit'],
                  'executable_sha256':digest((build/'w-production-oracle').read_bytes()),
                  'output_sha256':digest(output_path.read_bytes())},
        'comparison':{'verdict':'passed','policy':policy,'compared_value_count':len(comparisons),
                      'candidate_value_count':len(comparisons),'oracle_value_count':len(comparisons),
                      'max_absolute_error':result['maximum_absolute_difference_m_s'],
                      'max_relative_error':max(r['absolute_difference_m_s']/max(abs(r['actual_m_s']),abs(r['oracle_m_s']))
                          if max(abs(r['actual_m_s']),abs(r['oracle_m_s'])) else 0 for r in comparisons),
                      'first_failure':None}}
    dest = directory / 'comparison.json' 
    if execute:
        dest.write_text(json.dumps(result, indent=2) + '\n')
    else:
        require(json.loads(dest.read_text()) == result, 'stale/incomplete paired comparison')
    with (directory/'typed-evidence-check.log').open('wb') as stream:
        subprocess.run(['cargo','run','--quiet','--example','check_gpu_evidence','--',str(dest)],
                       cwd=ROOT,stdout=stream,stderr=subprocess.STDOUT,check=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('device','replay','check'))
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--oracle-run', type=Path)
    parser.add_argument('--software', action='store_true')
    parser.add_argument('--hardware', action='store_true')
    args = parser.parse_args()
    if args.command == 'device':
        device(args.report, args.software, args.hardware)
        print('SHARED-ADVECTION-197 device evidence passed: 8 real signed driver steps; paired replay remains separate')
    else:
        parser.error('--oracle-run is required') if args.oracle_run is None else None
        result = replay(args.report, args.oracle_run, args.software, args.hardware, args.command == 'replay')
        print(json.dumps(dict(status='PASS', compared_values=len(result['rows']), maximum_absolute_difference_m_s=result['maximum_absolute_difference_m_s'])))


if __name__ == '__main__':
    main()
