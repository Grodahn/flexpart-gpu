#!/usr/bin/env python3
"""Emit and audit the direct #120 surface-flux temporal oracle evidence.

The authoritative values are produced by pinned FLEXPART 11.1 routines in
``surface_flux_temporal_oracle.f90``. This module only writes the declared
input, decodes raw Fortran output, checks coverage and provenance, and records
the canonical sign mapping. Its diagnostic recomputations never replace the
Fortran values.
"""

import argparse
import json
import math
import os
import re
import struct
from pathlib import Path

from prepare_w_production_oracle import canonical_text_sha256, git, sha256

SCHEMA = {'id': 'flexpart-gpu.surface-flux-temporal-oracle', 'version': 1}
GRID_NX, GRID_NY = 4, 3
SNAPSHOT_TIMES_S = [0, 10800, 21600]
# Reuse the #74/#89 temporal comparison policy; #120 adds no tolerance.
ABSOLUTE_TOLERANCE = 1.0e-6
RELATIVE_TOLERANCE = 1.0e-5

# FLEXPART-convention values per snapshot (x-fastest, 4x3). SSHF follows the
# ECMWF downward-positive convention consumed by obukhov (getfields_mod.f90:1449).
FIELDS = [
    {'index': 1, 'flexpart_variable': 'sshf', 'grib_param_id': 146,
     'canonical_field': 'sensible_heat_flux', 'unit': 'watt_per_square_meter',
     'canonical_sign': 'positive_upward_flux', 'flexpart_sign': 'positive_downward',
     'flexpart_to_canonical_factor': -1.0,
     'flexpart_memory_array': 'windfields_mod::sshf(:,:,1,memind(m))',
     'values': [
         [42.5, 31.25, 18.0, 7.75, 25.5, 12.0, -3.5, -12.25, 8.0, -6.5, -20.75, -35.0],
         [-60.25, -95.5, -130.0, -152.75, -88.0, -121.5, -170.25, -198.0, -110.5, -149.0, -205.5, -240.25],
         [-185.0, -230.75, -262.5, -301.25, -210.5, -255.0, -298.75, -340.0, -240.25, -287.5, -333.0, -372.5]]},
    {'index': 2, 'flexpart_variable': 'ssr', 'grib_param_id': 176,
     'canonical_field': 'surface_solar_radiation', 'unit': 'watt_per_square_meter',
     'canonical_sign': 'non_negative', 'flexpart_sign': 'non_negative_after_readwind_clip',
     'flexpart_to_canonical_factor': 1.0,
     'flexpart_memory_array': 'windfields_mod::ssr(:,:,1,memind(m))',
     'values': [
         [0.0, 0.0, 3.5, 12.25, 0.0, 6.75, 21.5, 40.0, 2.25, 18.5, 47.75, 80.5],
         [210.5, 265.25, 318.0, 371.75, 240.0, 299.5, 352.25, 405.0, 268.75, 330.0, 388.5, 447.25],
         [512.25, 580.5, 641.0, 702.75, 545.5, 610.25, 676.0, 735.5, 579.0, 648.75, 712.5, 781.25]]},
    {'index': 3, 'flexpart_variable': 'ewss', 'grib_param_id': 180,
     'canonical_field': 'surface_stress_eastward', 'unit': 'newton_per_square_meter',
     'canonical_sign': 'positive_eastward', 'flexpart_sign': 'positive_eastward',
     'flexpart_to_canonical_factor': 1.0,
     'flexpart_memory_array': None,
     'values': [
         [0.125, 0.0625, -0.03125, -0.09375, 0.15625, 0.078125, 0.0, -0.0625, 0.1875, 0.09375, 0.015625, -0.046875],
         [0.25, 0.1875, 0.09375, -0.015625, 0.28125, 0.21875, 0.125, 0.03125, 0.3125, 0.234375, 0.15625, 0.0625],
         [0.40625, 0.34375, 0.25, 0.1328125, 0.4375, 0.375, 0.28125, 0.1875, 0.46875, 0.390625, 0.3125, 0.21875]]},
    {'index': 4, 'flexpart_variable': 'nsss', 'grib_param_id': 181,
     'canonical_field': 'surface_stress_northward', 'unit': 'newton_per_square_meter',
     'canonical_sign': 'positive_northward', 'flexpart_sign': 'positive_northward',
     'flexpart_to_canonical_factor': 1.0,
     'flexpart_memory_array': None,
     'values': [
         [-0.0703125, -0.109375, -0.15625, -0.203125, -0.046875, -0.078125, -0.125, -0.171875,
          -0.015625, -0.0546875, -0.09375, -0.140625],
         [0.0390625, 0.0078125, -0.03125, -0.0859375, 0.0625, 0.0234375, -0.015625, -0.0546875,
          0.09375, 0.046875, 0.0078125, -0.0390625],
         [0.171875, 0.140625, 0.09375, 0.046875, 0.1953125, 0.15625, 0.1171875, 0.0703125,
          0.21875, 0.1875, 0.140625, 0.09375]]},
]

