#!/usr/bin/env python3
"""Fail closed on incomplete #184 device evidence; science stays with #118/#80."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import struct

ROOT = Path(__file__).resolve().parents[2]


def require(condition, message):
    """Reject absent or inconsistent proof rather than reporting a successful skip."""
    if not condition:
        raise ValueError(message)


def require_f32_values(actual, expected, message):
    """Audit canonical f32 input bytes without reconstructing scientific geometry."""
    require(len(actual) == len(expected), message)
    require(all(math.isfinite(v) for v in actual + expected), message)
    require(struct.pack(f'<{len(actual)}f', *actual) ==
            struct.pack(f'<{len(expected)}f', *expected), message)


def check_oracle_inputs(snapshot, oracle, memory):
    """Bind uploaded canonical meteorology to the genuine driver's explicit inputs."""
    inputs = oracle['decoded_inputs']
    coordinate = snapshot['vertical_coordinate']
    require(coordinate['kind'] == 'hybrid_sigma_pressure' and
            coordinate['reference'] == 'model_native' and
            coordinate['ordering'] == 'increasing', 'oracle coordinate contract')
    require(len(coordinate['level_values']) == inputs['native_layers'], 'oracle native count')
    half_levels = inputs['half_level_a_pa_b'][::-1]
    for component, key in enumerate(('hybrid_a_interface_pa', 'hybrid_b_interface')):
        require_f32_values(coordinate[key], [row[component] for row in half_levels], 'oracle hybrid coefficients')
    columns = sorted((c for c in inputs['columns_repeated_in_y'] if c['memory'] == memory),
                     key=lambda c: c['x'])
    require([c['x'] for c in columns] == [0, 1], 'oracle columns')
    fields = {f['id']: f for f in snapshot['fields']}
    require(len(fields) == len(snapshot['fields']), 'duplicate canonical field')
    specs = {
        'temperature': ('kelvin', 'signed_scalar', 0),
        'specific_humidity': ('kilogram_per_kilogram', 'non_negative', 1),
        'wind_u': ('meter_per_second', 'positive_eastward', 2),
        'wind_v': ('meter_per_second', 'positive_northward', 3),
        'surface_pressure': ('pascal', 'non_negative', 'pressure_pa'),
        'temperature2m': ('kelvin', 'signed_scalar', 'temperature_2m_k'),
        'dewpoint2m': ('kelvin', 'signed_scalar', 'dewpoint_2m_k'),
        'orography': ('meter', 'signed_scalar', 'terrain'),
        'wind_u10m': ('meter_per_second', 'positive_eastward', 'u10'),
        'wind_v10m': ('meter_per_second', 'positive_northward', 'v10'),
    }
    for name, (unit, sign, lane) in specs.items():
        field = fields[name]
        volume = isinstance(lane, int)
        require(field['shape'] == ([2, 2, 3] if volume else [2, 2]) and
                field['axis_order'] == (['x', 'y', 'z'] if volume else ['x', 'y']) and
                field['storage_order'] == 'x_fastest', 'oracle field layout')
        require(field['unit'] == unit and field['sign'] == sign and
                field['horizontal_staggering'] == 'cell_center' and
                field['vertical_staggering'] == ('level_center' if volume else 'not_applicable'),
                'oracle physical metadata')
        time = field['time']
        require(time['kind'] == ('static' if name == 'orography' else 'instantaneous') and
                time['valid_time_epoch_seconds'] == oracle['scope']['time_s'][memory - 1] and
                time['calendar'] == 'gregorian' and time['interval_start_epoch_seconds'] is None and
                time['interval_end_epoch_seconds'] is None and time['accumulation'] is None,
                'oracle source time')
        expected = []
        for z in range(inputs['native_layers'] if volume else 1):
            for _y in range(2):
                for x, column in enumerate(columns):
                    rows = column['rows_t_k_q_kg_kg_u_m_s_v_m_s_omega_pa_s']
                    if volume:
                        value = rows[inputs['native_layers'] - z][lane]
                    elif lane == 'terrain':
                        value = inputs['terrain_m_asl'][x]
                    elif lane in ('u10', 'v10'):
                        value = rows[0][2 if lane == 'u10' else 3]
                    else:
                        value = column[lane]
                    expected.append(value)
        require_f32_values(field['values'], expected, f'oracle input {name}')


