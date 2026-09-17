#!/usr/bin/env python3
"""Compare corpus candidate outputs and, when present, Fortran oracle outputs.

Reads per-seed candidate JSON from target/corpus/candidate/<CASE>/seed_*.json
(written by src/bin/corpus-run.rs) and optional oracle particle dumps from
target/corpus/oracle/<CASE>/particles.json (written by scripts/run-corpus.sh
oracle step via partposit parsing or grid_conc resampling).

Computes machine-readable metrics per implemented case:
  mass conservation, center of mass, covariance/eigenvalues, vertical
  quantiles, gridded overlap, field correlation and process budgets.

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


def grid_overlap_and_correlation(candidate_seeds, oracle_particles, grid):
    """Shared coarse-grid overlap and correlation (diagnostic).

    Both samples are binned with the same operator: lon/lat cells from the
    case domain and the candidate OUTHEIGHTS levels. Returns None when no
    oracle sample is available.
    """
    if not oracle_particles:
        return None
    nx, ny = grid["nx"], grid["ny"]
    heights = grid["heights_m"]
    nz = len(heights)

    def bin_index(lon, lat, z):
        ix = int((lon - grid["xlon0"]) / grid["dx"])
        iy = int((lat - grid["ylat0"]) / grid["dy"])
        ix = min(max(ix, 0), nx - 1)
        iy = min(max(iy, 0), ny - 1)
        iz = nz - 1
        for k, h in enumerate(heights):
            if z <= h:
                iz = k
                break
        return (ix * ny + iy) * nz + iz

    cells = nx * ny * nz
    oracle_counts = [0] * cells
    for p in oracle_particles:
        oracle_counts[bin_index(p["lon_deg"], p["lat_deg"], p["z_m"])] += 1
    oracle_total = sum(oracle_counts) or 1
    oracle_frac = [c / oracle_total for c in oracle_counts]

    per_seed = []
    for seed in candidate_seeds:
        counts = [0] * cells
        for p in seed["particles"]:
            counts[bin_index(p["lon_deg"], p["lat_deg"], p["z_m"])] += 1
        total = sum(counts) or 1
        frac = [c / total for c in counts]
        overlap = sum(min(a, b) for a, b in zip(frac, oracle_frac))
        mean_a = sum(frac) / cells
        mean_b = sum(oracle_frac) / cells
        cov = sum((a - mean_a) * (b - mean_b) for a, b in zip(frac, oracle_frac)) / cells
        var_a = sum((a - mean_a) ** 2 for a in frac) / cells
        var_b = sum((b - mean_b) ** 2 for b in oracle_frac) / cells
        corr = cov / math.sqrt(var_a * var_b) if var_a > 0 and var_b > 0 else 0.0
        per_seed.append({"overlap": overlap, "field_correlation": corr})
    overlap_mean = sum(s["overlap"] for s in per_seed) / len(per_seed)
    corr_mean = sum(s["field_correlation"] for s in per_seed) / len(per_seed)
    return {
        "grid": grid,
        "per_seed": per_seed,
        "overlap_mean": overlap_mean,
        "field_correlation_mean": corr_mean,
        "note": "Diagnostic only; not a parity gate.",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-dir", required=True)
    parser.add_argument("--oracle-dir", required=False, default=None)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    candidate_dir = Path(args.candidate_dir)
    oracle_dir = Path(args.oracle_dir) if args.oracle_dir else None
    corpus_index = json.loads(
        (candidate_dir.parents[2] / "fixtures" / "corpus" / "corpus.json").read_text(
            encoding="utf-8"
        )
        if (candidate_dir.parents[2] / "fixtures" / "corpus" / "corpus.json").exists()
        else Path("fixtures/corpus/corpus.json").read_text(encoding="utf-8")
    )

    report = {
        "status": "DIAGNOSTIC_NO_PARITY_VERDICT",
        "input_audit": "INPUT_EQUIVALENCE_NOT_DEMONSTRATED remains in force for ETEX mini; "
        "no green corpus metric overrides it.",
        "cases": {},
    }
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
        # Process budget for deposition cases.
        if case_id in ("DRY-007", "WET-008"):
            initial = described[0]["total_mass_kg"]  # placeholder; runner records per-seed
            entry["process_budget"] = {
                "note": "Airborne mass per seed is reported; deposited = initial - airborne. "
                "Isolated analytic checks live in tests/integration/corpus.rs and "
                "tests/integration/deposition_decay.rs within versioned tolerances.",
                "initial_mass_kg": seeds[0]["metrics"]["initial_mass_kg"],
            }
        # Oracle pairing when available.
        oracle_particles = None
        if oracle_dir is not None:
            oracle_file = oracle_dir / case_id / "particles.json"
            if oracle_file.is_file():
                oracle_particles = json.loads(oracle_file.read_text(encoding="utf-8"))["particles"]
                entry["oracle_files"] = {str(oracle_file.resolve()): sha256(oracle_file)}
            else:
                entry["oracle"] = "oracle_output_missing (run scripts/run-corpus.sh oracle)"
        if oracle_particles is not None:
            grid = {
                "nx": 32,
                "ny": 32,
                "dx": 0.1,
                "dy": 0.1,
                "xlon0": 9.5,
                "ylat0": 8.5,
                "heights_m": [100.0, 250.0, 500.0, 750.0, 1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 5000.0],
            }
            if case_id == "ADV-ANA-001":
                grid = {
                    "nx": 64,
                    "ny": 64,
                    "dx": 0.1,
                    "dy": 0.1,
                    "xlon0": 6.0,
                    "ylat0": 47.0,
                    "heights_m": [100.0, 500.0, 1000.0, 2000.0, 5000.0],
                }
            comparison = grid_overlap_and_correlation(seeds, oracle_particles, grid)
            if comparison is not None:
                entry["oracle_comparison"] = comparison
        report["cases"][case_id] = entry

    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Corpus comparison report: {output}")


if __name__ == "__main__":
    main()
