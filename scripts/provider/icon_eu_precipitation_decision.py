#!/usr/bin/env python3
"""Decision evidence and policy for ICON-EU GRIB2 precipitation admission (#232).

Research tooling only: it is not a decoder for production, builds no canonical
snapshot and is not imported by production code. It answers the #232 questions
for the DWD ICON-EU regular-lat-lon run accumulations of the #230 sample:

* ``evidence`` decodes every precipitation message of the pinned sample twice,
  with ecCodes and with the independent WMO octet reader plus libaec
  (``icon_eu_grib2_sections``), requires bit-identical values, and records the
  packing keys, the same-run decreases of every component and of the canonical
  LSP/CP sums against the quantization bound, and the TOT_PREC closure.
* ``build`` writes the decision artifact: verdicts, admitted and rejected input
  classes, the exact bound, genuine rows from the evidence and policy controls.
* ``classify_field`` / ``classify_pair`` are the frozen decision predicates.
  ``tests/icon_eu_precipitation_decision.rs`` re-implements them independently.

GRIB2 values follow WMO Regulation 92.9.4 Note 4, ``Y * 10^D = R + X * 2^E``;
template 5.42 applies that simple-packing quantization before lossless CCSDS
coding (WMO template note 83), so CCSDS adds no quantization of its own.
"""

import argparse
import ctypes
import datetime
import hashlib
import json
import math
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import icon_eu_grib2_sections as sections  # noqa: E402  (independent reader, same directory)

DECISION_SCHEMA = "flexpart-gpu.icon-eu-precipitation-decision"
EVIDENCE_SCHEMA = "flexpart-gpu.icon-eu-precipitation-evidence"
SCHEMA_VERSION = 1
ISSUE = 232
CYCLE = "2026103100"
PRODUCT = "icon-eu_europe_regular-lat-lon"
FILE_TEMPLATE = PRODUCT + "_single-level_{cycle}_{step:03d}_{variable}.grib2"
STEPS = (0, 1, 2, 3, 47, 48, 78, 81, 84)
DWD_CENTRE = 78
CCSDS_ENCODING = "grib2_template_5_42_ccsds"
ACCUMULATION = 1
MINUTES = 0
SECONDS_PER_MINUTE = 60
SOURCE_UNIT = "kg m-2"
TOTAL = "TOT_PREC"
# WMO code table 4.2-0-1 parameter numbers (discipline 0, category 1).
PARAMETER_NUMBERS = {
    "TOT_PREC": 52,
    "SNOW_CON": 55,
    "SNOW_GSP": 56,
    "RAIN_CON": 76,
    "RAIN_GSP": 77,
}
COMPOSITION = {
    "large_scale_precipitation": ("RAIN_GSP", "SNOW_GSP"),
    "convective_precipitation": ("RAIN_CON", "SNOW_CON"),
}
COMPONENTS = tuple(name for parts in COMPOSITION.values() for name in parts)
VARIABLES = (TOTAL,) + COMPONENTS
CLOSURE_ANOMALY_CELL_LIMIT = 8


class EvidenceError(RuntimeError):
    """The sample, a decoder or the evidence contradicts the decision inputs."""


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def message_path(sample, variable, step):
    return os.path.join(sample, CYCLE, FILE_TEMPLATE.format(cycle=CYCLE, step=step, variable=variable))


# --- policy (also re-implemented independently in tests/icon_eu_precipitation_decision.rs) ---

def quantum(record):
    """Level spacing ``2^E * 10^-D`` in kg m-2; the run-start anchor is exact (0)."""
    if record["bitsPerValue"] == 0:
        return 0.0
    return math.ldexp(1.0, record["binaryScaleFactor"]) / 10.0 ** record["decimalScaleFactor"]


def packing_tuple(record):
    return (record["referenceValue"], record["binaryScaleFactor"], record["decimalScaleFactor"])


def ulp_f32(value):
    """Spacing from a finite non-negative ``f32`` value to the next larger ``f32``."""
    bits = struct.unpack("<I", struct.pack("<f", value))[0]
    return struct.unpack("<f", struct.pack("<I", bits + 1))[0] - value


def _admitted_packing(record):
    if record["encoding"] != CCSDS_ENCODING or record["bitmap_present"]:
        return False
    if record["decimalScaleFactor"] != 0 or record["referenceValue"] != 0.0:
        return False
    if record["bitsPerValue"] == 0:
        return record["lengthOfTimeRange"] == 0
    return 1 <= record["bitsPerValue"] <= 24