NODES = [(x, y) for y in range(GRID_NY) for x in range(GRID_NX)]
# FLEXPART keeps two memory slots; getfields_mod.f90:159-177 shifts to the
# next pair once itime reaches memtime(2), so 10800 s uses the second pair.
PAIRS = [
    {'snapshots': (0, 1), 'node_times_s': [0, 3600, 7777],
     'fractional': [(3600, 0.25, 0.5), (7777, 1.625, 1.375)]},
    {'snapshots': (1, 2), 'node_times_s': [10800, 14400, 21600],
     'fractional': [(10800, 0.25, 0.5), (14400, 2.75, 0.125), (21600, 1.625, 1.375)]},
]

# Source-level crosswalk owned by the #120 decision (docs/temporal-interpolation.md).
SOURCE_TRACE = {
    'flexpart_revision_files': ['src/windfields_mod.f90', 'src/getfields_mod.f90',
                                'src/interpol_mod.f90', 'src/drydepo_mod.f90'],
    'flex_extract_files': ['Source/Python/Classes/EcFlexpart.py', 'Source/Python/Mods/disaggregation.py'],
    'preprocessing': 'flex_extract EcFlexpart.deacc_fluxes (EcFlexpart.py:1123-1143) de-accumulates '
                     'J/m2 or N m-2 s by /3600/dtime, then disaggregation.dapoly '
                     '(disaggregation.py:62-103) reconstructs a point value at the central '
                     'boundary of four intervals and writes it with stepRange 0 at that validity time '
                     '(EcFlexpart.py:1180-1211); period-edge records are not reconstructions (1193-1250)',
    'reader': 'readwind_ecmwf stores values unscaled per memory slot (windfields_mod.f90:2351-2386); '
              'SSR < 0 is clipped to 0; EWSS/NSSS remain locals and form sfcstress (2543-2579); '
              'missing fluxes trigger the pbl_profile fallback (2583-2609)',
    'consumer': 'calcpar(memind(n)) consumes values at memtime(n) only (getfields_mod.f90:976-1067); '
                'derived ustar/wstar/oli (interpol_mod.f90:706-737), hmix (1119-1158) and vdep '
                '(drydepo_mod.f90:805-814) are interpolated in time',
    'original_production_temporal_interpolation_of_field': False,
}


def require(condition, message):
    """Reject incomplete evidence rather than producing a verdict."""
    if not condition:
        raise ValueError(message)


def f32(value):
    return struct.unpack('f', struct.pack('f', value))[0]


def close(a, b):
    return abs(a - b) <= ABSOLUTE_TOLERANCE or abs(a - b) <= RELATIVE_TOLERANCE * max(abs(a), abs(b))


def queries(pair):
    rows = [(t, float(x), float(y)) for t in pair['node_times_s'] for x, y in NODES]
    return rows + [(t, x, y) for t, x, y in pair['fractional']]


def input_text():
    """Serialize the declared fixture into the Fortran driver input."""
    lines = [f'{GRID_NX} {GRID_NY}', str(len(PAIRS))]
    for pair in PAIRS:
        lines.append(' '.join(str(SNAPSHOT_TIMES_S[s]) for s in pair['snapshots']))
        for field in FIELDS:
            for snapshot in pair['snapshots']:
                lines.append(' '.join(repr(v) for v in field['values'][snapshot]))
        rows = queries(pair)
        lines.append(str(len(rows)))
        lines.extend(f'{t} {x!r} {y!r}' for t, x, y in rows)
    return '\n'.join(lines) + '\n'


def validate_declaration():
    require(all(len(v) == GRID_NX * GRID_NY for f in FIELDS for v in f['values']), 'grid mismatch')
    require(all(f32(v) == v and math.isfinite(v) for f in FIELDS for s in f['values'] for v in s),
            'declared values must be finite and f32-exact')
    require(min(v for s in FIELDS[1]['values'] for v in s) >= 0, 'SSR must be non-negative')
    for field in (FIELDS[0], FIELDS[2], FIELDS[3]):
        flat = [v for s in field['values'] for v in s]
        require(min(flat) < 0 < max(flat), f"{field['flexpart_variable']} must exercise both signs")
    for field in FIELDS:
        for a, b in zip(field['values'], field['values'][1:]):
            require(all(x != y for x, y in zip(a, b)), 'every node must change between memory slots')


