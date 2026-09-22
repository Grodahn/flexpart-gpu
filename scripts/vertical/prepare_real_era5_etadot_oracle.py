#!/usr/bin/env python3
"""Prepare a full real ERA5 137-level calc_etadot oracle case for #70.

The checked-in ETEX native-mini fixture contains independent ERA5 Complete
model-level data and eta-coordinate vertical velocity (param 77) on all 137
hybrid levels. This script slices one timestamp into the fort.* files consumed
by the pinned flex_extract calc_etadot executable and records source lineage.

No candidate output is used to construct the oracle inputs.
"""

import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path

import eccodes as ec
import numpy as np

TIMESTAMP = "1994-10-23T15:00:00"
NATIVE_FILE = "era5-19941023-151821-ml.grib"
ETADOT_FILE = "era5-19941023-151821-etadot.grib"
SURFACE_FILE = "era5-surface-19941023-24.npz"
LEVEL_COUNT = 137
NX = 65
NY = 41

FORT_BY_PARAM = {
    130: "fort.11",  # temperature
    131: "fort.10",  # u
    132: "fort.10",  # v
    133: "fort.17",  # q
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_manifest_file(base: Path, manifest_name: str, file_name: str) -> dict:
    manifest_path = base / manifest_name
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest_name == "request-etadot.json":
        matches = [entry for entry in manifest["files"] if entry["file"] == file_name]
        if len(matches) != 1:
            raise ValueError(f"{file_name} is not uniquely declared in {manifest_name}")
        record = matches[0]
    else:
        if manifest.get("file") != file_name:
            raise ValueError(f"{manifest_name} does not declare {file_name}")
        record = manifest
    path = base / file_name
    if not path.is_file():
        raise FileNotFoundError(path)
    if path.stat().st_size != int(record["bytes"]):
        raise ValueError(f"{file_name} byte count differs from manifest")
    digest = sha256(path)
    if digest != record["sha256"]:
        raise ValueError(f"{file_name} SHA-256 differs from manifest")
    return {
        "manifest": str(manifest_path),
        "path": str(path),
        "bytes": path.stat().st_size,
        "sha256": digest,
    }


def timestamp_parts(timestamp: str) -> tuple[int, int]:
    parsed = dt.datetime.fromisoformat(timestamp)
    return int(parsed.strftime("%Y%m%d")), parsed.hour * 100


def select_model_messages(path: Path, date: int, hhmm: int, output_dir: Path):
    handles_by_param = {param: [] for param in FORT_BY_PARAM}
    pv = None
    template_message = None

    with path.open("rb") as source:
        while True:
            handle = ec.codes_grib_new_from_file(source)
            if handle is None:
                break
            try:
                if ec.codes_get_long(handle, "dataDate") != date:
                    continue
                if ec.codes_get_long(handle, "dataTime") != hhmm:
                    continue
                param = ec.codes_get_long(handle, "paramId")
                level = ec.codes_get_long(handle, "level")
                if param not in FORT_BY_PARAM or not 1 <= level <= LEVEL_COUNT:
                    continue
                if ec.codes_get_long(handle, "Ni") != NX or ec.codes_get_long(handle, "Nj") != NY:
                    raise ValueError("native ERA5 grid dimensions differ from the pinned fixture")
                this_pv = np.asarray(ec.codes_get_array(handle, "pv"), dtype=np.float64)
                if len(this_pv) != 2 * (LEVEL_COUNT + 1):
                    raise ValueError("native ERA5 message lacks the complete 138-interface PV array")
                if pv is None:
                    pv = this_pv
                elif not np.array_equal(pv, this_pv):
                    raise ValueError("native ERA5 messages have inconsistent hybrid coefficients")
                values = np.asarray(ec.codes_get_values(handle))
                if values.size != NX * NY or not np.isfinite(values).all():
                    raise ValueError(f"invalid ERA5 model-level values for param {param} level {level}")
                handles_by_param[param].append((level, ec.codes_get_message(handle)))
                if template_message is None and param == 130 and level == 1:
                    template_message = ec.codes_get_message(handle)
            finally:
                ec.codes_release(handle)

    for param, messages in handles_by_param.items():
        levels = {level for level, _ in messages}
        expected = set(range(1, LEVEL_COUNT + 1))
        if levels != expected or len(messages) != LEVEL_COUNT:
            raise ValueError(
                f"ERA5 param {param} coverage is incomplete: "
                f"{len(messages)} messages / {len(levels)} unique levels"
            )

    for target in sorted(set(FORT_BY_PARAM.values())):
        with (output_dir / target).open("wb") as sink:
            for param in sorted(p for p, fort in FORT_BY_PARAM.items() if fort == target):
                for _, raw in sorted(handles_by_param[param]):
                    sink.write(raw)

    if pv is None or template_message is None:
        raise ValueError("could not obtain ERA5 hybrid coefficients/template message")
    return pv, template_message


def select_etadot(path: Path, date: int, hhmm: int, output_dir: Path):
    messages = []
    metadata = None
    with path.open("rb") as source:
        while True:
            handle = ec.codes_grib_new_from_file(source)
            if handle is None:
                break
            try:
                if ec.codes_get_long(handle, "dataDate") != date:
                    continue
                if ec.codes_get_long(handle, "dataTime") != hhmm:
                    continue
                if ec.codes_get_long(handle, "paramId") != 77:
                    continue
                level = ec.codes_get_long(handle, "level")
                if not 1 <= level <= LEVEL_COUNT:
                    continue
                if ec.codes_get_long(handle, "Ni") != NX or ec.codes_get_long(handle, "Nj") != NY:
                    raise ValueError("ERA5 eta-dot grid dimensions differ from the pinned fixture")
                values = np.asarray(ec.codes_get_values(handle))
                if values.size != NX * NY or not np.isfinite(values).all():
                    raise ValueError(f"invalid ERA5 eta-dot values at level {level}")
                messages.append((level, ec.codes_get_message(handle)))
                if metadata is None:
                    metadata = {
                        key: ec.codes_get(handle, key)
                        for key in (
                            "edition", "centre", "dataDate", "dataTime",
                            "gridType", "typeOfLevel", "paramId", "shortName"
                        )
                    }
            finally:
                ec.codes_release(handle)

    levels = {level for level, _ in messages}
    expected = set(range(1, LEVEL_COUNT + 1))
    if levels != expected or len(messages) != LEVEL_COUNT:
        raise ValueError(
            f"ERA5 eta-dot coverage is incomplete: "
            f"{len(messages)} messages / {len(levels)} unique levels"
        )
    with (output_dir / "fort.21").open("wb") as sink:
        for _, raw in sorted(messages):
            sink.write(raw)
    return metadata


def write_lnsp(template_message: bytes, pv: np.ndarray, surface_pressure: np.ndarray,
               date: int, hhmm: int, output_path: Path):
    if surface_pressure.shape != (NY, NX):
        raise ValueError(
            f"surface pressure shape {surface_pressure.shape} != {(NY, NX)}"
        )
    if not np.isfinite(surface_pressure).all() or np.any(surface_pressure <= 0.0):
        raise ValueError("surface pressure contains invalid values")

    handle = ec.codes_new_from_message(template_message)
    try:
        ec.codes_set_long(handle, "paramId", 152)
        ec.codes_set_long(handle, "dataDate", date)
        ec.codes_set_long(handle, "dataTime", hhmm)
        ec.codes_set_long(handle, "level", 1)
        ec.codes_set_array(handle, "pv", pv)
        ec.codes_set_values(handle, np.log(surface_pressure).reshape(-1))
        if ec.codes_get_long(handle, "Ni") != NX or ec.codes_get_long(handle, "Nj") != NY:
            raise ValueError("generated lnsp grid dimensions changed unexpectedly")
        if ec.codes_get_long(handle, "NV") != 2 * (LEVEL_COUNT + 1):
            raise ValueError("generated lnsp message lost the 138-interface PV array")
        values = np.asarray(ec.codes_get_values(handle))
        if not np.isfinite(values).all():
            raise ValueError("generated lnsp field contains non-finite values")
        output_path.write_bytes(ec.codes_get_message(handle))
    finally:
        ec.codes_release(handle)


def write_namelist(output_path: Path):
    output_path.write_text(
        """&NAMGEN
  maxl = 65,
  maxb = 41,
  mlevel = 137,
  mlevelist = "1/to/137",
  mnauf = 639,
  metapar = 77,
  rlo0 = -8.0,
  rlo1 = 8.0,
  rla0 = 43.0,
  rla1 = 53.0,
  momega = 0,
  momegadiff = 0,
  mgauss = 0,
  msmooth = 0,
  meta = 1,
  metadiff = 0,
  mdpdeta = 1
/
""",
        encoding="utf-8",
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--timestamp", default=TIMESTAMP)
    args = parser.parse_args()

    base = args.native_dir.resolve()
    out = args.output_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)

    native_record = verify_manifest_file(base, "request.json", NATIVE_FILE)
    etadot_record = verify_manifest_file(base, "request-etadot.json", ETADOT_FILE)
    surface_record = verify_manifest_file(base, "surface-request.json", SURFACE_FILE)

    date, hhmm = timestamp_parts(args.timestamp)
    pv, template = select_model_messages(base / NATIVE_FILE, date, hhmm, out)
    eta_metadata = select_etadot(base / ETADOT_FILE, date, hhmm, out)

    with np.load(base / SURFACE_FILE) as archive:
        times = [value.decode() if isinstance(value, bytes) else str(value)
                 for value in archive["times"]]
        if args.timestamp not in times:
            raise ValueError(f"timestamp {args.timestamp} not present in surface fixture")
        idx = times.index(args.timestamp)
        surface_pressure = np.asarray(archive["surface_pressure"][idx], dtype=np.float64)

    write_lnsp(template, pv, surface_pressure, date, hhmm, out / "fort.12")
    write_namelist(out / "fort.4")

    required = ["fort.4", "fort.10", "fort.11", "fort.12", "fort.17", "fort.21"]
    generated = {
        name: {
            "path": str(out / name),
            "bytes": (out / name).stat().st_size,
            "sha256": sha256(out / name),
        }
        for name in required
    }
    provenance = {
        "schema": "flexpart-gpu.etadot-real-era5-case.v1",
        "classification": "real_era5_native_model_level_full_column",
        "timestamp": args.timestamp + "Z",
        "grid": {"nx": NX, "ny": NY},
        "vertical": {
            "model_levels": LEVEL_COUNT,
            "level_coverage": "1/to/137",
            "interface_coefficients": len(pv) // 2,
        },
        "raw_eta_metadata": eta_metadata,
        "sources": {
            "model_levels": native_record,
            "eta_dot": etadot_record,
            "surface": surface_record,
        },
        "generated_inputs": generated,
    }
    (out / "source-provenance.json").write_text(
        json.dumps(provenance, indent=2) + "\n", encoding="utf-8"
    )
    print(
        f"Prepared real ERA5 calc_etadot case at {out}: "
        f"{LEVEL_COUNT}/{LEVEL_COUNT} eta-dot levels, {NX}x{NY} grid"
    )


if __name__ == "__main__":
    main()
