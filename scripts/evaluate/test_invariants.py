"""Positive, adversarial and mutation tests for the #54 deterministic-invariant gate.

Run from the repository root with:

    python scripts/evaluate/test_invariants.py

Every expectation is constructed independently of the evaluator: analytic
displacements come from the case wind and duration, mass budgets from
explicitly chosen kilogram ledgers, and FLEXPART oracle outputs are written
here as Fortran unformatted records with known cell values.
"""

import copy
import json
import math
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(REPO / "scripts" / "provenance"))
sys.path.insert(0, str(REPO / "scripts" / "corpus"))

import evaluate_case
import invariants as inv
import metrics
import run_provenance as provenance

CASES = REPO / "fixtures" / "corpus" / "cases"
CONTRACT = inv.load_contract()
ORACLE_MANIFEST = evaluate_case.load_oracle_manifest(str(evaluate_case.DEFAULT_ORACLE_MANIFEST))
PINNED = ORACLE_MANIFEST["pinned_commit"]

# ADV-ANA-001 analytic expectation, independent of any evaluator code:
# 10 m/s eastward for 3600 s from (10E, 50N, 100 m).
ADV_LON0, ADV_LAT0, ADV_Z0 = 10.0, 50.0, 100.0
ADV_EAST_M = 10.0 * 3600.0
M_PER_DEG_LAT = 6_371_000.0 * math.pi / 180.0
M_PER_DEG_LON = M_PER_DEG_LAT * math.cos(math.radians(ADV_LAT0))


# ---------------------------------------------------------------------------
# Independent fixture builders
# ---------------------------------------------------------------------------

def adv_case(particles=4):
    """ADV-ANA-001 with a small particle count and a 2x2x2 output grid."""
    case = json.loads((CASES / "ADV-ANA-001.json").read_text(encoding="utf-8"))
    case["release"]["particle_count"] = particles
    case["release"]["mass_kg_per_particle"] = 1
    case["release"]["inventory"]["quantity_kg"] = particles
    case["output_grid"].update({"nx": 2, "ny": 2, "nz": 2, "heights_m": [100, 250]})
    return case


def dry_case():
    case = json.loads((CASES / "DRY-007.json").read_text(encoding="utf-8"))
    case["release"]["inventory"]["quantity_kg"] = 1
    return case


def stochastic_case(case_id="STOCH-001", count=3):
    case = json.loads((CASES / "WIND-UNI-002.json").read_text(encoding="utf-8"))
    case["case_id"] = case_id
    case["release"]["particle_count"] = 4
    case["release"]["mass_kg_per_particle"] = 0.25
    case["release"]["inventory"]["quantity_kg"] = 1
    case["stochastic"]["candidate_philox"] = {
        "base_key": [1, 2], "base_counter": [0, 0, 0, 0], "count": count,
        "derivation": "wrapping_add_key0_v1"}
    return case


def adv_positions(count, east_offsets=None, z_offsets=None):
    east_offsets = east_offsets or [0.0] * count
    z_offsets = z_offsets or [0.0] * count
    return {"lon_deg": [ADV_LON0 + (ADV_EAST_M + e) / M_PER_DEG_LON for e in east_offsets],
            "lat_deg": [ADV_LAT0] * count,
            "z_m": [ADV_Z0 + dz for dz in z_offsets]}


def particle_ev(case_id="ADV-ANA-001", masses=None, positions=None, reservoirs=None,
                count=None, active=None, initial=None, seed_index=0, rid="candidate:seed_000.json"):
    masses = masses if masses is not None else [1.0] * 4
    positions = positions or adv_positions(len(masses))
    particles = dict(positions, mass_kg=list(masses))
    return inv.particle_evidence(
        "candidate", rid, case_id=case_id, particles=particles,
        particle_count=len(masses) if count is None else count,
        active_particles=len(masses) if active is None else active,
        declared_initial_mass_kg=initial,
        reservoirs={"dry_deposited_kg": 0.0, "wet_deposited_kg": 0.0}
        if reservoirs is None else reservoirs,
        seed_index=seed_index,
        artifacts=[{"label": rid.split(":")[1], "path": "unused", "sha256": "0" * 64}])


def gridded_ev(case_id="ADV-ANA-001", conc=None, dry=None, wet=None, grid=None, chronology=None):
    epoch = 1704067200  # 2024-01-01T00:00:00Z
    conc = conc if conc is not None else [1.0] * 8
    return inv.gridded_evidence(
        "oracle", "oracle:out", case_id=case_id,
        fields=[{"field_id": "grid_conc_A/concentration", "kind": "concentration",
                 "unit": "native", "values": conc, "expected_cells": 8},
                {"field_id": "grid_conc_A/dry_deposition", "kind": "dry_deposition",
                 "unit": "native", "values": dry if dry is not None else [0.0] * 4,
                 "expected_cells": 4},
                {"field_id": "grid_conc_A/wet_deposition", "kind": "wet_deposition",
                 "unit": "native", "values": wet if wet is not None else [0.0] * 4,
                 "expected_cells": 4}],
        grid=grid or {"nx": 2, "ny": 2, "nz": 2, "xlon0": 6.0, "ylat0": 47.0,
                      "dx": 0.1, "dy": 0.1, "heights_m": [100.0, 250.0]},
        chronology=chronology or {"start_epoch_s": epoch, "interval_s": 1800,
                                  "averaging_s": 1800, "sampling_s": 300,
                                  "timestamps_epoch_s": [epoch + 1800, epoch + 3600]},
        artifacts=[{"label": "oracle_header", "path": "unused", "sha256": "0" * 64}])


def record(realization, invariant_id):
    for item in realization["invariants"]:
        if item["invariant_id"] == invariant_id:
            return item
    raise AssertionError(f"{invariant_id} missing")


def evaluate(case, evidence, provenance_ctx=None):
    return inv.evaluate_realization(CONTRACT, case, evidence, provenance_ctx)


def _rec(stream, payload):
    stream.write(struct.pack("<i", len(payload)))
    stream.write(payload)
    stream.write(struct.pack("<i", len(payload)))


def _sparse(flat, offset=0):
    """FLEXPART sparse runs: start index per run, run values with alternating sign."""
    indices, values, sign, i = [], [], 1.0, 0
    while i < len(flat):
        if flat[i] == 0:
            i += 1
            continue
        indices.append(offset + i)
        while i < len(flat) and flat[i] != 0:
            values.append(sign * abs(flat[i]))
            i += 1
        sign = -sign
    return indices, values


def _write_sparse(stream, flat, offset=0):
    indices, values = _sparse(flat, offset)
    _rec(stream, struct.pack("<i", len(indices)))
    _rec(stream, struct.pack(f"<{len(indices)}i", *indices))
    _rec(stream, struct.pack("<i", len(values)))
    _rec(stream, struct.pack(f"<{len(values)}f", *values))


