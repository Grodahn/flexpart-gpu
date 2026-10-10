#!/usr/bin/env python3
"""Fail closed on incomplete #184 device evidence; science stays with #118/#80."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[2]


def require(condition, message):
    """Reject absent or inconsistent proof rather than reporting a successful skip."""
    if not condition:
        raise ValueError(message)


def check(path, software=False, hardware=False):
    """Audit every supported row against the selected frozen or fresh direct oracle."""
    record = json.loads(path.read_text())
    oracle_path = Path(os.environ.get('FLEXPART_GPU_SHARED_HEIGHT_ORACLE',
                                     ROOT / 'fixtures/interpolation/shared-height-v1/report.json'))
    oracle = json.loads(oracle_path.read_text())
    pin = json.loads((ROOT / 'reference/flexpart-11.1.json').read_text())['pinned_commit']
    evidence = record['gpu_evidence']
    require(evidence['schema'] == {'id': 'flexpart-gpu.gpu-execution-evidence', 'version': 1}, 'schema')
    execution = evidence['execution']
    require(execution['status'] == 'passed' and execution['calculation_path'] == 'wgsl_device', 'device did not execute')
    require(execution['failure'] is None and execution['skip_reason'] is None, 'failed/skipped device')
    adapter = execution['adapter']
    require(adapter and adapter['name'] and adapter['backend'], 'missing adapter provenance')
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
    require(candidate['shader_sha256'] == hashlib.sha256((ROOT / 'src/shaders/shared_height_uv.wgsl').read_bytes()).hexdigest(), 'shader identity')
    require(candidate['input_sha256'] == hashlib.sha256(path.with_name('inputs.json').read_bytes()).hexdigest(), 'candidate input identity')
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
    require(seen == required, 'missing supported rows')
    target = record['target']
    heights = record['actual_target_height_m_agl']
    require(len(heights) == 4 and heights == target['height_m_agl'], 'target bytes')
    digest = hashlib.sha256(struct.pack('<4f', *heights)).hexdigest()
    require(target['height_sha256'] == digest and target['selected_xy'] == [0, 0], 'target identity/selection')
    for actual, reference in zip(heights, oracle['target_height_m_agl']):
        require(math.isfinite(actual) and abs(actual-reference) <= max(0.02, 1e-5*abs(reference)), '#30 target tolerance')
    require(len(record['sources']) == 2 and record['sources'][0] == target['initial_source'], 'source/target ownership')
    inputs = json.loads(path.with_name('inputs.json').read_text())
    require(len(record['serialized_sources']) == len(inputs) == 2, 'source input count')
    for source, serialized, snapshot in zip(record['sources'], record['serialized_sources'], inputs):
        source_hash = hashlib.sha256(serialized.encode()).hexdigest()
        require(source_hash == source['snapshot_sha256'] and json.loads(serialized) == snapshot, 'source hash')
        require(source['geometry_provenance']['source_snapshot_sha256'] == source_hash, '#30 source binding')
        require(source['levels'] == 4 and source['grid']['nx'] == source['grid']['ny'] == 2, 'source shape')
    require(record['adjacent_bracket_reuse_checked'] and record['changed_source_unchanged_target_checked'], 'reuse proof')
    require(record['sources'][0]['snapshot_sha256'] != record['sources'][1]['snapshot_sha256'], 'distinct source identity')
    require(record['sources'][0]['device_context_identity'] == record['sources'][1]['device_context_identity'], 'device context mismatch')
    require(len(record['resource_metadata']) == 2, 'missing physical metadata')
    for metadata, source in zip(record['resource_metadata'], record['sources']):
        require(metadata['fields'] == ['wind_u', 'wind_v'] and metadata['unit'] == 'meter_per_second', 'field/unit metadata')
        require(metadata['horizontal_staggering'] == 'cell_center' and metadata['vertical_staggering'] == 'level_center', 'staggering metadata')
        require(metadata['source'] == source and metadata['target'] == target, 'resource identity mismatch')
    comparison = evidence['comparison']
    require(comparison['verdict'] == 'passed' and comparison['compared_value_count'] == 64, 'missing paired verdict')
    return {'state': 'PASS', 'scope': 'U/V preparation only', 'values': len(seen), 'adapter': adapter['name']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report', type=Path)
    adapter = parser.add_mutually_exclusive_group()
    adapter.add_argument('--software', action='store_true')
    adapter.add_argument('--hardware', action='store_true')
    args = parser.parse_args()
    print(json.dumps(check(args.report, args.software, args.hardware)))
