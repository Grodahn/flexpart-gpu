#!/usr/bin/env python3
"""Prepare and decode #119 inputs for the existing unchanged rain-mode oracle.

No precipitation arithmetic is implemented here. Independent point members are
the pinned #207 darain observations, rounded to the FLEXPART reader's real4.
The optional device report supplies exact post-execution member bytes for a
second, matched-input original-routine comparison (never a GPU stage input).
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[2]
ORACLE = ROOT / 'fixtures/accumulation/precipitation-darain-oracle-v1.json'
POSITIONS = [(x, y) for y in range(3) for x in range(3)] + [(0.25, 0.75), (1.25, 0.5), (0.5, 1.5)]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def f32(value):
    return struct.unpack('<f', struct.pack('<f', value))[0]


def cases(device=None):
    genuine = json.loads(ORACLE.read_text())['genuine']
    members = {p: [m['result_mm_per_h'] for m in genuine['darain_members'][p]] for p in ('lsp', 'cp')}
    if device is not None:
        members = device['member_values_mm_per_h']
    times = [m['member_time_s'] for m in genuine['darain_members']['lsp']]
    for slot, left in enumerate(times):
        right_slot = min(slot + 1, len(times) - 1)
        right = times[right_slot]
        queries = [[x, y, left + offset] for offset in ([0] if left == right else [0, 2700, 5400, 8100])
                   for x, y in POSITIONS]
        yield {'id': f'pair-{slot:02}', 'slots': [slot, right_slot], 'memtime_s': [left, right],
               'planes': {p: [[f32(v) for v in members[p][s]] for s in (slot, right_slot)] for p in members},
               'queries': queries, 'nx': 3, 'ny': 3}
    yield {'id': 'arithmetic-control', 'slots': [], 'memtime_s': [0, 3600], 'nx': 4, 'ny': 3,
           'planes': {'lsp': [list(range(12)), list(range(100, 112))], 'cp': [[0.5]*12, [1.5]*12]},
           'queries': [[1.25, 0.5, t] for t in (0, 900, 1800, 2700, 3600)]}


def driver_input(case):
    nx, ny = case['nx'], case['ny']
    lines = ['rain', f'{nx} {ny}', '10 50 1 1', '0', ' '.join(map(str, case['memtime_s']))]
    for p in ('lsp', 'cp'):
        for plane in case['planes'][p]:
            lines.extend(repr(float(v)) for v in plane)
    for constant in (0.5, 0.5, 280.0, 280.0, 0.001, 0.001):
        lines.extend([str(constant)] * nx * ny)
    lines.append(str(len(case['queries'])))
    lines.extend(f'{x} {y} {t} 1' for x, y, t in case['queries'])
    return '\n'.join(lines) + '\n'


def parse_output(text):
    if not text.startswith('FLEXPART_INTERPOLATION_ROUTINE_ORACLE_V1\n') or '\nNUMPF 1\n' not in text:
        raise ValueError('missing original numpf=1 execution')
    rows = []
    for line in text.splitlines():
        fields = line.split()
        if fields and fields[0] == 'QUERY':
            rows.append({})
        elif fields and fields[0] == 'XY_ITIME_KZ':
            rows[-1]['query'] = [float(fields[1]), float(fields[2]), int(fields[3])]
        elif fields and fields[0] == 'DTS':
            rows[-1]['dts'] = [float(v) for v in fields[1:]]
        elif fields and fields[0] in ('LSP', 'CP'):
            rows[-1][fields[0].lower()] = float(fields[1])
    if not rows or any(set(r) != {'query', 'dts', 'lsp', 'cp'} for r in rows):
        raise ValueError('incomplete original output')
    return rows


def prepare(directory, device=None):
    inputs = directory / 'rain-inputs'
    inputs.mkdir(parents=True, exist_ok=True)
    manifest = list(cases(device))
    for case in manifest:
        path = inputs / f"rain-{case['id']}.txt"
        path.write_text(driver_input(case), encoding='utf-8', newline='\n')
        case['input_sha256'] = digest(path)
    (directory / 'inputs.json').write_text(json.dumps(manifest, indent=2) + '\n')


def decode(directory, output, oracle_directory=None):
    manifest = json.loads((directory / 'inputs.json').read_text())
    run = oracle_directory or directory / 'oracle'
    for case in manifest:
        path = run / 'runs' / f"rain-{case['id']}-output.txt"
        case['raw_input'] = (directory / 'rain-inputs' / f"rain-{case['id']}.txt").read_text()
        case['raw_output'] = path.read_text()
        case['output_sha256'] = digest(path)
        case['rows'] = parse_output(case['raw_output'])
        if [r['query'] for r in case['rows']] != case['queries']:
            raise ValueError('changed original query inputs')
    for name in ('checkout-before.txt', 'checkout-after.txt'):
        if (run / name).read_text().strip() != json.loads((ROOT / 'reference/flexpart-11.1.json').read_text())['pinned_commit']:
            raise ValueError('not a pristine pinned original')
    report = {'schema': 'flexpart-gpu.precipitation-temporal-oracle.v1', 'units': 'millimeter_per_hour',
              'darain_oracle_sha256': digest(ORACLE),
              'flexpart_revision': json.loads((ROOT / 'reference/flexpart-11.1.json').read_text())['pinned_commit'],
              'flex_extract_revision': json.loads((ROOT / 'reference/flex-extract.json').read_text())['pinned_commit'],
              'source_sha256': {p: digest(run / 'build/upstream' / p) for p in
                                ('interpol_mod.f90', 'getfields_mod.f90', 'windfields_mod.f90', 'wetdepo_mod.f90', 'par_mod.f90')},
              'harness_sha256': {p: digest(ROOT / 'scripts/interpolation' / p) for p in
                                 ('direct_oracle.sh', 'direct_interpolation_oracle.f90')},
              'executable_sha256': digest(run / 'build/interpolation-oracle'),
              'compiler': (run / 'compiler-version.txt').read_text().strip(),
              'cases': manifest}
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n', encoding='utf-8', newline='\n')
    return report


def run(directory, checkout, output, device_report=None, oracle_directory=None):
    """Reuse the original #202 scratch-build and unchanged direct oracle harness."""
    device = json.loads(device_report.read_text()) if device_report else None
    prepare(directory, device)
    oracle_directory = oracle_directory or directory / 'oracle'
    container_path = lambda p: '/workspace/flexpart-gpu/' + p.resolve().relative_to(ROOT).as_posix()
    image = json.loads((ROOT / 'reference/flexpart-11.1.json').read_text())['execution_profile']['docker']['image']
    image_id = subprocess.check_output(['docker', 'image', 'inspect', image, '--format', '{{.Id}}'], text=True).strip()
    command = ['docker', 'run', '--rm', '-v', f'{ROOT.as_posix()}:/workspace/flexpart-gpu',
               '-v', f'{checkout.resolve().as_posix()}:/workspace/flexpart:ro',
               '-w', '/workspace/flexpart-gpu', '--entrypoint', 'bash', image_id,
               'scripts/precipitation/member_mapping_oracle.sh', container_path(directory), container_path(oracle_directory)]
    log = directory / 'container.log'
    with log.open('w') as stream:
        subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT, check=True)
    result = decode(directory, output, oracle_directory)
    result['image_id'] = image_id
    if device:
        result['device_report_sha256'] = digest(device_report)
        result['candidate'] = device['candidate']
    output.write_text(json.dumps(result, indent=2, allow_nan=False) + '\n', encoding='utf-8', newline='\n')
    print(json.dumps({'state': 'PASS', 'original_output': str(output), 'log': str(log)}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('prepare', 'decode', 'run'))
    parser.add_argument('directory', type=Path)
    parser.add_argument('--device-report', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--checkout', type=Path)
    parser.add_argument('--oracle-directory', type=Path)
    args = parser.parse_args()
    if args.mode == 'prepare':
        prepare(args.directory, json.loads(args.device_report.read_text()) if args.device_report else None)
    elif args.mode == 'decode':
        decode(args.directory, args.output, args.oracle_directory)
    else:
        run(args.directory, args.checkout, args.output, args.device_report, args.oracle_directory)
