#!/usr/bin/env python3
"""Audit required #112 production-driver coverage and content-bound evidence."""
import hashlib
import itertools
import json
import math
from pathlib import Path
import subprocess


def audit_status(words, capacity, active_count, successful):
    """Reject incomplete/invalid lane ABI and inactive-tail writes in retained status."""
    assert len(words) == 6 * (capacity + 1) + 1
    assert type(words[-1]) is int and words[-1] == 0
    for stage in range(6):
        region = words[stage * (capacity + 1):(stage + 1) * (capacity + 1)]
        fatal, lanes = region[0], region[1:]
        assert type(fatal) is int and fatal in (0, 1)
        assert all(type(reason) is int and reason in (0, 2, 3, 4, 5, 6, 7, 8, 9) for reason in lanes)
        assert all(reason == 0 for reason in lanes[active_count:])
        assert bool(fatal) == any(reason >= 3 for reason in lanes)
        if successful:
            assert fatal == 0 and lanes[:active_count] == [2] * active_count


def audit(root=Path("target/ci-gate/resident-advection-production")):
    report = json.loads((root / "report.json").read_text(encoding="utf-8"))
    assert report["schema"] == "flexpart-gpu.resident-advection-production.v1"
    assert report["status"] == "passed"
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    assert report["candidate_revision"] == revision
    assert report["scientific_verdict"] == "NO_FULL_FLEXPART_PARITY_CLAIM"
    shaders = {"advection_predictor_query", "advection_corrector", "advection_step_guard", "advection_step_commit",
               "particle_query", "resident_status_reset", "resident_query_adapter", "horizontal_interpolation",
               "vertical_sample", "temporal_interpolation", "resident_sample_status"}
    assert set(report["shader_sha256"]) == shaders
    for shader, digest in report["shader_sha256"].items():
        assert hashlib.sha256(Path(f"src/shaders/{shader}.wgsl").read_bytes()).hexdigest() == digest
    expected = set(itertools.product((False, True), (False, True), (1, 3, 130), (None, (0, 0), (1, 1), (2, 1))))
    observed = set()
    for row in report["rows"]:
        failure = tuple(row["failure"]) if row["failure"] is not None else None
        identity = (row["backward"], row["varying"], row["active_count"], failure)
        assert identity not in observed
        observed.add(identity)
        assert row["capacity"] == 130 and row["owning_submissions"] == 1 and row["query_roundtrip"] is False
        assert row["adapter"]["adapter_class"] in ("hardware_gpu", "software_wgsl")
        assert row["adapter"]["name"] and row["adapter"]["backend"]
        input_bytes = (root / row["input_file"]).read_bytes()
        assert hashlib.sha256(input_bytes).hexdigest() == row["input_sha256"]
        inputs = json.loads(input_bytes)
        assert [hashlib.sha256(s.encode()).hexdigest() for s in inputs["snapshot_json"]] == row["source_snapshot_sha256"]
        assert len(inputs["runtimes"]) == len(inputs["native_motion_json"]) == 2
        audit_status(row["status"], 130, row["active_count"], failure is None)
        assert len(row["sampled_values"]) == 6 * 130 and len(row["queries"]) == 2 * 130
        assert [m["field_id"] for m in row["metadata"]] == ["wind_u", "wind_v", "vertical_velocity"] * 2
        assert all(m["active_count"] == row["active_count"] and m["geometry_identity"] and m["geometry_provenance"] for m in row["metadata"])
        for stage in range(6):
            for lane in range(row["active_count"]):
                if row["status"][stage * 131 + 1 + lane] == 2:
                    value = row["sampled_values"][stage * 130 + lane]
                    assert type(value) in (int, float) and math.isfinite(value)
        if failure is None:
            assert row["metadata"][0]["time"] != row["metadata"][3]["time"]
            for lane in range(row["active_count"]):
                current, predicted = row["queries"][lane], row["queries"][130 + lane]
                assert current["cell_x"] != predicted["cell_x"] or current["fraction_x"] != predicted["fraction_x"]
        else:
            component, stage = failure
            region = (stage * 3 + component) * 131
            assert row["status"][region] == 1 and row["atomic_preservation"] is True
            assert row["status"][region + 1] >= 3
            assert row["mixed_lane_failure"] == (row["active_count"] > 1)
            if row["active_count"] > 1:
                assert row["status"][region + 2:region + 1 + row["active_count"]] == [2] * (row["active_count"] - 1)
    assert observed == expected
    supplemental = json.loads((root / "interior-deferred-boundary.json").read_text(encoding="utf-8"))
    assert supplemental["candidate_revision"] == revision
    variants = {r["variant"]: r for r in supplemental["rows"]}
    assert set(variants) == {"interior", "deferred-fatal", "changing-top"}
    for row in variants.values():
        assert row["adapter"]["adapter_class"] in ("hardware_gpu", "software_wgsl")
        assert hashlib.sha256((root / row["input_file"]).read_bytes()).hexdigest() == row["input_sha256"]
    interior = variants["interior"]
    assert interior["metadata"][0]["time"]["instantaneous"]["epoch_seconds"] == 1704067201
    assert interior["metadata"][3]["time"]["instantaneous"]["epoch_seconds"] == 1704067202
    audit_status(interior["status"], 3, 1, True)
    audit_status(variants["deferred-fatal"]["status"], 3, 1, False)
    assert variants["deferred-fatal"]["status"][0] != 0
    assert variants["deferred-fatal"]["atomic_preservation"] is True
    assert variants["changing-top"]["rejected"] is True
    print("RESIDENT-ADVECTION-112 evidence audit passed: 48 required real-driver cases")


if __name__ == "__main__":
    audit()