def decode_output(path):
    lines = path.read_text(encoding='utf-8').splitlines()
    require(lines[:2] == ['FLEXPART_SURFACE_FLUX_TEMPORAL_ORACLE_V1', f'GRID {GRID_NX} {GRID_NY}'],
            'wrong oracle header')
    pairs, rows, cursor = [], [], 2
    while cursor < len(lines):
        tokens = lines[cursor].split()
        if tokens[0] == 'PAIR':
            require(len(tokens) == 4, 'wrong PAIR record')
            pairs.append([int(t) for t in tokens[1:]])
            cursor += 1
            continue
        require(tokens[0] == 'QUERY' and len(tokens) == 6, 'unknown oracle record')
        dts = lines[cursor + 1].split()
        require(dts[0] == 'DTS' and len(dts) == 4, 'missing DTS record')
        fields = {}
        for offset in range(len(FIELDS)):
            record = lines[cursor + 2 + offset].split()
            require(record[0] == 'FIELD' and len(record) == 5 and int(record[1]) == offset + 1,
                    'missing FIELD record')
            fields[offset + 1] = [float(v) for v in record[2:]]
        rows.append({'pair': int(tokens[1]), 'query': int(tokens[2]), 'itime_s': int(tokens[3]),
                     'xt': float(tokens[4]), 'yt': float(tokens[5]),
                     'dts': [float(v) for v in dts[1:]], 'fields': fields})
        cursor += 2 + len(FIELDS)
    values = [v for r in rows for v in [*r['dts'], *[x for f in r['fields'].values() for x in f]]]
    require(all(math.isfinite(v) for v in values), 'non-finite oracle value')
    return pairs, rows


def bilinear(values, x, y):
    ix, iy = int(x), int(y)
    dx, dy = x - ix, y - iy
    at = lambda i, j: values[min(i, GRID_NX - 1) + min(j, GRID_NY - 1) * GRID_NX]
    return ((1 - dx) * (1 - dy) * at(ix, iy) + dx * (1 - dy) * at(ix + 1, iy)
            + (1 - dx) * dy * at(ix, iy + 1) + dx * dy * at(ix + 1, iy + 1))


def audit(pairs, rows):
    """Bind each raw Fortran row to the declared inputs and require coverage."""
    expected_pairs = [[i + 1, *(SNAPSHOT_TIMES_S[s] for s in p['snapshots'])] for i, p in enumerate(PAIRS)]
    require(pairs == expected_pairs, 'oracle memory pairs differ from declaration')
    expected_rows = [(i + 1, q) for i, p in enumerate(PAIRS) for q in queries(p)]
    require(len(rows) == len(expected_rows), 'incomplete oracle queries')
    applications = set()
    records = []
    for row, (pair_number, (itime, x, y)) in zip(rows, expected_rows):
        pair = PAIRS[pair_number - 1]
        lower, upper = (SNAPSHOT_TIMES_S[s] for s in pair['snapshots'])
        require(row['pair'] == pair_number and row['itime_s'] == itime
                and row['xt'] == f32(x) and row['yt'] == f32(y), 'raw query/output mismatch')
        require(row['dts'][:2] == [itime - lower, upper - itime], 'find_time_vars mismatch')
        require(row['dts'][2] == f32(1.0 / f32(upper - lower)), 'find_time_vars dtt mismatch')
        if itime == SNAPSHOT_TIMES_S[0]:
            application = 'first_endpoint'
        elif itime == SNAPSHOT_TIMES_S[-1]:
            application = 'last_endpoint'
        elif itime in SNAPSHOT_TIMES_S:
            application = 'exact_source_timestamp'
        else:
            application = 'linear_interior'
        applications.add(application)
        node = x == int(x) and y == int(y)
        for field in FIELDS:
            slot_1, slot_2, value = row['fields'][field['index']]
            sources = [field['values'][s] for s in pair['snapshots']]
            slots = [bilinear(source, f32(x), f32(y)) for source in sources]
            if node:
                require([slot_1, slot_2] == [f32(s) for s in slots], 'memory slot does not store raw input')
            else:
                require(close(slot_1, slots[0]) and close(slot_2, slots[1]), 'hor_interpol diagnostic mismatch')
            weight = (itime - lower) / (upper - lower)
            require(close(value, slot_1 * (1 - weight) + slot_2 * weight), 'temporal diagnostic mismatch')
            if application == 'linear_interior':
                require(not close(value, slot_1) and not close(value, slot_2), 'interior not distinguished')
            factor = field['flexpart_to_canonical_factor']
            records.append({
                'canonical_field': field['canonical_field'], 'flexpart_variable': field['flexpart_variable'],
                'pair': pair_number, 'query': row['query'], 'itime_s': itime,
                'xt': row['xt'], 'yt': row['yt'], 'node': node, 'application': application,
                'dts': row['dts'], 'flexpart_memory_slot_values': [slot_1, slot_2],
                'flexpart_value': value, 'canonical_value': factor * value})
    require(applications == {'first_endpoint', 'linear_interior', 'exact_source_timestamp', 'last_endpoint'},
            'missing temporal application coverage')
    return records


