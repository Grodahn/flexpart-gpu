#!/usr/bin/env python3
"""Independent minimal GRIB2 octet reader for the #230 ICON-EU audit.

This is a research cross-check, not a decoder for production. It reads the
WMO FM 92 GRIB2 section octets directly (WMO-No. 306 Vol I.2, Sections 0-8)
without ecCodes definition tables, so a disagreement with ecCodes exposes a
table/definition interpretation error rather than repeating it.

Supported only what the audited DWD files use: grid template 3.0 (regular
lat/lon), product templates 4.0 and 4.8, data representation template 5.42
(CCSDS) and no bitmap. Anything else raises ``SectionError``. Template 5.42
data are unpacked by calling the CCSDS 121.0 reference library ``libaec``
(``aec_buffer_decode``) through ctypes when it is available; ecCodes is not
involved in that path. Values follow WMO 5.0/5.42 semantics::

    Y = (R + X * 2**E) / 10**D
"""

import ctypes
import ctypes.util
import struct

GRIB_MAGIC = b"GRIB"
END_MAGIC = b"7777"
SUPPORTED_GRID_TEMPLATE = 0
SUPPORTED_PRODUCT_TEMPLATES = (0, 8)
CCSDS_TEMPLATE = 42
NO_BITMAP = 255
# libaec flag bits (libaec.h): output data layout only, not compression semantics.
AEC_DATA_3BYTE = 2
AEC_DATA_MSB = 4
MICRO_DEGREE = 1.0e-6


class SectionError(ValueError):
    """The octets are not a complete GRIB2 message of the supported subset."""


def signed(value, octets):
    """Decode WMO sign-and-magnitude integers (most significant bit is the sign)."""
    sign_bit = 1 << (8 * octets - 1)
    return -(value & (sign_bit - 1)) if value & sign_bit else value


def u(data, offset, octets):
    return int.from_bytes(data[offset:offset + octets], "big")


def s(data, offset, octets):
    return signed(u(data, offset, octets), octets)


def split_messages(data):
    """Return the complete messages in a byte string; trailing bytes are an error."""
    messages, offset = [], 0
    while offset < len(data):
        if data[offset:offset + 4] != GRIB_MAGIC:
            raise SectionError(f"no GRIB indicator at byte {offset}")
        if data[offset + 7] != 2:
            raise SectionError("not GRIB edition 2")
        total = u(data, offset + 8, 8)
        message = data[offset:offset + total]
        if len(message) != total or message[-4:] != END_MAGIC:
            raise SectionError("truncated message or missing 7777 end section")
        messages.append(message)
        offset += total
    if not messages:
        raise SectionError("empty input")
    return messages


def sections(message):
    """Map section number to its octets (1-based WMO octet n is index n-1)."""
    found, offset = {0: message[:16]}, 16
    while offset < len(message) - 4:
        length, number = u(message, offset, 4), message[offset + 4]
        if length < 5 or offset + length > len(message) - 4 or number in found:
            raise SectionError(f"malformed section {number} at byte {offset}")
        found[number] = message[offset:offset + length]
        offset += length
    if offset != len(message) - 4:
        raise SectionError("section lengths do not end at the 7777 section")
    for required in (1, 3, 4, 5, 6, 7):
        if required not in found:
            raise SectionError(f"missing section {required}")
    return found


def parse_identification(sec):
    return {
        "centre": u(sec, 5, 2),
        "sub_centre": u(sec, 7, 2),
        "tables_version": sec[9],
        "local_tables_version": sec[10],
        "significance_of_reference_time": sec[11],
        "reference_time": "%04d-%02d-%02dT%02d:%02d:%02dZ" % (
            u(sec, 12, 2), sec[14], sec[15], sec[16], sec[17], sec[18]),
        "production_status": sec[19],
        "type_of_data": sec[20],
    }


def parse_grid(sec):
    template = u(sec, 12, 2)
    if template != SUPPORTED_GRID_TEMPLATE:
        raise SectionError(f"unsupported grid template 3.{template}")
    return {
        "number_of_points": u(sec, 6, 4),
        "template": template,
        "shape_of_earth": sec[14],
        "ni": u(sec, 30, 4),
        "nj": u(sec, 34, 4),
        "basic_angle": u(sec, 38, 4),
        "first_lat_deg": s(sec, 46, 4) * MICRO_DEGREE,
        "first_lon_deg": s(sec, 50, 4) * MICRO_DEGREE,
        "resolution_flags": sec[54],
        "last_lat_deg": s(sec, 55, 4) * MICRO_DEGREE,
        "last_lon_deg": s(sec, 59, 4) * MICRO_DEGREE,
        "di_deg": u(sec, 63, 4) * MICRO_DEGREE,
        "dj_deg": u(sec, 67, 4) * MICRO_DEGREE,
        "scanning_mode": sec[71],
    }


