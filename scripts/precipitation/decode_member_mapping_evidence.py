#!/usr/bin/env python3
"""Decode #202 flex_extract trace and interpol_rain outputs into compact evidence.

Inputs are the research trace written by ``flex_extract_member_trace.py`` and the
raw oracle outputs written by ``member_mapping_oracle.sh``. The output is the
deterministic decoded-evidence JSON committed beside the #202 decision
artifact. Checks fail closed: a missing case, member, query or a violated
identity exits non-zero instead of writing evidence.

Decoded identities (node queries only, so no spatial arithmetic is redone):

* ``pinned``: oracle == (r0*dt2 + r1*dt1) / dtt with the DTS values the
  original routine reports, within the #71 policy |d| <= 1e-6 + 1e-5*|o|;
* ``linear_point_blend`` = (r0*dt2 + r1*dt1) / (dt1 + dt2), the linear
  interpolation of the prepared point members that flex_extract's darain
  documentation names; the ratio oracle / linear is recorded.
"""

import argparse
import datetime as dt
import hashlib
import json
import os
import sys

import numpy as np

ABSOLUTE_TOLERANCE = 1.0e-6
RELATIVE_TOLERANCE = 1.0e-5
LSP = "142"
CP = "143"
EPOCH = dt.datetime(2023, 11, 1, 0)
NODE_QUERIES = {(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)}


def fail(message):
    sys.exit("member mapping evidence failed: " + message)


