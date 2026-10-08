#!/usr/bin/env python3
"""Audit #175's finite workflow selections and existing libtest evidence.

This checks coverage and logs only; it never launches tests or defines science.
"""
from __future__ import annotations

import argparse
import hashlib
from itertools import product
import json
from pathlib import Path
import re
import shlex
import sys

ROOT = Path(__file__).resolve().parents[1]
BASELINE = ROOT / 'docs/ci-test-coverage-before.json'
WORKFLOWS = ('.github/workflows/software-wgpu.yml', '.github/workflows/validation-gate.yml')
STATIC_ORDER = 'test_forward_timeloop_operator_call_order_is_preserved'
LIB_FILTERS = ('meteorology::field::tests', 'meteorology::snapshot::tests',
               'meteorology::vertical::', 'meteorology::vertical_sampling::tests')
DRIVER_MARKERS = ('TIMELOOP-ORDER-126', 'TIMELOOP-DEFERRED-126', 'TIMELOOP-ERROR-126',
                  'TIMELOOP-FORWARD-175', 'TIMELOOP-SORT-175', 'using software WGSL adapter')
CHANGED_STEPS = {
    'Preserve simulation timestep order and output boundaries (#126)',
    'Prove dry-deposition capacity/prefix handoff (#135)',
    'Prove wet-deposition capacity/prefix handoff (#138)',
    'Prove separated Hanna-Langevin capacity/prefix handoff (#139)',
    'Run corpus deterministic subset (Issue #6, point 2)',
    'Run full Rust library test suite', 'Run canonical meteorology contract tests',
}


def require(condition, message):
    """Reject incomplete coverage or evidence rather than reporting success."""
    if not condition:
        raise ValueError(message)


def steps(text):
    """Read the existing workflows' named step blocks without a YAML dependency."""
    result = {}
    for block in re.split(r'^      - ', text, flags=re.M)[1:]:
        title = block.splitlines()[0]
        if title.startswith('name: '):
            result[title[6:]] = block
    return result


def environment(text):
    """Keep explicit workflow and step adapter/requirement settings in the audit."""
    return dict(re.findall(r'^\s+(FLEXPART_GPU_\w+|WGPU_BACKEND|LIBGL_ALWAYS_SOFTWARE): (.+)$', text, re.M))


def invocations(text, workflow):
    """Expand finite shell configuration loops into exact Cargo command rows."""
    rows = []
    defaults = environment(text.split('    steps:')[0])
    for name, block in steps(text).items():
        loops = []
        for line in block.splitlines():
            stripped = line.strip()
            loop = re.fullmatch(r'for (\w+) in ([01](?: [01])*); do', stripped)
            if loop:
                loops.append((loop[1], loop[2].split()))
                continue
            if stripped == 'done':
                if loops:
                    loops.pop()
                continue
            if 'cargo test ' not in line or stripped.startswith('#'):
                continue
            prefixes, command = line.strip().split('cargo test ', 1)
            command = re.split(r'\s+(?:2>&1|>>?|\|)', command)[0]
            inline = dict(re.findall(r'(\w+)=("[^\"]*"|\S+)', prefixes))
            configurations = product(*(values for _, values in loops)) if loops else [()]
            for values in configurations:
                variables = dict(zip((key for key, _ in loops), values))
                env = {**defaults, **environment(block), **inline}
                for key, value in env.items():
                    value = value.strip('"')
                    for variable, replacement in variables.items():
                        value = value.replace('${'+variable+'}', replacement).replace('$'+variable, replacement)
                    env[key] = value
                rows.append({'workflow': workflow, 'step': name, 'argv': ['cargo', 'test', *shlex.split(command)], 'env': env})
    return rows


def driver_tests(target):
    """Use current declarations so an added production regression cannot disappear."""
    source = (ROOT / f'tests/{target}.rs').read_text(encoding='utf-8')
    return set(re.findall(r'#\[test\]\s+fn (test_\w+)\(', source))


def driver_cells(rows, tests_by_target=None):
    """Enumerate every driver test/configuration; source-text order is flag invariant."""
    cells = set()
    for row in rows:
        argv = row['argv']
        env = row['env']
        compaction = env.get('FLEXPART_GPU_COMPACTION', '0')
        validation = env.get('FLEXPART_GPU_VALIDATION', '0')
        for target in ('forward_timeloop', 'backward_timeloop'):
            if target not in argv:
                continue
            selected = set(tests_by_target[target]) if tests_by_target is not None else driver_tests(target)
            before = argv[:argv.index('--')] if '--' in argv else argv
            position = before.index(target)
            filters = before[position+1:]
            filters = [v for v in filters if not v.startswith('-') and v not in ('forward_timeloop', 'backward_timeloop')]
            if filters:
                selected = {test for test in selected if any(value in test for value in filters)}
            if '--skip' in argv:
                selected = {test for test in selected if argv[argv.index('--skip')+1] not in test}
            for test in selected:
                flags = ('independent', 'independent') if test == STATIC_ORDER else (compaction, validation)
                cells.add((target, test, *flags))
    return cells


