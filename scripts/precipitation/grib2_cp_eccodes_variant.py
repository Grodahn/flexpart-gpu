#!/usr/bin/env python3
"""Repeat the flex_extract GRIB2 conversion with another ecCodes release (#211).

The GRIB1->GRIB2 step of flex_extract 7.1.2 is ``EcFlexpart.process_output``
with ``FORMAT=GRIB2`` (``EcFlexpart.py:1893-1902``), which shells out to the
installed ecCodes ``grib_set -s edition=2,productDefinitionTemplateNumber=8``.
Its result therefore depends on the ecCodes definitions of the producer host.
This script reruns the *unmodified* method, from the flex_extract source tree
archived by ``grib2_cp_units_trace.py``, on that trace's pre-conversion GRIB1
files, under whatever ecCodes is installed (``grib_set`` must be on PATH),
and decodes the result with the same keys. See docs/research/grib2-cp-units-211.md.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import grib2_cp_units_trace as units_trace  # noqa: E402  (same directory)

VARIANT_DEFINITION_EXCERPTS = [
    ("grib2/localConcepts/ecmf/paramId.def", ["142", "143"]),
]


def definition_record(definitions):
    """Excerpt on-disk definitions; wheel builds embed them in memory (MEMFS)."""
    if not os.path.isdir(definitions):
        return {"embedded": definitions,
                "note": "definitions are compiled into the library; executed decode is the evidence"}
    return units_trace.definition_excerpts(definitions, VARIANT_DEFINITION_EXCERPTS)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--trace", required=True, help="output directory of grib2_cp_units_trace.py")
    parser.add_argument("--label", required=True, help="output subdirectory name")
    args = parser.parse_args()

    import eccodes

    # process_output shells out to grib_set; it must come from the same ecCodes
    # release as the bindings that decode the result.
    grib_set = shutil.which("grib_set")
    units_trace.require(grib_set is not None, "grib_set is not on PATH")
    grib_set_version = subprocess.run([grib_set, "-V"], capture_output=True, text=True,
                                      check=True).stdout.strip()
    units_trace.require(grib_set_version.endswith(eccodes.codes_get_api_version()),
                        "grib_set and Python ecCodes releases differ: " + grib_set_version)

    trace_dir = os.path.abspath(args.trace)
    with open(os.path.join(trace_dir, "grib2-cp-units-trace.json")) as handle:
        trace = json.load(handle)
    source_root = os.path.join(trace_dir, "flex_extract_src")
    for path, digest in trace["flex_extract"]["archived_sha256"].items():
        units_trace.require(units_trace.sha256_file(os.path.join(source_root, path)) == digest,
                            "archived flex_extract source changed: " + path)
    grib1 = trace["editions"]["GRIB1"]
    directory = os.path.join(trace_dir, args.label)
    if os.path.exists(directory):
        shutil.rmtree(directory)
    os.makedirs(directory)
    files = {}
    for name, record in sorted(grib1["files"].items()):
        source = os.path.join(trace_dir, grib1["directory"], name)
        units_trace.require(units_trace.sha256_file(source) == record["assembled_grib1_sha256"],
                            "pre-conversion GRIB1 file changed: " + name)
        shutil.copy2(source, os.path.join(directory, name))
        files[name] = {"member_stamp": record["member_stamp"],
                       "pre_conversion_sha256": record["assembled_grib1_sha256"]}
    process = units_trace.run_process_output(source_root, directory, sorted(files), "GRIB2")
    for name in files:
        path = os.path.join(directory, name)
        files[name]["sha256"] = units_trace.sha256_file(path)
        files[name]["bytes"] = os.path.getsize(path)
        files[name]["messages"] = units_trace.decode_file(path)
    variant = {
        "schema": "flexpart-gpu/grib2-cp-units-eccodes-variant/v1",
        "issue": 211,
        "runtime": {"python": sys.version.split()[0], "eccodes": eccodes.codes_get_api_version(),
                    "grib_set": grib_set_version,
                    "definitions": eccodes.codes_definition_path()},
        "eccodes_definitions": definition_record(eccodes.codes_definition_path()),
        "process_output": process,
        "files": files,
    }
    out_path = os.path.join(directory, "eccodes-variant.json")
    with open(out_path, "w") as handle:
        json.dump(variant, handle, indent=1, sort_keys=True)
    print(out_path, units_trace.sha256_file(out_path))


if __name__ == "__main__":
    main()
