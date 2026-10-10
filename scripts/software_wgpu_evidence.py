#!/usr/bin/env python3
"""Bind #177 domain execution to current-run artifacts and recheck scientific evidence.

This orchestrates the frozen existing CI assertions; it defines no science or cache.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
CONTRACT = ROOT / 'docs/software-wgpu-before.json'
SCHEMA = 'flexpart-gpu.software-wgpu-domain.v1'


def require(condition, message):
    """Fail closed on incomplete execution or provenance."""
    if not condition:
        raise ValueError(message)


def contract():
    """Load the finite original commands, evidence and ownership mapping."""
    return json.loads(CONTRACT.read_text(encoding='utf-8'))


def digest(path):
    """Bind a retained file to its bytes."""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def stage_id(index, step):
    """Retain the original cache step ids used by existing expressions."""
    match = re.search(r'^        id: (.+)$', step['block'], re.M)
    return match[1] if match else f'original-{index}'


def original_block(block):
    """Remove only the newly attached Actions execution-record id."""
    block = re.sub(r'^        id: original-\d+\n', '', block, flags=re.M)
    # The pinned action defaults to github.job. Materialize the old value after
    # renaming jobs so its actual cache identity remains unchanged.
    block = block.replace('          # Preserve the original automatic job-based #176 cache namespace.\n', '')
    return re.sub(r'^          shared-key: .+\n', '', block, flags=re.M)


def original_cache_namespace():
    """Materialize the pinned action's entire former key-plus-job prefix."""
    block = next(s['block'] for s in contract()['steps'] if s['name'] == 'Restore trusted Cargo compilation cache')
    return re.search(r'^          key: (.+)$', block, re.M)[1]+'-software-wgpu'


def split_jobs(text):
    """Read fixed job blocks without requiring a third-party YAML runtime."""
    parts = re.split(r'^  ([\w-]+):\n', text.split('jobs:\n', 1)[1], flags=re.M)
    return dict(zip(parts[1::2], parts[2::2]))


def monolithic_view(text, strict=False):
    """Expose the split workflow to #175's unchanged coverage and log auditor.

    No commands are synthesized: the view uses actual current step blocks and
    upload inputs. The frozen #175 inventory and scientific hashes stay intact.
    """
    import check_ci_test_coverage as coverage

    baseline = contract()
    jobs = split_jobs(text)
    domains = baseline['artifacts']
    require(set(jobs) == {*domains, 'software-wgpu'}, 'trigger/job/env drift: required domains')
    require(text.split('jobs:')[0] == baseline['header'].split('jobs:')[0], 'trigger/job/env drift')
    selected = {}
    for domain in domains:
        header = jobs[domain].split('    steps:')[0]
        require(coverage.environment(header) == baseline['environment'], 'trigger/job/env adapter drift')
        require('    runs-on: ubuntu-22.04\n' in header, 'adapter runner drift')
        require('    needs:' not in header and '    if:' not in header, 'independent domain conditional/dependency')
        selected[domain] = coverage.steps(jobs[domain])
        require('          shared-key: '+original_cache_namespace()+'\n' in selected[domain].get('Restore trusted Cargo compilation cache', ''),
                'original Cargo cache namespace changed')
    aggregate = jobs['software-wgpu']
    require('    name: software-wgpu\n' in aggregate and '    if: always()\n' in aggregate
            and '    needs: [gpu-meteorology, gpu-transport-physics]\n' in aggregate,
            'required aggregate identity/always/needs drift')
    require('run: python3 scripts/software_wgpu_evidence.py aggregate --root target/software-wgpu-download' in aggregate,
            'required aggregate validator absent')
    blocks = []
    for step in baseline['steps']:
        if step['name'] == 'Upload smoke logs':
            continue
        candidates = []
        for domain in step['owners']:
            require(step['name'] in selected[domain], 'protected evidence step missing: '+step['name'])
            candidates.append(original_block(selected[domain][step['name']]))
        require(len(set(candidates)) == 1, 'shared setup/device evidence drift: '+step['name'])
        if strict:
            require(hashlib.sha256(coverage.normalized(candidates[0]).encode()).hexdigest() == step['sha256'],
                    'original command/evidence changed: '+step['name'])
        blocks.append('      - '+candidates[0])
    # Keep #175's upload assertion meaningful by inspecting the new actual inputs.
    paths = set()
    for domain in domains:
        uploads = [b for b in selected[domain].values() if 'uses: actions/upload-artifact@v4' in b]
        require(len(uploads) == 1 and '        if: always()\n' in uploads[0], 'artifact upload must run on failure')
        require(coverage.uploaded_paths(uploads[0]) == {f'target/software-wgpu/{domain}/'}, 'missing retained artifact bundle')
        seal = selected[domain].get('Seal current-run domain evidence', '')
        require('run: python3 scripts/software_wgpu_evidence.py seal' in seal
                and 'DOMAIN_STEPS: ${{ toJSON(steps) }}' in seal and f'DOMAIN: {domain}' in seal,
                'required execution sealing absent')
        paths.update(baseline['artifacts'][domain])
    upload = '      - name: Upload smoke logs\n        if: always()\n        uses: actions/upload-artifact@v4\n        with:\n          path: |\n'
    upload += ''.join('            '+p+'\n' for p in sorted(paths))
    return baseline['header']+'    steps:\n'+''.join(blocks)+upload


