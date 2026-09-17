#!/usr/bin/env python3
"""Evaluate one case into a versioned JSON report plus a readable summary.

Documented commands (run from the repository root):

Synthetic uniform-wind oracle comparison::

    python scripts/evaluate/evaluate_case.py --case synthetic-uniform-wind \
        --fortran-output target/comparison/validate_run/output \
        --gpu-output target/validation/gpu_concentration.json \
        --output results/evaluation/synthetic-uniform-wind/report.json \
        --summary results/evaluation/synthetic-uniform-wind/summary.txt

ETEX paired observation comparison (mini stays diagnostic)::

    python scripts/evaluate/evaluate_case.py --case etex-mini \
        --measurements target/etex/mini/measurements.json \
        --fortran-output target/etex/mini/fortran_run/output \
        --gpu-output target/etex/mini/gpu_output.json \
        --output results/evaluation/etex-mini/report.json \
        --summary results/evaluation/etex-mini/summary.txt

Multi-seed aggregation of per-seed evaluation reports::

    python scripts/evaluate/evaluate_case.py --case aggregate-seeds \
        --seed-reports results/evaluation/<case>/seed_*.json \
        --output results/evaluation/<case>/aggregated.json \
        --summary results/evaluation/<case>/aggregated-summary.txt

Corpus particle ensembles (Issue #6 test corpus, per-seed particle files)::

    python scripts/evaluate/evaluate_case.py --case corpus-seeds \
        --case-def fixtures/corpus/cases/WIND-UNI-002.json \
        --candidate-seeds "target/corpus/candidate/WIND-UNI-002/seed_*.json" \
        --fortran-output target/corpus/oracle/WIND-UNI-002 \
        --output results/evaluation/WIND-UNI-002/report.json \
        --summary results/evaluation/WIND-UNI-002/summary.txt

The command fails (non-zero exit) on incompatible inputs or missing oracle
artifacts instead of interpolating or substituting values. A written report
with ``overall_status`` ``INTEGRITY_ERROR`` documents the failure.
"""

import argparse
import json
import math
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import io_corpus
import io_fortran
import io_gpu
import metrics
import report as report_lib

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
DEFAULT_ORACLE_MANIFEST = REPO_ROOT / "reference" / "flexpart-11.1.json"
DEFAULT_THRESHOLDS = REPO_ROOT / "evaluation" / "thresholds" / "scientific-thresholds-v1.json"


