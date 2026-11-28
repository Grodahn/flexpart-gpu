#!/usr/bin/env python3
"""Build or verify offline, direct-ecCodes checksums of all ERA5/ETEX cells."""

import argparse
from collections import defaultdict
from datetime import datetime, timezone
import hashlib
from io import BytesIO
import json
import math
from pathlib import Path

import eccodes
import numpy as np


ROOT = Path(__file__).resolve().parents[2]
RAW = ROOT / "fixtures/etex/native-mini"
EVIDENCE = ROOT / "fixtures/meteorology/era5-etex-native-v1.value-digests.json"
PAIR_EVIDENCE = ROOT / "fixtures/meteorology/era5-etex-native-lnsp-v1.value-digests.json"
OMEGA_EVIDENCE = ROOT / "fixtures/meteorology/era5-etex-native-omega-v1.value-digests.json"
MODEL_REQUESTS = ("request.json", "request-next-day.json")
REPLAY_MANIFEST = RAW / "era5-etex-model-replay-v1.json"
FIELDS = {130: "temperature", 131: "wind_u", 132: "wind_v", 133: "specific_humidity"}
PRODUCT_KEYS = {
    "centre": "ecmf", "subCentre": 0, "marsClass": "ea", "marsStream": "oper",
    "marsType": "an", "experimentVersionNumber": "0001", "localDefinitionNumber": 1,
    "generatingProcessIdentifier": 145, "productionStatusOfProcessedData": 0,
    "typeOfProcessedData": "an", "significanceOfReferenceTime": 1,
    "productDefinitionTemplateNumber": 0, "startStep": 0, "endStep": 0,
    "stepType": "instant", "second": 0,
}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def verified_grib(metadata):
    path = RAW / metadata["file"]
    raw = path.read_bytes()
    if sha256(raw) != metadata["sha256"] or len(raw) != metadata["bytes"]:
        raise ValueError(f"unexpected source identity: {path.name}")
    return raw


def grib2_frames(raw):
    """Pass complete messages from the checksum-verified buffer to ecCodes."""
    offset = 0
    while offset < len(raw):
        header = raw[offset:offset + 16]
        if len(header) != 16 or header[:4] != b"GRIB" or header[7] != 2:
            raise ValueError(f"invalid GRIB2 header at byte {offset}")
        end = offset + int.from_bytes(header[8:], "big")
        if end < offset + 20 or end > len(raw) or raw[end - 4:end] != b"7777":
            raise ValueError(f"invalid GRIB2 frame at byte {offset}")
        yield raw[offset:end]
        offset = end


def audit(manifest):
    metadata = json.loads(manifest.read_bytes())
    path = RAW / metadata["file"]
    raw = verified_grib(metadata)
    volumes = defaultdict(dict)
    sort_order = None
    pv_digest = None
    for frame in grib2_frames(raw):
        message = eccodes.codes_new_from_message(frame)
        try:
            parameter = eccodes.codes_get_long(message, "paramId")
            if parameter not in FIELDS:
                continue
            date = eccodes.codes_get_long(message, "validityDate")
            time = eccodes.codes_get_long(message, "validityTime")
            level = eccodes.codes_get_long(message, "level")
            dt = datetime.strptime(f"{date}{time:04}", "%Y%m%d%H%M")
            epoch = int(dt.replace(tzinfo=timezone.utc).timestamp())
            if sort_order is None:
                pv = eccodes.codes_get_array(message, "pv")
                if pv.shape != (276,) or not np.isfinite(pv).all():
                    raise ValueError(f"invalid PV vector in {path.name}")
                pv_digest = hashlib.sha256(pv.astype("<f4").tobytes()).hexdigest()
                lat = eccodes.codes_get_array(message, "latitudes")
                lon = eccodes.codes_get_array(message, "longitudes")
                lon = (lon + 180) % 360 - 180
                sort_order = np.lexsort((lon, lat))
                np.testing.assert_array_equal(
                    lat[sort_order], np.repeat(np.linspace(43, 53, 41), 65)
                )
                np.testing.assert_array_equal(
                    lon[sort_order], np.tile(np.linspace(-8, 8, 65), 41)
                )
            cells = eccodes.codes_get_values(message)[sort_order]
            if not np.isfinite(cells).all():
                raise ValueError(f"nonfinite values at {epoch}/{parameter}/{level}")
            levels = volumes[epoch, FIELDS[parameter]]
            if level in levels:
                raise ValueError(f"duplicate {epoch}/{parameter}/{level}")
            levels[level] = cells.astype("<f4").tobytes()
        finally:
            eccodes.codes_release(message)
    if len(volumes) != 3 * len(FIELDS):
        raise ValueError(f"missing time or field in {path.name}")
    result = {}
    for (epoch, field), levels in sorted(volumes.items()):
        if sorted(levels) != list(range(1, 138)):
            raise ValueError(f"missing or extra levels at {epoch}/{field}")
        data = b"".join(levels[level] for level in sorted(levels))
        result.setdefault(epoch, {})[field] = hashlib.sha256(data).hexdigest()
    return {
        "file": path.name,
        "raw_sha256": metadata["sha256"],
        "pv_f32_le_sha256": pv_digest,
        "volumes_by_epoch": result,
    }