def check(path, software=False, hardware=False):
    """Audit every supported row against the selected frozen or fresh direct oracle."""
    record = json.loads(path.read_text())
    oracle_path = Path(os.environ.get('FLEXPART_GPU_SHARED_HEIGHT_ORACLE',
                                     ROOT / 'fixtures/interpolation/shared-height-v1/report.json'))
    oracle = json.loads(oracle_path.read_text())
    pin = json.loads((ROOT / 'reference/flexpart-11.1.json').read_text())['pinned_commit']
    evidence = record['gpu_evidence']
    require(evidence['case_id'] == 'SHARED-HEIGHT-UV-184', 'wrong GPU case')
    require(evidence['schema'] == {'id': 'flexpart-gpu.gpu-execution-evidence', 'version': 1}, 'schema')
    execution = evidence['execution']
    require(execution['status'] == 'passed' and execution['calculation_path'] == 'wgsl_device', 'device did not execute')
    require(execution['failure'] is None and execution['skip_reason'] is None, 'failed/skipped device')
    adapter = execution['adapter']
    require(adapter and adapter['name'] and adapter['backend'], 'missing adapter provenance')
    require(adapter['backend'] in ('vulkan', 'metal', 'dx12', 'gl', 'browser_webgpu'), 'invalid backend')
    require(adapter['device_type'] in ('other', 'integrated_gpu', 'discrete_gpu', 'virtual_gpu', 'cpu'), 'invalid device type')
    require(adapter['adapter_class'] == ('software_wgsl' if adapter['device_type'] == 'cpu' else 'hardware_gpu'), 'adapter class contradicts device type')
    require(all(type(adapter[key]) is int and 0 <= adapter[key] <= 0xffffffff for key in ('vendor_id', 'device_id')) and
            all(isinstance(adapter[key], str) for key in ('driver', 'driver_info')) and
            type(adapter['software_fallback_requested']) is bool, 'incomplete adapter provenance')
    if software:
        require(adapter['adapter_class'] == 'software_wgsl', 'required software device')
    if hardware:
        require(adapter['adapter_class'] == 'hardware_gpu', 'required hardware device')
    provenance = oracle['provenance']
    require(provenance['pinned_commit'] == pin and provenance['checkout_clean'], 'wrong/dirty oracle')
    require(record['oracle_provenance'] == provenance, 'unbound oracle provenance')
    require(evidence['oracle']['revision'] == pin, 'oracle revision')
    require(evidence['oracle']['executable_sha256'] == provenance['artifacts_sha256']['build/w-production-oracle'], 'oracle binary')
    require(evidence['oracle']['output_sha256'] == provenance['artifacts_sha256']['output.txt'], 'oracle output')
    candidate = evidence['candidate']
    require(candidate['implementation_id'] == 'shared_height_uv_preparation_only' and
            isinstance(candidate['revision'], str) and candidate['revision'].strip(), 'missing candidate identity')
    require(candidate['shader_sha256'] == hashlib.sha256((ROOT / 'src/shaders/shared_height_uv.wgsl').read_bytes()).hexdigest(), 'shader identity')
    require(candidate['input_sha256'] == hashlib.sha256(path.with_name('inputs.json').read_bytes()).hexdigest(), 'candidate input identity')
    consumer = re.search(r'const CONSUMER: &str = r"(.*?)";',
                         (ROOT / 'tests/shared_height_gpu.rs').read_text(), re.DOTALL)
    require(consumer and record['consumer_shader_sha256'] ==
            hashlib.sha256(consumer[1].encode()).hexdigest(), 'consumer shader identity')
    revision = os.environ.get('FLEXPART_GPU_CANDIDATE_REVISION')
    if revision:
        require(candidate['revision'] == revision, 'stale candidate revision')
    require(record['one_encoder_one_submission'] is True, 'missing composed execution')
    expected = {(r['memory'], r['x'], r['level']): r for r in oracle['shared_rows']}
    required = {(m, x, y, z, c) for m in (1, 2) for x in (0, 1)
                for y in (0, 1) for z in range(1, 5) for c in (0, 1)}
    seen = set()
    for row in record['rows']:
        key = (row['memory'], row['x'], row['y'], row['level'], row['component'])
        require(key in required and key not in seen, 'duplicate/unsupported row')
        seen.add(key)
        actual = row['actual_m_s']
        reference = expected[(key[0], key[1], key[3])]['uv_w_m_s'][key[4]]
        require(math.isfinite(actual) and math.isfinite(reference), 'nonfinite velocity')
        difference = abs(actual-reference)
        limit = 1e-6 + 1e-5*max(abs(actual), abs(reference))
        require(row['oracle_m_s'] == reference and difference == row['absolute_difference_m_s'], 'unbound comparison')
        require(row['allowed_difference_m_s'] == limit and difference <= limit, 'U/V tolerance')
        require(difference <= 1e-6 or difference <= 1e-5 * max(abs(actual), abs(reference)),
                'generic comparison contradicts passing verdict')
    require(seen == required, 'missing supported rows')
    target = record['target']
    serialized_target = record['serialized_target_identity']
    target_preimage = dict(target, identity_sha256='')
    decoded_target = json.loads(serialized_target)
    # Direct serde f32 serialization is shortest-roundtrip; serde Value uses f64 numbers.
    require_f32_values(decoded_target['height_m_agl'], target_preimage['height_m_agl'], 'target preimage heights')
    decoded_target['height_m_agl'] = target_preimage['height_m_agl']
    require(decoded_target == target_preimage and
            hashlib.sha256(serialized_target.encode()).hexdigest() == target['identity_sha256'], 'target provenance hash')
    require(target['selection_rule'] == 'regional_mother_nonrestart_first_y_then_x_ps_gt_100000_at_0_0_ground_zero', 'target selection rule')
    heights = record['actual_target_height_m_agl']
    require(len(heights) == 4 and heights == target['height_m_agl'], 'target bytes')
    digest = hashlib.sha256(struct.pack('<4f', *heights)).hexdigest()
    require(target['height_sha256'] == digest and target['selected_xy'] == [0, 0], 'target identity/selection')
    for actual, reference in zip(heights, oracle['target_height_m_agl']):
        require(math.isfinite(actual) and abs(actual-reference) <= max(0.02, 1e-5*abs(reference)), '#30 target tolerance')
    require(len(record['sources']) == 2 and record['sources'][0] == target['initial_source'], 'source/target ownership')
    inputs = json.loads(path.with_name('inputs.json').read_text())
    require(len(record['serialized_sources']) == len(inputs) == 2, 'source input count')
    for memory, (source, serialized, snapshot) in enumerate(zip(record['sources'], record['serialized_sources'], inputs), 1):
        check_oracle_inputs(snapshot, oracle, memory)
        source_hash = hashlib.sha256(serialized.encode()).hexdigest()
        require(source_hash == source['snapshot_sha256'] and json.loads(serialized) == snapshot, 'source hash')
        require(source['geometry_provenance']['source_snapshot_sha256'] == source_hash, '#30 source binding')
        provenance = source['geometry_provenance']
        require(provenance['source_schema_id'] == snapshot['schema']['id'] and
                provenance['source_schema_version'] == snapshot['schema']['version'] and
                provenance['source_vertical_ordering'] == 'increasing' and
                provenance['source_level_count'] == 3 and
                provenance['pressure_algorithm_id'] == 'hybrid_interface_ab_local_ps_fulllevel_adjacent_mean_v1' and
                provenance['height_algorithm_id'] == 'flexpart11_verttransform_ecmwf_heights_v1' and
                provenance['w_height_algorithm_id'] == 'flexpart11_wzlev_from_uvzlev_v1', '#30 algorithm binding')
        geometry_identity = (f'snapshot={source_hash}:ordering=Increasing:levels=3:nx=2:ny=2:'
                             f'pressure={provenance["pressure_algorithm_id"]}:'
                             f'height={provenance["height_algorithm_id"]}:'
                             f'wheight={provenance["w_height_algorithm_id"]}')
        require(source['geometry_identity'] == geometry_identity, '#30 geometry identity')
        require(all(re.fullmatch('[0-9a-f]{64}', source[key]) for key in
                    ('device_context_identity', 'input_sha256')), 'incomplete source identity')
        require(source['levels'] == 4 and source['grid']['nx'] == source['grid']['ny'] == 2, 'source shape')
        require(source['grid'] == snapshot['horizontal_grid'] and
                source['time'] == next(f['time'] for f in snapshot['fields'] if f['id'] == 'wind_u'), 'source grid/time identity')
    require(record['adjacent_bracket_reuse_checked'] and record['changed_source_unchanged_target_checked'], 'reuse proof')
    require(record['sources'][0]['snapshot_sha256'] != record['sources'][1]['snapshot_sha256'], 'distinct source identity')
    require(record['sources'][0]['device_context_identity'] == record['sources'][1]['device_context_identity'], 'device context mismatch')
    require(len(record['resource_metadata']) == 2, 'missing physical metadata')
    for metadata, source in zip(record['resource_metadata'], record['sources']):
        require(metadata['fields'] == ['wind_u', 'wind_v'] and metadata['unit'] == 'meter_per_second', 'field/unit metadata')
        require(metadata['horizontal_staggering'] == 'cell_center' and metadata['vertical_staggering'] == 'level_center', 'staggering metadata')
        require(metadata['signs'] == ['positive_eastward', 'positive_northward'] and
                metadata['height_reference'] == 'above_ground_level' and
                metadata['storage_layout'] == '[target-level,y,x]; x fastest', 'physical resource contract')
        require(metadata['source'] == source and metadata['target'] == target, 'resource identity mismatch')
    comparison = evidence['comparison']
    require(comparison['verdict'] == 'passed' and comparison['compared_value_count'] == 64, 'missing paired verdict')
    require(comparison['oracle_value_count'] == comparison['candidate_value_count'] == 64 and
            comparison['first_failure'] is None and
            comparison['policy'] == {'absolute_tolerance': 1e-6, 'relative_tolerance': 1e-5}, 'inconsistent paired comparison')
    differences = [r['absolute_difference_m_s'] for r in record['rows']]
    relative = [difference / max(abs(r['actual_m_s']), abs(r['oracle_m_s']))
                if max(abs(r['actual_m_s']), abs(r['oracle_m_s'])) else 0.0
                for r, difference in zip(record['rows'], differences)]
    require(comparison['max_absolute_error'] == max(differences) and
            comparison['max_relative_error'] == max(relative), 'unbound error summary')
    return {'state': 'PASS', 'scope': 'U/V preparation only', 'values': len(seen), 'adapter': adapter['name']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report', type=Path)
    adapter = parser.add_mutually_exclusive_group()
    adapter.add_argument('--software', action='store_true')
    adapter.add_argument('--hardware', action='store_true')
    args = parser.parse_args()
    print(json.dumps(check(args.report, args.software, args.hardware)))
