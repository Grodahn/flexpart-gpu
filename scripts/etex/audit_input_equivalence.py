#!/usr/bin/env python3
"""Audit ETEX oracle/candidate inputs without inferring parity from shared files."""

import argparse
import hashlib
import json
import math
import re
from datetime import datetime
from pathlib import Path

import eccodes
import numpy as np


TIMES = ("1994-10-23T15:00:00", "1994-10-23T18:00:00",
         "1994-10-23T21:00:00", "1994-10-24T00:00:00",
         "1994-10-24T03:00:00", "1994-10-24T06:00:00")
LEVEL_PARAMS = {130: "temperature_k", 131: "u_m_s", 132: "v_m_s",
                133: "specific_humidity", 135: "omega_pa_s", 77: "etadot_s_inv"}
SURFACE_PARAMS = {134: "surface_pressure", 165: "10m_u_component_of_wind",
                  166: "10m_v_component_of_wind", 167: "2m_temperature",
                  168: "2m_dewpoint_temperature", 159: "boundary_layer_height",
                  146: "surface_sensible_heat_flux",
                  176: "surface_net_solar_radiation",
                  180: "mean_eastward_turbulent_surface_stress",
                  181: "mean_northward_turbulent_surface_stress",
                  143: "convective_precipitation", 142: "large_scale_precipitation",
                  173: "forecast_surface_roughness",
                  129: "geopotential_at_surface", 172: "land_sea_mask"}
GPU_3D = ("u_m_s", "v_m_s", "w_m_s", "temperature_k", "specific_humidity",
          "pressure_pa", "air_density_kg_m3", "density_gradient_kg_m2")
GPU_2D = ("surface_pressure", "10m_u_component_of_wind",
          "10m_v_component_of_wind", "2m_temperature", "2m_dewpoint_temperature",
          "large_scale_precipitation", "convective_precipitation",
          "surface_sensible_heat_flux", "surface_net_solar_radiation",
          "surface_stress", "friction_velocity", "convective_velocity_scale",
          "boundary_layer_height", "tropopause_height", "inverse_obukhov_length")
SAMPLE_COLUMNS = ((0, 0), (24, 20), (32, 20), (48, 20), (64, 40))
RD = 287.058
G = 9.80665


def sha256(path):
    h = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def read_grib(path):
    result = {}
    pv = None
    with path.open("rb") as source:
        while (handle := eccodes.codes_grib_new_from_file(source)) is not None:
            try:
                param = eccodes.codes_get_long(handle, "paramId")
                level = eccodes.codes_get_long(handle, "level")
                key = (param, level if param in LEVEL_PARAMS else 0)
                if key in result:
                    raise ValueError(f"duplicate GRIB message in {path.name}: {key}")
                result[key] = np.asarray(eccodes.codes_get_values(handle)).reshape(41, 65)
                if eccodes.codes_get_long(handle, "NV") == 276:
                    coefficients = np.asarray(eccodes.codes_get_array(handle, "pv"))
                    if pv is None:
                        pv = coefficients
                    elif not np.array_equal(pv, coefficients):
                        raise ValueError(f"inconsistent hybrid coefficients: {path.name}")
            finally:
                eccodes.codes_release(handle)
    return result, pv


def read_gpu(path, nx=65, ny=41, nz=16):
    values = np.fromfile(path, dtype="<f4")
    expected = 8 * nx * ny * nz + 15 * nx * ny
    if values.size != expected:
        raise ValueError(f"wrong GPU meteorology length: {path.name}")
    pos = 0
    result = {}
    for name in GPU_3D:
        size = nx * ny * nz
        result[name] = values[pos:pos + size].reshape(nx, ny, nz).transpose(2, 1, 0)[:, ::-1]
        pos += size
    for name in GPU_2D:
        size = nx * ny
        result[name] = values[pos:pos + size].reshape(nx, ny).T[::-1]
        pos += size
    return result