OMEGA_KEYS = {
    "edition": 2, "discipline": 0, "parameterCategory": 2, "parameterNumber": 8,
    "paramId": 135, "shortName": "w", "units": "Pa s**-1", "typeOfLevel": "hybrid",
    "NV": 276, "gridType": "regular_ll", "Ni": 65, "Nj": 41, "scanningMode": 0,
    "packingType": "grid_simple", "bitsPerValue": 16, "decimalScaleFactor": 0,
    "bitmapPresent": 0, "numberOfMissing": 0, "numberOfValues": 2665,
}


def audit_omega(manifest, pressure_pair):
    """Compare every omega message's identity with the verified ERA5 analysis pair."""
    request_bytes = manifest.read_bytes()
    metadata = json.loads(request_bytes)
    by_epoch = pressure_pair["by_epoch"]
    sequence = [(epoch, level) for epoch in sorted(by_epoch) for level in range(1, 138)]
    values = defaultdict(list)
    raw_frames = defaultdict(hashlib.sha256)
    packing_scales = set()
    count = 0
    for frame in grib2_frames(verified_grib(metadata)):
        message = eccodes.codes_new_from_message(frame)
        try:
            if eccodes.codes_get_long(message, "paramId") != 135:
                continue
            epoch, identity = join_identity(message, frame)
            level = eccodes.codes_get_long(message, "level")
            expected = by_epoch.get(epoch, {})
            if (
                message_keys(message, OMEGA_KEYS) != OMEGA_KEYS
                or eccodes.codes_get_long(message, "typeOfFirstFixedSurface") != 105
                or eccodes.codes_get_long(message, "typeOfSecondFixedSurface") != 255
                or eccodes.codes_get_long(message, "scaleFactorOfFirstFixedSurface") != 0
                or eccodes.codes_get_long(message, "scaledValueOfFirstFixedSurface") != level
                or any(expected.get(key) != value for key, value in identity.items())
                or count >= len(sequence)
                or sequence[count] != (epoch, level)
            ):
                raise ValueError(f"unexpected omega record: {epoch}/{level}")
            lat = eccodes.codes_get_array(message, "latitudes")
            lon = (eccodes.codes_get_array(message, "longitudes") + 180) % 360 - 180
            order = np.lexsort((lon, lat))
            np.testing.assert_array_equal(lat[order], np.repeat(np.linspace(43, 53, 41), 65))
            np.testing.assert_array_equal(lon[order], np.tile(np.linspace(-8, 8, 65), 41))
            cells = eccodes.codes_get_values(message)[order]
            if not (np.isfinite(cells) & (np.abs(cells) <= np.finfo(np.float32).max)).all():
                raise ValueError(f"invalid omega values: {epoch}/{level}")
            packing_scales.add(eccodes.codes_get_long(message, "binaryScaleFactor"))
            values[epoch].append(cells)
            raw_frames[epoch].update(frame)
            count += 1
        finally:
            eccodes.codes_release(message)
    if count != len(sequence):
        raise ValueError(f"incomplete omega coverage: {metadata['file']}")

    volumes = {}
    for epoch, levels in sorted(values.items()):
        raw = np.concatenate(levels).astype("<f8")
        grid = raw.astype("<f4")
        volumes[epoch] = {
            **{key: by_epoch[epoch][key] for key in ("time", "grid_section_sha256", "pv_f64_le_sha256")},
            "raw_omega_messages_sha256": raw_frames[epoch].hexdigest(),
            "raw_ec_codes_f64_le_sha256": sha256(raw.tobytes()),
            "volume_f32_le_sha256": sha256(grid.tobytes()),
            "cell_count": int(grid.size),
            "minimum_pa_s": float(grid.min()),
            "maximum_pa_s": float(grid.max()),
            "negative_count": int(np.count_nonzero(grid < 0)),
            "zero_count": int(np.count_nonzero(grid == 0)),
            "positive_count": int(np.count_nonzero(grid > 0)),
            "samples_pa_s_at_lon_0_lat_48_by_level": {
                level: float(grid[(level - 1) * 2665 + 20 * 65 + 32]) for level in (1, 69, 137)
            },
            "max_f64_to_f32_rounding_pa_s": float(np.abs(raw - grid).max()),
        }
    return {
        "file": metadata["file"],
        "raw_sha256": metadata["sha256"],
        "request_record_sha256": sha256(request_bytes),
        "matched_lnsp_raw_sha256": pressure_pair["pressure_sha256"],
        "binary_scale_factors": sorted(packing_scales),
        "omega_message_count": count,
        "by_epoch": volumes,
    }


