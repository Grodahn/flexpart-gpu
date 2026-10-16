#!/usr/bin/env python3
"""Require two distinct fresh builds and audit every retained #180 artifact."""

import argparse
import json
import re
from pathlib import Path

from verify_top_boundary_oracle import ARTIFACTS, ROOT, FIXTURE, TOOLS, decode, digest, require, text_digest, verify_linkage


def retained(run):
    """Verify retained bytes and executable call evidence, not just report status."""
    report = json.loads((run / 'report.json').read_text())
    require(report['state'] == 'PASS', 'record-only execution is not regression verification')
    require(report['science'] == decode(run / 'output.txt'), 'decoded report/raw disagreement')
    require(digest(run / 'output.txt') == digest(FIXTURE / 'output.txt'), 'frozen raw output changed')
    baseline = json.loads((FIXTURE / 'report.json').read_text())
    require(report['science'] == baseline['science'], 'scientific baseline disagreement')
    p = report['provenance']
    for key in ('pinned_commit', 'sources_sha256', 'tools_sha256', 'profile', 'driver_flags', 'verified_call_edges'):
        require(p[key] == baseline['provenance'][key], f'baseline provenance changed: {key}')
    require(p['tools_sha256'] == {s: text_digest(ROOT / s) for s in TOOLS}, 'stale verifier/tools')
    require(set(p['artifacts_sha256']) == set(ARTIFACTS) and len(p['linked_objects_sha256']) > 20,
            'incomplete provenance inventories')
    linked = (run / 'build/linked-objects.txt').read_text().splitlines()
    require(linked == sorted(set(linked)) and 'FLEXPART.o' not in linked and
            set(linked) == set(p['linked_objects_sha256']), 'incomplete linked-object hash inventory')
    for artifact, expected in p['artifacts_sha256'].items():
        require(digest(run / artifact) == expected, f'changed artifact: {artifact}')
    for obj, expected in p['linked_objects_sha256'].items():
        require(digest(run / 'build/upstream' / obj) == expected, f'changed linked object: {obj}')
    for source, expected in p['sources_sha256'].items():
        require(text_digest(run / 'build/upstream' / Path(source).relative_to('src')) == expected,
                f'changed source: {source}')
    for name in ('checkout-before.txt', 'checkout-after.txt'):
        require((run / name).read_text().splitlines() == [p['pinned_commit']], 'checkout record changed')
    require(re.fullmatch(r'sha256:[0-9a-f]{64}', p['image_id']), 'missing image identity')
    require((run / 'build/advance-original.text').read_bytes() == (run / 'build/advance-exported.text').read_bytes(),
            'reference instruction bytes changed')
    require(verify_linkage(run / 'build') == p['verified_call_edges'], 'call linkage changed')
    require((run / 'build/compiler-identity.txt').read_text().strip() == p['compiler'], 'compiler record changed')
    link_map = (run / 'build/w-production-oracle.link-map').read_text()
    origins = set(re.findall(r'(/tmp/flexpart-top-research\.[^/\s]+/build/upstream)/advance_mod.o', link_map))
    require(len(origins) == 1, 'missing or mixed fresh build origin')
    return report, origins.pop()


def compare(first, second):
    a, origin_a = retained(first)
    b, origin_b = retained(second)
    require(origin_a != origin_b, 'two relocated copies of one build cannot prove reproducibility')
    require(a['science'] == b['science'] and digest(first / 'output.txt') == digest(second / 'output.txt'),
            'fresh output did not reproduce exactly')
    require(a['provenance']['image_id'] == b['provenance']['image_id'], 'different immutable images')
    return {'schema': {'id': 'flexpart-gpu.top-boundary-reproducibility', 'version': 1},
            'state': 'PASS', 'comparison': 'exact raw bytes and decoded observations; distinct fresh source builds',
            'baseline_report_sha256': digest(FIXTURE / 'report.json'),
            'verifier_sha256': text_digest(Path(__file__)), 'raw_output_sha256': digest(first / 'output.txt'),
            'runs': [{'report_sha256': digest(run / 'report.json'), 'build_origin': origin,
                      'provenance': r['provenance']} for run, r, origin in ((first,a,origin_a),(second,b,origin_b))]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--first', type=Path, required=True)
    parser.add_argument('--second', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = compare(args.first, args.second)
    args.output.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8', newline='\n')
    print(json.dumps({'state': result['state'], 'push_cases': 24, 'advance_cases': 72,
                      'builds': 2, 'report': str(args.output)}))


if __name__ == '__main__':
    main()
