#!/usr/bin/env python3
"""Freeze the original flex_extract ``darain`` oracle for #207 precipitation members.

Two oracle sections are written, both produced by unmodified pinned code:

* ``genuine``: decoded from the #202 raw trace (``flex_extract_member_trace.py``
  executed unmodified ``EcFlexpart.deacc_fluxes`` on the declared accumulated
  GRIB example). The trace hash must equal the #202 decision artifact. The
  section carries the GRIB-decoded accumulated inputs, every interval-mean rate
  (``deac_vals``) exactly as handed to ``darain`` before its in-place clipping,
  and every float64 ``darain`` result with its member time.
* ``direct``: the pinned ``disaggregation.darain`` (git blob checked) executed
  on declared float32-representable interval-mean sequences with the original
  sliding-window call pattern of ``deacc_fluxes`` (append, call with four live
  arrays, pop). It covers negative inputs, both zero-denominator branches and
  the canonical accumulated sequence used by the #90 -> #207 production test.

Nothing here re-implements ``darain``. Must run inside the pinned
``flex-extract`` container. See docs/precipitation-members.md.
"""

import argparse
import datetime as dt
import hashlib
import json
import os
import subprocess
import sys
import types

import numpy as np

FLEX_EXTRACT_PIN = "e0005c99ac81d12faa45a8ff799debbd592b0dc0"
DISAGGREGATION_PATH = "Source/Python/Mods/disaggregation.py"
DISAGGREGATION_BLOB = "12ec74a7ee4cc0d0fe4555406160fcd7b4b9945e"
DECISION_PATH = "fixtures/accumulation/precipitation-member-mapping-v1.json"
EVIDENCE_PATH = "fixtures/accumulation/precipitation-member-mapping-evidence-v1.json"
ABSOLUTE_TOLERANCE = 1.0e-6
RELATIVE_TOLERANCE = 1.0e-5
PARAMS = {"lsp": 142, "cp": 143}
# #202 example grid (flex_extract_member_trace.py): nodes "i,j" with j = 0 south.
GRID = {"nx": 3, "ny": 3, "xlon0_deg": 10.0, "ylat0_deg": 50.0, "dx_deg": 1.0, "dy_deg": 1.0}
EPOCH = dt.datetime(2023, 11, 1, 0)


def fail(message):
    sys.exit("darain member oracle failed: " + message)


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def sha256_file(path):
    with open(path, "rb") as handle:
        return sha256_bytes(handle.read())


def within_policy(candidate, oracle):
    return abs(candidate - oracle) <= ABSOLUTE_TOLERANCE + RELATIVE_TOLERANCE * abs(oracle)


def git(git_dir, *args):
    env = dict(os.environ, GIT_CONFIG_COUNT="1", GIT_CONFIG_KEY_0="safe.directory",
               GIT_CONFIG_VALUE_0="*")
    return subprocess.run(["git", "-C", git_dir] + list(args), check=True,
                          capture_output=True, env=env).stdout


def load_pinned_darain(git_dir):
    """Load the unmodified pinned disaggregation module from the git object store."""
    resolved = git(git_dir, "rev-parse", FLEX_EXTRACT_PIN + "^{commit}").decode().strip()
    if resolved != FLEX_EXTRACT_PIN:
        fail("flex_extract pin does not resolve")
    listed = git(git_dir, "ls-tree", FLEX_EXTRACT_PIN, DISAGGREGATION_PATH).decode().split()
    if len(listed) < 3 or listed[2] != DISAGGREGATION_BLOB:
        fail("unexpected disaggregation.py blob")
    source = git(git_dir, "show", FLEX_EXTRACT_PIN + ":" + DISAGGREGATION_PATH)
    module = types.ModuleType("disaggregation")
    module.__file__ = DISAGGREGATION_PATH
    exec(compile(source, DISAGGREGATION_PATH, "exec"), module.__dict__)
    return module.darain, sha256_bytes(source)


def cell_order():
    return ["{},{}".format(i, j) for j in range(GRID["ny"]) for i in range(GRID["nx"])]


def by_cell(values_by_node):
    return [values_by_node[node] for node in cell_order()]


def seconds(stamp):
    return int((stamp - EPOCH).total_seconds())


