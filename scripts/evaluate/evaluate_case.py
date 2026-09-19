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


def _load_run_manifest(path):
    """Load an oracle run-manifest (write_oracle_run_manifest.py output)."""
    with open(path, encoding="utf-8") as stream:
        manifest = json.load(stream)
    for key in ("oracle", "candidate"):
        if key not in manifest:
            raise ValueError(f"run manifest lacks {key}: {path}")
    return manifest


def _verify_artifacts_against_manifest(manifest, labeled_paths):
    """Verify supplied files against manifest hashes.

    ``labeled_paths`` holds ``(label, path, kind)`` triples; kind is only
    carried through for the caller. Returns ``(verified, uncovered)`` label
    lists. A hash mismatch raises ValueError; files with no manifest entry
    are reported as uncovered (not verified, not failed).
    """
    digests = {}
    for section in ("input_sha256", "output_sha256"):
        section_hashes = manifest.get(section, {})
        if isinstance(section_hashes, dict):
            digests.update(section_hashes)
    by_basename = {}
    for key, value in digests.items():
        by_basename.setdefault(Path(key).name, []).append(value)
    verified, uncovered = [], []
    for label, path, _kind in labeled_paths:
        actual = report_lib.sha256_file(path)
        resolved = str(Path(path).resolve())
        if resolved in digests:
            if digests[resolved] != actual:
                raise ValueError(
                    f"artifact {label} hash differs from run manifest {resolved}")
            verified.append(label)
        elif Path(path).name in by_basename:
            if actual not in by_basename[Path(path).name]:
                raise ValueError(
                    f"artifact {label} hash differs from run manifest entries")
            verified.append(label)
        else:
            uncovered.append(label)
    return verified, uncovered


