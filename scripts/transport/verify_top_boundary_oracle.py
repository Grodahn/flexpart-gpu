#!/usr/bin/env python3
"""Audit genuine #180 execution, exact regression values and complete build provenance."""

import argparse
import hashlib
import json
import math
import os
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / 'fixtures/transport/top-boundary-v1'
TOOLS = [
    'scripts/transport/top_boundary_oracle.f90',
    'scripts/transport/top_boundary_oracle.sh',
    'scripts/transport/run_top_boundary_oracle.py',
    'scripts/transport/verify_top_boundary_oracle.py',
    'scripts/interpolation/w_production_oracle.sh',
    'scripts/oracle_build_cache.py', 'reference/flexpart-11.1.json',
]
ARTIFACTS = ['input.txt', 'output.txt', 'full-build.log', 'driver-build-run.log',
             'checkout-before.txt', 'checkout-after.txt'] + ['build/' + name for name in [
    'w-production-oracle', 'w-production-oracle-driver.o', 'w-production-oracle.nm',
    'w-production-oracle.call-sites', 'w-production-oracle.link-map', 'linked-objects.txt',
    'compiler-identity.txt', 'advance_mod.original.o', 'advance-original.text', 'advance-exported.text',
    'advance-original.disassembly', 'advance-exported.disassembly', 'executable.disassembly',
    'advance-original.elf', 'advance-exported.elf']]


def require(condition, message):
    """Incomplete evidence cannot become a successful research verdict."""
    if not condition:
        raise ValueError(message)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def text_digest(path):
    return hashlib.sha256(path.read_text(encoding='utf-8').encode()).hexdigest()


def git(checkout, *arguments):
    return subprocess.run(['git', '-C', str(checkout), *arguments], check=True,
                          capture_output=True, text=True).stdout.strip()


def decode(path):
    """Retain every raw observation and reject missing/duplicate/nonfinite rows."""
    lines = path.read_text().splitlines()
    require(lines[0] == 'FLEXPART_TOP_BOUNDARY_ORACLE_V1', 'incorrect raw header')
    rows = {'GRID': [], 'PUSH': [], 'ADVANCE': []}
    for line in lines[1:]:
        fields = line.split()
        tag = fields.pop(0)
        require(tag in rows, 'unknown raw row')
        integers, reals, boolean = {'GRID': (3, 4, False), 'PUSH': (4, 1, True),
                                   'ADVANCE': (5, 8, True)}[tag]
        require(len(fields) == integers + reals + int(boolean), 'incorrect raw row width')
        row = [*map(int, fields[:integers]), *map(float, fields[integers:integers+reals])]
        require(all(math.isfinite(v) for v in row), 'nonfinite observation')
        if boolean:
            require(fields[-1] in ('T', 'F'), 'incorrect stopped flag')
            row.append(fields[-1] == 'T')
        rows[tag].append(row)
    require(len(rows['GRID']) == 8 and len(rows['PUSH']) == 24 and len(rows['ADVANCE']) == 72,
            'incomplete experiment')
    grids = {tuple(r[:3]): r[3:] for r in rows['GRID']}
    pushes = {tuple(r[:4]): r[4:] for r in rows['PUSH']}
    advances = {tuple(r[1:5]): r[5:] for r in rows['ADVANCE']}
    require(set(grids) == {(g,m,x) for g in (0,1) for m in (1,2) for x in (0,1)}, 'grid coverage')
    require(set(pushes) == {(g,d,s,z) for g in (0,1) for d in (-1,1) for s in (0,1) for z in (-1,0,1)},
            'boundary coverage')
    require(set(advances) == {(g,d,e,s) for g in (0,1) for d in (-1,1) for e in (0,1) for s in range(1,10)},
            'transport coverage')
    require([r[0] for r in rows['ADVANCE']] == list(range(1,73)), 'duplicate advance identifiers')
    top = grids[0,1,0][0]
    require(all(r[0] == top for r in grids.values()), 'run target changed')
    require(len({r[1] for key,r in grids.items() if key[0] == 0}) == 1, 'equal control differs')
    require(len({r[1] for key,r in grids.items() if key[0] == 1}) == 4, 'differing native geometry absent')
    require(grids[1,1,0][1] == top, 'initial column does not own the target')
    below = pushes[0,1,0,0][0]
    require(below < top, 'push must place strictly below shared top')
    for (g,d,s,z), (actual, stopped) in pushes.items():
        if z < 0:
            require(actual < top and not stopped, 'below-top input stopped or pushed upward')
        elif d == -1 and s == 1:
            require(stopped and actual >= top, 'backward settling must stop without top placement')
        else:
            require(actual == below and not stopped, 'inclusive boundary action changed')
    for (g,d,e,s), row in advances.items():
        start, old, end, slope, x, y, z, settling, stopped = row
        require(0.5 < x < 1 and y == 0, 'horizontal transport between column weights missing')
        require(settling < 0 if s == 9 else settling == 0, 'genuine settling route not exercised')
        if s != 9:
            require(not stopped and z < top, 'ordinary transport boundary failed')
        if s in (2,3,5,6,7):
            require(z == below, 'boundary crossing must return original placement')
        if s == 4:
            require(z < below and z != start, 'predictor placement/correction order undistinguished')
        if s == 9 and d == -1:
            require(stopped and z >= top, 'backward real settling failed to stop')
        require(row == advances[1-g,d,e,s], 'native-column/time top leaked into transport boundary')
    return {'target_top_m_agl': top, 'observed_placement_m_agl': below,
            'grid_columns': ['geometry', 'memory', 'x', 'shared_top_m', 'native_top_m', 'surface_pa', 'target_level_2_m'],
            'push_columns': ['geometry', 'direction', 'settling_enabled', 'input_top_offset_0.0001_m', 'returned_z_m', 'nstop'],
            'advance_columns': ['id', 'geometry', 'direction', 'epoch_1200_s', 'scenario', 'initial_z_m',
                                'old_w_unsigned_m_s', 'end_w_unsigned_m_s', 'x_slope_unsigned_m_s',
                                'returned_x_cells', 'returned_y_cells', 'returned_z_m', 'settling_m_s', 'nstop'],
            'rows': rows}