def identity(env):
    """Require the candidate head, checked-out source, run and attempt identities."""
    keys = ('GITHUB_SHA', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'FLEXPART_GPU_CANDIDATE_REVISION')
    require(all(env.get(k) for k in keys), 'missing current-run identity')
    require(all(re.fullmatch('[0-9a-f]{40}', env[k]) for k in (keys[0], keys[3])), 'malformed revision')
    return {k: env[k] for k in keys}


def expected_stages(domain):
    """Enumerate all original required steps, including setup and cache integrity."""
    return [stage_id(i, s) for i, s in enumerate(contract()['steps'])
            if domain in s['owners'] and s['name'] != 'Upload smoke logs']


def files_for(root, domain):
    """Require every declared path/glob, with no cross-run or symlink handoff."""
    files = {}
    for pattern in contract()['artifacts'][domain]:
        matches = sorted(root.glob(pattern))
        require(matches, 'missing retained artifact: '+pattern)
        for path in matches:
            require(path.is_file() and not path.is_symlink() and path.stat().st_size > 0, 'empty/unsafe artifact: '+str(path))
            require(path.resolve().is_relative_to(root.resolve()), 'artifact escapes bundle')
            files[path.relative_to(root).as_posix()] = digest(path)
    return files


def execution_summary(root, files):
    """Require positive successful execution in every retained test transcript."""
    import check_ci_test_coverage as coverage
    logs = {p: coverage.summaries((root / p).read_text(encoding='utf-8'))
            for p in files if p.endswith('.log') and not p.endswith('gpu-preflight.log')}
    require(logs and all(n > 0 for n in logs.values()), 'zero required tests')
    preflight = json.loads((root / 'target/ci-gate/gpu-preflight.json').read_text())
    adapter = preflight['report']['adapter']
    require(adapter['adapter_class'] == 'software_wgsl' and adapter['backend'].lower() == 'vulkan'
            and adapter['name'], 'missing/incompatible adapter evidence')
    require(preflight['status'] == 'passed' and preflight['report']['smoke_test']['status'] == 'passed'
            and preflight['report']['smoke_test']['actual_value'] == preflight['report']['smoke_test']['expected_value'],
            'runtime smoke failed/skipped')
    return {'tests': sum(logs.values()), 'logs': logs, 'adapter': adapter}