def write_flexpart_output(directory, *, nx=2, ny=2, nz=2, heights=(100.0, 250.0),
                          loutstep=1800, loutaver=1800, loutsample=300,
                          stamps=("20240101003000", "20240101010000"), dry=None):
    """Write a binary FLEXPART header plus grid_conc files with known values."""
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    with open(directory / "header", "wb") as stream:
        _rec(stream, struct.pack("<2i", 20240101, 0))
        _rec(stream, struct.pack("<3i", loutstep, loutaver, loutsample))
        _rec(stream, struct.pack("<2f2i2f", 6.0, 47.0, nx, ny, 0.1, 0.1))
        _rec(stream, struct.pack("<i", nz) + struct.pack(f"<{nz}f", *heights[:nz]))
        _rec(stream, struct.pack("<2i", 20240101, 0))
        _rec(stream, struct.pack("<2i", 3, 1))
        for _ in range(3):
            _rec(stream, b"spec")
        _rec(stream, struct.pack("<i", 1))
        for _ in range(7):
            _rec(stream, b"pnt_")
        _rec(stream, b"meth")
        _rec(stream, struct.pack("<i", 1))
    surface = nx * ny
    for k, stamp in enumerate(stamps, start=1):
        with open(directory / f"grid_conc_{stamp}_001", "wb") as stream:
            _rec(stream, struct.pack("<i", 1800 * k))
            _write_sparse(stream, [0.0] * surface)
            _write_sparse(stream, dry if dry is not None else [0.0] * surface)
            _write_sparse(stream, [2.0] * (surface * nz), offset=surface)
    return directory


def write_seed(path, case_id, seed_index, positions, masses, *, key=(0, 0), revision="rev-a",
               reservoirs=None, particle_count=None, initial_mass=None):
    """Seed file with a runner metrics block derived from the written particles."""
    lons, lats, zs = positions["lon_deg"], positions["lat_deg"], positions["z_m"]
    n = len(masses)
    uniform = [1.0] * n
    com = metrics.center_of_mass_particles(lons, lats, zs, uniform)
    cov = metrics.horizontal_covariance(lons, lats, uniform)
    quantiles = metrics.unweighted_quantiles_linear(zs, (0.1, 0.5, 0.9))["quantiles"]
    mean_z = sum(zs) / n
    std_z = math.sqrt(sum((z - mean_z) ** 2 for z in zs) / n)
    eig = sorted(v * 1e6 for v in cov["eigenvalues_km2"])
    data = {
        "case_id": case_id, "seed_index": seed_index, "philox_key": list(key),
        "philox_counter": [0, 0, 0, 0], "adapter": "test-adapter", "is_software_adapter": True,
        "candidate_revision": revision,
        "particle_count": n if particle_count is None else particle_count,
        "active_particles": n,
        "particles": [{"lon_deg": lo, "lat_deg": la, "z_m": z, "mass_kg": m}
                      for lo, la, z, m in zip(lons, lats, zs, masses)],
        "metrics": {
            "total_mass_kg": sum(masses),
            "initial_mass_kg": float(n) if initial_mass is None else initial_mass,
            "mass_conservation_rel_error": 0.0,
            "com_lon_deg": com["lon_deg"], "com_lat_deg": com["lat_deg"], "com_z_m": com["z_m"],
            "cov_east_m2": (cov["sigma_east_km"] * 1000.0) ** 2,
            "cov_north_m2": (cov["sigma_north_km"] * 1000.0) ** 2,
            "cov_z_m2": std_z ** 2, "cov_east_north_m2": 0.0, "cov_east_z_m2": 0.0,
            "cov_north_z_m2": 0.0, "horizontal_eigenvalues_m2": eig,
            "z_min_m": min(zs), "z_p10_m": quantiles["0.1"], "z_p50_m": quantiles["0.5"],
            "z_p90_m": quantiles["0.9"], "z_max_m": max(zs), "z_mean_m": mean_z,
            "z_std_m": std_z},
    }
    for key_name, value in (reservoirs if reservoirs is not None
                            else {"deposited_dry_kg": 0.0, "deposited_wet_kg": 0.0}).items():
        data[key_name] = value
    Path(path).write_text(json.dumps(data), encoding="utf-8")
    return Path(path)


def write_case(path, case):
    Path(path).write_text(json.dumps(case), encoding="utf-8")
    return Path(path)


def args_for(case_path, seeds, **overrides):
    values = {"case_def": str(case_path), "candidate_seeds": [str(s) for s in seeds],
              "fortran_output": None, "require_input_equivalence": None,
              "invariants": str(inv.DEFAULT_CONTRACT)}
    values.update(overrides)
    import argparse
    defaults = {
        "thresholds": str(evaluate_case.DEFAULT_THRESHOLDS),
        "oracle_manifest": str(evaluate_case.DEFAULT_ORACLE_MANIFEST),
        "run_manifest": None, "input_equivalence_report": None, "oracle_checkout": None,
        "oracle_executable": None, "oracle_seed": None, "oracle_seed_controllable": False,
        "candidate_log": None, "candidate_executable": None, "candidate_revision": None,
        "seed": None, "candidate_seed_controllable": False, "note": []}
    defaults.update(values)
    return argparse.Namespace(**defaults)


def v1_manifest(root, case_path, candidate_outputs, oracle_outputs=(), *, seed_indices=None):
    """Authoritative #53 manifest binding each candidate file to its own execution."""
    case = provenance.case_manifest_identity(case_path)
    build = provenance.candidate_build_identity("rev-a", "a" * 64)
    common = dict(candidate_revision="rev-a", candidate_build=build,
                  candidate_executable_sha256="a" * 64, oracle_kind="pristine-oracle",
                  oracle_revision=PINNED, oracle_executable_sha256="b" * 64,
                  oracle_profile={"id": provenance.ORACLE_PROFILE_ID, "version": 1})
    executions = []
    for index, path in enumerate(candidate_outputs):
        seed_index = index if seed_indices is None else seed_indices[index]
        executions.append(provenance.build_execution_record(
            role="candidate", case=case, realization={"seed_index": seed_index},
            runtime_adapter="test-adapter", inputs_sha256={"COMMAND": "c" * 64},
            outputs_sha256={Path(path).name: provenance.digest(path)}, **common))
    if oracle_outputs:
        executions.append(provenance.build_execution_record(
            role="oracle", case=case, realization={}, cpu_runtime="flexpart-11.1-single-thread",
            inputs_sha256={"COMMAND": "c" * 64},
            outputs_sha256={Path(p).name: provenance.digest(p) for p in oracle_outputs},
            **common))
    manifest = provenance.create_run_manifest(
        cases=[case],
        candidate={"revision": "rev-a", "worktree_dirty": False,
                   "executable_sha256": "a" * 64, "build": build},
        oracle={"kind": "pristine-oracle", "pinned_commit": PINNED, "worktree_dirty": False,
                "executable_sha256": "b" * 64,
                "execution_profile": {"id": provenance.ORACLE_PROFILE_ID, "version": 1}},
        runtime={"adapter": "test-adapter"}, executions=executions, base=str(root))
    path = Path(root) / "run_manifest.json"
    path.write_text(json.dumps(manifest), encoding="utf-8")
    return path


def input_equivalence_report(root, verdict="INPUT_EQUIVALENT"):
    import input_equivalence as ie
    case = json.loads((CASES / "ADV-ANA-001.json").read_text(encoding="utf-8"))
    report = ie.build_report("ADV-ANA-001", case, REPO / "fixtures" / "corpus" / "fortran"
                             / "ADV-ANA-001")
    if verdict != "INPUT_EQUIVALENT":
        report["verdict"] = verdict
        report["fields"][0]["status"] = {"INPUT_MISMATCH": "mismatch",
                                         "INPUT_EQUIVALENCE_NOT_DEMONSTRATED":
                                         "not_demonstrated"}[verdict]
    path = Path(root) / f"input-equivalence-{verdict}.json"
    path.write_text(json.dumps(report), encoding="utf-8")
    return path


# ---------------------------------------------------------------------------
# Contract and aggregation semantics
# ---------------------------------------------------------------------------

