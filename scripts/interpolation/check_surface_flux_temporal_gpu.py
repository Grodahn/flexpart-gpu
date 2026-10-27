#!/usr/bin/env python3
"""Fail closed on incomplete #120 device evidence; science stays with the direct oracle."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[2]
FIELDS = ['sensible_heat_flux', 'surface_solar_radiation',
          'surface_stress_eastward', 'surface_stress_northward']
ABSOLUTE, RELATIVE = 1.0e-6, 1.0e-5


def require(condition, message):
    """Reject absent or inconsistent proof rather than reporting a successful skip."""
    if not condition:
        raise ValueError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def f32_bytes(values):
    return struct.pack(f'<{len(values)}f', *values)


def candidate_revision():
    revision = os.environ.get('FLEXPART_GPU_CANDIDATE_REVISION')
    if revision:
        return revision
    return subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()


def passes(actual, expected):
    difference = abs(actual - expected)
    return difference <= ABSOLUTE or difference <= RELATIVE * max(abs(actual), abs(expected))


def check_inputs(record, inputs, oracle):
    """Bind uploaded canonical snapshots to the frozen oracle declaration."""
    require([json.loads(s) for s in record['serialized_sources']] == inputs, 'serialized sources')
    serialized = ('[' + ','.join(record['serialized_sources']) + ']').encode()
    require(record['input_sha256'] == sha256(serialized), 'input hash')
    require(len(inputs) == len(oracle['snapshot_times_s']), 'snapshot count')
    for index, (snapshot, time) in enumerate(zip(inputs, oracle['snapshot_times_s'])):
        fields = {f['id']: f for f in snapshot['fields']}
        for spec in oracle['fields']:
            field = fields[spec['canonical_field']]
            require(field['unit'] == spec['unit'] and field['sign'] == spec['canonical_sign'],
                    'canonical unit/sign')
            require(field['shape'] == [oracle['grid']['nx'], oracle['grid']['ny']] and
                    field['storage_order'] == 'x_fastest', 'canonical layout')
            require(field['time'] == {'calendar': 'gregorian', 'kind': 'instantaneous',
                                      'valid_time_epoch_seconds': time,
                                      'interval_start_epoch_seconds': None,
                                      'interval_end_epoch_seconds': None, 'accumulation': None},
                    'canonical instantaneous time')
            require(f32_bytes(field['values']) ==
                    f32_bytes(spec['canonical_values_per_snapshot'][index]), 'uploaded values')
            factor = spec['flexpart_to_canonical_factor']
            require(spec['canonical_values_per_snapshot'][index] ==
                    [factor * v for v in spec['flexpart_values_per_snapshot'][index]], 'sign mapping')


def check_field(field, oracle, record, software):
    name = field['canonical_field']
    records = [r for r in oracle['records'] if r['canonical_field'] == name]
    require(len(records) == 77 and field['compared_values'] == 77, f'{name}: compared value count')
    expected = {('isolated_89' if r['node'] else 'composed_76', r['itime_s'], r['xt'], r['yt']): r
                for r in records}
    seen = set()
    for row in field['rows']:
        key = (row['path'], row['itime_s'], row['xt'], row['yt'])
        require(key in expected and key not in seen, f'{name}: unexpected or duplicate row {key}')
        seen.add(key)
        source = expected[key]
        # Oracle outputs are Fortran real(4); bind at that precision because the
        # Rust JSON reader may differ from Python in the last f64 bit.
        require(f32_bytes([row['oracle_canonical_value'], row['flexpart_value']]) ==
                f32_bytes([source['canonical_value'], source['flexpart_value']]) and
                row['application'] == source['application'], f'{name}: oracle binding')
        require(math.isfinite(row['gpu_value']) and passes(row['gpu_value'], source['canonical_value']),
                f'{name}: value outside the #74/#89 policy at {key}')
    require(seen == set(expected), f'{name}: missing oracle rows')
    evidence = field['gpu_evidence']
    execution = evidence['execution']
    require(execution['status'] == 'passed' and execution['calculation_path'] == 'wgsl_device' and
            execution['adapter'] is not None, f'{name}: device execution')
    adapter_class = execution['adapter']['adapter_class']
    require(adapter_class == ('software_wgsl' if software else adapter_class) and
            adapter_class in ('software_wgsl', 'hardware_gpu'), f'{name}: adapter class')
    require((execution['adapter']['device_type'] == 'cpu') == (adapter_class == 'software_wgsl'),
            f'{name}: adapter classification')
    candidate = evidence['candidate']
    require(candidate['revision'] == record['candidate_revision'] and
            candidate['shader_sha256'] == record['shader_sha256']['combined'] and
            candidate['input_sha256'] == record['input_sha256'], f'{name}: candidate identity')
    provenance = oracle['provenance']
    require(evidence['oracle'] == {
        'implementation_id': 'FLEXPART-11.1 hor_interpol + temporal_interpolation on memory slots',
        'revision': provenance['pinned_commit'],
        'executable_sha256': provenance['artifacts_sha256']['build/surface-flux-temporal-oracle'],
        'output_sha256': provenance['artifacts_sha256']['output.txt']}, f'{name}: oracle identity')
    comparison = evidence['comparison']
    require(comparison['verdict'] == 'passed' and comparison['oracle_value_count'] == 77 and
            comparison['candidate_value_count'] == 77 and comparison['compared_value_count'] == 77 and
            comparison['first_failure'] is None,
            f'{name}: comparison verdict')
    require(comparison['policy'] == {'absolute_tolerance': ABSOLUTE, 'relative_tolerance': RELATIVE},
            f'{name}: comparison policy')
    require(field['verdict'] == 'PASS' and record['verdicts'][name] == {
        'instantaneous': 'PASS', 'interval_mean_through_instantaneous_path': 'UNSUPPORTED',
        'accumulated': 'UNSUPPORTED'}, f'{name}: verdicts')
    source_hashes = [sha256(s.encode()) for s in record['serialized_sources']]
    require(len(field['handoffs']) == 5, f'{name}: handoff count')
    for handoff in field['handoffs']:
        metadata = handoff['metadata']
        bracket = metadata['temporal_bracket']
        lower, upper = bracket['lower_index'], bracket['upper_index']
        require(handoff['stages'] == [
            {'stage': 'horizontal', 'source_index': lower, 'plane_index': 0},
            {'stage': 'horizontal', 'source_index': upper, 'plane_index': 0},
            {'stage': 'temporal', 'source_index': None, 'plane_index': None}], f'{name}: stage order')
        require(handoff['device_copies'] > 0, f'{name}: device-resident handoff')
        spec = next(s for s in oracle['fields'] if s['canonical_field'] == name)
        require(metadata['field_id'] == name and metadata['unit'] == spec['unit'] and
                metadata['sign'] == spec['canonical_sign'] and metadata['value_count'] == 1,
                f'{name}: handoff identity')
        require(bracket['sampling_basis'] == 'point_in_time_surface_flux', f'{name}: sampling basis')
        require(metadata['source_snapshot_sha256'] == [source_hashes[lower], source_hashes[upper]],
                f'{name}: source snapshot identity')
        require([t['valid_time_epoch_seconds'] for t in metadata['source_times']] ==
                [oracle['snapshot_times_s'][lower], oracle['snapshot_times_s'][upper]] and
                all(t['kind'] == 'instantaneous' for t in metadata['source_times']),
                f'{name}: source times')


def check(path, oracle_path, software=False):
    record = json.loads(Path(path).read_text(encoding='utf-8'))
    oracle = json.loads(Path(oracle_path).read_text(encoding='utf-8'))
    raw_inputs = (Path(path).parent / 'inputs.json').read_bytes()
    inputs = json.loads(raw_inputs)
    require(record['input_sha256'] == sha256(raw_inputs), 'retained input bytes')
    require(record['schema'] == {'id': 'flexpart-gpu.surface-flux-temporal-gpu-evidence', 'version': 1},
            'schema')
    require(record['issue'] == 120 and record['candidate_revision'] == candidate_revision(), 'revision')
    pin = json.loads((ROOT / 'reference/flexpart-11.1.json').read_text())['pinned_commit']
    require(oracle['provenance']['pinned_commit'] == pin and oracle['provenance']['checkout_clean'],
            'oracle pin')
    require(record['oracle_provenance'] == oracle['provenance'], 'oracle provenance')
    horizontal = sha256((ROOT / 'src/shaders/horizontal_interpolation.wgsl').read_bytes())
    temporal = sha256((ROOT / 'src/shaders/temporal_interpolation.wgsl').read_bytes())
    require(record['shader_sha256'] == {'horizontal': horizontal, 'temporal': temporal,
                                        'combined': sha256((horizontal + temporal).encode())},
            'shader identity')
    require(record['comparison_policy'] == {'owner': '#74/#89', 'absolute': ABSOLUTE, 'relative': RELATIVE},
            'comparison policy')
    require(record['one_encoder_one_submission_per_field'] is True and
            record['intermediate_host_readbacks'] == 0, 'residency')
    require(record['software_adapter'] == (record['adapter']['adapter_class'] == 'software_wgsl'),
            'adapter flag')
    if software:
        require(record['adapter']['adapter_class'] == 'software_wgsl', 'software adapter required')
    check_inputs(record, inputs, oracle)
    require([f['canonical_field'] for f in record['fields']] == FIELDS and
            sorted(record['verdicts']) == sorted(FIELDS), 'approved field set')
    for field in record['fields']:
        check_field(field, oracle, record, software)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report', type=Path)
    parser.add_argument('--oracle', type=Path,
                        default=Path(os.environ.get('FLEXPART_GPU_SURFACE_FLUX_ORACLE',
                                                    ROOT / 'fixtures/interpolation/surface-flux-temporal-v1/report.json')))
    parser.add_argument('--software', action='store_true')
    args = parser.parse_args()
    check(args.report, args.oracle, args.software)
    print(json.dumps({'state': 'PASS', 'fields': FIELDS, 'report': str(args.report)}))


if __name__ == '__main__':
    main()
