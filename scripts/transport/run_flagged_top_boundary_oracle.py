#!/usr/bin/env python3
"""Execute #213 flagged gas rows against the existing immutable-image/pristine-source oracle infrastructure."""

import argparse
import json
import re
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkout', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--record', action='store_true', help='Record for inspection; no frozen-baseline verdict')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output_dir.resolve()
    relative = output.relative_to(root / 'target')
    if output.exists() or relative == Path('.'):
        parser.error('output must be new and strictly beneath this checkout target/')
    manifest = json.loads((root / 'reference/flexpart-11.1.json').read_text())
    image = manifest['execution_profile']['docker']['image']
    identity = subprocess.run(['docker', 'image', 'inspect', image, '--format', '{{.Id}}'],
                              check=True, capture_output=True, text=True).stdout.strip()
    if re.fullmatch(r'sha256:[0-9a-f]{64}', identity) is None:
        parser.error('missing immutable Docker image ID')
    output.parent.mkdir(parents=True, exist_ok=True)
    log = output.with_name(output.name + '-container.log')
    command = ['docker', 'run', '--rm', '-e', f'RESEARCH_IMAGE_ID={identity}',
               '-v', f'{root.as_posix()}:/workspace/flexpart-gpu',
               '-v', f'{args.checkout.resolve().as_posix()}:/workspace/flexpart:ro',
               '-w', '/workspace/flexpart', identity, 'bash',
               '/workspace/flexpart-gpu/scripts/transport/flagged_top_boundary_oracle.sh',
               '/workspace/flexpart', '/workspace/flexpart-gpu/target/' + relative.as_posix()]
    if args.record:
        command.append('--record')
    with log.open('w', encoding='utf-8') as stream:
        result = subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT)
    print(json.dumps({'state': ('RECORDED' if args.record else 'PASS') if result.returncode == 0 else 'FAIL',
                      'exit_code': result.returncode, 'report': str(output / 'report.json'), 'log': str(log)}))
    raise SystemExit(result.returncode)


if __name__ == '__main__':
    main()
