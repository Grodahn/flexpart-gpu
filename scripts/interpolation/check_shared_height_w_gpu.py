#!/usr/bin/env python3
"""Audit #188 actual four-control GPU preparation against genuine #186 SHARED rows."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import struct
from compare_interior_w_builds import verified_frozen, verified_run
from check_shared_height_gpu import require, require_f32_values

ROOT = Path(__file__).resolve().parents[2]


def digest(data):
    """Bind exact executed/uploaded bytes."""
    return hashlib.sha256(data).hexdigest()


def close(actual, expected):
    """Apply #80's unchanged finite additive policy in m/s."""
    require(math.isfinite(actual) and math.isfinite(expected), 'nonfinite value')
    require(abs(actual-expected) <= 1e-6 + 1e-5*max(abs(actual), abs(expected)), 'W comparison outside policy')


def equivalent(left, right):
    """Compare serde f32 shortest-roundtrip preimages with their Value representations."""
    if isinstance(left, dict) and isinstance(right, dict):
        return left.keys()==right.keys() and all(equivalent(left[k],right[k]) for k in left)
    if isinstance(left, list) and isinstance(right, list):
        return len(left)==len(right) and all(equivalent(a,b) for a,b in zip(left,right))
    if isinstance(left, float) and isinstance(right,(int,float)):
        return math.isfinite(left) and math.isfinite(right) and struct.pack('<f',left)==struct.pack('<f',right)
    return left==right