def normalized(block):
    """Treat transcript retention as equivalent to tee, preserving every assertion."""
    block = re.sub(r'2>&1 \| tee (target/[^\n]+)', r'> \1 2>&1', block)
    return block.strip()


def uploaded_paths(block):
    """Read actual upload inputs, excluding log paths mentioned in run commands."""
    match = re.search(r'^          path: (.+)$', block, re.M)
    require(match is not None, 'artifact upload has no path')
    if match[1] != '|':
        return {match[1].strip()}
    paths = set()
    for line in block[match.end():].splitlines():
        if not line.strip():
            continue
        if not line.startswith('            '):
            break
        paths.add(line.strip())
    return paths


def current_driver_inventory(baseline):
    """Keep historical test identities fixed while requiring newly added tests too."""
    current = {target: driver_tests(target) for target in baseline['driver_tests']}
    for target, names in baseline['driver_tests'].items():
        require(names and set(names) <= current[target], f'baseline driver test removed: {target}')
    return current


def audit(workflows=None, baseline=None):
    """Map every baseline invocation to preserved driver cells or a remaining command."""
    baseline = baseline or json.loads(BASELINE.read_text(encoding='utf-8'))
    workflows = workflows or {path: (ROOT / path).read_text(encoding='utf-8') for path in WORKFLOWS}
    after = []
    for path, text in workflows.items():
        # Triggers, required job identity and global adapter/provenance settings stay exact.
        require(text.split('    steps:')[0] == baseline['headers'][path], f'{path}: trigger/job/env drift')
        actual = steps(text)
        for name, block in baseline['protected_steps'][path].items():
            require(name in actual and hashlib.sha256(normalized(actual[name]).encode()).hexdigest() == block, f'{path}: protected evidence step changed: {name}')
        uploads = [block for block in actual.values()
                   if re.search(r'^        uses: actions/upload-artifact@v4$', block, re.M)]
        require(len(uploads) == 1, f'{path}: required artifact upload missing')
        require(re.search(r'^        if: always\(\)$', uploads[0], re.M),
                f'{path}: artifact upload must run on failure')
        paths = uploaded_paths(uploads[0])
        for artifact in baseline['artifact_paths'][path]:
            if artifact not in baseline['replaced_log_globs']:
                require(artifact.removeprefix('path: ') in paths, f'missing retained artifact: {artifact}')
        after.extend(invocations(text, path))
    before = baseline['invocations']
    current = current_driver_inventory(baseline)
    after_cells = driver_cells(after, current)
    require(driver_cells(before, baseline['driver_tests']) <= after_cells, 'missing baseline driver/configuration cell')
    for row in after:
        if any(target in row['argv'] for target in ('forward_timeloop', 'backward_timeloop')):
            require(row['env'].get('FLEXPART_GPU_PBL_CPU', '0') != '1', 'CPU replacement invalidates driver device evidence')
            reference = next(r['env'] for r in before if r['workflow'] == row['workflow']
                             and any(target in r['argv'] for target in ('forward_timeloop', 'backward_timeloop')))
            for key in ('FLEXPART_GPU_SOFTWARE', 'WGPU_BACKEND', 'LIBGL_ALWAYS_SOFTWARE',
                        'FLEXPART_GPU_CANDIDATE_REVISION'):
                require(row['env'].get(key) == reference.get(key), f'driver adapter/provenance environment changed: {key}')
    for row in before:
        argv = row['argv']
        if any(target in argv for target in ('forward_timeloop', 'backward_timeloop')):
            continue
        candidates = [r for r in after if r['workflow'] == row['workflow'] and r['env'] == row['env']]
        preserved = any(r['argv'] == argv for r in candidates)
        if '--lib' in argv and any(value in argv for value in LIB_FILTERS):
            preserved |= any(r['argv'][:3] == ['cargo', 'test', '--lib'] and
                             (len(r['argv']) == 3 or r['argv'][3] == '--') for r in candidates)
        require(preserved, f'unmapped baseline command: {argv}')
    active_lines = [line.strip() for text in workflows.values() for line in text.splitlines()
                    if not line.lstrip().startswith('#')]
    for requirement in baseline['required_checks']:
        require(any(line == requirement or line.startswith(requirement + ' ') for line in active_lines),
                f'missing required check: {requirement}')
    require(hashlib.sha256((ROOT / 'scripts/ci-gate.sh').read_text(encoding='utf-8').encode()).hexdigest() == baseline['gate_source_sha256'], 'pinned-oracle/fresh-report gate changed')
    forward = lambda rows: sum('forward_timeloop' in r['argv'] for r in rows)
    full = lambda rows: sum('forward_timeloop' in r['argv'] and '--skip' not in r['argv'] and
                           not any(v.startswith('test_forward_') for v in r['argv']) for r in rows)
    return {'state': 'PASS', 'baseline_invocations': len(before), 'after_invocations': len(after),
            'total_with_retained_gate': [len(before)+len(baseline['gate_invocations']), len(after)+len(baseline['gate_invocations'])],
            'forward_invocations': [forward(before), forward(after)],
            'full_forward_targets': [full(before), full(after)],
            'driver_configuration_cells': len(after_cells)}