def build_provenance(args, oracle_manifest, embedded_revisions=None,
                     embedded_adapters=None, artifact_paths=None):
    """Build oracle/candidate provenance without misattribution.

    The candidate revision is only reported when it is tied to the supplied
    artifacts: embedded in them, hash-verified through ``--run-manifest``
    covering every supplied candidate output, or explicitly declared with
    ``--candidate-revision`` (labeled as declared). The evaluator checkout's
    HEAD is never attributed to previously generated outputs. Likewise the
    oracle output is only attributed to the pinned oracle when every
    consumed oracle output is hash-covered by a run manifest whose oracle
    entry is the pinned clean commit; a verified source checkout alone does
    not link supplied outputs to a run, so output attribution stays
    explicitly unverified in that case.

    Returns ``(oracle, candidate, prov_missing, prov_notes)``.
    """
    missing = []
    notes = []
    artifact_paths = artifact_paths or []
    manifest = _load_run_manifest(args.run_manifest) if args.run_manifest else None
    if args.run_manifest:
        notes.append(f"Run manifest consumed: {args.run_manifest}")

    candidate_revision = None
    revision_source = None
    embedded_gap = False
    if embedded_revisions:
        unique = set(embedded_revisions)
        usable = {r for r in unique if r not in (None, "unknown")}
        unattributed = len(embedded_revisions) - sum(
            1 for r in embedded_revisions if r not in (None, "unknown"))
        if unattributed:
            embedded_gap = True
            missing.append(
                {"name": "candidate.revision",
                 "reason": f"{unattributed} of {len(embedded_revisions)} supplied "
                           "artifacts embed no usable revision (unknown/missing); "
                           "the set stays explicitly unverified"})
        elif len(usable) > 1:
            raise ValueError(
                f"candidate artifacts mix revisions: {sorted(usable)}")
        elif usable:
            candidate_revision = usable.pop()
            revision_source = "embedded-in-artifact"
    if candidate_revision is None and manifest is not None:
        candidates = [t for t in artifact_paths if t[2] == "candidate"]
        verified, uncovered = _verify_artifacts_against_manifest(
            manifest, candidates)
        for label in uncovered:
            missing.append({"name": f"provenance.{label}",
                            "reason": "artifact is not covered by the run manifest"})
        if verified and not uncovered:
            candidate_revision = manifest["candidate"].get("commit")
            revision_source = "run-manifest-hash-verified"
            notes.append(f"Candidate revision hash-verified via run manifest "
                         f"({len(verified)} artifacts)")
        elif verified:
            notes.append("Candidate revision not attributed via run manifest: "
                         f"{len(uncovered)} supplied candidate outputs are not "
                         "covered by it")
    if candidate_revision is None and args.candidate_revision:
        candidate_revision = args.candidate_revision
        revision_source = "declared-flag"
        notes.append("Candidate revision is declared via --candidate-revision "
                     "and is not hash-verified")
    candidate_dirty = None
    if manifest is not None and revision_source == "run-manifest-hash-verified":
        candidate_dirty = manifest["candidate"].get("worktree_dirty")
    if candidate_revision is None and not embedded_gap:
        missing.append({"name": "candidate.revision",
                        "reason": "no revision tied to the supplied artifacts "
                                  "(use embedded revisions, --run-manifest or "
                                  "--candidate-revision); the evaluator checkout HEAD "
                                  "is deliberately not attributed"})
    adapter = None
    if embedded_adapters:
        unique_adapters = set(embedded_adapters)
        if len(unique_adapters) != 1:
            raise ValueError(
                f"candidate artifacts mix adapters: {sorted(unique_adapters)}")
        adapter = unique_adapters.pop()
    elif args.candidate_log:
        adapter = report_lib.read_adapter_from_log(args.candidate_log)

    oracle_checkout = Path(args.oracle_checkout) if args.oracle_checkout else None
    attribution = "unverified"
    checkout_verified = False
    if oracle_checkout is not None:
        actual, dirty = report_lib.git_revision(oracle_checkout)
        if actual != oracle_manifest["pinned_commit"] or dirty:
            raise ValueError(
                "oracle checkout is not the pinned unmodified FLEXPART source "
                f"(expected {oracle_manifest['pinned_commit']}, got {actual}, dirty={dirty})")
        checkout_verified = True
        notes.append("Oracle sources verified at the pinned commit, but the "
                     "supplied outputs are not linked to that checkout run; "
                     "output attribution stays unverified "
                     "(use --run-manifest for output attribution)")
    if attribution == "unverified" and manifest is not None:
        oracle_state = manifest.get("oracle", {})
        oracles = [t for t in artifact_paths if t[2] == "oracle"]
        verified, uncovered = _verify_artifacts_against_manifest(
            manifest, oracles)
        for label in uncovered:
            missing.append({"name": f"provenance.{label}",
                            "reason": "artifact is not covered by the run manifest"})
        if (oracle_state.get("commit") == oracle_manifest["pinned_commit"]
                and not oracle_state.get("worktree_dirty")
                and verified and not uncovered and oracles):
            attribution = "run-manifest"
            notes.append(f"Oracle output hash-verified via run manifest "
                         f"({len(verified)} artifacts)")
        else:
            notes.append("Oracle output is not tied to a hash-verified pinned run")
    if attribution == "unverified" and manifest is None and oracle_checkout is None:
        notes.append("Oracle output is not tied to a verified pinned checkout "
                     "(use --oracle-checkout or --run-manifest); the pinned commit "
                     "below is the normative requirement, not an attribution")
    oracle = {
        "name": oracle_manifest.get("name", "FLEXPART"),
        "version": oracle_manifest.get("version", "11.1"),
        "pinned_commit": oracle_manifest["pinned_commit"],
        "attribution": attribution,
        "checkout_verified": checkout_verified,
        "seed": args.oracle_seed,
        "seed_controllable": bool(args.oracle_seed_controllable),
    }
    if not oracle["seed_controllable"]:
        oracle["seed_note"] = ("oracle seed is not exposed by this runner; "
                               "not suitable for multi-seed parity proof")
    candidate = {
        "revision": candidate_revision,
        "revision_source": revision_source,
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
    return oracle, candidate, missing, notes


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

def run_synthetic(args, oracle_manifest):
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
    oracle, candidate, prov_missing, prov_notes = build_provenance(
        args, oracle_manifest,
        artifact_paths=[("fortran_header", str(header_path), "oracle"),
                        ("fortran_concentration", str(last_conc), "oracle"),
                        ("candidate_output", str(args.gpu_output), "candidate")])
    missing.extend(prov_missing)
    notes.extend(prov_notes)
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
    # Spatial moments weight physical cell mass, including layer thickness.
    # The concentration-normalized shape metric above remains a separate
    # diagnostic with its own quantity stated in the report.
    oracle_mass = io_gpu.concentration_to_mass_per_cell(
        oracle_flat, nx, ny, nz, header["outlat0"], header["dxout"],
        header["dyout"], header["outheights"])
    oracle_com = metrics.center_of_mass_grid(
        oracle_mass, nx, ny, nz, header["outlon0"], header["outlat0"],
        header["dxout"], header["dyout"], header["outheights"])
    candidate_com = metrics.center_of_mass_grid(
        candidate_flat, nx, ny, nz, grid["xlon0"], grid["ylat0"],
        grid["dx"], grid["dy"], grid["heights_m"])
    center_distance_km = None
    if oracle_com and candidate_com:
        dlon = candidate_com["lon_deg"] - oracle_com["lon_deg"]
        dlat = candidate_com["lat_deg"] - oracle_com["lat_deg"]
        center_distance_km = math.hypot(
            dlon * metrics.DEG_TO_KM * math.cos(math.radians(oracle_com["lat_deg"])),
            dlat * metrics.DEG_TO_KM)
    oracle_cov = metrics.horizontal_covariance_grid(
        oracle_mass, nx, ny, nz, header["outlon0"], header["outlat0"],
        header["dxout"], header["dyout"])
    candidate_cov = metrics.horizontal_covariance_grid(
        candidate_flat, nx, ny, nz, grid["xlon0"], grid["ylat0"], grid["dx"], grid["dy"])
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
        "spatial_moments_unit": "relative mass per output cell (oracle); kg per output cell (candidate)",
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


def _consume_input_audit(args, case_id, missing, notes, input_hashes):
    """Consume the ETEX input-equivalence audit instead of asserting its status.

    Reads ``--input-equivalence-report`` (the ``audit_input_equivalence.py``
    output), hashes it and records its ``status``. A missing audit is an
    explicit gap, never a silent pass; an unreadable audit file is an
    integrity error. Returns the audit record or None.
    """
    if not args.input_equivalence_report:
        if case_id == "etex-mini":
            missing.append(
                {"name": "etex.input_equivalence_audit",
                 "reason": "no --input-equivalence-report supplied; input "
                           "equivalence is unverified by this evaluation"})
            notes.append(
                "Input equivalence was not evaluated by this report; see the "
                "audit artifact (target/etex/mini/input_equivalence_report.json). "
                "No concentration parity is claimed.")
        return None
    path = Path(args.input_equivalence_report)
    if not path.is_file():
        raise FileNotFoundError(f"missing input audit artifact: {path}")
    with open(path, encoding="utf-8") as stream:
        audit = json.load(stream)
    if "status" not in audit:
        raise ValueError(f"input audit lacks a status: {path}")
    digest = report_lib.sha256_file(path)
    input_hashes["input_equivalence_report"] = digest
    status = audit["status"]
    record = {"status": status, "sha256": digest,
              "schema_version": audit.get("schema_version")}
    if status == "FAIL":
        notes.append("Input audit reports FAIL: prepared inputs or scenario checks "
                     "violate technical tolerances; output metrics below stay diagnostic.")
    elif status == "INPUT_EQUIVALENCE_NOT_DEMONSTRATED":
        notes.append("Input audit reports INPUT_EQUIVALENCE_NOT_DEMONSTRATED: the "
                     "Fortran oracle and the candidate inputs differ scientifically "
                     "(levels, velocity, timesteps, PBL diagnostics); output metrics "
                     "below stay diagnostic and no concentration parity is claimed.")
    else:
        notes.append(f"Input audit reports unexpected status {status}; input "
                     "equivalence is treated as unverified.")
    return record


def run_etex(args, case_id, oracle_manifest):
    from datetime import datetime as dt
    missing = []
    notes = list(args.note or [])
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
    oracle, candidate, prov_missing, prov_notes = build_provenance(
        args, oracle_manifest,
        artifact_paths=[("measurements", str(args.measurements), "input"),
                        ("candidate_output", str(args.gpu_output), "candidate"),
                        ("fortran_header_txt", str(header_txt_path), "oracle"),
                        ("fortran_dates", str(dates_path), "oracle")] +
                       [("fortran_concentration", str(fortran_dir / name), "oracle")
                        for name in conc_hashes])
    missing.extend(prov_missing)
    notes.extend(prov_notes)
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
    audit_record = _consume_input_audit(args, case_id, missing, notes, input_hashes)
    etex["input_equivalence_audit"] = audit_record
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
    if audit_record is not None:
        audit_detail = f"input audit status {audit_record['status']}"
    else:
        audit_detail = "input equivalence not evaluated by this report"
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
                       f"({audit_detail})"))