def verify_call_edges(nm_text, call_sites_text, edges):
    """Prove MAIN__ calls each pinned routine in the linked large-model binary."""
    symbols = {t[2]: int(t[0], 16) for t in (line.split() for line in nm_text.splitlines()) if len(t) == 3}
    marker = '<MAIN__>:'
    start = call_sites_text.find(marker)
    require(start >= 0, 'call-site evidence lacks MAIN__')
    body = call_sites_text[start:]
    base_immediate = re.search(r'movabs \$0x([0-9a-f]+),%r11', body)
    base_anchor = re.search(r'lea\s+[^#]+# ([0-9a-f]+) <', body)
    require(base_immediate is not None and base_anchor is not None, 'cannot resolve call base')
    call_base = int(base_immediate.group(1), 16) + int(base_anchor.group(1), 16)
    evidence = []
    for callee in edges:
        require(callee in symbols, f'nm evidence lacks {callee}')
        displacement = (symbols[callee] - call_base) % (1 << 64)
        pattern = rf'movabs \$0x{displacement:x},%rax(?:(?!movabs).){{0,500}}?call\s+\*%rax'
        require(re.search(pattern, body, re.DOTALL) is not None, f'linked executable lacks MAIN__ -> {callee}')
        evidence.append({'caller': 'MAIN__', 'callee': callee, 'callee_address': f'0x{symbols[callee]:x}',
                         'verified_in_linked_executable': True})
    return evidence


CALL_EDGES = ['__interpol_mod_MOD_find_grid_indices', '__interpol_mod_MOD_find_grid_distances',
              '__interpol_mod_MOD_find_time_vars', '__interpol_mod_MOD_hor_interpol_4d',
              '__interpol_mod_MOD_temporal_interpolation']
RESEARCH_SOURCES = ['scripts/interpolation/surface_flux_temporal_oracle.f90',
                    'scripts/interpolation/surface_flux_temporal_oracle.sh',
                    'scripts/interpolation/run_surface_flux_temporal_oracle.py',
                    'scripts/interpolation/prepare_surface_flux_temporal_oracle.py',
                    'scripts/oracle_build_cache.py', 'reference/flexpart-11.1.json',
                    'reference/flex-extract.json']
FLEXPART_SOURCES = ['src/windfields_mod.f90', 'src/getfields_mod.f90', 'src/interpol_mod.f90',
                    'src/drydepo_mod.f90', 'src/com_mod.f90', 'src/par_mod.f90', 'src/makefile_gfortran']