class ContractTest(unittest.TestCase):
    def test_contract_identity_and_threshold_references(self):
        self.assertEqual(CONTRACT["contract_version"], 1)
        self.assertEqual(set(CONTRACT["_by_id"]), set(inv.IMPLEMENTED))
        closure = CONTRACT["_resolved"]["INV-MASS-CLOSURE"]
        self.assertEqual((closure["id"], closure["value"], closure["operator"]),
                         ("MASS_BUDGET_CLOSE", 1e-05, "abs_lt"))
        analytic = CONTRACT["_resolved"]["INV-ANALYTIC-UNIFORM-ADVECTION"]
        self.assertEqual({k: v["value"] for k, v in analytic.items()},
                         {"east_m": 360.0, "north_m": 1.0, "vertical_m": 0.01})

    def _mutated(self, mutate):
        document = json.loads(inv.DEFAULT_CONTRACT.read_text(encoding="utf-8"))
        mutate(document)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "contract.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaises(inv.InvariantContractError) as ctx:
                inv.load_contract(path)
        return str(ctx.exception)

    def test_mutations_fail_closed(self):
        mutations = {
            "unimplemented": lambda d: d["invariants"].pop(),
            "keys differ": lambda d: d["invariants"][0].__setitem__("hidden", 1),
            "version": lambda d: d["invariants"][0].__setitem__("version", 2),
            "missing from": lambda d: d["invariants"][2]["comparison"]["threshold"]
            .__setitem__("criterion", "NO_SUCH_GATE"),
            "precedence": lambda d: d["aggregation"].__setitem__(
                "precedence", ["FAIL", "INTEGRITY_ERROR", "BLOCKED", "PASS", "NOT_APPLICABLE"]),
            "only FAIL": lambda d: d["error_ids"].__setitem__("MASS_DEFICIT", "PASS"),
            "duplicate": lambda d: d["invariants"].append(copy.deepcopy(d["invariants"][0])),
            "report schema": lambda d: d.__setitem__("report_schema_version", "1.0.0"),
        }
        for expected, mutate in mutations.items():
            with self.subTest(expected=expected):
                self.assertIn(expected, self._mutated(mutate))

    def test_contract_identity_ignores_line_endings(self):
        raw = inv.DEFAULT_CONTRACT.read_bytes().replace(b"\r\n", b"\n")
        with tempfile.TemporaryDirectory() as directory:
            lf = Path(directory) / "lf.json"
            crlf = Path(directory) / "crlf.json"
            lf.write_bytes(raw)
            crlf.write_bytes(raw.replace(b"\n", b"\r\n"))
            self.assertEqual(inv.load_contract(lf)["_sha256"], inv.load_contract(crlf)["_sha256"])
            self.assertEqual(inv.load_contract(lf)["_sha256"], CONTRACT["_sha256"])

    def test_threshold_operator_drift_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for source in ("evaluation/thresholds/scientific-thresholds-v1.json",
                           "fixtures/corpus/thresholds.json"):
                target = root / source
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes((REPO / source).read_bytes())
            thresholds = root / "evaluation/thresholds/scientific-thresholds-v1.json"
            body = json.loads(thresholds.read_text(encoding="utf-8"))
            for criterion in body["criteria"]:
                if criterion["id"] == "MASS_BUDGET_CLOSE":
                    criterion["operator"] = "abs_le"
            thresholds.write_text(json.dumps(body), encoding="utf-8")
            with self.assertRaisesRegex(inv.InvariantContractError, "operator abs_lt"):
                inv.load_contract(inv.DEFAULT_CONTRACT, repo_root=root)

    def test_module_is_importable_without_evaluate_case(self):
        code = ("import sys; sys.path.insert(0, sys.argv[1]); import invariants; "
                "print(invariants.provenance_context({'schema': {}}, None, [])['mode'])")
        completed = subprocess.run([sys.executable, "-c", code, str(HERE)],
                                   capture_output=True, text=True)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stdout.strip(), "legacy")

    def test_emitting_undeclared_error_class_is_rejected(self):
        ctx = inv._Context(CONTRACT, None, None)
        with self.assertRaises(inv.InvariantContractError):
            inv._record(ctx, "INV-MASS-CLOSURE", inv.FAIL, error_id="MISSING_RESERVOIR")
        with self.assertRaises(inv.InvariantContractError):
            inv._record(ctx, "INV-MASS-CLOSURE", inv.PASS, error_id="MASS_DEFICIT")


class AggregationTest(unittest.TestCase):
    def test_precedence(self):
        P, F, I, N, B = inv.PASS, inv.FAIL, inv.INTEGRITY_ERROR, inv.NOT_APPLICABLE, inv.BLOCKED
        self.assertEqual(inv.aggregate_verdicts([P, F, P]), F)
        self.assertEqual(inv.aggregate_verdicts([P, B, F, I]), I)
        self.assertEqual(inv.aggregate_verdicts([P, N, B]), B)
        self.assertEqual(inv.aggregate_verdicts([P, N]), P)
        self.assertEqual(inv.aggregate_verdicts([N, N]), N)
        self.assertEqual(inv.aggregate_verdicts([]), B)
        self.assertEqual(inv.aggregate_verdicts([P, "OK"]), I)

    def test_report_mutations_fail_closed(self):
        good = evaluate(adv_case(), particle_ev())
        self.assertEqual(inv.aggregate_realizations([None])["verdict"], inv.BLOCKED)
        self.assertEqual(inv.aggregate_realizations([{"unexpected": 1}])["verdict"], inv.BLOCKED)
        emptied = dict(good, invariants=[])
        self.assertEqual(inv.aggregate_realizations([emptied])["verdict"], inv.BLOCKED)
        forged = copy.deepcopy(good)
        forged["invariants"][0]["verdict"] = "OK"
        result = inv.aggregate_realizations([forged])
        self.assertEqual(result["verdict"], inv.INTEGRITY_ERROR)
        self.assertEqual(result["integrity_errors"][-1]["error_id"], "EVALUATION_INTEGRITY_ERROR")


# ---------------------------------------------------------------------------
# Positive cases
# ---------------------------------------------------------------------------