def message_keys(message, keys):
    return {key: eccodes.codes_get(message, key) for key in keys}


def join_identity(message, frame):
    product = message_keys(message, PRODUCT_KEYS)
    if product != PRODUCT_KEYS:
        raise ValueError(f"unexpected ERA5 analysis metadata: {product}")
    time = message_keys(message, ("dataDate", "dataTime", "validityDate", "validityTime"))
    if (time["dataDate"], time["dataTime"]) != (time["validityDate"], time["validityTime"]):
        raise ValueError(f"analysis reference and validity differ: {time}")
    dt = datetime.strptime(f'{time["dataDate"]}{time["dataTime"]:04}', "%Y%m%d%H%M")
    epoch = int(dt.replace(tzinfo=timezone.utc).timestamp())
    start = eccodes.codes_get_long(message, "offsetSection3")
    end = start + eccodes.codes_get_long(message, "section3Length")
    pv = eccodes.codes_get_array(message, "pv")
    if pv.shape != (276,) or not np.isfinite(pv).all():
        raise ValueError("invalid native hybrid coefficients")
    return epoch, {
        "time": time,
        "grid_section_sha256": sha256(frame[start:end]),
        "pv_f64_le_sha256": sha256(pv.astype("<f8").tobytes()),
    }


def audit_lnsp(model_manifest, replay):
    model_manifest_bytes = model_manifest.read_bytes()
    model = json.loads(model_manifest_bytes)
    filename = model["file"].replace("-ml.grib", "-lnsp.request.json")
    manifest_bytes = (RAW / filename).read_bytes()
    pressure = json.loads(manifest_bytes)
    if (
        pressure["model_request_sha256"] != sha256(model_manifest_bytes)
        or pressure["model_sha256"] != model["sha256"]
        or pressure["dataset"] != model["dataset"]
        or pressure["source"] != model["source"]
        or pressure["request"] != {**model["request"], "levelist": "1", "param": "152"}
    ):
        raise ValueError(f"unexpected request lineage: {filename}")
    preserved_keys = ("source", "dataset", "request", "file", "bytes", "sha256")
    if (
        any(replay[key] != model[key] for key in preserved_keys)
        or replay["model_request_sha256"] != sha256(model_manifest_bytes)
        or replay["model_sha256"] != model["sha256"]
    ):
        raise ValueError(f"replayed CDS response differs from {model_manifest.name}")
    anchors = {}
    for frame in grib2_frames(verified_grib(model)):
        message = eccodes.codes_new_from_message(frame)
        try:
            if message_keys(message, ("paramId", "level")) == {"paramId": 130, "level": 1}:
                epoch, keys = join_identity(message, frame)
                if epoch in anchors:
                    raise ValueError(f"duplicate model join anchor: {epoch}")
                anchors[epoch] = keys
        finally:
            eccodes.codes_release(message)

    parameter = {
        "paramId": 152, "parameterCategory": 3, "parameterNumber": 25,
        "shortName": "lnsp", "typeOfLevel": "hybrid", "level": 1,
        "NV": 276, "packingType": "grid_simple", "bitsPerValue": 16,
        "bitmapPresent": 0, "numberOfMissing": 0,
    }
    by_epoch = {}
    for frame in grib2_frames(verified_grib(pressure)):
        message = eccodes.codes_new_from_message(frame)
        try:
            epoch, keys = join_identity(message, frame)
            if keys != anchors.get(epoch) or epoch in by_epoch:
                raise ValueError(f"unexpected pressure join identity: {epoch}")
            units = eccodes.codes_get(message, "units")
            if message_keys(message, parameter) != parameter or units not in ("Numeric", "~"):
                raise ValueError(f"unexpected lnsp parameter, level or packing: {epoch}")
            lat = eccodes.codes_get_array(message, "latitudes")
            lon = (eccodes.codes_get_array(message, "longitudes") + 180) % 360 - 180
            order = np.lexsort((lon, lat))
            np.testing.assert_array_equal(lat[order], np.repeat(np.linspace(43, 53, 41), 65))
            np.testing.assert_array_equal(lon[order], np.tile(np.linspace(-8, 8, 65), 41))
            lnsp = eccodes.codes_get_values(message)[order]
            pa = np.array([math.exp(value) for value in lnsp], dtype="<f4")
            if not ((pa >= 50_000) & (pa <= 110_000)).all():
                raise ValueError(f"invalid surface pressure: {epoch}")
            by_epoch[epoch] = {
                **keys,
                "pressure_message_sha256": sha256(frame),
                "lnsp_f64_le_sha256": sha256(lnsp.astype("<f8").tobytes()),
                "pressure_f32_le_sha256": sha256(pa.tobytes()),
                "pressure_pa_min": float(pa.min()),
                "pressure_pa_max": float(pa.max()),
            }
        finally:
            eccodes.codes_release(message)
    if len(anchors) != 3 or by_epoch.keys() != anchors.keys():
        raise ValueError(f"incomplete pressure coverage: {filename}")
    return {
        "model_sha256": model["sha256"],
        "pressure_sha256": pressure["sha256"],
        "pressure_request_sha256": sha256(manifest_bytes),
        "parameter": {**parameter, "accepted_units_labels": ["Numeric", "~"]},
        "by_epoch": dict(sorted(by_epoch.items())),
    }


