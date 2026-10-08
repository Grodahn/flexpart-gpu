#!/usr/bin/env python3
"""Negative fixtures for #175's coverage audit and changed workflow shell steps."""
from __future__ import annotations

import os
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import check_ci_test_coverage as coverage


class CoverageTests(unittest.TestCase):
    """Every baseline configuration and protected evidence assertion must survive."""

    def workflows(self):
        return {p: (coverage.ROOT / p).read_text(encoding='utf-8') for p in coverage.WORKFLOWS}

    def test_baseline_maps_and_invocations_decrease(self):
        result = coverage.audit()
        self.assertEqual(result['forward_invocations'], [12, 4])
        self.assertEqual(result['full_forward_targets'], [6, 2])
        self.assertLess(result['after_invocations'], result['baseline_invocations'])

    def test_missing_secondary_validation_cell_fails(self):
        texts = self.workflows()
        p = coverage.WORKFLOWS[0]
        texts[p] = texts[p].replace('for validation in 0 1; do', 'for validation in 0; do')
        with self.assertRaisesRegex(ValueError, 'configuration cell'):
            coverage.audit(texts)

    def test_missing_capacity_test_fails(self):
        texts = self.workflows()
        p = coverage.WORKFLOWS[0]
        texts[p] = texts[p].replace('--skip '+coverage.STATIC_ORDER,
                                   '--skip test_forward_timeloop_dry_deposition_multiple_active_and_full_capacity')
        with self.assertRaisesRegex(ValueError, 'configuration cell'):
            coverage.audit(texts)

    def test_removed_scientific_assertion_fails(self):
        texts = self.workflows()
        p = coverage.WORKFLOWS[0]
        texts[p] = texts[p].replace('assert report["status"] == "pass"', 'assert True')
        with self.assertRaisesRegex(ValueError, 'protected evidence'):
            coverage.audit(texts)

    def test_removed_device_requirement_fails(self):
        texts = self.workflows()
        p = coverage.WORKFLOWS[0]
        texts[p] = texts[p].replace('FLEXPART_GPU_SOFTWARE: "1"', 'FLEXPART_GPU_SOFTWARE: "0"')
        with self.assertRaisesRegex(ValueError, 'trigger/job/env'):
            coverage.audit(texts)

    def test_removed_library_audit_fails(self):
        texts = self.workflows()
        p = coverage.WORKFLOWS[1]
        texts[p] = texts[p].replace('python3 scripts/check_ci_test_coverage.py --library-log', 'echo')
        with self.assertRaisesRegex(ValueError, 'required check'):
            coverage.audit(texts)

    def test_driver_cpu_replacement_fails(self):
        texts = self.workflows()
        p = coverage.WORKFLOWS[0]
        texts[p] = texts[p].replace('FLEXPART_GPU_COMPACTION=1 FLEXPART_GPU_VALIDATION=',
                                   'FLEXPART_GPU_PBL_CPU=1 FLEXPART_GPU_COMPACTION=1 FLEXPART_GPU_VALIDATION=')
        with self.assertRaisesRegex(ValueError, 'CPU replacement'):
            coverage.audit(texts)

    def test_removed_baseline_driver_declaration_fails(self):
        current = {target: coverage.driver_tests(target) for target in ('forward_timeloop', 'backward_timeloop')}
        current['forward_timeloop'].remove('test_forward_timeloop_dry_deposition_multiple_active_and_full_capacity')
        with patch.object(coverage, 'driver_tests', side_effect=lambda target: current[target]):
            with self.assertRaisesRegex(ValueError, 'baseline driver test removed'):
                coverage.audit()

    def test_inline_adapter_override_fails(self):
        for key, value in [('FLEXPART_GPU_SOFTWARE', '0'), ('WGPU_BACKEND', 'dx12'),
                           ('LIBGL_ALWAYS_SOFTWARE', '0'), ('FLEXPART_GPU_CANDIDATE_REVISION', 'wrong')]:
            texts = self.workflows()
            p = coverage.WORKFLOWS[0]
            texts[p] = texts[p].replace('FLEXPART_GPU_COMPACTION=1 FLEXPART_GPU_VALIDATION=',
                                       f'{key}={value} FLEXPART_GPU_COMPACTION=1 FLEXPART_GPU_VALIDATION=')
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'adapter/provenance'):
                coverage.audit(texts)

    def test_commented_required_audit_fails(self):
        texts = self.workflows()
        p = coverage.WORKFLOWS[1]
        texts[p] = texts[p].replace('          python3 scripts/check_ci_test_coverage.py --library-log',
                                   '          # python3 scripts/check_ci_test_coverage.py --library-log')
        with self.assertRaisesRegex(ValueError, 'required check'):
            coverage.audit(texts)

    def test_log_path_in_command_does_not_replace_upload(self):
        texts = self.workflows()
        p = coverage.WORKFLOWS[0]
        texts[p] = texts[p].replace('            target/ci-gate/hanna-prefix.log\n', '')
        with self.assertRaisesRegex(ValueError, 'missing retained artifact'):
            coverage.audit(texts)

    def test_artifact_upload_must_run_on_failure(self):
        texts = self.workflows()
        p = coverage.WORKFLOWS[0]
        texts[p] = texts[p].replace('        if: always()', '        if: success()')
        with self.assertRaisesRegex(ValueError, 'upload must run on failure'):
            coverage.audit(texts)

    def library_fixture(self):
        baseline = json.loads(coverage.BASELINE.read_text(encoding='utf-8'))
        names = sorted({name for subset in baseline['library_tests'].values() for name in subset})
        return '\n'.join([*(f'test {name} ... ok' for name in names),
                          f'test result: ok. {len(names)} passed; 0 failed; 0 ignored'])

    def test_partial_library_subset_fails(self):
        output = self.library_fixture()
        name = json.loads(coverage.BASELINE.read_text(encoding='utf-8'))['library_tests'][coverage.LIB_FILTERS[0]][0]
        with self.assertRaisesRegex(ValueError, 'library subset incomplete'):
            coverage.check_library_log(output.replace(f'test {name} ... ok', ''))

    def fixture(self, compaction='0', backward=False):
        names = coverage.driver_tests('forward_timeloop')
        if compaction == '1':
            names.remove(coverage.STATIC_ORDER)
        if backward:
            names |= coverage.driver_tests('backward_timeloop')
        parts = [f'test {n} ... ok' for n in sorted(names)]
        parts += [f'test result: ok. {len(names)} passed; 0 failed; 0 ignored']
        parts += list(coverage.DRIVER_MARKERS)
        if backward:
            parts += ['TIMELOOP-BACKWARD-175']
        parts += [f'{m}: capacity=8 active={a} compaction={compaction}'
                  for m in ('DRY-FORWARD-135', 'WET-FORWARD-138') for a in (1, 3, 8)]
        return '\n'.join(parts)

    def test_all_four_driver_log_cells(self):
        for compaction in ('0', '1'):
            for validation in ('0', '1'):
                with self.subTest(compaction=compaction, validation=validation):
                    coverage.check_driver_log(self.fixture(compaction), compaction, validation)
        coverage.check_driver_log(self.fixture(backward=True), '0', '0', True)

    def test_missing_marker_adapter_capacity_and_skips_fail(self):
        fixture = self.fixture(backward=True)
        missing = [*coverage.DRIVER_MARKERS, 'TIMELOOP-BACKWARD-175',
                   'WET-FORWARD-138: capacity=8 active=3 compaction=0']
        for marker in missing:
            with self.subTest(marker=marker), self.assertRaises(ValueError):
                coverage.check_driver_log(fixture.replace(marker, ''), '0', '0', True)
        for suffix in ('NoAdapter', 'skipped', 'FAILED', 'test result: ok. 0 passed; 0 failed; 0 ignored',
                       'test result: ok. 1 passed; 0 failed; 1 ignored'):
            with self.subTest(suffix=suffix), self.assertRaises(ValueError):
                coverage.check_driver_log(fixture+'\n'+suffix, '0', '0', True)

    def test_library_subset_and_zero_test_fail_closed(self):
        output = self.library_fixture()
        coverage.check_library_log(output)
        for prefix in coverage.LIB_FILTERS:
            with self.subTest(prefix=prefix), self.assertRaises(ValueError):
                coverage.check_library_log(output.replace(prefix, 'other'))
        with self.assertRaises(ValueError):
            coverage.check_library_log('test result: ok. 0 passed; 0 failed; 0 ignored')

    def test_workflow_shell_preserves_exit_and_rejects_missing_evidence(self):
        # Exercise actual changed step bodies, with Cargo failures before any
        # compiler/device work. This is a shell fixture, not another test runner.
        bash = 'C:/Program Files/Git/bin/bash.exe' if os.name == 'nt' else shutil.which('bash')
        self.assertTrue(bash and Path(bash).exists(), 'Bash required for workflow shell negative fixtures')
        text = self.workflows()[coverage.WORKFLOWS[0]]
        blocks = coverage.steps(text)
        technical = coverage.steps(self.workflows()[coverage.WORKFLOWS[1]])
        blocks['Run full Rust library test suite'] = technical['Run full Rust library test suite']
        names = [n for n in coverage.CHANGED_STEPS if n in blocks]
        for name in names:
            body = blocks[name].split('        run: |\n', 1)[1]
            body = '\n'.join(line[10:] for line in body.splitlines())
            body = body.replace('python3 scripts/check_ci_test_coverage.py',
                '"'+sys.executable.replace('\\', '/')+'" "'+str(coverage.ROOT / 'scripts/check_ci_test_coverage.py').replace('\\', '/')+'"')
            for status in (37, 127, 0):
                with self.subTest(step=name, status=status), tempfile.TemporaryDirectory() as directory:
                    work = Path(directory)
                    (work/'target/ci-gate').mkdir(parents=True)
                    # Exported Bash function avoids executable-extension/PATH differences.
                    stub = f'cargo() {{ echo "test result: ok. 0 passed; 0 failed; 0 ignored"; return {status}; }}\n'
                    result = subprocess.run([bash, '-c', stub+body], cwd=work, capture_output=True, text=True)
                    if status:
                        self.assertEqual(result.returncode, status, result.stderr)
                    else:
                        self.assertNotEqual(result.returncode, 0, 'zero tests/missing evidence passed')


if __name__ == '__main__':
    unittest.main()