def genuine_section(trace, decision):
    variant = trace["variants"]["rrint0"]
    control = variant["control"]
    if (control["rrint"], control["purefc"], control["basetime"], control["marsclass"]) != (0, 0, None, "OD"):
        fail("genuine trace is not the RRINT=0, PUREFC=0, BASETIME-unset OD variant")
    dtime_s = trace["dtime_hours"] * 3600

    accumulated = {name: [] for name in PARAMS}
    for base_date, base_hour in trace["forecast_bases"]:
        base = dt.datetime.fromisoformat(base_date) + dt.timedelta(hours=base_hour)
        stamp = base.strftime("%Y%m%d%H")
        record = trace["accumulated_inputs"]["FCOG_acc_SL.{}.4242.1".format(stamp)]
        for name, param in PARAMS.items():
            messages = sorted((m for m in record["messages"] if m["paramId"] == param),
                              key=lambda m: m["stepRange_int"])
            for message in messages:
                if message["bitsPerValue"] != 24 or message["editionNumber"] != 1:
                    fail("unexpected accumulated GRIB packing")
                accumulated[name].append({
                    "reset_s": seconds(base),
                    "valid_s": seconds(base + dt.timedelta(hours=message["stepRange_int"])),
                    "step_hours": message["stepRange_int"],
                    "values_m": by_cell(message["values_by_node"]),
                })

    interval_means, members = {}, {}
    for name, param in PARAMS.items():
        calls = [c for c in variant["darain_calls"] if c["caller"]["paramId"] == param]
        calls.sort(key=lambda c: c["caller"]["t_m2dt"])
        means = {}
        rows = []
        for call in calls:
            member = dt.datetime.fromisoformat(call["caller"]["t_m2dt"])
            member_s = seconds(member)
            inputs = [by_cell(v) for v in call["inputs_mm_per_h_before_call"]]
            if len(inputs) != 4:
                fail("darain call without four intervals")
            # The first call that receives an interval sees it before darain clips in place.
            for position, values in enumerate(inputs):
                end_s = member_s + (position - 1) * dtime_s
                means.setdefault(end_s, values)
            rows.append({"member_time_s": member_s,
                         "support_s": [member_s - 2 * dtime_s, member_s + 2 * dtime_s],
                         "inputs_mm_per_h": inputs,
                         "result_mm_per_h": by_cell(call["result_mm_per_h"])})
        ends = sorted(means)
        if ends != [ends[0] + k * dtime_s for k in range(len(ends))]:
            fail(name + ": interval means are not contiguous")
        interval_means[name] = {"interval_end_s": ends, "values_mm_per_h": [means[e] for e in ends]}
        members[name] = rows
    member_times = [row["member_time_s"] for row in members["lsp"]]
    if member_times != [row["member_time_s"] for row in members["cp"]]:
        fail("LSP and CP member times differ")
    n = len(interval_means["lsp"]["interval_end_s"])
    s0 = interval_means["lsp"]["interval_end_s"][0] - dtime_s
    if member_times != [s0 + j * dtime_s for j in range(2, n - 1)]:
        fail("genuine member times are not T_j = s0 + j*DTIME, j = 2..N-2")

    # The decoded #202 prepared members are the same darain results after GRIB packing.
    evidence = decision["prepared"]
    worst = 0.0
    for name, param in PARAMS.items():
        for row in members[name]:
            packed = by_cell(evidence[str(row["member_time_s"])][str(param)])
            for value, reference in zip(row["result_mm_per_h"], packed):
                if not within_policy(reference, value):
                    fail("darain result differs from the #202 prepared member")
                worst = max(worst, abs(reference - value))

    leading = [w for w in variant["writes"] if w["file"] == "flux2023110103"]
    return {
        "source": "#202 raw trace, variant rrint0 (unmodified EcFlexpart.deacc_fluxes)",
        "grid": GRID,
        "cell_order": "canonical x-fastest; cell = j*nx + i for flex_extract node 'i,j' with j = 0 south",
        "dtime_hours": trace["dtime_hours"],
        "epoch": trace["epoch"],
        "sequence_start_s": s0,
        "accumulated_unit": "m (GRIB1 paramId 142/143, 24-bit packed, decoded by ecCodes)",
        "accumulated": accumulated,
        "interval_means": interval_means,
        "darain_members": members,
        "rejected_leading_persistence": {
            "member_time_s": seconds(dt.datetime(2023, 11, 1, 3)),
            "reason": "flex_extract writes deac_vals[0] at the third read step; it is its own interval mean, not a darain member",
            "values_mm_per_h": {name: by_cell(next(w for w in leading if w["caller"]["paramId"] == p)["values_mm_per_h"])
                                for name, p in PARAMS.items()},
        },
        "max_abs_packed_member_minus_darain": worst,
        "negative_interval_means": [
            {"parameter": name, "interval_end_s": end, "cell": cell, "value_mm_per_h": value}
            for name in PARAMS
            for end, row in zip(interval_means[name]["interval_end_s"], interval_means[name]["values_mm_per_h"])
            for cell, value in enumerate(row) if value < 0.0],
    }