def epoch_to_iso(seconds):
    return datetime.fromtimestamp(int(seconds), tz=timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def load_oracle_manifest(path):
    with open(path, encoding="utf-8") as stream:
        manifest = json.load(stream)
    for key in ("pinned_commit",):
        if key not in manifest:
            raise ValueError(f"oracle manifest lacks {key}: {path}")
    manifest.setdefault("name", "FLEXPART")
    manifest.setdefault("version", "11.1")
    return manifest


def build_provenance(args, oracle_manifest):
    candidate_checkout = Path(args.candidate_checkout) if args.candidate_checkout else REPO_ROOT
    candidate_revision, candidate_dirty = report_lib.git_revision(candidate_checkout)
    if args.candidate_revision:
        candidate_revision = args.candidate_revision
    adapter = None
    if args.candidate_log:
        adapter = report_lib.read_adapter_from_log(args.candidate_log)
    oracle_checkout = Path(args.oracle_checkout) if args.oracle_checkout else None
    oracle_commit_verified = None
    if oracle_checkout is not None:
        actual, dirty = report_lib.git_revision(oracle_checkout)
        if actual != oracle_manifest["pinned_commit"] or dirty:
            raise ValueError(
                "oracle checkout is not the pinned unmodified FLEXPART source "
                f"(expected {oracle_manifest['pinned_commit']}, got {actual}, dirty={dirty})")
        oracle_commit_verified = actual
    oracle = {
        "name": oracle_manifest.get("name", "FLEXPART"),
        "version": oracle_manifest.get("version", "11.1"),
        "pinned_commit": oracle_manifest["pinned_commit"],
        "checkout_verified": bool(oracle_checkout is not None),
        "seed": args.oracle_seed,
        "seed_controllable": bool(args.oracle_seed_controllable),
    }
    if not oracle["seed_controllable"]:
        oracle["seed_note"] = ("oracle seed is not exposed by this runner; "
                               "not suitable for multi-seed parity proof")
    candidate = {
        "revision": candidate_revision,
        "worktree_dirty": candidate_dirty,
        "adapter": adapter,
        "seed": args.seed,
        "seed_controllable": bool(args.candidate_seed_controllable),
    }
    if not candidate["seed_controllable"]:
        candidate["seed_note"] = ("candidate seed is not controlled by this runner; "
                                  "single-seed results stay diagnostic")
    if args.candidate_executable:
        candidate["executable_sha256"] = report_lib.sha256_file(args.candidate_executable)
    if args.oracle_executable:
        oracle["executable_sha256"] = report_lib.sha256_file(args.oracle_executable)
    return oracle, candidate, oracle_commit_verified


def _integrity_report(case_id, args, oracle, candidate, input_hashes, missing, detail, notes):
    thresholds_doc = json.loads(Path(args.thresholds).read_text(encoding="utf-8"))
    evaluations = report_lib.evaluate_thresholds(
        thresholds_doc, {"case_id": case_id})
    built = report_lib.make_report(
        case_id=case_id, time_window=None, grid=None, oracle=oracle,
        candidate=candidate, input_hashes=input_hashes,
        alignment={"grid_and_window_match": False, "checks": []},
        particle_metrics=None, grid_metrics=None, etex=None, multiseed=None,
        threshold_evaluations=evaluations,
        thresholds_version=thresholds_doc.get("version", "unknown"),
        missing_metrics=missing, notes=notes,
        overall_status="INTEGRITY_ERROR", status_detail=detail)
    return built


def write_outputs(built, args):
    report_lib.write_report_json(built, args.output)
    summary = report_lib.render_summary(built)
    if args.summary:
        summary_path = Path(args.summary)
        summary_path.parent.mkdir(parents=True, exist_ok=True)
        summary_path.write_text(summary, encoding="utf-8")
    print(summary)
    print(f"Report: {args.output}")


# ---------------------------------------------------------------------------
# Synthetic uniform-wind case
# ---------------------------------------------------------------------------

def run_synthetic(args, oracle_manifest, oracle, candidate):
    missing = []
    notes = list(args.note or [])
    notes.append("Normalized concentration shape comparison is diagnostic only; "
                 "it is not mass-conservation evidence.")
    input_hashes = {}
    fortran_dir = Path(args.fortran_output)
    header_path = fortran_dir / "header"
    if not header_path.is_file():
        raise FileNotFoundError(f"missing oracle artifact: {header_path}")
    input_hashes["fortran_header"] = report_lib.sha256_file(header_path)
    conc_files = io_fortran.find_grid_conc_files(fortran_dir)
    last_conc = conc_files[-1]
    header, endian = io_fortran.read_header_with_endian(str(header_path))
    oracle_grid = {
        "outlon0": header["outlon0"], "outlat0": header["outlat0"],
        "nx": header["numxgrid"], "ny": header["numygrid"], "nz": header["numzgrid"],
        "dx": header["dxout"], "dy": header["dyout"],
        "outheights": header["outheights"],
    }
    oracle_nested = io_fortran.read_grid_conc(str(last_conc), header, endian)
    oracle_flat = io_fortran.flatten_grid_nx_ny_nz(oracle_nested)
    for value in oracle_flat:
        if not math.isfinite(value) or value < 0:
            raise ValueError("oracle concentration field has non-finite or negative values")
    input_hashes["fortran_concentration"] = report_lib.sha256_file(last_conc)

    gpu = io_gpu.read_fortran_validation_json(args.gpu_output)
    input_hashes["candidate_output"] = report_lib.sha256_file(args.gpu_output)
    grid = gpu["grid"]
    candidate_grid = {
        "nx": grid["nx"], "ny": grid["ny"], "nz": grid["nz"],
        "xlon0": grid["xlon0"], "ylat0": grid["ylat0"],
        "dx": grid["dx"], "dy": grid["dy"], "heights_m": grid["heights_m"],
    }
    candidate_flat = [float(v) for v in gpu["concentration_mass_kg"]]
    candidate_counts = [int(v) for v in gpu["particle_count_per_cell"]]
    for value in candidate_flat:
        if not math.isfinite(value) or value < 0:
            raise ValueError("candidate mass field has non-finite or negative values")

    # Window alignment: the oracle averaging window ends at the last file.
    stamp = last_conc.name.split("_")[2]
    fortran_end = int(datetime.strptime(stamp, "%Y%m%d%H%M%S")
                      .replace(tzinfo=timezone.utc).timestamp())
    oracle_window = {
        "window_start_epoch_seconds": fortran_end - header["loutaver"],
        "window_end_epoch_seconds": fortran_end,
        "averaging_seconds": header["loutaver"],
        "sampling_seconds": header["loutsample"],
        "samples": header["loutaver"] // header["loutsample"] + 1,
        "endpoint_weight": 0.5,
    }
    candidate_window = {
        "window_start_epoch_seconds": gpu["window_start_epoch_seconds"],
        "window_end_epoch_seconds": gpu["window_end_epoch_seconds"],
        "averaging_seconds": gpu["averaging_seconds"],
        "sampling_seconds": gpu["sampling_seconds"],
        "samples": gpu["samples"],
        "endpoint_weight": float(gpu["endpoint_weight"]),
    }
    grid_match, grid_checks = report_lib.check_grid_alignment(oracle_grid, candidate_grid)
    window_match, window_checks = report_lib.check_window_alignment(oracle_window, candidate_window)
    alignment = {"grid_and_window_match": bool(grid_match and window_match),
                 "grid_checks": grid_checks, "window_checks": window_checks}
    if not (grid_match and window_match):
        raise ValueError(f"grid/window alignment failed: grid_match={grid_match} "
                         f"window_match={window_match}")

    nx, ny, nz = grid["nx"], grid["ny"], grid["nz"]
    # Candidate mass budget (deposition and decay disabled in this case).
    released_mass_kg = 1.0
    remaining_mass_kg = float(sum(candidate_flat))
    budget = metrics.mass_budget(released_mass_kg, remaining_mass_kg)
    notes.append(f"Candidate mass budget uses released mass {released_mass_kg} kg; "
                 "deposition and decay are disabled in this case.")

    # Grid-space diagnostics compare concentration against concentration so
    # grid geometry does not weight the two fields differently. The candidate
    # mass field (kg per cell) is converted to kg/m3 with latitude-dependent
    # cell volumes and OUTHEIGHTS layer thicknesses; the oracle field is
    # already a concentration in native units. Both are then normalized to
    # sum 1 for the shape diagnostic, which is explicitly not conservation.
    candidate_conc = io_gpu.mass_to_concentration_kg_m3(
        candidate_flat, nx, ny, nz, grid["xlon0"], grid["ylat0"],
        grid["dx"], grid["dy"], grid["heights_m"])
    oracle_sum = sum(oracle_flat)
    candidate_conc_sum = sum(candidate_conc)
    if oracle_sum > 0 and candidate_conc_sum > 0:
        oracle_norm = [v / oracle_sum for v in oracle_flat]
        candidate_norm = [v / candidate_conc_sum for v in candidate_conc]
        shape = metrics.field_comparison_metrics(oracle_norm, candidate_norm,
                                                 "oracle(avg) vs candidate(avg) normalized shape")
        footprint = metrics.footprint_overlap(oracle_norm, candidate_norm, threshold=0.0)
    else:
        shape = None
        footprint = None
        missing.append({"name": "grid_metrics.normalized_shape",
                        "reason": "one field is all zeros; shape comparison undefined"})
    oracle_com = metrics.center_of_mass_grid(
        oracle_flat, nx, ny, nz, header["outlon0"], header["outlat0"],
        header["dxout"], header["dyout"], header["outheights"])
    candidate_com = metrics.center_of_mass_grid(
        candidate_conc, nx, ny, nz, grid["xlon0"], grid["ylat0"],
        grid["dx"], grid["dy"], grid["heights_m"])
    center_distance_km = None
    if oracle_com and candidate_com:
        dlon = candidate_com["lon_deg"] - oracle_com["lon_deg"]
        dlat = candidate_com["lat_deg"] - oracle_com["lat_deg"]
        center_distance_km = math.hypot(
            dlon * metrics.DEG_TO_KM * math.cos(math.radians(oracle_com["lat_deg"])),
            dlat * metrics.DEG_TO_KM)
    oracle_cov = metrics.horizontal_covariance_grid(
        oracle_flat, nx, ny, nz, header["outlon0"], header["outlat0"],
        header["dxout"], header["dyout"])
    candidate_cov = metrics.horizontal_covariance_grid(
        candidate_conc, nx, ny, nz, grid["xlon0"], grid["ylat0"], grid["dx"], grid["dy"])
    eigenvalue_ratios = None
    if oracle_cov and candidate_cov:
        eigenvalue_ratios = [
            (c / o if o > 0 else None)
            for c, o in zip(candidate_cov["eigenvalues_km2"], oracle_cov["eigenvalues_km2"])]
    oracle_levels = [sum(oracle_nested[ix][iy][iz] for ix in range(nx) for iy in range(ny))
                     for iz in range(nz)]
    candidate_counts_3d = candidate_counts
    candidate_level_counts = []
    for iz in range(nz):
        candidate_level_counts.append(sum(
            candidate_counts_3d[((ix * ny) + iy) * nz + iz]
            for ix in range(nx) for iy in range(ny)))
    candidate_conc_levels = []
    for iz in range(nz):
        candidate_conc_levels.append(sum(
            candidate_conc[((ix * ny) + iy) * nz + iz]
            for ix in range(nx) for iy in range(ny)))
    oracle_total = sum(oracle_levels)
    candidate_conc_total = sum(candidate_conc_levels)
    vertical_profile = {
        "oracle_level_sums": [float(v) for v in oracle_levels],
        "oracle_level_fractions": [float(v / oracle_total) if oracle_total > 0 else 0.0
                                   for v in oracle_levels],
        "candidate_level_counts": [int(v) for v in candidate_level_counts],
        "candidate_concentration_level_sums_kg_m3": [float(v) for v in candidate_conc_levels],
        "candidate_concentration_level_fractions": [
            float(v / candidate_conc_total) if candidate_conc_total > 0 else 0.0
            for v in candidate_conc_levels],
        "heights_m": [float(v) for v in grid["heights_m"]],
        "unit": ("oracle native concentration units; candidate end-state particle counts "
                 "and candidate concentration (kg/m3) level sums with fractions"),
    }

    # Particle-space metrics: available only when the oracle wrote partposit.
    partposit_files = io_fortran.find_partposit_files(fortran_dir)
    particle_section = {"mass_budget": budget, "particle_comparison": None,
                        "vertical_quantiles": None, "unit": "kg/m"}
    if partposit_files:
        f_lons, f_lats, f_zs = io_fortran.read_partposit(str(partposit_files[-1]), endian)
        if f_lons:
            masses = [released_mass_kg / len(f_lons)] * len(f_lons)
            oracle_particle_com = metrics.center_of_mass_particles(f_lons, f_lats, f_zs, masses)
            oracle_particle_cov = metrics.horizontal_covariance(f_lons, f_lats, masses)
            oracle_quantiles = metrics.vertical_quantiles(f_zs, masses)
            particle_section["oracle_partposit"] = {
                "file": partposit_files[-1].name,
                "particle_count": len(f_lons),
                "center_of_mass": oracle_particle_com,
                "horizontal_covariance": oracle_particle_cov,
                "vertical_quantiles_m": oracle_quantiles,
            }
            z_stats = gpu.get("particle_z_stats")
            if z_stats:
                particle_section["candidate_end_state"] = {
                    "particle_count": z_stats.get("count"),
                    "lon_mean_deg": z_stats.get("lon_mean"),
                    "lat_mean_deg": z_stats.get("lat_mean"),
                    "z_mean_m": z_stats.get("mean_m"),
                    "z_std_m": z_stats.get("std_m"),
                }
            else:
                missing.append({"name": "particle_metrics.candidate_end_state",
                                "reason": "candidate output lacks particle_z_stats"})
        else:
            missing.append({"name": "particle_metrics.oracle_partposit",
                            "reason": "oracle partposit file is empty"})
    else:
        missing.append({"name": "particle_metrics.oracle_partposit",
                        "reason": "oracle wrote no partposit dump in this run; "
                                  "particle-space moments unavailable"})
        z_stats = gpu.get("particle_z_stats")
        if z_stats:
            particle_section["candidate_end_state"] = {
                "particle_count": z_stats.get("count"),
                "lon_mean_deg": z_stats.get("lon_mean"),
                "lat_mean_deg": z_stats.get("lat_mean"),
                "z_mean_m": z_stats.get("mean_m"),
                "z_std_m": z_stats.get("std_m"),
            }
    if candidate.get("adapter") is None:
        missing.append({"name": "candidate.adapter",
                        "reason": "no candidate log provided; adapter unknown"})
    if candidate.get("seed") is None:
        missing.append({"name": "candidate.seed",
                        "reason": "candidate seed not exposed; single-seed result stays diagnostic"})

    grid_section = {
        "field_comparison_normalized_shape": shape,
        "shape_note": ("Concentration-against-concentration shape diagnostic only; "
                       "not mass-conservation evidence. "
                       f"Raw sums: oracle={oracle_sum:.6e} (native concentration units), "
                       f"candidate={remaining_mass_kg:.6e} kg mass "
                       f"({candidate_conc_sum:.6e} kg/m3 concentration sum)."),
        "concentration_unit": "normalized dimensionless (oracle native units and candidate kg/m3)",
        "footprint_overlap": footprint,
        "oracle_center_of_mass": oracle_com,
        "candidate_center_of_mass": candidate_com,
        "center_distance_km": center_distance_km,
        "oracle_horizontal_covariance": oracle_cov,
        "candidate_horizontal_covariance": candidate_cov,
        "covariance_eigenvalue_ratios_candidate_over_oracle": eigenvalue_ratios,
        "vertical_profile": vertical_profile,
        "raw_sums": {"oracle_native_concentration_units": float(oracle_sum),
                     "candidate_mass_kg": float(remaining_mass_kg),
                     "candidate_concentration_kg_m3_sum": float(candidate_conc_sum)},
    }
    time_window = {
        "start_iso": epoch_to_iso(oracle_window["window_start_epoch_seconds"]),
        "end_iso": epoch_to_iso(oracle_window["window_end_epoch_seconds"]),
        "start_epoch_s": oracle_window["window_start_epoch_seconds"],
        "end_epoch_s": oracle_window["window_end_epoch_seconds"],
        "averaging_s": oracle_window["averaging_seconds"],
        "sampling_s": oracle_window["sampling_seconds"],
        "samples": oracle_window["samples"],
        "endpoint_weight": oracle_window["endpoint_weight"],
        "window_count": 1,
    }
    grid_report = {"nx": nx, "ny": ny, "nz": nz, "xlon0_deg": grid["xlon0"],
                   "ylat0_deg": grid["ylat0"], "dx_deg": grid["dx"],
                   "dy_deg": grid["dy"], "heights_m": [float(v) for v in grid["heights_m"]]}
    thresholds_doc = json.loads(Path(args.thresholds).read_text(encoding="utf-8"))
    context = {"case_id": "synthetic-uniform-wind",
               "particle_metrics.mass_budget.relative_error": budget["relative_error"],
               "integrity.non_negative_fields": True,
               "alignment.grid_and_window_match": True}
    evaluations = report_lib.evaluate_thresholds(thresholds_doc, context)
    failed = [e for e in evaluations if e.get("verdict") == "FAIL"]
    if failed:
        overall, detail = "FAIL", "in-scope threshold violated: " + ", ".join(e["id"] for e in failed)
    else:
        overall, detail = ("DIAGNOSTIC", "mass budget and alignment hold; gridded shape "
                           "comparison is single-seed diagnostic, no parity verdict")
    notes.append("Grid diagnostics compare concentration against concentration: the "
                 "candidate mass field is converted to kg/m3 with cell volumes before "
                 "normalization. Raw sums are reported separately with units.")
    return report_lib.make_report(
        case_id="synthetic-uniform-wind", time_window=time_window, grid=grid_report,
        oracle=oracle, candidate=candidate, input_hashes=input_hashes,
        alignment=alignment, particle_metrics=particle_section,
        grid_metrics=grid_section, etex=None, multiseed=None,
        threshold_evaluations=evaluations,
        thresholds_version=thresholds_doc.get("version", "unknown"),
        missing_metrics=missing, notes=notes,
        overall_status=overall, status_detail=detail)


# ---------------------------------------------------------------------------
# ETEX cases
# ---------------------------------------------------------------------------

def _station_mean(windows, grid, lon, lat, start, end):
    duration = (end - start).total_seconds()
    if duration <= 0:
        raise ValueError("invalid observation sampling interval")
    weighted, covered = 0.0, 0.0
    for window in windows:
        overlap = (min(end, window["end"]) - max(start, window["start"])).total_seconds()
        if overlap <= 0:
            continue
        value = io_gpu.bilinear_interpolate(
            window["surface"], grid["xlon0_key"], grid["ylat0_key"],
            grid["dx"], grid["dy"], grid["nx"], grid["ny"], lon, lat)
        if value is None:
            return None
        weighted += overlap * value
        covered += overlap
    if abs(covered - duration) > 1:
        return None
    return weighted / duration


def run_etex(args, case_id, oracle_manifest, oracle, candidate):
    from datetime import datetime as dt
    missing = []
    notes = list(args.note or [])
    if case_id == "etex-mini":
        notes.append("ETEX mini remains a pipeline/observation-pairing regression; "
                     "input audit status INPUT_EQUIVALENCE_NOT_DEMONSTRATED is unchanged "
                     "and no concentration parity is claimed.")
    measurements_data = io_gpu.read_measurements_json(args.measurements)
    observations = measurements_data["measurements"]
    gpu_data = io_gpu.read_etex_gpu_json(args.gpu_output)
    gpu_grid_raw = gpu_data["grid"]
    gpu_grid = {"nx": gpu_grid_raw["nx"], "ny": gpu_grid_raw["ny"], "nz": gpu_grid_raw["nz"],
                "dx": gpu_grid_raw["dx"], "dy": gpu_grid_raw["dy"],
                "xlon0_key": gpu_grid_raw["xlon0"], "ylat0_key": gpu_grid_raw["ylat0"]}
    gpu_windows = []
    for step in gpu_data["timesteps"]:
        surface = io_gpu.gpu_mass_to_surface_concentration_pg_m3(
            step["concentration_mass_kg"], gpu_grid["nx"], gpu_grid["ny"], gpu_grid["nz"],
            gpu_grid["dx"], gpu_grid["dy"], gpu_grid["xlon0_key"], gpu_grid["ylat0_key"],
            gpu_grid_raw["heights_m"])
        gpu_windows.append({
            "start": dt.fromtimestamp(step["window_start_epoch_seconds"], tz=timezone.utc).replace(tzinfo=None),
            "end": dt.fromtimestamp(step["epoch_seconds"], tz=timezone.utc).replace(tzinfo=None),
            "surface": surface,
        })
    fortran_dir = Path(args.fortran_output)
    header_txt_path = fortran_dir / "header_txt"
    dates_path = fortran_dir / "dates"
    if not header_txt_path.is_file():
        raise FileNotFoundError(f"missing oracle artifact: {header_txt_path}")
    if not dates_path.is_file():
        raise FileNotFoundError(f"missing oracle artifact: {dates_path}")
    fortran_grid_raw = io_fortran.parse_header_txt(str(header_txt_path))
    if fortran_grid_raw["interval_s"] <= 0 or \
            fortran_grid_raw["interval_s"] != fortran_grid_raw["averaging_s"]:
        raise ValueError("oracle output is not a complete averaging window")
    stamps = io_fortran.read_dates(str(dates_path))
    fortran_windows = []
    conc_hashes = {}
    for stamp in stamps:
        end = dt.strptime(stamp, "%Y%m%d%H%M%S")
        start_epoch = int(end.replace(tzinfo=timezone.utc).timestamp()) - fortran_grid_raw["averaging_s"]
        start = dt.fromtimestamp(start_epoch, tz=timezone.utc).replace(tzinfo=None)
        field_path = fortran_dir / f"grid_conc_{stamp}_001"
        if not field_path.is_file():
            raise FileNotFoundError(f"missing oracle artifact: {field_path}")
        if field_path.stat().st_size < 60:
            raise ValueError(f"oracle concentration file is incomplete: {field_path}")
        nested = io_fortran.read_grid_conc_txt_format(
            str(field_path), fortran_grid_raw["nx"], fortran_grid_raw["ny"], fortran_grid_raw["nz"])
        surface = [[float(nested[ix][iy][0]) for ix in range(fortran_grid_raw["nx"])]
                   for iy in range(fortran_grid_raw["ny"])]
        for row in surface:
            for value in row:
                if not math.isfinite(value) or value < 0:
                    raise ValueError("oracle surface field has non-finite or negative values")
        fortran_windows.append({"start": start, "end": end, "surface": surface})
        conc_hashes[field_path.name] = report_lib.sha256_file(field_path)
    # Grid alignment (surface-layer heights compared; full column follows OUTGRID).
    grid_match, grid_checks = report_lib.check_grid_alignment(
        {"outlon0": fortran_grid_raw["outlon0"], "outlat0": fortran_grid_raw["outlat0"],
         "nx": fortran_grid_raw["nx"], "ny": fortran_grid_raw["ny"], "nz": fortran_grid_raw["nz"],
         "dx": fortran_grid_raw["dx"], "dy": fortran_grid_raw["dy"],
         "outheights": fortran_grid_raw["outheights"]},
        {"xlon0": gpu_grid_raw["xlon0"], "ylat0": gpu_grid_raw["ylat0"],
         "nx": gpu_grid_raw["nx"], "ny": gpu_grid_raw["ny"], "nz": gpu_grid_raw["nz"],
         "dx": gpu_grid_raw["dx"], "dy": gpu_grid_raw["dy"],
         "heights_m": gpu_grid_raw["heights_m"]})
    if len(gpu_windows) != len(fortran_windows):
        raise ValueError("GPU and oracle output-window counts differ")
    for gpu_w, fortran_w in zip(gpu_windows, fortran_windows):
        if (gpu_w["start"], gpu_w["end"]) != (fortran_w["start"], fortran_w["end"]):
            raise ValueError("GPU and oracle output windows differ")
    alignment = {"grid_and_window_match": bool(grid_match),
                 "grid_checks": grid_checks,
                 "window_count": len(gpu_windows)}
    if not grid_match:
        raise ValueError("output-grid mismatch between GPU and oracle")
    fortran_lookup = {"nx": fortran_grid_raw["nx"], "ny": fortran_grid_raw["ny"],
                      "dx": fortran_grid_raw["dx"], "dy": fortran_grid_raw["dy"],
                      "xlon0_key": fortran_grid_raw["outlon0"],
                      "ylat0_key": fortran_grid_raw["outlat0"]}
    pairs = []
    skipped_no_coverage = 0
    for obs in observations:
        start = dt.fromisoformat(obs["start_time"])
        end = dt.fromisoformat(obs["end_time"])
        gpu_value = _station_mean(gpu_windows, gpu_grid, obs["lon"], obs["lat"], start, end)
        fortran_value = _station_mean(fortran_windows, fortran_lookup, obs["lon"], obs["lat"], start, end)
        if gpu_value is None or fortran_value is None:
            skipped_no_coverage += 1
            continue
        pairs.append({"station": obs["station"], "start_time": obs["start_time"],
                      "end_time": obs["end_time"],
                      "observed_pg_m3": float(obs["concentration_pg_m3"]),
                      "gpu_pg_m3": float(gpu_value),
                      "fortran_pg_m3": float(fortran_value)})
    if not pairs:
        raise ValueError("no observation has complete paired model coverage")
    observed = [p["observed_pg_m3"] for p in pairs]
    gpu_values = [p["gpu_pg_m3"] for p in pairs]
    fortran_values = [p["fortran_pg_m3"] for p in pairs]
    etex = {
        "experiment": "ETEX-1",
        "units": "pg/m3",
        "paired_observation_count": len(pairs),
        "skipped_no_coverage": skipped_no_coverage,
        "output_window_count": len(gpu_windows),
        "missing_rule": ("pairs with incomplete model coverage are excluded and counted "
                         "in skipped_no_coverage; zero values follow Chang-Hanna None rules; "
                         "negative values are integrity errors"),
        "gpu_vs_observations": metrics.etex_station_metrics(observed, gpu_values),
        "fortran_vs_observations": metrics.etex_station_metrics(observed, fortran_values),
        "gpu_vs_fortran_at_stations": metrics.etex_station_metrics(fortran_values, gpu_values),
        "gpu_timing": metrics.etex_timing(pairs, "gpu_pg_m3"),
        "fortran_timing": metrics.etex_timing(pairs, "fortran_pg_m3"),
    }
    if candidate.get("adapter") is None:
        missing.append({"name": "candidate.adapter",
                        "reason": "no candidate log provided; adapter unknown"})
    missing.append({"name": "particle_metrics",
                    "reason": "ETEX runs provide time-averaged concentration windows; "
                              "no particle dumps are evaluated here"})
    if skipped_no_coverage:
        missing.append({"name": "etex.skipped_pairs",
                        "reason": f"{skipped_no_coverage} observations lack complete model coverage"})
    input_hashes = {
        "measurements": report_lib.sha256_file(args.measurements),
        "candidate_output": report_lib.sha256_file(args.gpu_output),
        "fortran_header_txt": report_lib.sha256_file(header_txt_path),
        "fortran_dates": report_lib.sha256_file(dates_path),
    }
    for name, digest in conc_hashes.items():
        input_hashes[f"fortran_{name}"] = digest
    time_window = {
        "start_iso": gpu_windows[0]["start"].strftime("%Y-%m-%dT%H:%M:%SZ"),
        "end_iso": gpu_windows[-1]["end"].strftime("%Y-%m-%dT%H:%M:%SZ"),
        "start_epoch_s": int(gpu_windows[0]["start"].replace(tzinfo=timezone.utc).timestamp()),
        "end_epoch_s": int(gpu_windows[-1]["end"].replace(tzinfo=timezone.utc).timestamp()),
        "averaging_s": gpu_data["averaging_seconds"],
        "sampling_s": gpu_data["sampling_seconds"],
        "samples": gpu_data["averaging_seconds"] // gpu_data["sampling_seconds"] + 1,
        "endpoint_weight": 0.5,
        "window_count": len(gpu_windows),
    }
    grid_report = {"nx": gpu_grid_raw["nx"], "ny": gpu_grid_raw["ny"], "nz": gpu_grid_raw["nz"],
                   "xlon0_deg": gpu_grid_raw["xlon0"], "ylat0_deg": gpu_grid_raw["ylat0"],
                   "dx_deg": gpu_grid_raw["dx"], "dy_deg": gpu_grid_raw["dy"],
                   "heights_m": [float(v) for v in gpu_grid_raw["heights_m"]]}
    thresholds_doc = json.loads(Path(args.thresholds).read_text(encoding="utf-8"))
    context = {"case_id": case_id,
               "integrity.non_negative_fields": True,
               "alignment.grid_and_window_match": True}
    evaluations = report_lib.evaluate_thresholds(thresholds_doc, context)
    return report_lib.make_report(
        case_id=case_id, time_window=time_window, grid=grid_report,
        oracle=oracle, candidate=candidate, input_hashes=input_hashes,
        alignment=alignment, particle_metrics=None, grid_metrics=None,
        etex=etex, multiseed=None,
        threshold_evaluations=evaluations,
        thresholds_version=thresholds_doc.get("version", "unknown"),
        missing_metrics=missing, notes=notes,
        overall_status="DIAGNOSTIC",
        status_detail=("paired observation diagnostics computed; no parity verdict "
                       "(ETEX mini input equivalence not demonstrated)"))


# ---------------------------------------------------------------------------
# Multi-seed aggregation
# ---------------------------------------------------------------------------

def run_aggregate(args, oracle_manifest, oracle, candidate):
    reports = []
    for pattern in args.seed_reports:
        expanded = sorted(str(p) for p in REPO_ROOT.glob(pattern)) if any(
            ch in pattern for ch in "*?[") else [pattern]
        if not expanded:
            raise FileNotFoundError(f"no seed reports match: {pattern}")
        for path in expanded:
            with open(path, encoding="utf-8") as stream:
                reports.append((path, json.load(stream)))
    if len(reports) < 1:
        raise ValueError("aggregation needs at least one seed report")
    case_ids = {doc.get("case_id") for _, doc in reports}
    if len(case_ids) != 1:
        raise ValueError(f"seed reports mix case ids: {sorted(case_ids)}")
    case_id = case_ids.pop()
    schemas = {doc.get("schema_version") for _, doc in reports}
    if schemas != {report_lib.SCHEMA_VERSION}:
        raise ValueError(f"seed reports mix schema versions: {sorted(schemas)}")
    scalar_paths = [
        ("grid_metrics.center_distance_km", lambda d: (d.get("grid_metrics") or {}).get("center_distance_km")),
        ("grid_metrics.field_correlation",
         lambda d: ((d.get("grid_metrics") or {}).get("field_comparison_normalized_shape") or {}).get("correlation")),
        ("particle_metrics.mass_budget_relative_error",
         lambda d: ((d.get("particle_metrics") or {}).get("mass_budget") or {}).get("relative_error")),
        ("etex.gpu_fb",
         lambda d: ((d.get("etex") or {}).get("gpu_vs_observations") or {}).get("fractional_bias")),
        ("etex.gpu_nmse",
         lambda d: ((d.get("etex") or {}).get("gpu_vs_observations") or {}).get("nmse")),
    ]
    aggregated = {}
    for name, getter in scalar_paths:
        values = [getter(doc) for _, doc in reports]
        values = [v for v in values if v is not None]
        if values:
            aggregated[name] = metrics.aggregate_seed_values(values)
        else:
            aggregated[name] = None
    oracle_controllable = bool(args.oracle_seed_controllable)
    multiseed = {
        "n_seeds": len(reports),
        "seed_reports": [path for path, _ in reports],
        "oracle_seed_controllable": oracle_controllable,
        "metrics": aggregated,
    }
    missing = []
    notes = list(args.note or [])
    if not oracle_controllable:
        missing.append({"name": "multiseed.oracle_seed_control",
                        "reason": "oracle runner does not expose a controllable seed; "
                                  "NOT_SUITABLE_FOR_MULTISEED_PARITY"})
        notes.append("Runs without a controllable oracle seed cannot support a "
                     "multi-seed parity proof.")
    if len(reports) < 10:
        notes.append(f"Only {len(reports)} seeds aggregated; at least 10 independent seeds "
                     "are required for a stochastic parity claim.")
    thresholds_doc = json.loads(Path(args.thresholds).read_text(encoding="utf-8"))
    evaluations = report_lib.evaluate_thresholds(
        thresholds_doc, {"case_id": case_id})
    first = reports[0][1]
    built = report_lib.make_report(
        case_id=case_id, time_window=first.get("time_window"), grid=first.get("grid"),
        oracle=oracle, candidate=candidate,
        input_hashes={f"seed_report_{i}": report_lib.sha256_file(p) for i, (p, _) in enumerate(reports)},
        alignment=first.get("alignment"), particle_metrics=None, grid_metrics=None,
        etex=None, multiseed=multiseed,
        threshold_evaluations=evaluations,
        thresholds_version=thresholds_doc.get("version", "unknown"),
        missing_metrics=missing, notes=notes,
        overall_status="DIAGNOSTIC",
        status_detail=(f"aggregated {len(reports)} seeds; "
                       "diagnostic unless >= 10 seeds with controllable oracle seed"))
    return built


def _expand_patterns(patterns):
    expanded = []
    for pattern in patterns:
        if any(char in pattern for char in "*?["):
            matches = sorted(str(p) for p in REPO_ROOT.glob(pattern))
            if not matches:
                raise FileNotFoundError(f"no files match: {pattern}")
            expanded.extend(matches)
        else:
            expanded.append(pattern)
    return expanded


def _flexpart_stamp_to_epoch(stamp):
    return int(datetime.strptime(stamp, "%Y%m%d%H%M%S")
               .replace(tzinfo=timezone.utc).timestamp())


def run_corpus_seeds(args, oracle_manifest, oracle, candidate):
    """Evaluate corpus candidate particle ensembles over independent seeds.

    Consumes ``seed_*.json`` files from ``src/bin/corpus-run.rs`` together
    with the versioned case definition. Particle metrics (kilogram mass
    budget, center of mass, horizontal covariance/eigenvalues, vertical
    quantiles) use :mod:`metrics` and are aggregated over seeds with means,
    dispersion and approximate 95% confidence intervals. The runner's own
    ``metrics`` block is cross-checked as an independent implementation.
    An optional oracle directory contributes particle-space diagnostics from
    ``partposit`` dumps. There is no parity verdict: the corpus thresholds
    version no oracle-vs-candidate gate and the Fortran runner exposes no
    controllable seed.
    """
    missing = []
    notes = list(args.note or [])
    case_def = io_corpus.read_case_definition(args.case_def)
    case_id = case_def["case_id"]
    release_mass = float(case_def["release"]["mass_kg_total"])
    switches = case_def["physics_switches"]
    deposition_on = bool(switches.get("dry_deposition") or
                         switches.get("wet_deposition") or switches.get("decay"))
    expected_seeds = int(case_def["seeds"].get("count", 0)) or None

    seed_paths = _expand_patterns(args.candidate_seeds)
    seeds = []
    input_hashes = {"case_definition": report_lib.sha256_file(args.case_def)}
    for path in seed_paths:
        data = io_corpus.read_seed_file(path)
        if data["case_id"] != case_id:
            raise ValueError(f"seed file {path} belongs to {data['case_id']}, "
                             f"expected {case_id}")
        seeds.append((path, data))
        input_hashes[f"candidate_{Path(path).name}"] = report_lib.sha256_file(path)
    if not seeds:
        raise ValueError("corpus-seeds needs at least one seed file")
    notes.append("Particle metrics only: seed files carry particle ensembles, "
                 "no shared concentration grid.")
    if deposition_on:
        notes.append("Deposition or decay is enabled: the deposited/decayed "
                     "reservoirs are not recorded in seed files, so the kilogram "
                     "budget stays open by the removed mass and is diagnostic.")
        missing.append({"name": "particle_metrics.deposited_reservoirs",
                        "reason": "seed files record particle masses only; "
                                  "removed mass cannot be closed into a budget"})

    per_seed = []
    for path, data in seeds:
        lons, lats, heights, masses = io_corpus.seed_particles(data)
        for value in masses:
            if not math.isfinite(value) or value < 0:
                raise ValueError(f"non-finite or negative particle mass in {path}")
        budget = metrics.mass_budget(release_mass, float(sum(masses)))
        com = metrics.center_of_mass_particles(lons, lats, heights, masses)
        cov = metrics.horizontal_covariance(lons, lats, masses)
        quantiles = metrics.vertical_quantiles(heights, masses, (0.1, 0.5, 0.9))
        mean_z = sum(heights) / len(heights)
        std_z = math.sqrt(sum((z - mean_z) ** 2 for z in heights) / len(heights))
        cross_check = _cross_check_runner_metrics(data["metrics"], com, cov, quantiles,
                                                  mean_z, std_z)
        per_seed.append({
            "seed_file": Path(path).name,
            "seed_index": data["seed_index"],
            "philox_key": data["philox_key"],
            "adapter": data["adapter"],
            "particle_count": len(masses),
            "mass_budget": budget,
            "center_of_mass": com,
            "horizontal_covariance": cov,
            "vertical_quantiles_m": quantiles,
            "z_mean_m": mean_z,
            "z_std_m": std_z,
            "z_min_m": min(heights),
            "z_max_m": max(heights),
            "runner_metrics_cross_check": cross_check,
        })
    adapters = {entry["adapter"] for entry in per_seed}
    if len(adapters) != 1:
        raise ValueError(f"seed files mix adapters: {sorted(adapters)}")
    if candidate.get("adapter") is None:
        candidate["adapter"] = per_seed[0]["adapter"]

    series = {
        "total_mass_kg": [e["mass_budget"]["remaining_mass_kg"] for e in per_seed],
        "mass_rel_error": [e["mass_budget"]["relative_error"] for e in per_seed],
        "com_lon_deg": [e["center_of_mass"]["lon_deg"] for e in per_seed],
        "com_lat_deg": [e["center_of_mass"]["lat_deg"] for e in per_seed],
        "com_z_m": [e["center_of_mass"]["z_m"] for e in per_seed],
        "sigma_east_km": [e["horizontal_covariance"]["sigma_east_km"] for e in per_seed],
        "sigma_north_km": [e["horizontal_covariance"]["sigma_north_km"] for e in per_seed],
        "eigenvalue_small_km2": [e["horizontal_covariance"]["eigenvalues_km2"][0] for e in per_seed],
        "eigenvalue_large_km2": [e["horizontal_covariance"]["eigenvalues_km2"][1] for e in per_seed],
        "z_p10_m": [e["vertical_quantiles_m"]["quantiles_m"]["0.1"] for e in per_seed],
        "z_p50_m": [e["vertical_quantiles_m"]["quantiles_m"]["0.5"] for e in per_seed],
        "z_p90_m": [e["vertical_quantiles_m"]["quantiles_m"]["0.9"] for e in per_seed],
    }
    aggregated = {name: metrics.aggregate_seed_values(values)
                  for name, values in series.items()}
    multiseed = {
        "n_seeds": len(seeds),
        "expected_seeds": expected_seeds,
        "seed_files": [Path(p).name for p, _ in seeds],
        "oracle_seed_controllable": False,
        "oracle_seed_note": ("Fortran random_mod.f90 derives seeds internally "
                             "(iseed1=-7-i, iseed2=-88-i) with no namelist override; "
                             "NOT_SUITABLE_FOR_MULTISEED_PARITY"),
        "metrics": aggregated,
    }
    missing.append({"name": "multiseed.oracle_seed_control",
                    "reason": "oracle runner exposes no controllable seed; "
                              "NOT_SUITABLE_FOR_MULTISEED_PARITY"})
    if expected_seeds and len(seeds) != expected_seeds:
        notes.append(f"Found {len(seeds)} seeds but the case requests {expected_seeds}; "
                     "at least 10 independent seeds are required for a stochastic claim.")
    elif len(seeds) < 10:
        notes.append(f"Only {len(seeds)} seeds evaluated; at least 10 independent seeds "
                     "are required for a stochastic parity claim.")

    oracle_section = None
    if args.fortran_output:
        fortran_dir = Path(args.fortran_output)
        partposit_files = io_fortran.find_partposit_files(fortran_dir)
        if partposit_files:
            f_lons, f_lats, f_zs = io_fortran.read_partposit(str(partposit_files[-1]))
            if f_lons:
                unit_mass = [release_mass / len(f_lons)] * len(f_lons)
                oracle_com = metrics.center_of_mass_particles(f_lons, f_lats, f_zs, unit_mass)
                oracle_cov = metrics.horizontal_covariance(f_lons, f_lats, unit_mass)
                oracle_quantiles = metrics.vertical_quantiles(f_zs, unit_mass, (0.1, 0.5, 0.9))
                distances = []
                for entry in per_seed:
                    dlon = entry["center_of_mass"]["lon_deg"] - oracle_com["lon_deg"]
                    dlat = entry["center_of_mass"]["lat_deg"] - oracle_com["lat_deg"]
                    distances.append(math.hypot(
                        dlon * metrics.DEG_TO_KM * math.cos(math.radians(oracle_com["lat_deg"])),
                        dlat * metrics.DEG_TO_KM))
                oracle_section = {
                    "file": partposit_files[-1].name,
                    "particle_count": len(f_lons),
                    "center_of_mass": oracle_com,
                    "horizontal_covariance": oracle_cov,
                    "vertical_quantiles_m": oracle_quantiles,
                    "candidate_center_distances_km": distances,
                    "verdict": "DIAGNOSTIC",
                }
                input_hashes["oracle_partposit"] = report_lib.sha256_file(partposit_files[-1])
            else:
                missing.append({"name": "oracle.partposit",
                                "reason": "oracle partposit file is empty"})
        else:
            missing.append({"name": "oracle.partposit",
                            "reason": "oracle wrote no partposit dump; particle-space "
                                      "oracle comparison unavailable"})
    else:
        missing.append({"name": "oracle",
                        "reason": "no oracle directory supplied; candidate-only evaluation"})

    domain = case_def["domain"]
    integration = case_def.get("integration", {})
    start_stamp = str(integration.get("start", "20240101000000"))
    start_epoch = _flexpart_stamp_to_epoch(start_stamp)
    duration = int(integration.get("total_s", 0))
    time_window = {
        "start_iso": epoch_to_iso(start_epoch),
        "end_iso": epoch_to_iso(start_epoch + duration),
        "start_epoch_s": start_epoch,
        "end_epoch_s": start_epoch + duration,
        "averaging_s": None,
        "sampling_s": int(integration.get("dt_s", 0)) or None,
        "samples": None,
        "endpoint_weight": None,
        "window_count": None,
        "steps": int(integration.get("steps", 0)) or None,
    }
    grid_report = {"nx": domain.get("nx"), "ny": domain.get("ny"), "nz": domain.get("nz"),
                   "xlon0_deg": domain.get("xlon0_deg"), "ylat0_deg": domain.get("ylat0_deg"),
                   "dx_deg": domain.get("dx_deg"), "dy_deg": domain.get("dy_deg"),
                   "heights_m": None}
    missing.append({"name": "grid.heights_m",
                    "reason": "case domain carries wind levels, not output heights; "
                              "no gridded comparison is attempted"})
    particle_section = {"per_seed": per_seed, "aggregated": aggregated,
                        "unit": "kg/m/km"}
    thresholds_doc = json.loads(Path(args.thresholds).read_text(encoding="utf-8"))
    mean_rel_error = aggregated["mass_rel_error"]["mean"]
    context = {"case_id": "corpus-seeds",
               "particle_metrics.mass_budget.relative_error": mean_rel_error,
               "integrity.non_negative_fields": True,
               "alignment.grid_and_window_match": None}
    if deposition_on:
        context["particle_metrics.mass_budget.relative_error"] = None
    evaluations = report_lib.evaluate_thresholds(thresholds_doc, context)
    # The shared threshold file only scopes synthetic/ETEX cases, so every
    # corpus criterion currently reports SKIP; that is recorded, not hidden.
    return report_lib.make_report(
        case_id=case_id, time_window=time_window, grid=grid_report,
        oracle=oracle, candidate=candidate, input_hashes=input_hashes,
        alignment={"grid_and_window_match": None,
                   "checks": [],
                   "note": "particle-ensemble case: no shared concentration grid asserted"},
        particle_metrics=particle_section, grid_metrics=None,
        etex=None, multiseed=multiseed,
        threshold_evaluations=evaluations,
        thresholds_version=thresholds_doc.get("version", "unknown"),
        missing_metrics=missing, notes=notes,
        overall_status="DIAGNOSTIC",
        status_detail=("corpus particle-ensemble diagnostics over "
                       f"{len(seeds)} seeds; no parity verdict"))


def _cross_check_runner_metrics(runner, com, cov, quantiles, mean_z, std_z):
    """Compare this evaluation against the runner's independent metrics block.

    The corpus runner computes its moments in metres; this evaluation uses
    kilometres. Differences are reported, never asserted, so a divergence
    is investigated rather than hidden.
    """
    diffs = {}
    diffs["com_lon_deg"] = abs(float(runner["com_lon_deg"]) - com["lon_deg"])
    diffs["com_lat_deg"] = abs(float(runner["com_lat_deg"]) - com["lat_deg"])
    diffs["com_z_m"] = abs(float(runner["com_z_m"]) - com["z_m"])
    diffs["sigma_east_m"] = abs(
        math.sqrt(max(float(runner["cov_east_m2"]), 0.0)) - cov["sigma_east_km"] * 1000.0)
    diffs["sigma_north_m"] = abs(
        math.sqrt(max(float(runner["cov_north_m2"]), 0.0)) - cov["sigma_north_km"] * 1000.0)
    runner_eig = sorted(float(v) for v in runner["horizontal_eigenvalues_m2"])
    mine_eig = sorted(v * 1e6 for v in cov["eigenvalues_km2"])
    diffs["eigenvalue_small_m2"] = abs(runner_eig[0] - mine_eig[0])
    diffs["eigenvalue_large_m2"] = abs(runner_eig[1] - mine_eig[1])
    diffs["z_p10_m"] = abs(float(runner["z_p10_m"]) - quantiles["quantiles_m"]["0.1"])
    diffs["z_p50_m"] = abs(float(runner["z_p50_m"]) - quantiles["quantiles_m"]["0.5"])
    diffs["z_p90_m"] = abs(float(runner["z_p90_m"]) - quantiles["quantiles_m"]["0.9"])
    diffs["z_mean_m"] = abs(float(runner["z_mean_m"]) - mean_z)
    diffs["z_std_m"] = abs(float(runner["z_std_m"]) - std_z)
    diffs["total_mass_kg"] = abs(float(runner["total_mass_kg"]) - quantiles["total_mass_kg"])
    return diffs


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--case", required=True,
                        choices=["synthetic-uniform-wind", "etex-mini", "etex-full",
                                 "aggregate-seeds", "corpus-seeds"])
    parser.add_argument("--fortran-output", default=None)
    parser.add_argument("--gpu-output", default=None)
    parser.add_argument("--measurements", default=None)
    parser.add_argument("--seed-reports", nargs="*", default=None)
    parser.add_argument("--candidate-seeds", nargs="*", default=None)
    parser.add_argument("--case-def", default=None)
    parser.add_argument("--output", required=True)
    parser.add_argument("--summary", default=None)
    parser.add_argument("--thresholds", default=str(DEFAULT_THRESHOLDS))
    parser.add_argument("--oracle-manifest", default=str(DEFAULT_ORACLE_MANIFEST))
    parser.add_argument("--oracle-checkout", default=None)
    parser.add_argument("--oracle-executable", default=None)
    parser.add_argument("--oracle-seed", type=int, default=None)
    parser.add_argument("--oracle-seed-controllable", action="store_true")
    parser.add_argument("--candidate-checkout", default=None)
    parser.add_argument("--candidate-executable", default=None)
    parser.add_argument("--candidate-log", default=None)
    parser.add_argument("--candidate-revision", default=None)
    parser.add_argument("--seed", type=int, default=None)
    parser.add_argument("--candidate-seed-controllable", action="store_true")
    parser.add_argument("--note", action="append", default=[])
    return parser