def parse_product(sec):
    coordinates, template = u(sec, 5, 2), u(sec, 7, 2)
    if template not in SUPPORTED_PRODUCT_TEMPLATES:
        raise SectionError(f"unsupported product template 4.{template}")
    product = {
        "template": template,
        "parameter_category": sec[9],
        "parameter_number": sec[10],
        "type_of_generating_process": sec[11],
        "unit_of_time_range": sec[17],
        "forecast_time": u(sec, 18, 4),
        "first_surface_type": sec[22],
        "first_surface_scaled_value": u(sec, 24, 4),
        "second_surface_type": sec[28],
        "second_surface_scaled_value": u(sec, 30, 4),
    }
    end = 34
    if template == 8:
        product.update({
            "end_of_interval": "%04d-%02d-%02dT%02d:%02d:%02dZ" % (
                u(sec, 34, 2), sec[36], sec[37], sec[38], sec[39], sec[40]),
            "time_range_count": sec[41],
            "missing_in_statistics": u(sec, 42, 4),
            "statistical_processing": sec[46],
            "time_increment_type": sec[47],
            "unit_of_statistical_range": sec[48],
            "statistical_range_length": u(sec, 49, 4),
        })
        end = 58
    product["vertical_coordinate_octets"] = sec[end:end + 4 * coordinates].hex()
    return product


def parse_representation(sec):
    template = u(sec, 9, 2)
    if template != CCSDS_TEMPLATE:
        raise SectionError(f"unsupported data representation template 5.{template}")
    return {
        "template": template,
        "number_of_values": u(sec, 5, 4),
        "reference_value": struct.unpack(">f", sec[11:15])[0],
        "binary_scale_factor": s(sec, 15, 2),
        "decimal_scale_factor": s(sec, 17, 2),
        "bits_per_value": sec[19],
        "type_of_original_values": sec[20],
        "ccsds_flags": sec[21],
        "ccsds_block_size": sec[22],
        "ccsds_rsi": u(sec, 23, 2),
    }


def parse(message):
    """Return the independently decoded metadata of one message (no data values)."""
    found = sections(message)
    bitmap = found[6][5]
    if bitmap != NO_BITMAP:
        raise SectionError(f"unsupported bitmap indicator {bitmap}")
    return {
        "edition": message[7],
        "discipline": message[6],
        "total_length": len(message),
        "identification": parse_identification(found[1]),
        "local_use_present": 2 in found,
        "grid": parse_grid(found[3]),
        "product": parse_product(found[4]),
        "representation": parse_representation(found[5]),
        "bitmap_indicator": bitmap,
        "data_octets": len(found[7]) - 5,
    }


class _AecStream(ctypes.Structure):
    _fields_ = [
        ("next_in", ctypes.c_void_p), ("avail_in", ctypes.c_size_t),
        ("total_in", ctypes.c_size_t), ("next_out", ctypes.c_void_p),
        ("avail_out", ctypes.c_size_t), ("total_out", ctypes.c_size_t),
        ("bits_per_sample", ctypes.c_uint), ("block_size", ctypes.c_uint),
        ("rsi", ctypes.c_uint), ("flags", ctypes.c_uint), ("state", ctypes.c_void_p),
    ]


def load_libaec():
    """Return the libaec shared library, or ``None`` when it is not installed."""
    name = ctypes.util.find_library("aec") or "libaec.so.0"
    try:
        library = ctypes.CDLL(name)
    except OSError:
        return None
    library.aec_buffer_decode.argtypes = [ctypes.POINTER(_AecStream)]
    return library


def decode_values(message, libaec):
    """Unpack template 5.42 values with libaec; returns a list of floats."""
    found = sections(message)
    rep = parse_representation(found[5])
    count, bits = rep["number_of_values"], rep["bits_per_value"]
    scale = 2.0 ** rep["binary_scale_factor"] / 10.0 ** rep["decimal_scale_factor"]
    offset = rep["reference_value"] / 10.0 ** rep["decimal_scale_factor"]
    if bits == 0:
        return [offset] * count
    width = 1 if bits <= 8 else 2 if bits <= 16 else 4
    packed = bytes(found[7][5:])
    output = ctypes.create_string_buffer(count * width)
    stream = _AecStream()
    source = ctypes.create_string_buffer(packed, len(packed))
    stream.next_in = ctypes.cast(source, ctypes.c_void_p)
    stream.avail_in = len(packed)
    stream.next_out = ctypes.cast(output, ctypes.c_void_p)
    stream.avail_out = count * width
    stream.bits_per_sample = bits
    stream.block_size = rep["ccsds_block_size"]
    stream.rsi = rep["ccsds_rsi"]
    stream.flags = rep["ccsds_flags"] & ~(AEC_DATA_3BYTE | AEC_DATA_MSB)
    status = libaec.aec_buffer_decode(ctypes.byref(stream))
    if status != 0 or stream.total_out != count * width:
        raise SectionError(f"libaec decode failed: status {status}")
    code = {1: "B", 2: "H", 4: "I"}[width]
    integers = struct.unpack(f"<{count}{code}", output.raw)
    return [offset + integer * scale for integer in integers]