def summaries(output):
    """Require real successful tests with no ignored execution or missing summary."""
    values = re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored', output)
    require(values, 'missing successful test summary')
    require('FAILED' not in output and not re.search(r'(?im)^(?!test )[^\n]*(?:skipp(?:ed|ing)|NoAdapter|no adapter)', output), 'failure/skip/missing adapter')
    rows = [tuple(map(int, row)) for row in values]
    require(all(p > 0 and f == 0 and i == 0 for p, f, i in rows), 'zero/failed/ignored tests')
    return sum(row[0] for row in rows)


def check_driver_log(output, compaction, validation, backward=False):
    """Check every selected test and existing device/capacity marker in a fresh log."""
    baseline = json.loads(BASELINE.read_text(encoding='utf-8'))
    current = current_driver_inventory(baseline)
    expected = set(current['forward_timeloop'])
    if compaction == '1':
        expected.remove(STATIC_ORDER)
    if backward:
        expected |= current['backward_timeloop']
    require(summaries(output) == len(expected), 'driver executed-test count mismatch')
    names = set(re.findall(r'\b(test_\w+)\s+\.\.\.', output))
    require(names == expected, 'missing/unexpected driver test names')
    for marker in DRIVER_MARKERS:
        require(marker in output, f'missing device marker: {marker}')
    if backward:
        require('TIMELOOP-BACKWARD-175' in output, 'backward device execution missing')
    for active in (1, 3, 8):
        for marker in ('DRY-FORWARD-135', 'WET-FORWARD-138'):
            require(f'{marker}: capacity=8 active={active} compaction={compaction}' in output,
                    f'missing capacity/configuration marker: {marker}/{active}/{compaction}')
    require(compaction in ('0', '1') and validation in ('0', '1'), 'invalid configuration')
    return {'state': 'PASS', 'tests': len(expected), 'compaction': compaction, 'validation': validation}


def check_library_log(output):
    """Prove the removed canonical subsets executed in the complete library suite."""
    count = summaries(output)
    baseline = json.loads(BASELINE.read_text(encoding='utf-8'))
    passed = set(re.findall(r'^test ([\w:]+) \.\.\. ok\r?$', output, re.M))
    canonical = set()
    for prefix in LIB_FILTERS:
        required = set(baseline['library_tests'][prefix])
        require(required and required <= passed, f'library subset incomplete: {prefix}')
        canonical |= required
    return {'state': 'PASS', 'library_tests': count, 'canonical_subsets': len(LIB_FILTERS),
            'canonical_tests': len(canonical)}


def main():
    """Emit one audit result; evidence failures always return nonzero."""
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group()
    group.add_argument('--log', type=Path)
    group.add_argument('--library-log', type=Path)
    parser.add_argument('--compaction', choices=('0', '1'))
    parser.add_argument('--validation', choices=('0', '1'))
    parser.add_argument('--backward', action='store_true')
    args = parser.parse_args()
    try:
        if args.log:
            require(args.compaction is not None and args.validation is not None, 'driver configuration required')
            result = check_driver_log(args.log.read_text(encoding='utf-8'), args.compaction, args.validation, args.backward)
        elif args.library_log:
            result = check_library_log(args.library_log.read_text(encoding='utf-8'))
        else:
            result = audit()
        print(json.dumps(result, sort_keys=True))
        return 0
    except (OSError, ValueError, KeyError) as error:
        print(json.dumps({'state': 'FAIL', 'error': str(error)}))
        return 1


if __name__ == '__main__':
    sys.exit(main())
