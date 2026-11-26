#!/usr/bin/env python3
"""Research replay of ICON-EU amounts through two unchanged pinned routines."""
import argparse
import bz2
import json
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/provider"))
sys.path.insert(0, str(ROOT / "scripts/precipitation"))

import icon_eu_fetch as retrieval
import icon_eu_precipitation_decision as admission
from darain_member_oracle import load_pinned_darain
from temporal_oracle import digest, driver_input, f32, parse_output

FIXTURE = ROOT / "fixtures/meteorology/icon-eu-precipitation-time-v1"
STEPS = tuple(range(6))
FIELDS = dict(zip(("lsp", "cp"), admission.COMPOSITION))


def read_json(path):
    return json.loads(path.read_bytes())


def write_json(path, value):
    path.write_bytes((json.dumps(value, indent=2, allow_nan=False) + "\n").encode())


def raw_messages():
    request = FIXTURE / "sample-request.json"
    manifest = read_json(FIXTURE / "retrieval-manifest.json")
    expected = {url: Path(path).as_posix() for url, path in retrieval.expand_request(read_json(request))}
    entries = manifest["files"]
    paths = [entry["path"] for entry in entries]
    if (manifest["request_sha256"] != digest(request)
            or len(set(paths)) != len(paths)
            or expected != {entry["url"]: entry["path"] for entry in entries}):
        raise ValueError("unexpected retrieval manifest")
    with zipfile.ZipFile(FIXTURE / "dwd-sample.zip") as archive:
        if sorted(archive.namelist()) != sorted(paths):
            raise ValueError("incomplete or duplicate archive contents")
        for entry in entries:
            packed = archive.read(entry["path"])
            message = bz2.decompress(packed)
            if (retrieval.sha256(packed) != entry["bz2_sha256"]
                    or retrieval.sha256(message) != entry["grib2_sha256"]
                    or len(packed) != entry["bz2_bytes"] or len(message) != entry["grib2_bytes"]):
                raise ValueError(f"source checksum or byte count mismatch: {entry['path']}")
            yield entry, message


def decode(np, eccodes):
    messages = {}
    libaec = admission.sections.load_libaec()
    if libaec is None:
        raise ValueError("libaec is required")
    cycle = read_json(FIXTURE / "sample-request.json")["cycle"]
    run = admission.epoch_seconds(f"{cycle[:4]}-{cycle[4:6]}-{cycle[6:8]}T{cycle[8:]}:00:00Z")
    for entry, message in raw_messages():
        parsed = admission.sections.parse(message)
        record = admission.packing_record(message)
        variable, = (v for v, number in admission.PARAMETER_NUMBERS.items()
                     if number == record["parameterNumber"])
        step, remainder = divmod(record["lengthOfTimeRange"], 60)
        expected = retrieval.file_name("single-level", cycle, variable, step, None)
        key = (variable, step)
        identity = (
            record["discipline"], record["parameterCategory"],
            record["productDefinitionTemplateNumber"], record["typeOfStatisticalProcessing"],
            record["numberOfTimeRanges"], record["typeOfTimeIncrement"],
            record["indicatorOfUnitOfTimeRange"], record["indicatorOfUnitForTimeRange"],
            record["forecastTime"], record["forecast_reference_s"], record["end_of_interval_s"],
        )
        if (key in messages or step not in STEPS or remainder or entry["path"] != f"{cycle}/{expected}"
                or identity != (0, 1, 8, 1, 1, 2, 0, 0, 0, run, run + step * 3600)
                or parsed["representation"]["template"] != 42
                or not admission._admitted_packing(record)
                or any(record[k] != v for k, v in admission.ICON_EU_SOURCE.items())):
            raise ValueError(f"invalid ICON-EU time or source record: {entry['path']}")
        integers = admission.decode_integers(message, libaec, np)
        independent = integers * (admission.quantum(record) or 1.0)
        values, _ = admission.eccodes_values(message, eccodes, np)
        if (values.size != record["Ni"] * record["Nj"]
                or not np.isfinite(values).all() or (values < 0).any()
                or (step == 0 and values.any()) or not np.array_equal(values, independent)):
            raise ValueError(f"nonmatching or invalid decoded amounts: {entry['path']}")
        messages[key] = {
            "record": record, "values": values, "grid": parsed["grid"],
            "summary": {"path": entry["path"], "variable": variable, "step": step,
                        "start_s": run, "end_s": record["end_of_interval_s"],
                        "message_sha256": entry["grib2_sha256"],
                        "values_sha256_f8le": admission.sha256_bytes(values.astype("<f8").tobytes())},
        }
    if set(messages) != {(variable, step) for variable in admission.VARIABLES for step in STEPS}:
        raise ValueError("missing required components or time points")
    return messages


