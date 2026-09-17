#!/usr/bin/env python3
"""Compare paired ETEX observations with FLEXPART 11.1 and flexpart-gpu.

Both models must provide complete, matching three-hour concentration windows.
Each station record is matched by its actual sampling interval. The report
contains diagnostics, not a scientific parity verdict.
"""

import argparse
import hashlib
import json
import math
from collections import defaultdict
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from statistics import median

import numpy as np

from compare_etex_fortran_obs import parse_header_txt, read_grid_conc
from compare_gpu_obs import gpu_mass_to_concentration, interpolate_to_station

DETECTION_THRESHOLD_PG_M3 = 10.0


@dataclass
class Window:
    start: datetime
    end: datetime
    surface_pg_m3: np.ndarray


def utc_epoch(seconds):
    return datetime.fromtimestamp(seconds, timezone.utc).replace(tzinfo=None)


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def check_field(field, label):
    if not np.all(np.isfinite(field)) or np.any(field < 0):
        raise ValueError(f"{label} has non-finite or negative concentrations")


def gpu_windows(path):
    with open(path, encoding="utf-8") as source:
        output = json.load(source)
    grid = output["grid"]
    averaging = output["averaging_seconds"]
    sampling = output["sampling_seconds"]
    if averaging <= 0 or sampling <= 0 or averaging % sampling:
        raise ValueError("invalid GPU averaging and sampling intervals")
    windows = []
    for timestep in output["timesteps"]:
        start = timestep["window_start_epoch_seconds"]
        end = timestep["epoch_seconds"]
        if end - start != averaging or timestep["samples"] != averaging // sampling + 1:
            raise ValueError("GPU concentration window is incomplete")
        if len(timestep["concentration_mass_kg"]) != grid["nx"] * grid["ny"] * grid["nz"]:
            raise ValueError("GPU concentration field has the wrong shape")
        surface = gpu_mass_to_concentration(
            timestep["concentration_mass_kg"], grid["nx"], grid["ny"],
            grid["nz"], grid["dx"], grid["dy"], grid["xlon0"],
            grid["ylat0"], grid["heights_m"])
        check_field(surface, "GPU surface field")
        windows.append(Window(utc_epoch(start), utc_epoch(end), surface))
    if not windows:
        raise ValueError("GPU output has no concentration windows")
    return grid, windows


def fortran_windows(directory):
    directory = Path(directory)
    grid = parse_header_txt(str(directory / "header_txt"))
    required = ("outlon0", "outlat0", "nx", "ny", "nz", "dx", "dy",
                "outheights", "interval_s", "averaging_s", "sampling_s")
    if any(key not in grid for key in required):
        raise ValueError("FLEXPART header_txt lacks grid or sampling metadata")
    if grid["interval_s"] <= 0 or grid["interval_s"] != grid["averaging_s"]:
        raise ValueError("FLEXPART output is not a complete averaging window")
    with open(directory / "dates", encoding="utf-8") as source:
        dates = [line.strip() for line in source if line.strip()]
    if not dates:
        raise ValueError("FLEXPART dates file is empty")
    windows, files = [], []
    for stamp in dates:
        end = datetime.strptime(stamp, "%Y%m%d%H%M%S")
        start = utc_epoch(int(end.replace(tzinfo=timezone.utc).timestamp())
                          - grid["averaging_s"])
        field_path = directory / f"grid_conc_{stamp}_001"
        if not field_path.is_file():
            raise FileNotFoundError(field_path)
        if field_path.stat().st_size < 60:
            raise ValueError(f"FLEXPART concentration file is incomplete: {field_path}")
        field = read_grid_conc(str(field_path), grid["nx"], grid["ny"], grid["nz"])
        surface = np.asarray(field[:, :, 0], dtype=np.float64)
        check_field(surface, "FLEXPART surface field")
        windows.append(Window(start, end, surface))
        files.append(field_path)
    return grid, windows, files


def matching_grid(gpu, fortran):
    names = (("nx", "nx"), ("ny", "ny"), ("nz", "nz"),
             ("xlon0", "outlon0"), ("ylat0", "outlat0"),
             ("dx", "dx"), ("dy", "dy"))
    for gpu_name, fortran_name in names:
        if not math.isclose(gpu[gpu_name], fortran[fortran_name],
                            rel_tol=0, abs_tol=1e-5):
            raise ValueError(f"output-grid mismatch: {gpu_name}")
    if not math.isclose(gpu["heights_m"][0], fortran["outheights"][0],
                        rel_tol=0, abs_tol=1e-5):
        raise ValueError("surface-layer heights differ")