def f32(value):
    return float(np.float32(value))


def run_original_pattern(darain, sequence):
    """Call the pinned darain exactly as deacc_fluxes does: four live arrays, then pop."""
    live, rows = [], []
    for index, values in enumerate(sequence):
        live.append(np.array(values, dtype=np.float64))
        if len(live) == 4:
            before = [list(map(float, a)) for a in live]
            result = darain(live)
            rows.append({"window_start": index - 3, "inputs_mm_per_h": before,
                         "result_mm_per_h": [float(v) for v in result]})
            live.pop(0)
    return rows


# Declared interval-mean rates [mm/h] per cell; each column is one cell.
BRANCH_CELLS = {
    "zero": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    "constant": [1.25, 1.25, 1.25, 1.25, 1.25, 1.25],
    "alternating_dry_first": [0.0, 2.0, 0.0, 2.0, 0.0, 2.0],
    "alternating_wet_first": [2.0, 0.0, 2.0, 0.0, 2.0, 0.0],
    "isolated_burst": [0.0, 0.0, 4.0, 0.0, 0.0, 0.0],
    "negative_mixed": [-0.5, 1.0, -2.0, 3.0, -1.0, 0.5],
    "negative_all": [-1.0, -2.0, -0.25, -3.0, -1.0, -2.0],
    "negative_zero_denominators": [-1.0, 1.5, 0.0, -2.0, 0.75, -0.5],
    "varying": [0.3, 1.7, 0.05, 2.9, 4.4, 0.8],
    "magnitude_range": [1.0e-4, 50.0, 3.0e-3, 120.0, 0.02, 7.5],
}

# Canonical accumulated sequence for the #90 -> #207 production test: 3 x 2 grid,
# DTIME = 3 h, two declared runs (reset at s0 and s0 + 4*DTIME). Amounts are
# dyadic so deaccumulation is exact in float32 and float64 alike.
PRODUCTION_DTIME_HOURS = 3
PRODUCTION_GRID = {"nx": 3, "ny": 2, "xlon0_deg": 10.0, "ylat0_deg": 50.0, "dx_deg": 1.0, "dy_deg": 1.0}
PRODUCTION_RUN_LENGTHS = [4, 3]
PRODUCTION_AMOUNTS_MM = {
    "lsp": [
        [0.75, 1.5, 0.375, 3.0, 0.0, 6.0, 1.125],
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [1.5, 1.5, 1.5, 1.5, 1.5, 1.5, 1.5],
        [0.0, 3.0, 0.0, 3.0, 0.0, 3.0, 0.0],
        [0.0, 0.0, 0.0, 9.0, 0.0, 0.0, 0.0],
        [0.375, 0.75, 1.125, 1.5, 1.875, 2.25, 2.625],
    ],
    "cp": [
        [0.375, 0.0, 0.75, 0.375, 1.5, 0.0, 0.75],
        [3.0, 0.0, 3.0, 0.0, 3.0, 0.0, 3.0],
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.1875, 0.1875, 0.1875, 0.1875, 0.1875, 0.1875, 0.1875],
        [0.0, 0.0, 4.5, 0.0, 0.0, 0.0, 0.0],
        [2.25, 1.875, 1.5, 1.125, 0.75, 0.375, 0.0],
    ],
}


