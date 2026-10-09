#!/usr/bin/env python3
"""Audit direct #118 output; competing algorithms are diagnostics, never oracles."""

import argparse
import json
import math
import os
import re
import struct
from pathlib import Path

from prepare_w_production_oracle import (
    ABSOLUTE_TOLERANCE_M_S,
    RELATIVE_TOLERANCE,
    canonical_text_sha256,
    git,
    sha256,
    verify_call_edges,
)


def require(condition, message):
    """Reject incomplete scientific evidence rather than producing a verdict."""
    if not condition:
        raise ValueError(message)


def close(a, b):
    """Reuse #80's predeclared combined velocity comparison policy."""
    return abs(a - b) <= ABSOLUTE_TOLERANCE_M_S + RELATIVE_TOLERANCE * max(abs(a), abs(b))


def f32(value):
    return struct.unpack('f', struct.pack('f', value))[0]


def decode_input(path):
    """Preserve every controlled thermodynamic, hybrid and motion input."""
    lines = [list(map(float, line.split())) for line in path.read_text().splitlines()]
    require(bool(lines) and lines[0] == [3] and len(lines) == 36, 'wrong input dimensions')
    coefficients = lines[1:5]
    require(all(len(row) == 2 for row in coefficients), 'invalid A/B input')
    require(coefficients[0] == [0, 1], 'hybrid surface must be actual local pressure')
    columns = []
    cursor = 6
    for memory in (1, 2):
        for x in (0, 1):
            surface = lines[cursor]
            require(len(surface) == 3, 'invalid surface input')
            rows = lines[cursor + 1:cursor + 5]
            require(all(len(row) == 5 for row in rows), 'invalid native input')
            require(rows[0][0] == surface[1] and rows[0][1] == rows[1][1],
                    'ground T/q must follow pinned readwind_ecmwf assignment')
            columns.append({'memory': memory, 'x': x, 'pressure_pa': surface[0],
                            'temperature_2m_k': surface[1], 'dewpoint_2m_k': surface[2],
                            'rows_t_k_q_kg_kg_u_m_s_v_m_s_omega_pa_s': rows})
            cursor += 5
    require(lines[cursor] == [9] and all(len(q) == 5 for q in lines[cursor+1:]), 'invalid query input')
    require(all(math.isfinite(v) for row in lines for v in row), 'non-finite raw input')
    return {'native_layers': 3, 'ordering': 'bottom_to_top', 'half_level_a_pa_b': coefficients,
            'terrain_m_asl': lines[5], 'columns_repeated_in_y': columns,
            'query_time_s_x_y_target_bracket_fraction': lines[cursor+1:],
            'motion': 'interface omega Pa/s positive pressure-increasing; normalized by pristine pinmconv',
            'artificial_ground': 'row 1 U/V represents 10m wind assigned to z=0; T/q supplied explicitly'}


def sample(heights, values, z):
    """Diagnostic linear sampling only; authoritative values come from Fortran."""
    require(len(heights) == len(values) and len(heights) >= 2, 'invalid profile')
    require(all(a < b for a, b in zip(heights, heights[1:])), 'unordered heights')
    if z <= heights[0]:
        return values[0]
    if z >= heights[-1]:
        return values[-1]
    for k in range(len(heights) - 1):
        if z <= heights[k + 1]:
            fraction = (z - heights[k]) / (heights[k + 1] - heights[k])
            return values[k] * (1 - fraction) + values[k + 1] * fraction
    raise AssertionError('bounded sampling missed bracket')


