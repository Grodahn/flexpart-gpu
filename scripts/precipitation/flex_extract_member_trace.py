#!/usr/bin/env python3
"""Trace pinned flex_extract precipitation preparation into FLEXPART members (#202).

Research instrumentation for issue #202. It executes the *unmodified*
flex_extract 7.1.2 routine ``EcFlexpart.deacc_fluxes`` (pinned commit from
``reference/flex-extract.json``) on a deterministic accumulated-precipitation
GRIB sequence and records:

* the decoded accumulated inputs (the declared finite source example);
* every interval-mean rate list handed to ``disaggregation.darain`` and its
  result (observed through a pass-through wrapper, the original function runs);
* every GRIB message the routine writes, with the frame state that chose it;
* the decoded prepared flux files, including the GRIB keys read by
  FLEXPART 11.1 ``readwind_ecmwf`` (``stepRange`` -> ``ipf = stepRange + 1``);
* direct ``interpol_rain`` driver inputs built from the prepared members.

The prepared outputs are the scientific authority of this trace; nothing here
re-implements the preparation. Must run inside the pinned ``flex-extract``
container (ecCodes Python bindings). See docs/research/precipitation-interval-mapping-202.md.
"""

import argparse
import copy
import datetime as dt
import hashlib
import inspect
import json
import os
import shutil
import subprocess
import sys
import types

import numpy as np

FLEX_EXTRACT_PIN = "e0005c99ac81d12faa45a8ff799debbd592b0dc0"
# Git blob ids at the pin; any mismatch fails the trace closed.
FLEX_EXTRACT_BLOBS = {
    "Source/Python/Classes/EcFlexpart.py": "65ba97f7e4212e4187accda307d79436bee23231",
    "Source/Python/Classes/GribUtil.py": "6712f354f0e7af7189b2b6db97e47c225559011f",
    "Source/Python/Classes/UioFiles.py": "f3e5a7ce2e22dea67b5ba241b7ffcee67c825c2f",
    "Source/Python/Mods/disaggregation.py": "12ec74a7ee4cc0d0fe4555406160fcd7b4b9945e",
    "Source/Python/Mods/tools.py": "0286555e38617a01a78fdff51f94f6575d02ab7f",
    "Source/Python/_config.py": "dae1de931aac4124e1a5246dbc9c93a2663c15fa",
    "Templates/ecmwf_grib1_table_128": "7ff3a6c95c3c8b72b0b3923915e5a357d9c27d30",
}

EPOCH = dt.datetime(2023, 11, 1, 0)
DTIME_HOURS = 3
DTIME_SECONDS = DTIME_HOURS * 3600
FORECAST_BASES = [(dt.date(2023, 11, 1), 0), (dt.date(2023, 11, 1), 12),
                  (dt.date(2023, 11, 2), 0), (dt.date(2023, 11, 2), 12)]
STEPS_HOURS = [3, 6, 9, 12]
INTERVAL_COUNT = len(FORECAST_BASES) * len(STEPS_HOURS)
PPID = "4242"
LSP_PARAM = 142
CP_PARAM = 143
GRIB_BITS_PER_VALUE = 24
# 3 x 3 regular lat/lon grid, scanned north to south as in MARS output.
NX = 3
NY = 3
LON_FIRST_DEG = 10.0
LAT_FIRST_DEG = 52.0
GRID_STEP_DEG = 1.0


def _realistic():
    return [0.12, 0.45, 2.7, 5.1, 3.3, 1.8, 0.6, 0.0,
            0.0, 0.9, 2.4, 4.2, 3.6, 1.5, 0.3, 0.06]


def _burst():
    return [0.0, 0.0, 0.0, 1.5, 6.0, 12.0, 7.5, 3.0,
            0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]