def audit_payload(root, domain):
    """Recompute counts and execute the unchanged scientific Python assertions."""
    import check_ci_test_coverage as coverage

    files = files_for(root, domain)
    execution = execution_summary(root, files)
    logs = execution['logs']
    # All original inline report assertions are reused, including shader/input,
    # oracle hashes, exact row sets, tolerances and checked-out source identity.
    for step in contract()['steps']:
        if domain not in step['owners']:
            continue
        for check in re.findall(r"          python3 - <<'PY'\n(.*?)          PY\n", step['block'], re.S):
            code = '\n'.join(line[10:] for line in check.splitlines())
            env = dict(os.environ)
            if 'FLEXPART_GPU_CANDIDATE_REVISION: ${{ github.sha }}' in step['block']:
                env['FLEXPART_GPU_CANDIDATE_REVISION'] = env['GITHUB_SHA']
            result = subprocess.run([sys.executable, '-c', code], cwd=root, env=env, capture_output=True, text=True)
            require(result.returncode == 0, step['name']+': '+result.stderr[-2000:])
    if domain == 'gpu-meteorology':
        paths = [p for p in files if p.startswith('target/ci-gate/accumulation-gpu/') and p.endswith('.json')]
        for prefix in ('large_scale_precipitation', 'convective_precipitation'):
            require(sum(Path(p).name.startswith(prefix+'-cell-') for p in paths) == 12, 'accumulation cell count')
        for path in paths:
            report = json.loads((root / path).read_text())
            require(report['schema_id'] == 'flexpart-gpu.accumulation-gpu-evidence'
                    and report['schema_version'] == 1 and report['verdict'] == 'passed', 'accumulation report failed')
            for key in ('amount_evidence', 'rate_evidence'):
                evidence = report[key]
                require(evidence['execution']['status'] == 'passed'
                        and evidence['execution']['calculation_path'] == 'wgsl_device'
                        and evidence['execution']['adapter']['adapter_class'] == 'software_wgsl'
                        and evidence['comparison']['verdict'] == 'passed', 'accumulation comparison failed')
    else:
        for path, count in logs.items():
            text = (root / path).read_text(encoding='utf-8')
            if path.endswith('timeloop-production.log'):
                coverage.check_driver_log(text, '0', '0', True)
            if path.endswith('timeloop-validation.log'):
                coverage.check_driver_log(text, '0', '1')
            match = re.search(r'hanna-forward-1-([01]).log$', path)
            if match:
                coverage.check_driver_log(text, '1', match[1])
        markers = {'sw-wgpu-advection.log': ['SW-WGPU-ADVECTION-001', 'software_adapter=true'],
                   'pbl-options.log': ['PBL-OPTIONS-147:', 'software_adapter=true public_paths=3 device_results=identical'],
                   'dry-prefix.log': ['DRY-PREFIX-135', 'DRY-BOUNDS-135'],
                   'wet-prefix.log': ['WET-BOUNDS-138', *[f'capacity=8 active={n}' for n in (1, 3, 8)]],
                   'hanna-prefix.log': ['HANNA-BOUNDS-139', *[f'capacity=8 active={n} substeps={s}' for n in (1, 3, 8) for s in (0, 4)]]}
        for filename, required in markers.items():
            text = (root / 'target/ci-gate' / filename).read_text()
            require(all(m in text for m in required), 'required completion marker absent: '+filename)
        require(logs['target/ci-gate/sw-wgpu-advection.log'] == 1, 'advection executed count')
        # Validate each retained configuration, not only the last overwritten report.
        for mode in ('production', 'validation', 'compaction'):
            directory = root / 'target/ci-gate/resident-advection-production'
            for name in ('report', 'interior-deferred-boundary'):
                shutil.copyfile(directory / f'{mode}-{name}.json', directory / f'{name}.json')
            result = subprocess.run([sys.executable, str(ROOT / 'scripts/check_resident_advection_evidence.py')], cwd=root,
                                    capture_output=True, text=True)
            require(result.returncode == 0, 'resident advection '+mode+': '+result.stderr[-2000:])
    return execution


def validate_bundle(root, domain, expected_identity):
    """Reject false status strings by checking bytes, outcomes and scientific reports."""
    record = json.loads((root / 'manifest.json').read_text())
    require(record['schema'] == SCHEMA and record['domain'] == domain, 'malformed domain manifest')
    require(record['identity'] == expected_identity, 'different candidate/source/run/attempt')
    require(record['contract_sha256'] == digest(CONTRACT), 'different required contract')
    require(record['status'] == 'passed', 'incomplete domain execution')
    stages = record['stages']
    require(set(stages) == set(expected_stages(domain)), 'required stage absent')
    require(all(s == {'outcome': 'success', 'conclusion': 'success'} for s in stages.values()), 'upstream command failure/cancel/skip')
    require(record['files'] == files_for(root, domain), 'missing/corrupt artifact bytes')
    actual = audit_payload(root, domain)
    require(record['execution'] == actual, 'adapter/test execution summary mismatch')
    return actual


