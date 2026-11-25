#!/usr/bin/env python3
"""Build or verify offline, direct-ecCodes checksums of all ERA5/ETEX cells."""

import argparse
from collections import defaultdict
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path

import eccodes
import numpy as np


ROOT = Path(__file__).resolve().parents[2]
RAW = ROOT / "fixtures/etex/native-mini"
EVIDENCE = ROOT / "fixtures/meteorology/era5-etex-native-v1.value-digests.json"
FIELDS = {130: "temperature", 131: "wind_u", 132: "wind_v", 133: "specific_humidity"}


def audit(manifest):
    metadata = json.loads(manifest.read_bytes())
    path = RAW / metadata["file"]
    raw = path.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != metadata["sha256"] or len(raw) != metadata["bytes"]:
        raise ValueError(f"unexpected source identity: {path.name}")
    volumes = defaultdict(dict)
    sort_order = None
    pv_digest = None
    with path.open("rb") as file:
        while (message := eccodes.codes_grib_new_from_file(file)) is not None:
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
        "raw_sha256": digest,
        "pv_f32_le_sha256": pv_digest,
        "volumes_by_epoch": result,
    }


def audit_surface(sources):
    metadata = json.loads((RAW / "surface-request.json").read_bytes())
    path = RAW / metadata["file"]
    raw = path.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if len(raw) != metadata["bytes"] or digest != metadata["sha256"]:
        raise ValueError(f"unexpected ARCO source identity: {path.name}")
    model_epochs = sorted(epoch for source in sources for epoch in source["volumes_by_epoch"])
    with np.load(path, allow_pickle=False) as surface:
        times = [time.decode("ascii") for time in surface["times"]]
        if times != metadata["times"]:
            raise ValueError("ARCO archive times differ from its request record")
        epochs = [int(datetime.fromisoformat(time).replace(tzinfo=timezone.utc).timestamp())
                  for time in times]
        if epochs != model_epochs:
            raise ValueError("ARCO times differ from GRIB valid times")
        np.testing.assert_array_equal(surface["latitudes"], np.linspace(53, 43, 41))
        np.testing.assert_array_equal(surface["longitudes"], np.linspace(-8, 8, 65))
        pressure = surface["surface_pressure"]
        if pressure.shape != (6, 41, 65) or not (np.isfinite(pressure) & (pressure > 0)).all():
            raise ValueError("invalid ARCO surface pressure array")
    return {
        "file": path.name,
        "raw_sha256": digest,
        "bytes": len(raw),
        "valid_times_and_grid_match": True,
        "admission": "blocked_missing_original_analysis_identity",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="regenerate the checked-in evidence")
    args = parser.parse_args()
    result = {
        "encoding": "IEEE-754 f32 little-endian; x fastest, latitude ascending, levels 1 through 137",
        "sources": [audit(RAW / name) for name in ("request.json", "request-next-day.json")],
    }
    result["surface_audit"] = audit_surface(result["sources"])
    rendered = json.dumps(result, indent=2) + "\n"
    if args.write:
        EVIDENCE.write_bytes(rendered.encode("ascii"))
    elif rendered != EVIDENCE.read_text(encoding="ascii"):
        raise SystemExit("real-byte ecCodes reference digests differ")
    print(f"Verified 24 volume digests with Python ecCodes {eccodes.codes_get_api_version()}")


if __name__ == "__main__":
    main()