# ---------------------------------------------------------------------------
# Multi-seed aggregation
# ---------------------------------------------------------------------------

def run_aggregate(args, oracle_manifest):
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
    # Seed independence: files are only independent seeds when their recorded
    # candidate seeds are distinct. Reports without a recorded seed cannot
    # prove independence, so the aggregation stays explicitly unverified.
    recorded_seeds = [doc.get("candidate", {}).get("seed") for _, doc in reports]
    recorded_seeds = [s for s in recorded_seeds if s is not None]
    if len(recorded_seeds) != len(set(recorded_seeds)):
        raise ValueError("seed reports contain duplicate candidate seeds; "
                         "files are not independent seeds")
    if len(recorded_seeds) < len(reports):
        seed_independence = "unverified"
    else:
        seed_independence = "verified-unique"
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
        "seed_independence": seed_independence,
        "recorded_candidate_seeds": recorded_seeds,
        "oracle_seed_controllable": oracle_controllable,
        "metrics": aggregated,
    }
    missing = []
    notes = list(args.note or [])
    if seed_independence == "unverified":
        missing.append({"name": "multiseed.seed_independence",
                        "reason": "some seed reports record no candidate seed; "
                                  "file independence is unverified"})
        notes.append("Seed independence is unverified: reports without recorded "
                     "candidate seeds may repeat the same run.")
    # Candidate revision across reports: only a unanimous revision with a
    # trusted per-report source is attributed; legacy reports without a
    # revision source never contribute an attribution.
    trusted_sources = {"embedded-in-artifact", "run-manifest-hash-verified",
                       "declared-flag"}
    consensus = {(d.get("candidate", {}).get("revision"),
                  d.get("candidate", {}).get("revision_source")) for _, d in reports}
    oracle, candidate, prov_missing, prov_notes = build_provenance(
        args, oracle_manifest)
    missing.extend(prov_missing)
    notes.extend(prov_notes)
    if len(consensus) == 1:
        revision, source = consensus.pop()
        if revision is not None and source in trusted_sources:
            candidate["revision"] = revision
            candidate["revision_source"] = source
            missing[:] = [m for m in missing if m.get("name") != "candidate.revision"]
    else:
        revisions = sorted({r for r, _ in consensus if r is not None})
        if revisions:
            notes.append(f"Seed reports mix candidate revisions {revisions}; "
                         "no single revision is attributed.")
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


