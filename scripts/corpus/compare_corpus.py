#!/usr/bin/env python3
"""Compare corpus candidate outputs and, when present, Fortran oracle outputs.

Reads per-seed candidate JSON from target/corpus/candidate/<CASE>/seed_*.json
(written by src/bin/corpus-run.rs, including per-seed deposited-mass
reservoirs) and optional oracle summaries from
target/corpus/oracle/<CASE>/oracle_summary.json (written by
scripts/corpus/decode_oracle_output.py from the preserved raw
``header``/``grid_conc_*`` oracle outputs).

FLEXPART 11.1 writes binary concentration output only; per-particle
partposit dumps are NetCDF-only in v11.1, so the oracle side is compared on
the shared output grid: candidate end-state particles are binned with the
oracle OUTGRID operator and correlated against the decoded oracle
concentration field. Oracle budget reservoirs (airborne, dry-deposited,
wet-deposited grid sums) come from the same decoded slices.

Computes machine-readable metrics per implemented case:
  mass conservation, center of mass, covariance/eigenvalues, vertical
  quantiles, gridded overlap, field correlation and process budgets.
Budget closure is reported per seed (candidate) and per run (oracle) and is
only marked closed when every reservoir is present; closure never implies
cross-model parity.

Oracle comparisons are diagnostic only. No threshold in
fixtures/corpus/thresholds.json turns them into a parity pass, and this
script never overrides INPUT_EQUIVALENCE_NOT_DEMONSTRATED.

Usage:
    python3 scripts/corpus/compare_corpus.py \
        --candidate-dir target/corpus/candidate \
        --oracle-dir target/corpus/oracle \
        --output target/corpus/comparison_report.json
"""

import argparse
import hashlib
import json
import math
from pathlib import Path


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def load_seeds(case_dir: Path):
    files = sorted(case_dir.glob("seed_*.json"))
    seeds = []
    for path in files:
        seeds.append(json.loads(path.read_text(encoding="utf-8")))
    return seeds


def particle_arrays(seed):
    lons = [p["lon_deg"] for p in seed["particles"]]
    lats = [p["lat_deg"] for p in seed["particles"]]
    zs = [p["z_m"] for p in seed["particles"]]
    masses = [p["mass_kg"] for p in seed["particles"]]
    return lons, lats, zs, masses


def describe_seed(seed):
    metrics = seed["metrics"]
    return {
        "seed_index": seed["seed_index"],
        "philox_key": seed["philox_key"],
        "active_particles": seed["active_particles"],
        "total_mass_kg": metrics["total_mass_kg"],
        "mass_conservation_rel_error": metrics["mass_conservation_rel_error"],
        "com_lon_deg": metrics["com_lon_deg"],
        "com_lat_deg": metrics["com_lat_deg"],
        "com_z_m": metrics["com_z_m"],
        "cov_east_m2": metrics["cov_east_m2"],
        "cov_north_m2": metrics["cov_north_m2"],
        "cov_z_m2": metrics["cov_z_m2"],
        "cov_east_north_m2": metrics["cov_east_north_m2"],
        "cov_east_z_m2": metrics["cov_east_z_m2"],
        "cov_north_z_m2": metrics["cov_north_z_m2"],
        "horizontal_eigenvalues_m2": metrics["horizontal_eigenvalues_m2"],
        "z_min_m": metrics["z_min_m"],
        "z_p10_m": metrics["z_p10_m"],
        "z_p50_m": metrics["z_p50_m"],
        "z_p90_m": metrics["z_p90_m"],
        "z_max_m": metrics["z_max_m"],
        "z_mean_m": metrics["z_mean_m"],
        "z_std_m": metrics["z_std_m"],
    }


def ensemble_stats(described):
    # Mean and std across the 10 candidate Philox seeds.
    keys = [
        "total_mass_kg",
        "com_lon_deg",
        "com_lat_deg",
        "com_z_m",
        "cov_east_m2",
        "cov_north_m2",
        "cov_z_m2",
        "z_p10_m",
        "z_p50_m",
        "z_p90_m",
        "z_mean_m",
        "z_std_m",
    ]
    stats = {}
    for key in keys:
        values = [d[key] for d in described]
        mean = sum(values) / len(values)
        var = sum((v - mean) ** 2 for v in values) / len(values)
        stats[key] = {"mean": mean, "std": math.sqrt(var), "n": len(values)}
    return stats