def classify_field(field, observation):
    """Admission of one canonical LSP/CP observation; returns (verdict, code)."""
    expected = COMPOSITION.get(field)
    if expected is None:
        return "REJECT", "unsupported_field"
    records = observation.get("components")
    if not records:
        return "UNRESOLVED", "missing_packing_provenance"
    wanted = sorted((0, 1, PARAMETER_NUMBERS[name]) for name in expected)
    found = sorted((r["discipline"], r["parameterCategory"], r["parameterNumber"]) for r in records)
    if found != wanted:
        return "REJECT", "component_set_mismatch"
    if len({r["message_sha256"] for r in records}) != len(records):
        return "REJECT", "packing_provenance_mismatch"
    if any(r["centre"] != DWD_CENTRE or r["product"] != PRODUCT for r in records):
        return "UNRESOLVED", "unsupported_source_class"
    if any(not _admitted_packing(r) for r in records):
        return "UNRESOLVED", "unsupported_packing"
    if any(r["source_unit"] != SOURCE_UNIT for r in records):
        return "UNRESOLVED", "unsupported_conversion"
    statistics = {(r["productDefinitionTemplateNumber"], r["typeOfStatisticalProcessing"],
                   r["numberOfTimeRanges"], r["indicatorOfUnitOfTimeRange"], r["forecastTime"])
                  for r in records}
    if statistics != {(8, ACCUMULATION, 1, MINUTES, 0)}:
        return "REJECT", "statistical_processing_mismatch"
    runs = {(r["forecast_reference_s"], r["lengthOfTimeRange"]) for r in records}
    if len(runs) != 1:
        return "REJECT", "run_identity_mismatch"
    reference, length_minutes = runs.pop()
    if (observation["reset_s"] != reference
            or observation["valid_time_s"] != reference + SECONDS_PER_MINUTE * length_minutes):
        return "REJECT", "run_identity_mismatch"
    closure = observation.get("closure_verified")
    if closure is None:
        return "UNRESOLVED", "missing_closure_reference"
    if not closure:
        return "REJECT", "component_closure_violation"
    return "ADMIT", "admitted_component_sum"


def admission_bound(previous, current):
    """``B = sum_k (q_prev,k + q_cur,k) + ulp(c_prev)/2 + ulp(c_cur)/2`` over components
    whose (R, E, D) differ; components with identical keys cannot decrease."""
    by_parameter = {r["parameterNumber"]: r for r in current["components"]}
    source = 0.0
    for before in previous["components"]:
        after = by_parameter[before["parameterNumber"]]
        if packing_tuple(before) != packing_tuple(after):
            source += quantum(before) + quantum(after)
    rounding = ulp_f32(previous["canonical_value"]) / 2 + ulp_f32(current["canonical_value"]) / 2
    return source, source + rounding


def classify_pair(field, previous, current):
    """Same-run pair rule; returns (verdict, code, bound or None)."""
    values = (previous.get("canonical_value"), current.get("canonical_value"))
    if field not in COMPOSITION:
        return "REJECT", "unsupported_field", None
    if any(value is None for value in values):
        return "REJECT", "missing_value", None
    if any(not math.isfinite(value) for value in values):
        return "REJECT", "non_finite_value", None
    if any(value < 0 for value in values):
        return "REJECT", "negative_accumulated_amount", None
    times = [previous.get("valid_time_s"), current.get("valid_time_s"),
             previous.get("reset_s"), current.get("reset_s")]
    if any(value is None for value in times):
        return "REJECT", "missing_time_metadata", None
    if times[1] <= times[0]:
        return "REJECT", "non_increasing_valid_time", None
    if times[2] != times[3]:
        return "REJECT", "reset_mismatch", None
    for observation in (previous, current):
        verdict, code = classify_field(field, observation)
        if verdict != "ADMIT":
            return verdict, code, None
    if values[1] >= values[0]:
        return "OUTSIDE_RULE", "ordinary_same_run_interval", None
    source, bound = admission_bound(previous, current)
    if source == 0.0:
        return "REJECT", "identical_packing_parameters", None
    if -(values[1] - values[0]) < bound:
        return "ADMIT_BOUNDED", "quantization_compatible_decrease", bound
    return "REJECT", "decrease_exceeds_quantization_bound", bound