def station_mean(windows, grid, lon, lat, start, end):
    duration = (end - start).total_seconds()
    if duration <= 0:
        raise ValueError("invalid observation sampling interval")
    weighted, covered = 0.0, 0.0
    for window in windows:
        overlap = (min(end, window.end) - max(start, window.start)).total_seconds()
        if overlap <= 0:
            continue
        value = interpolate_to_station(
            window.surface_pg_m3,
            grid["xlon0"] if "xlon0" in grid else grid["outlon0"],
            grid["ylat0"] if "ylat0" in grid else grid["outlat0"],
            grid["dx"], grid["dy"], grid["nx"], grid["ny"], lon, lat)
        if value is None:
            return None
        weighted += overlap * value
        covered += overlap
    if not math.isclose(covered, duration, rel_tol=0, abs_tol=1):
        return None
    return weighted / duration


def metrics(observed, modeled):
    observed = np.asarray(observed, dtype=np.float64)
    modeled = np.asarray(modeled, dtype=np.float64)
    if len(observed) == 0 or len(observed) != len(modeled):
        raise ValueError("metrics need nonempty paired values")
    difference = modeled - observed
    mean_observed = float(observed.mean())
    mean_modeled = float(modeled.mean())
    fac2_mask = (observed > 0) & (modeled > 0)
    ratios = modeled[fac2_mask] / observed[fac2_mask]
    correlation = (float(np.corrcoef(observed, modeled)[0, 1])
                   if np.std(observed) > 0 and np.std(modeled) > 0 else None)
    return {
        "n": len(observed),
        "bias_pg_m3": float(difference.mean()),
        "rmse_pg_m3": float(np.sqrt(np.mean(difference ** 2))),
        "correlation": correlation,
        "fractional_bias": (2 * float(difference.mean()) / (mean_observed + mean_modeled)
                            if mean_observed + mean_modeled > 0 else None),
        "nmse": (float(np.mean(difference ** 2)) / (mean_observed * mean_modeled)
                 if mean_observed > 0 and mean_modeled > 0 else None),
        "fac2": (float(np.mean((ratios >= 0.5) & (ratios <= 2)))
                 if len(ratios) else None),
        "fac2_pairs": int(len(ratios)),
    }


def timing(pairs, model_key):
    stations = defaultdict(list)
    for pair in pairs:
        stations[pair["station"]].append(pair)
    arrival_lags, peak_lags, peak_ratios = [], [], []
    for records in stations.values():
        records.sort(key=lambda record: record["start_time"])
        obs_arrival = next((r for r in records
                            if r["observed_pg_m3"] >= DETECTION_THRESHOLD_PG_M3), None)
        mod_arrival = next((r for r in records
                            if r[model_key] >= DETECTION_THRESHOLD_PG_M3), None)
        if obs_arrival and mod_arrival:
            arrival_lags.append((datetime.fromisoformat(mod_arrival["start_time"])
                                 - datetime.fromisoformat(obs_arrival["start_time"])
                                 ).total_seconds() / 3600)
        obs_peak = max(records, key=lambda r: r["observed_pg_m3"])
        mod_peak = max(records, key=lambda r: r[model_key])
        if obs_peak["observed_pg_m3"] > 0:
            peak_lags.append((datetime.fromisoformat(mod_peak["start_time"])
                              - datetime.fromisoformat(obs_peak["start_time"])
                              ).total_seconds() / 3600)
            peak_ratios.append(mod_peak[model_key] / obs_peak["observed_pg_m3"])
    return {
        "detection_threshold_pg_m3": DETECTION_THRESHOLD_PG_M3,
        "arrival_stations": len(arrival_lags),
        "median_arrival_error_hours": median(arrival_lags) if arrival_lags else None,
        "peak_stations": len(peak_lags),
        "median_peak_time_error_hours": median(peak_lags) if peak_lags else None,
        "median_peak_magnitude_ratio": median(peak_ratios) if peak_ratios else None,
    }