def report(checkout, flex_extract, run):
    root = Path(__file__).resolve().parents[2]
    manifest = json.loads((root / 'reference/flexpart-11.1.json').read_text())
    extract_manifest = json.loads((root / 'reference/flex-extract.json').read_text())
    require(git(checkout, 'rev-parse', 'HEAD') == manifest['pinned_commit'], 'wrong pinned revision')
    require(not git(checkout, 'status', '--porcelain'), 'dirty authoritative checkout')
    require(git(flex_extract, 'rev-parse', 'HEAD') == extract_manifest['pinned_commit'],
            'wrong pinned flex_extract revision')
    require(not git(flex_extract, 'status', '--porcelain'), 'dirty flex_extract checkout')
    validate_declaration()
    require((run / 'input.txt').read_text(encoding='utf-8') == input_text(), 'input differs from declaration')
    pairs, rows = decode_output(run / 'output.txt')
    records = audit(pairs, rows)
    edges = verify_call_edges((run / 'build/surface-flux-temporal-oracle.nm').read_text(),
                              (run / 'build/surface-flux-temporal-oracle.call-sites').read_text(), CALL_EDGES)
    image_id = os.environ.get('RESEARCH_IMAGE_ID', '')
    require(re.fullmatch(r'sha256:[0-9a-f]{64}', image_id) is not None, 'missing resolved Docker image identity')
    for source in FLEXPART_SOURCES:
        require(canonical_text_sha256(checkout / source) ==
                canonical_text_sha256(run / 'build/upstream' / Path(source).name),
                f'linked routine source changed: {source}')
    linked = (run / 'build/linked-objects.txt').read_text().splitlines()
    require(linked == sorted(set(linked)) and len(linked) > 3 and 'FLEXPART.o' not in linked,
            'invalid linked upstream object inventory')
    artifacts = ['input.txt', 'output.txt', 'full-build.log', 'driver-build-run.log',
                 'build/surface-flux-temporal-oracle', 'build/surface-flux-temporal-oracle-driver.o',
                 'build/surface-flux-temporal-oracle.nm', 'build/surface-flux-temporal-oracle.call-sites',
                 'build/surface-flux-temporal-oracle.link-map', 'build/linked-objects.txt',
                 'build/compiler-identity.txt']
    return {
        'schema': SCHEMA, 'issue': 120,
        'validation_level': 'direct_pinned_routines_on_field_memory_slots',
        'comparison_policy': {'owner': 'issue #74/#89 temporal default', 'absolute': ABSOLUTE_TOLERANCE,
                              'relative': RELATIVE_TOLERANCE,
                              'formula': 'abs(a-b) <= absolute OR abs(a-b) <= relative*max(abs(a),abs(b))'},
        'grid': {'nx': GRID_NX, 'ny': GRID_NY, 'storage_order': 'x_fastest'},
        'snapshot_times_s': SNAPSHOT_TIMES_S,
        'fields': [{k: v for k, v in f.items() if k != 'values'} | {
            'flexpart_values_per_snapshot': f['values'],
            'canonical_values_per_snapshot': [[f['flexpart_to_canonical_factor'] * v for v in s]
                                              for s in f['values']]} for f in FIELDS],
        'source_trace': SOURCE_TRACE,
        'records': records,
        'provenance': {
            'pinned_commit': manifest['pinned_commit'], 'checkout_clean': True,
            'flex_extract_pinned_commit': extract_manifest['pinned_commit'], 'flex_extract_checkout_clean': True,
            'flex_extract_sources_sha256': {p: canonical_text_sha256(flex_extract / p)
                                            for p in SOURCE_TRACE['flex_extract_files']},
            'compiler': (run / 'build/compiler-identity.txt').read_text().strip(),
            'docker_image_id': image_id, 'build_profile': manifest['execution_profile'],
            'driver_flags': ['-O0', '-fopenmp', '-mcmodel=large'],
            'sources_sha256': {s: canonical_text_sha256(checkout / s) for s in FLEXPART_SOURCES},
            'research_sources_sha256': {p: canonical_text_sha256(root / p) for p in RESEARCH_SOURCES},
            'linked_objects_sha256': {p: sha256(run / 'build/upstream' / p) for p in linked},
            'artifacts_sha256': {p: sha256(run / p) for p in artifacts},
            'verified_call_edges': edges}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--emit-input', type=Path)
    parser.add_argument('--checkout', type=Path)
    parser.add_argument('--flex-extract', type=Path)
    parser.add_argument('--run', type=Path)
    parser.add_argument('--expected', type=Path)
    args = parser.parse_args()
    if args.emit_input:
        validate_declaration()
        args.emit_input.write_text(input_text(), encoding='utf-8', newline='\n')
        return
    require(None not in (args.checkout, args.flex_extract, args.run, args.expected), 'missing audit arguments')
    result = report(args.checkout, args.flex_extract, args.run)
    (args.run / 'report.json').write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    # Executable hashes belong to each run; raw scientific output and stable
    # source identities must reproduce the frozen fixture exactly.
    frozen = args.expected / 'report.json'
    require(frozen.exists(), 'frozen evidence absent; inspect report and freeze it explicitly')
    expected = json.loads(frozen.read_text(encoding='utf-8'))
    for key in ('records', 'fields', 'grid', 'snapshot_times_s', 'source_trace', 'comparison_policy'):
        require(result[key] == expected[key], f'frozen decoded evidence changed: {key}')
    for key in ('pinned_commit', 'flex_extract_pinned_commit', 'flex_extract_sources_sha256',
                'sources_sha256', 'research_sources_sha256'):
        require(result['provenance'][key] == expected['provenance'][key], f'provenance changed: {key}')
    require(sha256(args.run / 'output.txt') == sha256(args.expected / 'output.txt'), 'raw output changed')
    require(sha256(args.run / 'input.txt') == sha256(args.expected / 'input.txt'), 'raw input changed')
    print(json.dumps({'state': 'PASS', 'records': len(result['records']),
                      'report': str(args.run / 'report.json')}))


if __name__ == '__main__':
    main()
