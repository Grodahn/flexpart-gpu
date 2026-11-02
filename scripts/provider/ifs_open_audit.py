"""Offline research audit of preserved IFS GRIB2 messages, not a provider decoder.

ecCodes is mandatory. Independent metadata/octet and libaec comparison reuses
the deliberately narrow #230 reader; no canonical snapshots are constructed.
"""

import argparse
import ctypes
import datetime as dt
import hashlib
import json
from pathlib import Path
import sys

import eccodes as ec
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import icon_eu_grib2_sections as octets

KEYS = (
    "edition centre subCentre tablesVersion localTablesVersion class stream type "
    "generatingProcessIdentifier typeOfGeneratingProcess productionStatusOfProcessedData "
    "discipline parameterCategory parameterNumber paramId shortName name units "
    "dataDate dataTime validityDate validityTime stepUnits forecastTime startStep endStep stepRange stepType "
    "typeOfStatisticalProcessing typeOfTimeIncrement indicatorOfUnitForTimeRange lengthOfTimeRange "
    "productDefinitionTemplateNumber typeOfFirstFixedSurface typeOfSecondFixedSurface "
    "typeOfLevel level NV gridType gridDefinitionTemplateNumber Ni Nj numberOfDataPoints "
    "latitudeOfFirstGridPointInDegrees longitudeOfFirstGridPointInDegrees "
    "latitudeOfLastGridPointInDegrees longitudeOfLastGridPointInDegrees "
    "iDirectionIncrementInDegrees jDirectionIncrementInDegrees scanningMode uvRelativeToGrid "
    "bitmapPresent missingValue numberOfMissing numberOfValues "
    "packingType dataRepresentationTemplateNumber bitsPerValue referenceValue binaryScaleFactor "
    "decimalScaleFactor ccsdsFlags ccsdsBlockSize ccsdsRsi"
).split()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read_metadata(handle):
    """Keep numeric WMO surfaces rather than ecCodes' definition-dependent labels."""
    numeric = {"typeOfFirstFixedSurface", "typeOfSecondFixedSurface"}
    return {key: ec.codes_get(handle, key, int) if key in numeric else ec.codes_get(handle, key)
            for key in KEYS if ec.codes_is_defined(handle, key)}


def validate_inventory(root, manifest):
    """Check complete coherent-cycle index inventory before trusting availability."""
    validate_product(manifest)
    expected = {(r, s, f) for r, fs in (("0p1", ("sfc", "pl")), ("0p25", ("",)))
                for s in range(0, 49, 3) for f in fs}
    identities, inventory = set(), []
    for item in manifest["indexes"]:
        identity = item["resolution"], item["step"], item["suffix"]
        if identity in identities:
            raise ValueError("duplicate index")
        identities.add(identity)
        data = (root / item["path"]).read_bytes()
        if len(data) != item["bytes"] or sha(data) != item["sha256"]:
            raise ValueError("index integrity")
        rows = [json.loads(line) for line in data.splitlines()]
        if not rows:
            raise ValueError("empty inventory")
        offset = 0
        for row in rows:
            for key, value in {"date": manifest["cycle"][:8], "time": manifest["cycle"][8:] + "00",
                               "stream": "oper", "type": "fc", "step": str(item["step"])}.items():
                initial_extreme = (key == "step" and item["step"] == 0 and
                                   row["param"] in ("mx2t3", "mn2t3") and row[key] == "3")
                if row[key] != value and not initial_extreme:
                    raise ValueError(f"index identity {key}")
            if row["_offset"] != offset or row["_length"] <= 0:
                raise ValueError("noncontiguous index")
            offset += row["_length"]
        inventory.append({"path": item["path"], "resolution": item["resolution"], "step": item["step"],
                          "suffix": item["suffix"], "file_bytes": offset, "message_count": len(rows),
                          "levels_by_parameter": {p: {l: sorted({int(r["levelist"]) for r in rows if r["param"] == p and r["levtype"] == l and "levelist" in r})
                                                       for l in sorted({r["levtype"] for r in rows if r["param"] == p})}
                                                  for p in sorted({r["param"] for r in rows})}})
    if identities != expected:
        raise ValueError("incomplete index/cadence coverage")
    return inventory