# --- evidence (needs numpy, ecCodes and libaec; run in the pinned container) ---

def load_manifest(path):
    with open(path, encoding="utf-8") as handle:
        manifest = json.load(handle)
    return {os.path.basename(entry["path"]).removesuffix(".bz2"): entry for entry in manifest["files"]}


def load_audit_rows(path):
    with open(path, encoding="utf-8") as handle:
        evidence = json.load(handle)
    return {os.path.basename(row["path"]): row for row in evidence["rows"]}


def decode_integers(message, libaec, numpy):
    """Packed integers X of one template-5.42 message, decoded by libaec only."""
    found = sections.sections(message)
    rep = sections.parse_representation(found[5])
    count, bits = rep["number_of_values"], rep["bits_per_value"]
    if bits == 0:
        return numpy.zeros(count, dtype=numpy.int64)
    if bits > 16 or rep["ccsds_flags"] & sections.AEC_DATA_SIGNED:
        raise EvidenceError("only unsigned samples of at most 16 bits are in the audited class")
    packed = bytes(found[7][5:])
    output = ctypes.create_string_buffer(count * 2)
    source = ctypes.create_string_buffer(packed, len(packed))
    stream = sections._AecStream()
    stream.next_in = ctypes.cast(source, ctypes.c_void_p)
    stream.avail_in = len(packed)
    stream.next_out = ctypes.cast(output, ctypes.c_void_p)
    stream.avail_out = count * 2
    stream.bits_per_sample = bits
    stream.block_size = rep["ccsds_block_size"]
    stream.rsi = rep["ccsds_rsi"]
    stream.flags = rep["ccsds_flags"] & ~(sections.AEC_DATA_3BYTE | sections.AEC_DATA_MSB)
    status = libaec.aec_buffer_decode(ctypes.byref(stream))
    if status != 0 or stream.total_out != count * 2:
        raise EvidenceError(f"libaec decode failed: status {status}")
    return numpy.frombuffer(output.raw, dtype="<u2").astype(numpy.int64)


def epoch_seconds(timestamp):
    parsed = datetime.datetime.strptime(timestamp, "%Y-%m-%dT%H:%M:%SZ")
    return int(parsed.replace(tzinfo=datetime.timezone.utc).timestamp())


def packing_record(parsed, digest):
    rep, product = parsed["representation"], parsed["product"]
    return {
        "encoding": CCSDS_ENCODING,
        "message_sha256": digest,
        "centre": parsed["identification"]["centre"],
        "product": PRODUCT,
        "discipline": parsed["discipline"],
        "parameterCategory": product["parameter_category"],
        "parameterNumber": product["parameter_number"],
        "productDefinitionTemplateNumber": product["template"],
        "typeOfStatisticalProcessing": product.get("statistical_processing"),
        "numberOfTimeRanges": product.get("time_range_count"),
        "indicatorOfUnitOfTimeRange": product["unit_of_time_range"],
        "forecastTime": product["forecast_time"],
        "lengthOfTimeRange": product.get("statistical_range_length"),
        "forecast_reference_s": epoch_seconds(parsed["identification"]["reference_time"]),
        "referenceValue": rep["reference_value"],
        "binaryScaleFactor": rep["binary_scale_factor"],
        "decimalScaleFactor": rep["decimal_scale_factor"],
        "bitsPerValue": rep["bits_per_value"],
        "bitmap_present": parsed["bitmap_indicator"] != sections.NO_BITMAP,
        "source_unit": SOURCE_UNIT,
    }


def eccodes_values(message, eccodes, numpy):
    handle = eccodes.codes_new_from_message(message)
    try:
        values = numpy.asarray(eccodes.codes_get_double_array(handle, "values"), dtype=numpy.float64)
        labels = {key: eccodes.codes_get_string(handle, key)
                  for key in ("shortName", "paramId", "units", "packingType", "stepRange", "stepUnits")}
    finally:
        eccodes.codes_release(handle)
    return values, labels


