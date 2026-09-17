"""Versioned evaluation report schema and alignment checks.

Schema version ``1.0.0``. Every report carries units, the evaluated time
window, the spatial grid, oracle and candidate revisions, the adapter, seeds,
input hashes and an explicit list of missing metrics. Reports are comparable
across runs because field names, units and missing-data rules are fixed here.

Overall status values:

- ``INTEGRITY_ERROR``: technical failure (missing artifacts, unpinned
  oracle, grid/window mismatch, negative or non-finite fields). No
  scientific statement is made.
- ``DIAGNOSTIC``: metrics were computed but no parity verdict is given
  (ETEX mini, single-seed stochastic runs, normalized shape-only views).
- ``PASS`` / ``FAIL``: threshold evaluation for in-scope criteria.
"""

import hashlib
import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path

SCHEMA_VERSION = "1.0.0"
GRID_ABS_TOL = 1e-5


def utc_now_iso():
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def hash_inputs(paths):
    """Hash input files; missing paths raise FileNotFoundError."""
    result = {}
    for label, path in paths:
        file_path = Path(path)
        if not file_path.is_file():
            raise FileNotFoundError(f"missing input {label}: {path}")
        result[label] = sha256_file(file_path)
    return result


def git_revision(checkout_dir):
    """Return (commit, dirty) for a git checkout, or (None, None)."""
    try:
        commit = subprocess.run(
            ["git", "-C", str(checkout_dir), "rev-parse", "HEAD"],
            check=True, capture_output=True, text=True).stdout.strip()
        status = subprocess.run(
            ["git", "-C", str(checkout_dir), "status", "--porcelain"],
            check=True, capture_output=True, text=True).stdout.strip()
        return commit, bool(status)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None, None


def read_adapter_from_log(log_path):
    """Extract the single unambiguous wgpu adapter line, or None."""
    if log_path is None:
        return None
    with open(log_path, encoding="utf-8", errors="replace") as stream:
        matches = [line.strip() for line in stream
                   if "wgpu adapter:" in line or
                   "wgpu adapter (software fallback requested):" in line]
    if len(matches) != 1:
        raise ValueError(f"candidate log lacks one unambiguous adapter record: {log_path}")
    return matches[0]


def check_grid_alignment(oracle_grid, candidate_grid):
    """Compare grid definitions; returns (match, checks list)."""
    checks = []
    keys = (("nx", "nx"), ("ny", "ny"), ("nz", "nz"),
            ("xlon0", "outlon0"), ("ylat0", "outlat0"),
            ("dx", "dx"), ("dy", "dy"))
    match = True
    for cand_key, oracle_key in keys:
        oracle_value = float(oracle_grid[oracle_key])
        candidate_value = float(candidate_grid[cand_key])
        ok = abs(oracle_value - candidate_value) <= GRID_ABS_TOL
        checks.append({"check": f"grid.{cand_key}", "oracle": oracle_value,
                       "candidate": candidate_value, "match": bool(ok)})
        match = match and ok
    oracle_heights = [float(v) for v in oracle_grid["outheights"]]
    candidate_heights = [float(v) for v in candidate_grid["heights_m"]]
    if len(oracle_heights) != len(candidate_heights):
        checks.append({"check": "grid.nz_heights_length", "oracle": len(oracle_heights),
                       "candidate": len(candidate_heights), "match": False})
        return False, checks
    heights_ok = all(abs(o - c) <= GRID_ABS_TOL
                     for o, c in zip(oracle_heights, candidate_heights))
    checks.append({"check": "grid.heights_m", "oracle": oracle_heights,
                   "candidate": candidate_heights, "match": bool(heights_ok)})
    return bool(match and heights_ok), checks


def check_window_alignment(oracle_window, candidate_window):
    """Compare averaging windows; returns (match, checks list)."""
    checks = []
    match = True
    for key in ("window_start_epoch_seconds", "window_end_epoch_seconds",
                "averaging_seconds", "sampling_seconds", "samples"):
        ok = oracle_window[key] == candidate_window[key]
        checks.append({"check": f"window.{key}", "oracle": oracle_window[key],
                       "candidate": candidate_window[key], "match": bool(ok)})
        match = match and ok
    for key in ("endpoint_weight",):
        ok = abs(float(oracle_window[key]) - float(candidate_window[key])) <= 1e-9
        checks.append({"check": f"window.{key}", "oracle": float(oracle_window[key]),
                       "candidate": float(candidate_window[key]), "match": bool(ok)})
        match = match and ok
    return bool(match), checks


def evaluate_thresholds(thresholds_doc, context):
    """Evaluate in-scope threshold criteria against a context dict.

    ``context`` maps metric paths to values, e.g.
    ``{"particle_metrics.mass_budget.relative_error": 1e-7}``.
    Returns a list of per-criterion results with PASS/FAIL/SKIP verdicts.
    """
    results = []
    case_id = context.get("case_id", "")
    for criterion in thresholds_doc.get("criteria", []):
        if case_id not in criterion.get("applies_to", []):
            results.append({"id": criterion["id"], "verdict": "SKIP",
                            "reason": f"not in scope for {case_id}"})
            continue
        metric_path = criterion["metric"]
        value = context.get(metric_path, None)
        if value is None:
            results.append({"id": criterion["id"], "metric": metric_path,
                            "verdict": "SKIP", "reason": "metric missing"})
            continue
        operator = criterion["operator"]
        threshold = criterion["value"]
        if operator == "abs_lt":
            passed = abs(float(value)) < float(threshold)
        elif operator == "equals":
            passed = bool(value) == bool(threshold)
        else:
            raise ValueError(f"unknown threshold operator: {operator}")
        results.append({"id": criterion["id"], "metric": metric_path,
                        "value": value, "threshold": threshold,
                        "unit": criterion.get("unit"),
                        "verdict": "PASS" if passed else "FAIL"})
    return results