def decode(path):
    lines = path.read_text(encoding='utf-8').splitlines()
    require(bool(lines) and lines[0] == 'FLEXPART_SHARED_HEIGHT_ORACLE_V1', 'wrong oracle header')
    native, shared, queries = {}, {}, []
    for line in lines[1:]:
        tokens = line.split()
        tag = tokens[0]
        if tag in ('NATIVE', 'SHARED'):
            require(len(tokens) == (12 if tag == 'NATIVE' else 8), 'wrong field count')
            key = tuple(map(int, tokens[1:4]))
            target = native if tag == 'NATIVE' else shared
            require(key not in target, 'duplicate profile row')
            target[key] = list(map(float, tokens[4:]))
        elif tag == 'QUERY':
            require(len(tokens) == 10, 'wrong query count')
            queries.append([int(tokens[1]), int(tokens[2]), *map(float, tokens[3:])])
        else:
            raise ValueError('unknown oracle record')
    keys = {(m, x, k) for m in (1, 2) for x in (0, 1) for k in range(1, 5)}
    require(set(native) == keys and set(shared) == keys, 'incomplete 2x2x3 two-time evidence')
    require(len(queries) == 9 and [q[0] for q in queries] == list(range(1, 10)), 'incomplete queries')
    require(all(math.isfinite(v) for row in [*native.values(), *shared.values(), *queries] for v in row),
            'non-finite oracle value')
    return native, shared, queries


def investigate(native, shared, queries):
    comparisons = []
    for number, time, x, y, z, asl, *oracle in queries:
        require(0 < x < 1 and 0 < y < 1 and 0 <= time <= 3600, 'query outside supported domain')
        terrain_error = abs(asl - z - (120 * (1 - x) + 870 * x))
        require(terrain_error <= 0.02, 'terrain equivalence mismatch (#30 height tolerance)')
        methods = {'native_vertical_then_horizontal': [], 'averaged_native_geometry': [],
                   'shared_horizontal_vertical_temporal': []}
        for component in range(3):
            at_times = {name: [] for name in methods}
            for memory in (1, 2):
                rows = [[native[memory, col, k] for k in range(1, 5)] for col in (0, 1)]
                # U/V use native centers (including artificial surface). W uses
                # #30/#80's native interfaces and already converted omega.
                heights = [[r[0 if component < 2 else 1] for r in col] for col in rows]
                values = [[r[4 + component if component < 2 else 7] for r in col] for col in rows]
                direct = (1 - x) * sample(heights[0], values[0], z) + x * sample(heights[1], values[1], z)
                averaged_z = [(1 - x) * a + x * b for a, b in zip(*heights)]
                averaged_values = [(1 - x) * a + x * b for a, b in zip(*values)]
                shared_z = [shared[memory, 0, k][0] for k in range(1, 5)]
                shared_values = [(1 - x) * shared[memory, 0, k][component + 1] +
                                 x * shared[memory, 1, k][component + 1] for k in range(1, 5)]
                at_times['native_vertical_then_horizontal'].append(direct)
                at_times['averaged_native_geometry'].append(sample(averaged_z, averaged_values, z))
                at_times['shared_horizontal_vertical_temporal'].append(sample(shared_z, shared_values, z))
            for method, values_at_time in at_times.items():
                methods[method].append(values_at_time[0] * (1 - time / 3600) + values_at_time[1] * time / 3600)
        require(all(close(a, b) for a, b in zip(oracle, methods['shared_horizontal_vertical_temporal'])),
                'decoded shared-grid diagnostic disagrees with actual Fortran sample')
        comparisons.append({'query': number, 'time_s': time, 'x': x, 'y': y,
                            'height_m_agl': z, 'equivalent_height_m_asl_diagnostic': asl,
                            'oracle_uv_w_m_s': oracle, 'diagnostics_m_s': methods,
                            'absolute_differences_m_s': {method: [abs(a-b) for a,b in zip(oracle, values)]
                                                        for method, values in methods.items()}})
    for alternative in ('native_vertical_then_horizontal', 'averaged_native_geometry'):
        for component in range(3):
            require(any(not close(row['oracle_uv_w_m_s'][component],
                                  row['diagnostics_m_s'][alternative][component]) for row in comparisons),
                    f'fixture does not distinguish {alternative}, component {component}')
    # The three equal-height cases at time endpoints and midpoint must exercise
    # actual temporal blending, rather than identical stored meteorology.
    for component in range(3):
        a, mid, b = [comparisons[i]['oracle_uv_w_m_s'][component] for i in (5, 6, 7)]
        require(not close(a, b) and close(mid, (a+b)/2), 'temporal route not distinguished')
    return comparisons


