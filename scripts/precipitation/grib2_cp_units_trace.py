#!/usr/bin/env python3
"""Trace flex_extract GRIB2 precipitation preparation for the FLEXPART CP reader (#211).

Research instrumentation for issue #211. Inside the pinned ``flex-extract``
container this script:

* runs the *unmodified* flex_extract 7.1.2 ``EcFlexpart.deacc_fluxes`` on the
  declared #202 accumulated LSP/CP example (``RRINT=0``), reusing the #202
  runner ``flex_extract_member_trace.run_deacc_fluxes``;
* assembles complete FLEXPART input files ``EN<YYMMDDHH>`` in the order of
  ``EcFlexpart.create`` (``EcFlexpart.py:1757-1765``): model-level fields,
  the genuine prepared ``flux<T>`` file, surface fields, orography/land-sea mask;
* runs the *unmodified* ``EcFlexpart.process_output`` with ``FORMAT=GRIB2``
  (``EcFlexpart.py:1893-1902``: ``grib_set -s edition=2,
  productDefinitionTemplateNumber=8``) on a copy, and with ``FORMAT=GRIB1`` on
  another copy, so both editions carry the same prepared values;
* decodes every message with the ecCodes keys that FLEXPART 11.1
  ``readwind_ecmwf`` requests plus unit and packing metadata;
* records the ecCodes definition entries that map LSP/CP between editions.

Only the precipitation messages are original-producer output. The model-level,
surface and orography messages are candidate-created support fields that make
the files complete enough for ``gridcheck_ecmwf``/``readwind_ecmwf``; their
values are not under audit. See docs/research/grib2-cp-units-211.md.
"""

import argparse
import copy
import hashlib
import json
import os
import shutil
import sys
import types

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import flex_extract_member_trace as member_trace  # noqa: E402  (#202 runner, same directory)

import numpy as np  # noqa: E402

LSP_PARAM = member_trace.LSP_PARAM
CP_PARAM = member_trace.CP_PARAM
PRECIPITATION_PARAMS = (LSP_PARAM, CP_PARAM)
# Prepared member times (flux<YYYYMMDDHH>) assembled into complete input files.
MEMBER_STAMPS = ["2023110112", "2023110206"]
OUTPUT_PREFIX = "EN"
VARIANT = "rrint0"
# Model levels of the support fields; FLEXPART derives nlev_ec from the pv size.
MODEL_LEVELS = 3
# ECMWF half-level coefficients, top to bottom: a [Pa] then b [1].
PV_A_PA = [0.0, 2000.0, 6000.0, 0.0]
PV_B = [0.0, 0.2, 0.6, 1.0]
GRIB_BITS_PER_VALUE = member_trace.GRIB_BITS_PER_VALUE

# Candidate-created support fields (constant per message), not under audit.
MODEL_LEVEL_FIELDS = [(130, [240.0, 260.0, 280.0]), (131, [12.0, 8.0, 5.0]),
                      (132, [3.0, 2.0, 1.0]), (133, [0.0005, 0.002, 0.005]),
                      (77, [0.0, 0.0, 0.0])]
SURFACE_FIELDS = [(134, 100000.0), (151, 101300.0), (164, 0.5), (165, 3.0), (166, 1.0),
                  (167, 285.0), (168, 280.0), (141, 0.0), (146, -20.0), (176, 100.0),
                  (180, 0.1), (181, 0.05)]
OROLSM_FIELDS = [(129, 0.0), (172, 1.0), (160, 10.0)]

DECODE_KEYS_INT = ["editionNumber", "centre", "paramId", "dataDate", "dataTime", "level",
                   "bitsPerValue", "binaryScaleFactor", "decimalScaleFactor",
                   "numberOfVerticalCoordinateValues", "Ni", "Nj"]
DECODE_KEYS_STR = ["shortName", "name", "units", "typeOfLevel", "stepType", "stepRange",
                   "packingType"]
DECODE_KEYS_GRIB1 = ["table2Version", "indicatorOfParameter", "indicatorOfTypeOfLevel"]
DECODE_KEYS_GRIB2 = ["discipline", "parameterCategory", "parameterNumber",
                     "typeOfFirstFixedSurface", "productDefinitionTemplateNumber",
                     "typeOfStatisticalProcessing"]

