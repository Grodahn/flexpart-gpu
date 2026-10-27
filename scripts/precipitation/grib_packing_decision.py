#!/usr/bin/env python3
"""Decide #75/#90 admission of GRIB-packing same-run accumulation decreases (#216).

Research tooling for issue #216; it changes no production behaviour. Two modes:

* ``eccodes-keys`` runs inside the pinned ``flex-extract`` container
  (ecCodes 2.24.2, the decoder of the #202 trace) and dumps the packing keys and
  decoded values of every message of the committed #202 accumulated GRIB input.
* ``build`` uses the Python standard library only. It independently parses the
  same GRIB1 bytes (sections 0-5, WMO FM 92 edition 1, grid-point simple
  packing), reproduces the ecCodes values bit for bit, rebuilds the encoder
  input from the declared #202 interval amounts, recomputes the pinned
  flex_extract ``deacc_fluxes``/``darain`` arithmetic, the canonical ``f32``
  values of the #207 harness and the #75 rejection points, derives the
  quantization bound from the actual message metadata and writes the decision
  artifact with its negative controls.

See docs/research/grib-packing-accumulation-216.md.
"""

import argparse
import copy
import datetime as dt
import hashlib
import json
import math
import os
import struct
import sys
from fractions import Fraction

SCHEMA = "flexpart-gpu/grib-packing-decrease-decision/v1"
GRIB_DIR = "fixtures/accumulation/precipitation-202-accumulated-grib"
MAPPING_DECISION = "fixtures/accumulation/precipitation-member-mapping-v1.json"
MAPPING_EVIDENCE = "fixtures/accumulation/precipitation-member-mapping-evidence-v1.json"
DARAIN_ORACLE = "fixtures/accumulation/precipitation-darain-oracle-v1.json"
ECCODES_KEYS_PATH = "fixtures/accumulation/grib-packing-eccodes-keys-v1.json"
FLEX_EXTRACT_PIN = "e0005c99ac81d12faa45a8ff799debbd592b0dc0"
ECFLEXPART_BLOB = "65ba97f7e4212e4187accda307d79436bee23231"
FLEXPART_PIN = "c70586c2b7f5258850705325881c61f557ea9bd8"
FLEX_EXTRACT_IMAGE = "sha256:717f14dfd25610d2cc54624243610e8f654ba3a746ea973fd2b33f9841f288ec"
PARAMS = {"lsp": 142, "cp": 143}
NX = 3
NY = 3
DTIME_HOURS = 3
DTIME_SECONDS = DTIME_HOURS * 3600
EPOCH = dt.datetime(2023, 11, 1, 0)
# GRIB1 param 142/143 (ECMWF table 128) are metres of water equivalent;
# 1 m water = 1000 kg/m2 at the canonical boundary.
CANONICAL_PER_SOURCE_UNIT = 1000
# flex_extract deacc_fluxes: values / fak with fak = 1/1000 (EcFlexpart.py:1123-1131);
# the #207 harness builds canonical f32 values by the same quotient.
FLEX_EXTRACT_FAK = 1.0 / 1000.0
# Canonical conversion error term (see docs): with D = 0 the decode adds R once and
# the unit scale costs at most two more f64 roundings; their combined relative error
# 3 * 2**-53 stays below 2**-50 of (|c| + ulp(c)), which also covers c = 0.
F64_PATH_RELATIVE = Fraction(1, 2 ** 50)
# Declared canonical conversion of the supported class: IEEE double decode, unit
# scale by 1000 (or division by 0.001) in double, one round-to-nearest to f32.
CANONICAL_CONVERSION = "f64_decode_unit_scale_then_f32_nearest"

ECCODES_KEYS = [
    "editionNumber", "centre", "table2Version", "indicatorOfParameter", "paramId",
    "shortName", "units", "dataDate", "dataTime", "stepUnits", "stepType",
    "timeRangeIndicator", "startStep", "endStep", "gridType", "Ni", "Nj",
    "latitudeOfFirstGridPointInDegrees", "longitudeOfFirstGridPointInDegrees",
    "latitudeOfLastGridPointInDegrees", "longitudeOfLastGridPointInDegrees",
    "iDirectionIncrementInDegrees", "jDirectionIncrementInDegrees", "jScansPositively",
    "packingType", "bitsPerValue", "referenceValue", "binaryScaleFactor",
    "decimalScaleFactor", "bitmapPresent", "numberOfValues", "numberOfCodedValues",
    "numberOfMissing", "missingValue",
]


def fail(message):
    sys.exit("grib packing decision failed: " + message)


def require(condition, message):
    if not condition:
        fail(message)


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def sha256_file(path):
    with open(path, "rb") as handle:
        return sha256_bytes(handle.read())


