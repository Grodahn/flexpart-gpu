#!/usr/bin/env python3
"""Freeze the #211 GRIB2 CP units evidence from the producer trace and reader oracle.

Combines ``grib2_cp_units_trace.py`` (flex_extract preparation, GRIB1/GRIB2
messages decoded with ecCodes 2.24.2), optional ``grib2_cp_eccodes_variant.py``
outputs (the same conversion under another ecCodes release) and the outputs of
``grib2_cp_reader_oracle.sh`` (genuine FLEXPART 11.1 ``readwind_ecmwf``) into
one per-node comparison table. Every reported ratio is computed from recorded
values; nothing here re-implements the reader. Values within the #71 policy
``|d| <= 1e-6 + 1e-5*|o|`` count as equal; power-of-1000 ratios are reported
as the exponent of the nearest power. See docs/research/grib2-cp-units-211.md.
"""

import argparse
import hashlib
import json
import math
import os

ABSOLUTE_TOLERANCE = 1.0e-6
RELATIVE_TOLERANCE = 1.0e-5
LSP_PARAM = 142
CP_PARAM = 143
FIELDS = (("lsp", LSP_PARAM), ("cp", CP_PARAM))
# Values below this many mm/h cannot expose a factor of 1000 against the
# absolute tolerance, so they only enter the exact zero/nonzero accounting.
RATIO_MIN_MM_PER_HOUR = 1.0e-3


def require(condition, message):
    if not condition:
        raise SystemExit("grib2 cp evidence failed: " + message)


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def within_policy(candidate, oracle):
    return abs(candidate - oracle) <= ABSOLUTE_TOLERANCE + RELATIVE_TOLERANCE * abs(oracle)


def power_of_1000(ratio):
    if ratio <= 0.0:
        return None
    return round(math.log10(ratio) / 3.0)


def read_reader_output(path):
    nodes = {}
    meta = {}
    with open(path) as handle:
        lines = handle.read().splitlines()
    require(lines[0] == "FLEXPART_GRIB2_CP_READER_ORACLE_V1", path + ": unexpected header")
    for line in lines[1:]:
        fields = line.split()
        if fields[0] == "NODE":
            ipf, i, j = (int(v) for v in fields[1:4])
            require(ipf == 1, path + ": numpf=1 expected")
            nodes["{},{}".format(i, j)] = {"lsp": float(fields[4]), "cp": float(fields[5])}
        else:
            meta[fields[0]] = fields[1:]
    return meta, nodes


def reader_log_lines(path):
    with open(path) as handle:
        return sorted({" ".join(line.split()) for line in handle
                       if "parId" in line or "WARNING" in line or "ERROR" in line
                       or "windfields_mod.f90:" in line})


def reader_exit(path):
    with open(path) as handle:
        return int(handle.read().strip())


def precipitation_messages(messages):
    selected = {}
    for message in messages:
        if message.get("support_field"):
            continue
        if message["editionNumber"] == 1:
            key = message["indicatorOfParameter"]
        elif message["parameterCategory"] == 1 and message["parameterNumber"] == 10:
            key = CP_PARAM
        else:
            key = message["paramId"]
        require(key not in selected, "duplicate precipitation message")
        selected[key] = message
    return selected


GRIB2_KEYS = ["paramId", "shortName", "units", "discipline", "parameterCategory", "parameterNumber",
              "typeOfFirstFixedSurface", "stepType", "stepRange", "stepRange_int",
              "typeOfStatisticalProcessing", "productDefinitionTemplateNumber", "packingType",
              "bitsPerValue", "binaryScaleFactor", "decimalScaleFactor", "referenceValue"]
GRIB1_KEYS = ["paramId", "shortName", "units", "table2Version", "indicatorOfParameter",
              "indicatorOfTypeOfLevel", "stepType", "stepRange", "stepRange_int", "packingType",
              "bitsPerValue", "binaryScaleFactor", "decimalScaleFactor", "referenceValue"]


def metadata(message):
    keys = GRIB1_KEYS if message["editionNumber"] == 1 else GRIB2_KEYS
    record = {"editionNumber": message["editionNumber"]}
    record.update({key: message.get(key) for key in keys})
    return record