def report(checkout, run):
    root = Path(__file__).resolve().parents[2]
    manifest = json.loads((root / 'reference/flexpart-11.1.json').read_text())
    require(git(checkout, 'rev-parse', 'HEAD') == manifest['pinned_commit'], 'wrong pinned revision')
    require(not git(checkout, 'status', '--porcelain'), 'dirty authoritative checkout')
    native, shared, queries = decode(run / 'output.txt')
    inputs = decode_input(run / 'input.txt')
    require(inputs['terrain_m_asl'] == [120, 870], 'unexpected terrain fixture')
    for col in inputs['columns_repeated_in_y']:
        for level, raw in enumerate(col['rows_t_k_q_kg_kg_u_m_s_v_m_s_omega_pa_s'], 1):
            row = native[col['memory'], col['x'], level]
            require(row[4:7] == list(map(f32, raw[2:])), 'raw wind input/output mismatch')
            if level == 1:
                require(row[2] == f32(col['pressure_pa']), 'raw pressure input/output mismatch')
    for raw, actual in zip(inputs['query_time_s_x_y_target_bracket_fraction'], queries):
        require(actual[1:4] == [int(raw[0]), f32(raw[1]), f32(raw[2])], 'raw query/output mismatch')
    target = [shared[1, 0, k][0] for k in range(1, 5)]
    require(target == [native[1, 0, k][0] for k in range(1, 5)], 'initializer did not select first high-pressure column')
    require(all([shared[m, x, k][0] for k in range(1, 5)] == target for m in (1, 2) for x in (0, 1)),
            'target grid changed across columns/times')
    require(any(native[1, 0, k][0] != native[1, 1, k][0] for k in range(2, 5)), 'uniform columns')
    require(any(native[1, 0, k][0] != native[2, 0, k][0] for k in range(2, 5)), 'time-invariant native geometry')
    edges = verify_call_edges((run / 'build/w-production-oracle.nm').read_text(),
                             (run / 'build/w-production-oracle.call-sites').read_text())
    link_map = (run / 'build/w-production-oracle.link-map').read_text()
    initializer = re.search(r'^__verttransform_mod_MOD_verttransform_init\s+[^\n]*verttransform_mod.o\n'
                            r'\s+w-production-oracle-driver.o$', link_map, re.MULTILINE)
    require(initializer is not None, 'initializer not linked from pristine object to driver')
    image_id = os.environ.get('RESEARCH_IMAGE_ID', '')
    require(re.fullmatch(r'sha256:[0-9a-f]{64}', image_id) is not None, 'missing resolved Docker image identity')
    sources = ['src/verttransform_mod.f90', 'src/windfields_mod.f90', 'src/interpol_mod.f90',
               'src/getfields_mod.f90', 'src/initialise_mod.f90', 'src/makefile_gfortran', 'src/par_mod.f90']
    for source in sources:
        require(canonical_text_sha256(checkout / source) ==
                canonical_text_sha256(run / 'build/upstream' / Path(source).name),
                f'linked routine source changed: {source}')
    artifacts = ['input.txt', 'output.txt', 'full-build.log', 'driver-build-run.log',
                 'build/w-production-oracle', 'build/w-production-oracle-driver.o',
                 'build/w-production-oracle.nm', 'build/w-production-oracle.call-sites',
                 'build/w-production-oracle.link-map', 'build/linked-objects.txt', 'build/compiler-identity.txt']
    linked = (run / 'build/linked-objects.txt').read_text().splitlines()
    require(linked == sorted(set(linked)) and len(linked) > 3 and 'FLEXPART.o' not in linked,
            'invalid linked upstream object inventory')
    return {'schema': {'id': 'flexpart-gpu.shared-height-research', 'version': 1}, 'issue': 118,
            'verdict': 'shared_grid_remapping_required_for_supported_route',
            'validation_level': 'direct_pinned_routines_synthetic_research_only',
            'comparison_policy': {'owner': 'issue #80', 'absolute_m_s': ABSOLUTE_TOLERANCE_M_S,
                                  'relative': RELATIVE_TOLERANCE,
                                  'formula': 'abs(a-b) <= absolute + relative*max(abs(a),abs(b))'},
            'scope': {'fields': ['U center', 'V center', 'omega interface -> geometric W'],
                      'horizontal': 'regional 2x2, repeated y columns, fractional x/y',
                      'time_s': [0, 3600], 'terrain_m_asl': [120, 870],
                      'height_reference': 'AGL; ASL equivalence is diagnostic caller conversion',
                      'slope': 'pristine regional boundary branch skips slope; interior W unsupported'},
            'target_height_m_agl': target,
            'decoded_inputs': inputs,
            'native_rows': [{'memory': m, 'x': x, 'level': k, 'height_m_agl': row[0],
                             'interface_height_m_agl': row[1], 'pressure_pa': row[2],
                             'pinmconv_m_pa': row[3], 'u_m_s': row[4], 'v_m_s': row[5],
                             'omega_pa_s': row[6], 'normalized_interface_w_m_s': row[7]}
                            for (m,x,k),row in sorted(native.items())],
            'shared_rows': [{'memory': m, 'x': x, 'level': k, 'height_m_agl': row[0],
                             'uv_w_m_s': row[1:]} for (m,x,k),row in sorted(shared.items())],
            'comparisons': investigate(native, shared, queries),
            'provenance': {'pinned_commit': manifest['pinned_commit'], 'checkout_clean': True,
                           'compiler': (run / 'build/compiler-identity.txt').read_text().strip(),
                           'docker_image_id': image_id,
                           'initializer_link_reference_verified': True,
                           'build_profile': manifest['execution_profile'],
                           'driver_flags': ['-O0', '-fopenmp', '-mcmodel=large'],
                           'sources_sha256': {s: canonical_text_sha256(checkout / s) for s in sources},
                           'research_sources_sha256': {p: canonical_text_sha256(root / p) for p in [
                               'scripts/interpolation/shared_height_oracle.f90',
                               'scripts/interpolation/shared_height_oracle.sh',
                               'scripts/interpolation/run_shared_height_oracle.py',
                               'scripts/interpolation/prepare_shared_height_oracle.py',
                               'scripts/interpolation/w_production_oracle.sh',
                               'scripts/interpolation/prepare_w_production_oracle.py',
                               'reference/flexpart-11.1.json']},
                           'linked_objects_sha256': {p: sha256(run / 'build/upstream' / p) for p in linked},
                           'artifacts_sha256': {p: sha256(run / p) for p in artifacts},
                           'verified_call_edges': edges}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkout', type=Path, required=True)
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--expected', type=Path, required=True)
    args = parser.parse_args()
    result = report(args.checkout, args.run)
    (args.run / 'report.json').write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    # Binary/build hashes belong to each run, not a cross-environment equality
    # claim. Frozen raw scientific output and stable source identities must match.
    frozen = args.expected / 'report.json'
    if frozen.exists():
        expected = json.loads(frozen.read_text(encoding='utf-8'))
        for key in ('target_height_m_agl', 'decoded_inputs', 'native_rows', 'shared_rows', 'comparisons', 'scope', 'comparison_policy'):
            require(result[key] == expected[key], f'frozen decoded evidence changed: {key}')
        for key in ('pinned_commit', 'sources_sha256', 'research_sources_sha256'):
            require(result['provenance'][key] == expected['provenance'][key], f'provenance changed: {key}')
        require(sha256(args.run / 'output.txt') == sha256(args.expected / 'output.txt'), 'raw output changed')
        require(canonical_text_sha256(args.run / 'input.txt') == canonical_text_sha256(args.expected / 'input.txt'),
                'raw input changed')
    else:
        raise ValueError('frozen evidence absent; inspect report and freeze it explicitly')
    print(json.dumps({'state': 'PASS', 'verdict': result['verdict'], 'queries': len(result['comparisons']),
                      'report': str(args.run / 'report.json')}))


if __name__ == '__main__':
    main()