def load_json(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def grib_files(grib_dir):
    names = sorted(n for n in os.listdir(grib_dir) if n.startswith("FCOG_acc_SL."))
    require(len(names) == 4, "expected the four #202 accumulated GRIB inputs")
    return [(name, os.path.join(grib_dir, name)) for name in names]


# --- eccodes-keys mode (pinned container) --------------------------------------

def dump_eccodes_keys(grib_dir, out_path):
    import eccodes

    files = {}
    for name, path in grib_files(grib_dir):
        messages = []
        with open(path, "rb") as handle:
            while True:
                gid = eccodes.codes_grib_new_from_file(handle)
                if gid is None:
                    break
                keys = {}
                for key in ECCODES_KEYS:
                    try:
                        native = eccodes.codes_get_native_type(gid, key)
                        if native is float:
                            keys[key] = eccodes.codes_get_double(gid, key)
                        elif native is int:
                            keys[key] = eccodes.codes_get_long(gid, key)
                        else:
                            keys[key] = eccodes.codes_get_string(gid, key)
                    except eccodes.KeyValueNotFoundError:
                        keys[key] = None
                keys["values"] = [float(v) for v in eccodes.codes_get_values(gid)]
                messages.append(keys)
                eccodes.codes_release(gid)
        files[name] = {"sha256": sha256_file(path), "messages": messages}
    record = {
        "schema": "flexpart-gpu/grib-packing-eccodes-keys/v1",
        "issue": 216,
        "eccodes": eccodes.codes_get_api_version(),
        "files": files,
    }
    with open(out_path, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(record, handle, indent=1, sort_keys=True)
        handle.write("\n")
    print(out_path, sha256_file(out_path))


# --- independent GRIB1 decoding (standard library) -----------------------------

def u24(data, pos):
    return int.from_bytes(data[pos:pos + 3], "big")


def sign_magnitude(raw, bits):
    sign_bit = 1 << (bits - 1)
    magnitude = raw & (sign_bit - 1)
    return -magnitude if raw & sign_bit else magnitude


def ibm_single(word):
    """Exact value of a GRIB1 IBM System/360 single-precision reference value."""
    sign = -1 if word & 0x80000000 else 1
    exponent = (word >> 24) & 0x7F
    fraction = Fraction(word & 0x00FFFFFF, 1 << 24)
    return sign * fraction * Fraction(16) ** (exponent - 64)


def unpack_unsigned(data, bits, count):
    stream = int.from_bytes(data, "big")
    total = len(data) * 8
    require(count * bits <= total, "BDS shorter than declared values")
    mask = (1 << bits) - 1
    return [(stream >> (total - (k + 1) * bits)) & mask for k in range(count)]


def parse_grib1_message(message, file_name, index):
    require(message[:4] == b"GRIB" and message[-4:] == b"7777", "GRIB framing")
    require(message[7] == 1, "only GRIB edition 1 is parsed")
    pos = 8
    pds_len = u24(message, pos)
    pds = message[pos:pos + pds_len]
    pos += pds_len
    flag = pds[7]
    has_gds, has_bms = bool(flag & 0x80), bool(flag & 0x40)
    century = pds[24]
    year = (century - 1) * 100 + pds[12]
    pds_keys = {
        "pds_octets": pds_len,
        "table2Version": pds[3],
        "centre": pds[4],
        "gds_present": has_gds,
        "bitmap_present": has_bms,
        "indicatorOfParameter": pds[8],
        "indicatorOfTypeOfLevel": pds[9],
        "reference_time": [year, pds[13], pds[14], pds[15], pds[16]],
        "unitOfTimeRange": pds[17],
        "P1": pds[18],
        "P2": pds[19],
        "timeRangeIndicator": pds[20],
        "decimalScaleFactor": sign_magnitude(int.from_bytes(pds[26:28], "big"), 16),
    }
    require(has_gds and not has_bms, "supported #202 messages carry a GDS and no bitmap")
    gds_len = u24(message, pos)
    gds = message[pos:pos + gds_len]
    pos += gds_len
    grid = {
        "dataRepresentationType": gds[5],
        "Ni": int.from_bytes(gds[6:8], "big"),
        "Nj": int.from_bytes(gds[8:10], "big"),
        "La1_millideg": sign_magnitude(u24(gds, 10), 24),
        "Lo1_millideg": sign_magnitude(u24(gds, 13), 24),
        "La2_millideg": sign_magnitude(u24(gds, 17), 24),
        "Lo2_millideg": sign_magnitude(u24(gds, 20), 24),
        "Di_millideg": int.from_bytes(gds[23:25], "big"),
        "Dj_millideg": int.from_bytes(gds[25:27], "big"),
        "scanningMode": gds[27],
    }
    bds_len = u24(message, pos)
    bds = message[pos:pos + bds_len]
    pos += bds_len
    require(message[pos:pos + 4] == b"7777" and pos + 4 == len(message), "section 5")
    bds_flag = bds[3] >> 4
    unused_bits = bds[3] & 0x0F
    binary_scale = sign_magnitude(int.from_bytes(bds[4:6], "big"), 16)
    reference_word = int.from_bytes(bds[6:10], "big")
    bits = bds[10]
    count = grid["Ni"] * grid["Nj"]
    packed = unpack_unsigned(bds[11:], bits, count)
    require(len(bds[11:]) * 8 - unused_bits >= count * bits, "BDS unused-bit count")
    return {
        "file": file_name,
        "index": index,
        "message_sha256": sha256_bytes(message),
        "message_octets": len(message),
        "pds": pds_keys,
        "grid": grid,
        "bds": {
            "octets": bds_len,
            "flag_bits": bds_flag,
            "spherical_harmonics": bool(bds_flag & 0x8),
            "second_order_packing": bool(bds_flag & 0x4),
            "integer_values": bool(bds_flag & 0x2),
            "additional_flags": bool(bds_flag & 0x1),
            "unused_bits": unused_bits,
            "binaryScaleFactor": binary_scale,
            "referenceValue_ibm_hex": "{:08x}".format(reference_word),
            "referenceValue": ibm_single(reference_word),
            "bitsPerValue": bits,
        },
        "packed": packed,
    }


def parse_grib1_file(path, name):
    with open(path, "rb") as handle:
        data = handle.read()
    messages = []
    pos = 0
    while pos < len(data):
        require(data[pos:pos + 4] == b"GRIB", name + ": message start")
        total = u24(data, pos + 4)
        messages.append(parse_grib1_message(data[pos:pos + total], name, len(messages)))
        pos += total
    return messages


def quantum(e, d):
    """GRIB simple-packing level spacing 2**E * 10**-D in source units (exact)."""
    return Fraction(2) ** e / Fraction(10) ** d


def decode_exact(message, x):
    b = message["bds"]
    return (b["referenceValue"] + x * Fraction(2) ** b["binaryScaleFactor"]) / \
        Fraction(10) ** message["pds"]["decimalScaleFactor"]


def decode_eccodes_double(message, x):
    """ecCodes grib_decode_double_array: (x * 2**E + R) * 10**-D in double."""
    b = message["bds"]
    require(message["pds"]["decimalScaleFactor"] == 0, "the #202 example uses D = 0")
    return float(x) * math.ldexp(1.0, b["binaryScaleFactor"]) + float(b["referenceValue"])


def encode_ecmwf_nearest(message, value):
    """ecCodes simple-packing integer: (unsigned long)(((v*10**D) - R) * 2**-E + 0.5)."""
    b = message["bds"]
    scaled = (value - float(b["referenceValue"])) * math.ldexp(1.0, -b["binaryScaleFactor"])
    return int(math.floor(scaled + 0.5))


def grib_index(i, j):
    """GRIB north-to-south row-major index of FLEXPART node (i, j), j = 0 south."""
    return (NY - 1 - j) * NX + i


def canonical_cell(i, j):
    return j * NX + i


def f32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def f32_hex(value):
    return "{:08x}".format(struct.unpack("<I", struct.pack("<f", value))[0])


def f64_hex(value):
    return "{:016x}".format(struct.unpack("<Q", struct.pack("<d", value))[0])


def f32_ulp(value):
    """Gap from |value| to the next larger f32 (exact)."""
    magnitude = abs(value)
    bits = struct.unpack("<I", struct.pack("<f", magnitude))[0]
    above = struct.unpack("<f", struct.pack("<I", bits + 1))[0]
    return Fraction(above) - Fraction(magnitude)


def f32_below(value, steps):
    bits = struct.unpack("<I", struct.pack("<f", value))[0]
    return struct.unpack("<f", struct.pack("<I", bits - steps))[0]


def frac_json(value):
    return {"float": float(value), "exact": "{}/{}".format(value.numerator, value.denominator)}


def epoch_seconds(year, month, day, hour, minute):
    stamp = dt.datetime(year, month, day, hour, minute)
    return int((stamp - EPOCH).total_seconds())


# --- policy (also re-implemented independently in tests/grib_packing_decision.rs) ---

SUPPORTED_FIELDS = ("large_scale_precipitation", "convective_precipitation")
# GRIB1 ECMWF table 128 parameter of each supported canonical field.
FIELD_PARAM_IDS = {"large_scale_precipitation": 142, "convective_precipitation": 143}
# Strict JSON has no NaN/Infinity; controls spell non-finite values as strings.
NON_FINITE_TOKENS = {"NaN": math.nan, "Infinity": math.inf, "-Infinity": -math.inf}


def canonical_number(value):
    return NON_FINITE_TOKENS[value] if isinstance(value, str) else value


def canonical_bound(pair):
    """B = s (q_prev + q_cur) + r_prev + r_cur in canonical kg/m2 (exact)."""
    terms = []
    for side in ("previous", "current"):
        q = pair[side]["packing"]
        scale = Fraction(q["canonical_per_source_unit"])
        c = Fraction(pair[side]["canonical_value"])
        ulp = f32_ulp(pair[side]["canonical_value"])
        terms.append(scale * quantum(q["binaryScaleFactor"], q["decimalScaleFactor"])
                     + ulp / 2 + F64_PATH_RELATIVE * (abs(c) + ulp))
    return terms[0] + terms[1]


def classify_structure(pair):
    """Identity, value, time and reset checks shared by policies A, B and C.

    Returns ``("DECREASE", d)`` with the exact canonical difference when the
    pair is a well-formed same-run decrease, otherwise the final verdict.
    """
    prev, cur = pair["previous"], pair["current"]
    if pair["field"] not in SUPPORTED_FIELDS:
        return "REJECT", "unsupported_field"
    if pair["canonical_unit"] != "kilogram_per_square_meter":
        return "REJECT", "unit_mismatch"
    if prev["grid_id"] != cur["grid_id"] or prev["source_id"] != cur["source_id"]:
        return "REJECT", "identity_mismatch"
    for side in (prev, cur):
        if side["canonical_value"] is None:
            return "REJECT", "missing_value"
        value = canonical_number(side["canonical_value"])
        if not math.isfinite(value):
            return "REJECT", "non_finite_value"
        if value < 0.0:
            return "REJECT", "negative_accumulated_amount"
    if prev["valid_time_s"] is None or cur["valid_time_s"] is None:
        return "REJECT", "missing_time_metadata"
    if cur["valid_time_s"] <= prev["valid_time_s"]:
        return "REJECT", "non_increasing_valid_time"
    if prev["reset_s"] is None or cur["reset_s"] is None:
        return "REJECT", "missing_time_metadata"
    if cur["reset_s"] == prev["valid_time_s"] and cur["reset_s"] > prev["reset_s"]:
        return "OUTSIDE_RULE", "declared_contiguous_reset"
    if cur["reset_s"] != prev["reset_s"] or cur["reset_s"] >= prev["valid_time_s"]:
        return "REJECT", "reset_mismatch"
    difference = Fraction(cur["canonical_value"]) - Fraction(prev["canonical_value"])
    if difference >= 0:
        return "OUTSIDE_RULE", "ordinary_same_run_interval"
    return "DECREASE", difference


def classify(pair):
    """Policy B: return (verdict, code) for one declared consecutive pair."""
    verdict, difference = classify_structure(pair)
    if verdict != "DECREASE":
        return verdict, difference
    prev, cur = pair["previous"], pair["current"]
    for side in (prev, cur):
        packing = side["packing"]
        if packing is None:
            return "UNRESOLVED", "missing_packing_provenance"
        if (packing["encoding"] != "grib1_grid_simple" or packing["bitsPerValue"] < 1
                or packing["bitmap_present"] or packing["decimalScaleFactor"] != 0):
            return "UNRESOLVED", "unsupported_packing"
        if packing["canonical_conversion"] != CANONICAL_CONVERSION:
            return "UNRESOLVED", "unsupported_conversion"
        if (packing["message_sha256"] != side["source_message_sha256"]
                or packing["paramId"] != FIELD_PARAM_IDS[pair["field"]]):
            return "REJECT", "packing_provenance_mismatch"
        if packing["forecast_reference_s"] != side["reset_s"] or \
                packing["forecast_reference_s"] + packing["step_s"] != side["valid_time_s"]:
            return "REJECT", "run_identity_mismatch"
        if packing["source_unit"] != "m" or \
                packing["canonical_per_source_unit"] != CANONICAL_PER_SOURCE_UNIT:
            return "REJECT", "unit_mismatch"
    keys = ("referenceValue", "binaryScaleFactor", "decimalScaleFactor")
    if all(prev["packing"][k] == cur["packing"][k] for k in keys):
        return "REJECT", "identical_packing_parameters"
    if -difference < canonical_bound(pair):
        return "ADMIT_BOUNDED", "quantization_compatible_decrease"
    return "REJECT", "decrease_exceeds_quantization_bound"


def classify_strict(pair):
    """Policy A: the current #75 rule; every same-run decrease is rejected."""
    verdict, code = classify_structure(pair)
    if verdict != "DECREASE":
        return verdict, code
    return "REJECT", "same_run_negative_delta"


def classify_unconditional(pair):
    """Policy C: #75 structure kept, NegativeDelta dropped, darain clips later."""
    verdict, code = classify_structure(pair)
    if verdict != "DECREASE":
        return verdict, code
    return "ADMIT_UNBOUNDED", "clipped_by_darain"


# --- build mode -----------------------------------------------------------------

def interval_amount_sum(series, first, last):
    """Left-to-right float64 sum as Python 3.10 ``sum`` in the #202 trace."""
    total = 0
    for value in series[first:last + 1]:
        total = total + value
    return total


def encoder_inputs_m(evidence, name, node, base_index, step_index):
    series = evidence["interval_amounts_mm"][name]["{},{}".format(*node)]
    first = base_index * 4
    return interval_amount_sum(series, first, first + step_index) / 1000.0


def darain(xa, xb, xc, xd, clip):
    if clip:
        xa, xb, xc, xd = (0.0 if x < 0.0 else x for x in (xa, xb, xc, xd))
    xac = xb * xc / (xa + xc) if xa + xc > 0.0 else 0.5 * xb
    xbd = xb * xc / (xb + xd) if xb + xd > 0.0 else 0.5 * xc
    return xac + xbd


def build(args):
    repo = os.path.abspath(args.repo)
    grib_dir = os.path.join(repo, GRIB_DIR)
    evidence = load_json(os.path.join(repo, MAPPING_EVIDENCE))
    oracle = load_json(os.path.join(repo, DARAIN_ORACLE))
    keys = load_json(os.path.join(repo, ECCODES_KEYS_PATH))
    genuine = oracle["genuine"]
    require(keys["eccodes"] == evidence["runtime"]["eccodes"] == "2.24.2", "ecCodes identity")

    # Phase 1a: immutable inputs and two independent decoders.
    messages = []
    file_records = {}
    for name, path in grib_files(grib_dir):
        digest = sha256_file(path)
        require(digest == evidence["accumulated_input_sha256"][name], name + ": sha256")
        require(digest == keys["files"][name]["sha256"], name + ": ecCodes dump sha256")
        parsed = parse_grib1_file(path, name)
        require(len(parsed) == 8, name + ": expected 4 steps x LSP/CP")
        file_records[name] = {"sha256": digest, "octets": os.path.getsize(path),
                              "message_count": len(parsed)}
        for message, ec in zip(parsed, keys["files"][name]["messages"]):
            bds = message["bds"]
            require(ec["packingType"] == "grid_simple", "ecCodes packingType")
            require(not bds["spherical_harmonics"] and not bds["second_order_packing"]
                    and not bds["integer_values"] and not bds["additional_flags"], "BDS flag")
            require(ec["bitsPerValue"] == bds["bitsPerValue"], "bitsPerValue")
            require(ec["binaryScaleFactor"] == bds["binaryScaleFactor"], "binaryScaleFactor")
            require(ec["decimalScaleFactor"] == message["pds"]["decimalScaleFactor"], "D")
            require(Fraction(ec["referenceValue"]) == bds["referenceValue"], "referenceValue")
            require(ec["bitmapPresent"] == 0 and ec["numberOfMissing"] == 0, "missing values")
            require(ec["paramId"] == message["pds"]["indicatorOfParameter"], "paramId")
            require(ec["timeRangeIndicator"] == message["pds"]["timeRangeIndicator"], "TRI")
            decoded = [decode_eccodes_double(message, x) for x in message["packed"]]
            require(decoded == ec["values"], "stdlib decode differs from ecCodes")
            require(all(Fraction(y) == decode_exact(message, x)
                        for y, x in zip(decoded, message["packed"])), "inexact decode")
            message["eccodes"] = ec
            message["decoded"] = decoded
            messages.append(message)

    by_key = {}
    for message in messages:
        pds = message["pds"]
        ref = pds["reference_time"]
        require(pds["unitOfTimeRange"] == 1 and pds["P2"] == 0, "hour steps, P2 = 0")
        message["forecast_reference_s"] = epoch_seconds(*ref)
        message["step_s"] = pds["P1"] * 3600
        name = "lsp" if pds["indicatorOfParameter"] == PARAMS["lsp"] else "cp"
        by_key[(name, message["forecast_reference_s"], message["step_s"])] = message

    # Phase 1b: the decoded values are the values seen by flex_extract and #207.
    grid_id = None
    for name in PARAMS:
        for record in genuine["accumulated"][name]:
            message = by_key[(name, record["reset_s"], record["valid_s"] - record["reset_s"])]
            g = message["grid"]
            gid = (g["Ni"], g["Nj"], g["La1_millideg"], g["Lo1_millideg"], g["Di_millideg"],
                   g["Dj_millideg"], g["scanningMode"])
            require(grid_id in (None, gid), "grid identity changes between messages")
            grid_id = gid
            for j in range(NY):
                for i in range(NX):
                    require(record["values_m"][canonical_cell(i, j)] ==
                            message["decoded"][grib_index(i, j)], "oracle accumulated value")

    # Phase 2a: encoder reconstruction from the declared #202 interval amounts.
    reconstructed = 0
    max_error_over_half_quantum = Fraction(0)
    bases = sorted({m["forecast_reference_s"] for m in messages})
    for message in messages:
        name = "lsp" if message["pds"]["indicatorOfParameter"] == PARAMS["lsp"] else "cp"
        base_index = bases.index(message["forecast_reference_s"])
        step_index = message["step_s"] // DTIME_SECONDS - 1
        q = quantum(message["bds"]["binaryScaleFactor"], message["pds"]["decimalScaleFactor"])
        values = [0.0] * (NX * NY)
        for j in range(NY):
            for i in range(NX):
                values[grib_index(i, j)] = encoder_inputs_m(evidence, name, (i, j),
                                                            base_index, step_index)
        message["encoder_input_m"] = values
        for value, x, y in zip(values, message["packed"], message["decoded"]):
            require(encode_ecmwf_nearest(message, value) == x, "encoder reconstruction")
            error = abs(Fraction(y) - Fraction(value))
            max_error_over_half_quantum = max(max_error_over_half_quantum, error / (q / 2))
            reconstructed += 1
        value_range = max(values) - float(message["bds"]["referenceValue"])
        e = message["bds"]["binaryScaleFactor"]
        limit = (1 << message["bds"]["bitsPerValue"]) - 1
        message["scale_choice"] = {
            "field_maximum_m": max(values),
            "range_over_quantum": math.floor(value_range * math.ldexp(1.0, -e) + 0.5),
            "range_over_half_quantum": math.floor(value_range * math.ldexp(1.0, 1 - e) + 0.5),
            "max_packed_integer": limit,
            "binary_scale_is_smallest_fitting": (
                math.floor(value_range * math.ldexp(1.0, -e) + 0.5) <= limit
                < math.floor(value_range * math.ldexp(1.0, 1 - e) + 0.5)),
        }
    require(max_error_over_half_quantum <= 1, "ecCodes rounding exceeds half a quantum")

    # Phase 1c: flex_extract arithmetic, canonical f32 and every same-run pair.
    pair_stats = {"same_run_pairs": 0, "true_flat_pairs": 0, "decoded_decreases": 0,
                  "decoded_flat": 0, "decoded_increases_on_true_flat": 0,
                  "canonical_decreases": 0, "binary_scale_changes": 0,
                  "decreases_on_unchanged_packing": 0, "true_increase_decoded_decrease": 0}
    rows = []
    sequences = {}
    for name in PARAMS:
        ends = genuine["interval_means"][name]["interval_end_s"]
        for j in range(NY):
            for i in range(NX):
                cell = canonical_cell(i, j)
                obs = []
                for record in genuine["accumulated"][name]:
                    message = by_key[(name, record["reset_s"],
                                      record["valid_s"] - record["reset_s"])]
                    y = message["decoded"][grib_index(i, j)]
                    obs.append({"message": message, "valid_s": record["valid_s"],
                                "reset_s": record["reset_s"], "decoded_m": y,
                                "true_m": message["encoder_input_m"][grib_index(i, j)],
                                "x": message["packed"][grib_index(i, j)],
                                "canonical": f32(y / FLEX_EXTRACT_FAK)})
                sequences[(name, cell)] = obs
                for k, ob in enumerate(obs):
                    mm = ob["decoded_m"] / FLEX_EXTRACT_FAK
                    if k == 0 or ob["reset_s"] != obs[k - 1]["reset_s"]:
                        deac = mm / DTIME_HOURS
                    else:
                        deac = (mm - obs[k - 1]["decoded_m"] / FLEX_EXTRACT_FAK) / DTIME_HOURS
                    oracle_mean = genuine["interval_means"][name]["values_mm_per_h"][k][cell]
                    require(ends[k] == ob["valid_s"] and deac == oracle_mean, "deac_vals")
                    ob["deac"] = deac
                    if k == 0 or ob["reset_s"] != obs[k - 1]["reset_s"]:
                        continue
                    prev = obs[k - 1]
                    pair_stats["same_run_pairs"] += 1
                    e_changed = (prev["message"]["bds"]["binaryScaleFactor"] !=
                                 ob["message"]["bds"]["binaryScaleFactor"])
                    pair_stats["binary_scale_changes"] += int(e_changed)
                    true_flat = prev["true_m"] == ob["true_m"]
                    pair_stats["true_flat_pairs"] += int(true_flat)
                    decoded_diff = Fraction(ob["decoded_m"]) - Fraction(prev["decoded_m"])
                    canonical_diff = Fraction(ob["canonical"]) - Fraction(prev["canonical"])
                    require(decoded_diff >= 0 or canonical_diff <= 0, "f32 monotonicity")
                    require(decoded_diff < 0 or canonical_diff >= 0, "f32 created a decrease")
                    if true_flat:
                        pair_stats["decoded_flat"] += int(decoded_diff == 0)
                        pair_stats["decoded_increases_on_true_flat"] += int(decoded_diff > 0)
                    if decoded_diff < 0:
                        pair_stats["decoded_decreases"] += 1
                        pair_stats["decreases_on_unchanged_packing"] += int(not e_changed)
                        pair_stats["true_increase_decoded_decrease"] += int(not true_flat)
                        rows.append((name, i, j, cell, k))
                    pair_stats["canonical_decreases"] += int(canonical_diff < 0)
    expected = sorted((n["parameter"], n["cell"], n["interval_end_s"])
                      for n in genuine["negative_interval_means"])
    require(sorted((r[0], r[3], sequences[(r[0], r[3])][r[4]]["valid_s"]) for r in rows)
            == expected, "decreases differ from the #207 oracle")

    # Phase 1d: darain clipping, independently of the recorded oracle results.
    clip_check = {}
    for name in PARAMS:
        means = genuine["interval_means"][name]["values_mm_per_h"]
        ends = genuine["interval_means"][name]["interval_end_s"]
        members = genuine["darain_members"][name]
        changed = []
        for slot, member in enumerate(members):
            for cell in range(NX * NY):
                window = [means[slot + n][cell] for n in range(4)]
                clipped = darain(*window, clip=True)
                require(clipped == member["result_mm_per_h"][cell], "clipped darain")
                unclipped = darain(*window, clip=False)
                if unclipped != clipped:
                    changed.append({"member_time_s": member["member_time_s"], "cell": cell,
                                    "unclipped_mm_per_h": unclipped,
                                    "original_mm_per_h": clipped})
        projected = [row[:] for row in means]
        for n in genuine["negative_interval_means"]:
            if n["parameter"] == name:
                projected[ends.index(n["interval_end_s"])][n["cell"]] = 0.0
        for slot, member in enumerate(members):
            for cell in range(NX * NY):
                window = [projected[slot + n][cell] for n in range(4)]
                require(darain(*window, clip=True) == member["result_mm_per_h"][cell],
                        "projected admitted rows change a darain member")
        clip_check[name] = {"members": len(members), "values": len(members) * NX * NY,
                            "explicit_clip_recomputation_equal": True,
                            "zero_projection_of_admitted_rows_equal": True,
                            "values_changed_without_clip": changed}

    # Phase 1-2: the three offending rows.
    offending = []
    for name, i, j, cell, k in rows:
        obs = sequences[(name, cell)]
        prev, cur = obs[k - 1], obs[k]
        sides = {}
        for label, ob in (("previous", prev), ("current", cur)):
            m = ob["message"]
            q = quantum(m["bds"]["binaryScaleFactor"], m["pds"]["decimalScaleFactor"])
            sides[label] = {
                "observation_index": obs.index(ob),
                "valid_time_s": ob["valid_s"],
                "reset_s": ob["reset_s"],
                "forecast_reference_s": m["forecast_reference_s"],
                "step_hours": m["step_s"] // 3600,
                "file": m["file"],
                "message_index": m["index"],
                "message_sha256": m["message_sha256"],
                "grib_value_index": grib_index(i, j),
                "packed_integer": ob["x"],
                "binaryScaleFactor": m["bds"]["binaryScaleFactor"],
                "decimalScaleFactor": m["pds"]["decimalScaleFactor"],
                "referenceValue": float(m["bds"]["referenceValue"]),
                "bitsPerValue": m["bds"]["bitsPerValue"],
                "quantum_m": frac_json(q),
                "field_maximum_m": m["scale_choice"]["field_maximum_m"],
                "encoder_input_m": ob["true_m"],
                "encoder_input_m_f64_hex": f64_hex(ob["true_m"]),
                "decoded_m": ob["decoded_m"],
                "decoded_m_f64_hex": f64_hex(ob["decoded_m"]),
                "quantization_error_m": frac_json(Fraction(ob["decoded_m"]) -
                                                  Fraction(ob["true_m"])),
                "quantization_error_over_quantum": float(
                    (Fraction(ob["decoded_m"]) - Fraction(ob["true_m"])) / q),
                "flex_extract_mm": ob["decoded_m"] / FLEX_EXTRACT_FAK,
                "canonical_kg_per_m2_f32": ob["canonical"],
                "canonical_f32_hex": f32_hex(ob["canonical"]),
                "canonical_f32_ulp": float(f32_ulp(ob["canonical"])),
            }
        decoded_diff = Fraction(cur["decoded_m"]) - Fraction(prev["decoded_m"])
        flex_diff_mm = cur["decoded_m"] / FLEX_EXTRACT_FAK - prev["decoded_m"] / FLEX_EXTRACT_FAK
        canonical_diff = Fraction(cur["canonical"]) - Fraction(prev["canonical"])
        amount32 = f32(cur["canonical"] - prev["canonical"])
        rate_si32 = f32(amount32 / f32(DTIME_SECONDS))
        rate_mmh32 = f32(rate_si32 * f32(3600.0))
        q_prev = quantum(sides["previous"]["binaryScaleFactor"],
                         sides["previous"]["decimalScaleFactor"])
        q_cur = quantum(sides["current"]["binaryScaleFactor"],
                        sides["current"]["decimalScaleFactor"])
        pair = genuine_pair(name, prev, cur)
        bound = canonical_bound(pair)
        verdict = classify(pair)
        require(verdict == ("ADMIT_BOUNDED", "quantization_compatible_decrease"), "verdict")
        offending.append({
            "parameter": name,
            "param_id": PARAMS[name],
            "node_i_j": [i, j],
            "canonical_cell": cell,
            "interval_start_s": prev["valid_s"],
            "interval_end_s": cur["valid_s"],
            "interval_duration_s": cur["valid_s"] - prev["valid_s"],
            "run_identity": {"same_declared_reset": prev["reset_s"] == cur["reset_s"],
                             "same_forecast_reference": prev["message"]["forecast_reference_s"]
                             == cur["message"]["forecast_reference_s"],
                             "reset_s": cur["reset_s"]},
            "previous": sides["previous"],
            "current": sides["current"],
            "encoder_inputs_identical": prev["true_m"] == cur["true_m"],
            "packing_parameters_differ": (sides["previous"]["binaryScaleFactor"],
                                          sides["previous"]["referenceValue"]) !=
            (sides["current"]["binaryScaleFactor"], sides["current"]["referenceValue"]),
            "signed_difference": {
                "decoded_m": frac_json(decoded_diff),
                "decoded_over_previous_quantum": float(decoded_diff / q_prev),
                "flex_extract_mm": flex_diff_mm,
                "canonical_kg_per_m2": frac_json(canonical_diff),
                "canonical_f32_ulps_of_previous": float(
                    canonical_diff / f32_ulp(prev["canonical"])),
            },
            "signed_interval_mean_rate": {
                "flex_extract_deac_vals_mm_per_h": cur["deac"],
                "oracle_negative_interval_mean_mm_per_h": next(
                    n["value_mm_per_h"] for n in genuine["negative_interval_means"]
                    if n["parameter"] == name and n["cell"] == cell),
                "canonical_f32_diagnostic_amount_kg_per_m2": amount32,
                "canonical_f32_diagnostic_rate_mm_per_h": rate_mmh32,
                "canonical_f32_diagnostic_note": "what the #90 WGSL arithmetic would yield "
                                                 "if the row were not rejected; never emitted",
            },
            "darain_input_mm_per_h": cur["deac"],
            "darain_clipped_mm_per_h": 0.0,
            "rejection_75_90": {
                "error": "AccumulationError::NegativeDelta",
                "observation_index": k,
                "previous_kg_per_m2": prev["canonical"],
                "current_kg_per_m2": cur["canonical"],
                "site": "src/meteorology/accumulation.rs validate_interval_metadata",
                "caller": "src/gpu/meteorology.rs upload_accumulation -> "
                          "validate_interval_sequence",
                "effect": "CanonicalGpuField::upload fails for the complete field",
            },
            "bounds": {
                "source_adjacent_level_m": frac_json(q_prev + q_cur),
                "source_nearest_rounding_m": frac_json((q_prev + q_cur) / 2),
                "canonical_kg_per_m2": frac_json(bound),
                "decrease_over_source_adjacent_bound": float(-decoded_diff / (q_prev + q_cur)),
                "decrease_over_source_nearest_bound": float(
                    -decoded_diff / ((q_prev + q_cur) / 2)),
                "decrease_over_canonical_bound": float(-canonical_diff / bound),
            },
            "classification": {"verdict": verdict[0], "code": verdict[1]},
        })

    controls = negative_controls(rows, sequences)
    misclassified_by_c = [c["id"] for c in controls
                          if c["policy_c"][0] == "ADMIT_UNBOUNDED"
                          and c["expected_verdict"] != "ADMIT_BOUNDED"]
    require(misclassified_by_c, "policy C misclassifies no control")
    artifact = assemble(args, repo, evidence, oracle, keys, file_records, messages, offending,
                        pair_stats, clip_check, controls, reconstructed,
                        max_error_over_half_quantum, grid_id)
    policy_c = artifact["decision"]["policies"]["C_unconditional_flex_extract"]
    policy_c["misclassified_controls"] = misclassified_by_c
    with open(args.out, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(artifact, handle, indent=1, sort_keys=True, allow_nan=False)
        handle.write("\n")
    print(args.out, sha256_file(args.out))


def packing_record(message):
    return {
        "encoding": "grib1_grid_simple",
        "message_sha256": message["message_sha256"],
        "paramId": message["pds"]["indicatorOfParameter"],
        "bitsPerValue": message["bds"]["bitsPerValue"],
        "binaryScaleFactor": message["bds"]["binaryScaleFactor"],
        "decimalScaleFactor": message["pds"]["decimalScaleFactor"],
        "referenceValue": float(message["bds"]["referenceValue"]),
        "bitmap_present": message["pds"]["bitmap_present"],
        "forecast_reference_s": message["forecast_reference_s"],
        "step_s": message["step_s"],
        "source_unit": message["eccodes"]["units"],
        "canonical_per_source_unit": CANONICAL_PER_SOURCE_UNIT,
        "canonical_conversion": CANONICAL_CONVERSION,
    }


def genuine_pair(name, prev, cur):
    field = "large_scale_precipitation" if name == "lsp" else "convective_precipitation"
    pair = {"field": field, "canonical_unit": "kilogram_per_square_meter"}
    for label, ob in (("previous", prev), ("current", cur)):
        pair[label] = {
            "grid_id": "regular_ll_3x3_lat52_lon10_step1",
            "source_id": "ecmf-table128-param{}".format(PARAMS[name]),
            "valid_time_s": ob["valid_s"],
            "reset_s": ob["reset_s"],
            "canonical_value": ob["canonical"],
            "source_message_sha256": ob["message"]["message_sha256"],
            "packing": packing_record(ob["message"]),
        }
    return pair


def swap_valid_times(pair):
    previous, current = pair["previous"], pair["current"]
    previous["valid_time_s"], current["valid_time_s"] = (current["valid_time_s"],
                                                         previous["valid_time_s"])


def negative_controls(rows, sequences):
    """Explicit pair records with expected verdicts; each mutates one attribute."""
    name, _, _, cell, k = rows[0]
    obs = sequences[(name, cell)]
    base = genuine_pair(name, obs[k - 1], obs[k])
    controls = []

    def add(control_id, description, pair):
        verdict, code = classify(pair)
        controls.append({"id": control_id, "description": description, "pair": pair,
                         "expected_verdict": verdict, "expected_code": code,
                         "policy_a": list(classify_strict(pair)),
                         "policy_c": list(classify_unconditional(pair))})

    prev_c = base["previous"]["canonical_value"]
    bound = canonical_bound(base)
    ulp = f32_ulp(prev_c)
    admitted_steps = math.ceil(bound / ulp) - 1
    for steps, label in ((admitted_steps, "largest_admitted"),
                         (admitted_steps + 1, "smallest_rejected")):
        pair = copy.deepcopy(base)
        pair["current"]["canonical_value"] = f32_below(prev_c, steps)
        add("bound_" + label, "{} f32 steps below the previous value against a bound of "
            "{:.3f} steps".format(steps, float(bound / ulp)), pair)

    pair = copy.deepcopy(base)
    pair["current"]["canonical_value"] = f32(prev_c - 2.0 * float(bound))
    add("decrease_twice_bound", "genuine-looking decrease of twice the bound", pair)

    # Genuine data: the 39 h observation of the 36 h run declared as the 24 h run.
    cell0 = sequences[(name, canonical_cell(0, 0))]
    run_end = next(n for n, ob in enumerate(cell0) if ob["valid_s"] == 129600)
    misdeclared = genuine_pair(name, cell0[run_end], cell0[run_end + 1])
    misdeclared["current"]["reset_s"] = misdeclared["previous"]["reset_s"]
    misdeclared["current"]["packing"]["forecast_reference_s"] = misdeclared["previous"]["reset_s"]
    misdeclared["current"]["packing"]["step_s"] = (
        misdeclared["current"]["valid_time_s"] - misdeclared["previous"]["reset_s"])
    add("undeclared_reset_consistent_provenance", "genuine 36 h -> 39 h pair of node (0,0) "
        "with the 36 h restart hidden in both canonical and packing metadata", misdeclared)
    pair = genuine_pair(name, cell0[run_end], cell0[run_end + 1])
    pair["current"]["reset_s"] = pair["previous"]["reset_s"]
    add("undeclared_reset_provenance_conflict", "same genuine pair declared same-run while its "
        "message reference time is the 36 h restart", pair)

    mutations = [
        ("identical_packing_parameters", "previous message re-declared with the current "
         "message's scale factors (monotone quantizer)",
         lambda p: p["previous"]["packing"].update(
             binaryScaleFactor=p["current"]["packing"]["binaryScaleFactor"])),
        ("missing_packing_provenance", "previous observation without packing provenance",
         lambda p: p["previous"].update(packing=None)),
        ("packing_provenance_for_other_message", "provenance bound to a different message",
         lambda p: p["current"]["packing"].update(message_sha256="0" * 64)),
        ("provenance_param_mismatch", "LSP observation with paramId 143 provenance",
         lambda p: p["current"]["packing"].update(paramId=143)),
        ("unsupported_packing_second_order", "GRIB1 second-order packing",
         lambda p: p["current"]["packing"].update(encoding="grib1_grid_second_order")),
        ("unsupported_packing_grib2", "GRIB2 simple packing (accumulated GRIB2 input not "
         "exercised; #211 covers prepared GRIB2 members only)",
         lambda p: p["current"]["packing"].update(encoding="grib2_grid_simple")),
        ("decimal_scale_nonzero", "decimalScaleFactor 1 (10**-D evaluated by repeated "
         "division in ecCodes; error term not derived)",
         lambda p: p["current"]["packing"].update(decimalScaleFactor=1)),
        ("unsupported_constant_field", "bitsPerValue = 0 constant field",
         lambda p: p["current"]["packing"].update(bitsPerValue=0)),
        ("bitmap_present", "message with a bitmap / missing-value representation",
         lambda p: p["current"]["packing"].update(bitmap_present=True)),
        ("undeclared_conversion_path", "canonical f32 produced through an f32 decode",
         lambda p: p["current"]["packing"].update(canonical_conversion="f32_decode")),
        ("reversed_timestamps", "valid times swapped", swap_valid_times),
        ("missing_timestamp", "current valid time absent",
         lambda p: p["current"].update(valid_time_s=None)),
        ("missing_reset", "current reset absent",
         lambda p: p["current"].update(reset_s=None)),
        ("mid_window_reset", "current reset inside the previous window",
         lambda p: p["current"].update(reset_s=p["previous"]["valid_time_s"] - 3600)),
        ("mixed_grid_identity", "current observation on another grid",
         lambda p: p["current"].update(grid_id="regular_ll_3x3_lat53_lon10_step1")),
        ("mixed_source_identity", "current observation from parameter 143",
         lambda p: p["current"].update(source_id="ecmf-table128-param143")),
        ("missing_accumulated_observation", "current value absent (missing message/cell)",
         lambda p: p["current"].update(canonical_value=None)),
        ("wrong_source_unit", "packing quantum declared in mm",
         lambda p: p["current"]["packing"].update(source_unit="mm",
                                                  canonical_per_source_unit=1)),
        ("wrong_canonical_unit", "canonical unit not kg/m2",
         lambda p: p.update(canonical_unit="meter")),
        ("non_finite_nan", "current value NaN",
         lambda p: p["current"].update(canonical_value="NaN")),
        ("non_finite_infinity", "previous value +inf",
         lambda p: p["previous"].update(canonical_value="Infinity")),
        ("negative_accumulation", "current accumulated amount below zero",
         lambda p: p["current"].update(canonical_value=-p["current"]["canonical_value"])),
        ("unsupported_field", "decrease of a non-precipitation field",
         lambda p: p.update(field="surface_sensible_heat_flux")),
    ]
    for control_id, description, mutate in mutations:
        pair = copy.deepcopy(base)
        mutate(pair)
        add(control_id, description, pair)
    flat = copy.deepcopy(base)
    flat["current"]["canonical_value"] = flat["previous"]["canonical_value"]
    flat["previous"]["packing"] = None
    flat["current"]["packing"] = None
    add("flat_without_provenance", "equal accumulation needs no packing provenance", flat)
    return controls


def assemble(args, repo, evidence, oracle, keys, file_records, messages, offending,
             pair_stats, clip_check, controls, reconstructed, max_error, grid_id):
    message_table = []
    for m in messages:
        message_table.append({
            "file": m["file"], "index": m["index"], "message_sha256": m["message_sha256"],
            "paramId": m["pds"]["indicatorOfParameter"],
            "forecast_reference_s": m["forecast_reference_s"], "step_hours": m["step_s"] // 3600,
            "timeRangeIndicator": m["pds"]["timeRangeIndicator"],
            "eccodes_stepType": m["eccodes"]["stepType"], "eccodes_units": m["eccodes"]["units"],
            "packingType": m["eccodes"]["packingType"], "bitsPerValue": m["bds"]["bitsPerValue"],
            "binaryScaleFactor": m["bds"]["binaryScaleFactor"],
            "decimalScaleFactor": m["pds"]["decimalScaleFactor"],
            "referenceValue": float(m["bds"]["referenceValue"]),
            "referenceValue_ibm_hex": m["bds"]["referenceValue_ibm_hex"],
            "bitmap_present": m["pds"]["bitmap_present"],
            "packed_integers": m["packed"],
            "decoded_m": m["decoded"],
            "encoder_input_m": m["encoder_input_m"],
            "scale_choice": m["scale_choice"],
        })
    return {
        "schema": SCHEMA,
        "issue": 216,
        "related_issues": [75, 90, 202, 207, 32, 29],
        "validation_level": "original-source analysis; exact bit-level decoding of the "
                            "committed #202 GRIB input with two independent decoders; "
                            "no GPU execution, no production change, no parity claim",
        "references": {
            "flex_extract": {"pinned_commit": FLEX_EXTRACT_PIN,
                             "deaccumulation": "Source/Python/Classes/EcFlexpart.py:1123-1143",
                             "darain": "Source/Python/Mods/disaggregation.py:106-157",
                             "darain_clip": "Source/Python/Mods/disaggregation.py:144-147",
                             "blobs": {
                                 "Source/Python/Mods/disaggregation.py":
                                     oracle["flex_extract"]["blob"],
                                 "Source/Python/Classes/EcFlexpart.py": ECFLEXPART_BLOB}},
            "flexpart": {"pinned_commit": FLEXPART_PIN,
                         "precipitation_reader_clip": "src/windfields_mod.f90:2335-2348"},
            "repository_revision": args.revision,
        },
        "consumed": {
            "grib_files": file_records,
            "mapping_decision_sha256": sha256_file(os.path.join(repo, MAPPING_DECISION)),
            "mapping_evidence_sha256": sha256_file(os.path.join(repo, MAPPING_EVIDENCE)),
            "darain_oracle_sha256": sha256_file(os.path.join(repo, DARAIN_ORACLE)),
            "raw_trace_sha256": evidence["trace_sha256"],
            "eccodes_keys": ECCODES_KEYS_PATH,
            "eccodes_keys_sha256": sha256_file(os.path.join(repo, ECCODES_KEYS_PATH)),
            "eccodes_version": keys["eccodes"],
            "flex_extract_image": FLEX_EXTRACT_IMAGE,
        },
        "decoding": {
            "edition": 1,
            "packing": "grid-point simple packing (BDS flag 0, grid_simple)",
            "formula": "Y = (R + X * 2**E) * 10**-D (ecCodes grib_decode_double_array)",
            "decoders": ["ecCodes 2.24.2 codes_get_values (pinned container)",
                         "scripts/precipitation/grib_packing_decision.py stdlib BDS parser",
                         "tests/grib_packing_decision.rs BDS parser"],
            "values_decoded": reconstructed,
            "decode_exact_in_double": True,
            "stdlib_equals_eccodes_bitwise": True,
            "grid_identity": list(grid_id),
            "messages": message_table,
        },
        "encoder_reconstruction": {
            "source": "fixed interval amounts of "
                      "scripts/precipitation/flex_extract_member_trace.py "
                      "recorded in the #202 evidence (interval_amounts_mm), left-to-right "
                      "float64 sums / 1000 as in Python 3.10.12",
            "rule": "X = floor((v - R) * 2**-E + 0.5) (ecCodes nearest)",
            "values_reproduced": reconstructed,
            "max_abs_error_over_half_quantum": frac_json(max_error),
        },
        "pair_statistics": pair_stats,
        "darain_clip": {
            "source": "disaggregation.py:144-147 numpy x[x < 0.] = 0. before both quotients",
            "recomputation": clip_check,
            "flexpart_reader": "windfields_mod.f90:2340,2348 clip negative lsprec/convprec at read",
        },
        "offending_rows": offending,
        "attribution": {
            "fixture": "PROVEN_BY_CONSTRUCTION",
            "fixture_basis": "for all three rows the encoder input accumulations are identical "
                             "float64 values; the stored integers are reproduced from those inputs "
                             "with each message's own E and R, and the binary scale factor changes "
                             "because the field maximum crosses a packing range",
            "operational": "CONSISTENT_ONLY",
            "operational_basis": "operational GRIB does not carry the unquantized model value; a "
                                 "decoded decrease within the bound is indistinguishable from a "
                                 "flat true accumulation, never proof of packing causation",
            "proof_would_require": "the unquantized producer values or an encoder record for both "
                                   "messages (not available from MARS/GRIB)",
        },
        "decision": decision_section(),
        "negative_controls": controls,
    }


def decision_section():
    return {
        "verdict": "ADMIT_BOUNDED",
        "scope": "decision only; #75/#90 runtime still rejects every same-run decrease",
        "implementation_status": "not_implemented",
        "runtime_effect_until_follow_up": "none: rows without the canonical packing provenance "
                                          "below remain REJECT (NegativeDelta)",
        "policies": {
            "A_strict_rejection": {
                "verdict": "REJECTED_AS_PERMANENT_POLICY",
                "effect_on_202_example": "LSP and CP uploads fail; 0 of 18 cell series reach #207",
                "reason": "rejects decreases that faithful independent quantization must "
                          "produce whenever a flat accumulation is repacked with another scale; "
                          "#207 could never consume packed accumulated GRIB",
                "retained_as": "behaviour for every input outside the admitted class",
            },
            "B_evidence_bounded": {"verdict": "SELECTED"},
            "C_unconditional_flex_extract": {
                "verdict": "REJECTED",
                "reason": "flex_extract trusts message order and differences any pair; an "
                          "undeclared restart or corrupt message becomes a negative rate that "
                          "darain or the FLEXPART reader silently clips, losing the interval",
                "misclassified_controls": "derived: controls admitted by C but not by B",
            },
        },
        "supported_class": {
            "fields": list(SUPPORTED_FIELDS),
            "temporal_kind": "accumulated_since_reset",
            "canonical_unit": "kilogram_per_square_meter",
            "sign": "non_negative",
            "source": "GRIB edition 1 ECMWF table 128 paramId 142 (LSP) / 143 (CP) "
                      "(m water equivalent), regular lat/lon grid-point data",
            "packing": "BDS simple packing (grid_simple), bitsPerValue >= 1, "
                       "decimalScaleFactor 0, no bitmap, "
                       "floating-point data flag; exercised: bitsPerValue 24, D = 0",
            "decode": "Y = (R + X 2**E) 10**-D in IEEE double",
            "canonical_conversion": CANONICAL_CONVERSION + ": c = f32_nearest(Y * 1000) "
                                    "or f32_nearest(Y / 0.001), the product in double",
            "run_identity": "same declared reset R_prev == R_cur < t_prev; both messages carry "
                            "forecast reference time == R and reference + step == valid time",
            "pair": "consecutive declared observations (i-1, i) of one cell; no carry, no skip",
        },
        "bound": {
            "quantum_source_units": "q_k = 2**E_k * 10**-D_k [m] per message",
            "faithful_quantizer_assumptions": [
                "Q1: each decoded value is one of the two levels R + X q bracketing the "
                "unquantized value, so |v_k - Y_k| < q_k",
                "Q2: for fixed (R, E, D) the level choice is monotone in v",
            ],
            "source_bound": "v_cur >= v_prev implies Y_cur - Y_prev > -(q_prev + q_cur)",
            "canonical_term": "r_k = ulp_f32(c_k) / 2 + 2**-50 (|c_k| + ulp_f32(c_k)) (one f32 "
                              "rounding plus at most three f64 roundings in decode and unit "
                              "conversion; ulp_f32(0) = 2**-149)",
            "canonical_bound": "B = 1000 (q_prev + q_cur) + r_prev + r_cur [kg/m2]",
            "admission": "d = c_cur - c_prev < 0, (R, E, D) not identical, and -d < B",
            "identical_parameters": "monotone quantization cannot decrease: reject any decrease",
            "nearest_rounding_bound": "diagnostic only: (q_prev + q_cur) / 2; the encoder "
                                      "rounding mode is not recorded in GRIB",
            "arithmetic": "d is exact in f64 from the two f32 values; B is evaluated in f64; "
                          "strict comparison",
            "not_a_tolerance": "B depends only on the two messages' metadata and the canonical "
                               "values; no fixed epsilon, no #71 comparison tolerance",
        },
        "representation": {
            "canonical_interval_product": "remains non-negative: an admitted pair yields "
                                          "amount = rate_si = rate_mm_per_h = +0.0",
            "signed_intermediate_on_device": "none; #90 buffers never carry a negative value",
            "next_interval": "c_{i+1} - c_i from the unmodified lower c_i, as flex_extract "
                             "orig_vals; no carry-forward maximum",
            "telescoping": "sum of interval amounts = final total + sum of projected admitted "
                           "decreases, each below its B",
            "projection_owner": "#75 classification (host validation) and #90 WGSL "
                                "(amount := max(curr - prev, 0) for admitted entries only)",
            "provenance": "per admitted (interval, cell): signed canonical difference, B, both "
                          "message hashes and packing keys; kept outside the cell-invariant "
                          "AccumulatedIntervalMetadata",
            "flex_extract_equivalence": "every original consumer uses max(deac_vals, 0): darain "
                                        "clips before use; boundary writes are clipped by the "
                                        "FLEXPART reader",
        },
        "consumers": {
            "76_prepare_sample": "AccumulatedInterval amount / rate_si / rate_mm_per_h; receives "
                                 "+0.0 for admitted pairs; no production caller today",
            "207_precipitation_members": "binds the #90 mm/h grid in place; darain clip is a no-op "
                                         "on +0.0, members equal the original; kernel unchanged",
            "114_119_future": "consume #76/#207 products only; never the signed difference",
            "diagnostic_reports": "may expose the signed difference as a typed diagnostic, never "
                                  "as RateMillimeterPerHour, RateKgPerSquareMeterPerSecond or "
                                  "AmountKgPerSquareMeter",
        },
        "classes": [
            {"class": "supported_class above with packing provenance", "verdict": "ADMIT_BOUNDED"},
            {"class": "any input without canonical packing provenance (all current inputs); "
                      "runtime: NegativeDelta until #219, fail closed afterwards",
             "verdict": "UNRESOLVED"},
            {"class": "identical (R, E, D) in both messages", "verdict": "REJECT"},
            {"class": "decrease >= B", "verdict": "REJECT"},
            {"class": "identity, time, reset, unit, finiteness or provenance inconsistency",
             "verdict": "REJECT"},
            {"class": "non-precipitation accumulated fields", "verdict": "REJECT"},
            {"class": "GRIB1 second-order/complex/spherical, bitsPerValue 0, "
                      "decimalScaleFactor != 0, bitmap",
             "verdict": "UNRESOLVED"},
            {"class": "GRIB2 any template (incl. JPEG/CCSDS), NetCDF scale/offset packing, "
                      "ERA5 class EA, other conversion paths", "verdict": "UNRESOLVED"},
        ],
        "prerequisite": {
            "owner": "#29 schema field (smallest) populated by #32 providers",
            "missing_now": "the canonical Field carries only unit, sign, time and f32 values; "
                           "Snapshot provenance is the schema identity only",
            "required_record": "optional per-Field source quantization: encoding, message sha256, "
                               "paramId, bitsPerValue, E, D, R, bitmap flag, forecast reference, "
                               "step, source unit, canonical scale, conversion path",
            "rule": "#75 must not infer or default any of these keys",
        },
        "follow_up": "one implementation issue: canonical packing provenance + #75/#90 bounded "
                     "admission + genuine #202 -> #90 -> #207 production replay",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="mode", required=True)
    dump = sub.add_parser("eccodes-keys")
    dump.add_argument("--grib-dir", required=True)
    dump.add_argument("--out", required=True)
    make = sub.add_parser("build")
    make.add_argument("--repo", required=True)
    make.add_argument("--revision", required=True)
    make.add_argument("--out", required=True)
    args = parser.parse_args()
    if args.mode == "eccodes-keys":
        dump_eccodes_keys(args.grib_dir, args.out)
    else:
        build(args)


if __name__ == "__main__":
    main()