def compare_case(case, produced, grib1, grib2, stored1, stored2):
    rows = []
    for field, param in FIELDS:
        for node in sorted(produced[str(param)]):
            declared = produced[str(param)][node]
            g1 = grib1[param]["values_by_node"][node]
            g2 = grib2[param]["values_by_node"][node] if param in grib2 else None
            s1 = stored1[node][field]
            s2 = stored2[node][field]
            row = {"case": case, "field": field, "node": node,
                   "declared_mm_per_h": declared,
                   "grib1_decoded": g1, "grib1_packed": grib1[param]["packed_by_node"][node],
                   "grib2_decoded": g2,
                   "grib2_packed": grib2[param]["packed_by_node"][node] if param in grib2 else None,
                   "reader_grib1_mm_per_h": s1, "reader_grib2_mm_per_h": s2,
                   "zero": declared == 0.0}
            row["grib1_reader_matches_declared"] = within_policy(s1, declared)
            row["grib2_reader_matches_declared"] = within_policy(s2, declared)
            row["grib2_reader_matches_grib1_reader"] = within_policy(s2, s1)
            if g2 is not None and declared >= RATIO_MIN_MM_PER_HOUR:
                row["grib2_decoded_over_declared"] = g2 / declared
                row["grib2_decoded_power_of_1000"] = power_of_1000(g2 / declared)
            if declared >= RATIO_MIN_MM_PER_HOUR and s2 > 0.0:
                row["reader_grib2_over_declared"] = s2 / declared
                row["reader_grib2_power_of_1000"] = power_of_1000(s2 / declared)
            rows.append(row)
    return rows