def read_messages(sample, manifest, audit_rows, eccodes, libaec, numpy):
    messages = {}
    for variable in VARIABLES:
        for step in STEPS:
            path = message_path(sample, variable, step)
            name = os.path.basename(path)
            with open(path, "rb") as handle:
                message = handle.read()
            digest = sha256_bytes(message)
            entry = manifest.get(name)
            if entry is None or entry["grib2_sha256"] != digest:
                raise EvidenceError(f"{name}: bytes differ from the pinned retrieval manifest")
            parsed = sections.parse(message)
            record = packing_record(parsed, digest)
            if (record["parameterNumber"] != PARAMETER_NUMBERS[variable]
                    or record["lengthOfTimeRange"] != SECONDS_PER_MINUTE * step):
                raise EvidenceError(f"{name}: parameter or accumulation length differs from the file name")
            integers = decode_integers(message, libaec, numpy)
            q = quantum(record)
            independent = record["referenceValue"] + integers * (q if q else 1.0)
            values, labels = eccodes_values(message, eccodes, numpy)
            identical = bool(numpy.array_equal(values, independent))
            if not identical:
                raise EvidenceError(f"{name}: ecCodes and libaec values differ")
            values_hash = sha256_bytes(values.astype("<f8").tobytes())
            audit = audit_rows.get(name)
            if audit is None or audit["values"]["values_sha256_f8le"] != values_hash:
                raise EvidenceError(f"{name}: values differ from the #230 two-version evidence")
            messages[(variable, step)] = {
                "record": record, "grid": parsed["grid"], "integers": integers, "values": values,
                "summary": {
                    "variable": variable, "step": step, "file": name, "bytes": len(message),
                    "packing": record, "quantum_kg_m2": q,
                    "max_packed_integer": int(integers.max()),
                    "max_kg_m2": float(values.max()),
                    "nonzero_cells": int((integers != 0).sum()),
                    "decoders_bit_identical": identical,
                    "values_sha256_f8le": values_hash,
                    "values_sha256_matches_230_evidence": True,
                    "values_identical_eccodes_2_24_2_and_2_48_0_per_230": audit["values_identical_across_decoders"],
                    "eccodes_labels": {"2.24.2": labels, "2.48.0": audit["labels"]["secondary"]},
                },
            }
    return messages


def component_pair(numpy, before, after):
    q_before, q_after = quantum(before["record"]), quantum(after["record"])
    identical = packing_tuple(before["record"]) == packing_tuple(after["record"])
    delta = after["values"] - before["values"]
    decreasing = delta < 0
    bound = 0.0 if identical else q_before + q_after
    most_negative = float(delta.min())
    multiples = {}
    if decreasing.any():
        unit = min(q for q in (q_before, q_after) if q > 0)
        counts = numpy.unique(numpy.rint(-delta[decreasing] / unit).astype(numpy.int64), return_counts=True)
        multiples = {str(int(k)): int(v) for k, v in zip(*counts)}
    return {
        "variable": before["summary"]["variable"],
        "from_step": before["summary"]["step"],
        "to_step": after["summary"]["step"],
        "identical_packing": identical,
        "previous_quantum_kg_m2": q_before,
        "current_quantum_kg_m2": q_after,
        "decreasing_cells": int(decreasing.sum()),
        "most_negative_delta_kg_m2": most_negative,
        "source_bound_kg_m2": bound,
        "round_to_nearest_nested_bound_kg_m2": 0.0 if identical else max(q_before, q_after) / 2,
        "decrease_in_smallest_quantum_multiples": multiples,
        "within_source_bound": bool(-most_negative < bound) if decreasing.any() else True,
    }


def canonical_sum(numpy, messages, parts, step):
    """Canonical ``f32`` amount: exact ``f64`` component sum, one round-to-nearest."""
    total = messages[(parts[0], step)]["values"] + messages[(parts[1], step)]["values"]
    return total.astype(numpy.float32)


def composite_pair(numpy, messages, field, parts, before_step, after_step):
    c_before = canonical_sum(numpy, messages, parts, before_step)
    c_after = canonical_sum(numpy, messages, parts, after_step)
    source = 0.0
    for part in parts:
        before, after = messages[(part, before_step)]["record"], messages[(part, after_step)]["record"]
        if packing_tuple(before) != packing_tuple(after):
            source += quantum(before) + quantum(after)
    delta = c_after.astype(numpy.float64) - c_before.astype(numpy.float64)
    bound = (source + numpy.spacing(c_before).astype(numpy.float64) / 2
             + numpy.spacing(c_after).astype(numpy.float64) / 2)
    decreasing = delta < 0
    result = {
        "field": field, "from_step": before_step, "to_step": after_step,
        "source_bound_kg_m2": source,
        "decreasing_cells": int(decreasing.sum()),
        "violating_cells": int((decreasing & (-delta >= bound)).sum()),
        "most_negative_delta_kg_m2": float(delta.min()),
        "max_decrease_to_bound_ratio": float((-delta[decreasing] / bound[decreasing]).max()) if decreasing.any() else 0.0,
        "genuine_row": None,
    }
    if decreasing.any():
        index = int(numpy.argmin(delta))
        result["genuine_row"] = {
            "cell": index,
            "previous": genuine_observation(messages, parts, before_step, float(c_before[index]), index),
            "current": genuine_observation(messages, parts, after_step, float(c_after[index]), index),
        }
    return result


