#!/usr/bin/env python3
"""Audit complete #185 public U/V query evidence against the direct #118 oracle."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import struct

from check_shared_height_gpu import check_oracle_inputs, require

ROOT = Path(__file__).resolve().parents[2]
SHADERS = ('shared_height_uv', 'shared_height_query_status', 'resident_query_adapter',
           'horizontal_interpolation', 'vertical_sample', 'temporal_interpolation',
           'particle_query', 'resident_status_reset', 'resident_sample_status', 'resident_fixture_consumer')
CASES = {'static-identities-scope-time-asl', 'adjacent-bracket-reuse', 'cross-workgroup',
         'inactive-tail', 'reuse-after-fatal', 'empty-prefix', 'unexecuted-preparation',
         'pending-column', 'failed-column', 'nonfinite-preparation', 'failed-target',
         'nonfinite-corner', 'incomplete-lane', 'inactive-interior', 'nonfinite-query'}
SOURCES = ('src/gpu/meteorology.rs', 'src/gpu/meteorology/resident.rs',
           'src/gpu/meteorology/shared_uv.rs', 'src/gpu/shared_height.rs',
           'tests/shared_height_query.rs', 'tests/support/shared_height.rs')


def digest(path):
    """Hash the bytes actually consumed by the device fixture."""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check(path, software=False, hardware=False):
    """Reject incomplete rows, stale identities, skipped execution and weakened comparisons."""
    report = json.loads(path.read_text())
    oracle = json.loads(Path(os.environ.get('FLEXPART_GPU_SHARED_HEIGHT_ORACLE',
                        ROOT / 'fixtures/interpolation/shared-height-v1/report.json')).read_text())
    evidence = report['evidence']
    require(report['schema'] == {'id': 'flexpart-gpu.shared-height-query', 'version': 1}, 'schema')
    require(evidence['schema'] == {'id': 'flexpart-gpu.gpu-execution-evidence', 'version': 1}, 'execution schema')
    require(evidence['case_id'] == 'SHARED-HEIGHT-UV-QUERY-185', 'case identity')
    candidate = evidence['candidate']
    revision = os.environ.get('FLEXPART_GPU_CANDIDATE_REVISION') or subprocess.check_output(
        ['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    require(candidate['revision'] == revision, 'stale candidate revision')
    require(candidate['implementation_id'] == 'canonical_shared_height_uv_queries', 'implementation identity')
    inputs_path = path.with_name('inputs.json')
    require(candidate['input_sha256'] == digest(inputs_path), 'input hash')
    snapshots = json.loads(inputs_path.read_text())
    require(len(snapshots) == 2, 'source count')
    for memory, snapshot in enumerate(snapshots, 1):
        check_oracle_inputs(snapshot, oracle, memory)
    expected_shaders = {name: digest(ROOT / f'src/shaders/{name}.wgsl') for name in SHADERS}
    require(report['shaders'] == expected_shaders, 'shader identity')
    require(report['source_hashes'] == {p: digest(ROOT / p) for p in SOURCES}, 'source identity')
    require(candidate['shader_sha256'] == hashlib.sha256(json.dumps(
        expected_shaders, sort_keys=True, separators=(',', ':')).encode()).hexdigest(), 'shader bundle hash')
    execution = evidence['execution']
    require(execution['status'] == 'passed' and execution['calculation_path'] == 'wgsl_device'
            and execution['failure'] is None and execution['skip_reason'] is None, 'device execution')
    adapter = execution['adapter']
    require(adapter['name'] and adapter['backend'] in ('vulkan', 'dx12', 'metal', 'gl', 'browser_webgpu'), 'adapter identity')
    require(adapter['adapter_class'] == ('software_wgsl' if adapter['device_type'] == 'cpu' else 'hardware_gpu'), 'adapter class')
    require(not software or adapter['adapter_class'] == 'software_wgsl', 'required software execution')
    require(not hardware or adapter['adapter_class'] == 'hardware_gpu', 'required hardware execution')
    provenance = oracle['provenance']
    pin = json.loads((ROOT / 'reference/flexpart-11.1.json').read_text())['pinned_commit']
    require(provenance['checkout_clean'] is True and provenance['pinned_commit'] == pin, 'clean pinned oracle')
    require(report['oracle_provenance'] == provenance and evidence['oracle']['revision'] == pin, 'oracle identity')
    for key, artifact in [('executable_sha256', 'build/w-production-oracle'), ('output_sha256', 'output.txt')]:
        require(evidence['oracle'][key] == provenance['artifacts_sha256'][artifact], 'direct oracle artifact')
    expected_keys = {(api, query, component) for api in ('#76', '#171') for query in range(1, 10) for component in range(2)}
    rows = report['rows']
    require(len(rows) == 36 and {(r['api'], r['query'], r['component']) for r in rows} == expected_keys, 'complete unique U/V query rows')
    queries = {row['query']: row for row in oracle['comparisons']}
    f32_bits = lambda value: struct.unpack('<I', struct.pack('<f', value))[0]
    expected_queries = [[q['time_s'], 0, 0, f32_bits(q['x']), f32_bits(q['y']), f32_bits(q['height_m_agl'])]
                        for q in oracle['comparisons']]
    require(report['query_inputs'] == expected_queries and report['query_input_sha256'] == hashlib.sha256(
        json.dumps(expected_queries, separators=(',', ':')).encode()).hexdigest(), 'exact split particle query inputs')
    for row in rows:
        expected = queries[row['query']]['oracle_uv_w_m_s'][row['component']]
        actual = row['actual_m_s']
        require(math.isfinite(actual) and math.isfinite(expected), 'nonfinite comparison')
        error = abs(actual - expected)
        limit = 1e-6 + 1e-5 * max(abs(actual), abs(expected))
        require(row['oracle_m_s'] == expected and error <= limit, 'direct QUERY mismatch')
        require(row['absolute_difference_m_s'] == error and row['allowed_difference_m_s'] == limit, 'comparison metrics')
    comparison = evidence['comparison']
    require(comparison['verdict'] == 'passed' and comparison['compared_value_count'] == 36
            and comparison['oracle_value_count'] == comparison['candidate_value_count'] == 36
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
    heights = report['actual_target_height_m_agl']
    require(heights == target['height_m_agl'] and len(heights) == 4 and
            hashlib.sha256(struct.pack('<4f', *heights)).hexdigest() == target['height_sha256'], 'device target bytes')
    for actual, expected in zip(heights, oracle['target_height_m_agl']):
        require(math.isfinite(actual) and abs(actual-expected) <= max(0.02, 1e-5*abs(expected)), '#30 target tolerance')
    serialized_target = report['serialized_target_identity']
    preimage = json.loads(serialized_target)
    require(struct.pack('<4f', *preimage['height_m_agl']) == struct.pack('<4f', *heights), 'target preimage heights')
    preimage['height_m_agl'] = target['height_m_agl']
    require(preimage == dict(target, identity_sha256='') and
            hashlib.sha256(serialized_target.encode()).hexdigest() == target['identity_sha256'], 'target provenance hash')
    require(len(report['serialized_sources']) == 2, 'source serializations')
    for source, snapshot, serialized in zip(report['sources'], snapshots, report['serialized_sources']):
        require(json.loads(serialized) == snapshot and
                hashlib.sha256(serialized.encode()).hexdigest() == source['snapshot_sha256'], 'source snapshot hash')
        require(source['grid'] == snapshot['horizontal_grid'], 'source grid')
        require(source['time'] == next(f['time'] for f in snapshot['fields'] if f['id'] == 'wind_u'), 'source time')
        require(source['geometry_provenance']['source_snapshot_sha256'] == source['snapshot_sha256'], 'geometry lineage')
    for metadata_name in ('host_metadata', 'resident_metadata'):
        require(len(report[metadata_name]) == 18, 'metadata count')
        for index, metadata in enumerate(report[metadata_name]):
            require(metadata['field_id'] == ('wind_u' if index < 9 else 'wind_v') and
                    metadata['unit'] == 'meter_per_second' and metadata['vertical_staggering'] == 'level_center', 'physical metadata')
            shared = metadata['shared_height']
            require(shared == {'preparation': 'shared_height_uv_184',
                    'target_identity_sha256': target['identity_sha256'], 'target_height_sha256': target['height_sha256'],
                    'source_input_sha256': [s['input_sha256'] for s in report['sources']]}, 'prepared source/target metadata')
            require(metadata['source_snapshot_sha256'] == [s['snapshot_sha256'] for s in report['sources']]
                    and metadata['geometry_identity'] == [s['geometry_identity'] for s in report['sources']]
                    and metadata['source_times'] == [s['time'] for s in report['sources']], 'selected source metadata')
            query = queries[index % 9 + 1]
            selection = metadata['request']['time'] if metadata_name == 'host_metadata' else metadata['time']
            require(selection == {'instantaneous': {'calendar': 'gregorian', 'epoch_seconds': query['time_s']}}, 'query time')
            if metadata_name == 'host_metadata':
                request = metadata['request']
                require(f32_bits(request['xt']) == f32_bits(query['x']) and
                        f32_bits(request['yt']) == f32_bits(query['y']) and
                        f32_bits(request['height']['meters']) == f32_bits(query['height_m_agl']) and
                        request['height']['reference'] == 'above_ground_level', 'host query coordinates')
    cases = {case['case']: case for case in report['negative_cases']}
    require(len(report['negative_cases']) == len(CASES) and set(cases) == CASES
            and all(c['passed'] is True for c in cases.values()), 'failure/reuse coverage')
    for name in CASES - {'static-identities-scope-time-asl', 'adjacent-bracket-reuse', 'inactive-tail', 'reuse-after-fatal', 'empty-prefix', 'inactive-interior'}:
        require(cases[name]['status']['fatal'] is True, 'fatal checkpoint '+name)
    require(cases['cross-workgroup']['status']['lanes'][-1] == 3 and cases['incomplete-lane']['status']['lanes'][-1] == 9, 'cross-workgroup/incomplete lane')
    for name, active in [('inactive-tail', 64), ('reuse-after-fatal', 130), ('empty-prefix', 0)]:
        require(cases[name]['status'] == {'fatal': False, 'lanes': [2]*active + [0]*(130-active)}, 'active prefix/reset '+name)
    expected = [2]*130
    expected[1] = 0
    require(cases['inactive-interior']['status'] == {'fatal': False, 'lanes': expected}, 'inactive interior lane')
    require(cases['nonfinite-query']['status']['lanes'][-1] == 5, 'nonfinite query status')
    reuse = cases['adjacent-bracket-reuse']
    require(math.isfinite(reuse['value_m_s']) and reuse['value_m_s'] == reuse['retained_value_m_s'], 'adjacent bracket reuse value')
    require(report['one_encoder_one_submission'] is True and report['scope'] == 'U/V query integration only', 'composition/scope')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report', type=Path)
    parser.add_argument('--software', action='store_true')
    parser.add_argument('--hardware', action='store_true')
    args = parser.parse_args()
    check(args.report, args.software, args.hardware)
    print('SHARED-HEIGHT-UV-QUERY-185: 36 direct oracle comparisons and all status/identity cases passed')