# Definitions of the container's ecCodes 2.24.2 (codes_definition_path() is empty there).
ECCODES_DEFINITIONS = "/usr/share/eccodes/definitions"
DEFINITION_EXCERPTS = [
    ("grib2/localConcepts/ecmf/paramId.def", ["142", "143"]),
    ("grib2/localConcepts/ecmf/units.def", ["Large-scale precipitation", "Convective precipitation"]),
    ("grib2/paramId.def", ["3063"]),
    ("grib2/units.def", ["Convective precipitation (water)"]),
    ("grib1/localConcepts/ecmf/paramId.def", ["142", "143"]),
]


def require(condition, message):
    if not condition:
        raise SystemExit("grib2 cp units trace failed: " + message)


def sha256_file(path):
    return member_trace.sha256_file(path)


def grib1_message(handle, param, values, data_date, data_time, level_type, level=0, pv=None):
    from eccodes import codes_grib_new_from_samples, codes_set, codes_set_array, codes_set_values
    from eccodes import codes_write, codes_release

    gid = codes_grib_new_from_samples("regular_ll_sfc_grib1")
    codes_set(gid, "centre", 98)
    codes_set(gid, "table2Version", 128)
    codes_set(gid, "indicatorOfParameter", param)
    codes_set(gid, "Ni", member_trace.NX)
    codes_set(gid, "Nj", member_trace.NY)
    codes_set(gid, "latitudeOfFirstGridPointInDegrees", member_trace.LAT_FIRST_DEG)
    codes_set(gid, "longitudeOfFirstGridPointInDegrees", member_trace.LON_FIRST_DEG)
    codes_set(gid, "latitudeOfLastGridPointInDegrees",
              member_trace.LAT_FIRST_DEG - (member_trace.NY - 1) * member_trace.GRID_STEP_DEG)
    codes_set(gid, "longitudeOfLastGridPointInDegrees",
              member_trace.LON_FIRST_DEG + (member_trace.NX - 1) * member_trace.GRID_STEP_DEG)
    codes_set(gid, "iDirectionIncrementInDegrees", member_trace.GRID_STEP_DEG)
    codes_set(gid, "jDirectionIncrementInDegrees", member_trace.GRID_STEP_DEG)
    codes_set(gid, "dataDate", data_date)
    codes_set(gid, "dataTime", data_time)
    codes_set(gid, "typeOfLevel", level_type)
    codes_set(gid, "level", level)
    if pv is not None:
        codes_set(gid, "PVPresent", 1)
        codes_set_array(gid, "pv", pv)
    codes_set(gid, "bitsPerValue", GRIB_BITS_PER_VALUE)
    codes_set_values(gid, np.array(values, dtype=np.float64))
    codes_write(gid, handle)
    codes_release(gid)


def write_support_files(directory, stamp):
    data_date = int(stamp[:8])
    data_time = int(stamp[8:10]) * 100
    points = member_trace.NX * member_trace.NY
    pv = PV_A_PA + PV_B
    with open(os.path.join(directory, "fort.15"), "wb") as handle:
        for level in range(1, MODEL_LEVELS + 1):
            for param, per_level in MODEL_LEVEL_FIELDS:
                grib1_message(handle, param, [per_level[level - 1]] * points, data_date,
                              data_time, "hybrid", level, pv)
    with open(os.path.join(directory, "fort.16"), "wb") as handle:
        for param, value in SURFACE_FIELDS:
            grib1_message(handle, param, [value] * points, data_date, data_time, "surface")
    orolsm = "OG_OROLSM__SL.{}.{}.grb".format(stamp, member_trace.PPID)
    with open(os.path.join(directory, orolsm), "wb") as handle:
        for param, value in OROLSM_FIELDS:
            grib1_message(handle, param, [value] * points, data_date, data_time, "surface")
    return ["fort.15", "flux" + stamp, "fort.16", orolsm]


def assemble_input_file(directory, stamp, flux_path):
    """Concatenate parts in EcFlexpart.create order (EcFlexpart.py:1757-1765, cwc=0)."""
    shutil.copy2(flux_path, os.path.join(directory, "flux" + stamp))
    parts = write_support_files(directory, stamp)
    name = OUTPUT_PREFIX + stamp[2:10]
    with open(os.path.join(directory, name), "wb") as output:
        for part in parts:
            with open(os.path.join(directory, part), "rb") as source:
                shutil.copyfileobj(source, output)
    return name, parts