def genuine_observation(messages, parts, step, canonical, index):
    first = messages[(parts[0], step)]["record"]
    return {
        "canonical_value": canonical,
        "valid_time_s": first["forecast_reference_s"] + SECONDS_PER_MINUTE * first["lengthOfTimeRange"],
        "reset_s": first["forecast_reference_s"],
        "closure_verified": True,
        "component_values_kg_m2": {part: float(messages[(part, step)]["values"][index]) for part in parts},
        "components": [messages[(part, step)]["record"] for part in parts],
    }


def closure(numpy, messages, step):
    total = messages[(TOTAL, step)]
    residual = total["values"] - sum(messages[(name, step)]["values"] for name in COMPONENTS)
    quanta = [quantum(messages[(name, step)]["record"]) for name in VARIABLES]
    bound = sum(quanta)
    beyond = numpy.flatnonzero(numpy.abs(residual) > bound)
    grid = total["grid"]
    if grid["scanning_mode"] != 64:
        raise EvidenceError("closure cell coordinates assume south-to-north scanning (mode 64)")
    cells = []
    for index in beyond[:CLOSURE_ANOMALY_CELL_LIMIT]:
        row, column = divmod(int(index), grid["ni"])
        cells.append({
            "cell": int(index), "row": row, "column": column,
            "latitude_deg": round(grid["first_lat_deg"] + row * grid["dj_deg"], 6),
            "longitude_deg": round((grid["first_lon_deg"] + column * grid["di_deg"] + 180.0) % 360.0 - 180.0, 6),
            "residual_kg_m2": float(residual[index]),
            "total_kg_m2": float(total["values"][index]),
            "components_kg_m2": {name: float(messages[(name, step)]["values"][index]) for name in COMPONENTS},
        })
    return {
        "step": step,
        "bound_kg_m2": bound,
        "half_quantum_bound_kg_m2": bound / 2,
        "min_residual_kg_m2": float(residual.min()),
        "max_residual_kg_m2": float(residual.max()),
        "cells_beyond_bound": int(beyond.size),
        "cells_beyond_bound_positive": int((residual[beyond] > 0).sum()),
        "cells_beyond_half_quantum_bound": int((numpy.abs(residual) > bound / 2).sum()),
        "closure_verified": bool(beyond.size == 0),
        "cells": cells,
    }


def build_evidence(sample, manifest_path, audit_path):
    import numpy
    eccodes = __import__("eccodes")
    libaec = sections.load_libaec()
    if libaec is None:
        raise EvidenceError("libaec is required for the independent decoder")
    manifest = load_manifest(manifest_path)
    messages = read_messages(sample, manifest, load_audit_rows(audit_path), eccodes, libaec, numpy)
    pairs = list(zip(STEPS, STEPS[1:]))
    return {
        "schema": EVIDENCE_SCHEMA,
        "version": SCHEMA_VERSION,
        "issue": ISSUE,
        "status": "DERIVED_EVIDENCE: the raw GRIB2 bytes identified by message_sha256 are authoritative",
        "cycle": CYCLE,
        "steps": list(STEPS),
        "retrieval_manifest_sha256": sha256_file(manifest_path),
        "audit_evidence_sha256": sha256_file(audit_path),
        "decoders": {
            "eccodes": eccodes.codes_get_api_version(),
            "independent": "scripts/provider/icon_eu_grib2_sections.py octets plus libaec aec_buffer_decode",
            "third_by_hash": "ecCodes 2.48.0 values via #230 values_sha256_f8le",
        },
        "messages": [messages[(variable, step)]["summary"] for variable in VARIABLES for step in STEPS],
        "component_pairs": [component_pair(numpy, messages[(name, a)], messages[(name, b)])
                            for name in VARIABLES for a, b in pairs],
        "canonical_pairs": [composite_pair(numpy, messages, field, parts, a, b)
                            for field, parts in COMPOSITION.items() for a, b in pairs],
        "closure": [closure(numpy, messages, step) for step in STEPS],
    }


