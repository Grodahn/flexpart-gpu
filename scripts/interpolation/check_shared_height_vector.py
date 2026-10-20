#!/usr/bin/env python3
"""Audit #190 complete canonical U/V/W results against genuine pinned #186 QUERY output."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import struct
import subprocess
from compare_interior_w_builds import verified_frozen, verified_run
from check_shared_height_w_gpu import equivalent, close
from check_shared_height_gpu import require, require_f32_values

ROOT=Path(__file__).resolve().parents[2]
SHADERS=('shared_height_uv','shared_height_w_remap','interior_w_slope','shared_height_query_status',
    'resident_query_adapter','horizontal_interpolation','vertical_sample','temporal_interpolation',
    'particle_query','resident_status_reset','resident_sample_status','resident_fixture_consumer')
SOURCES=('src/gpu/meteorology.rs','src/gpu/meteorology/resident.rs','src/gpu/meteorology/shared_uv.rs',
    'src/gpu/meteorology/shared_w.rs','src/gpu/shared_height_w.rs','src/gpu/shared_height.rs',
    'tests/shared_height_vector.rs','tests/support/interior_w.rs')

def digest(data):
    """Bind the bytes consumed by the device and its source owners."""
    return hashlib.sha256(data).hexdigest()

def check(path,software=False,hardware=False):
    """Reject missing, stale, skipped or scientifically incomplete evidence."""
    frozen=verified_frozen()
    oracle_path=os.environ.get('FLEXPART_GPU_INTERIOR_W_ORACLE')
    oracle=verified_run(Path(oracle_path).parent) if oracle_path else frozen
    report=json.loads(path.read_text())
    evidence=report['evidence'];record=report
    inputs_bytes=path.with_name('inputs.json').read_bytes();inputs=json.loads(inputs_bytes)
    snapshots=inputs['snapshots']
    require(report['schema']=={'id':'flexpart-gpu.shared-height-vector','version':1},'schema')
    require(evidence['schema']=={'id':'flexpart-gpu.gpu-execution-evidence','version':1},'GPU schema')
    require(evidence['case_id']=='SHARED-HEIGHT-UVW-QUERY-190','case')
    candidate=evidence['candidate']
    revision=os.environ.get('FLEXPART_GPU_CANDIDATE_REVISION') or subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()
    require(candidate['revision']==revision and candidate['implementation_id']=='canonical_shared_height_uvw_queries','candidate identity')
    require(candidate['input_sha256']==digest(inputs_bytes),'input bytes')
    require(re.fullmatch('[0-9a-f]{64}',report['candidate_executable_sha256']),'candidate executable')
    expected_shaders={name:digest((ROOT/f'src/shaders/{name}.wgsl').read_bytes()) for name in SHADERS}
    require(report['shaders']==expected_shaders and candidate['shader_sha256']==digest(json.dumps(expected_shaders,sort_keys=True,separators=(',',':')).encode()),'shader bundle')
    require(report['source_hashes']=={name:digest((ROOT/name).read_bytes()) for name in SOURCES},'source bytes')
    source=(ROOT/'tests/shared_height_vector.rs').read_text()
    consumer=source.split('const VECTOR_CONSUMER: &str = r"',1)[1].split('";',1)[0]
    require(report['vector_consumer_sha256']==digest(consumer.encode()),'vector fixture shader')
    execution=evidence['execution'];adapter=execution['adapter']
    require(execution['status']=='passed' and execution['calculation_path']=='wgsl_device' and execution['failure'] is None and execution['skip_reason'] is None,'actual device execution')
    require(adapter['name'] and adapter['backend'] in ('vulkan','dx12','metal','gl','browser_webgpu'),'adapter')
    require(adapter['adapter_class']==('software_wgsl' if adapter['device_type']=='cpu' else 'hardware_gpu'),'adapter classification')
    require(not software or adapter['adapter_class']=='software_wgsl','required software adapter')
    require(not hardware or adapter['adapter_class']=='hardware_gpu','required hardware adapter')
    provenance=oracle['provenance']
    require(report['oracle_provenance']==provenance and evidence['oracle']['revision']==provenance['pinned_commit'],'oracle provenance')
    for key,artifact in [('executable_sha256','build/w-production-oracle'),('output_sha256','output.txt')]:
        require(evidence['oracle'][key]==provenance['artifacts_sha256'][artifact],'direct oracle hashes')
    queries=[]
    for query in oracle['scientific']['queries']:
        if query['key'][0]=='full':
            v=query['time_x_y_agl_asl_uv_w']
            queries.append(dict(query=query['key'][1],time_s=int(v[0]),x=v[1],y=v[2],height_m_agl=v[3],oracle_uv_w_m_s=v[5:8]))
    require(len(queries)==15,'oracle full query coverage')
    expected_keys = {(api, query, component) for api in ('#76', '#171') for query in range(1, 16) for component in range(3)}
    rows = report['rows']
    require(len(rows) == 90 and {(r['api'], r['query'], r['component']) for r in rows} == expected_keys, 'complete unique U/V/W query rows')
    query_map = {row['query']: row for row in queries}
    f32_bits = lambda value: struct.unpack('<I', struct.pack('<f', value))[0]
    expected_queries = [[q['time_s'], 0, 0, f32_bits(q['x']), f32_bits(q['y']), f32_bits(q['height_m_agl'])]
                        for q in queries]
    require(report['query_inputs'] == expected_queries and report['query_input_sha256'] == hashlib.sha256(
        json.dumps(expected_queries, separators=(',', ':')).encode()).hexdigest(), 'exact split particle query inputs')
    for row in rows:
        expected = query_map[row['query']]['oracle_uv_w_m_s'][row['component']]
        actual = row['actual_m_s']
        require(math.isfinite(actual) and math.isfinite(expected), 'nonfinite comparison')
        error = abs(actual - expected)
        limit = 1e-6 + 1e-5 * max(abs(actual), abs(expected))
        require(row['oracle_m_s'] == expected and error <= limit, 'direct QUERY mismatch')
        require(row['absolute_difference_m_s'] == error and row['allowed_difference_m_s'] == limit, 'comparison metrics')
    comparison = evidence['comparison']
    require(comparison['verdict'] == 'passed' and comparison['compared_value_count'] == 90
            and comparison['oracle_value_count'] == comparison['candidate_value_count'] == 90
            and comparison['policy'] == {'absolute_tolerance': 1e-6, 'relative_tolerance': 1e-5}
            and comparison['first_failure'] is None, 'paired finite verdict')
    differences = [r['absolute_difference_m_s'] for r in rows]
    relative = [error / max(abs(r['actual_m_s']), abs(r['oracle_m_s']))
                if max(abs(r['actual_m_s']), abs(r['oracle_m_s'])) else 0.0
                for r, error in zip(rows, differences)]
    require(comparison['max_absolute_error'] == max(differences) and
            comparison['max_relative_error'] == max(relative), 'paired comparison metrics')
    target = report['targets'][0]
    require(len(report['targets']) == 2 and report['targets'][1] == target, 'common run target')
    require(len(report['sources']) == 2 and target['initial_source'] == report['sources'][0], 'initial source')
    require(report['sources'] == [metadata['source'] for metadata in report['uv_metadata']],
            'query source differs from prepared U/V source')
    heights = report['actual_target_height_m_agl']
    require(heights == target['height_m_agl'] and len(heights) == 4 and
            hashlib.sha256(struct.pack('<4f', *heights)).hexdigest() == target['height_sha256'], 'device target bytes')
    for actual, expected in zip(heights, oracle['scientific']['target_height_m_agl']):
        require(math.isfinite(actual) and abs(actual-expected) <= max(0.02, 1e-5*abs(expected)), '#30 target tolerance')
    serialized_target = report['serialized_target_identity']
    preimage = json.loads(serialized_target)
    require(struct.pack('<4f', *preimage['height_m_agl']) == struct.pack('<4f', *heights), 'target preimage heights')
    preimage['height_m_agl'] = target['height_m_agl']
    require(preimage == dict(target, identity_sha256='') and
            hashlib.sha256(serialized_target.encode()).hexdigest() == target['identity_sha256'], 'target provenance hash')
    require(len(report['serialized_sources']) == 2, 'source serializations')
    for source, snapshot, serialized in zip(report['sources'], snapshots, report['serialized_sources']):
        require(equivalent(json.loads(serialized), snapshot) and
                hashlib.sha256(serialized.encode()).hexdigest() == source['snapshot_sha256'], 'source snapshot hash')
        require(source['grid'] == snapshot['horizontal_grid'], 'source grid')
        require(source['time'] == next(f['time'] for f in snapshot['fields'] if f['id'] == 'wind_u'), 'source time')
        require(source['geometry_provenance']['source_snapshot_sha256'] == source['snapshot_sha256'], 'geometry lineage')
    for metadata_name in ('host_metadata','resident_metadata'):
        require(len(report[metadata_name])==45,'metadata coverage')
        for index,metadata in enumerate(report[metadata_name]):
            component=index//15;query=queries[index%15]
            require(metadata['field_id']==('wind_u','wind_v','vertical_velocity')[component] and
                metadata['unit']=='meter_per_second' and metadata['vertical_staggering']=='level_center' and
                metadata['sign']==('positive_eastward','positive_northward','positive_upward')[component] and
                metadata['horizontal_staggering']=='cell_center', 'physical output contract')
            require(metadata['horizontal_grid']==report['sources'][0]['grid'] and
                metadata['geometry_provenance']==[s['geometry_provenance'] for s in report['sources']],
                'query grid/geometry provenance')
            shared=metadata['shared_height']
            require(shared=={'preparation':'shared_height_uvw_190','target_identity_sha256':target['identity_sha256'],
                'target_height_sha256':target['height_sha256'],'source_input_sha256':[s['input_sha256'] for s in report['sources']],
                'corrected_w':report['w_sources']},'complete vector provenance')
            require(metadata['source_snapshot_sha256']==[s['snapshot_sha256'] for s in report['sources']] and
                metadata['geometry_identity']==[s['geometry_identity'] for s in report['sources']] and
                metadata['source_times']==[s['time'] for s in report['sources']],'bracket sources')
            selection=metadata['request']['time'] if metadata_name=='host_metadata' else metadata['time']
            require(selection=={'instantaneous':{'calendar':'gregorian','epoch_seconds':query['time_s']}},'query time')
            sample_time=query['time_s']
            require(metadata['temporal_bracket']=={
                'lower_index':0, 'upper_index':1,
                'source_timestamps':{'lower_epoch_seconds':0, 'upper_epoch_seconds':3600},
                'application':{0:'first_endpoint',1800:'linear_interior',3600:'last_endpoint'}[sample_time],
                'weights':{'dt1_seconds':sample_time,'dt2_seconds':3600-sample_time,
                           'inverse_span_per_second':1/3600}}, 'query bracket identity')
            if metadata_name=='resident_metadata':
                require(all(metadata[key]==1 for key in ('capacity','active_count','component_count','lane_stride')),
                        'resident query layout')
            else:
                require(metadata['value_count']==1 and
                    len(metadata['resolved_heights_agl_m'])==2 and
                    all(f32_bits(height)==f32_bits(query['height_m_agl'])
                        for height in metadata['resolved_heights_agl_m']), 'host query layout')
            if component==2 and metadata_name=='host_metadata':require(metadata['motion_provenance']==[s['normalization'] for s in report['w_sources']],'W normalization handoff')
            if metadata_name=='host_metadata':
                request=metadata['request']
                require(f32_bits(request['xt'])==f32_bits(query['x']) and f32_bits(request['yt'])==f32_bits(query['y']) and
                    f32_bits(request['height']['meters'])==f32_bits(query['height_m_agl']) and
                    request['height']['reference']=='above_ground_level','query coordinates')
    vector=report['vector_rows']
    require(len(vector)==45 and {(r['query'],r['component']) for r in vector}=={(q,c) for q in range(1,16) for c in range(3)},'vector consumer coverage')
    for row in vector:
        expected=query_map[row['query']]['oracle_uv_w_m_s'][row['component']]
        require(row['api']=='fixture' and row['oracle_m_s']==expected,'vector reference')
        close(row['actual_m_s'],expected)
    seen={('full',m,x,y,k):report['actual_prepared_w'][m-1][(k-1)*9+y*3+x]
        for m in (1,2) for x in range(3) for y in range(3) for k in range(1,5)}
    require(len(inputs['snapshots'])==len(inputs['native_motion'])==len(record['w_sources'])==len(record['uv_metadata'])==2, 'source count')
    require(record['final_w_status']==record['final_uv_status']==[[2]*9,[2]*9], 'preparation not completed')
    heights=record['actual_target_height_m_agl']
    serialized_target=record.get('serialized_target_identity')
    require(isinstance(serialized_target,str), 'missing target identity preimage')
    target_preimage=dict(target,identity_sha256='')
    decoded_target=json.loads(serialized_target)
    # Direct serde f32 serialization is shortest-roundtrip; serde Value uses f64 numbers.
    require_f32_values(decoded_target['height_m_agl'],target_preimage['height_m_agl'], 'target preimage heights')
    decoded_target['height_m_agl']=target_preimage['height_m_agl']
    require(decoded_target==target_preimage and
            digest(serialized_target.encode())==target['identity_sha256'], 'target provenance hash')
    require(target['selection_rule']=='regional_mother_nonrestart_first_y_then_x_ps_gt_100000_at_0_0_ground_zero', 'target selection rule')
    require(heights==target['height_m_agl'] and len(heights)==4 and heights[0]==0, 'target values')
    require(target['height_sha256']==digest(struct.pack('<4f',*heights)) and target['selected_xy']==[0,0], 'target byte hash/selection')
    for a,b in zip(heights,oracle['scientific']['target_height_m_agl']):require(abs(a-b)<=max(.02,1e-5*abs(b)), 'target #30 policy')
    for index in range(2):
        m=index+1; snapshot=inputs['snapshots'][index];native=inputs['native_motion'][index]
        metadata=record['uv_metadata'][index];source=metadata['source'];motion=record['w_sources'][index]
        for key,value,serialized_key in (('snapshot',snapshot,'serialized_sources'),('native',native,'serialized_native_motion')):
            serialized=record[serialized_key][index];require(equivalent(json.loads(serialized),value), 'serialized '+key)
        require(source['snapshot_sha256']==digest(record['serialized_sources'][index].encode()) and
                source['geometry_provenance']['source_snapshot_sha256']==source['snapshot_sha256'], 'snapshot/geometry identity')
        require(metadata['target']==target and metadata['unit']=='meter_per_second' and metadata['fields']==['wind_u','wind_v'], 'same target and prepared U/V')
        require(equivalent(json.loads(record['serialized_uv_metadata'][index]),metadata) and
                motion['prepared_uv_sha256']==digest(record['serialized_uv_metadata'][index].encode()), 'prepared U/V identity')
        require(json.loads(record['serialized_grids'][index])==snapshot['horizontal_grid']==source['grid'] and
                motion['horizontal_sha256']==digest(record['serialized_grids'][index].encode()), 'horizontal identity')
        w_metadata=record['w_metadata'][index]
        require(w_metadata['field']=='vertical_velocity' and w_metadata['unit']=='meter_per_second' and
                w_metadata['sign']=='positive_upward' and w_metadata['source_staggering']=='level_interface' and
                w_metadata['height_reference']=='above_ground_level' and w_metadata['storage_layout']=='[target-level,y,x]; x fastest' and
                w_metadata['correction_algorithm']=='flexpart11_regional_interior_w_slope_v1' and
                w_metadata['source']==motion and w_metadata['prepared_uv']==metadata, 'W physical metadata')
        require(native['kind']=='pressure_velocity_omega' and native['unit']=='pascal_per_second' and
                native['sign']=='positive_pressure_increasing' and native['vertical_staggering']=='level_interface', 'native physical metadata')
        norm=motion['normalization']
        require(norm['source_native_motion_sha256']==digest(record['serialized_native_motion'][index].encode()) and
                norm['source_id']==native['provenance']['source_id'] and
                norm['algorithm_id']=='omega_interface_flexpart11_pinmconv_v1' and
                norm['source_kind']=='pressure_velocity_omega' and norm['source_unit']=='pascal_per_second' and
                norm['source_sign']=='positive_pressure_increasing' and
                norm['source_vertical_staggering']==norm['output_vertical_staggering']=='level_interface' and
                motion['correction_algorithm']=='flexpart11_regional_interior_w_slope_v1', 'normalization/correction contract')
        lanes=record['native_interface_lanes'][index]
        require(len(lanes)==36 and motion['interface_input_sha256']==digest(struct.pack('<72f',*(v for lane in lanes for v in lane))), 'interface upload hash')
        for k in range(4):
            for y in range(3):
                for x in range(3):
                    lane=lanes[k*9+y*3+x]
                    r=next(r['values'] for r in oracle['scientific']['native_rows'] if r['key']==[m,x,y,k+1])
                    require(math.isfinite(lane[0]) and abs(lane[0]-r[1])<=max(.02,1e-5*abs(r[1])), 'native interface heights')
                    close(lane[1],r[7])
                    if k in (0,3):require(seen[('full',m,x,y,k+1)]==lane[1], 'endpoint is not exact normalized copy')
        # Bind every meteorological lane to the direct driver's input, without reconstructing geometry.
        columns=[c for c in oracle['scientific']['decoded_inputs']['columns'] if c['memory']==m]
        fields={f['id']:f for f in snapshot['fields']}
        for name,lane in (('temperature',0),('specific_humidity',1),('wind_u',2),('wind_v',3)):
            expected_values=[c['center_t_q_u_v_interface_omega'][z][lane] for z in (3,2,1) for c in columns]
            require_f32_values(fields[name]['values'],expected_values,'oracle canonical '+name)
        require_f32_values(native['values'],[c['center_t_q_u_v_interface_omega'][z][4] for z in (3,2,1,0) for c in columns], 'native omega inputs')
        for name,lane in (('surface_pressure',0),('temperature2m',1),('dewpoint2m',2)):
            require_f32_values(fields[name]['values'],[c['surface_pa_t_k_td_k'][lane] for c in columns], 'oracle surface '+name)
        for name,lane in (('wind_u10m',2),('wind_v10m',3)):
            require_f32_values(fields[name]['values'],[c['center_t_q_u_v_interface_omega'][0][lane] for c in columns], 'oracle artificial ground')
        grid=snapshot['horizontal_grid'];require([grid[k] for k in ('dx_deg','dy_deg','xlon0_deg','ylat0_deg')]==oracle['scientific']['decoded_inputs']['grid_dx_dy_lon0_lat0_deg'], 'oracle horizontal grid')
        require(source['time']==fields['wind_u']['time'] and source['time']['valid_time_epoch_seconds']==index*3600, 'source time')
    require(record['uv_metadata'][0]['source']==target['initial_source'] and
            record['uv_metadata'][0]['source']['device_context_identity']==record['uv_metadata'][1]['source']['device_context_identity'], 'target/context ownership')
    cases=report['negative_cases']
    expected_static={'missing-w','missing-uv','reversed-w','reversed-uv','wrong-geometry','wrong-source','wrong-time',
        'wrong-horizontal','wrong-normalization','wrong-target','different-uv-owner','wrong-context',
        *[f'{case}-{component}' for case in ('asl','outside-time') for component in range(3)],
        'native-nonuniform','native-interface-w','adjacent-bracket-reuse','invalid-capacity','invalid-prefix'}
    expected_device={'cross-workgroup','inactive-tail','empty-prefix','inactive-interior','nonfinite-query','incomplete-w-lane',
        'unexecuted-w','remapped-w','failed-w','nonfinite-w-status','nonfinite-w-corner','failed-uv','failed-target','nonfinite-v-corner'}
    require(len(cases)==len({c['case'] for c in cases})==37 and {c['case'] for c in cases}==expected_static|expected_device and all(c['passed'] is True for c in cases),'failure/reuse coverage')
    for case in cases:
        name=case['case']
        if name=='adjacent-bracket-reuse':
            require(len(case['values'])==3 and all(math.isfinite(a) and a==b for a,b in case['values']),'retained adjacent source')
        if name=='native-interface-w':
            require(case['rejection']=='#80 freezes a single vertical column without horizontal slope correction',
                    'native interface W did not reach the unsupported geometry boundary')
        if name not in expected_device:continue
        result=case['result'];statuses=result['statuses'];values=result['output']
        require(len(statuses)==3 and len(values)==130 and all(len(s['lanes'])==130 for s in statuses),'vector checkpoint dimensions')
        fatal=name not in ('inactive-tail','empty-prefix','inactive-interior','incomplete-w-lane')
        require(all(s['fatal'] is fatal for s in statuses),'fatal checkpoint '+name)
        active=64 if name=='inactive-tail' else 0 if name=='empty-prefix' else 130
        if name in ('inactive-tail','empty-prefix','inactive-interior'):
            lanes=[2]*active+[0]*(130-active)
            if name=='inactive-interior':lanes[1]=0
            require(all(s['lanes']==lanes for s in statuses),'active prefix/inactive lane')
        if name=='cross-workgroup':require(all(s['lanes'][-1]==3 for s in statuses),'signed cell status')
        if name=='nonfinite-query':require(all(s['lanes'][-1]==5 for s in statuses),'nonfinite query status')
        if name=='incomplete-w-lane':require(statuses[2]['lanes'][-1]==1 and all(s['lanes'][-1]==2 for s in statuses[:2]),'independent incomplete W')
        for lane,value in enumerate(values):
            rejected=fatal or name=='incomplete-w-lane' or lane>=active or (name=='inactive-interior' and lane==1)
            if rejected:require(value==[-999]*4,'non-atomic vector write '+name)
            else:
                require(value[3]==1,'missing vector consumer')
                for component in range(3):close(value[component],queries[2+5]['oracle_uv_w_m_s'][component])
    require(report['one_encoder_one_submission'] is True and report['intermediate_d2h_count']==0 and
        report['scoped_device_errors']==[] and report['scope']=='U/V/W query integration only','resident execution scope')
    return {'state':'PASS','values':90,'vector_values':45,'adapter':adapter['name'],
        'max_absolute_error_m_s':comparison['max_absolute_error']}

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('report',type=Path)
    group=parser.add_mutually_exclusive_group();group.add_argument('--software',action='store_true');group.add_argument('--hardware',action='store_true')
    args=parser.parse_args();print(json.dumps(check(args.report,args.software,args.hardware)))