def validate_product(manifest):
    """The audit's product scope never aliases AIFS, ENS or another stream."""
    for key, expected in (("model", "ifs"), ("stream", "oper"), ("type", "fc")):
        if manifest.get(key) != expected:
            raise ValueError(f"unsupported product {key}")


def validate_raw(data, item):
    """Reject altered/truncated source bytes before handing them to a native decoder."""
    if sha(data) != item["sha256"] or len(data) != item["bytes"]:
        raise ValueError("raw message integrity")
    if len(octets.split_messages(data)) != 1:
        raise ValueError("one message required")


def validate_message(data, item, manifest, metadata):
    """Reject cross-run, altered and unsupported samples before value comparison."""
    validate_product(manifest)
    if item["resolution"] not in ("0p1", "0p25"):
        raise ValueError("unsupported resolution")
    validate_raw(data, item)
    expected = {"edition": 2, "centre": "ecmf", "class": "od", "stream": "oper", "type": "fc",
                "dataDate": int(manifest["cycle"][:8]), "dataTime": int(manifest["cycle"][8:]) * 100,
                "endStep": item["step"], "stepUnits": 1, "NV": 0,
                "packingType": "grid_ccsds", "dataRepresentationTemplateNumber": 42,
                "bitmapPresent": 0, "numberOfMissing": 0, "scanningMode": 0,
                "gridType": "regular_ll", "gridDefinitionTemplateNumber": 0, "uvRelativeToGrid": 0,
                "Ni": 3600 if item["resolution"] == "0p1" else 1440,
                "Nj": 1801 if item["resolution"] == "0p1" else 721,
                "iDirectionIncrementInDegrees": 0.1 if item["resolution"] == "0p1" else 0.25,
                "jDirectionIncrementInDegrees": 0.1 if item["resolution"] == "0p1" else 0.25,
                "latitudeOfFirstGridPointInDegrees": 90.0, "latitudeOfLastGridPointInDegrees": -90.0,
                "longitudeOfFirstGridPointInDegrees": 180.0,
                "longitudeOfLastGridPointInDegrees": 179.9 if item["resolution"] == "0p1" else 179.75}
    points = expected["Ni"] * expected["Nj"]
    expected.update(numberOfDataPoints=points, numberOfValues=points)
    selector = item["selector"]
    height = {"2t": 2, "2d": 2, "10u": 10, "10v": 10}.get(selector["param"], 0)
    expected.update(level=int(selector.get("levelist", height)),
                    typeOfLevel="isobaricInhPa" if selector["levtype"] == "pl" else
                    "heightAboveGround" if height else "entireAtmosphere" if selector["param"] == "tcc" else "surface")
    for key, value in expected.items():
        if metadata.get(key) != value:
            raise ValueError(f"message identity {key}: {metadata.get(key)} != {value}")
    units = {"2t": "K", "sp": "Pa", "tp": "m", "cp": "m", "t": "K", "q": "kg kg**-1",
             "u": "m s**-1", "v": "m s**-1", "w": "Pa s**-1", "gh": "gpm", "ewss": "N m**-2 s",
             "ssr": "J m**-2", "sf": "m of water equivalent", "sd": "m of water equivalent",
             "2d": "K", "10u": "m s**-1", "10v": "m s**-1", "lsm": "(0 - 1)",
             "z": "m**2 s**-2", "tcc": "(0 - 1)", "nsss": "N m**-2 s"}
    if metadata["units"] != units[selector["param"]]:
        raise ValueError(f"{item['path']}: units changed: {metadata['units']}")
    if metadata["shortName"] != selector["param"]:
        raise ValueError("parameter changed")
    bits = {"2t": 9, "sp": 12, "tp": 12, "cp": 12, "t": 9, "q": 9,
            "u": 11, "v": 11, "w": 12, "gh": 12, "ewss": 12, "ssr": 10, "sf": 11,
            "2d": 9, "10u": 11, "10v": 11, "lsm": 8, "z": 16, "sd": 9, "tcc": 7, "nsss": 12}
    accumulation = selector["param"] in ("tp", "cp", "ewss", "nsss", "ssr", "sf")
    expected_bits = 0 if accumulation and item["step"] == 0 else bits[selector["param"]]
    if metadata["bitsPerValue"] != expected_bits:
        raise ValueError("packing precision changed")
    if accumulation and (metadata["startStep"] != 0 or metadata["stepType"] != "accum" or
                         metadata.get("typeOfStatisticalProcessing") != 1):
        raise ValueError("unexpected accumulation window/statistics")
    if not accumulation and metadata["stepType"] != "instant":
        raise ValueError("instantaneous field changed")
    valid = dt.datetime.strptime(manifest["cycle"], "%Y%m%d%H") + dt.timedelta(hours=item["step"])
    if metadata["validityDate"] != int(valid.strftime("%Y%m%d")) or metadata["validityTime"] != valid.hour * 100:
        raise ValueError("valid time changed")