def seal(domain, env):
    """Retain available logs even on failure; seal only complete fresh execution."""
    require(domain in contract()['artifacts'], 'unknown required domain')
    destination = ROOT / 'target/software-wgpu' / domain
    destination.mkdir(parents=True, exist_ok=False)
    for pattern in contract()['artifacts'][domain]:
        for source in ROOT.glob(pattern):
            if source.is_file() and not source.is_symlink():
                target = destination / source.relative_to(ROOT)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
    record = {'schema': SCHEMA, 'domain': domain, 'status': 'failed'}
    try:
        monolithic_view((ROOT / '.github/workflows/software-wgpu.yml').read_text(), strict=True)
        record['identity'] = identity(env)
        require(subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip() == env['GITHUB_SHA'], 'source checkout mismatch')
        steps = json.loads(env['DOMAIN_STEPS'])
        record['stages'] = {key: {field: steps[key][field] for field in ('outcome', 'conclusion')}
                            for key in expected_stages(domain)}
        require(all(s == {'outcome': 'success', 'conclusion': 'success'} for s in record['stages'].values()), 'required upstream step did not succeed')
        record['contract_sha256'] = digest(CONTRACT)
        record['files'] = files_for(ROOT, domain)
        record['execution'] = audit_payload(ROOT, domain)
        record['status'] = 'passed'
    finally:
        (destination / 'manifest.json').write_text(json.dumps(record, indent=2)+'\n')
    print(json.dumps({'domain': domain, 'tests': record['execution']['tests'], 'files': len(record['files']), 'state': 'PASS'}))


def aggregate(root, results, env):
    """Require every domain result and validate current-run downloaded evidence."""
    domains = contract()['artifacts']
    require(set(results) == set(domains), 'missing/unexpected required domain result')
    require(all(results[d]['result'] == 'success' for d in domains), 'required domain failed/cancelled/skipped')
    expected = identity(env)
    require(subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip() == expected['GITHUB_SHA'],
            'aggregate source checkout mismatch')
    execution = {}
    for domain in domains:
        name = f'software-wgpu-{domain.removeprefix("gpu-")}-{env["GITHUB_RUN_ID"]}-{env["GITHUB_RUN_ATTEMPT"]}'
        bundle = root / name
        require(bundle.is_dir(), 'required evidence artifact absent: '+name)
        # Assertions use the actual checked-out current source and pinned fixtures.
        prepare_sources(bundle)
        execution[domain] = validate_bundle(bundle, domain, expected)
    return {'state': 'PASS', 'identity': expected, 'domains': execution}


def prepare_sources(bundle):
    """Expose current source and frozen fixtures to the original validators."""
    for directory in ('src', 'tests', 'fixtures'):
        destination = bundle / directory
        require(not destination.exists(), 'unexpected source in downloaded artifact')
        if os.name == 'nt':
            shutil.copytree(ROOT / directory, destination)
        else:
            destination.symlink_to(ROOT / directory, target_is_directory=True)


def main():
    """Write a machine-readable aggregate result on both success and failure."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('audit', 'seal', 'aggregate'))
    parser.add_argument('--root', type=Path)
    args = parser.parse_args()
    report = {'state': 'FAIL'}
    try:
        if args.action == 'audit':
            monolithic_view((ROOT / '.github/workflows/software-wgpu.yml').read_text(), strict=True)
            for path, expected in contract()['independent_workflow_sha256'].items():
                require(hashlib.sha256((ROOT / path).read_text().encode()).hexdigest() == expected,
                        'independent workflow changed: '+path)
            report = {'state': 'PASS', 'domains': list(contract()['artifacts'])}
        elif args.action == 'seal':
            seal(os.environ['DOMAIN'], os.environ)
            return 0
        else:
            report = aggregate(args.root, json.loads(os.environ['REQUIRED_RESULTS']), os.environ)
        print(json.dumps(report))
        return 0
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        report['error'] = str(error)
        print(json.dumps(report), file=sys.stderr)
        return 1
    finally:
        if args.action == 'aggregate':
            path = ROOT / 'target/software-wgpu-aggregate.json'
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(report, indent=2)+'\n')


if __name__ == '__main__':
    sys.exit(main())