def sha256_file(path):
    with open(path, "rb") as handle:
        return sha256_bytes(handle.read().replace(b"\r\n", b"\n"))


# --- decision artifact ---

def mutate(observation, **changes):
    result = json.loads(json.dumps(observation))
    for key, value in changes.items():
        result[key] = value
    return result


def mutate_component(observation, index, **changes):
    result = json.loads(json.dumps(observation))
    result["components"][index].update(changes)
    return result


def genuine_rows(evidence):
    rows = []
    for pair in evidence["canonical_pairs"]:
        if pair["genuine_row"] is None:
            continue
        row = pair["genuine_row"]
        rows.append({
            "name": f"{pair['field']} {pair['from_step']}->{pair['to_step']} h cell {row['cell']}",
            "field": pair["field"], "previous": row["previous"], "current": row["current"],
        })
    return rows


def control_cases(evidence):
    """Policy controls; each mutates one genuine pair or observation."""
    lsp = next(r for r in genuine_rows(evidence)
               if r["field"] == "large_scale_precipitation" and "1->2" in r["name"])
    field, previous, current = lsp["field"], lsp["previous"], lsp["current"]
    _, bound = admission_bound(previous, current)
    level = previous["canonical_value"]
    step = ulp_f32(level)
    cases = [
        ("genuine LSP 1->2 decrease", field, previous, current),
        ("flat pair is an ordinary interval", field, previous, mutate(current, canonical_value=level)),
        ("decrease just inside B", field, previous,
         mutate(current, canonical_value=largest_f32_above(level - bound))),
        ("decrease equal to or beyond B", field, previous,
         mutate(current, canonical_value=smallest_f32_at_or_below(level - bound))),
        ("twice the bound", field, previous, mutate(current, canonical_value=level - 2 * bound)),
        ("hidden restart to zero", field, previous, mutate(current, canonical_value=0.0)),
        ("identical packing in both components", field, previous,
         mutate_component(mutate_component(current, 0, binaryScaleFactor=previous["components"][0]["binaryScaleFactor"]),
                          1, binaryScaleFactor=previous["components"][1]["binaryScaleFactor"])),
        ("unsupported field", "surface_pressure", previous, current),
        ("missing current value", field, previous, mutate(current, canonical_value=None)),
        ("non-finite current value", field, previous, mutate(current, canonical_value=float("nan"))),
        ("negative amount", field, previous, mutate(current, canonical_value=-step)),
        ("missing valid time", field, previous, mutate(current, valid_time_s=None)),
        ("reversed valid time", field, previous, mutate(current, valid_time_s=previous["valid_time_s"])),
        ("reset differs", field, previous, mutate(current, reset_s=previous["reset_s"] + 3600)),
        ("missing packing provenance", field, previous, mutate(current, components=None)),
        ("missing snow component", field, previous, mutate(current, components=current["components"][:1])),
        ("convective component in LSP", field, previous, mutate_component(current, 1, parameterNumber=55)),
        ("graupel in place of snow", field, previous, mutate_component(current, 1, parameterNumber=75)),
        ("duplicate message", field, previous,
         mutate_component(current, 1, message_sha256=current["components"][0]["message_sha256"])),
        ("other centre", field, previous, mutate_component(current, 0, centre=7)),
        ("other DWD product (ICON-D2)", field, previous,
         mutate_component(current, 0, product="icon-d2_germany_regular-lat-lon")),
        ("simple packing template 5.0", field, previous,
         mutate_component(current, 0, encoding="grib2_template_5_0_simple")),
        ("bitmap present", field, previous, mutate_component(current, 0, bitmap_present=True)),
        ("decimal scale factor", field, previous, mutate_component(current, 0, decimalScaleFactor=1)),
        ("non-zero reference value", field, previous, mutate_component(current, 0, referenceValue=0.5)),
        ("constant field after run start", field, previous, mutate_component(current, 0, bitsPerValue=0)),
        ("rate unit claimed", field, previous, mutate_component(current, 0, source_unit="kg m-2 s-1")),
        ("average instead of accumulation", field, previous,
         mutate_component(current, 0, typeOfStatisticalProcessing=0)),
        ("hour time unit", field, previous, mutate_component(current, 0, indicatorOfUnitOfTimeRange=1)),
        ("accumulation not from run start", field, previous, mutate_component(current, 0, forecastTime=60)),
        ("components of different steps", field, previous,
         mutate_component(current, 0, lengthOfTimeRange=previous["components"][0]["lengthOfTimeRange"])),
        ("valid time not reference plus length", field, previous,
         mutate(current, valid_time_s=current["valid_time_s"] + 60)),
        ("reset not the run start", field, mutate(previous, reset_s=previous["reset_s"] - 3600),
         mutate(current, reset_s=current["reset_s"] - 3600)),
        ("closure violated", field, previous, mutate(current, closure_verified=False)),
        ("closure reference absent", field, previous, mutate(current, closure_verified=None)),
    ]
    controls = []
    for name, case_field, before, after in cases:
        verdict, code, case_bound = classify_pair(case_field, before, after)
        controls.append({"name": name, "field": case_field, "previous": before, "current": after,
                         "expected_verdict": verdict, "expected_code": code, "bound_kg_m2": case_bound})
    return controls