def independent(data, values):
    """Compare every quantized value against direct CCSDS unpacking, without ecCodes."""
    library = octets.load_libaec()
    if library is None:
        local = Path(ec.__file__).parent / "aec.dll"
        library = ctypes.CDLL(str(local))
        library.aec_buffer_decode.argtypes = [ctypes.POINTER(octets._AecStream)]
    decoded = np.asarray(octets.decode_values(data, library), dtype=np.float64)
    if decoded.shape != values.shape or not np.array_equal(decoded, values):
        raise ValueError("independent values disagree")
    return {"method": "independent WMO octets + direct libaec; no ecCodes definitions",
            "count": len(decoded), "max_absolute_difference": float(np.max(np.abs(decoded - values)))}


def compare_metadata(raw, metadata):
    """Cross-check table-independent section fields against ecCodes key decoding."""
    pairs = [(raw["discipline"], metadata["discipline"]), (raw["identification"]["centre"], 98)]
    for source, target in (("parameter_category", "parameterCategory"), ("parameter_number", "parameterNumber"),
                           ("template", "productDefinitionTemplateNumber"), ("first_surface_type", "typeOfFirstFixedSurface")):
        pairs.append((raw["product"][source], metadata[target]))
    for source, target in (("number_of_points", "numberOfDataPoints"), ("ni", "Ni"), ("nj", "Nj"),
                           ("scanning_mode", "scanningMode"), ("first_lat_deg", "latitudeOfFirstGridPointInDegrees"),
                           ("first_lon_deg", "longitudeOfFirstGridPointInDegrees")):
        pairs.append((raw["grid"][source], metadata[target]))
    for source, target in (("template", "dataRepresentationTemplateNumber"), ("bits_per_value", "bitsPerValue"),
                           ("reference_value", "referenceValue"), ("binary_scale_factor", "binaryScaleFactor"),
                           ("decimal_scale_factor", "decimalScaleFactor")):
        pairs.append((raw["representation"][source], metadata[target]))
    if any(left != right for left, right in pairs) or raw["product"]["vertical_coordinate_octets"]:
        raise ValueError(f"independent metadata disagreement: {[(left, right) for left, right in pairs if left != right]}")