def _threshold_gate_value(thresholds_doc, criterion_id):
    """Look up a numeric gate value from the versioned thresholds document."""
    for criterion in thresholds_doc.get("criteria", []):
        if criterion.get("id") == criterion_id:
            return float(criterion["value"])
    raise ValueError(f"thresholds lack criterion {criterion_id}")


def _candidate_philox_block(case_def, case_id):
    """Canonical v2 Philox block for a case definition.

    Reads ``stochastic.candidate_philox`` (base_key, base_counter, count,
    identical_repeats). Raises ValueError for legacy v1 documents, missing
    identities (except ADV-ANA-001, which hardcodes key ``[0, 0]``), and
    malformed values. No default key is ever substituted.
    """
    if case_def.get("schema_version") != 2 or "version" in case_def:
        raise ValueError(
            f"case {case_id} is not a canonical v2 document "
            "(schema_version 2 required; v1 is frozen)")
    stochastic = case_def.get("stochastic") or {}
    block = stochastic.get("candidate_philox")
    if block is None:
        if case_id == "ADV-ANA-001":
            return None
        raise ValueError(
            f"case {case_id} declares no stochastic.candidate_philox; "
            "no default key substituted")
    base_key = block.get("base_key")
    base_counter = block.get("base_counter")
    if (not isinstance(base_key, list) or len(base_key) != 2
            or not all(isinstance(v, int) and 0 <= v < 2**32 for v in base_key)):
        raise ValueError(f"case {case_id} has malformed candidate base_key")
    if (not isinstance(base_counter, list) or len(base_counter) != 4
            or not all(isinstance(v, int) and 0 <= v < 2**32 for v in base_counter)):
        raise ValueError(f"case {case_id} has malformed candidate base_counter")
    return block


