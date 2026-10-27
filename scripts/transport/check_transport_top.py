#!/usr/bin/env python3
"""Audit #198 real-driver state against immutable #180 outputs; optionally require a fresh oracle."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess

from compare_top_boundary_builds import retained
from verify_top_boundary_oracle import require

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / 'fixtures/transport/top-boundary-v1/report.json'
SOURCE_FILES = {
    'src/gpu/advection_resident.rs', 'src/gpu/meteorology/transport_boundary.rs',
    'src/gpu/meteorology/transport_top_tests.rs', 'src/simulation/timeloop/forward/timestep.rs',
    'src/simulation/timeloop/forward/operators.rs', 'src/simulation/timeloop/backward.rs',
    'src/simulation/timeloop/forward.rs', 'src/gpu/meteorology/advection_production_tests.rs', 'tests/support/shared_height.rs',
    'src/gpu/meteorology.rs', 'src/simulation/timeloop/meteorology.rs', 'src/simulation/timeloop/forcing.rs',
}
SHADERS = {
    'transport_top_fixture_assign', 'advection_predictor_query', 'advection_corrector', 'advection_step_guard', 'advection_step_commit',
    'particle_query', 'horizontal_interpolation', 'vertical_sample', 'temporal_interpolation',
    'shared_height_query_status', 'resident_sample_status', 'resident_status_reset',
    'resident_query_adapter', 'shared_height_uv', 'shared_height_w_remap', 'interior_w_slope',
}
START = 1704067200


def digest(data):
    return hashlib.sha256(data).hexdigest()


def f32(value):
    return struct.unpack('<f', struct.pack('<f', value))[0]


def artifact(root, row, key):
    raw = (root / row[key + '_file']).read_bytes()
    require(digest(raw) == row[key + '_sha256'], 'changed ' + key + ' artifact')
    return json.loads(raw)


def velocity(a, b):
    """Existing #80/#190 combined velocity policy, exclusively for sampled m/s."""
    return math.isfinite(a) and math.isfinite(b) and abs(a-b) <= 1e-6 + 1e-5 * max(abs(a), abs(b))


def source_inputs(inputs, geometry, direction):
    """Bind thermodynamic source inputs independently to the unchanged #118/#180 controls."""
    baseline = json.loads((ROOT / 'fixtures/interpolation/shared-height-v1/report.json').read_text())['decoded_inputs']
    columns = {(c['memory'], c['x']): c for c in baseline['columns_repeated_in_y']}
    snapshots = [json.loads(s) for s in inputs['snapshot_json']]
    for member, snapshot in enumerate(snapshots, 1):
        grid = snapshot['horizontal_grid']
        require((grid['nx'],grid['ny'],grid['xlon0_deg'],grid['ylat0_deg'],grid['dx_deg'],grid['dy_deg']) == (2,2,0,0,1,1), 'original regional grid')
        coordinate = snapshot['vertical_coordinate']
        require(coordinate['hybrid_a_interface_pa'] == [10000,10000,5000,0]
                and coordinate['hybrid_b_interface'] == [0.1,0.3,0.7,1.0], 'original hybrid coefficients')
        fields = {f['id']:f for f in snapshot['fields']}
        for name, lane, key in (('surface_pressure',None,'pressure_pa'),('temperature',0,None),
                               ('specific_humidity',1,None),('temperature2m',None,'temperature_2m_k'),
                               ('dewpoint2m',None,'dewpoint_2m_k')):
            expected = []
            for z in range(3 if lane is not None else 1):
                for _ in range(2):
                    for x in range(2):
                        c = columns[(1,0)] if geometry == 0 else columns[(member,x)]
                        expected.append(c[key] if key else c['rows_t_k_q_kg_kg_u_m_s_v_m_s_omega_pa_s'][3-z][lane])
            field = fields[name]
            require(list(map(f32,field['values'])) == list(map(f32,expected)), 'original source input ' + name)
            require(field['time']['valid_time_epoch_seconds'] == START+(member-1)*direction*3600, 'original source validity')
        require(list(map(f32,fields['orography']['values'])) == list(map(f32,baseline['terrain_m_asl']*2)), 'original terrain')
        native_json = inputs['native_motion_json'][member-1]
        native = json.loads(native_json)
        require(native['kind'] == 'pressure_velocity_omega' and native['values'] == [0]*16, 'explicit zero-omega preparation input')
        normalization = inputs['runtimes'][member-1]['vertical_velocity']['provenance']
        require(digest(native_json.encode()) == normalization['source_native_motion_sha256'], 'native normalization hash')
    return snapshots