def interval_rates(np, messages):
    closure = {step: admission.closure(np, messages, step) for step in STEPS}
    verified = {step: row["closure_verified"] for step, row in closure.items()}
    if not all(verified.values()):
        raise ValueError(f"failed #232 closure at steps {[s for s in STEPS if not verified[s]]}")
    rates, diagnostics = {}, {}
    for label, field in FIELDS.items():
        parts = admission.COMPOSITION[field]
        amounts = [admission.canonical_sum(np, messages, parts, step) for step in STEPS]
        for step, values in zip(STEPS, amounts):
            observation = admission.genuine_observation(messages, parts, step, values, 0, verified[step])
            if admission.classify_field(field, observation)[0] != "ADMIT" or not np.isfinite(values).all():
                raise ValueError(f"rejected #232 field at {step}")
        rates[label], diagnostics[label] = [], []
        for left, right in zip(STEPS, STEPS[1:]):
            row = admission.composite_pair(np, messages, verified, field, parts, left, right)
            if row["violating_cells"] or (row["decreasing_cells"] and not row["source_bound_kg_m2"]):
                raise ValueError(f"rejected #232 interval {left} to {right}")
            # #232/#90 projection, expressed in mm/h for exactly one hour.
            difference = amounts[right].astype(np.float64) - amounts[left].astype(np.float64)
            rates[label].append(np.maximum(difference, 0.0))
            diagnostics[label].append(row["decreasing_cells"])
    return rates, diagnostics, list(closure.values())


def candidate_member(np, inputs):
    """Analytical comparator only. The reference is unmodified flex_extract."""
    a, b, c, d = inputs
    product = b * c
    return (np.divide(product, a + c, out=b / 2, where=(a + c) > 0)
            + np.divide(product, b + d, out=c / 2, where=(b + d) > 0))


def reference_members(np, rates, darain):
    members, statistics = {}, {}
    for name, intervals in rates.items():
        members[name] = []
        deltas = []
        for start in (0, 1):
            inputs = intervals[start:start + 4]
            reference = darain([a.copy() for a in inputs])
            difference = np.abs(candidate_member(np, inputs) - reference)
            if not np.all(difference <= 1e-6 + 1e-5 * np.abs(reference)):
                raise ValueError("candidate member differs from original darain")
            members[name].append(reference)
            deltas.append(float(difference.max()))
        amount = intervals[2]
        # Exact integral of the ideal piecewise linear rates over one hour.
        linear = (members[name][0] + members[name][1]) / 2
        residual = np.abs(linear - amount)
        statistics[name] = {
            "max_candidate_error_mm_h": deltas,
            "max_ordinary_linear_amount_error_mm": float(residual.max()),
            "max_dt_over_three_amount_error_mm": float(np.max(np.abs(3 * linear - amount))),
            "dry_middle_interval_cells_with_nonzero_linear_amount": int(((amount == 0) & (linear > 0)).sum()),
        }
    return members, statistics


def prepare(directory, checkout):
    import eccodes
    import numpy as np

    messages = decode(np, eccodes)
    rates, projections, closure = interval_rates(np, messages)
    darain, source_hash = load_pinned_darain(str(checkout))
    members, statistics = reference_members(np, rates, darain)
    record = messages[("TOT_PREC", 0)]["record"]
    nx, ny = record["Ni"], record["Nj"]
    input_dir = directory / "rain-inputs"
    input_dir.mkdir(parents=True, exist_ok=False)
    cases = []
    for name in FIELDS:
        center = int(np.argmax(rates[name][2]))
        y, x = divmod(center, nx)
        x, y = min(max(x, 1), nx - 2), min(max(y, 1), ny - 2)
        nodes = [(y + j) * nx + x + i for j in (-1, 0, 1) for i in (-1, 0, 1)]
        for suffix, times, slots, query_times in (
                ("pair", [7200, 10800], (0, 1), range(7200, 10801, 900)),
                ("final", [10800, 10800], (1, 1), (10800,))):
            case = {
                "id": f"{name}-{suffix}", "nx": 3, "ny": 3, "memtime_s": times,
                "source_cell_indices": nodes,
                "interval_rates_mm_h": {f: [a[nodes].tolist() for a in rates[f]] for f in rates},
                "planes": {f: [members[f][i][nodes].astype(np.float32).tolist() for i in slots]
                           for f in members},
                "queries": [[xq, yq, t] for xq, yq in ((1, 1), (1.25, 0.5)) for t in query_times],
            }
            path = input_dir / f"rain-{case['id']}.txt"
            path.write_bytes(driver_input(case).encode())
            cases.append(case)
    write_json(directory / "preparation.json", {
        "cycle": read_json(FIXTURE / "sample-request.json")["cycle"],
        "eccodes_version": eccodes.codes_get_api_version(),
        "numpy_version": np.__version__,
        "darain_source_sha256": source_hash,
        "messages": [m["summary"] for m in messages.values()],
        "step_closure": closure,
        "projected_cells_per_interval": projections,
        "member_statistics": statistics,
        "cases": cases,
    })