def verify_linkage(build):
    """Require actual call instructions and the original defining object's binding."""
    disassembly = (build / 'executable.disassembly').read_text()
    symbols = {fields[2]: int(fields[0],16) for line in (build / 'w-production-oracle.nm').read_text().splitlines()
               if len(fields := line.split()) == 3}
    edges = []
    for caller, callees in {
        'MAIN__': ['__advance_mod_MOD_advance', '__advance_mod_MOD_pushpartdown',
                   '__verttransform_mod_MOD_verttransform_init', '__verttransform_mod_MOD_verttransform_ecmwf_heights'],
        '__advance_mod_MOD_advance': ['__interpol_mod_MOD_interpol_wind', '__interpol_mod_MOD_interpol_wind_short',
                                     '__settling_mod_MOD_get_settling'],
    }.items():
        body = re.search(r'^[0-9a-f]+ <' + re.escape(caller) + r'>:\n(.*?)(?=\n[0-9a-f]+ <|\Z)',
                         disassembly, re.M | re.S)
        require(body is not None, f'missing executable routine {caller}')
        for callee in callees:
            direct = re.search(r'\bcall\s+[0-9a-f]+ <' + re.escape(callee) + r'>', body[1])
            # Same GNU large-model GOT-relative resolution used by #80's verifier.
            immediate = re.search(r'movabs \$0x([0-9a-f]+),%r11', body[1])
            anchor = re.search(r'lea\s+[^#]+# ([0-9a-f]+) <', body[1])
            indirect = None
            if immediate and anchor and callee in symbols:
                base = int(immediate[1],16) + int(anchor[1],16)
                displacement = (symbols[callee] - base) % (1 << 64)
                indirect = re.search(rf'movabs \$0x{displacement:x},%rax(?:(?!movabs).){{0,1800}}?call\s+\*%rax',
                                     body[1], re.S)
            require(direct is not None or indirect is not None, f'no genuine call {caller} -> {callee}')
            edges.append([caller, callee])
    link_map = (build / 'w-production-oracle.link-map').read_text()
    require(re.search(r'^__advance_mod_MOD_advance\s+[^\n]*/upstream/advance_mod.o\n(?:[ \t]+[^\n]+\n)*?[ \t]+w-production-oracle-driver.o$',
                      link_map, re.M) is not None, 'public advance not bound to pristine compiled object')
    return edges