def compare(measurements_path, gpu_path, fortran_dir, era5_dir=None,
            gpu_manifest=None, oracle_manifest=None, candidate_revision=None,
            candidate_dirty=None, gpu_log=None, fortran_log=None):
    with open(measurements_path, encoding="utf-8") as source:
        observations = json.load(source)["measurements"]
    gpu_grid, gpu_output = gpu_windows(gpu_path)
    fortran_grid, fortran_output, fortran_files = fortran_windows(fortran_dir)
    matching_grid(gpu_grid, fortran_grid)
    if len(gpu_output) != len(fortran_output):
        raise ValueError("GPU and FLEXPART output-window counts differ")
    for gpu, fortran in zip(gpu_output, fortran_output):
        if (gpu.start, gpu.end) != (fortran.start, fortran.end):
            raise ValueError("GPU and FLEXPART output windows differ")

    pairs = []
    for observation in observations:
        start = datetime.fromisoformat(observation["start_time"])
        end = datetime.fromisoformat(observation["end_time"])
        lon, lat = observation["lon"], observation["lat"]
        gpu_value = station_mean(gpu_output, gpu_grid, lon, lat, start, end)
        fortran_value = station_mean(fortran_output, fortran_grid, lon, lat, start, end)
        if gpu_value is None or fortran_value is None:
            continue
        pairs.append({
            "station": observation["station"],
            "start_time": observation["start_time"],
            "end_time": observation["end_time"],
            "observed_pg_m3": observation["concentration_pg_m3"],
            "gpu_pg_m3": gpu_value,
            "fortran_pg_m3": fortran_value,
        })
    if not pairs:
        raise ValueError("no observation has complete paired model coverage")
    observed = [pair["observed_pg_m3"] for pair in pairs]
    gpu = [pair["gpu_pg_m3"] for pair in pairs]
    fortran = [pair["fortran_pg_m3"] for pair in pairs]
    report = {
        "experiment": "ETEX-1",
        "status": "METRICS_COMPUTED_NO_PARITY_VERDICT",
        "oracle": "FLEXPART v11.1",
        "units": "pg/m3",
        "paired_observation_count": len(pairs),
        "output_window_count": len(gpu_output),
        "gpu_vs_observations": metrics(observed, gpu),
        "fortran_vs_observations": metrics(observed, fortran),
        "gpu_vs_fortran_at_stations": metrics(fortran, gpu),
        "gpu_timing": timing(pairs, "gpu_pg_m3"),
        "fortran_timing": timing(pairs, "fortran_pg_m3"),
        "input_sha256": {
            "measurements": sha256(measurements_path),
            "gpu_output": sha256(gpu_path),
            "fortran_header": sha256(Path(fortran_dir) / "header_txt"),
            "fortran_dates": sha256(Path(fortran_dir) / "dates"),
            "fortran_concentration_files": {p.name: sha256(p) for p in fortran_files},
        },
        "pairs": pairs,
    }
    if era5_dir is not None:
        arrays = sorted(Path(era5_dir).glob("*.npy"))
        if arrays:
            if not (Path(era5_dir) / "metadata.json").is_file():
                raise ValueError("ERA5 source metadata are missing")
            report["input_sha256"]["era5_metadata"] = sha256(Path(era5_dir) / "metadata.json")
            report["input_sha256"]["era5_arrays"] = {p.name: sha256(p) for p in arrays}
        else:
            native = sorted(Path(era5_dir).glob("*.grib"))
            surface = sorted(Path(era5_dir).glob("*.npz"))
            if len(native) != 4 or len(surface) != 1:
                raise ValueError("native ERA5 model-level or surface files are missing")
            report["input_sha256"]["era5_native"] = {
                p.name: sha256(p) for p in native + surface}
    if gpu_manifest is not None:
        report["input_sha256"]["gpu_meteorology_manifest"] = sha256(gpu_manifest)
    if gpu_log is not None:
        report["input_sha256"]["gpu_run_log"] = sha256(gpu_log)
        with open(gpu_log, encoding="utf-8", errors="replace") as source:
            adapter_lines = [line.strip() for line in source
                             if "wgpu adapter:" in line or
                             "wgpu adapter (software fallback requested):" in line]
        if len(adapter_lines) != 1:
            raise ValueError("GPU run log lacks one unambiguous wgpu adapter record")
        report["gpu_adapter"] = adapter_lines[0]
    if fortran_log is not None:
        report["input_sha256"]["fortran_run_log"] = sha256(fortran_log)
    if oracle_manifest is not None:
        with open(oracle_manifest, encoding="utf-8") as source:
            reference = json.load(source)
        report["oracle_commit"] = reference["pinned_commit"]
        report["input_sha256"]["oracle_manifest"] = sha256(oracle_manifest)
    if candidate_revision is not None:
        report["candidate_revision"] = candidate_revision
        report["candidate_worktree_dirty"] = candidate_dirty
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--measurements", required=True)
    parser.add_argument("--gpu-output", required=True)
    parser.add_argument("--fortran-output", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--era5-dir", required=True)
    parser.add_argument("--gpu-manifest", required=True)
    parser.add_argument("--gpu-log", required=True)
    parser.add_argument("--fortran-log", required=True)
    parser.add_argument("--oracle-manifest", required=True)
    parser.add_argument("--candidate-revision", required=True)
    parser.add_argument("--candidate-dirty", action="store_true")
    args = parser.parse_args()
    report = compare(args.measurements, args.gpu_output, args.fortran_output,
                     args.era5_dir, args.gpu_manifest, args.oracle_manifest,
                     args.candidate_revision, args.candidate_dirty,
                     args.gpu_log, args.fortran_log)
    path = Path(args.output)
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8") as destination:
        json.dump(report, destination, indent=2, allow_nan=False)
    print(f"Compared {report['paired_observation_count']} paired ETEX observations")
    print(f"FLEXPART 11.1: {report['fortran_vs_observations']}")
    print(f"flexpart-gpu: {report['gpu_vs_observations']}")
    print(f"Report: {path}")


if __name__ == "__main__":
    main()