# Interval amounts [mm per 3-h interval] keyed by FLEXPART node (i, j), j=0 south.
LSP_INTERVAL_AMOUNTS_MM = {
    (0, 0): [3.0] * INTERVAL_COUNT,
    (1, 0): _burst(),
    (2, 0): [0.0] * INTERVAL_COUNT,
    (0, 1): [0.3 * (k + 1) for k in range(INTERVAL_COUNT)],
    (1, 1): _realistic(),
    (2, 1): [0.0 if k % 2 == 0 else 4.5 for k in range(INTERVAL_COUNT)],
    (0, 2): [0.5 * v for v in reversed(_realistic())],
    (1, 2): [1.5] * INTERVAL_COUNT,
    (2, 2): [0.0] * INTERVAL_COUNT,
}
CP_INTERVAL_AMOUNTS_MM = {
    (0, 0): [0.6] * INTERVAL_COUNT,
    (1, 0): [0.25 * v for v in _burst()],
    (2, 0): [0.0] * INTERVAL_COUNT,
    (0, 1): [0.15 * (k % 4) for k in range(INTERVAL_COUNT)],
    (1, 1): [0.5 * v for v in reversed(_realistic())],
    (2, 1): [1.2 if k % 3 == 0 else 0.0 for k in range(INTERVAL_COUNT)],
    (0, 2): [0.2] * INTERVAL_COUNT,
    (1, 2): [0.4 * v for v in _realistic()],
    (2, 2): [0.0] * INTERVAL_COUNT,
}

VARIANTS = {
    # Default operational non-pure-forecast extraction: RRINT=0, no BASETIME.
    "rrint0": {"rrint": 0, "basetime": None, "start_date": "20231101", "end_date": "20231102"},
    # BASETIME path whose final step triggers flex_extract's trailing-boundary writes.
    "rrint0_basetime0": {"rrint": 0, "basetime": 0, "start_date": "20231101", "end_date": "20231103"},
    # RRINT=1 (IA3, Hittmeir et al. 2018) to observe the members FLEXPART numpf=1 rejects.
    "rrint1": {"rrint": 1, "basetime": None, "start_date": "20231101", "end_date": "20231102"},
}

# Memory pairs (indices of 3-hourly member times t_k = (k + 1) * 3 h) probed in
# genuine interpol_rain; offsets are seconds after the first member.
RAIN_PAIRS = {
    "leading_boundary": (0, 1),
    "first_interior": (1, 2),
    # Shares member 6 with "interior": the exact interior boundary from both sides.
    "interior_left": (5, 6),
    "interior": (6, 7),
    "last_interior": (12, 13),
}
RAIN_OFFSETS_S = [-3600, 0, 3600, 5400, 7200, DTIME_SECONDS, DTIME_SECONDS + 3600]
RAIN_QUERY_NODES = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5), (1.25, 0.5)]


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require(condition, message):
    if not condition:
        raise SystemExit("flex_extract member trace failed: " + message)


def extract_pinned_source(git_dir, destination):
    env = dict(os.environ, GIT_CONFIG_COUNT="1", GIT_CONFIG_KEY_0="safe.directory",
               GIT_CONFIG_VALUE_0="*")
    resolved = subprocess.run(["git", "-C", git_dir, "rev-parse", FLEX_EXTRACT_PIN + "^{commit}"],
                              check=True, capture_output=True, text=True, env=env).stdout.strip()
    require(resolved == FLEX_EXTRACT_PIN, "flex_extract pin does not resolve")
    for path, blob in FLEX_EXTRACT_BLOBS.items():
        listed = subprocess.run(["git", "-C", git_dir, "ls-tree", FLEX_EXTRACT_PIN, path],
                                check=True, capture_output=True, text=True, env=env).stdout.split()
        require(len(listed) >= 3 and listed[2] == blob, "unexpected blob for " + path)
    if os.path.exists(destination):
        shutil.rmtree(destination)
    os.makedirs(destination)
    archive = subprocess.run(["git", "-C", git_dir, "archive", FLEX_EXTRACT_PIN, "Source", "Templates"],
                             check=True, capture_output=True, env=env).stdout
    subprocess.run(["tar", "-x", "-C", destination], input=archive, check=True)
    hashes = {}
    for path in FLEX_EXTRACT_BLOBS:
        hashes[path] = sha256_file(os.path.join(destination, path))
    return hashes


def grib_row_major(node_values):
    """Order FLEXPART (i, j) node values as GRIB north-to-south scan values."""
    values = []
    for row in range(NY):
        j = NY - 1 - row
        for i in range(NX):
            values.append(node_values[(i, j)])
    return values