def run_process_output(source_root, directory, names, grib_format):
    """Execute the unmodified EcFlexpart.process_output on assembled files."""
    python_root = os.path.join(source_root, "Source", "Python")
    if python_root not in sys.path:
        sys.path.insert(0, python_root)
    from Classes import EcFlexpart as ec_module
    import _config

    require(not _config.FLAG_ON_ECMWFSERVER, "ECMWF server branches must stay disabled")
    flexpart = ec_module.EcFlexpart.__new__(ec_module.EcFlexpart)
    flexpart.outputfilelist = list(names)
    flexpart.inputdir = directory
    control = types.SimpleNamespace(format=grib_format, ectrans=0, ecstorage=0, ecfsdir="",
                                    gateway="", destination="", inputdir=directory,
                                    outputdir=directory)
    log_path = os.path.join(directory, "process_output-{}.log".format(grib_format))
    stdout = sys.stdout
    try:
        with open(log_path, "w") as log:
            sys.stdout = log
            flexpart.process_output(control)
    finally:
        sys.stdout = stdout
    return {"format": grib_format, "log": os.path.basename(log_path),
            "log_sha256": sha256_file(log_path)}


def packed_integers(values, reference, binary_scale, decimal_scale):
    """Recover simple-packing integers X with Y = (R + X*2^E) / 10^D, checked exactly."""
    integers = []
    for value in values:
        packed = round((value * 10.0 ** decimal_scale - reference) / 2.0 ** binary_scale)
        rebuilt = (reference + packed * 2.0 ** binary_scale) / 10.0 ** decimal_scale
        require(rebuilt == value, "simple-packing round trip is not exact")
        integers.append(int(packed))
    return integers


def decode_file(path):
    from eccodes import (codes_grib_new_from_file, codes_get, codes_get_double,
                         codes_get_values, codes_release, codes_is_defined)

    messages = []
    with open(path, "rb") as handle:
        while True:
            gid = codes_grib_new_from_file(handle)
            if gid is None:
                break
            message = {key: codes_get(gid, key, int) for key in DECODE_KEYS_INT}
            message.update({key: codes_get(gid, key, str) for key in DECODE_KEYS_STR})
            extra = DECODE_KEYS_GRIB1 if message["editionNumber"] == 1 else DECODE_KEYS_GRIB2
            for key in extra:
                message[key] = codes_get(gid, key, int) if codes_is_defined(gid, key) else None
            # readwind_ecmwf windfields_mod.f90:2063-2066 reads stepRange as an integer.
            message["stepRange_int"] = codes_get(gid, "stepRange", int)
            message["referenceValue"] = codes_get_double(gid, "referenceValue")
            values = [float(v) for v in codes_get_values(gid)]
            if message["paramId"] not in PRECIPITATION_PARAMS and message["shortName"] != "acpcp":
                message["support_field"] = True
            else:
                message["values_by_node"] = member_trace.node_values_from_grib(values)
                message["packed_by_node"] = member_trace.node_values_from_grib(
                    packed_integers(values, message["referenceValue"],
                                    message["binaryScaleFactor"], message["decimalScaleFactor"]))
            messages.append(message)
            codes_release(gid)
    return messages


