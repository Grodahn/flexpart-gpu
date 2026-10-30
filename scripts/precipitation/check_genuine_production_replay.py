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
TEMPORAL_FIXTURE = ROOT / 'fixtures/interpolation/precipitation-temporal-v1.json'
TEMPORAL_FIXTURE_SHA256 = '8654e238cfe94293bb69e681eb4a4a29000bf83d46fe76a34bcaba62d19c8488'


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


def check_temporal(path, software=False, hardware=False, matched=None):
    """Extend the #219 auditor through #87/#119; recheck every original row.

    ``matched`` additionally requires a fresh original execution using the exact
    post-submission member bytes. Without it this audits the frozen independent
    original fixture only, never the fresh matched-input acceptance gate.
    """
    import temporal_oracle as original
    check_temporal_residency_sources()
    require(path.is_file(), 'missing temporal report')
    r = json.loads(path.read_text())
    fixture = json.loads(TEMPORAL_FIXTURE.read_text())
    require(r['schema'] == 'flexpart-gpu.precipitation-temporal-device.v1', 'temporal schema')
    revision = os.environ.get('FLEXPART_GPU_CANDIDATE_REVISION') or subprocess.check_output(
        ['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    require(r['candidate'] == {'revision': revision, 'worktree_clean': True}, 'stale temporal candidate')
    require(r['fixture_sha256'] == digest(TEMPORAL_FIXTURE) == TEMPORAL_FIXTURE_SHA256, 'original fixture identity')
    require(fixture['flexpart_revision'] == FLEXPART_PIN and fixture['flex_extract_revision'] == FLEX_EXTRACT_PIN,
            'temporal original pins')
    require(fixture['darain_oracle_sha256'] == r['darain_oracle_sha256'] == digest(ORACLE), 'member oracle')
    require(r['decision_216_sha256'] == digest(DECISION), '#216 decision')
    decision = json.loads(DECISION.read_text())
    require(dict(r['grib_files_sha256']) == {k: v['sha256'] for k, v in decision['consumed']['grib_files'].items()},
            'temporal GRIB identities')
    shaders = {**SHADERS, 'horizontal': ROOT / 'src/shaders/horizontal_interpolation.wgsl',
               'precipitation_temporal': ROOT / 'src/shaders/precipitation_temporal.wgsl'}
    require(r['shaders'] == {k: digest(p) for k, p in shaders.items()}, 'temporal shader hashes')
    for name, sha in fixture['harness_sha256'].items():
        require(digest(ROOT / 'scripts/interpolation' / name) == sha, 'modified original harness')
    require(r['encoders'] == r['submissions'] == 1 and r['host_transfers_between_stages'] == 0,
            'temporal residency')
    require(r['stage_order'] == ['#90 LSP', '#90 CP', '#207 LSP', '#207 CP', '#87 selected planes', '#119'], 'temporal stages')
    adapter = r['adapter']
    require(adapter['name'] and adapter['backend'] in BACKENDS, 'temporal adapter')
    adapter_class = 'software_wgsl' if adapter['device_type'] == 'cpu' else 'hardware_gpu'
    require(adapter['adapter_class'] == adapter_class, 'temporal adapter classification')
    require(not software or adapter_class == 'software_wgsl', 'temporal software execution')
    require(not hardware or adapter_class == 'hardware_gpu', 'temporal hardware execution')
    require(r['status'] == 0 and r['verdict'] == 'passed', 'fatal temporal status/verdict')
    require(r['unit'] == r['metadata']['unit'] == 'millimeter_per_hour', 'temporal units')
    require(r['comparison_policy'] == {'absolute': ABS_TOL, 'relative': REL_TOL}, 'temporal policy')
    sources = {'temporal': 'src/gpu/precipitation_temporal.rs', 'test': 'tests/precipitation_temporal_gpu.rs',
               'member': 'src/gpu/precipitation_members.rs', 'spatial': 'src/gpu/horizontal.rs',
               'rate': 'src/gpu/meteorology/interval_rates.rs'}
    require(r['source_sha256'] == {k: digest(ROOT / p) for k, p in sources.items()}, 'temporal source identity')
    evidence = r['gpu_evidence']
    require(evidence['schema'] == {'id': 'flexpart-gpu.gpu-execution-evidence', 'version': 1}
            and evidence['candidate']['revision'] == revision
            and evidence['candidate']['shader_sha256'] == r['shaders']['precipitation_temporal'], '#91 temporal candidate')
    require(evidence['execution'] == {'status': 'passed', 'calculation_path': 'wgsl_device', 'adapter': adapter,
                                     'failure': None, 'skip_reason': None}, '#91 temporal execution')
    require(evidence['comparison']['verdict'] == 'passed' and evidence['comparison']['compared_value_count'] == 1176
            and evidence['comparison']['policy'] == {'absolute_tolerance': ABS_TOL, 'relative_tolerance': REL_TOL},
            '#91 temporal comparison')
    require(evidence['oracle']['revision'] == FLEXPART_PIN
            and evidence['oracle']['executable_sha256'] == fixture['executable_sha256']
            and evidence['oracle']['output_sha256'] == digest(TEMPORAL_FIXTURE), '#91 original identity')
    require(json.loads(r['input_manifest']) == r['metadata'] and
            hashlib.sha256(r['input_manifest'].encode()).hexdigest() == evidence['candidate']['input_sha256'], 'temporal input manifest')
    metadata = r['metadata']['members']
    require(metadata['calendar'] == 'gregorian' and metadata['cell_count'] == 9 and
            metadata['horizontal_grid'] == {'nx': 3, 'ny': 3, 'dx_deg': 1.0, 'dy_deg': 1.0,
                'xlon0_deg': 10.0, 'ylat0_deg': 50.0, 'longitude_domain': 'minus180_to180'}, 'genuine grid/calendar')
    require(metadata['gpu_adapter'] == adapter, 'member adapter identity')
    require(metadata['preparation'] == {'rrint': 0, 'purefc': 0, 'basetime_hour': None, 'numpf': 1, 'method': 'darain'}
            and metadata['representation'] == 'flex_extract-7.1.2-rrint0-darain'
            and metadata['unit'] == 'millimeter_per_hour', 'point-member preparation')
    admissions = metadata['large_scale']['quantization_admissions'] + metadata['convective']['quantization_admissions']
    check_admissions({'admissions_216': admissions}, decision)
    genuine = json.loads(ORACLE.read_text())['genuine']
    times = [m['member_time_s'] + EPOCH_UNIX_S for m in genuine['darain_members']['lsp']]
    require([m['valid_time_epoch_seconds'] for m in metadata['members']] == times, 'complete temporal member timestamps')
    require(metadata['delta_seconds'] == 10800 and metadata['sequence_start_epoch_seconds'] == EPOCH_UNIX_S,
            'time reference')
    intervals = metadata['intervals']
    require(len(intervals) == 16, 'complete temporal interval support')
    for k, interval in enumerate(intervals):
        require(interval['start_epoch_seconds'] == EPOCH_UNIX_S + k*10800 and
                interval['end_epoch_seconds'] == EPOCH_UNIX_S + (k+1)*10800 and interval['duration_seconds'] == 10800,
                'temporal interval identity')
    for slot, m in enumerate(metadata['members']):
        require(m['slot'] == slot and m['support_intervals'] == intervals[slot:slot+4]
                and m['support_start_epoch_seconds'] == times[slot]-21600
                and m['support_end_epoch_seconds'] == times[slot]+21600, 'temporal four-interval support')
    require(len(metadata['source_snapshot_sha256']) == 16, 'complete source identity')
    for p, field in PARAMETERS:
        source = metadata['large_scale' if p == 'lsp' else 'convective']
        require(source['field_id'] == field and source['source_unit'] == 'kilogram_per_square_meter'
                and len(source['source_times']) == 16, 'distinct source rate identity')
        for time, observation in zip(source['source_times'], genuine['accumulated'][p]):
            require(time['calendar'] == 'gregorian' and time['kind'] == 'accumulated_since_reset'
                    and time['valid_time_epoch_seconds'] == observation['valid_s'] + EPOCH_UNIX_S
                    and time['interval_start_epoch_seconds'] == observation['reset_s'] + EPOCH_UNIX_S
                    and time['accumulation']['reset_epoch_seconds'] == observation['reset_s'] + EPOCH_UNIX_S,
                    'source observation time')
        planes, bits = r['member_values_mm_per_h'][p], r['member_bits'][p]
        require(len(planes) == len(bits) == 13, 'complete member values')
        for plane, raw, oracle_member in zip(planes, bits, genuine['darain_members'][p]):
            require(len(plane) == len(raw) == 9, 'complete member plane')
            for v, b, expected in zip(plane, raw, oracle_member['result_mm_per_h']):
                require(f32_from_bits(b) == v and within_policy(v, expected), 'genuine reconstructed member')
    expected_cases = list(original.cases())
    require(len(fixture['cases']) == len(expected_cases) == 14, 'original pair set')
    for c, expected in zip(fixture['cases'], expected_cases):
        for key in ('id', 'slots', 'memtime_s', 'planes', 'queries', 'nx', 'ny'):
            require(c[key] == expected[key], 'modified original inputs: '+key)
        require(c['raw_input'] == original.driver_input(expected) and
                hashlib.sha256(c['raw_input'].encode()).hexdigest() == c['input_sha256'], 'original input bytes')
        require(hashlib.sha256(c['raw_output'].encode()).hexdigest() == c['output_sha256'] and
                original.parse_output(c['raw_output']) == c['rows'], 'original output bytes')
    golden = fixture['cases'][-1]['rows']
    require([[row['lsp'], row['cp']] for row in golden] ==
            [[9.75, 1.5], [84.75, 2.25], [159.75, 3.0], [234.75, 3.75], [309.75, 4.5]], 'frozen arithmetic control')
    pairs = r['metadata']['memory_pairs']
    queries = r['metadata']['queries']
    rows = r['rows']
    expected_rows = [(c, row) for c in fixture['cases'][:-1] for row in c['rows']]
    require(len(rows) == len(pairs) == len(queries) == len(expected_rows) == 588, 'complete temporal comparisons')
    for index, (row, query, pair, (case, oracle_row)) in enumerate(zip(rows, queries, pairs, expected_rows)):
        x, y, t = oracle_row['query']
        require(query == row['query'] == {'xt': x, 'yt': y, 'itime_epoch_seconds': t + EPOCH_UNIX_S}
                and row['index'] == index and row['case'] == case['id'], 'query identity')
        left, right = case['slots']
        require(pair == {'left': metadata['members'][left], 'right': metadata['members'][right]}, 'selected memory pair')
        require(row['original'] == [oracle_row['lsp'], oracle_row['cp']], 'temporal original outputs')
        require(len(row['candidate']) == len(row['candidate_bits']) == 2, 'both precipitation fields')
        for c, b, o, difference in zip(row['candidate'], row['candidate_bits'], row['original'], row['absolute_differences']):
            require(f32_from_bits(b) == c and within_policy(c, o) and difference == abs(c-o), 'temporal mismatch')
        require(row['verdict'] == 'passed', 'row verdict')
    if matched is not None:
        fresh = json.loads(matched.read_text())
        require(fresh['device_report_sha256'] == digest(path) and fresh['candidate'] == r['candidate'], 'fresh candidate attribution')
        check_matched_temporal(r, fixture, fresh)
    return r


def check_temporal_residency_sources():
    """Extend #76's bounded no-host-completion audit to the actual called stages."""
    functions = {
        'src/gpu/precipitation_temporal.rs': ["pub fn encode<'s>", 'fn encode_time('],
        'src/gpu/precipitation_members.rs': ["pub fn encode<'s>", 'fn encode_darain('],
        'src/gpu/meteorology/interval_rates.rs': ['pub fn encode_interval_rates('],
        'src/gpu/horizontal.rs': ['pub fn encode_horizontal_samples('],
        'src/gpu/accumulation.rs': ['pub fn encode_accumulated_intervals_gpu_with_kernel('],
    }
    for path, names in functions.items():
        source = (ROOT / path).read_text()
        for name in names:
            start = source.index('{', source.index(name))
            level = 1
            end = start + 1
            while level:
                require(end < len(source), 'missing encode body')
                level += (source[end] == '{') - (source[end] == '}')
                end += 1
            body = source[start:end]
            for forbidden in ('.submit(', '.poll(', 'block_on(', 'download_', '.download(', 'map_async(', '.write_buffer('):
                require(forbidden not in body, 'intermediate host completion in '+path+': '+forbidden)


def check_matched_temporal(report, fixture, matched):
    """Require unchanged original execution with exactly matched input member bits."""
    import temporal_oracle as original
    for key in ('schema', 'units', 'darain_oracle_sha256', 'flexpart_revision', 'flex_extract_revision', 'source_sha256', 'harness_sha256', 'compiler'):
        require(matched[key] == fixture[key], 'fresh original identity '+key)
    require(len(matched['executable_sha256']) == 64, 'fresh executable hash')
    expected_cases = list(original.cases(report))
    require(len(matched['cases']) == len(expected_cases), 'matched pair count')
    outputs = []
    for case, expected in zip(matched['cases'], expected_cases):
        require(case['raw_input'] == original.driver_input(expected), 'matched original member bytes')
        for key in ('id', 'slots', 'memtime_s', 'planes', 'queries', 'nx', 'ny'):
            require(case[key] == expected[key], 'matched input '+key)
        require(hashlib.sha256(case['raw_input'].encode()).hexdigest() == case['input_sha256'] and
                hashlib.sha256(case['raw_output'].encode()).hexdigest() == case['output_sha256'], 'matched raw hashes')
        require(case['rows'] == original.parse_output(case['raw_output']) and
                [r['query'] for r in case['rows']] == case['queries'], 'matched output queries')
        if case['id'] != 'arithmetic-control':
            outputs.extend([row['lsp'], row['cp']] for row in case['rows'])
    require(len(outputs) == len(report['rows']) == 588, 'matched output count')
    for candidate, oracle in zip(report['rows'], outputs):
        require(all(within_policy(c, o) for c, o in zip(candidate['candidate'], oracle)), 'fresh matched original mismatch')


def check_temporal_both(directory):
    """Require hardware and software plus both fresh exact-input original runs."""
    hardware = check_temporal(directory / 'hardware_gpu.json', hardware=True,
                              matched=directory / 'matched-hardware.json')
    software = check_temporal(directory / 'software_wgsl.json', software=True,
                              matched=directory / 'matched-software.json')
    require(hardware['candidate'] == software['candidate'], 'same temporal candidate on both adapters')
    return hardware, software


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('report', type=Path, nargs='?')
    parser.add_argument('--software', action='store_true')
    parser.add_argument('--hardware', action='store_true')
    parser.add_argument('--both', type=Path, metavar='DIR')
    parser.add_argument('--temporal', action='store_true')
    parser.add_argument('--matched', type=Path)
    args = parser.parse_args()
    if args.temporal and args.both:
        check_temporal_both(args.both)
        print('#119 temporal sampling: hardware and software PASS, fresh matched original (1176 outputs each)')
    elif args.temporal:
        require(args.report is not None, 'temporal report required')
        check_temporal(args.report, args.software, args.hardware, args.matched)
        print('#119 temporal sampling: PASS (588 queries, 1176 original comparisons' +
              (', fresh matched inputs)' if args.matched else ', frozen original fixture)'))
    elif args.both:
        check_both(args.both)
        print('#219 genuine replay: hardware and software PASS (18/18 series, 234/234 members)')
    else:
        require(args.report is not None, 'report path required')
        check(args.report, args.software, args.hardware)
        print('#219 genuine replay: PASS (18/18 series, 234/234 members, 3 admissions)')