def write_accumulated_inputs(input_dir):
    from eccodes import (codes_grib_new_from_samples, codes_set, codes_set_values,
                         codes_write, codes_release)

    os.makedirs(input_dir, exist_ok=True)
    written = []
    for base_index, (base_date, base_hour) in enumerate(FORECAST_BASES):
        name = "FCOG_acc_SL.{}.{}.1".format(base_date.strftime("%Y%m%d") + "{:0>2}".format(base_hour), PPID)
        path = os.path.join(input_dir, name)
        with open(path, "wb") as handle:
            for step_index, step in enumerate(STEPS_HOURS):
                for param, amounts in ((LSP_PARAM, LSP_INTERVAL_AMOUNTS_MM),
                                       (CP_PARAM, CP_INTERVAL_AMOUNTS_MM)):
                    first = base_index * len(STEPS_HOURS)
                    last = first + step_index
                    accumulated_m = {node: sum(series[first:last + 1]) / 1000.0
                                     for node, series in amounts.items()}
                    gid = codes_grib_new_from_samples("regular_ll_sfc_grib1")
                    codes_set(gid, "centre", 98)
                    codes_set(gid, "table2Version", 128)
                    codes_set(gid, "indicatorOfParameter", param)
                    codes_set(gid, "Ni", NX)
                    codes_set(gid, "Nj", NY)
                    codes_set(gid, "latitudeOfFirstGridPointInDegrees", LAT_FIRST_DEG)
                    codes_set(gid, "longitudeOfFirstGridPointInDegrees", LON_FIRST_DEG)
                    codes_set(gid, "latitudeOfLastGridPointInDegrees", LAT_FIRST_DEG - (NY - 1) * GRID_STEP_DEG)
                    codes_set(gid, "longitudeOfLastGridPointInDegrees", LON_FIRST_DEG + (NX - 1) * GRID_STEP_DEG)
                    codes_set(gid, "iDirectionIncrementInDegrees", GRID_STEP_DEG)
                    codes_set(gid, "jDirectionIncrementInDegrees", GRID_STEP_DEG)
                    codes_set(gid, "dataDate", int(base_date.strftime("%Y%m%d")))
                    codes_set(gid, "dataTime", base_hour * 100)
                    codes_set(gid, "step", step)
                    codes_set(gid, "bitsPerValue", GRIB_BITS_PER_VALUE)
                    codes_set_values(gid, np.array(grib_row_major(accumulated_m), dtype=np.float64))
                    codes_write(gid, handle)
                    codes_release(gid)
        written.append(path)
    return written


GRIB_KEYS = ["editionNumber", "centre", "table2Version", "indicatorOfParameter", "paramId",
             "dataDate", "dataTime", "stepType", "timeRangeIndicator", "startStep", "endStep",
             "Ni", "Nj", "bitsPerValue"]


def decode_grib(path):
    from eccodes import codes_grib_new_from_file, codes_get, codes_get_values, codes_release

    messages = []
    with open(path, "rb") as handle:
        while True:
            gid = codes_grib_new_from_file(handle)
            if gid is None:
                break
            message = {key: codes_get(gid, key) for key in GRIB_KEYS}
            message["stepRange"] = codes_get(gid, "stepRange", str)
            # FLEXPART 11.1 windfields_mod.f90:425-427,2063-2066 reads stepRange as an integer.
            message["stepRange_int"] = codes_get(gid, "stepRange", int)
            message["values"] = [float(v) for v in codes_get_values(gid)]
            messages.append(message)
            codes_release(gid)
    return messages


def node_values_from_grib(values):
    nodes = {}
    for row in range(NY):
        for i in range(NX):
            nodes["{},{}".format(i, NY - 1 - row)] = values[row * NX + i]
    return nodes


class Recorder:
    """Observes the original routine without changing its arguments or results."""

    def __init__(self):
        self.darain_calls = []
        self.writes = []

    @staticmethod
    def _caller_state():
        frame = inspect.currentframe().f_back.f_back
        while frame is not None and frame.f_code.co_name != "deacc_fluxes":
            frame = frame.f_back
        require(frame is not None, "instrumentation called outside deacc_fluxes")
        local = frame.f_locals
        param = local.get("parId")
        series = local.get("deac_vals", {}).get(param, [])
        return {
            "paramId": param,
            "read_step_h": local.get("step"),
            "t_dt": str(local.get("t_dt")),
            "t_m2dt": str(local.get("t_m2dt")),
            "deac_vals_mm_per_h": [node_values_from_grib([float(v) for v in a]) for a in series],
        }

    def wrap_darain(self, original):
        def darain(alist):
            state = self._caller_state()
            before = [node_values_from_grib([float(v) for v in a]) for a in alist]
            result = original(alist)
            self.darain_calls.append({
                "caller": state,
                "inputs_mm_per_h_before_call": before,
                "result_mm_per_h": node_values_from_grib([float(v) for v in result]),
            })
            return result
        return darain

    def wrap_codes_write(self, original):
        from eccodes import codes_get, codes_get_values

        def codes_write(gid, handle):
            state = self._caller_state()
            self.writes.append({
                "file": os.path.basename(handle.name),
                "dataDate": codes_get(gid, "dataDate"),
                "dataTime": codes_get(gid, "dataTime"),
                "stepRange": codes_get(gid, "stepRange", str),
                "values_mm_per_h": node_values_from_grib([float(v) for v in codes_get_values(gid)]),
                "caller": state,
            })
            return original(gid, handle)
        return codes_write