def namelist(path):
    text = path.read_text(encoding="utf-8")
    text = re.sub(r"!.*", "", text)
    return {name.upper(): value.strip().strip('"\'')
            for name, value in re.findall(r"\b([A-Za-z_][A-Za-z_0-9]*)\s*=\s*([^,\n/]+)", text)}


def scenario_audit(manifest, config_dir, thresholds):
    command = namelist(config_dir / "COMMAND")
    release = namelist(config_dir / "RELEASES")
    outgrid = namelist(config_dir / "OUTGRID")
    outgrid_text = (config_dir / "OUTGRID").read_text(encoding="utf-8")
    species = namelist(config_dir / "SPECIES" / "SPECIES_024")
    checks = {}

    def check(name, oracle, candidate, tolerance=0):
        if isinstance(oracle, (float, int)):
            passed = math.isclose(oracle, candidate, rel_tol=0, abs_tol=tolerance)
        else:
            passed = oracle == candidate
        checks[name] = {"status": "PASS" if passed else "FAIL",
                        "oracle": oracle, "candidate": candidate}

    sim = manifest["simulation"]
    src = manifest["release"]
    output = manifest["output"]
    check("simulation_start", f"{command['IBDATE']}{int(command['IBTIME']):06d}", sim["start"])
    check("simulation_end", f"{command['IEDATE']}{int(command['IETIME']):06d}", sim["end"])
    check("release_start", f"{release['IDATE1']}{int(release['ITIME1']):06d}", src["start"])
    check("release_end", f"{release['IDATE2']}{int(release['ITIME2']):06d}", src["end"])
    for key, oracle, candidate in (
        ("release_lon", float(release["LON1"]), src["lon"]),
        ("release_lon2", float(release["LON2"]), src["lon"]),
        ("release_lat", float(release["LAT1"]), src["lat"]),
        ("release_lat2", float(release["LAT2"]), src["lat"]),
        ("release_z_min_m", float(release["Z1"]), src["z_min"]),
        ("release_z_max_m", float(release["Z2"]), src["z_max"]),
        ("release_mass_kg", float(release["MASS"]) / 1000, src["mass_kg"]),
        ("particle_count", int(release["PARTS"]), src["particle_count"]),
        ("output_interval_s", int(command["LOUTSTEP"]), output["interval_seconds"]),
        ("output_average_s", int(command["LOUTAVER"]), output["interval_seconds"]),
        ("output_sample_s", int(command["LOUTSAMPLE"]), output["sampling_seconds"]),
        ("output_nx", int(outgrid["NUMXGRID"]), output["nx"]),
        ("output_ny", int(outgrid["NUMYGRID"]), output["ny"]),
        ("output_lon0", float(outgrid["OUTLON0"]), manifest["xlon0_deg"]),
        ("output_lat0", float(outgrid["OUTLAT0"]), manifest["ylat0_deg"]),
        ("output_dx", float(outgrid["DXOUT"]), manifest["dx_deg"]),
        ("output_dy", float(outgrid["DYOUT"]), manifest["dy_deg"]),
    ):
        check(key, oracle, candidate, thresholds["scenario_abs_tolerance"])
    heights_match = re.search(r"\bOUTHEIGHTS\s*=\s*([^/]+)", outgrid_text)
    if heights_match is None:
        raise ValueError("Fortran OUTGRID lacks OUTHEIGHTS")
    oracle_heights = [float(value) for value in
                      re.findall(r"[-+]?\d+(?:\.\d+)?", heights_match.group(1))]
    check("output_heights_m", oracle_heights, output["heights_m"])
    checks["fortran_inert_species"] = {
        "status": "PASS" if int(release["SPECNUM_REL"]) == 24 else "FAIL",
        "oracle": int(release["SPECNUM_REL"]),
        "candidate": "inert tracer with no species identifier"}
    checks["fortran_forward_direction"] = {
        "status": "PASS" if int(command["LDIRECT"]) == 1 else "FAIL",
        "oracle": int(command["LDIRECT"]), "candidate": "forward"}
    checks["fortran_agl_release"] = {
        "status": "PASS" if int(release["ZKIND"]) == 1 else "FAIL",
        "oracle": int(release["ZKIND"]), "candidate": "AGL"}
    checks["convection_disabled"] = {
        "status": "PASS" if int(command["LCONVECTION"]) == 0 else "FAIL",
        "oracle": int(command["LCONVECTION"]), "candidate": "not dispatched"}
    checks["decay_disabled"] = {
        "status": "PASS" if float(species["PDECAY"]) < 0 else "FAIL",
        "oracle": species["PDECAY"], "candidate": "not dispatched"}
    checks["deposition_disabled"] = {
        "status": "PASS" if float(species["PDRYVEL"]) < 0 and
        float(species["PWETB_GAS"]) < 0 else "FAIL",
        "oracle": "species removal disabled", "candidate": "zero forcing"}
    checks["integration_semantics"] = {
        "status": "NOT_DEMONSTRATED",
        "oracle": {"CTL": command["CTL"], "IFINE": command["IFINE"],
                   "LSYNCTIME": command["LSYNCTIME"]},
        "candidate": {"fixed_dt_seconds": sim["dt_seconds"]},
        "reason": "FLEXPART adaptive/internal substeps are not proven equivalent to the GPU fixed step"}
    checks["pbl_diagnostic"] = {
        "status": "NOT_DEMONSTRATED",
        "oracle": "FLEXPART native-profile/Richardson calculation",
        "candidate": "ERA5 boundary_layer_height surface field",
        "reason": "Mixing-height diagnostics differ"}
    checks["particle_initialization"] = {
        "status": "NOT_DEMONSTRATED",
        "oracle": "FLEXPART release sampling and random sequence",
        "candidate": "GPU ETEX release sampling and seed",
        "reason": "Matching source bounds and particle count do not imply matching particle realizations"}
    return checks