def summarize(rows):
    summary = {}
    for field, _ in FIELDS:
        selected = [r for r in rows if r["field"] == field]
        nonzero = [r for r in selected if r["declared_mm_per_h"] >= RATIO_MIN_MM_PER_HOUR]
        summary[field] = {
            "rows": len(selected),
            "zero_rows": sum(r["zero"] for r in selected),
            "ratio_rows": len(nonzero),
            "grib1_reader_matches_declared": all(r["grib1_reader_matches_declared"] for r in selected),
            "grib2_reader_matches_declared": all(r["grib2_reader_matches_declared"] for r in selected),
            "grib2_reader_matches_grib1_reader": all(r["grib2_reader_matches_grib1_reader"]
                                                     for r in selected),
            "grib2_decoded_powers_of_1000": sorted({r.get("grib2_decoded_power_of_1000")
                                                    for r in nonzero}, key=str),
            "reader_grib2_powers_of_1000": sorted({r.get("reader_grib2_power_of_1000")
                                                   for r in nonzero}, key=str),
            "max_abs_reader_grib2_minus_grib1": max(abs(r["reader_grib2_mm_per_h"]
                                                        - r["reader_grib1_mm_per_h"])
                                                    for r in selected),
        }
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--trace", required=True)
    parser.add_argument("--oracle", required=True)
    parser.add_argument("--variant", action="append", default=[],
                        help="label of a grib2_cp_eccodes_variant.py output directory")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    trace_path = os.path.join(args.trace, "grib2-cp-units-trace.json")
    with open(trace_path) as handle:
        trace = json.load(handle)
    produced_all = trace["producer_written_mm_per_h"]
    grib1_files = trace["editions"]["GRIB1"]["files"]
    sources = [("GRIB2", "grib2", trace["editions"]["GRIB2"]["files"], trace["runtime"]["eccodes"])]
    variants = {}
    for label in args.variant:
        path = os.path.join(args.trace, label, "eccodes-variant.json")
        with open(path) as handle:
            variant = json.load(handle)
        variants[label] = {"sha256": sha256_file(path), "runtime": variant["runtime"],
                           "eccodes_definitions": variant["eccodes_definitions"]}
        sources.append((label, label.replace("prepared-", "", 1), variant["files"],
                        variant["runtime"]["eccodes"]))

    conversions = {}
    for label, run_prefix, files, producer_eccodes in sources:
        rows = []
        cases = {}
        for name in sorted(grib1_files):
            stamp = grib1_files[name]["member_stamp"]
            grib1 = precipitation_messages(grib1_files[name]["messages"])
            grib2 = precipitation_messages(files[name]["messages"])
            require(sorted(grib1) == [LSP_PARAM, CP_PARAM], name + ": GRIB1 LSP/CP missing")
            run = os.path.join(args.oracle, "runs", "{}-{}-".format(run_prefix, name))
            out1 = os.path.join(args.oracle, "runs", "grib1-{}-output.txt".format(name))
            require(reader_exit(out1.replace("-output.txt", "-exit.txt")) == 0,
                    name + ": GRIB1 control reader run failed")
            exit2 = reader_exit(run + "exit.txt")
            case = "{}:{}".format(label, stamp)
            cases[case] = {
                "file": name, "member_stamp": stamp,
                "grib1_sha256": grib1_files[name]["sha256"], "grib2_sha256": files[name]["sha256"],
                "grib1_metadata": {str(k): metadata(v) for k, v in grib1.items()},
                "grib2_metadata": {str(k): metadata(v) for k, v in grib2.items()},
                "reader_grib1_output_sha256": sha256_file(out1),
                "reader_grib2_exit_status": exit2,
                "reader_grib2_log_sha256": sha256_file(run + "reader.log"),
                "reader_grib2_log_diagnostics": reader_log_lines(run + "reader.log"),
            }
            if exit2 != 0:
                require(not os.path.exists(run + "output.txt"), name + ": stopped run wrote output")
                cases[case]["reader_grib2_outcome"] = "STOP"
                continue
            meta1, stored1 = read_reader_output(out1)
            meta2, stored2 = read_reader_output(run + "output.txt")
            require(meta1["GRID"] == meta2["GRID"], name + ": reader grid differs")
            cases[case].update({"reader_grib2_outcome": "STORED", "reader_grid": meta2["GRID"],
                                "reader_grib2_output_sha256": sha256_file(run + "output.txt")})
            rows.extend(compare_case(case, produced_all[stamp], grib1, grib2, stored1, stored2))
        outcomes = sorted({c["reader_grib2_outcome"] for c in cases.values()})
        require(len(outcomes) == 1, label + ": mixed reader outcomes")
        conversions[label] = {"producer_eccodes": producer_eccodes, "reader_outcome": outcomes[0],
                              "cases": cases, "summary": summarize(rows) if rows else None,
                              "rows": rows}

    evidence = {
        "schema": "flexpart-gpu/grib2-cp-units-evidence/v1",
        "issue": 211,
        "trace_sha256": sha256_file(trace_path),
        "producer": {"flex_extract": trace["flex_extract"], "runtime": trace["runtime"],
                     "variant": trace["variant"], "variant_control": trace["variant_control"],
                     "eccodes_definitions": trace["eccodes_definitions"]},
        "eccodes_variants": variants,
        "reader_eccodes": open(os.path.join(args.oracle, "eccodes-version.txt")).read().split("\n")[:2],
        "reader_compiler": open(os.path.join(args.oracle, "compiler-version.txt")).read().strip(),
        "reader_checkout": [open(os.path.join(args.oracle, name)).read().split()
                            for name in ("checkout-before.txt", "checkout-after.txt")],
        "reader_hashes": open(os.path.join(args.oracle, "hashes.txt")).read().splitlines(),
        "interval_amounts_mm": trace["interval_amounts_mm"],
        "tolerance_policy": {"absolute": ABSOLUTE_TOLERANCE, "relative": RELATIVE_TOLERANCE,
                             "source": "#71 tolerance policy (meteorology::temporal DEFAULT_*_TOL)",
                             "ratio_min_mm_per_hour": RATIO_MIN_MM_PER_HOUR},
        "conversions": conversions,
    }
    with open(args.out, "w", newline="\n") as handle:
        json.dump(evidence, handle, indent=1, sort_keys=True)
        handle.write("\n")
    print(args.out, sha256_file(args.out))


if __name__ == "__main__":
    main()