def production_case(darain):
    dtime_s = PRODUCTION_DTIME_HOURS * 3600
    count = sum(PRODUCTION_RUN_LENGTHS)
    section = {"grid": PRODUCTION_GRID, "dtime_hours": PRODUCTION_DTIME_HOURS,
               "sequence_start_s": 0, "run_lengths": PRODUCTION_RUN_LENGTHS,
               "accumulated_unit": "kg/m2 (canonical; numerically mm)", "parameters": {}}
    for name, cells in PRODUCTION_AMOUNTS_MM.items():
        observations, start = [], 0
        for run in PRODUCTION_RUN_LENGTHS:
            totals = [0.0] * len(cells)
            for step in range(run):
                k = start + step
                for cell, series in enumerate(cells):
                    totals[cell] += series[k]
                observations.append({"reset_s": start * dtime_s, "valid_s": (k + 1) * dtime_s,
                                     "values_kg_per_m2": [f32(v) for v in totals]})
            start += run
        means = [[series[k] / PRODUCTION_DTIME_HOURS for series in cells] for k in range(count)]
        if any(f32(v) != v for row in means for v in row):
            fail("production interval means must be float32-exact")
        rows = run_original_pattern(darain, means)
        for row in rows:
            row["member_time_s"] = (row["window_start"] + 2) * dtime_s
        section["parameters"][name] = {"accumulated": observations,
                                       "interval_means_mm_per_h": means, "darain_members": rows}
    return section


def direct_section(darain):
    names = list(BRANCH_CELLS)
    count = len(BRANCH_CELLS[names[0]])
    branches = [[f32(BRANCH_CELLS[name][k]) for name in names] for k in range(count)]
    minimum = branches[:5]
    return {
        "routine": "disaggregation.darain (pinned, unmodified), original deacc_fluxes sliding-window call pattern",
        "interval_mean_unit": "mm/h",
        "cases": {
            "darain_branches": {"cells": names, "interval_means_mm_per_h": branches,
                                "darain_members": run_original_pattern(darain, branches)},
            "minimum_support": {"cells": names, "interval_means_mm_per_h": minimum,
                                "darain_members": run_original_pattern(darain, minimum)},
        },
        "production_handoff": production_case(darain),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--trace", required=True, help="#202 flex-extract-trace.json")
    parser.add_argument("--flex-extract-git", required=True)
    parser.add_argument("--repo", required=True, help="flexpart-gpu checkout")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    decision_path = os.path.join(args.repo, DECISION_PATH)
    evidence_path = os.path.join(args.repo, EVIDENCE_PATH)
    decision = json.load(open(decision_path))
    trace_sha = sha256_file(args.trace)
    if trace_sha != decision["evidence"]["raw_trace_sha256"]:
        fail("trace hash does not match the #202 decision artifact")
    if sha256_file(evidence_path) != decision["evidence"]["decoded_sha256"]:
        fail("#202 decoded evidence hash does not match the decision artifact")
    evidence = json.load(open(evidence_path))
    trace = json.load(open(args.trace))
    darain, darain_sha = load_pinned_darain(args.flex_extract_git)

    oracle = {
        "schema": "flexpart-gpu/precipitation-darain-oracle/v1",
        "issue": 207,
        "decision_issue": 202,
        "tolerance_policy": {"absolute": ABSOLUTE_TOLERANCE, "relative": RELATIVE_TOLERANCE,
                             "rule": "|candidate - oracle| <= absolute + relative * |oracle|",
                             "source": "#71 (src/meteorology/temporal.rs DEFAULT_ABS_TOL / DEFAULT_REL_TOL)"},
        "flex_extract": {"pinned_commit": FLEX_EXTRACT_PIN, "routine": DISAGGREGATION_PATH + ":106-157",
                         "blob": DISAGGREGATION_BLOB, "source_sha256": darain_sha},
        "runtime": {"python": sys.version.split()[0], "numpy": np.__version__},
        "consumed": {"trace_sha256": trace_sha,
                     "decision_sha256": sha256_file(decision_path),
                     "decoded_evidence_sha256": sha256_file(evidence_path)},
        "genuine": genuine_section(trace, {"prepared": evidence["prepared_members_rrint0"]}),
        "direct": direct_section(darain),
    }
    with open(args.out, "w", newline="\n") as handle:
        json.dump(oracle, handle, indent=1, sort_keys=True)
        handle.write("\n")
    print(args.out, sha256_file(args.out))


if __name__ == "__main__":
    main()