def check_rows(case, rows):
    if [r["query"] for r in rows] != case["queries"]:
        raise ValueError("mismatched oracle query")
    max_error = 0.0
    for row in rows:
        x, y, time = row["query"]
        i, j = int(x), int(y)
        fx, fy = x - i, y - j
        left, right = case["memtime_s"]
        dt1, dt2, duration = time - left, right - time, right - left
        if not duration:
            dt2, duration = 1, 1
        if row["dts"] != [dt1, dt2, f32(duration / 3)]:
            raise ValueError("unexpected original temporal weights")
        for field in FIELDS:
            spatial = []
            for plane in case["planes"][field]:
                low, high = j * 3 + i, (j + 1) * 3 + i
                spatial.append(((1 - fx)*plane[low] + fx*plane[low + 1])*(1 - fy)
                               + ((1 - fx)*plane[high] + fx*plane[high + 1])*fy)
            expected = (spatial[0] * dt2 + spatial[1] * dt1) / (duration / 3)
            error = abs(row[field] - expected)
            if not error <= 1e-6 + 1e-5 * abs(row[field]):
                raise ValueError("analytical argument differs from original interpol_rain")
            max_error = max(max_error, error)
    return max_error


def replay_hashes(path):
    """Read the original harness checksum log independent of mount paths."""
    checksums = {}
    for line in path.read_text().splitlines():
        checksum, recorded_path = line.split("  ", 1)
        name = Path(recorded_path).name
        if name in checksums:
            raise ValueError(f"duplicate original harness checksum: {name}")
        checksums[name] = checksum
    return checksums


def checked_replay_bytes(path, checksums):
    data = path.read_bytes()
    if checksums.get(path.name) != admission.sha256_bytes(data):
        raise ValueError(f"missing or mismatched original harness checksum: {path.name}")
    return data


def finish(directory, output, image_id):
    run = directory / "oracle"
    preparation = read_json(directory / "preparation.json")
    revision = read_json(ROOT / "reference/flexpart-11.1.json")["pinned_commit"]
    if any((run / f"checkout-{phase}.txt").read_text().strip() != revision for phase in ("before", "after")):
        raise ValueError("unverified original FLEXPART checkout")
    expected_cases = [f"{field}-{suffix}" for field in FIELDS for suffix in ("pair", "final")]
    if [case["id"] for case in preparation["cases"]] != expected_cases:
        raise ValueError("missing, repeated or unexpected original case")
    checksums = replay_hashes(run / "hashes.txt")
    driver = ROOT / "scripts/interpolation/direct_interpolation_oracle.f90"
    executable = run / "build/interpolation-oracle"
    source_names = ("interpol_mod.f90", "getfields_mod.f90", "windfields_mod.f90", "par_mod.f90")
    sources = [run / "build/upstream" / name for name in source_names]
    for path in (driver, executable, *sources):
        checked_replay_bytes(path, checksums)
    for case in preparation["cases"]:
        input_path = directory / "rain-inputs" / f"rain-{case['id']}.txt"
        raw = checked_replay_bytes(input_path, checksums)
        if raw != driver_input(case).encode():
            raise ValueError(f"original input differs from prepared case: {case['id']}")
        result_path = run / "runs" / f"rain-{case['id']}-output.txt"
        result = checked_replay_bytes(result_path, checksums)
        case.update(raw_input=raw.decode(), raw_output=result.decode())
        case["input_sha256"] = checksums[input_path.name]
        case["output_sha256"] = checksums[result_path.name]
        case["rows"] = parse_output(case["raw_output"])
        case["max_candidate_error_mm_h"] = check_rows(case, case["rows"])
        if case["id"].endswith("-pair"):
            first, last = case["rows"][0], case["rows"][4]
            case["center_amount_comparison_mm"] = {
                field: {"icon_interval_amount": case["interval_rates_mm_h"][field][2][4],
                        "integral_of_original_rates": (first[field] + last[field]) / 2}
                for field in FIELDS
            }
    write_json(output, {
        "schema": "flexpart-gpu.icon-eu-precipitation-time-evidence.v1",
        "manifest_sha256": digest(FIXTURE / "retrieval-manifest.json"),
        "flexpart_revision": revision,
        "flex_extract_revision": read_json(ROOT / "reference/flex-extract.json")["pinned_commit"],
        "execution_image_id": image_id,
        "compiler": (run / "compiler-version.txt").read_text().strip(),
        "executable_sha256": checksums[executable.name],
        "source_sha256": {name: digest(run / "build/upstream" / name) for name in
                          ("interpol_mod.f90", "getfields_mod.f90", "windfields_mod.f90", "wetdepo_mod.f90")},
        "driver_sha256": checksums[driver.name],
        **preparation,
    })


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    prep = sub.add_parser("prepare")
    prep.add_argument("directory", type=Path)
    prep.add_argument("checkout", type=Path, help="git object store containing the pinned flex_extract commit")
    done = sub.add_parser("finish")
    done.add_argument("directory", type=Path)
    done.add_argument("output", type=Path)
    done.add_argument("image_id")
    options = parser.parse_args()
    if options.action == "prepare":
        prepare(options.directory, options.checkout)
    else:
        finish(options.directory, options.output, options.image_id)
