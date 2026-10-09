#!/usr/bin/env python3
"""Run #118's focused direct oracle in the repository's existing Fortran image."""

import argparse
import json
import re
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkout', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output_dir.resolve()
    relative = output.relative_to(root / 'target')
    if output.exists() or relative == Path('.'):
        parser.error('output must be a new directory strictly beneath this checkout target/')
    if not (args.checkout.resolve() / '.git').is_dir():
        parser.error('oracle checkout must be an independent pristine Git checkout')
    manifest = json.loads((root / 'reference/flexpart-11.1.json').read_text())
    image = manifest['execution_profile']['docker']['image']
    identity = subprocess.run(['docker', 'image', 'inspect', image, '--format', '{{.Id}}'],
                              check=True, capture_output=True, text=True).stdout.strip()
    if re.fullmatch(r'sha256:[0-9a-f]{64}', identity) is None:
        parser.error('Docker inspect did not return an immutable image identity')
    output.parent.mkdir(parents=True, exist_ok=True)
    log = output.with_name(output.name + '-container.log')
    command = ['docker', 'run', '--rm', '-e', f'RESEARCH_IMAGE_ID={identity}',
               '-v', f'{root.as_posix()}:/workspace/flexpart-gpu',
               '-v', f'{args.checkout.resolve().as_posix()}:/workspace/flexpart:ro',
               '-w', '/workspace/flexpart', identity, 'bash',
               '/workspace/flexpart-gpu/scripts/interpolation/shared_height_oracle.sh',
               '/workspace/flexpart', '/workspace/flexpart-gpu/target/' + relative.as_posix()]
    with log.open('w', encoding='utf-8') as stream:
        completed = subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT)
    if completed.returncode:
        print(json.dumps({'state': 'FAIL', 'exit_code': completed.returncode, 'log': str(log)}))
        raise SystemExit(completed.returncode)
    print(json.dumps({'state': 'PASS', 'report': str(output / 'report.json'), 'log': str(log)}))


if __name__ == '__main__':
    main()