def bin_index(lon, lat, z, grid):
    # Shared OUTGRID binning operator. The vertical rule mirrors the oracle:
    # FLEXPART assigns the first level with outheight strictly greater than
    # the particle height (output_mod.f90:725 `if (outheight(kz).gt.z)`,
    # same rule in outgrid_mod.f90:625), so boundary heights bin upward.
    nx, ny = grid["nx"], grid["ny"]
    heights = grid["heights_m"]
    nz = len(heights)
    ix = int((lon - grid["xlon0"]) / grid["dx"])
    iy = int((lat - grid["ylat0"]) / grid["dy"])
    ix = min(max(ix, 0), nx - 1)
    iy = min(max(iy, 0), ny - 1)
    iz = nz - 1
    for k, h in enumerate(heights):
        if h > z:
            iz = k
            break
    return (ix * ny + iy) * nz + iz


def grid_overlap_and_correlation(candidate_seeds, oracle_frac, grid):
    """Candidate particles vs decoded oracle concentration on one grid.

    Candidate end-state particle masses are binned with the oracle OUTGRID
    operator (mass-weighted: deposited particles carry less mass); the
    oracle side is the decoded mass-proportional concentration field. Both
    sides are normalized to fractions before overlap and Pearson
    correlation. End-state snapshot vs time average is a known window
    mismatch, so this stays diagnostic; it never gates parity.
    """
    cells = grid["nx"] * grid["ny"] * len(grid["heights_m"])
    if not oracle_frac or len(oracle_frac) != cells:
        return None
    oracle_total = sum(oracle_frac)
    if oracle_total <= 0:
        return None
    oracle_norm = [c / oracle_total for c in oracle_frac]

    per_seed = []
    for seed in candidate_seeds:
        mass = [0.0] * cells
        for p in seed["particles"]:
            mass[bin_index(p["lon_deg"], p["lat_deg"], p["z_m"], grid)] += p["mass_kg"]
        total = sum(mass) or 1.0
        frac = [c / total for c in mass]
        overlap = sum(min(a, b) for a, b in zip(frac, oracle_norm))
        mean_a = sum(frac) / cells
        mean_b = sum(oracle_norm) / cells
        cov = sum((a - mean_a) * (b - mean_b) for a, b in zip(frac, oracle_norm)) / cells
        var_a = sum((a - mean_a) ** 2 for a in frac) / cells
        var_b = sum((b - mean_b) ** 2 for b in oracle_norm) / cells
        corr = cov / math.sqrt(var_a * var_b) if var_a > 0 and var_b > 0 else 0.0
        per_seed.append({"overlap": overlap, "field_correlation": corr})
    overlap_mean = sum(s["overlap"] for s in per_seed) / len(per_seed)
    corr_mean = sum(s["field_correlation"] for s in per_seed) / len(per_seed)
    return {
        "grid": grid,
        "per_seed": per_seed,
        "overlap_mean": overlap_mean,
        "field_correlation_mean": corr_mean,
        "window_note": "Candidate end-state snapshot vs oracle time-averaged "
        "concentration window; diagnostic only, not a parity gate.",
    }


def load_oracle_summaries(oracle_dir):
    """Load all decoded oracle summaries keyed by case id."""
    summaries = {}
    oracle_dir = Path(oracle_dir)
    if not oracle_dir.is_dir():
        return summaries
    for summary_file in sorted(oracle_dir.glob("*/oracle_summary.json")):
        try:
            summaries[summary_file.parent.name] = json.loads(summary_file.read_text(encoding="utf-8"))
        except (ValueError, OSError):
            continue
    return summaries


def same_grid(header_a, header_b):
    return (
        header_a["numxgrid"] == header_b["numxgrid"]
        and header_a["numygrid"] == header_b["numygrid"]
        and header_a["numzgrid"] == header_b["numzgrid"]
        and abs(header_a["dxout"] - header_b["dxout"]) < 1e-9
        and abs(header_a["dyout"] - header_b["dyout"]) < 1e-9
    )