def run_deacc_fluxes(source_root, variant_name, settings, inputs, work_dir):
    python_root = os.path.join(source_root, "Source", "Python")
    if python_root not in sys.path:
        sys.path.insert(0, python_root)
    import eccodes
    from Classes import EcFlexpart as ec_module
    from Classes.UioFiles import UioFiles

    input_dir = os.path.join(work_dir, "input")
    if os.path.exists(work_dir):
        shutil.rmtree(work_dir)
    os.makedirs(input_dir)
    for path in inputs:
        shutil.copy2(path, input_dir)

    control = types.SimpleNamespace(
        rrint=settings["rrint"], purefc=0, marsclass="OD", dtime=str(DTIME_HOURS),
        basetime=settings["basetime"], start_date=settings["start_date"],
        end_date=settings["end_date"], maxstep=STEPS_HOURS[-1], accmaxstep="12",
        inputdir=input_dir, time=["00"], step=["0", "21"])

    flexpart = ec_module.EcFlexpart.__new__(ec_module.EcFlexpart)
    flexpart.params = {"OG_acc_SL": ["LSP/CP", "SFC", "1", "1.0/1.0"]}
    flexpart.number = "OFF"

    recorder = Recorder()
    original_darain = ec_module.disaggregation.darain
    original_write = eccodes.codes_write
    ec_module.disaggregation.darain = recorder.wrap_darain(original_darain)
    eccodes.codes_write = recorder.wrap_codes_write(original_write)
    log_path = os.path.join(work_dir, "deacc_fluxes.log")
    stdout = sys.stdout
    try:
        with open(log_path, "w") as log:
            sys.stdout = log
            inputfiles = UioFiles(input_dir, "*OG_acc_SL*." + PPID + ".*")
            flexpart.deacc_fluxes(inputfiles, control)
    finally:
        sys.stdout = stdout
        ec_module.disaggregation.darain = original_darain
        eccodes.codes_write = original_write

    outputs = {}
    for name in sorted(os.listdir(input_dir)):
        if not name.startswith("flux"):
            continue
        path = os.path.join(input_dir, name)
        messages = decode_grib(path)
        for message in messages:
            message["values_by_node"] = node_values_from_grib(message.pop("values"))
        outputs[name] = {"sha256": sha256_file(path), "bytes": os.path.getsize(path),
                         "messages": messages}
    return {
        "control": {key: value for key, value in vars(control).items() if key != "inputdir"},
        "log": os.path.relpath(log_path, work_dir),
        "log_sha256": sha256_file(log_path),
        "darain_calls": recorder.darain_calls,
        "writes": recorder.writes,
        "flux_files": outputs,
    }


def member_time_seconds(name):
    stamp = dt.datetime.strptime(name[len("flux"):len("flux") + 10], "%Y%m%d%H")
    return int((stamp - EPOCH).total_seconds())


def members_by_time(variant):
    """Prepared members as FLEXPART numpf=1 would store them (real4, ipf=1)."""
    members = {}
    for name, record in variant["flux_files"].items():
        messages = record["messages"]
        if not messages:
            continue
        precip = [m for m in messages if m["paramId"] in (LSP_PARAM, CP_PARAM)]
        require(all(m["stepRange_int"] == 0 for m in precip), name + ": non-zero stepRange")
        by_param = {m["paramId"]: m["values_by_node"] for m in precip}
        require(sorted(by_param) == [LSP_PARAM, CP_PARAM], name + ": missing LSP/CP member")
        members[member_time_seconds(name)] = by_param
    return members