def sha256_file(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def within_policy(candidate, oracle):
    return abs(candidate - oracle) <= ABSOLUTE_TOLERANCE + RELATIVE_TOLERANCE * abs(oracle)


def f32(value):
    return float(np.float32(max(value, 0.0)))


def parse_rain_output(path):
    lines = open(path).read().split("\n")
    if not lines or lines[0] != "FLEXPART_INTERPOLATION_ROUTINE_ORACLE_V1":
        fail(path + ": unexpected oracle header")
    if "MODE rain" not in lines or "NUMPF 1" not in lines:
        fail(path + ": not a numpf=1 rain run")
    rows, row = [], None
    for line in lines:
        fields = line.split()
        if not fields:
            continue
        if fields[0] == "QUERY":
            row = {}
            rows.append(row)
        elif fields[0] == "XY_ITIME_KZ":
            row.update(xt=float(fields[1]), yt=float(fields[2]), itime=int(fields[3]))
        elif fields[0] == "DTS":
            row.update(dt1=float(fields[1]), dt2=float(fields[2]), dtt=float(fields[3]))
        elif fields[0] in ("LSP", "CP"):
            row[fields[0].lower()] = float(fields[1])
    for row in rows:
        if not {"xt", "yt", "itime", "dt1", "dt2", "dtt", "lsp", "cp"} <= set(row):
            fail(path + ": incomplete query block")
    return rows


def member_seconds(name):
    stamp = dt.datetime.strptime(name[4:14], "%Y%m%d%H")
    return int((stamp - EPOCH).total_seconds())


def prepared_members(variant):
    members = {}
    for name, record in variant["flux_files"].items():
        precip = [m for m in record["messages"] if m["paramId"] in (142, 143)]
        if not precip:
            continue
        members[member_seconds(name)] = {
            str(m["paramId"]): m["values_by_node"] for m in precip if m["stepRange_int"] == 0}
    return members


def interval_means(trace, param):
    """Interval-mean rates [mm/h] as recorded inside deacc_fluxes, keyed by end time."""
    means = {}
    for write in trace["variants"]["rrint0"]["writes"]:
        caller = write["caller"]
        if str(caller["paramId"]) != param:
            continue
        end = dt.datetime.fromisoformat(caller["t_dt"])
        series = caller["deac_vals_mm_per_h"]
        for offset, values in enumerate(reversed(series)):
            stamp = end - dt.timedelta(hours=trace["dtime_hours"] * offset)
            means.setdefault(int((stamp - EPOCH).total_seconds()), values)
    return means


def decode_probe_rows(trace, oracle_dir):
    members = prepared_members(trace["variants"]["rrint0"])
    cases = {}
    for case, manifest in trace["rain_inputs"].items():
        path = os.path.join(oracle_dir, "runs", "rain-{}-output.txt".format(case))
        if not os.path.exists(path):
            fail("missing oracle output for " + case)
        t0, t1 = manifest["memtime_s"]
        rows = parse_rain_output(path)
        if len(rows) != manifest["query_count"]:
            fail(case + ": query count mismatch")
        decoded = []
        for row in rows:
            entry = dict(row, memtime_s=[t0, t1])
            if (row["xt"], row["yt"]) in NODE_QUERIES:
                node = "{},{}".format(int(row["xt"]), int(row["yt"]))
                for key, param in (("lsp", LSP), ("cp", CP)):
                    r0, r1 = f32(members[t0][param][node]), f32(members[t1][param][node])
                    pinned = (r0 * row["dt2"] + r1 * row["dt1"]) / row["dtt"]
                    linear = (r0 * row["dt2"] + r1 * row["dt1"]) / (row["dt1"] + row["dt2"])
                    if not within_policy(pinned, row[key]):
                        fail("{} {} itime {}: oracle {} != pinned arithmetic {}".format(
                            case, key, row["itime"], row[key], pinned))
                    entry[key + "_members_mm_per_h"] = [r0, r1]
                    entry[key + "_pinned_expected"] = pinned
                    entry[key + "_linear_point_blend"] = linear
                    entry[key + "_ratio_to_linear"] = row[key] / linear if linear > 0 else None
                entry["node"] = node
            decoded.append(entry)
        cases[case] = {"memtime_s": [t0, t1], "raw_output": os.path.basename(path),
                       "raw_output_sha256": sha256_file(path), "rows": decoded}
    return cases


def member_vs_interval_audit(trace):
    """Compare each interior prepared member with both adjacent interval means."""
    members = prepared_members(trace["variants"]["rrint0"])
    dtime_s = trace["dtime_hours"] * 3600
    audit = {}
    for param in (LSP, CP):
        means = interval_means(trace, param)
        rows = []
        for t in sorted(members):
            if t - dtime_s not in means or t + dtime_s not in means:
                continue
            for node, value in sorted(members[t][param].items()):
                before, after = means[t][node], means[t + dtime_s][node]
                rows.append({"member_time_s": t, "node": node, "member_mm_per_h": value,
                             "mean_ending_at_member_mm_per_h": before,
                             "mean_starting_at_member_mm_per_h": after,
                             "equals_ending_mean": within_policy(value, before),
                             "equals_starting_mean": within_policy(value, after)})
        audit[param] = rows
    return audit


def darain_reproduces_members(trace):
    """Largest |member - recorded darain result| after GRIB re-packing."""
    worst = 0.0
    variant = trace["variants"]["rrint0"]
    for write in variant["writes"]:
        name = write["file"]
        decoded = [m for m in variant["flux_files"].get(name, {}).get("messages", [])
                   if m["paramId"] == write["caller"]["paramId"]]
        if not decoded:
            continue
        for node, value in write["values_mm_per_h"].items():
            worst = max(worst, abs(decoded[0]["values_by_node"][node] - value))
    return worst


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--trace", required=True)
    parser.add_argument("--oracle", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    trace = json.load(open(args.trace))
    cases = decode_probe_rows(trace, args.oracle)
    variants = trace["variants"]
    rrint1_counts = sorted({len([m for m in f["messages"] if m["paramId"] in (142, 143)])
                            for f in variants["rrint1"]["flux_files"].values() if f["messages"]})
    rrint0_counts = sorted({len([m for m in f["messages"] if m["paramId"] in (142, 143)])
                            for f in variants["rrint0"]["flux_files"].values() if f["messages"]})
    basetime = variants["rrint0_basetime0"]["flux_files"]
    duplicate = (basetime["flux2023110221"]["messages"][0]["values_by_node"]
                 == basetime["flux2023110218"]["messages"][0]["values_by_node"])
    evidence = {
        "schema": "flexpart-gpu/precipitation-member-evidence/v1",
        "issue": 202,
        "tolerance_policy": {"absolute": ABSOLUTE_TOLERANCE, "relative": RELATIVE_TOLERANCE,
                             "rule": "|candidate - oracle| <= absolute + relative * |oracle|"},
        "trace_sha256": sha256_file(args.trace),
        "oracle_hashes_sha256": sha256_file(os.path.join(args.oracle, "hashes.txt")),
        "flex_extract": trace["flex_extract"],
        "runtime": trace["runtime"],
        "epoch": trace["epoch"],
        "dtime_hours": trace["dtime_hours"],
        "interval_amounts_mm": trace["interval_amounts_mm"],
        "accumulated_input_sha256": {k: v["sha256"] for k, v in trace["accumulated_inputs"].items()},
        "prepared_flux_sha256": {v: {k: f["sha256"] for k, f in rec["flux_files"].items()}
                                 for v, rec in variants.items()},
        "prepared_members_rrint0": {str(t): m for t, m in sorted(prepared_members(variants["rrint0"]).items())},
        "precip_messages_per_flux_file": {"rrint0": rrint0_counts, "rrint1": rrint1_counts},
        "basetime_trailing_duplicate": duplicate,
        "max_abs_member_minus_darain_result": darain_reproduces_members(trace),
        "member_vs_interval_means": member_vs_interval_audit(trace),
        "interpol_rain_cases": cases,
        "rain_input_sha256": {k: v["input_sha256"] for k, v in trace["rain_inputs"].items()},
    }
    with open(args.out, "w", newline="\n") as handle:
        json.dump(evidence, handle, indent=1, sort_keys=True)
        handle.write("\n")
    print(args.out, sha256_file(args.out))


if __name__ == "__main__":
    main()
