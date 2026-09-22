#!/usr/bin/env python3
"""Extract a pinned flex_extract calc_etadot oracle run into canonical JSONs.

Container-side script (runs in the flexpart-fortran image, which ships
python3-eccodes extended with libemos-dev/libemos-bin so the pinned
calc_etadot example can be executed). It consumes the products of an oracle
run:

  <example-dir>/fort.21   raw eta-coordinate velocity (GRIB parameter 77)
  <example-dir>/fort.15   transformed pressure velocity (77) + t (130) + sp (134)
  <example-dir>/fort.12   ln(ps) spectral field + vertical coordinate pv

and writes

  --output-dir/snapshot.json   canonical snapshot (surface_pressure, hybrid
                               vertical coordinate in interface index 0 = top)
  --output-dir/motion.json     eta_coordinate_velocity motion (per second,
                               level-centre staggered, raw deta/dt values)
  --output-dir/oracle.json     oracle Etadot reference field (Pa/s) plus inputs

The snapshot+oracle writing rules are the ones validated against the pinned
rust candidate (eta_dot_to_pressure_velocity, mdpdeta=1):

  * candidate interface index K == calc_etadot level K (both top-first);
  * motion values are indexed by level k-1 (level 1 first = top), 0 for the
    pristine levels below the MLEVELIST window (READLATLON zero-fills);
  * the vertical coordinate uses one canonical reference surface pressure
    (max of the psi-field) so that a+b*ps_ref reconstructs an interface row.
"""

import argparse
import datetime as dt
import hashlib
import json
import os
import sys

import eccodes as ec

ETADOT_PARAM = 77
SP_PARAM = 134

GRID_KEYS = [
    "latitudeOfFirstGridPointInDegrees",
    "longitudeOfFirstGridPointInDegrees",
    "iDirectionIncrementInDegrees",
    "jDirectionIncrementInDegrees",
    "Ni",
    "Nj",
]

PROVENANCE_GRIB_KEYS = [
    "edition",
    "centre",
    "dataDate",
    "dataTime",
    "validityDate",
    "validityTime",
    "gridType",
    "typeOfLevel",
    "stepType",
    "paramId",
    "shortName",
]