def rain_driver_input(members, t0, t1, queries):
    lines = ["rain", "{} {}".format(NX, NY),
             "{} {} {} {}".format(LON_FIRST_DEG, LAT_FIRST_DEG - (NY - 1) * GRID_STEP_DEG,
                                  GRID_STEP_DEG, GRID_STEP_DEG),
             "0", "{} {}".format(t0, t1)]

    def plane(values_by_node):
        for j in range(NY):
            for i in range(NX):
                lines.append(repr(float(np.float32(max(values_by_node["{},{}".format(i, j)], 0.0)))))

    plane(members[t0][LSP_PARAM])
    plane(members[t1][LSP_PARAM])
    plane(members[t0][CP_PARAM])
    plane(members[t1][CP_PARAM])
    # Unused by the precipitation outputs; constant cloud/temperature planes.
    for constant in (0.5, 0.5, 280.0, 280.0, 0.001, 0.001):
        for _ in range(NX * NY):
            lines.append(repr(constant))
    lines.append(str(len(queries)))
    for xt, yt, itime in queries:
        lines.append("{} {} {} 1".format(xt, yt, itime))
    return "\n".join(lines) + "\n"


def write_rain_inputs(variant, out_dir):
    members = members_by_time(variant)
    times = sorted(members)
    cases = {}
    for label, (first, second) in RAIN_PAIRS.items():
        t0 = (first + 1) * DTIME_SECONDS
        t1 = (second + 1) * DTIME_SECONDS
        require(t0 in members and t1 in members, label + ": prepared member missing")
        queries = [(xt, yt, t0 + offset) for offset in RAIN_OFFSETS_S for xt, yt in RAIN_QUERY_NODES]
        cases[label] = (t0, t1, queries)
    last = times[-1]
    # Real-caller state at the final admitted time: getfields rotates memtime(2)
    # into memtime(1) and finds no later field (getfields_mod.f90:167-203).
    cases["final_duplicate_time"] = (last, last, [(xt, yt, last) for xt, yt in RAIN_QUERY_NODES])
    manifest = {}
    for label, (t0, t1, queries) in cases.items():
        path = os.path.join(out_dir, "rain-{}.txt".format(label))
        with open(path, "w") as handle:
            handle.write(rain_driver_input(members, t0, t1, queries))
        manifest[label] = {"memtime_s": [t0, t1], "input": os.path.basename(path),
                           "input_sha256": sha256_file(path), "query_count": len(queries)}
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--flex-extract-git", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    source_root = os.path.join(out_dir, "flex_extract_src")
    source_hashes = extract_pinned_source(args.flex_extract_git, source_root)

    input_dir = os.path.join(out_dir, "accumulated-input")
    if os.path.exists(input_dir):
        shutil.rmtree(input_dir)
    inputs = write_accumulated_inputs(input_dir)
    decoded_inputs = {}
    for path in inputs:
        messages = decode_grib(path)
        for message in messages:
            message["values_by_node"] = node_values_from_grib(message.pop("values"))
        decoded_inputs[os.path.basename(path)] = {"sha256": sha256_file(path), "messages": messages}

    variants = {}
    for name, settings in VARIANTS.items():
        variants[name] = run_deacc_fluxes(source_root, name, settings, inputs,
                                          os.path.join(out_dir, "variant-" + name))

    rain_dir = os.path.join(out_dir, "rain-inputs")
    os.makedirs(rain_dir, exist_ok=True)
    rain_manifest = write_rain_inputs(variants["rrint0"], rain_dir)

    import eccodes
    trace = {
        "schema": "flexpart-gpu/precipitation-member-trace/v1",
        "issue": 202,
        "flex_extract": {"pinned_commit": FLEX_EXTRACT_PIN, "blobs": FLEX_EXTRACT_BLOBS,
                         "archived_sha256": source_hashes},
        "runtime": {"python": sys.version.split()[0], "numpy": np.__version__,
                    "eccodes": eccodes.codes_get_api_version()},
        "epoch": EPOCH.isoformat() + "Z",
        "dtime_hours": DTIME_HOURS,
        "forecast_bases": [[d.isoformat(), h] for d, h in FORECAST_BASES],
        "steps_hours": STEPS_HOURS,
        "interval_amounts_mm": {
            "lsp": {"{},{}".format(*node): v for node, v in LSP_INTERVAL_AMOUNTS_MM.items()},
            "cp": {"{},{}".format(*node): v for node, v in CP_INTERVAL_AMOUNTS_MM.items()},
        },
        "accumulated_inputs": decoded_inputs,
        "variants": variants,
        "rain_inputs": rain_manifest,
    }
    trace_path = os.path.join(out_dir, "flex-extract-trace.json")
    with open(trace_path, "w") as handle:
        json.dump(copy.deepcopy(trace), handle, indent=1, sort_keys=True)
    print(trace_path, sha256_file(trace_path))


if __name__ == "__main__":
    main()