def audit_surface(sources):
    metadata = json.loads((RAW / "surface-request.json").read_bytes())
    path = RAW / metadata["file"]
    raw = path.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if len(raw) != metadata["bytes"] or digest != metadata["sha256"]:
        raise ValueError(f"unexpected ARCO source identity: {path.name}")
    model_epochs = sorted(epoch for source in sources for epoch in source["volumes_by_epoch"])
    with np.load(BytesIO(raw), allow_pickle=False) as surface:
        check_surface_content(surface, metadata["times"], model_epochs)
    return {
        "file": path.name,
        "raw_sha256": digest,
        "bytes": len(raw),
        "valid_times_and_grid_match": True,
        "admission": "blocked_missing_original_analysis_identity",
    }


def check_surface_content(surface, expected_times, model_epochs):
    times = [time.decode("ascii") for time in surface["times"]]
    if times != expected_times:
        raise ValueError("ARCO archive times differ from its request record")
    epochs = [
        int(datetime.fromisoformat(time).replace(tzinfo=timezone.utc).timestamp())
        for time in times
    ]
    if epochs != model_epochs:
        raise ValueError("ARCO times differ from GRIB valid times")
    np.testing.assert_array_equal(surface["latitudes"], np.linspace(53, 43, 41))
    np.testing.assert_array_equal(surface["longitudes"], np.linspace(-8, 8, 65))
    pressure = surface["surface_pressure"]
    if pressure.shape != (6, 41, 65) or not (np.isfinite(pressure) & (pressure > 0)).all():
        raise ValueError("invalid ARCO surface pressure array")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="regenerate the checked-in evidence")
    args = parser.parse_args()
    result = {
        "encoding": "IEEE-754 f32 little-endian; x fastest, latitude ascending, levels 1 through 137",
        "sources": [audit(RAW / name) for name in MODEL_REQUESTS],
    }
    result["surface_audit"] = audit_surface(result["sources"])
    replay_bytes = REPLAY_MANIFEST.read_bytes()
    replay = json.loads(replay_bytes)
    if replay["profile"] != "era5-etex-model-replay-v1" or set(replay["sources"]) != set(MODEL_REQUESTS):
        raise ValueError("unexpected CDS replay manifest coverage")
    pairs = {
        "profile": "era5-etex-native-lnsp-pair-v1",
        "model_replay_manifest_sha256": sha256(replay_bytes),
        "product": PRODUCT_KEYS,
        "normalization": "Sort by ascending latitude and normalized longitude; exp(lnsp) in binary64, round to binary32 Pa",
        "sources": [audit_lnsp(RAW / name, replay["sources"][name]) for name in MODEL_REQUESTS],
    }
    omega = {
        "profile": "era5-etex-native-omega-v1",
        "decoder": "Python ecCodes values sorted by decoded cell coordinates and source level",
        "encoding": "IEEE-754 little-endian; x fastest, latitude ascending, levels 1 through 137",
        "parameter_keys": {**OMEGA_KEYS, "typeOfFirstFixedSurface": 105, "typeOfSecondFixedSurface": 255},
        "product_keys": PRODUCT_KEYS,
        "sources": [
            audit_omega(RAW / name, pair)
            for name, pair in zip(MODEL_REQUESTS, pairs["sources"], strict=True)
        ],
    }
    for path, evidence in ((EVIDENCE, result), (PAIR_EVIDENCE, pairs), (OMEGA_EVIDENCE, omega)):
        rendered = json.dumps(evidence, indent=2) + "\n"
        if args.write:
            path.write_bytes(rendered.encode("ascii"))
        elif rendered != path.read_text(encoding="ascii"):
            raise SystemExit(f"real-byte ecCodes reference digests differ: {path.name}")
    print(f"Verified 24 model volumes, 6 pressure grids and 6 omega volumes with Python ecCodes {eccodes.codes_get_api_version()}")


if __name__ == "__main__":
    main()
