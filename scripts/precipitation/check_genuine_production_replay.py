#!/usr/bin/env python3
"""Audit the genuine #202 -> #90 -> #207 production replay evidence (#219).

Recomputes every comparison from the committed fixtures instead of trusting the
report: the #202 GRIB file hashes, the frozen #216 decision (admitted rows and
their exact bounds), the pinned darain oracle (``max(deac_vals, 0)`` interval
rates and all 234 members), the shader hashes and the candidate revision. It
fails closed on a missing or stale report, a wrong adapter class, an admitted
interval that is not exactly +0.0, a -0.0 anywhere, any #71 comparison failure,
or a composition that is not one encoder and one submission.

    python scripts/precipitation/check_genuine_production_replay.py REPORT [--software|--hardware]
    python scripts/precipitation/check_genuine_production_replay.py --both DIR
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[2]
DECISION = ROOT / 'fixtures/accumulation/grib-packing-decrease-decision-v1.json'
ORACLE = ROOT / 'fixtures/accumulation/precipitation-darain-oracle-v1.json'
GRIB_DIR = ROOT / 'fixtures/accumulation/precipitation-202-accumulated-grib'
SHADERS = {'accumulated_interval': ROOT / 'src/shaders/accumulated_interval.wgsl',
           'darain': ROOT / 'src/shaders/darain_precipitation_member.wgsl'}
FLEX_EXTRACT_PIN = 'e0005c99ac81d12faa45a8ff799debbd592b0dc0'
FLEXPART_PIN = 'c70586c2b7f5258850705325881c61f557ea9bd8'
EPOCH_UNIX_S = 1_698_796_800  # 2023-11-01T00:00:00Z, the oracle epoch
ABS_TOL, REL_TOL = 1e-6, 1e-5  # the existing #71 policy; no new tolerance
PARAMETERS = (('lsp', 'large_scale_precipitation'), ('cp', 'convective_precipitation'))
STAGES = [{'accumulated_transform': 'large_scale_precipitation'},
          {'accumulated_transform': 'convective_precipitation'},
          {'darain_reconstruction': 'large_scale_precipitation'},
          {'darain_reconstruction': 'convective_precipitation'}]
BACKENDS = ('vulkan', 'dx12', 'metal', 'gl', 'browser_webgpu')
REPORT_NAMES = {'hardware': 'genuine-production-replay-hardware_gpu.json',
                'software': 'genuine-production-replay-software_wgsl.json'}


def require(condition, message):
    if not condition:
        raise ValueError('genuine production replay: ' + message)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def f32_from_bits(text):
    require(isinstance(text, str) and len(text) == 8, 'f32 bit pattern')
    return struct.unpack('<f', bytes.fromhex(text)[::-1])[0]


def within_policy(candidate, oracle):
    return (math.isfinite(candidate) and math.isfinite(oracle)
            and abs(candidate - oracle) <= ABS_TOL + REL_TOL * abs(oracle))


def admitted_rows(decision):
    rows = {}
    for row in decision['offending_rows']:
        key = (row['parameter'], row['canonical_cell'], row['current']['observation_index'])
        rows[key] = row
    require(len(rows) == 3, 'frozen #216 rows')
    return rows


def check_identity(report, decision, oracle_sha):
    require(report['schema'] == {'id': 'flexpart-gpu.genuine-production-replay-219', 'version': 1},
            'schema')
    require(report['issue'] == 219 and report['closes_gate_of'] == 207, 'issue identity')
    revision = os.environ.get('FLEXPART_GPU_CANDIDATE_REVISION') or subprocess.check_output(
        ['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    require(report['candidate'] == {'revision': revision, 'worktree_clean': True},
            'stale or uncommitted candidate revision')
    require(report['references'] == {'flex_extract': FLEX_EXTRACT_PIN, 'flexpart': FLEXPART_PIN},
            'original reference pins')
    require(decision['references']['flex_extract']['pinned_commit'] == FLEX_EXTRACT_PIN
            and decision['references']['flexpart']['pinned_commit'] == FLEXPART_PIN, 'decision pins')
    require(report['decision'] == {'schema': decision['schema'], 'sha256': digest(DECISION),
                                   'verdict': 'ADMIT_BOUNDED'}, '#216 decision identity')
    files = {p.name[:-len('.grib')]: digest(p) for p in sorted(GRIB_DIR.glob('*.grib'))}
    require(len(files) == 4 and files == {k: v['sha256'] for k, v in
            decision['consumed']['grib_files'].items()}, 'committed #202 GRIB identity')
    inputs = report['inputs']
    require(inputs['grib_files_sha256'] == files, '#202 input hashes')
    require(inputs['darain_oracle_sha256'] == oracle_sha ==
            decision['consumed']['darain_oracle_sha256'], 'darain oracle identity')
    require(len(inputs['source_snapshot_sha256']) == 16 and all(
        len(h) == 64 for h in inputs['source_snapshot_sha256']), 'source snapshot identities')
    require(inputs['horizontal_grid'] == report['metadata']['horizontal_grid'], 'grid identity')
    require(report['shaders'] == {name: digest(path) for name, path in SHADERS.items()},
            'shader identity')


def check_execution(report, software, hardware):
    adapter = report['adapter']
    require(adapter['name'] and adapter['backend'] in BACKENDS, 'adapter identity')
    expected_class = 'software_wgsl' if adapter['device_type'] == 'cpu' else 'hardware_gpu'
    require(adapter['adapter_class'] == expected_class, 'adapter class')
    require(not software or expected_class == 'software_wgsl', 'required software WGSL adapter')
    require(not hardware or expected_class == 'hardware_gpu', 'required hardware GPU adapter')
    require(report['metadata']['gpu_adapter'] == adapter, 'member metadata adapter')
    require(report['stages'] == STAGES and report['encoders'] == 1 and report['submissions'] == 1
            and report['host_transfers_between_stages'] == 0, 'one encoder, one submission')


def check_admissions(report, decision):
    frozen = admitted_rows(decision)
    admissions = report['admissions_216']
    require(len(admissions) == 3, 'three #216 admissions')
    seen = set()
    for admission in admissions:
        name = 'lsp' if admission['field_id'] == 'large_scale_precipitation' else 'cp'
        key = (name, admission['cell_index'], admission['interval_index'])
        require(key in frozen and key not in seen, 'admission identity')
        seen.add(key)
        row = frozen[key]
        require(admission['decision'] == decision['schema'] and
                admission['verdict'] == 'ADMIT_BOUNDED' and
                admission['code'] == 'quantization_compatible_decrease', 'admission verdict')
        require(admission['bound_exact'] == row['bounds']['canonical_kg_per_m2']['exact'], 'exact B')
        require(admission['signed_difference_exact'] ==
                row['signed_difference']['canonical_kg_per_m2']['exact'], 'exact d')
        require(admission['signed_difference_kg_per_square_meter'] < 0 and
                -admission['signed_difference_kg_per_square_meter'] <
                admission['bound_kg_per_square_meter'], 'strict bound')
        require(admission['interval_end_epoch_seconds'] == row['interval_end_s'] + EPOCH_UNIX_S,
                'admission interval')
        for side in ('previous', 'current'):
            source = admission[side + '_source']
            require(source['source_message_sha256'] == source['packing']['message_sha256'] ==
                    row[side]['message_sha256'], 'admission message identity')
            require(source['packing']['mars_class'] == 'OD' and
                    source['packing']['encoding'] == 'grib1_grid_simple', 'admission class')
    return frozen


def check_lanes(report, oracle, frozen):
    genuine = oracle['genuine']
    lanes = report['lanes']
    require([lane['parameter'] for lane in lanes] == ['lsp', 'cp'], 'lane identity')
    series = members = 0
    for (name, field_id), lane in zip(PARAMETERS, lanes):
        means = genuine['interval_means'][name]['values_mm_per_h']
        rows = lane['interval_rates_90']
        require(len(rows) == 16 * 9 and {(r['cell'], r['interval']) for r in rows} ==
                {(c, k) for c in range(9) for k in range(16)}, '#90 rows complete')
        for row in rows:
            key = (name, row['cell'], row['interval'])
            deac = means[row['interval']][row['cell']]
            expected = max(deac, 0.0)
            rate = f32_from_bits(row['gpu_rate_bits'])
            require(row['gpu_rate_bits'] != '80000000', 'negative zero rate')
            require(row['admitted_216'] == (key in frozen), 'admission flag')
            if key in frozen:
                require(deac < 0 and row['gpu_rate_bits'] == '00000000', 'admitted interval is +0.0')
            require(row['flex_extract_deac_vals_mm_per_h'] == deac and
                    row['expected_mm_per_h'] == expected, 'oracle interval value')
            require(within_policy(rate, expected) and row['verdict'] == 'passed', '#90 rate mismatch')
        series += 9
        admissions = [a for a in lane['admissions_216'] if a['field_id'] == field_id]
        require(len(admissions) == len(lane['admissions_216']) ==
                sum(1 for key in frozen if key[0] == name), 'lane admissions')
        require(report['metadata'][('large_scale' if name == 'lsp' else 'convective')]
                ['quantization_admissions'] == lane['admissions_216'], 'member metadata admissions')
        oracle_members = genuine['darain_members'][name]
        expected_values = [v for m in oracle_members for v in m['result_mm_per_h']]
        member_rows = lane['members']['rows']
        require(len(expected_values) == len(member_rows) == 117, 'member rows complete')
        for index, (row, expected) in enumerate(zip(member_rows, expected_values)):
            require(row['index'] == index and row['oracle'] == expected, 'member oracle value')
            require(within_policy(row['gpu'], expected) and row['verdict'] == 'passed',
                    'darain member mismatch')
        members += len(member_rows)
        require(lane['members']['verdict'] == 'passed', 'lane verdict')
        evidence = lane['members']['gpu_evidence']
        require(evidence['execution']['calculation_path'] == 'wgsl_device' and
                evidence['execution']['status'] == 'passed' and
                evidence['execution']['adapter'] == report['adapter'], 'lane device execution')
        require(evidence['comparison']['verdict'] == 'passed' and
                evidence['comparison']['compared_value_count'] == 117, 'lane paired verdict')
    times = [m['valid_time_epoch_seconds'] for m in report['metadata']['members']]
    require(times == [m['member_time_s'] + EPOCH_UNIX_S for m in genuine['darain_members']['lsp']],
            'exact member timestamps')
    for member in report['metadata']['members']:
        t = member['valid_time_epoch_seconds']
        require(member['support_start_epoch_seconds'] == t - 21_600 and
                member['support_end_epoch_seconds'] == t + 21_600 and
                len(member['support_intervals']) == 4, 'four-interval support')
    require(len(report['metadata']['intervals']) == 16, 'no missing interval')
    require(report['counts'] == {'interval_series': series, 'interval_series_required': 18,
                                 'darain_members': members, 'darain_members_required': 234,
                                 'admissions': 3} and (series, members) == (18, 234), 'counts')


def check(path, software=False, hardware=False):
    """Validate one report; raise ValueError on any incomplete or failed evidence."""
    require(path.is_file(), 'missing report ' + str(path))
    report = json.loads(path.read_text(encoding='utf-8'))
    decision = json.loads(DECISION.read_text(encoding='utf-8'))
    oracle = json.loads(ORACLE.read_text(encoding='utf-8'))
    check_identity(report, decision, digest(ORACLE))
    check_execution(report, software, hardware)
    frozen = check_admissions(report, decision)
    check_lanes(report, oracle, frozen)
    require(report['verdict'] == 'passed', 'verdict')
    return report


def check_both(directory):
    """Require passing hardware and software evidence for the same revision."""
    hardware = check(directory / REPORT_NAMES['hardware'], hardware=True)
    software = check(directory / REPORT_NAMES['software'], software=True)
    require(hardware['candidate'] == software['candidate'], 'same candidate revision')
    return hardware, software


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('report', type=Path, nargs='?')
    parser.add_argument('--software', action='store_true')
    parser.add_argument('--hardware', action='store_true')
    parser.add_argument('--both', type=Path, metavar='DIR')
    args = parser.parse_args()
    if args.both:
        check_both(args.both)
        print('#219 genuine replay: hardware and software PASS (18/18 series, 234/234 members)')
    else:
        require(args.report is not None, 'report path required')
        check(args.report, args.software, args.hardware)
        print('#219 genuine replay: PASS (18/18 series, 234/234 members, 3 admissions)')