def _expected_philox_identity(case_def, case_id, seed_index):
    """Expected (key, counter) for a seed file, or None when unchecked.

    Follows the corpus runner derivation: ``ADV-ANA-001`` hardcodes key
    ``[0, 0]``; manifests with ``identical_repeats`` (REPEAT-009) rerun the
    base key identically by design; all other driver cases use
    ``[base0 + seed_index, base1]`` with the declared counter (wrapping at
    2^32). Returns None only for ADV-ANA-001.
    """
    if case_id == "ADV-ANA-001":
        return [0, 0], [0, 0, 0, 0]
    block = _candidate_philox_block(case_def, case_id)
    base_key = block["base_key"]
    base_counter = block["base_counter"]
    if block.get("identical_repeats", False):
        expected_key = [int(base_key[0]), int(base_key[1])]
    else:
        expected_key = [(int(base_key[0]) + int(seed_index)) % 2**32,
                        int(base_key[1])]
    return expected_key, [int(v) for v in base_counter]


def _validate_seed_identities(seeds, case_def, case_id):
    """Reject files that are not distinct, correctly derived seeds.

    Every file must carry a unique ``seed_index`` and a Philox identity
    matching the case derivation. Duplicate (key, counter) pairs are
    rejected, except for ``REPEAT-009`` whose designed repeat reruns one
    identity across its files (all of which must then share it).
    """
    seen_indices = set()
    seen_identities = set()
    for path, data in seeds:
        seed_index = data["seed_index"]
        if seed_index in seen_indices:
            raise ValueError(f"duplicate seed_index {seed_index} in {path}")
        seen_indices.add(seed_index)
        identity = (tuple(int(v) for v in data["philox_key"]),
                    tuple(int(v) for v in data["philox_counter"]))
        expected = _expected_philox_identity(case_def, case_id, seed_index)
        if expected is not None and list(identity[0]) != expected[0]:
            raise ValueError(
                f"seed file {path} key {list(identity[0])} does not match "
                f"the case Philox derivation (expected {expected[0]})")
        if expected is not None and list(identity[1]) != expected[1]:
            raise ValueError(
                f"seed file {path} counter {list(identity[1])} does not match "
                f"the case Philox derivation (expected {expected[1]})")
        seen_identities.add(identity)
    if case_id == "REPEAT-009":
        if len(seen_identities) != 1:
            raise ValueError("REPEAT-009 must rerun a single Philox identity; "
                             f"found {len(seen_identities)} distinct identities")
    elif len(seen_identities) != len(seeds):
        raise ValueError("seed files contain duplicate Philox identities; "
                         "files are not independent seeds")