def run(root, independent_values=False, legacy_numeric_diagnostic=False):
    manifest = json.loads((root / "retrieval-manifest.json").read_text())
    inventory = validate_inventory(root, manifest)
    expected = {f"0p1-0-{p}-850.grib2" for p in ("t", "u", "v", "w", "q", "gh")}
    expected |= {f"0p1-0-{p}-sfc.grib2" for p in ("2t", "sp", "2d", "10u", "10v", "lsm", "z", "sd", "tcc")}
    expected |= {f"0p1-{s}-{p}-sfc.grib2" for s in (0, 3, 6, 24, 48) for p in ("tp", "cp")}
    expected |= {f"0p1-3-{p}-sfc.grib2" for p in ("ewss", "nsss", "sf", "ssr")}
    expected |= {"0p1-3-t-850.grib2"}
    expected |= {f"0p25-{s}-{p}-sfc.grib2" for s in (0, 3) for p in ("2t", "tp")}
    if {i["path"] for i in manifest["messages"]} != expected or len(manifest["messages"]) != len(expected):
        raise ValueError("missing or unexpected representative message")
    records = []
    for item in manifest["messages"]:
        data = (root / item["path"]).read_bytes()
        validate_raw(data, item)
        handle = ec.codes_new_from_message(data)
        try:
            metadata = read_metadata(handle)
            checked = dict(metadata)
            if legacy_numeric_diagnostic:
                if ec.codes_get_api_version() != "2.24.2":
                    raise ValueError("legacy diagnostic is restricted to ecCodes 2.24.2")
                modern = json.loads((root / "decoded-2.48.0.json").read_text())
                reference = next(r for r in modern["messages"] if r["path"] == item["path"])
                if reference["sha256"] != sha(data):
                    raise ValueError("legacy comparison source mismatch")
                if metadata["shortName"] == "unknown" and item["selector"]["param"] in ("cp", "ewss", "nsss", "sf", "tcc"):
                    checked.update({k: reference["metadata"][k] for k in ("shortName", "units", "paramId")})
            validate_message(data, item, manifest, checked)
            raw = octets.parse(data)
            compare_metadata(raw, metadata)
            values = np.asarray(ec.codes_get_values(handle), dtype=np.float64)
            if not np.isfinite(values).all():
                raise ValueError("nonfinite source values")
            points = [0, len(values) // 2, len(values) - 1]
            # Include central-European 50 N, 10 E in source scan order (longitude starts at 180 E).
            spacing = metadata["iDirectionIncrementInDegrees"]
            points.append(round((90 - 50) / spacing) * metadata["Ni"] + round(((10 - 180) % 360) / spacing))
            quantum = 0 if metadata["bitsPerValue"] == 0 else 2.0 ** metadata["binaryScaleFactor"] / 10.0 ** metadata["decimalScaleFactor"]
            record = {"path": item["path"], "sha256": item["sha256"], "metadata": metadata, "octets": raw,
                      "quantum": quantum, "rounding_half_quantum": quantum / 2,
                      "values": {"count": len(values), "min": float(values.min()), "max": float(values.max()),
                                 "sha256_f64_le": sha(values.astype("<f8").tobytes()),
                                 "samples": [{"index": i, "value": float(values[i])} for i in points]}}
            if independent_values:
                record["independent_values"] = independent(data, values)
            if legacy_numeric_diagnostic and record["values"] != reference["values"]:
                raise ValueError("old/new ecCodes values disagree")
            records.append(record)
        finally:
            ec.codes_release(handle)
    if not records:
        raise ValueError("missing required raw messages")
    expected_files = {x["path"] for x in manifest["messages"]}
    if len(expected_files) != len(records):
        raise ValueError("duplicate messages")
    return {"schema": "flexpart-gpu.ifs-open-decoded-research", "version": 1,
            "ecCodes": ec.codes_get_api_version(), "legacy_numeric_diagnostic": legacy_numeric_diagnostic,
            "inventory": inventory, "messages": records}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--independent-values", action="store_true")
    parser.add_argument("--legacy-numeric-diagnostic", action="store_true",
                        help="2.24.2 only: compare values despite documented unknown parameter labels")
    args = parser.parse_args()
    args.output.write_text(json.dumps(run(args.root, args.independent_values, args.legacy_numeric_diagnostic), indent=2) + "\n", newline="\n")
