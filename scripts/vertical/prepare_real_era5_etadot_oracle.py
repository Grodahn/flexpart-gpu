#!/usr/bin/env python3
"""Prepare one complete real ERA5 137-level column for calc_etadot (#70).

The checked-in ETEX native-mini fixture contains independent ERA5 Complete
fields on all 137 hybrid model levels.  We select the real column at
48.0 N, 2.0 W (the ETEX release-area grid point) and replicate that column on
flex_extract's small 6x6 installation-test grid.

This is intentional: calc_etadot requires spectral ln(ps) in fort.12. A
spatially constant field can be represented exactly by the T0 spherical-
harmonic coefficient, so the selected column's *real* surface pressure can be
fed to the pristine oracle without an additional grid-to-spectral
interpolation library. Every vertical value used by the test comes from the
verified ERA5 source column; replication only supplies calc_etadot's horizontal
work array.

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
SOURCE_NX = 65
SOURCE_NY = 41
SOURCE_X = 24
SOURCE_Y_DESC = 20
SOURCE_LON_DEG = -2.0
SOURCE_LAT_DEG = 48.0
ORACLE_NX = 6
ORACLE_NY = 6
ORACLE_POINTS = ORACLE_NX * ORACLE_NY

LEVEL_PARAMS = (130, 131, 132, 133)
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


def sha256_json(value) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


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


def source_point(values) -> float:
    array = np.asarray(values, dtype=np.float64)
    if array.size != SOURCE_NX * SOURCE_NY:
        raise ValueError(f"source ERA5 field has {array.size} values, expected {SOURCE_NX * SOURCE_NY}")
    value = float(array.reshape(SOURCE_NY, SOURCE_NX)[SOURCE_Y_DESC, SOURCE_X])
    if not np.isfinite(value):
        raise ValueError("selected ERA5 column contains a non-finite value")
    return value


def read_real_profiles(path: Path, date: int, hhmm: int):
    profiles = {param: {} for param in LEVEL_PARAMS}
    pv = None
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
                if param not in LEVEL_PARAMS or not 1 <= level <= LEVEL_COUNT:
                    continue
                if (
                    ec.codes_get_long(handle, "Ni") != SOURCE_NX
                    or ec.codes_get_long(handle, "Nj") != SOURCE_NY
                ):
                    raise ValueError("native ERA5 model-level grid differs from the pinned fixture")
                this_pv = np.asarray(ec.codes_get_array(handle, "pv"), dtype=np.float64)
                if len(this_pv) != 2 * (LEVEL_COUNT + 1):
                    raise ValueError("native ERA5 message lacks the complete 138-interface PV array")
                if pv is None:
                    pv = this_pv
                elif not np.array_equal(pv, this_pv):
                    raise ValueError("native ERA5 messages have inconsistent hybrid coefficients")
                if level in profiles[param]:
                    raise ValueError(f"duplicate ERA5 model-level field: param {param}, level {level}")
                profiles[param][level] = source_point(ec.codes_get_values(handle))
            finally:
                ec.codes_release(handle)

    expected = set(range(1, LEVEL_COUNT + 1))
    for param, profile in profiles.items():
        if set(profile) != expected:
            raise ValueError(
                f"ERA5 param {param} column coverage is incomplete: "
                f"{len(profile)}/{LEVEL_COUNT} levels"
            )
    if pv is None:
        raise ValueError("ERA5 hybrid coefficients were not found")
    return profiles, pv


def read_real_etadot(path: Path, date: int, hhmm: int):
    profile = {}
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
                if (
                    ec.codes_get_long(handle, "Ni") != SOURCE_NX
                    or ec.codes_get_long(handle, "Nj") != SOURCE_NY
                ):
                    raise ValueError("native ERA5 eta-dot grid differs from the pinned fixture")
                if level in profile:
                    raise ValueError(f"duplicate ERA5 eta-dot level {level}")
                profile[level] = source_point(ec.codes_get_values(handle))
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
    if set(profile) != set(range(1, LEVEL_COUNT + 1)):
        raise ValueError(f"ERA5 eta-dot column coverage is incomplete: {len(profile)}/{LEVEL_COUNT}")
    return profile, metadata


def first_message(path: Path) -> bytes:
    with path.open("rb") as source:
        handle = ec.codes_grib_new_from_file(source)
        if handle is None:
            raise ValueError(f"empty GRIB template: {path}")
        try:
            return ec.codes_get_message(handle)
        finally:
            ec.codes_release(handle)


def write_replicated_levels(
    template_message: bytes,
    profiles: dict,
    pv: np.ndarray,
    date: int,
    hhmm: int,
    output_dir: Path,
):
    messages_by_fort = {name: [] for name in set(FORT_BY_PARAM.values())}
    for param in LEVEL_PARAMS:
        for level in range(1, LEVEL_COUNT + 1):
            handle = ec.codes_new_from_message(template_message)
            try:
                ec.codes_set_long(handle, "paramId", param)
                ec.codes_set_long(handle, "dataDate", date)
                ec.codes_set_long(handle, "dataTime", hhmm)
                ec.codes_set_long(handle, "level", level)
                ec.codes_set_array(handle, "pv", pv)
                ec.codes_set_values(
                    handle,
                    np.full(ORACLE_POINTS, profiles[param][level], dtype=np.float64),
                )
                if (
                    ec.codes_get_long(handle, "Ni") != ORACLE_NX
                    or ec.codes_get_long(handle, "Nj") != ORACLE_NY
                ):
                    raise ValueError("regular oracle template is not 6x6")
                messages_by_fort[FORT_BY_PARAM[param]].append(ec.codes_get_message(handle))
            finally:
                ec.codes_release(handle)

    for name, messages in messages_by_fort.items():
        with (output_dir / name).open("wb") as sink:
            for message in messages:
                sink.write(message)


def write_replicated_etadot(
    template_message: bytes,
    profile: dict,
    pv: np.ndarray,
    date: int,
    hhmm: int,
    output_path: Path,
):
    with output_path.open("wb") as sink:
        for level in range(1, LEVEL_COUNT + 1):
            handle = ec.codes_new_from_message(template_message)
            try:
                ec.codes_set_long(handle, "paramId", 77)
                ec.codes_set_long(handle, "dataDate", date)
                ec.codes_set_long(handle, "dataTime", hhmm)
                ec.codes_set_long(handle, "level", level)
                ec.codes_set_array(handle, "pv", pv)
                ec.codes_set_values(
                    handle,
                    np.full(ORACLE_POINTS, profile[level], dtype=np.float64),
                )
                if (
                    ec.codes_get_long(handle, "Ni") != ORACLE_NX
                    or ec.codes_get_long(handle, "Nj") != ORACLE_NY
                ):
                    raise ValueError("eta-dot oracle template is not 6x6")
                sink.write(ec.codes_get_message(handle))
            finally:
                ec.codes_release(handle)


def write_constant_spectral_lnsp(
    template_message: bytes,
    pv: np.ndarray,
    surface_pressure_pa: float,
    date: int,
    hhmm: int,
    output_path: Path,
):
    handle = ec.codes_new_from_message(template_message)
    try:
        if ec.codes_get(handle, "gridType") not in ("sh", "rotated_sh", "stretched_sh", "stretched_rotated_sh"):
            raise ValueError("fort.12 template is not a spherical-harmonic field")
        ec.codes_set_long(handle, "paramId", 152)
        ec.codes_set_long(handle, "dataDate", date)
        ec.codes_set_long(handle, "dataTime", hhmm)
        ec.codes_set_array(handle, "pv", pv)

        # ECMWF's normalized Y_0^0 basis equals one. Therefore the first real
        # spectral coefficient represents the spatial constant directly; all
        # other real/imaginary coefficients are zero.
        size = ec.codes_get_size(handle, "values")
        coefficients = np.zeros(size, dtype=np.float64)
        coefficients[0] = float(np.log(surface_pressure_pa))
        ec.codes_set_values(handle, coefficients)

        if ec.codes_get_long(handle, "NV") != 2 * (LEVEL_COUNT + 1):
            raise ValueError("spectral ln(ps) lost the complete 138-interface PV array")
        output_path.write_bytes(ec.codes_get_message(handle))
    finally:
        ec.codes_release(handle)


def write_namelist(template_path: Path, output_path: Path):
    text = template_path.read_text(encoding="utf-8")
    replacements = {
        "mlevel = 91": "mlevel = 137",
        'mlevelist = "88/to/91"': 'mlevelist = "1/to/137"',
    }
    for old, new in replacements.items():
        if old not in text:
            raise ValueError(f"unexpected pinned fort.4 template; missing {old!r}")
        text = text.replace(old, new)
    output_path.write_text(text, encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-dir", type=Path, required=True)
    parser.add_argument("--flex-extract-checkout", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--timestamp", default=TIMESTAMP)
    args = parser.parse_args()

    base = args.native_dir.resolve()
    checkout = args.flex_extract_checkout.resolve()
    example = checkout / "Testing" / "Installation" / "Calc_etadot"
    out = args.output_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)

    native_record = verify_manifest_file(base, "request.json", NATIVE_FILE)
    etadot_record = verify_manifest_file(base, "request-etadot.json", ETADOT_FILE)
    surface_record = verify_manifest_file(base, "surface-request.json", SURFACE_FILE)

    date, hhmm = timestamp_parts(args.timestamp)
    profiles, pv = read_real_profiles(base / NATIVE_FILE, date, hhmm)
    eta_profile, eta_metadata = read_real_etadot(base / ETADOT_FILE, date, hhmm)

    with np.load(base / SURFACE_FILE) as archive:
        times = [
            value.decode() if isinstance(value, bytes) else str(value)
            for value in archive["times"]
        ]
        if args.timestamp not in times:
            raise ValueError(f"timestamp {args.timestamp} not present in surface fixture")
        idx = times.index(args.timestamp)
        latitudes = np.asarray(archive["latitudes"], dtype=np.float64)
        longitudes = np.asarray(archive["longitudes"], dtype=np.float64)
        if latitudes[SOURCE_Y_DESC] != SOURCE_LAT_DEG or longitudes[SOURCE_X] != SOURCE_LON_DEG:
            raise ValueError("selected ETEX ERA5 source-column indices changed")
        surface_pressure_pa = float(
            np.asarray(archive["surface_pressure"][idx], dtype=np.float64)[
                SOURCE_Y_DESC, SOURCE_X
            ]
        )
        if not np.isfinite(surface_pressure_pa) or surface_pressure_pa <= 0.0:
            raise ValueError("selected real ERA5 surface pressure is invalid")

    regular_template = first_message(example / "fort.21")
    spectral_template = first_message(example / "fort.12")
    write_replicated_levels(regular_template, profiles, pv, date, hhmm, out)
    write_replicated_etadot(
        regular_template, eta_profile, pv, date, hhmm, out / "fort.21"
    )
    write_constant_spectral_lnsp(
        spectral_template, pv, surface_pressure_pa, date, hhmm, out / "fort.12"
    )
    write_namelist(example / "fort.4", out / "fort.4")

    required = ["fort.4", "fort.10", "fort.11", "fort.12", "fort.17", "fort.21"]
    generated = {
        name: {
            "path": str(out / name),
            "bytes": (out / name).stat().st_size,
            "sha256": sha256(out / name),
        }
        for name in required
    }
    selected_profiles = {
        str(param): [profiles[param][level] for level in range(1, LEVEL_COUNT + 1)]
        for param in LEVEL_PARAMS
    }
    selected_profiles["77"] = [eta_profile[level] for level in range(1, LEVEL_COUNT + 1)]
    selected_profiles_sha256 = sha256_json(selected_profiles)

    provenance = {
        "schema": "flexpart-gpu.etadot-real-era5-case.v1",
        "classification": "real_era5_native_model_level_full_column",
        "timestamp": args.timestamp + "Z",
        "source_grid": {"nx": SOURCE_NX, "ny": SOURCE_NY},
        "selected_column": {
            "x_index": SOURCE_X,
            "y_index_north_to_south": SOURCE_Y_DESC,
            "longitude_deg": SOURCE_LON_DEG,
            "latitude_deg": SOURCE_LAT_DEG,
            "surface_pressure_pa": surface_pressure_pa,
            "model_levels": LEVEL_COUNT,
            "level_coverage": "1/to/137",
            "profile_values_sha256": selected_profiles_sha256,
            "profile_values": selected_profiles,
        },
        "oracle_grid": {
            "nx": ORACLE_NX,
            "ny": ORACLE_NY,
            "construction": "selected real ERA5 column replicated horizontally",
            "surface_pressure_representation": "constant ln(ps) encoded as T0 spherical-harmonic coefficient",
        },
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
        "Prepared complete real ERA5 column for calc_etadot: "
        f"{SOURCE_LAT_DEG:.2f}N {SOURCE_LON_DEG:.2f}E, "
        f"{LEVEL_COUNT}/{LEVEL_COUNT} eta-dot levels, replicated to "
        f"{ORACLE_NX}x{ORACLE_NY}"
    )


if __name__ == "__main__":
    main()