def run_corpus_seeds(args, oracle_manifest):
    """Evaluate corpus candidate particle ensembles over independent seeds.

    Consumes ``seed_*.json`` files from ``src/bin/corpus-run.rs`` together
    with the versioned case definition. Seed files are validated as distinct,
    correctly derived Philox identities before any statistics are computed.
    Particle metrics (kilogram mass budget, center of mass, horizontal
    covariance/eigenvalues, vertical quantiles) use :mod:`metrics` and are
    aggregated over seeds with means, dispersion and approximate 95%
    confidence intervals. The mass-closure gate applies to every seed: the
    worst absolute error governs, so opposite-sign errors cannot cancel into
    a pass. The runner's own ``metrics`` block is cross-checked on an
    unweighted basis with unit-aware tolerances; a divergence is an
    integrity error. An optional oracle directory contributes particle-space
    diagnostics from ``partposit`` dumps. There is no parity verdict: the
    corpus thresholds version no oracle-vs-candidate gate and the Fortran
    runner exposes no controllable seed.
    """
    missing = []
    notes = list(args.note or [])
    case_def = io_corpus.read_case_definition(args.case_def)
    case_id = case_def["case_id"]
    release_mass = float(case_def["release"]["mass_kg_total"])
    switches = case_def["physics_switches"]
    deposition_on = bool(switches.get("dry_deposition") or
                         switches.get("wet_deposition") or switches.get("decay"))
    _candidate_philox_block(case_def, case_id)
    candidate_block = (case_def.get("stochastic") or {}).get("candidate_philox") or {}
    expected_seeds = int(candidate_block.get("count", 0)) or None
    thresholds_doc = json.loads(Path(args.thresholds).read_text(encoding="utf-8"))
    gate_value = _threshold_gate_value(thresholds_doc, "MASS_BUDGET_CLOSE")

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
    _validate_seed_identities(seeds, case_def, case_id)
    notes.append("Particle metrics only: seed files carry particle ensembles, "
                 "no shared concentration grid.")
    # Resolve oracle artifacts before provenance so the run manifest can be
    # hash-verified against them. A supplied oracle directory must exist and
    # hold recognized artifacts; otherwise this is an integrity error, not a
    # silent candidate-only run.
    partposit_files, grid_files = [], []
    fortran_dir = None
    if args.fortran_output:
        fortran_dir = Path(args.fortran_output)
        if not fortran_dir.is_dir():
            raise FileNotFoundError(
                f"oracle directory does not exist: {fortran_dir}")
        partposit_files = io_fortran.find_partposit_files(fortran_dir)
        try:
            grid_files = io_fortran.find_grid_conc_files(fortran_dir)
        except FileNotFoundError:
            grid_files = []
        if not partposit_files and not grid_files:
            raise FileNotFoundError(
                f"oracle directory holds no partposit_* or grid_conc_* "
                f"artifacts: {fortran_dir}")
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
        if deposition_on:
            budget_verdict = "NOT_APPLICABLE"
        else:
            budget_verdict = ("PASS" if abs(budget["relative_error"]) < gate_value
                              else "FAIL")
        com = metrics.center_of_mass_particles(lons, lats, heights, masses)
        cov = metrics.horizontal_covariance(lons, lats, masses)
        quantiles = metrics.vertical_quantiles(heights, masses, (0.1, 0.5, 0.9))
        mean_z = sum(heights) / len(heights)
        std_z = math.sqrt(sum((z - mean_z) ** 2 for z in heights) / len(heights))
        # Unweighted basis for the runner cross-check: the runner's moments
        # are unweighted means, so the check compares like with like and only
        # flags genuine implementation divergence (see P2 review finding).
        count = len(masses)
        uniform = [1.0] * count
        uw_com = metrics.center_of_mass_particles(lons, lats, heights, uniform)
        uw_cov = metrics.horizontal_covariance(lons, lats, uniform)
        uw_quantiles = metrics.unweighted_quantiles_linear(heights, (0.1, 0.5, 0.9))
        uw_mean_z = sum(heights) / count
        uw_std_z = math.sqrt(sum((z - uw_mean_z) ** 2 for z in heights) / count)
        uw_total = float(sum(masses))
        cross_check = _cross_check_runner_metrics(
            data["metrics"], uw_com, uw_cov, uw_quantiles, uw_mean_z, uw_std_z,
            uw_total)
        per_seed.append({
            "seed_file": Path(path).name,
            "seed_index": data["seed_index"],
            "philox_key": data["philox_key"],
            "philox_counter": data["philox_counter"],
            "adapter": data["adapter"],
            "particle_count": len(masses),
            "mass_budget": dict(budget, verdict=budget_verdict),
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
    oracle_file = (partposit_files[-1] if partposit_files
                   else (grid_files[-1] if grid_files else None))
    oracle, candidate, prov_missing, prov_notes = build_provenance(
        args, oracle_manifest,
        embedded_revisions=[data.get("candidate_revision") for _, data in seeds],
        embedded_adapters=[data.get("adapter") for _, data in seeds],
        artifact_paths=[(f"candidate_{Path(p).name}", str(p), "candidate")
                        for p, _ in seeds] +
                       ([(f"oracle_{oracle_file.name}", str(oracle_file), "oracle")]
                        if oracle_file is not None else []))
    missing.extend(prov_missing)
    notes.extend(prov_notes)

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
    # Mass-closure gate: every seed is evaluated and the worst absolute
    # error governs, so opposite-sign errors cannot cancel into a pass.
    if deposition_on:
        budget_gate = {"gate": "SKIPPED",
                       "reason": "deposition/decay reservoirs are not recorded"}
        worst_abs_rel_error = None
        failed_seeds = []
    else:
        failed_seeds = [e["seed_file"] for e in per_seed
                        if e["mass_budget"]["verdict"] != "PASS"]
        worst_abs_rel_error = max(abs(e["mass_budget"]["relative_error"])
                                  for e in per_seed)
        budget_gate = {"gate": "PASS" if not failed_seeds else "FAIL",
                       "gate_value": gate_value,
                       "worst_abs_relative_error": worst_abs_rel_error,
                       "failed_seed_count": len(failed_seeds),
                       "failed_seeds": failed_seeds}
    multiseed = {
        "n_seeds": len(seeds),
        "expected_seeds": expected_seeds,
        "seed_files": [Path(p).name for p, _ in seeds],
        "oracle_seed_controllable": False,
        "oracle_seed_note": ("Fortran random_mod.f90 derives seeds internally "
                             "(iseed1=-7-i, iseed2=-88-i) with no namelist override; "
                             "NOT_SUITABLE_FOR_MULTISEED_PARITY"),
        "mass_budget_gate": budget_gate,
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
        elif grid_files:
            missing.append({"name": "oracle.particle_comparison",
                            "reason": "oracle directory holds only grid_conc files "
                                      f"({len(grid_files)}); particle-space oracle "
                                      "comparison needs a partposit dump, and seed "
                                      "files carry no concentration grid, so this "
                                      "run is classified candidate-only"})
            notes.append("Oracle grid_conc files are present but not evaluated: "
                         "without a partposit dump there is no particle-space "
                         "counterpart, and gridding the candidate would duplicate "
                         "the corpus comparison script's scope.")
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
    context = {"case_id": "corpus-seeds",
               "particle_metrics.mass_budget.relative_error": worst_abs_rel_error,
               "integrity.non_negative_fields": True,
               "alignment.grid_and_window_match": None}
    if deposition_on:
        context["particle_metrics.mass_budget.relative_error"] = None
    evaluations = report_lib.evaluate_thresholds(thresholds_doc, context)
    divergent = [e["seed_file"] for e in per_seed
                 if e["runner_metrics_cross_check"]["verdict"] == "DIVERGENT"]
    failed_gates = [e["id"] for e in evaluations if e.get("verdict") == "FAIL"]
    if divergent:
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
            overall_status="INTEGRITY_ERROR",
            status_detail=("runner cross-check DIVERGENT for seeds "
                           f"{divergent}; the two implementations disagree, so no "
                           "reported metric can be trusted"))
    if failed_gates:
        overall, detail = ("FAIL", "in-scope threshold violated: " +
                           ", ".join(failed_gates))
    else:
        overall, detail = ("DIAGNOSTIC", "corpus particle-ensemble diagnostics over "
                           f"{len(seeds)} seeds; no parity verdict")
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
        overall_status=overall,
        status_detail=detail)


# Unit-aware tolerances for the runner cross-check. Absolute tolerances are
# in the stated units; relative entries use max(rel * |reference|, floor).
# Values assume identical formulas in f64; summation-order slack dominates.
CROSS_CHECK_TOLERANCES = {
    "com_lon_deg": {"abs": 1e-9, "unit": "deg"},
    "com_lat_deg": {"abs": 1e-9, "unit": "deg"},
    "com_z_m": {"abs": 1e-6, "unit": "m"},
    "sigma_east_m": {"abs": 1e-6, "unit": "m"},
    "sigma_north_m": {"abs": 1e-6, "unit": "m"},
    "eigenvalue_small_m2": {"rel": 1e-9, "abs_floor": 1e-3, "unit": "m2"},
    "eigenvalue_large_m2": {"rel": 1e-9, "abs_floor": 1e-3, "unit": "m2"},
    "z_p10_m": {"abs": 1e-9, "unit": "m"},
    "z_p50_m": {"abs": 1e-9, "unit": "m"},
    "z_p90_m": {"abs": 1e-9, "unit": "m"},
    "z_mean_m": {"abs": 1e-9, "unit": "m"},
    "z_std_m": {"abs": 1e-9, "unit": "m"},
    "total_mass_kg": {"rel": 1e-9, "abs_floor": 1e-12, "unit": "kg"},
}


def _within_tolerance(diff, reference, spec):
    if "abs" in spec:
        return diff <= spec["abs"]
    return diff <= max(spec["rel"] * abs(reference), spec["abs_floor"])


def _cross_check_runner_metrics(runner, uw_com, uw_cov, uw_quantiles, uw_mean_z,
                                uw_std_z, uw_total_kg):
    """Cross-check this evaluation against the runner's metrics block.

    The comparison runs on an unweighted basis because the runner's moments
    are unweighted means while the reported scientific metrics are
    mass-weighted; comparing like with like isolates genuine implementation
    divergence from definition differences. Runner covariances/eigenvalues
    are converted from m^2 with 1 km^2 = 1e6 m^2. Returns differences,
    tolerances and a CONSISTENT/DIVERGENT verdict; a divergence is an
    integrity error for the whole report.
    """
    runner_reference = {
        "com_lon_deg": float(runner["com_lon_deg"]),
        "com_lat_deg": float(runner["com_lat_deg"]),
        "com_z_m": float(runner["com_z_m"]),
        "sigma_east_m": math.sqrt(max(float(runner["cov_east_m2"]), 0.0)),
        "sigma_north_m": math.sqrt(max(float(runner["cov_north_m2"]), 0.0)),
        "z_p10_m": float(runner["z_p10_m"]),
        "z_p50_m": float(runner["z_p50_m"]),
        "z_p90_m": float(runner["z_p90_m"]),
        "z_mean_m": float(runner["z_mean_m"]),
        "z_std_m": float(runner["z_std_m"]),
        "total_mass_kg": float(runner["total_mass_kg"]),
    }
    runner_eig = sorted(float(v) for v in runner["horizontal_eigenvalues_m2"])
    mine_eig = sorted(v * 1e6 for v in uw_cov["eigenvalues_km2"])
    runner_reference["eigenvalue_small_m2"] = runner_eig[0]
    runner_reference["eigenvalue_large_m2"] = runner_eig[1]
    mine = {
        "com_lon_deg": uw_com["lon_deg"],
        "com_lat_deg": uw_com["lat_deg"],
        "com_z_m": uw_com["z_m"],
        "sigma_east_m": uw_cov["sigma_east_km"] * 1000.0,
        "sigma_north_m": uw_cov["sigma_north_km"] * 1000.0,
        "eigenvalue_small_m2": mine_eig[0],
        "eigenvalue_large_m2": mine_eig[1],
        "z_p10_m": uw_quantiles["quantiles"]["0.1"],
        "z_p50_m": uw_quantiles["quantiles"]["0.5"],
        "z_p90_m": uw_quantiles["quantiles"]["0.9"],
        "z_mean_m": uw_mean_z,
        "z_std_m": uw_std_z,
        "total_mass_kg": uw_total_kg,
    }
    differences = {}
    violations = []
    for name, spec in CROSS_CHECK_TOLERANCES.items():
        diff = abs(runner_reference[name] - mine[name])
        differences[name] = diff
        if not _within_tolerance(diff, runner_reference[name], spec):
            violations.append({"metric": name,
                               "runner": runner_reference[name],
                               "evaluation": mine[name],
                               "abs_difference": diff,
                               "tolerance": spec,
                               "unit": spec["unit"]})
    return {"differences": differences,
            "tolerances": CROSS_CHECK_TOLERANCES,
            "violations": violations,
            "verdict": "DIVERGENT" if violations else "CONSISTENT"}


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
    parser.add_argument("--run-manifest", default=None,
                        help="Oracle run manifest for hash-verified provenance")
    parser.add_argument("--input-equivalence-report", default=None,
                        help="ETEX input-equivalence audit report to consume")
    parser.add_argument("--oracle-seed", type=int, default=None)
    parser.add_argument("--oracle-seed-controllable", action="store_true")
    parser.add_argument("--candidate-log", default=None)
    parser.add_argument("--candidate-executable", default=None)
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
        if args.case == "synthetic-uniform-wind":
            if not args.fortran_output or not args.gpu_output:
                parser.error("synthetic-uniform-wind needs --fortran-output and --gpu-output")
            built = run_synthetic(args, oracle_manifest)
        elif args.case in ("etex-mini", "etex-full"):
            if not args.measurements or not args.fortran_output or not args.gpu_output:
                parser.error(f"{args.case} needs --measurements, --fortran-output and --gpu-output")
            built = run_etex(args, args.case, oracle_manifest)
        elif args.case == "aggregate-seeds":
            if not args.seed_reports:
                parser.error("aggregate-seeds needs --seed-reports")
            built = run_aggregate(args, oracle_manifest)
        elif args.case == "corpus-seeds":
            if not args.candidate_seeds or not args.case_def:
                parser.error("corpus-seeds needs --candidate-seeds and --case-def")
            built = run_corpus_seeds(args, oracle_manifest)
        else:
            parser.error(f"unknown case: {args.case}")
    except (FileNotFoundError, ValueError) as exc:
        try:
            oracle_manifest = load_oracle_manifest(args.oracle_manifest)
        except (FileNotFoundError, ValueError):
            print(f"INTEGRITY_ERROR: {exc}", file=sys.stderr)
            sys.exit(2)
        try:
            oracle, candidate, _missing, _notes = build_provenance(args, oracle_manifest)
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
    if built["overall_status"] == "INTEGRITY_ERROR":
        sys.exit(2)


if __name__ == "__main__":
    main()