def calibrate_oracle_scale(summaries, target_header, closure_tolerance):
    """Calibrate mass-proportional units per released gram from an inert run.

    Raw grid sums are not mass-conserving across windows when the plume
    redistributes vertically, so no run-internal scale is used. An inert
    tracer run (zero dry/wet reservoirs in both slices) on the identical
    OUTGRID instead gives a conserved scale: its volume/latitude-weighted
    airborne reservoirs coincide across windows (self-consistency gate) and
    their mean per released gram calibrates every deposition case sharing
    the grid. Returns (scale, source_case, self_consistency_error) or
    (None, reason, None).
    """
    for case_id, summary in summaries.items():
        header = summary["header"]
        if not same_grid(header, target_header):
            continue
        first_w = summary["first_slice"]["weighted"]
        last_w = summary["last_slice"]["weighted"]
        if (
            first_w["dry_deposited"] != 0.0
            or first_w["wet_deposited"] != 0.0
            or last_w["dry_deposited"] != 0.0
            or last_w["wet_deposited"] != 0.0
        ):
            continue  # not inert; cannot calibrate
        if first_w["airborne"] <= 0:
            continue
        consistency = abs(last_w["airborne"] - first_w["airborne"]) / first_w["airborne"]
        if consistency > closure_tolerance:
            continue
        scale = (first_w["airborne"] + last_w["airborne"]) / 2.0 / summary["release_mass_g"]
        return scale, case_id, consistency
    return None, "no inert same-grid oracle run with closed self-consistency", None


