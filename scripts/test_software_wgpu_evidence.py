#!/usr/bin/env python3
"""Focused fail-closed fixtures for #177 orchestration, without GPU computation."""
import copy
import json
import os
from pathlib import Path
import subprocess
import shutil
import tempfile
import unittest
from unittest.mock import patch

import software_wgpu_evidence as evidence

REAL_AUDIT_PAYLOAD = evidence.audit_payload
REAL_CONTRACT = evidence.contract


class AggregateTests(unittest.TestCase):
    """Validate provenance, bytes and genuine command outcomes across both domains."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.env = {'GITHUB_SHA': 'a'*40, 'FLEXPART_GPU_CANDIDATE_REVISION': 'b'*40,
                    'GITHUB_RUN_ID': '123', 'GITHUB_RUN_ATTEMPT': '2'}
        self.domains = ('gpu-meteorology', 'gpu-transport-physics')
        self.results = {d: {'result': 'success'} for d in self.domains}
        self.contract = {'artifacts': {d: ['target/ci-gate/test.log', 'target/ci-gate/gpu-preflight.json'] for d in self.domains},
                         'steps': [{'block': '', 'name': 'Runtime smoke', 'owners': list(self.domains)}]}
        self.addCleanup(patch.stopall)
        patch.object(evidence, 'contract', return_value=self.contract).start()
        patch.object(evidence, 'prepare_sources').start()
        patch.object(evidence.subprocess, 'check_output', return_value=self.env['GITHUB_SHA']+'\n').start()
        # Scientific assertions are separately exercised by the unchanged CI
        # validators and real artifact inspection; these fixtures test the handoff.
        patch.object(evidence, 'audit_payload', side_effect=lambda root, domain:
                     evidence.execution_summary(root, evidence.files_for(root, domain))).start()
        for domain in self.domains:
            bundle = self.bundle(domain)
            (bundle / 'target/ci-gate').mkdir(parents=True)
            (bundle / 'target/ci-gate/test.log').write_text('test actual ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored\n')
            preflight = {'status': 'passed', 'report': {'adapter': {'adapter_class': 'software_wgsl', 'backend': 'Vulkan', 'name': 'llvmpipe'},
                         'smoke_test': {'status': 'passed', 'actual_value': 17, 'expected_value': 17}}}
            (bundle / 'target/ci-gate/gpu-preflight.json').write_text(json.dumps(preflight))
            record = {'schema': evidence.SCHEMA, 'domain': domain, 'identity': self.env,
                      'status': 'passed', 'stages': {'original-0': {'outcome': 'success', 'conclusion': 'success'}},
                      'contract_sha256': evidence.digest(evidence.CONTRACT), 'files': evidence.files_for(bundle, domain),
                      'execution': evidence.execution_summary(bundle, evidence.files_for(bundle, domain))}
            (bundle / 'manifest.json').write_text(json.dumps(record))

    def bundle(self, domain):
        return self.root / f'software-wgpu-{domain.removeprefix("gpu-")}-123-2'

    def edit_record(self, domain, change):
        path = self.bundle(domain) / 'manifest.json'
        record = json.loads(path.read_text())
        change(record)
        path.write_text(json.dumps(record))

    def aggregate(self):
        return evidence.aggregate(self.root, self.results, self.env)

    def test_all_domains_valid_pass(self):
        self.assertEqual(self.aggregate()['state'], 'PASS')

    def test_frozen_workflow_paths_are_portable(self):
        baseline = json.loads(evidence.CONTRACT.read_text())
        for path in baseline['independent_workflow_sha256']:
            self.assertNotIn('\\', path)
            self.assertTrue((evidence.ROOT / path).is_file())

    def test_transport_top_addition_preserves_independent_workflow(self):
        path = '.github/workflows/validation-gate.yml'
        baseline = json.loads(evidence.CONTRACT.read_text())['independent_workflow_sha256'][path]
        text = (evidence.ROOT / path).read_text()
        evidence.audit_independent_workflow(path, text, baseline)
        for changed in (text.replace(evidence.TRANSPORT_TOP_TECHNICAL_STEP, ''),
                        text.replace('name: Technical validation gate', 'name: changed old gate')):
            with self.assertRaises(ValueError):
                evidence.audit_independent_workflow(path, changed, baseline)

    def test_original_cache_namespace_is_required(self):
        baseline = json.loads(evidence.CONTRACT.read_text())
        text = (evidence.ROOT / '.github/workflows/software-wgpu.yml').read_text()
        with patch.object(evidence, 'contract', return_value=baseline):
            evidence.monolithic_view(text, strict=True)
            with self.assertRaisesRegex(ValueError, 'cache namespace'):
                evidence.monolithic_view(text.replace('          shared-key: ', '          shared-key: other-'))

    def test_complete_vector_stage_and_artifacts_are_required(self):
        baseline = REAL_CONTRACT()
        text = (evidence.ROOT / '.github/workflows/software-wgpu.yml').read_text()
        with patch.object(evidence, 'contract', return_value=baseline):
            self.assertIn('shared-height-vector-190', evidence.expected_stages('gpu-meteorology'))
            self.assertIn('target/ci-gate/shared-height-vector/*.json', baseline['artifacts']['gpu-meteorology'])
            evidence.monolithic_view(text, strict=True)
            changed = text.replace('      - '+evidence.VECTOR_STEP, '')
            self.assertNotEqual(text, changed)
            with self.assertRaisesRegex(ValueError, 'protected evidence step missing'):
                evidence.monolithic_view(changed, strict=True)

    def test_shared_height_query_stage_and_artifacts_are_required(self):
        baseline = REAL_CONTRACT()
        text = (evidence.ROOT / '.github/workflows/software-wgpu.yml').read_text()
        with patch.object(evidence, 'contract', return_value=baseline):
            self.assertIn('shared-height-query-185', evidence.expected_stages('gpu-meteorology'))
            self.assertIn('target/ci-gate/shared-height-query/*.json', baseline['artifacts']['gpu-meteorology'])
            evidence.monolithic_view(text, strict=True)
            changed = text.replace('      - '+evidence.QUERY_STEP, '')
            self.assertNotEqual(text, changed)
            with self.assertRaisesRegex(ValueError, 'protected evidence step missing'):
                evidence.monolithic_view(changed, strict=True)

    def test_each_domain_failure_cancellation_skip_rejected(self):
        for domain in self.domains:
            for state in ('failure', 'cancelled', 'skipped'):
                with self.subTest(domain=domain, state=state):
                    self.results[domain]['result'] = state
                    with self.assertRaisesRegex(ValueError, 'failed/cancelled/skipped'):
                        self.aggregate()
                    self.results[domain]['result'] = 'success'

    def test_missing_result_rejected(self):
        del self.results[self.domains[0]]
        with self.assertRaises(ValueError):
            self.aggregate()

    def test_absent_artifact_rejected(self):
        self.bundle(self.domains[0]).rename(self.root / 'wrong-artifact')
        with self.assertRaisesRegex(ValueError, 'artifact absent'):
            self.aggregate()

    def test_wrong_commit_run_attempt_rejected(self):
        domain = self.domains[0]
        for key in self.env:
            with self.subTest(key=key):
                self.edit_record(domain, lambda r: r.update(identity={**self.env, key: 'wrong'}))
                with self.assertRaisesRegex(ValueError, 'different candidate/source/run/attempt'):
                    self.aggregate()
                self.edit_record(domain, lambda r: r.update(identity=self.env))

    def test_missing_adapter_rejected_even_with_updated_hash(self):
        domain = self.domains[0]
        path = self.bundle(domain) / 'target/ci-gate/gpu-preflight.json'
        value = json.loads(path.read_text()); del value['report']['adapter']
        path.write_text(json.dumps(value))
        self.edit_record(domain, lambda r: r.update(files=evidence.files_for(self.bundle(domain), domain)))
        with self.assertRaises(KeyError):
            self.aggregate()

    def test_zero_test_rejected_even_with_updated_hash(self):
        domain = self.domains[0]
        (self.bundle(domain) / 'target/ci-gate/test.log').write_text('test result: ok. 0 passed; 0 failed; 0 ignored\n')
        self.edit_record(domain, lambda r: r.update(files=evidence.files_for(self.bundle(domain), domain)))
        with self.assertRaisesRegex(ValueError, 'zero'):
            self.aggregate()

    def test_malformed_manifest_rejected(self):
        (self.bundle(self.domains[0]) / 'manifest.json').write_text('{')
        with self.assertRaises(ValueError):
            self.aggregate()

    def test_malformed_evidence_rejected(self):
        domain = self.domains[0]
        (self.bundle(domain) / 'target/ci-gate/gpu-preflight.json').write_text('{')
        self.edit_record(domain, lambda r: r.update(files=evidence.files_for(self.bundle(domain), domain)))
        with self.assertRaises(ValueError):
            self.aggregate()

    def test_unreferenced_malformed_required_json_fails_before_revalidation(self):
        domain = self.domains[1]
        bundle = self.bundle(domain)
        path = 'target/ci-gate/retained-report.json'
        self.contract['artifacts'][domain].append(path)
        (bundle / path).write_text('{malformed')
        with self.assertRaisesRegex(ValueError, 'invalid required JSON artifact.*retained-report'):
            REAL_AUDIT_PAYLOAD(bundle, domain)

    def test_nonstandard_json_numbers_fail_closed(self):
        path = self.root / 'required.json'
        for value in ('NaN', 'Infinity', '-Infinity'):
            path.write_text('{"value": '+value+'}')
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, 'nonstandard JSON number'):
                evidence.validate_json_files(self.root, {'required.json': evidence.digest(path)})

    def test_resident_revalidation_preserves_original_bytes(self):
        directory = self.root / 'target/ci-gate/resident-advection-production'
        directory.mkdir(parents=True)
        modes = ('original', 'production', 'validation', 'compaction')
        for mode in modes:
            prefix = '' if mode == 'original' else mode+'-'
            for name in ('report', 'interior-deferred-boundary'):
                (directory / f'{prefix}{name}.json').write_text(json.dumps({'selection': mode}))
        (directory / 'inputs.json').write_text('{"retained_input": true}')
        original = {p.name: p.read_bytes() for p in directory.iterdir()}
        observed = []

        def inspect(root, selected, label):
            self.assertEqual(root, self.root)
            for name in ('report', 'interior-deferred-boundary'):
                self.assertEqual(json.loads((selected / f'{name}.json').read_text())['selection'], label)
            self.assertEqual((selected / 'inputs.json').read_bytes(), original['inputs.json'])
            observed.append(label)

        with patch.object(evidence, 'run_resident_audit', side_effect=inspect):
            evidence.revalidate_resident_advection(self.root)
        self.assertEqual(observed, list(modes))
        self.assertEqual({p.name: p.read_bytes() for p in directory.iterdir()}, original)
        self.assertEqual(list(directory.parent.glob('resident-validation-*')), [])

    def test_original_resident_report_must_pass_actual_validator(self):
        directory = self.root / 'target/ci-gate/resident-advection-production'
        directory.mkdir(parents=True)
        report = directory / 'report.json'
        report.write_text('{malformed original}')
        with self.assertRaisesRegex(ValueError, 'resident advection original'):
            evidence.revalidate_resident_advection(self.root)
        self.assertEqual(report.read_text(), '{malformed original}')

    def test_resident_validator_failure_cannot_hide_behind_pass_output(self):
        result = subprocess.CompletedProcess([], 37, stdout='PASS', stderr='')
        with patch.object(evidence.subprocess, 'run', return_value=result):
            with self.assertRaisesRegex(ValueError, 'resident advection failed-command'):
                evidence.run_resident_audit(self.root, self.root, 'failed-command')

    def test_seal_rejects_copy_that_differs_from_validated_source(self):
        domain = self.domains[1]
        shutil.copytree(self.bundle(domain) / 'target', self.root / 'target')
        workflow = self.root / '.github/workflows/software-wgpu.yml'
        workflow.parent.mkdir(parents=True)
        workflow.write_text('')
        env = {**self.env, 'DOMAIN_STEPS': json.dumps({'original-0': {'outcome': 'success', 'conclusion': 'success'}})}
        copy_file = shutil.copyfile

        def changed_copy(source, target):
            result = copy_file(source, target)
            if target.name == 'test.log':
                target.write_text('different uploaded bytes')
            return result

        with patch.object(evidence, 'ROOT', self.root), patch.object(evidence, 'monolithic_view'), \
                patch.object(evidence.shutil, 'copyfile', side_effect=changed_copy):
            with self.assertRaisesRegex(ValueError, 'uploaded evidence differs'):
                evidence.seal(domain, env)
        manifest = self.root / 'target/software-wgpu' / domain / 'manifest.json'
        self.assertEqual(json.loads(manifest.read_text())['status'], 'failed')

    def test_missing_evidence_and_tampered_bytes_rejected(self):
        path = self.bundle(self.domains[0]) / 'target/ci-gate/test.log'
        path.write_text('misleading PASS')
        with self.assertRaisesRegex(ValueError, 'artifact bytes'):
            self.aggregate()
        path.unlink()
        with self.assertRaisesRegex(ValueError, 'missing retained artifact'):
            self.aggregate()

    def test_successful_job_without_required_step_rejected(self):
        self.edit_record(self.domains[0], lambda r: r.update(stages={}))
        with self.assertRaisesRegex(ValueError, 'stage absent'):
            self.aggregate()

    def test_runtime_step_failure_rejected_despite_successful_job(self):
        for state in ('failure', 'cancelled', 'skipped'):
            self.edit_record(self.domains[0], lambda r: r.update(stages={'original-0': {'outcome': state, 'conclusion': 'success'}}))
            with self.subTest(state=state), self.assertRaisesRegex(ValueError, 'upstream command'):
                self.aggregate()

    def test_actual_runtime_shell_failure_preserves_exit_despite_pass_marker(self):
        step = next(s for s in json.loads(evidence.CONTRACT.read_text())['steps'] if s['name'].startswith('Prove H2D'))
        body = step['block'].split('        run: |\n')[1]
        body = '\n'.join(line[10:] for line in body.splitlines())
        bash = 'C:/Program Files/Git/bin/bash.exe' if os.name == 'nt' else 'bash'
        result = subprocess.run([bash, '-c', 'cargo() { echo PASS; return 37; }\n'+body], cwd=self.root, capture_output=True)
        self.assertEqual(result.returncode, 37)

    def test_failed_smoke_value_and_wrong_adapter_rejected(self):
        for mutate in (lambda r: r['report']['smoke_test'].update(actual_value=0),
                       lambda r: r['report']['adapter'].update(adapter_class='hardware_gpu'),
                       lambda r: r['report']['adapter'].update(backend='Dx12')):
            domain = self.domains[0]; path = self.bundle(domain) / 'target/ci-gate/gpu-preflight.json'
            original = json.loads(path.read_text()); value = copy.deepcopy(original); mutate(value)
            path.write_text(json.dumps(value))
            self.edit_record(domain, lambda r: r.update(files=evidence.files_for(self.bundle(domain), domain)))
            with self.assertRaises(ValueError):
                self.aggregate()
            path.write_text(json.dumps(original))


if __name__ == '__main__':
    unittest.main()