def definition_excerpts(definitions=ECCODES_DEFINITIONS, selection=DEFINITION_EXCERPTS):
    excerpts = []
    for relative, labels in selection:
        path = os.path.join(definitions, relative)
        with open(path) as handle:
            lines = handle.read().splitlines()
        blocks = {}
        for label in labels:
            for index, line in enumerate(lines):
                quoted = "'{}' = {{".format(label)
                if line.startswith(quoted) or (line.startswith("#") and line[1:].strip() == label):
                    start = index - 1 if line.startswith(quoted) else index
                    end = index + 1
                    while end < len(lines) and lines[end].strip() != "}":
                        end += 1
                    blocks[label] = {"first_line": start + 1,
                                     "text": lines[start:end + 1]}
                    break
            require(label in blocks, "definition entry {} missing in {}".format(label, relative))
        excerpts.append({"file": relative, "sha256": sha256_file(path), "entries": blocks})
    return excerpts


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--flex-extract-git", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    source_root = os.path.join(out_dir, "flex_extract_src")
    source_hashes = member_trace.extract_pinned_source(args.flex_extract_git, source_root)

    input_dir = os.path.join(out_dir, "accumulated-input")
    if os.path.exists(input_dir):
        shutil.rmtree(input_dir)
    inputs = member_trace.write_accumulated_inputs(input_dir)
    variant = member_trace.run_deacc_fluxes(source_root, VARIANT, member_trace.VARIANTS[VARIANT],
                                            inputs, os.path.join(out_dir, "deacc-" + VARIANT))
    # Values handed to codes_write by deacc_fluxes, before GRIB packing.
    written = {}
    for write in variant["writes"]:
        stamp = write["file"][len("flux"):]
        param = write["caller"]["paramId"]
        require((stamp, param) not in written, "duplicate prepared write for " + write["file"])
        written[(stamp, param)] = write["values_mm_per_h"]

    editions = {}
    for grib_format in ("GRIB1", "GRIB2"):
        directory = os.path.join(out_dir, "prepared-" + grib_format.lower())
        if os.path.exists(directory):
            shutil.rmtree(directory)
        os.makedirs(directory)
        files = {}
        names = []
        for stamp in MEMBER_STAMPS:
            flux = os.path.join(out_dir, "deacc-" + VARIANT, "input", "flux" + stamp)
            require(os.path.exists(flux), "prepared flux file missing for " + stamp)
            name, parts = assemble_input_file(directory, stamp, flux)
            names.append(name)
            files[name] = {"member_stamp": stamp, "parts": parts,
                           "flux_sha256": sha256_file(flux),
                           "assembled_grib1_sha256": sha256_file(os.path.join(directory, name))}
        process = run_process_output(source_root, directory, names, grib_format)
        for name in names:
            path = os.path.join(directory, name)
            files[name]["sha256"] = sha256_file(path)
            files[name]["bytes"] = os.path.getsize(path)
            files[name]["messages"] = decode_file(path)
        editions[grib_format] = {"directory": os.path.relpath(directory, out_dir),
                                 "process_output": process, "files": files}

    import eccodes
    trace = {
        "schema": "flexpart-gpu/grib2-cp-units-trace/v1",
        "issue": 211,
        "flex_extract": {"pinned_commit": member_trace.FLEX_EXTRACT_PIN,
                         "blobs": member_trace.FLEX_EXTRACT_BLOBS,
                         "archived_sha256": source_hashes},
        "runtime": {"python": sys.version.split()[0], "numpy": np.__version__,
                    "eccodes": eccodes.codes_get_api_version(),
                    "definitions": ECCODES_DEFINITIONS},
        "variant": VARIANT,
        "variant_control": variant["control"],
        "deacc_log_sha256": variant["log_sha256"],
        "interval_amounts_mm": {
            "lsp": {"{},{}".format(*n): v for n, v in member_trace.LSP_INTERVAL_AMOUNTS_MM.items()},
            "cp": {"{},{}".format(*n): v for n, v in member_trace.CP_INTERVAL_AMOUNTS_MM.items()},
        },
        "producer_written_mm_per_h": {stamp: {str(param): written[(stamp, param)]
                                              for param in PRECIPITATION_PARAMS}
                                      for stamp in MEMBER_STAMPS},
        "support_fields": {"model_levels": MODEL_LEVELS, "pv_a_pa": PV_A_PA, "pv_b": PV_B,
                           "model_level": MODEL_LEVEL_FIELDS, "surface": SURFACE_FIELDS,
                           "orolsm": OROLSM_FIELDS,
                           "role": "candidate-created completeness fields; not under audit"},
        "eccodes_definitions": definition_excerpts(),
        "editions": editions,
    }
    trace_path = os.path.join(out_dir, "grib2-cp-units-trace.json")
    with open(trace_path, "w") as handle:
        json.dump(copy.deepcopy(trace), handle, indent=1, sort_keys=True)
    print(trace_path, sha256_file(trace_path))


if __name__ == "__main__":
    main()