def make_report(case_id, time_window, grid, oracle, candidate, input_hashes,
                alignment, particle_metrics, grid_metrics, etex,
                multiseed, threshold_evaluations, thresholds_version,
                missing_metrics, notes, overall_status, status_detail):
    """Assemble a schema 1.0.0 report dict."""
    return {
        "schema_version": SCHEMA_VERSION,
        "case_id": case_id,
        "created_utc": utc_now_iso(),
        "overall_status": overall_status,
        "status_detail": status_detail,
        "time_window": time_window,
        "grid": grid,
        "oracle": oracle,
        "candidate": candidate,
        "input_sha256": input_hashes,
        "alignment": alignment,
        "particle_metrics": particle_metrics,
        "grid_metrics": grid_metrics,
        "etex": etex,
        "multiseed": multiseed,
        "thresholds": {
            "version": thresholds_version,
            "evaluations": threshold_evaluations,
        },
        "missing_metrics": missing_metrics,
        "notes": notes,
    }


def write_report_json(report, output_path):
    path = Path(output_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8") as stream:
        json.dump(report, stream, indent=2, allow_nan=False)
        stream.write("\n")
    return path


def render_summary(report):
    """Render a human-readable summary of a report."""
    lines = []
    lines.append(f"Case: {report['case_id']} (schema {report['schema_version']})")
    lines.append(f"Status: {report['overall_status']} -- {report['status_detail']}")
    window = report.get("time_window")
    if window:
        if window.get("averaging_s") is not None:
            lines.append(
                f"Window: {window.get('start_iso')} to {window.get('end_iso')} "
                f"(averaging {window.get('averaging_s')} s, sampling {window.get('sampling_s')} s, "
                f"samples {window.get('samples')})")
        else:
            lines.append(
                f"Window: {window.get('start_iso')} to {window.get('end_iso')} "
                f"(dt {window.get('sampling_s')} s, steps {window.get('steps')})")
    grid = report.get("grid")
    if grid:
        lines.append(
            f"Grid: {grid['nx']}x{grid['ny']}x{grid['nz']} "
            f"origin ({grid['xlon0_deg']}, {grid['ylat0_deg']}) "
            f"dx={grid['dx_deg']} dy={grid['dy_deg']}")
    oracle = report.get("oracle", {})
    candidate = report.get("candidate", {})
    lines.append(f"Oracle: {oracle.get('name')} {oracle.get('version')} "
                 f"commit {oracle.get('pinned_commit')} "
                 f"(attribution: {oracle.get('attribution')})")
    revision = candidate.get("revision")
    source = candidate.get("revision_source")
    if revision is None:
        revision_label = "unknown (not tied to artifacts)"
    elif source:
        revision_label = f"{revision} via {source}"
    else:
        revision_label = str(revision)
    lines.append(f"Candidate: revision {revision_label} "
                 f"dirty={candidate.get('worktree_dirty')} "
                 f"adapter={candidate.get('adapter')} seed={candidate.get('seed')}")
    particle = report.get("particle_metrics")
    if particle and particle.get("mass_budget"):
        budget = particle["mass_budget"]
        lines.append(
            f"Mass budget: initial={budget['initial_mass_kg']:.6e} kg "
            f"remaining={budget['remaining_mass_kg']:.6e} kg "
            f"rel_err={budget['relative_error']}")
    grid_m = report.get("grid_metrics")
    if grid_m:
        if grid_m.get("field_comparison"):
            fc = grid_m["field_comparison"]
            lines.append(
                f"Grid shape (diagnostic only): correlation={fc['correlation']} "
                f"nrmse={fc['normalized_rmse']}")
        if grid_m.get("center_distance_km") is not None:
            lines.append(f"Center distance: {grid_m['center_distance_km']:.3f} km")
    multiseed = report.get("multiseed")
    if multiseed and multiseed.get("mass_budget_gate"):
        gate = multiseed["mass_budget_gate"]
        lines.append(
            f"Mass-closure gate over seeds: {gate.get('gate')} "
            f"(worst abs rel err {gate.get('worst_abs_relative_error')}, "
            f"failed seeds {gate.get('failed_seed_count')})")
    etex = report.get("etex")
    if etex:
        for key in ("fortran_vs_observations", "gpu_vs_observations"):
            block = etex.get(key)
            if block:
                lines.append(
                    f"{key}: n={block['n']} FB={block['fractional_bias']} "
                    f"NMSE={block['nmse']} corr={block['correlation']} "
                    f"FAC2={block['fac2']} ({block['fac2_pairs']} positive pairs)")
    missing = report.get("missing_metrics", [])
    if missing:
        lines.append("Missing metrics:")
        for item in missing:
            lines.append(f"  - {item['name']}: {item['reason']}")
    for evaluation in report.get("thresholds", {}).get("evaluations", []):
        lines.append(f"Threshold {evaluation['id']}: {evaluation['verdict']}")
    for note in report.get("notes", []):
        lines.append(f"Note: {note}")
    return "\n".join(lines) + "\n"