def audit(checkout, run):
    """Reconstruct linkage and source identity instead of trusting stored pass flags."""
    manifest = json.loads((ROOT / 'reference/flexpart-11.1.json').read_text())
    pin = manifest['pinned_commit']
    require(git(checkout, 'rev-parse', 'HEAD') == pin and not git(checkout, 'status', '--porcelain'),
            'reference must be the clean pin')
    for name in ('checkout-before.txt', 'checkout-after.txt'):
        require((run / name).read_text().splitlines() == [pin], 'missing clean-checkout record')
    science = decode(run / 'output.txt')
    require(text_digest(run / 'input.txt') == text_digest(ROOT / 'fixtures/interpolation/shared-height-v1/input.txt'),
            'input is not the retained #118 profile fixture')
    build = run / 'build'
    exported = build / 'advance-reexported.o'
    try:
        subprocess.run(['objcopy', '--globalize-symbol=__advance_mod_MOD_pushpartdown',
                        str(build / 'advance_mod.original.o'), str(exported)], check=True,
                       capture_output=True)
        require(digest(exported) == digest(build / 'upstream/advance_mod.o'), 'object differs from visibility-only export')
    finally:
        exported.unlink(missing_ok=True)
    require((build / 'advance-original.text').read_bytes() == (build / 'advance-exported.text').read_bytes(),
            'reference code bytes modified')
    edges = verify_linkage(build)
    sources = {}
    for source in git(checkout, 'ls-files', 'src').splitlines():
        # Upstream make stamps the normal model main; this main is never linked.
        if source == 'src/FLEXPART.f90':
            continue
        relative = Path(source).relative_to('src')
        require(text_digest(checkout / source) == text_digest(build / 'upstream' / relative),
                f'archived source changed: {source}')
        sources[source] = text_digest(checkout / source)
    objects = (build / 'linked-objects.txt').read_text().splitlines()
    require(objects == sorted(set(objects)) and 'advance_mod.o' in objects and 'FLEXPART.o' not in objects,
            'incorrect object inventory')
    image = os.environ.get('RESEARCH_IMAGE_ID', '')
    require(re.fullmatch(r'sha256:[0-9a-f]{64}', image) is not None, 'missing immutable image ID')
    return {'schema': {'id': 'flexpart-gpu.top-boundary-research', 'version': 1}, 'issue': 180,
            'validation_level': 'direct_pinned_routines_synthetic_research_only',
            'comparison_policy': 'Exact raw output and decoded observations in fixed immutable image; no GPU tolerance established.',
            'science': science, 'provenance': {
                'pinned_commit': pin, 'image_id': image, 'profile': manifest['execution_profile'],
                'compiler': (build / 'compiler-identity.txt').read_text().strip(),
                'driver_flags': ['-O0', '-fopenmp', '-mcmodel=large'],
                'sources_sha256': sources, 'tools_sha256': {s: text_digest(ROOT / s) for s in TOOLS},
                'linked_objects_sha256': {s: digest(build / 'upstream' / s) for s in objects},
                'artifacts_sha256': {s: digest(run / s) for s in ARTIFACTS}, 'verified_call_edges': edges,
                'symbol_export': 'pushpartdown visibility only; repeating objcopy on original reproduces the entire linked object',
            }}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkout', type=Path, required=True)
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--record', action='store_true')
    args = parser.parse_args()
    result = audit(args.checkout, args.run)
    if not args.record:
        expected = json.loads((FIXTURE / 'report.json').read_text())
        require(result['science'] == expected['science'], 'frozen scientific observations changed')
        require(digest(args.run / 'output.txt') == digest(FIXTURE / 'output.txt'), 'frozen raw output changed')
        for key in ('pinned_commit', 'sources_sha256', 'tools_sha256', 'profile', 'driver_flags', 'verified_call_edges'):
            require(result['provenance'][key] == expected['provenance'][key], f'provenance changed: {key}')
    result['state'] = 'RECORDED' if args.record else 'PASS'
    (args.run / 'report.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'state': result['state'], 'push_cases': 24, 'advance_cases': 72,
                      'report': str(args.run / 'report.json')}))


if __name__ == '__main__':
    main()