def candidate_process_budget(seeds, closure_tolerance):
    """Per-seed airborne/deposited reservoirs with gated closure.

    Reservoirs are recorded separately by the runner from driver-reported
    per-slot removal probabilities (dry step before wet step), never inferred
    as initial-minus-airborne. Closure is marked closed per seed only when
    all three reservoirs are present and recover the initial mass.
    """
    per_seed = []
    for seed in seeds:
        metrics = seed["metrics"]
        airborne = metrics["total_mass_kg"]
        initial = metrics["initial_mass_kg"]
        dry = seed.get("deposited_dry_kg")
        wet = seed.get("deposited_wet_kg")
        closure = seed.get("budget_closure_rel_error")
        status = seed.get("budget_status", "open")
        complete = dry is not None and wet is not None and closure is not None
        if not complete:
            status = "open (reservoirs missing)"
        per_seed.append(
            {
                "seed_index": seed["seed_index"],
                "initial_mass_kg": initial,
                "airborne_kg": airborne,
                "dry_deposited_kg": dry,
                "wet_deposited_kg": wet,
                "closure_rel_error": closure,
                "status": status if complete else "open (reservoirs missing)",
            }
        )
    closed = [s for s in per_seed if s["status"] == "closed"]
    return {
        "per_seed": per_seed,
        "closure_tolerance_rel": closure_tolerance,
        "closed_seeds": len(closed),
        "total_seeds": len(per_seed),
        "note": "Dry removal is charged before wet removal each step, matching "
        "driver dispatch order. Closure never implies cross-model parity.",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-dir", required=True)
    parser.add_argument("--oracle-dir", required=False, default=None)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    candidate_dir = Path(args.candidate_dir)
    oracle_dir = Path(args.oracle_dir) if args.oracle_dir else None
    repo_root = Path(__file__).resolve().parents[2]
    corpus_index = json.loads((repo_root / "fixtures" / "corpus" / "corpus.json").read_text(encoding="utf-8"))
    thresholds = json.loads((repo_root / "fixtures" / "corpus" / "thresholds.json").read_text(encoding="utf-8"))
    closure_tolerance = thresholds["thresholds"]["budget_closure_rel"]["value"]

    report = {
        "status": "DIAGNOSTIC_NO_PARITY_VERDICT",
        "input_audit": "INPUT_EQUIVALENCE_NOT_DEMONSTRATED remains in force for ETEX mini; "
        "no green corpus metric overrides it.",
        "cases": {},
    }
    oracle_summaries = load_oracle_summaries(oracle_dir) if oracle_dir is not None else {}
    for case in corpus_index["cases"]:
        case_id = case["id"]
        if case["status"] != "implemented":
            report["cases"][case_id] = {
                "status": "blocked",
                "blocked_by": case.get("blocked_by", ""),
            }
            continue
        if case_id == "ETEX-MINI-013":
            report["cases"][case_id] = {
                "status": "implemented",
                "note": "Covered by scripts/run-etex.sh mini; see target/etex/mini/.",
            }
            continue
        case_dir = candidate_dir / case_id
        if not case_dir.is_dir():
            report["cases"][case_id] = {"status": "missing_candidate_output"}
            continue
        seeds = load_seeds(case_dir)
        if not seeds:
            report["cases"][case_id] = {"status": "missing_candidate_output"}
            continue
        described = [describe_seed(s) for s in seeds]
        entry = {
            "status": "implemented",
            "seeds": described,
            "ensemble": ensemble_stats(described),
            "candidate_files": {
                str(p.resolve()): sha256(p) for p in sorted(case_dir.glob("seed_*.json"))
            },
        }
        # Process budget with separately recorded reservoirs (all cases;
        # deposition cases exercise non-zero removal paths).
        entry["process_budget"] = candidate_process_budget(seeds, closure_tolerance)
        # Oracle pairing when a decoded summary is available.
        oracle_summary = oracle_summaries.get(case_id)
        if oracle_summary is not None:
            summary_file = oracle_dir / case_id / "oracle_summary.json"
            entry["oracle_files"] = {str(summary_file.resolve()): sha256(summary_file)}
            for raw_name, raw_hash in oracle_summary.get("raw_sha256", {}).items():
                entry["oracle_files"][f"<raw>/{raw_name}"] = raw_hash
        elif oracle_dir is not None:
            entry["oracle"] = "oracle_output_missing (run scripts/run-corpus.sh oracle)"
        if oracle_summary is not None:
            header = oracle_summary["header"]
            grid = {
                "nx": header["numxgrid"],
                "ny": header["numygrid"],
                "dx": header["dxout"],
                "dy": header["dyout"],
                "xlon0": header["outlon0"],
                "ylat0": header["outlat0"],
                "heights_m": header["outheights"],
            }
            last_w = oracle_summary["last_slice"]["weighted"]
            # Primary oracle closure: mass-proportional reservoirs against
            # the calibrated scale from an inert same-grid run. Raw grid sums
            # are never closed directly: they mix window-averaged
            # concentration with cumulative deposition and drift up to ~20%
            # across windows with zero removal (WIND-UNI-002).
            scale, calib_from, calib_consistency = calibrate_oracle_scale(
                oracle_summaries, oracle_summary["header"], closure_tolerance
            )
            if scale is not None:
                expected = scale * oracle_summary["release_mass_g"]
                total = last_w["airborne"] + last_w["dry_deposited"] + last_w["wet_deposited"]
                cal_closure = abs(total - expected) / expected
                oracle_status = (
                    "closed"
                    if cal_closure <= closure_tolerance
                    else "open (closure exceeds versioned bound)"
                )
            else:
                cal_closure = None
                oracle_status = f"open ({calib_from})"
            deposited = last_w["dry_deposited"] + last_w["wet_deposited"]
            entry["oracle_budget"] = {
                "release_mass_g": oracle_summary["release_mass_g"],
                "reservoirs_weighted": last_w,
                "calibration_from": calib_from,
                "scale_weighted_per_g": scale,
                "calibration_self_consistency_rel_error": calib_consistency,
                "calibrated_closure_rel_error": cal_closure,
                "deposited_split_of_deposited": {
                    "dry": (last_w["dry_deposited"] / deposited) if deposited > 0 else None,
                    "wet": (last_w["wet_deposited"] / deposited) if deposited > 0 else None,
                },
                "status": oracle_status,
                "closure_tolerance_rel": closure_tolerance,
                "note": "Oracle reservoirs are mass-proportional weighted sums "
                "(concentration by layer thickness and cos(latitude), "
                "deposition by cos(latitude)) sharing one run scale; "
                "candidate budgets close in kilograms. Deposited-split "
                "fractions are exact; absolute masses are not converted "
                "across models.",
            }
            # Candidate end-state particle masses binned to the oracle
            # OUTGRID vs the decoded mass-proportional oracle field.
            comparison = grid_overlap_and_correlation(
                seeds, oracle_summary.get("mass_proportional_fractions"), grid
            )
            if comparison is not None:
                comparison["oracle_center_of_mass"] = oracle_summary.get("center_of_mass")
                comparison["oracle_vertical_profile_native"] = oracle_summary.get("vertical_profile_native")
                entry["oracle_comparison"] = comparison
        report["cases"][case_id] = entry

    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Corpus comparison report: {output}")


if __name__ == "__main__":
    main()