def main():
    parser = build_parser()
    args = parser.parse_args()
    try:
        oracle_manifest = load_oracle_manifest(args.oracle_manifest)
        oracle, candidate, _ = build_provenance(args, oracle_manifest)
        if args.case == "synthetic-uniform-wind":
            if not args.fortran_output or not args.gpu_output:
                parser.error("synthetic-uniform-wind needs --fortran-output and --gpu-output")
            built = run_synthetic(args, oracle_manifest, oracle, candidate)
        elif args.case in ("etex-mini", "etex-full"):
            if not args.measurements or not args.fortran_output or not args.gpu_output:
                parser.error(f"{args.case} needs --measurements, --fortran-output and --gpu-output")
            built = run_etex(args, args.case, oracle_manifest, oracle, candidate)
        elif args.case == "aggregate-seeds":
            if not args.seed_reports:
                parser.error("aggregate-seeds needs --seed-reports")
            built = run_aggregate(args, oracle_manifest, oracle, candidate)
        elif args.case == "corpus-seeds":
            if not args.candidate_seeds or not args.case_def:
                parser.error("corpus-seeds needs --candidate-seeds and --case-def")
            built = run_corpus_seeds(args, oracle_manifest, oracle, candidate)
        else:
            parser.error(f"unknown case: {args.case}")
    except (FileNotFoundError, ValueError) as exc:
        try:
            oracle_manifest = load_oracle_manifest(args.oracle_manifest)
        except (FileNotFoundError, ValueError):
            print(f"INTEGRITY_ERROR: {exc}", file=sys.stderr)
            sys.exit(2)
        try:
            oracle, candidate, _ = build_provenance(args, oracle_manifest)
        except (FileNotFoundError, ValueError) as inner:
            print(f"INTEGRITY_ERROR: {inner}", file=sys.stderr)
            sys.exit(2)
        case_id = args.case if args.case != "aggregate-seeds" else "aggregate-seeds"
        built = _integrity_report(
            case_id, args, oracle, candidate, {},
            [{"name": "evaluation", "reason": str(exc)}], f"integrity error: {exc}",
            list(args.note or []))
        write_outputs(built, args)
        print(f"INTEGRITY_ERROR: {exc}", file=sys.stderr)
        sys.exit(2)
    write_outputs(built, args)
    if built["overall_status"] == "FAIL":
        sys.exit(1)


if __name__ == "__main__":
    main()