def check(path, software=False, hardware=False):
    """Reject missing, stale, unbound or numerically incomplete scientific evidence."""
    frozen = verified_frozen()
    oracle_path = os.environ.get('FLEXPART_GPU_INTERIOR_W_ORACLE')
    oracle = verified_run(Path(oracle_path).parent) if oracle_path else frozen
    record = json.loads(path.read_text())
    inputs_bytes = path.with_name('inputs.json').read_bytes()
    inputs = json.loads(inputs_bytes)
    evidence = record['gpu_evidence']
    require(evidence['schema'] == {'id':'flexpart-gpu.gpu-execution-evidence','version':1}, 'GPU schema')
    require(evidence['case_id'] == 'INTERIOR-W-PREP-188', 'wrong GPU case')
    require(re.fullmatch('[0-9a-f]{64}',record['candidate_executable_sha256']), 'missing candidate binary hash')
    require(set(record['source_sha256'])=={'src/gpu/shared_height_w.rs','src/gpu/shared_height.rs','tests/shared_height_w_gpu.rs'}, 'source inventory')
    for name,expected_hash in record['source_sha256'].items():
        require(expected_hash==digest((ROOT/name).read_bytes()), 'candidate source hash')
    candidate = evidence['candidate']
    require(candidate['implementation_id'] == 'shared_height_w_preparation_only', 'wrong implementation')
    revision = os.environ.get('FLEXPART_GPU_CANDIDATE_REVISION')
    require(re.fullmatch('[0-9a-f]{40}',candidate['revision']) and (not revision or candidate['revision']==revision), 'candidate revision')
    shader = b'\n'.join((ROOT/'src/shaders'/p).read_bytes() for p in
                       ('shared_height_uv.wgsl','shared_height_w_remap.wgsl','interior_w_slope.wgsl'))
    require(candidate['shader_sha256']==digest(shader), 'shader bundle')
    require(candidate['input_sha256']==digest(inputs_bytes), 'candidate input hash')
    consumer = re.search(r'const CONSUMER: &str = r"(.*?)";', (ROOT/'tests/shared_height_w_gpu.rs').read_text(), re.S)
    require(consumer and record['consumer_shader_sha256']==digest(consumer[1].encode()), 'consumer shader hash')
    execution=evidence['execution']; adapter=execution['adapter']
    require(execution['status']=='passed' and execution['calculation_path']=='wgsl_device' and
            execution['failure'] is None and execution['skip_reason'] is None, 'device did not execute')
    require(adapter and adapter['name'] and adapter['backend'] in ('vulkan','dx12','metal','gl','browser_webgpu'), 'missing adapter')
    require(adapter['device_type'] in ('other','integrated_gpu','discrete_gpu','virtual_gpu','cpu') and
            adapter['adapter_class']==('software_wgsl' if adapter['device_type']=='cpu' else 'hardware_gpu'), 'contradictory adapter')
    require(all(type(adapter[k]) is int for k in ('vendor_id','device_id')) and
            all(isinstance(adapter[k],str) for k in ('driver','driver_info')) and
            type(adapter['software_fallback_requested']) is bool, 'incomplete adapter')
    if software: require(adapter['adapter_class']=='software_wgsl', 'required software adapter')
    if hardware: require(adapter['adapter_class']=='hardware_gpu', 'required hardware adapter')
    provenance=oracle['provenance']
    require(record['oracle_provenance']==provenance, 'oracle provenance mismatch')
    require(evidence['oracle']['revision']==provenance['pinned_commit'] and
            evidence['oracle']['executable_sha256']==provenance['artifacts_sha256']['build/w-production-oracle'] and
            evidence['oracle']['output_sha256']==provenance['artifacts_sha256']['output.txt'], 'oracle hash mismatch')
    require(record['submission_count']==1 and record['intermediate_d2h_count']==0 and
            record['adjacent_reuse_checked'] is True and record['retained_dependencies_executed'] is True, 'resident execution/lifetime')
    expected={tuple(r['key']):r['height_uv_w'] for r in oracle['scientific']['shared_rows']}
    require(len(record['rows'])==len(expected)==288, 'incomplete control coverage')
    seen={};differences=[];relative=[]
    for row in record['rows']:
        key=tuple(row['key']);require(key in expected and key not in seen, 'duplicate/unsupported row')
        reference=expected[key];actual=row['actual_m_s']
        require(row['consumer_accepted']==1.0, 'status-aware consumer rejected')
        require(math.isfinite(actual),'nonfinite control')
        if key[0] in ('full','zero'):close(actual,reference[3])
        require(row['oracle_m_s']==reference[3] and row['difference_m_s']==abs(actual-reference[3]), 'unbound row')
        require(len(row['prepared_uv_m_s'])==2, 'missing U/V multipliers')
        for a,b in zip(row['prepared_uv_m_s'],reference[1:3]):close(a,b)
        seen[key]=actual
        if key[0]=='full':
            differences.append(abs(actual-reference[3]));scale=max(abs(actual),abs(reference[3]));relative.append(abs(actual-reference[3])/scale if scale else 0)
    require(seen.keys()==expected.keys(), 'missing SHARED rows')
    for m in (1,2):
        for x in range(3):
            for y in range(3):
                for k in range(1,5):
                    if x in (0,2) or y in (0,2) or k in (1,4):
                        require(len({seen[(v,m,x,y,k)] for v in ('full','x','y','zero')})==1, 'changed boundary/endpoint')
    require(len(record['contributions'])==4, 'missing independent slopes')
    for row, (m,k) in zip(record['contributions'], ((m,k) for m in (1,2) for k in (2,3))):
        require((row['memory'],row['level'])==(m,k), 'slope coverage')
        value=lambda v:seen[(v,m,1,1,k)]
        ref=lambda v:expected[(v,m,1,1,k)][3]
        x=value('x')-value('zero');y=value('y')-value('zero');full=value('full')-value('zero')
        for actual,reference in ((x,ref('x')-ref('zero')),(y,ref('y')-ref('zero')),(full,ref('full')-ref('zero')),(full,x+y)):close(actual,reference)
        require(x*y>0 and all(abs(v)>1e-6+1e-5*abs(v) for v in (x,y)), 'missing direction/cancellation')
        require(row['x_m_s']==x and row['y_m_s']==y and row['combined_m_s']==full and
                row['oracle_x_m_s']==ref('x')-ref('zero') and row['oracle_y_m_s']==ref('y')-ref('zero') and
                row['additive_residual_m_s']==full-x-y, 'unbound slope diagnostics')
        for wrong in (value('zero'),value('zero')+2*full):
            require(abs(wrong-ref('full'))>1e-6+1e-5*max(abs(wrong),abs(ref('full'))), 'correction not distinguished')
    for diagnostic in oracle['scientific']['normalization_diagnostics']:
        actual=seen[('zero',diagnostic['memory'],1,1,diagnostic['level'])]
        close(actual,diagnostic['direct_zero_m_s'])
        for name in ('omitted_diagnostic','double_normalization_diagnostic_m_s'):
            wrong=diagnostic[name];require(abs(actual-wrong)>1e-6+1e-5*max(abs(actual),abs(wrong)), 'normalization ambiguity')
    require(len(inputs['snapshots'])==len(inputs['native_motion'])==len(record['w_sources'])==len(record['uv_metadata'])==2, 'source count')
    require(record['final_w_status']==record['final_uv_status']==[[2]*9,[2]*9], 'preparation not completed')
    target=record['target']; heights=record['actual_target_height_m_agl']
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
        for key,value,serialized_key in (('snapshot',snapshot,'serialized_snapshots'),('native',native,'serialized_native_motion')):
            serialized=record[serialized_key][index];require(equivalent(json.loads(serialized),value), 'serialized '+key)
        require(source['snapshot_sha256']==digest(record['serialized_snapshots'][index].encode()) and
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
    comparison=evidence['comparison']
    require(comparison['verdict']=='passed' and comparison['first_failure'] is None and
            comparison['compared_value_count']==comparison['oracle_value_count']==comparison['candidate_value_count']==72 and
            comparison['policy']=={'absolute_tolerance':1e-6,'relative_tolerance':1e-5}, 'paired comparison')
    require(comparison['max_absolute_error']==max(differences) and comparison['max_relative_error']==max(relative), 'comparison summary')
    return {'state':'PASS','scope':'W preparation only','values':288,'max_absolute_error_m_s':max(differences),'adapter':adapter['name']}


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report',type=Path)
    group=parser.add_mutually_exclusive_group();group.add_argument('--software',action='store_true');group.add_argument('--hardware',action='store_true')
    args=parser.parse_args();print(json.dumps(check(args.report,args.software,args.hardware)))