def largest_f32_above(value):
    candidate = struct.unpack("<f", struct.pack("<f", value))[0]
    while candidate <= value:
        candidate += ulp_f32(candidate)
    return candidate


def smallest_f32_at_or_below(value):
    candidate = struct.unpack("<f", struct.pack("<f", value))[0]
    if candidate > value:
        bits = struct.unpack("<I", struct.pack("<f", candidate))[0]
        candidate = struct.unpack("<f", struct.pack("<I", bits - 1))[0]
    return candidate


DERIVED_DECISION_KEYS = ("evidence_sha256", "sample_step_admission", "genuine_rows", "controls")


def build_decision(evidence, evidence_path, decision):
    """Return ``decision`` with its evidence-derived keys regenerated.

    The verdicts, classes, references and handoff are authored in the committed
    artifact; only the keys in ``DERIVED_DECISION_KEYS`` are recomputed here.
    """
    rows = []
    for row in genuine_rows(evidence):
        verdict, code, bound = classify_pair(row["field"], row["previous"], row["current"])
        rows.append(dict(row, verdict=verdict, code=code, bound_kg_m2=bound))
    result = {key: value for key, value in decision.items() if key not in DERIVED_DECISION_KEYS}
    result.update({
        "schema": DECISION_SCHEMA,
        "version": SCHEMA_VERSION,
        "issue": ISSUE,
        "evidence_sha256": sha256_file(evidence_path),
        "sample_step_admission": {str(entry["step"]): "ADMIT" if entry["closure_verified"] else "REJECT"
                                  for entry in evidence["closure"]},
        "genuine_rows": rows,
        "controls": control_cases(evidence),
    })
    return result


def write_json(path, payload):
    text = json.dumps(payload, indent=1, sort_keys=True, allow_nan=False, default=_reject)
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text + "\n")


def _reject(value):
    raise TypeError(f"not JSON serializable: {value!r}")


def _json_safe(payload):
    """Encode NaN control values as the string "NaN" (JSON has no NaN)."""
    if isinstance(payload, float) and not math.isfinite(payload):
        return "NaN"
    if isinstance(payload, dict):
        return {key: _json_safe(value) for key, value in payload.items()}
    if isinstance(payload, list):
        return [_json_safe(value) for value in payload]
    return payload


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    evidence_parser = commands.add_parser("evidence")
    evidence_parser.add_argument("--sample", required=True, help="#230 sample root holding 2026103100/")
    evidence_parser.add_argument("--manifest", required=True)
    evidence_parser.add_argument("--audit-evidence", required=True)
    evidence_parser.add_argument("--out", required=True)
    build_parser = commands.add_parser("build")
    build_parser.add_argument("--evidence", required=True)
    build_parser.add_argument("--decision", required=True, help="decision artifact, rewritten in place")
    args = parser.parse_args(argv)
    try:
        if args.command == "evidence":
            write_json(args.out, build_evidence(args.sample, args.manifest, args.audit_evidence))
        else:
            with open(args.evidence, encoding="utf-8") as handle:
                evidence = json.load(handle)
            with open(args.decision, encoding="utf-8") as handle:
                decision = json.load(handle)
            write_json(args.decision, _json_safe(build_decision(evidence, args.evidence, decision)))
    except (EvidenceError, sections.SectionError, OSError, KeyError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