class PositiveTest(unittest.TestCase):
    def test_finite_nonnegative_fields_pass(self):
        result = evaluate(adv_case(), particle_ev())
        self.assertEqual(record(result, "INV-FIELD-FINITE")["verdict"], inv.PASS)
        self.assertEqual(record(result, "INV-FIELD-NONNEGATIVE")["verdict"], inv.PASS)
        self.assertEqual(record(result, "INV-FIELD-FINITE")["cells_evaluated"], 6)

    def test_disabled_process_exact_zero_pass(self):
        candidate = evaluate(adv_case(), particle_ev())
        oracle = evaluate(adv_case(), gridded_ev())
        for result in (candidate, oracle):
            for invariant in ("INV-DISABLED-DRY-DEPOSITION-ZERO",
                              "INV-DISABLED-WET-DEPOSITION-ZERO"):
                item = record(result, invariant)
                self.assertEqual((item["verdict"], item["evidence_mode"]),
                                 (inv.PASS, "exact_ledger"))
        decay = record(candidate, "INV-DISABLED-DECAY-NO-LOSS")
        self.assertEqual((decay["verdict"], decay["evidence_mode"]), (inv.PASS, "closure_bound"))

    def test_mass_closure_with_complete_reservoirs_pass(self):
        result = evaluate(dry_case(), particle_ev(
            case_id="DRY-007", masses=[0.175] * 4,
            reservoirs={"dry_deposited_kg": 0.3, "wet_deposited_kg": 0.0}))
        closure = record(result, "INV-MASS-CLOSURE")
        self.assertEqual(closure["verdict"], inv.PASS)
        self.assertEqual(closure["identity"], "released = airborne + dry_deposited_kg")
        self.assertEqual(closure["excluded_disabled_reservoirs"], {"wet_deposited_kg": 0.0})
        self.assertAlmostEqual(closure["measured"]["accounted_mass_kg"], 1.0)
        self.assertEqual(record(result, "INV-DISABLED-WET-DEPOSITION-ZERO")["verdict"], inv.PASS)
        self.assertEqual(record(result, "INV-DISABLED-DRY-DEPOSITION-ZERO")["verdict"],
                         inv.NOT_APPLICABLE)

    def test_deterministic_analytic_fixture_pass(self):
        result = evaluate(adv_case(), particle_ev(
            positions=adv_positions(4, east_offsets=[0.0, 359.0, -359.0, 0.0])))
        item = record(result, "INV-ANALYTIC-UNIFORM-ADVECTION")
        self.assertEqual(item["verdict"], inv.PASS)
        self.assertEqual(item["expected"], {"east_m": 36000.0, "north_m": 0.0, "vertical_m": 0.0})
        self.assertAlmostEqual(item["worst_particle"]["east_m"]["residual_m"], 359.0, places=3)

    def test_valid_output_grid_and_window_pass(self):
        result = evaluate(adv_case(), gridded_ev())
        self.assertEqual(record(result, "INV-OUTPUT-GRID")["verdict"], inv.PASS)
        self.assertEqual(record(result, "INV-OUTPUT-CHRONOLOGY")["verdict"], inv.PASS)

    def test_explicit_not_applicable(self):
        result = evaluate(adv_case(), particle_ev())
        self.assertEqual(record(result, "INV-ANALYTIC-DECAY")["verdict"], inv.NOT_APPLICABLE)
        turbulent = evaluate(stochastic_case(), particle_ev(case_id="STOCH-001",
                                                            masses=[0.25] * 4))
        item = record(turbulent, "INV-ANALYTIC-UNIFORM-ADVECTION")
        self.assertEqual(item["verdict"], inv.NOT_APPLICABLE)
        self.assertIn("physics_switches.turbulence", item["applicability"]["reason"])
        backward = adv_case()
        backward["simulation_direction"] = "backward"
        self.assertEqual(record(evaluate(backward, particle_ev()), "INV-MASS-CLOSURE")["verdict"],
                         inv.NOT_APPLICABLE)

    def test_missing_case_is_blocked_not_not_applicable(self):
        result = evaluate(None, particle_ev())
        for invariant in ("INV-MASS-CLOSURE", "INV-ANALYTIC-UNIFORM-ADVECTION",
                          "INV-DISABLED-DRY-DEPOSITION-ZERO", "INV-OUTPUT-UNITS"):
            item = record(result, invariant)
            self.assertEqual((item["verdict"], item["error_id"]),
                             (inv.BLOCKED, "CASE_MANIFEST_UNAVAILABLE"), invariant)

    def test_deterministic_repeat_pass_and_fail(self):
        case = stochastic_case(count=2)
        case["stochastic"]["candidate_philox"]["derivation"] = "reuse_base_identity_v1"
        first = particle_ev(case_id="STOCH-001", masses=[0.25] * 4, rid="candidate:seed_000.json")
        second = particle_ev(case_id="STOCH-001", masses=[0.25] * 4, rid="candidate:seed_001.json",
                             seed_index=1)
        gate = inv.build_gate(CONTRACT, case, [first, second])
        self.assertEqual(gate["ensemble"][0]["verdict"], inv.PASS)
        second["particles"]["z_m"][2] += 1e-9
        gate = inv.build_gate(CONTRACT, case, [first, second])
        repeat = gate["ensemble"][0]
        self.assertEqual((repeat["verdict"], repeat["error_id"]), (inv.FAIL, "REPEAT_NOT_IDENTICAL"))
        self.assertEqual(repeat["offenders"][0]["particle"], 2)
        self.assertEqual(gate["verdict"], inv.FAIL)

    def test_candidate_only_unary_check_makes_no_paired_claim(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case_path = write_case(root / "ADV-ANA-001.json", adv_case())
            seed = write_seed(root / "seed_000.json", "ADV-ANA-001", 0, adv_positions(4), [1.0] * 4)
            built = evaluate_case.run_corpus_seeds(args_for(case_path, [seed]), ORACLE_MANIFEST)
        gate = built["deterministic_invariants"]
        realization = gate["realizations"][0]
        unary = [r for r in realization["invariants"]
                 if r["invariant_id"] not in ("INV-PROVENANCE-BINDING", "INV-OUTPUT-GRID",
                                              "INV-OUTPUT-CHRONOLOGY")]
        self.assertTrue(all(r["verdict"] in (inv.PASS, inv.NOT_APPLICABLE) for r in unary))
        self.assertEqual(gate["paired_scoring"]["status"], "NOT_REQUESTED")
        self.assertEqual(gate["verdict"], inv.BLOCKED)
        self.assertEqual(built["overall_status"], "DIAGNOSTIC")
        self.assertEqual(built["schema_version"], "1.2.0")
        self.assertFalse(built["distributional_metrics"]["eligible_for_paired_scoring"])
        self.assertEqual(built["particle_metrics"]["per_seed"][0]["mass_budget"]["verdict"],
                         inv.PASS)


# ---------------------------------------------------------------------------
# Negative cases: fields, closure, disabled physics, analytic processes
# ---------------------------------------------------------------------------

class FieldNegativeTest(unittest.TestCase):
    def test_nan_concentration_is_integrity_error_with_cell(self):
        conc = [1.0] * 8
        conc[5] = float("nan")
        result = evaluate(adv_case(), gridded_ev(conc=conc))
        item = record(result, "INV-FIELD-FINITE")
        self.assertEqual((item["verdict"], item["error_id"]), (inv.INTEGRITY_ERROR, "NONFINITE_VALUE"))
        self.assertEqual(item["offenders"][0],
                         {"field": "grid_conc_A/concentration", "cell": 5, "unit": "native",
                          "value": "NaN"})
        self.assertEqual(record(result, "INV-FIELD-NONNEGATIVE")["error_id"],
                         "UPSTREAM_FIELD_INTEGRITY")
        self.assertEqual(result["verdict"], inv.INTEGRITY_ERROR)
        json.dumps(result, allow_nan=False)

    def test_infinite_mass_is_integrity_error(self):
        result = evaluate(adv_case(), particle_ev(masses=[1.0, float("inf"), 1.0, 1.0]))
        item = record(result, "INV-FIELD-FINITE")
        self.assertEqual((item["error_id"], item["offenders"][0]["value"]),
                         ("NONFINITE_VALUE", "Infinity"))
        self.assertEqual(record(result, "INV-MASS-CLOSURE")["error_id"], "UPSTREAM_FIELD_INTEGRITY")

    def test_negative_airborne_mass(self):
        result = evaluate(adv_case(), particle_ev(masses=[1.0, -0.5, 1.0, 1.0]))
        item = record(result, "INV-FIELD-NONNEGATIVE")
        self.assertEqual((item["verdict"], item["error_id"]), (inv.INTEGRITY_ERROR, "NEGATIVE_VALUE"))
        self.assertEqual((item["offenders"][0]["field"], item["offenders"][0]["cell"]),
                         ("particles/mass_kg", 1))

    def test_negative_deposited_mass(self):
        result = evaluate(dry_case(), particle_ev(
            case_id="DRY-007", masses=[0.25] * 4,
            reservoirs={"dry_deposited_kg": -0.01, "wet_deposited_kg": 0.0}))
        item = record(result, "INV-FIELD-NONNEGATIVE")
        self.assertEqual(item["error_id"], "NEGATIVE_VALUE")
        self.assertEqual(item["offenders"][0]["field"], "reservoir/dry_deposited_kg")

    def test_negative_concentration_in_one_cell(self):
        conc = [1.0] * 8
        conc[3] = -1e-30
        item = record(evaluate(adv_case(), gridded_ev(conc=conc)), "INV-FIELD-NONNEGATIVE")
        self.assertEqual((item["error_id"], item["offender_count"], item["offenders"][0]["cell"]),
                         ("NEGATIVE_VALUE", 1, 3))

    def test_malformed_field_shape(self):
        result = evaluate(adv_case(), gridded_ev(conc=[1.0] * 7))
        self.assertEqual(record(result, "INV-FIELD-FINITE")["error_id"], "MALFORMED_FIELD")


class ClosureNegativeTest(unittest.TestCase):
    def test_silent_mass_disappearance(self):
        result = evaluate(adv_case(), particle_ev(masses=[1.0, 1.0, 1.0, 0.9]))
        closure = record(result, "INV-MASS-CLOSURE")
        self.assertEqual((closure["verdict"], closure["error_id"]), (inv.FAIL, "MASS_DEFICIT"))
        self.assertAlmostEqual(closure["residual_kg"], -0.1)
        decay = record(result, "INV-DISABLED-DECAY-NO-LOSS")
        self.assertEqual((decay["verdict"], decay["error_id"]), (inv.FAIL, "UNEXPLAINED_MASS_LOSS"))

    def test_mass_double_counting(self):
        result = evaluate(dry_case(), particle_ev(
            case_id="DRY-007", masses=[0.25] * 4,
            reservoirs={"dry_deposited_kg": 0.3, "wet_deposited_kg": 0.0}))
        closure = record(result, "INV-MASS-CLOSURE")
        self.assertEqual((closure["verdict"], closure["error_id"]), (inv.FAIL, "MASS_EXCESS"))
        self.assertAlmostEqual(closure["residual_relative"], 0.3)

    def test_missing_required_reservoir(self):
        result = evaluate(dry_case(), particle_ev(case_id="DRY-007", masses=[0.25] * 4,
                                                  reservoirs={"wet_deposited_kg": 0.0}))
        closure = record(result, "INV-MASS-CLOSURE")
        self.assertEqual((closure["verdict"], closure["error_id"], closure["missing_reservoir"]),
                         (inv.BLOCKED, "MISSING_RESERVOIR", "dry_deposited_kg"))

    def test_terminated_particles_are_not_deposition(self):
        result = evaluate(adv_case(), particle_ev(masses=[1.0] * 3, count=4, active=3))
        self.assertEqual(record(result, "INV-MASS-CLOSURE")["error_id"],
                         "MISSING_TERMINATION_RESERVOIR")

    def test_initial_mass_contradicting_case(self):
        result = evaluate(adv_case(), particle_ev(initial=5.0))
        self.assertEqual(record(result, "INV-MASS-CLOSURE")["error_id"],
                         "INITIAL_MASS_CONTRADICTS_CASE")

    def test_oracle_without_kg_ledger_is_blocked(self):
        item = record(evaluate(adv_case(), gridded_ev()), "INV-MASS-CLOSURE")
        self.assertEqual((item["verdict"], item["error_id"]), (inv.BLOCKED, "MASS_LEDGER_UNAVAILABLE"))


class DisabledAndAnalyticNegativeTest(unittest.TestCase):
    def test_disabled_deposition_with_nonzero_deposit(self):
        candidate = evaluate(adv_case(), particle_ev(
            masses=[1.0, 1.0, 1.0, 0.9],
            reservoirs={"dry_deposited_kg": 0.1, "wet_deposited_kg": 0.0}))
        item = record(candidate, "INV-DISABLED-DRY-DEPOSITION-ZERO")
        self.assertEqual((item["verdict"], item["error_id"]),
                         (inv.FAIL, "DISABLED_PROCESS_NONZERO_RESERVOIR"))
        # The identity omits the disabled term, so the removed mass is a deficit.
        self.assertEqual(record(candidate, "INV-MASS-CLOSURE")["error_id"], "MASS_DEFICIT")
        oracle = evaluate(adv_case(), gridded_ev(dry=[0.0, 0.0, 3.5, 0.0]))
        item = record(oracle, "INV-DISABLED-DRY-DEPOSITION-ZERO")
        self.assertEqual((item["error_id"], item["offenders"][0]["cell"]),
                         ("DISABLED_PROCESS_NONZERO_RESERVOIR", 2))

    def test_incorrect_analytic_advection(self):
        result = evaluate(adv_case(), particle_ev(
            positions=adv_positions(4, east_offsets=[0.0, 0.0, 400.0, 0.0])))
        item = record(result, "INV-ANALYTIC-UNIFORM-ADVECTION")
        self.assertEqual((item["verdict"], item["error_id"]),
                         (inv.FAIL, "ANALYTIC_DISPLACEMENT_EXCEEDED"))
        self.assertEqual((item["offenders"][0]["component"], item["offenders"][0]["particle"]),
                         ("east_m", 2))
        vertical = evaluate(adv_case(), particle_ev(
            positions=adv_positions(4, z_offsets=[0.0, 0.02, 0.0, 0.0])))
        self.assertEqual(record(vertical, "INV-ANALYTIC-UNIFORM-ADVECTION")["offenders"][0]
                         ["component"], "vertical_m")

    def test_non_numeric_position_is_malformed(self):
        positions = adv_positions(4)
        positions["z_m"][1] = None
        item = record(evaluate(adv_case(), particle_ev(positions=positions)),
                      "INV-ANALYTIC-UNIFORM-ADVECTION")
        self.assertEqual((item["verdict"], item["error_id"]),
                         (inv.INTEGRITY_ERROR, "MALFORMED_FIELD"))
        positions["z_m"][1] = float("nan")
        item = record(evaluate(adv_case(), particle_ev(positions=positions)),
                      "INV-ANALYTIC-UNIFORM-ADVECTION")
        self.assertEqual(item["error_id"], "NONFINITE_VALUE")

    def test_analytic_case_outside_threshold_scope_is_blocked(self):
        case = adv_case()
        case["case_id"] = "ADV-ANA-OTHER"
        item = record(evaluate(case, particle_ev(case_id="ADV-ANA-OTHER")),
                      "INV-ANALYTIC-UNIFORM-ADVECTION")
        self.assertEqual((item["verdict"], item["error_id"]), (inv.BLOCKED, "NO_VERSIONED_THRESHOLD"))

    def test_incorrect_deterministic_decay(self):
        # Decay-disabled control: any decay ledger must be exactly zero.
        result = evaluate(adv_case(), particle_ev(
            masses=[1.0, 1.0, 1.0, 0.99],
            reservoirs={"dry_deposited_kg": 0.0, "wet_deposited_kg": 0.0, "decayed_kg": 0.01}))
        item = record(result, "INV-DISABLED-DECAY-NO-LOSS")
        self.assertEqual((item["verdict"], item["error_id"]),
                         (inv.FAIL, "DISABLED_PROCESS_NONZERO_RESERVOIR"))
        # Decay enabled: no half-life binding exists, so never PASS.
        case = adv_case()
        case["physics_switches"]["decay"] = True
        enabled = evaluate(case, particle_ev(
            reservoirs={"dry_deposited_kg": 0.0, "wet_deposited_kg": 0.0, "decayed_kg": 0.5},
            masses=[1.0, 1.0, 1.0, 0.5]))
        item = record(enabled, "INV-ANALYTIC-DECAY")
        self.assertEqual((item["verdict"], item["error_id"]),
                         (inv.BLOCKED, "DECAY_SEMANTICS_UNAVAILABLE"))
        self.assertEqual(record(enabled, "INV-MASS-CLOSURE")["identity"],
                         "released = airborne + decayed_kg")


class OutputNegativeTest(unittest.TestCase):
    def test_mismatched_output_window(self):
        epoch = 1704067200
        bad_average = gridded_ev(chronology={"start_epoch_s": epoch, "interval_s": 1800,
                                             "averaging_s": 3600, "sampling_s": 300,
                                             "timestamps_epoch_s": [epoch + 1800, epoch + 3600]})
        item = record(evaluate(adv_case(), bad_average), "INV-OUTPUT-CHRONOLOGY")
        self.assertEqual((item["verdict"], item["error_id"]),
                         (inv.INTEGRITY_ERROR, "OUTPUT_WINDOW_MISMATCH"))
        self.assertEqual([m["field"] for m in item["offenders"]], ["averaging_s"])
        missing_window = gridded_ev(chronology={"start_epoch_s": epoch, "interval_s": 1800,
                                                "averaging_s": 1800, "sampling_s": 300,
                                                "timestamps_epoch_s": [epoch + 1800]})
        item = record(evaluate(adv_case(), missing_window), "INV-OUTPUT-CHRONOLOGY")
        self.assertEqual([m["field"] for m in item["offenders"]], ["timestamps_epoch_s"])

    def test_grid_shape_mismatch(self):
        grid = {"nx": 3, "ny": 2, "nz": 2, "xlon0": 6.0, "ylat0": 47.0, "dx": 0.1, "dy": 0.1,
                "heights_m": [100.0, 250.0]}
        item = record(evaluate(adv_case(), gridded_ev(grid=grid)), "INV-OUTPUT-GRID")
        self.assertEqual((item["verdict"], item["error_id"]),
                         (inv.INTEGRITY_ERROR, "GRID_SHAPE_MISMATCH"))
        grid = dict(grid, nx=2, xlon0=6.5)
        item = record(evaluate(adv_case(), gridded_ev(grid=grid)), "INV-OUTPUT-GRID")
        self.assertEqual(item["error_id"], "OUTPUT_GRID_MISMATCH")

    def test_wrong_physical_units(self):
        case = adv_case()
        case["units"]["mass"] = "g"
        item = record(evaluate(case, particle_ev()), "INV-OUTPUT-UNITS")
        self.assertEqual((item["verdict"], item["error_id"]), (inv.INTEGRITY_ERROR, "UNIT_MISMATCH"))

    def test_wrong_case_manifest(self):
        item = record(evaluate(adv_case(), particle_ev(case_id="WIND-UNI-002")),
                      "INV-CASE-IDENTITY")
        self.assertEqual((item["verdict"], item["error_id"]), (inv.INTEGRITY_ERROR, "CASE_ID_MISMATCH"))

    def test_particle_count_inconsistent(self):
        item = record(evaluate(adv_case(), particle_ev(count=5, active=5)), "INV-PARTICLE-COUNT")
        self.assertEqual(item["error_id"], "PARTICLE_COUNT_INCONSISTENT")


# ---------------------------------------------------------------------------
# Per-realization failure preservation
# ---------------------------------------------------------------------------

class RealizationPreservationTest(unittest.TestCase):
    def _three_seeds(self, case):
        evidences = []
        for index, masses in enumerate(([0.25] * 4, [0.25, 0.25, 0.25, 0.2], [0.25] * 4)):
            evidences.append(particle_ev(case_id=case["case_id"], masses=masses,
                                         rid=f"candidate:seed_{index:03d}.json",
                                         seed_index=index))
        return evidences

    def test_failed_realization_surrounded_by_passing_ones(self):
        case = stochastic_case()
        gate = inv.build_gate(CONTRACT, case, self._three_seeds(case))
        closures = [record(r, "INV-MASS-CLOSURE")["verdict"] for r in gate["realizations"]]
        self.assertEqual(closures, [inv.PASS, inv.FAIL, inv.PASS])
        self.assertEqual(gate["verdict"], inv.FAIL)
        self.assertEqual([(f["realization_id"], f["invariant_id"], f["error_id"])
                          for f in gate["failed"]],
                         [("candidate:seed_001.json", "INV-MASS-CLOSURE", "MASS_DEFICIT"),
                          ("candidate:seed_001.json", "INV-DISABLED-DECAY-NO-LOSS",
                           "UNEXPLAINED_MASS_LOSS")])
        # Separately evaluated realizations aggregate to the same FAIL.
        blocks = [inv.build_gate(CONTRACT, case, [e]) for e in self._three_seeds(case)]
        self.assertEqual(inv.aggregate_realizations(blocks)["verdict"], inv.FAIL)

    def test_stochastic_agreement_never_overrides_negative_mass(self):
        case = stochastic_case()
        evidences = self._three_seeds(case)
        evidences[1] = particle_ev(case_id=case["case_id"], masses=[0.25, 0.25, 0.5, 0.0],
                                   rid="candidate:seed_001.json", seed_index=1)
        evidences[1]["particles"]["mass_kg"][3] = -1e-12
        gate = inv.build_gate(CONTRACT, case, evidences)
        self.assertEqual(gate["verdict"], inv.INTEGRITY_ERROR)
        self.assertEqual(gate["integrity_errors"][0]["realization_id"], "candidate:seed_001.json")

    def test_cli_seed_and_aggregate_reports_preserve_failure(self):
        case = stochastic_case()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case_path = write_case(root / "STOCH-001.json", case)
            positions = {"lon_deg": [10.0, 10.01, 10.02, 10.03], "lat_deg": [50.0] * 4,
                         "z_m": [50.0, 51.0, 52.0, 53.0]}
            seeds = []
            for index, masses in enumerate(([0.25] * 4, [0.25, 0.25, 0.25, 0.2], [0.25] * 4)):
                seeds.append(write_seed(root / f"seed_{index:03d}.json", "STOCH-001", index,
                                        positions, masses, key=(1 + index, 2),
                                        initial_mass=1.0))
            built = evaluate_case.run_corpus_seeds(args_for(case_path, seeds), ORACLE_MANIFEST)
            self.assertEqual(built["overall_status"], "FAIL")
            self.assertEqual(built["multiseed"]["mass_budget_gate"]["failed_seeds"],
                             ["seed_001.json"])
            # One report per seed, then the existing aggregate-seeds path.
            report_paths = []
            for seed in seeds:
                single = evaluate_case.run_corpus_seeds(args_for(case_path, [seed]),
                                                        ORACLE_MANIFEST)
                path = root / f"report_{seed.stem}.json"
                path.write_text(json.dumps(single), encoding="utf-8")
                report_paths.append(str(path))
            aggregate = evaluate_case.run_aggregate(
                args_for(case_path, [], seed_reports=report_paths), ORACLE_MANIFEST)
        self.assertEqual(aggregate["overall_status"], "FAIL")
        gate = aggregate["deterministic_invariants"]
        self.assertEqual(gate["verdict"], inv.FAIL)
        self.assertEqual({f["realization_id"] for f in gate["failed"]}, {"candidate:seed_001.json"})
        self.assertEqual(gate["realization_count"], 3)


# ---------------------------------------------------------------------------
# Oracle artifacts, #52 and #53 integration through the evaluator
# ---------------------------------------------------------------------------

class OracleArtifactTest(unittest.TestCase):
    def _run(self, root, oracle_dir, **overrides):
        case_path = write_case(root / "ADV-ANA-001.json", adv_case())
        seed = write_seed(root / "seed_000.json", "ADV-ANA-001", 0, adv_positions(4), [1.0] * 4)
        return evaluate_case.run_corpus_seeds(
            args_for(case_path, [seed], fortran_output=str(oracle_dir), **overrides),
            ORACLE_MANIFEST)

    def test_valid_binary_oracle_grid_window_and_deposition(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            built = self._run(root, write_flexpart_output(root / "oracle"))
        oracle = [r for r in built["deterministic_invariants"]["realizations"]
                  if r["role"] == "oracle"][0]
        for invariant in ("INV-FIELD-FINITE", "INV-FIELD-NONNEGATIVE", "INV-OUTPUT-GRID",
                          "INV-OUTPUT-CHRONOLOGY", "INV-DISABLED-DRY-DEPOSITION-ZERO",
                          "INV-DISABLED-WET-DEPOSITION-ZERO"):
            self.assertEqual(record(oracle, invariant)["verdict"], inv.PASS, invariant)
        self.assertEqual(record(oracle, "INV-FIELD-FINITE")["cells_evaluated"], 2 * (8 + 4 + 4))
        self.assertEqual(built["deterministic_invariants"]["paired_scoring"]["status"], "BLOCKED")

    def test_oracle_window_and_grid_mismatch_fail_closed(self):
        cases = {"OUTPUT_WINDOW_MISMATCH": {"loutaver": 3600},
                 "GRID_SHAPE_MISMATCH": {"nx": 3}}
        for error_id, overrides in cases.items():
            with self.subTest(error_id=error_id), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with self.assertRaises(inv.InvariantGateError) as ctx:
                    self._run(root, write_flexpart_output(root / "oracle", **overrides))
                self.assertEqual(ctx.exception.error_id, error_id)
                self.assertEqual(ctx.exception.block["verdict"], inv.INTEGRITY_ERROR)

    def test_oracle_dry_deposit_with_disabled_dry_deposition(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            built = self._run(root, write_flexpart_output(root / "oracle",
                                                          dry=[0.0, 4.0, 0.0, 0.0]))
        self.assertEqual(built["overall_status"], "FAIL")
        failed = built["deterministic_invariants"]["failed"]
        self.assertEqual([(f["realization_id"], f["invariant_id"], f["error_id"]) for f in failed],
                         [("oracle:oracle", "INV-DISABLED-DRY-DEPOSITION-ZERO",
                           "DISABLED_PROCESS_NONZERO_RESERVOIR")])
        oracle = [r for r in built["deterministic_invariants"]["realizations"]
                  if r["role"] == "oracle"][0]
        item = record(oracle, "INV-DISABLED-DRY-DEPOSITION-ZERO")
        self.assertEqual([(o["field"], o["cell"]) for o in item["offenders"]],
                         [("grid_conc_20240101003000_001/dry_deposition", 1),
                          ("grid_conc_20240101010000_001/dry_deposition", 1)])

    def test_corrupted_oracle_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            oracle = write_flexpart_output(root / "oracle")
            target = oracle / "grid_conc_20240101010000_001"
            target.write_bytes(target.read_bytes()[:-6])
            with self.assertRaises(inv.InvariantGateError) as ctx:
                self._run(root, oracle)
        self.assertEqual(ctx.exception.error_id, "ORACLE_OUTPUT_INCOMPLETE")

    def test_non_equivalent_inputs_refuse_paired_scoring(self):
        for verdict in ("INPUT_MISMATCH", "INPUT_EQUIVALENCE_NOT_DEMONSTRATED"):
            with self.subTest(verdict=verdict), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with self.assertRaises(inv.InvariantGateError) as ctx:
                    self._run(root, write_flexpart_output(root / "oracle"),
                              require_input_equivalence=str(
                                  input_equivalence_report(root, verdict)))
                self.assertEqual(ctx.exception.error_id, verdict)

    def test_equivalent_inputs_without_provenance_stay_blocked(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            built = self._run(root, write_flexpart_output(root / "oracle"),
                              require_input_equivalence=str(input_equivalence_report(root)))
        paired = built["deterministic_invariants"]["paired_scoring"]
        self.assertEqual((paired["status"], paired["input_equivalence"]),
                         ("BLOCKED", "INPUT_EQUIVALENT"))
        self.assertIn("#53", paired["reason"])

    def test_equivalent_inputs_with_full_provenance_are_eligible(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case_path = write_case(root / "ADV-ANA-001.json", adv_case())
            seed = write_seed(root / "seed_000.json", "ADV-ANA-001", 0, adv_positions(4),
                              [1.0] * 4)
            oracle = write_flexpart_output(root / "oracle")
            manifest = v1_manifest(root, case_path, [seed],
                                   sorted(p for p in oracle.iterdir() if p.is_file()))
            built = evaluate_case.run_corpus_seeds(
                args_for(case_path, [seed], fortran_output=str(oracle),
                         run_manifest=str(manifest),
                         require_input_equivalence=str(input_equivalence_report(root))),
                ORACLE_MANIFEST)
        gate = built["deterministic_invariants"]
        self.assertEqual(gate["paired_scoring"]["status"], "ELIGIBLE")
        for realization in gate["realizations"]:
            binding = record(realization, "INV-PROVENANCE-BINDING")
            self.assertEqual(binding["verdict"], inv.PASS, realization["realization_id"])
            self.assertEqual(realization["execution_identity"]["case_id"], "ADV-ANA-001")
        oracle_realization = [r for r in gate["realizations"] if r["role"] == "oracle"][0]
        self.assertEqual(record(oracle_realization, "INV-CASE-IDENTITY")["verdict"], inv.PASS)
        self.assertEqual(oracle_realization["execution_identity"]["oracle_kind"], "pristine-oracle")
        self.assertEqual(built["overall_status"], "DIAGNOSTIC")


class ProvenanceIntegrationTest(unittest.TestCase):
    def _setup(self, root, **manifest_kwargs):
        case_path = write_case(root / "ADV-ANA-001.json", adv_case())
        seed = write_seed(root / "seed_000.json", "ADV-ANA-001", 0, adv_positions(4), [1.0] * 4)
        manifest = v1_manifest(root, case_path, [seed], **manifest_kwargs)
        return case_path, seed, manifest

    def test_missing_provenance_is_blocked(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case_path = write_case(root / "ADV-ANA-001.json", adv_case())
            seed = write_seed(root / "seed_000.json", "ADV-ANA-001", 0, adv_positions(4),
                              [1.0] * 4)
            built = evaluate_case.run_corpus_seeds(args_for(case_path, [seed]), ORACLE_MANIFEST)
        item = record(built["deterministic_invariants"]["realizations"][0],
                      "INV-PROVENANCE-BINDING")
        self.assertEqual((item["verdict"], item["error_id"]), (inv.BLOCKED, "PROVENANCE_MISSING"))

    def test_bound_execution_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            case_path, seed, manifest = self._setup(Path(directory))
            built = evaluate_case.run_corpus_seeds(
                args_for(case_path, [seed], run_manifest=str(manifest)), ORACLE_MANIFEST)
        realization = built["deterministic_invariants"]["realizations"][0]
        self.assertEqual(record(realization, "INV-PROVENANCE-BINDING")["verdict"], inv.PASS)
        self.assertEqual(realization["execution_identity"]["realization"], {"seed_index": 0})

    def test_mismatched_artifact_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            case_path, seed, manifest = self._setup(Path(directory))
            seed.write_text(seed.read_text(encoding="utf-8").replace('"rev-a"', '"rev-b"'),
                            encoding="utf-8")
            with self.assertRaises(provenance.ProvenanceError) as ctx:
                evaluate_case.run_corpus_seeds(
                    args_for(case_path, [seed], run_manifest=str(manifest)), ORACLE_MANIFEST)
        self.assertEqual(evaluate_case.classify_failure(ctx.exception)[0], "PROVENANCE_INVALID")

    def test_execution_bound_to_wrong_realization(self):
        with tempfile.TemporaryDirectory() as directory:
            case_path, seed, manifest = self._setup(Path(directory), seed_indices=[7])
            with self.assertRaises(inv.InvariantGateError) as ctx:
                evaluate_case.run_corpus_seeds(
                    args_for(case_path, [seed], run_manifest=str(manifest)), ORACLE_MANIFEST)
        self.assertEqual(ctx.exception.error_id, "EXECUTION_BINDING_MISMATCH")

    def test_case_manifest_changed_after_execution(self):
        with tempfile.TemporaryDirectory() as directory:
            case_path, seed, manifest = self._setup(Path(directory))
            case = json.loads(case_path.read_text(encoding="utf-8"))
            case["notes"].append("edited after the run")
            write_case(case_path, case)
            with self.assertRaises(inv.InvariantGateError) as ctx:
                evaluate_case.run_corpus_seeds(
                    args_for(case_path, [seed], run_manifest=str(manifest)), ORACLE_MANIFEST)
        self.assertEqual(ctx.exception.error_id, "EXECUTION_BINDING_MISMATCH")

    def test_mixed_execution_provenance(self):
        case = stochastic_case(count=2)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case_path = write_case(root / "STOCH-001.json", case)
            positions = {"lon_deg": [10.0] * 4, "lat_deg": [50.0] * 4, "z_m": [50.0] * 4}
            seeds = [write_seed(root / f"seed_{i:03d}.json", "STOCH-001", i, positions,
                                [0.25] * 4, key=(1 + i, 2), revision=rev, initial_mass=1.0)
                     for i, rev in enumerate(("rev-a", "rev-b"))]
            with self.assertRaises(inv.InvariantGateError) as ctx:
                evaluate_case.run_corpus_seeds(args_for(case_path, seeds), ORACLE_MANIFEST)
        self.assertEqual(ctx.exception.error_id, "MIXED_CANDIDATE_REVISION")

    def test_duplicate_execution_identity_across_realizations(self):
        ctx = {"mode": "v1", "verified": {"seed_000.json", "seed_001.json"},
               "bindings": {}, "case_manifest_sha256": None}
        execution = {"execution_id": "e" * 64, "role": "candidate", "case_id": "STOCH-001",
                     "realization": {}}
        ctx["bindings"] = {"seed_000.json": [execution], "seed_001.json": [execution]}
        case = stochastic_case(count=2)
        evidences = [particle_ev(case_id="STOCH-001", masses=[0.25] * 4,
                                 rid=f"candidate:seed_{i:03d}.json", seed_index=i)
                     for i in range(2)]
        gate = inv.build_gate(CONTRACT, case, evidences, provenance=ctx)
        errors = {(e["realization_id"], e["error_id"]) for e in gate["integrity_errors"]}
        self.assertEqual(errors, {("candidate:seed_000.json", "EXECUTION_IDENTITY_AMBIGUOUS"),
                                  ("candidate:seed_001.json", "EXECUTION_IDENTITY_AMBIGUOUS")})


# ---------------------------------------------------------------------------
# Command-line behavior: written reports, exit codes, error identifiers
# ---------------------------------------------------------------------------

class CommandLineTest(unittest.TestCase):
    def _cli(self, root, extra):
        output = root / "report.json"
        completed = subprocess.run(
            [sys.executable, str(HERE / "evaluate_case.py"), "--case", "corpus-seeds",
             "--output", str(output)] + extra, capture_output=True, text=True)
        return completed.returncode, json.loads(output.read_text(encoding="utf-8"))

    def test_nan_seed_writes_integrity_report(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case_path = write_case(root / "ADV-ANA-001.json", adv_case())
            seed = write_seed(root / "seed_000.json", "ADV-ANA-001", 0, adv_positions(4),
                              [1.0] * 4)
            data = json.loads(seed.read_text(encoding="utf-8"))
            data["particles"][2]["mass_kg"] = float("nan")
            seed.write_text(json.dumps(data), encoding="utf-8")
            code, report = self._cli(root, ["--case-def", str(case_path),
                                            "--candidate-seeds", str(seed)])
        self.assertEqual(code, 2)
        self.assertEqual(report["overall_status"], "INTEGRITY_ERROR")
        gate = report["deterministic_invariants"]
        self.assertEqual(gate["integrity_errors"][0]["error_id"], "NONFINITE_VALUE")
        finite = record(gate["realizations"][0], "INV-FIELD-FINITE")
        self.assertEqual(finite["offenders"][0]["cell"], 2)
        self.assertIn("[NONFINITE_VALUE]", report["status_detail"])

    def test_mass_loss_exits_with_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case_path = write_case(root / "ADV-ANA-001.json", adv_case())
            seed = write_seed(root / "seed_000.json", "ADV-ANA-001", 0, adv_positions(4),
                              [1.0, 1.0, 1.0, 0.5])
            code, report = self._cli(root, ["--case-def", str(case_path),
                                            "--candidate-seeds", str(seed)])
        self.assertEqual(code, 1)
        self.assertEqual(report["overall_status"], "FAIL")
        self.assertIn("INV-MASS-CLOSURE [MASS_DEFICIT]", report["status_detail"])

    def test_mutated_contract_is_integrity_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            document = json.loads(inv.DEFAULT_CONTRACT.read_text(encoding="utf-8"))
            document["invariants"] = document["invariants"][1:]
            contract = root / "contract.json"
            contract.write_text(json.dumps(document), encoding="utf-8")
            case_path = write_case(root / "ADV-ANA-001.json", adv_case())
            seed = write_seed(root / "seed_000.json", "ADV-ANA-001", 0, adv_positions(4),
                              [1.0] * 4)
            code, report = self._cli(root, ["--case-def", str(case_path), "--candidate-seeds",
                                            str(seed), "--invariants", str(contract)])
        self.assertEqual(code, 2)
        self.assertEqual(report["deterministic_invariants"]["integrity_errors"][0]["error_id"],
                         "INVARIANT_CONTRACT_INVALID")

    def test_wrong_case_manifest_exit_code(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case_path = write_case(root / "ADV-ANA-001.json", adv_case())
            seed = write_seed(root / "seed_000.json", "WIND-UNI-002", 0, adv_positions(4),
                              [1.0] * 4)
            code, report = self._cli(root, ["--case-def", str(case_path),
                                            "--candidate-seeds", str(seed)])
        self.assertEqual(code, 2)
        self.assertEqual(report["deterministic_invariants"]["integrity_errors"][0]["error_id"],
                         "CASE_ID_MISMATCH")


if __name__ == "__main__":
    unittest.main()