def sha256_file(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def safe_grib_metadata(handle):
    result = {}
    for key in PROVENANCE_GRIB_KEYS:
        try:
            result[key] = ec.codes_get(handle, key)
        except Exception:
            result[key] = None
    return result


def valid_time_epoch_seconds(metadata):
    date = metadata.get("validityDate") or metadata.get("dataDate")
    time = metadata.get("validityTime")
    if time is None:
        time = metadata.get("dataTime")
    if date is None or time is None:
        raise ValueError("raw GRIB lacks date/time metadata for canonical valid_time")
    date = int(date)
    time = int(time)
    year = date // 10000
    month = (date // 100) % 100
    day = date % 100
    hour = time // 100
    minute = time % 100
    return int(
        dt.datetime(year, month, day, hour, minute, tzinfo=dt.timezone.utc).timestamp()
    )


def read_messages(path, param_filter=None, level_filter=None):
    out = []
    with open(path, "rb") as f:
        while True:
            g = ec.codes_grib_new_from_file(f)
            if g is None:
                break
            param = ec.codes_get(g, "paramId")
            level = ec.codes_get(g, "level")
            if param_filter is None or param in param_filter:
                if level_filter is None or level in level_filter:
                    out.append((param, level, list(ec.codes_get_values(g))))
            ec.codes_release(g)
    return out


def read_first_handle(path):
    with open(path, "rb") as f:
        g = ec.codes_grib_new_from_file(f)
        if g is None:
            raise ValueError(f"empty GRIB file: {path}")
        result = g
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--example-dir", type=str, required=True)
    parser.add_argument("--output-dir", type=str, required=True)
    parser.add_argument("--valid-time-epoch-seconds", type=int, default=None)
    args = parser.parse_args()

    example_dir = args.example_dir
    output_dir = args.output_dir
    os.makedirs(output_dir, exist_ok=True)

    raw = read_messages(os.path.join(example_dir, "fort.21"), param_filter=[ETADOT_PARAM])
    oracle = read_messages(os.path.join(example_dir, "fort.15"), param_filter=[ETADOT_PARAM])
    sp = read_messages(os.path.join(example_dir, "fort.15"), param_filter=[SP_PARAM])
    if not raw or not oracle or not sp:
        raise ValueError("missing oracle field (fort.21 raw, fort.15 etadot and sp)")

    raw_path = os.path.join(example_dir, "fort.21")
    oracle_path_grib = os.path.join(example_dir, "fort.15")
    hybrid_path = os.path.join(example_dir, "fort.12")

    handle = read_first_handle(raw_path)
    try:
        grid = {key: ec.codes_get(handle, key) for key in GRID_KEYS}
        raw_grib_metadata = safe_grib_metadata(handle)
    finally:
        ec.codes_release(handle)

    valid_time = (
        args.valid_time_epoch_seconds
        if args.valid_time_epoch_seconds is not None
        else valid_time_epoch_seconds(raw_grib_metadata)
    )

    handle = read_first_handle(hybrid_path)
    try:
        pv = list(ec.codes_get_array(handle, "pv"))
    finally:
        ec.codes_release(handle)

    nlev = len(pv) // 2 - 1
    a = pv[: nlev + 1]
    b = pv[nlev + 1 :]

    nx, ny = grid["Ni"], grid["Nj"]
    nxy = nx * ny
    ps_flat = sp[0][2]
    if len(ps_flat) != nxy:
        raise ValueError("surface-pressure field length mismatch")

    levels = sorted({lvl for (_, lvl, _) in raw})
    raw_by_level = {lvl: vals for (_, lvl, vals) in raw}
    oracle_by_level = {lvl: vals for (_, lvl, vals) in oracle}
    expected_first_levels = {88, 89, 90, 91}
    if set(levels) != expected_first_levels:
        raise ValueError(
            f"unexpected raw etadot level set {levels} "
            f"(expected {sorted(expected_first_levels)} for the pinned example)"
        )

    motion_values = [0.0] * (nlev * nxy)
    for lvl in levels:
        for idx in range(nxy):
            motion_values[(lvl - 1) * nxy + idx] = raw_by_level[lvl][idx]

    reference_ps = float(max(ps_flat))
    snapshot = {
        "schema": {"id": "flexpart-gpu.canonical-meteorology", "version": 1},
        "horizontal_grid": {
            "nx": nx,
            "ny": ny,
            "xlon0_deg": grid["longitudeOfFirstGridPointInDegrees"],
            "ylat0_deg": grid["latitudeOfFirstGridPointInDegrees"],
            "dx_deg": grid["iDirectionIncrementInDegrees"],
            "dy_deg": grid["jDirectionIncrementInDegrees"],
            "longitude_domain": "minus180_to180",
        },
        "vertical_coordinate": {
            "kind": "hybrid_sigma_pressure",
            "reference": "model_native",
            "ordering": "increasing",
            "level_values": [
                0.5 * (a[k] + b[k] * reference_ps + a[k + 1] + b[k + 1] * reference_ps)
                for k in range(nlev)
            ],
            "interface_values": [a[k] + b[k] * reference_ps for k in range(nlev + 1)],
            "hybrid_a_interface_pa": [float(x) for x in a],
            "hybrid_b_interface": [float(x) for x in b],
            "reference_surface_pressure_pa": reference_ps,
            "surface_pressure_dependency": "surface_pressure",
        },
        "fields": [
            {
                "id": "surface_pressure",
                "shape": [nx, ny],
                "axis_order": ["x", "y"],
                "storage_order": "x_fastest",
                "unit": "pascal",
                "sign": "non_negative",
                "horizontal_staggering": "cell_center",
                "vertical_staggering": "not_applicable",
                "time": {
                    "calendar": "gregorian",
                    "kind": "instantaneous",
                    "valid_time_epoch_seconds": valid_time,
                },
                "values": [float(v) for v in ps_flat],
            }
        ],
    }

    motion = {
        "kind": "eta_coordinate_velocity",
        "unit": "per_second",
        "sign": "positive_eta_decreasing",
        "vertical_staggering": "level_center",
        "values": [float(v) for v in motion_values],
        "provenance": {
            "source_id": "flex-extract-7.1.2-Calc_etadot-installation-example"
        },
    }

    reference = {
        "oracle_tag": "flex_extract-7.1.2-Calc_etadot-installation-example",
        "source_fixture": {
            "classification": "upstream_flex_extract_native_model_level",
            "origin": "Testing/Installation/Calc_etadot at the pinned pristine flex_extract revision",
            "raw_grib_metadata": raw_grib_metadata,
            "source_hashes_sha256": {
                "fort.12": sha256_file(hybrid_path),
                "fort.21": sha256_file(raw_path),
                "fort.15": sha256_file(oracle_path_grib),
            },
        },
        "nx": nx,
        "ny": ny,
        "nlev": nlev,
        "levels_present": levels,
        "surface_pressure_pa_xy": [float(v) for v in ps_flat],
        "raw_etadot_per_s_xy_by_level": {
            str(lvl): [float(v) for v in raw_by_level[lvl]] for lvl in levels
        },
        "oracle_etadot_pa_s_xy_by_level": {
            str(lvl): [float(v) for v in oracle_by_level[lvl]] for lvl in levels
        },
    }

    snapshot_path = os.path.join(output_dir, "snapshot.json")
    motion_path = os.path.join(output_dir, "motion.json")
    oracle_path = os.path.join(output_dir, "oracle.json")
    for path, payload in (
        (snapshot_path, snapshot),
        (motion_path, motion),
        (oracle_path, reference),
    ):
        with open(path, "w") as f:
            json.dump(payload, f, indent=2)
    print(
        f"wrote {snapshot_path}, {motion_path}, {oracle_path} "
        f"(nx={nx}, ny={ny}, nlev={nlev}, levels={levels})",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()