def native_column_geometry(t, q, sp, pv):
    """Independent scalar-column reconstruction for sampled GPU heights."""
    half = pv[:138] + pv[138:] * sp
    p = 0.5 * (half[:-1] + half[1:])
    z = np.empty(137, dtype=np.float64)
    tv = t * (1 + 0.608 * q)
    z[-1] = RD * tv[-1] * math.log(sp / p[-1]) / G
    for level in range(135, -1, -1):
        z[level] = z[level + 1] + RD * (tv[level] + tv[level + 1]) * \
            math.log(p[level + 1] / p[level]) / (2 * G)
    return p, z


def field_error(source, candidate):
    delta = np.asarray(candidate, dtype=np.float64) - np.asarray(source, dtype=np.float64)
    return {"max_abs": float(np.max(np.abs(delta))),
            "rmse": float(np.sqrt(np.mean(delta * delta))),
            "n": int(delta.size)}


def audit(native_dir, meteo_dir, gpu_dir, config_dir, output_path, thresholds):
    manifest = json.loads((gpu_dir / "manifest.json").read_text(encoding="utf-8"))
    if tuple(x["datetime"] for x in manifest["timesteps"]) != TIMES:
        raise ValueError("candidate ERA5 time axis is incomplete")
    if manifest["nz"] != 16 or len(manifest["heights_m"]) != 16:
        raise ValueError("unexpected GPU vertical grid")
    with np.load(native_dir / "era5-surface-19941023-24.npz") as archive:
        surface = {name: archive[name] for name in SURFACE_PARAMS.values()}
    source_paths = [native_dir / name for name in (
        "era5-19941023-151821-ml.grib", "era5-19941024-000306-ml.grib",
        "era5-19941023-151821-etadot.grib", "era5-19941024-000306-etadot.grib")]
    source_hashes = {p.name: sha256(p) for p in source_paths +
                     [native_dir / "era5-surface-19941023-24.npz"]}
    provenance_match = all(manifest["meteorology"]["native_grib_sha256"].get(p.name)
                           == source_hashes[p.name] for p in source_paths) and \
        manifest["meteorology"].get("surface_archive_sha256") == \
        source_hashes["era5-surface-19941023-24.npz"]
    prepared_hashes = manifest["meteorology"]["fortran_input_sha256"]
    prepared_match = True
    source = {}
    pv = None
    for path in source_paths:
        with path.open("rb") as stream:
            while (handle := eccodes.codes_grib_new_from_file(stream)) is not None:
                try:
                    date = eccodes.codes_get_long(handle, "dataDate")
                    time = eccodes.codes_get_long(handle, "dataTime")
                    stamp = datetime.strptime(f"{date}{time:04d}", "%Y%m%d%H%M").isoformat()
                    param = eccodes.codes_get_long(handle, "paramId")
                    level = eccodes.codes_get_long(handle, "level")
                    key = (stamp, param, level)
                    if key in source:
                        raise ValueError(f"duplicate ERA5 field: {key}")
                    source[key] = np.asarray(eccodes.codes_get_values(handle)).reshape(41, 65)
                    if pv is None:
                        pv = np.asarray(eccodes.codes_get_array(handle, "pv"))
                finally:
                    eccodes.codes_release(handle)
    statistics = {name: [] for name in ("u_m_s", "v_m_s", "temperature_k",
                                        "specific_humidity", "pressure_pa", "w_m_s")}
    native_exact = True
    surface_stats = {name: [] for name in SURFACE_PARAMS.values()}
    gpu_surface_stats = {name: [] for name in (
        "surface_pressure", "10m_u_component_of_wind", "10m_v_component_of_wind",
        "2m_temperature", "2m_dewpoint_temperature", "large_scale_precipitation",
        "convective_precipitation", "surface_sensible_heat_flux",
        "surface_net_solar_radiation", "boundary_layer_height")}
    heights = np.asarray(manifest["heights_m"], dtype=np.float64)
    for time_index, entry in enumerate(manifest["timesteps"]):
        stamp = entry["datetime"]
        fortran_path = meteo_dir / f"EN{datetime.fromisoformat(stamp):%Y%m%d%H}"
        gpu_path = gpu_dir / entry["file"]
        prepared_match &= prepared_hashes.get(fortran_path.name) == sha256(fortran_path)
        prepared_match &= prepared_hashes.get(gpu_path.name) == sha256(gpu_path)
        fortran, fortran_pv = read_grib(fortran_path)
        if not np.array_equal(pv, fortran_pv):
            raise ValueError("Fortran hybrid coefficients differ from ERA5")
        # The oracle receives eta velocity (77); omega (135) is candidate-only.
        required = {(param, level) for param in LEVEL_PARAMS if param != 135
                    for level in range(1, 138)}
        if not required.issubset(fortran):
            raise ValueError(f"Fortran GRIB lacks native fields: {fortran_path.name}")
        for param, level in required:
            if not np.array_equal(source[(stamp, param, level)], fortran[(param, level)]):
                native_exact = False
        gpu = read_gpu(gpu_path)
        for param, name in SURFACE_PARAMS.items():
            expected = surface[name][time_index]
            if name in ("surface_sensible_heat_flux", "surface_net_solar_radiation"):
                expected = expected / 3600
            elif name in ("convective_precipitation", "large_scale_precipitation"):
                expected = expected * 1000
            surface_stats[name].append(field_error(expected, fortran[(param, 0)]))
            if name in gpu_surface_stats:
                gpu_expected = expected
                if name == "boundary_layer_height":
                    gpu_expected = np.maximum(expected, 50)
                gpu_surface_stats[name].append(field_error(gpu_expected, gpu[name]))
        for x, y in SAMPLE_COLUMNS:
            sp = float(surface["surface_pressure"][time_index, y, x])
            t = np.array([source[(stamp, 130, level)][y, x]
                          for level in range(1, 138)])
            q = np.array([source[(stamp, 133, level)][y, x]
                          for level in range(1, 138)])
            pressure, z = native_column_geometry(t, q, sp, pv)
            for param, name in ((131, "u_m_s"), (132, "v_m_s"),
                                (130, "temperature_k"), (133, "specific_humidity")):
                profile = np.array([source[(stamp, param, level)][y, x]
                                    for level in range(1, 138)])
                expected = np.interp(heights, z[::-1], profile[::-1])
                statistics[name].append(field_error(expected, gpu[name][:, y, x]))
            expected_p = np.interp(heights, z[::-1], pressure[::-1])
            statistics["pressure_pa"].append(field_error(expected_p, gpu["pressure_pa"][:, y, x]))
            omega = np.array([source[(stamp, 135, level)][y, x]
                              for level in range(1, 138)])
            expected_w = -np.interp(heights, z[::-1], omega[::-1]) / \
                (expected_p / (RD * np.interp(heights, z[::-1], t[::-1]) *
                               (1 + 0.608 * np.interp(heights, z[::-1], q[::-1]))) * G)
            statistics["w_m_s"].append(field_error(expected_w, gpu["w_m_s"][:, y, x]))
    field_results = {name: {"max_abs": max(x["max_abs"] for x in errors),
                            "rmse_of_samples": math.sqrt(sum(x["rmse"] ** 2 for x in errors)
                                                         / len(errors)),
                            "sample_count": sum(x["n"] for x in errors)}
                     for name, errors in statistics.items()}
    surface_results = {name: max(x["max_abs"] for x in errors)
                       for name, errors in surface_stats.items()}
    gpu_surface_results = {name: max(x["max_abs"] for x in errors)
                           for name, errors in gpu_surface_stats.items()}
    mapping_pass = all(field_results[name]["max_abs"] <= limit
                       for name, limit in thresholds["gpu_mapping_abs_tolerance"].items())
    surface_pass = all(surface_results[name] <= limit
                       for name, limit in thresholds["fortran_surface_abs_tolerance"].items())
    gpu_surface_pass = all(gpu_surface_results[name] <= limit
                           for name, limit in thresholds["gpu_surface_abs_tolerance"].items())
    scenario = scenario_audit(manifest, config_dir, thresholds)
    hard_fail = not provenance_match or not prepared_match or \
        not native_exact or not mapping_pass or \
        not surface_pass or not gpu_surface_pass or \
        any(check["status"] == "FAIL" for check in scenario.values())
    result = {
        "schema_version": 1,
        "status": "FAIL" if hard_fail else "INPUT_EQUIVALENCE_NOT_DEMONSTRATED",
        "source_sha256": source_hashes,
        "manifest_native_grib_hashes_match": provenance_match,
        "manifest_prepared_input_hashes_match": prepared_match,
        "fortran_native_fields_exact": native_exact,
        "gpu_mapping_within_technical_tolerance": mapping_pass,
        "gpu_mapping_samples": field_results,
        "fortran_surface_within_technical_tolerance": surface_pass,
        "fortran_surface_max_abs": surface_results,
        "gpu_surface_within_technical_tolerance": gpu_surface_pass,
        "gpu_surface_max_abs": gpu_surface_results,
        "scenario_checks": scenario,
        "unresolved": [
            "FLEXPART uses 137 native hybrid levels; GPU uses 16 fixed AGL levels",
            "FLEXPART uses ERA5 eta-coordinate velocity and native slope correction; GPU uses omega-to-m/s conversion",
            "FLEXPART internal/adaptive timesteps and GPU fixed 900 s step are not demonstrated equivalent",
            "FLEXPART and GPU mixing-height diagnostics differ",
            "Particle positions and stochastic seeds are not matched across models",
        ],
        "technical_thresholds": thresholds,
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(f"ETEX input audit: {result['status']}")
    for name, value in field_results.items():
        print(f"  GPU {name}: max_abs={value['max_abs']:.6g}")
    if hard_fail:
        raise ValueError("ETEX input mapping or scenario check failed; see audit report")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-dir", type=Path, required=True)
    parser.add_argument("--meteo-dir", type=Path, required=True)
    parser.add_argument("--gpu-dir", type=Path, required=True)
    parser.add_argument("--config-dir", type=Path, required=True)
    parser.add_argument("--thresholds", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    thresholds = json.loads(args.thresholds.read_text(encoding="utf-8"))
    audit(args.native_dir, args.meteo_dir, args.gpu_dir, args.config_dir,
          args.output, thresholds)


if __name__ == "__main__":
    main()
