#!/usr/bin/env python3
"""Audit the #203 settling/sticky-stop contract matrix without running scientific validation.

The matrix may only reference frozen #180 rows; it never carries oracle values. This checker
confirms that every reference resolves to genuine rows with the stated stop classification,
that cited pinned source lines contain the cited statements, and that every named repository
owner still exists. A PASS is contract consistency, never settling or transport parity.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MATRIX = ROOT / 'docs/research/settling-stop-203-matrix.json'
SCHEMA_ID = 'flexpart-gpu.settling-stop-contract'
SCHEMA_VERSION = 1
REQUIRED_CASES = [f'C{index:02d}' for index in range(1, 20)]
CASE_FIELDS = ('id', 'required', 'title', 'original', 'input', 'predictor', 'corrector', 'nstop',
               'downstream', 'gpu_classification', 'evidence', 'status', 'owner', 'blocked_by')
OWNER_FIELDS = ('component', 'path', 'symbol', 'owner', 'invariant', 'gap', 'producer', 'consumer',
                'lifetime', 'validation_side', 'assigned_to')
# Expectations are classifications of genuine rows; numeric oracle values are never allowed here.
ALLOWED_EXPECT = {'count', 'nstop', 'settling_nonzero', 'f32_representable'}
TOP_OFFSET_COLUMN = 'input_top_offset_0.0001_m'
TOP_OFFSET_M = 1.0e-4


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def f32_representable(value: float) -> bool:
    return struct.unpack('<f', struct.pack('<f', value))[0] == value


def fixture_tables(fixture: dict) -> dict[str, list[dict]]:
    science = fixture['science']
    columns = {'PUSH': science['push_columns'], 'ADVANCE': science['advance_columns']}
    return {table: [dict(zip(columns[table], row)) for row in science['rows'][table]] for table in columns}


def select(rows: list[dict], where: dict) -> list[dict]:
    def matches(row: dict) -> bool:
        for key, wanted in where.items():
            require(key in row, f'unknown fixture column {key}')
            if row[key] not in (wanted if isinstance(wanted, list) else [wanted]):
                return False
        return True
    return [row for row in rows if matches(row)]


def check_reference(case_id: str, reference: dict, tables: dict, top_m: float) -> int:
    require(reference.get('table') in tables, f'{case_id}: unknown oracle table')
    expect = reference.get('expect', {})
    require(set(expect) <= ALLOWED_EXPECT, f'{case_id}: expectation carries invented oracle values {sorted(set(expect) - ALLOWED_EXPECT)}')
    require('count' in expect, f'{case_id}: oracle reference must declare a row count')
    rows = select(tables[reference['table']], reference.get('where', {}))
    require(len(rows) == expect['count'], f'{case_id}: expected {expect["count"]} rows, found {len(rows)}')
    for row in rows:
        if 'nstop' in expect:
            require(bool(row['nstop']) == expect['nstop'], f'{case_id}: nstop classification disagrees with frozen row')
        if 'settling_nonzero' in expect:
            require((row['settling_m_s'] != 0.0) == expect['settling_nonzero'], f'{case_id}: settling classification disagrees')
        if 'f32_representable' in expect:
            require(TOP_OFFSET_COLUMN in row, f'{case_id}: representability needs a PUSH probe row')
            incoming = top_m + row[TOP_OFFSET_COLUMN] * TOP_OFFSET_M
            require(f32_representable(incoming) == expect['f32_representable'], f'{case_id}: representability disagrees')
    return len(rows)


def check_case(case: dict, anchors: set, matrix: dict, tables: dict, top_m: float) -> int:
    case_id = case.get('id', '?')
    for field in CASE_FIELDS:
        require(field in case, f'{case_id}: missing {field}')
    require(case['status'] in matrix['status_vocabulary'], f'{case_id}: unknown status {case["status"]}')
    require(case['gpu_classification'] in matrix['gpu_classifications'], f'{case_id}: unknown GPU classification')
    for anchor in case['original']['anchors']:
        require(anchor in anchors, f'{case_id}: unknown source anchor {anchor}')
    evidence = case['evidence']
    references = evidence['oracle_rows']
    if case['status'] == 'oracle_blocked':
        require(bool(evidence['oracle_gap']), f'{case_id}: oracle-blocked case must name the missing genuine rows')
    if case['status'] == 'supported_current':
        require(references and not case['blocked_by'], f'{case_id}: supported case needs genuine rows and no blocker')
    if case['status'] in ('oracle_blocked', 'producer_blocked'):
        require(bool(case['blocked_by']), f'{case_id}: blocked case must name its owner dependency')
    direction = case['input']['direction']
    final_stop = case['nstop']['final']
    if direction == 1:
        require(final_stop is False, f'{case_id}: forward transport can never set the top-boundary stop')
        require(case['gpu_classification'] != 'success_scientific_stop', f'{case_id}: forward cannot publish a stop')
    if case['gpu_classification'] == 'success_scientific_stop':
        require(direction == -1 and case['input']['lsettling'] is True, f'{case_id}: a stop needs backward transport and enabled lsettling')
        require('exclu' in case['downstream'], f'{case_id}: a stopped particle must be excluded downstream')
    if final_stop is True:
        require('exclu' in case['downstream'], f'{case_id}: a final stop must exclude the particle')
    if case['input']['lsettling'] is False:
        require(final_stop in (False, None), f'{case_id}: disabled settling cannot stop at the top')
    if case['gpu_classification'].startswith(('device_fatal', 'host_reject')):
        require(final_stop is not True, f'{case_id}: a fatal or rejected step publishes no stop')
    return sum(check_reference(case_id, reference, tables, top_m) for reference in references)


def check_owner(owner: dict) -> None:
    name = owner.get('component', '?')
    for field in OWNER_FIELDS:
        require(field in owner, f'owner {name}: missing {field}')
    for path_key, symbol_key in (('path', 'symbol'), ('gap_path', 'gap_symbol')):
        if path_key not in owner:
            continue
        path = ROOT / owner[path_key]
        require(path.is_file(), f'owner {name}: missing path {owner[path_key]}')
        require(owner[symbol_key] in path.read_text(encoding='utf-8'), f'owner {name}: symbol not found: {owner[symbol_key]}')


def git(checkout: Path, *arguments: str) -> str:
    return subprocess.check_output(['git', '-C', str(checkout), *arguments], text=True)


def check_sources(matrix: dict, checkout: Path) -> int:
    authority = matrix['authority']
    require(git(checkout, 'rev-parse', 'HEAD').strip() == authority['pinned_commit'], 'checkout is not the pinned revision')
    require(git(checkout, 'status', '--porcelain', '--untracked-files=no').strip() == '', 'pinned checkout has tracked modifications')
    advance = (checkout / 'src/advance_mod.f90').read_bytes()
    require(digest(advance.replace(b'\r\n', b'\n')) == authority['advance_mod_sha256'], 'advance_mod.f90 hash differs from #180')
    for anchor in matrix['source_anchors']:
        lines = (checkout / anchor['file']).read_text(encoding='utf-8', errors='replace').splitlines()
        first, last = anchor['lines']
        require(1 <= first <= last <= len(lines), f'{anchor["id"]}: line range outside file')
        window = '\n'.join(lines[first - 1:last])
        for token in anchor['tokens']:
            require(token in window, f'{anchor["id"]}: "{token}" not in {anchor["file"]}:{first}-{last}')
    return len(matrix['source_anchors'])


def check(matrix_path: Path = MATRIX, checkout: Path | None = None) -> dict:
    matrix = json.loads(matrix_path.read_text(encoding='utf-8'))
    require(matrix.get('schema') == {'id': SCHEMA_ID, 'version': SCHEMA_VERSION}, 'unexpected matrix schema')
    require(matrix.get('issue') == 203, 'matrix must belong to #203')
    require(matrix.get('validation_level') == 'architecture_contract_definition', 'unexpected validation level')
    require(matrix.get('claims') and not any(matrix['claims'].values()), 'a contract matrix must not claim parity, proof or tolerance')
    require((ROOT / matrix['decision_record']).is_file(), 'missing decision record')
    authority = matrix['authority']
    raw = (ROOT / authority['frozen_raw_output']).read_bytes().replace(b'\r\n', b'\n')
    require(digest(raw) == authority['frozen_raw_output_sha256'], 'frozen #180 raw output changed')
    fixture = json.loads((ROOT / authority['frozen_fixture']).read_text(encoding='utf-8'))
    require(fixture['provenance']['pinned_commit'] == authority['pinned_commit'], 'fixture pin differs')
    tables = fixture_tables(fixture)
    top_m = fixture['science']['target_top_m_agl']

    anchor_ids = [anchor['id'] for anchor in matrix['source_anchors']]
    require(len(anchor_ids) == len(set(anchor_ids)), 'duplicate source anchors')
    case_ids = [case.get('id') for case in matrix['cases']]
    require(len(case_ids) == len(set(case_ids)), 'duplicate case ids')
    missing = [case_id for case_id in REQUIRED_CASES if case_id not in case_ids]
    require(not missing, f'missing required cases {missing}')
    for case in matrix['cases']:
        require(case.get('required') is (case.get('id') in REQUIRED_CASES), f'{case.get("id")}: wrong required marker')
    referenced_rows = sum(check_case(case, set(anchor_ids), matrix, tables, top_m) for case in matrix['cases'])
    for owner in matrix['owners']:
        check_owner(owner)
    anchors_checked = check_sources(matrix, checkout) if checkout else 0
    return {
        'state': 'PASS' if checkout else 'PASS_SOURCE_ANCHORS_NOT_CHECKED',
        'schema_version': SCHEMA_VERSION,
        'cases': len(case_ids),
        'required_cases': len(REQUIRED_CASES),
        'referenced_oracle_rows': referenced_rows,
        'owners': len(matrix['owners']),
        'source_anchors_checked': anchors_checked,
        'scientific_verdict': 'CONTRACT_ONLY_NO_SETTLING_OR_TRANSPORT_PARITY',
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--matrix', type=Path, default=MATRIX)
    parser.add_argument('--checkout', type=Path, help='pristine pinned FLEXPART checkout for source-anchor checks')
    parser.add_argument('--allow-missing-checkout', action='store_true',
                        help='explicitly skip source anchors; reports PASS_SOURCE_ANCHORS_NOT_CHECKED')
    arguments = parser.parse_args()
    if arguments.checkout is None and not arguments.allow_missing_checkout:
        parser.error('--checkout is required unless --allow-missing-checkout is given')
    try:
        result = check(arguments.matrix, arguments.checkout)
    except (ValueError, KeyError, TypeError, OSError, subprocess.CalledProcessError) as error:
        print(json.dumps({'state': 'FAIL', 'error': str(error)}))
        return 1
    print(json.dumps(result))
    return 0


if __name__ == '__main__':
    sys.exit(main())