def check(path, software=False, hardware=False, oracle_run=None):
    report = json.loads(path.read_text())
    require(report['schema'] == 'flexpart-gpu.transport-top.v1' and report['execution'] == 'wgsl_device', 'device schema/execution')
    revision = os.environ.get('FLEXPART_GPU_CANDIDATE_REVISION') or subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    require(report['candidate_revision'] == revision, 'stale candidate revision')
    require(report['oracle_fixture_sha256'] == digest(FIXTURE.read_bytes()), 'changed oracle fixture')
    require(set(report['source_hashes']) == SOURCE_FILES and set(report['shader_hashes']) == SHADERS, 'incomplete source/shader inventory')
    for file, expected in report['source_hashes'].items():
        require(digest((ROOT / file).read_bytes()) == expected, 'source hash ' + file)
    for shader, expected in report['shader_hashes'].items():
        require(digest((ROOT / 'src/shaders' / (shader + '.wgsl')).read_bytes()) == expected, 'shader hash ' + shader)
    require(len(report['candidate_executable_sha256']) == 64, 'candidate executable identity')
    oracle = json.loads(FIXTURE.read_text())
    provenance = None
    if oracle_run is not None:
        fresh, _ = retained(oracle_run)
        require(fresh['science'] == oracle['science'], 'fresh scientific outputs differ')
        provenance = fresh['provenance']
    original = {r[0]: r for r in oracle['science']['rows']['ADVANCE']}
    require(len(report['rows']) == 72 and {r['oracle_id'] for r in report['rows']} == set(original), 'missing/duplicate original cases')
    verdicts = []
    placements = []
    adapters = []
    for row in report['rows']:
        ref = original[row['oracle_id']]
        _, geometry, direction, epoch, scenario, z0, old, end, slope, _, _, final_z, _, nstop = ref
        adapter = row['adapter']
        adapters.append(adapter)
        require(adapter['name'] and adapter['backend'] and adapter['driver_info'] is not None, 'adapter provenance')
        expected_class = 'software_wgsl' if adapter['device_type'] == 'cpu' else 'hardware_gpu'
        require(adapter['adapter_class'] == expected_class, 'adapter classification')
        require(not software or expected_class == 'software_wgsl', 'software adapter required')
        require(not hardware or expected_class == 'hardware_gpu', 'hardware adapter required')
        inputs = artifact(path.parent, row, 'input')
        target = inputs['target']
        require(inputs['nxmax_from_nxfield'] == target['initial_source']['grid']['nx'] == 2, 'nxmax provenance')
        require(target['initial_source']['time']['valid_time_epoch_seconds'] == START, 'run initializer changed with source chronology')
        require(inputs['device_target'] == target['height_m_agl'] and inputs['device_target'][-1] == 9521.84375
                and inputs['target_status'] == [2], 'device target/status')
        require(digest(struct.pack('<4f', *inputs['device_target'])) == target['height_sha256'], 'target hash')
        preimage = json.loads(inputs['target_identity_json'])
        require(digest(inputs['target_identity_json'].encode()) == target['identity_sha256'], 'target identity preimage hash')
        require(list(map(f32,preimage['height_m_agl'])) == list(map(f32,target['height_m_agl'])), 'target identity f32 height bytes')
        preimage['height_m_agl'] = target['height_m_agl']
        require(preimage == dict(target,identity_sha256=''), 'target identity preimage contents')
        require(inputs['initial_z'] == z0, 'initial state')
        snapshots = source_inputs(inputs, geometry, direction)
        require(len(snapshots) == len(inputs['runtimes']) == len(inputs['controlled_shared_winds']) == 2, 'source counts')
        ordered = [0, 1] if direction == 1 else [1, 0]
        for source, index in zip(inputs['controlled_shared_winds'], ordered):
            uv = source['prepared_w']['prepared_uv']
            require(uv['target'] == target and uv['source']['snapshot_sha256'] == digest(inputs['snapshot_json'][index].encode()), 'source target/snapshot identity')
            require(uv['source']['geometry_provenance'] == inputs['runtimes'][index]['provenance'], 'geometry provenance')
            require(source['prepared_w']['source']['normalization'] == inputs['runtimes'][index]['vertical_velocity']['provenance'], 'normalization identity')
            require(source['source_time']['valid_time_epoch_seconds'] == START + index * 3600 * direction, 'source chronology')
            require(snapshots[index]['horizontal_grid'] == target['initial_source']['grid'], 'mother grid identity')
            fraction = f32(f32(index * 3600 - epoch * 1200) / 600)
            u = f32(f32(direction * 0.5) / f32(600 * f32(1 / 111200)))
            ws = [f32(direction * f32(f32(old + f32(f32(end-old)*fraction)) + f32(slope * (i % 2)))) for i in range(16)]
            require(list(map(f32,source['u'])) == [u]*16 and source['v'] == [0]*16 and list(map(f32,source['w'])) == ws, 'original shared forcing input controls')
        if scenario == 9:
            require(row['settling_rejected'] is True and 'sticky particle-stop state' in row['error'], 'unavailable settling accepted')
            require(len(row['initial_bytes']) == 131*96 and row['initial_bytes'] == row['final_bytes'], 'settling rejection changed scientific particles')
            verdicts.append({'oracle_id':ref[0], 'verdict':'UNSUPPORTED_SETTLING_REJECTED', 'original_nstop':nstop})
            continue
        require(row['settling_rejected'] is False and nstop is False, 'ordinary particle stop/fatal distinction')
        device = artifact(path.parent, row, 'device')
        require(device['status'] == ([0]+[2]*130+[0])*6+[0], 'failed or incomplete device status')
        require(len(device['queries']) == 262 and len(device['sampled_values']) == 786 and len(device['metadata']) == 6, 'device output shape')
        require(all(math.isfinite(v) for v in device['sampled_values']), 'nonfinite sampled vector')
        corrected = bytes(device['corrected_bytes'])
        published = bytes(device['published_bytes'])
        require(len(corrected) == len(published) == 131*96 and corrected[130*96:] == published[130*96:], 'inactive tail integrity')
        require(struct.unpack_from('<i', corrected, 130*96+64)[0] == -123, 'inactive sentinel')
        top = inputs['device_target'][-1]
        placed = oracle['science']['observed_placement_m_agl']
        for stage in (0,1):
            for component, field in enumerate(('wind_u','wind_v','vertical_velocity')):
                metadata = device['metadata'][stage*3+component]
                require(metadata['field_id'] == field and metadata['time']['instantaneous']['epoch_seconds'] == START + (epoch*1200+stage*600)*direction, 'signed query time')
                shared = metadata['shared_height']
                require(shared['preparation'] == 'shared_height_uvw_190' and len(shared['corrected_w']) == 2
                        and shared['target_identity_sha256'] == target['identity_sha256']
                        and shared['target_height_sha256'] == target['height_sha256'], 'complete prepared target identity')
                require(metadata['source_snapshot_sha256'] == [digest(inputs['snapshot_json'][i].encode()) for i in ordered], 'sample source identity')
                for p, w in zip(inputs['controlled_shared_winds'],shared['corrected_w']):
                    require(w['normalization'] == p['prepared_w']['source']['normalization']
                            and w['prepared_uv_sha256'] == p['prepared_w']['source']['prepared_uv_sha256'], 'sample W normalization/correction identity')
        for lane in range(130):
            current, predictor = (device['queries'][lane+i*131] for i in (0,1))
            require(current['active'] == predictor['active'] == 1, 'active query')
            u0 = device['sampled_values'][lane]
            predictor_x = f32(0.25 + f32(f32(u0*f32(1/111200))*f32(600*direction)))
            require(current['cell_x'] == predictor['cell_x'] == 0 and current['fraction_x'] == 0.25 and predictor['fraction_x'] == predictor_x
                    and current['fraction_y'] == predictor['fraction_y'] == 0, 'actual sampling coordinates')
            require(current['height_agl_m'] == z0, 'current sample height')
            w0 = device['sampled_values'][2*131+lane]
            expected_u = f32(f32(direction*0.5) / f32(600*f32(1/111200)))
            for stage in (0,1):
                require(velocity(device['sampled_values'][stage*3*131+lane],expected_u)
                        and velocity(device['sampled_values'][(stage*3+1)*131+lane],0), 'sampled U/V policy')
            raw = f32(f32(z0) + f32(w0*f32(600*direction)))
            expected_predictor = placed if raw >= top else max(0,raw)
            require(predictor['height_agl_m'] == expected_predictor, 'boundary must precede advanced sampling')
            require(velocity(w0,direction*(old+slope*current['fraction_x'])) and velocity(device['sampled_values'][5*131+lane],direction*(end+slope*predictor['fraction_x'])), 'sampled W policy')
            z = struct.unpack_from('<f',corrected,lane*96+8)[0]
            require(math.isfinite(z), 'nonfinite advection state')
            require(struct.unpack_from('<I',corrected,lane*96+20)[0] & 1, 'ordinary particle incorrectly stopped')
            if scenario != 8:
                require(z == f32(final_z), 'exact original f32 projection: no trajectory tolerance')
            if final_z == placed:
                require(z == placed, 'exact ordinary f32 placement')
                placements.append(z)
        verdicts.append({'oracle_id':ref[0], 'verdict':'NO_TRAJECTORY_POLICY_204' if scenario == 8 else 'EXACT_F32_PROJECTION', 'device_z_f32':z, 'exact_projection_equal':z == f32(final_z), 'original_z_f64':final_z,
                         'original_z_f32':f32(final_z), 'ordinary_nstop':False})
    require(len(placements) == 5200 and all(a == adapters[0] for a in adapters), 'placement/adapter coverage')
    p = provenance or oracle['provenance']
    gpu_evidence = {
        'schema':{'id':'flexpart-gpu.gpu-execution-evidence','version':1},
        'case_id':'ADV-GPU-TOP-198-EXACT-ORDINARY-PLACEMENT',
        'candidate':{'implementation_id':'real_forward_backward_resident_petterssen_ordinary_top',
                     'revision':revision,'shader_sha256':digest(json.dumps(report['shader_hashes'],sort_keys=True).encode()),
                     'input_sha256':digest(json.dumps([r['input_sha256'] for r in report['rows']]).encode())},
        'execution':{'status':'passed','calculation_path':'wgsl_device','adapter':adapters[0], 'failure':None,'skip_reason':None},
        'oracle':{'implementation_id':'FLEXPART-11.1::advance/pushpartdown (exact ordinary placement only)',
                  'revision':p['pinned_commit'],'executable_sha256':p['artifacts_sha256']['build/w-production-oracle'],
                  'output_sha256':p['artifacts_sha256']['output.txt']},
        'comparison':{'verdict':'passed','policy':{'absolute_tolerance':0.0,'relative_tolerance':0.0},
                      'oracle_value_count':5200,'candidate_value_count':5200,'compared_value_count':5200,
                      'max_absolute_error':0.0,'max_relative_error':0.0,'first_failure':None},
    }
    return {'state':'PASS','validation_level':'real_driver_advection_stage', 'candidate_revision':revision,
            'reference_revision':oracle['provenance']['pinned_commit'], 'fresh_oracle_provenance':provenance,
            'ordinary_advance_rows':64,'exact_projection_rows':56,'trajectory_diagnostic_rows':8,'rejected_settling_rows':8,'f64_only_push_probes':'EXCLUDED_UNREPRESENTABLE',
            'comparison':'boundary/action and exact f32 projection for scenarios 1-7; scenario 8 has no trajectory verdict (#204)',
            'scientific_verdict':'BOUNDARY_ACTION_VERIFIED_TRAJECTORY_PARITY_NOT_ESTABLISHED',
            'gpu_evidence':gpu_evidence,'verdicts':verdicts, 'candidate_report_sha256':digest(path.read_bytes())}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--oracle-run',type=Path)
    group = parser.add_mutually_exclusive_group()
    group.add_argument('--software',action='store_true')
    group.add_argument('--hardware',action='store_true')
    args = parser.parse_args()
    result = check(args.report,args.software,args.hardware,args.oracle_run)
    output = args.report.with_name('fresh-comparison.json' if args.oracle_run else 'fixture-comparison.json')
    output.write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({k:v for k,v in result.items() if k not in ('fresh_oracle_provenance','verdicts','gpu_evidence')}))


if __name__ == '__main__':
    main()
