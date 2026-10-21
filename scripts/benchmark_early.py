#!/usr/bin/env python3
"""Run #199's separately labelled canonical hardware and pristine 11.1 timings.

One immutable candidate revision, fresh result directories, and no speedup verdict.
The existing oracle build cache, input generator and decoder remain their owners.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import re
import shlex
import shutil
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO / "scripts/corpus"))
import generate_fortran_fixtures as fixtures
import oracle_build_cache as oracle

MANIFEST = REPO / "fixtures/benchmarks/early-11.1-v1.json"
PRODUCTION_PATHS = ["src", ":(exclude)src/bin/bench-canonical.rs", "Cargo.toml"]
GPU_ENV = {"FLEXPART_GPU_PROFILE": "1", "FLEXPART_GPU_PBL_CPU": "0",
           "FLEXPART_GPU_VALIDATION": "0", "FLEXPART_GPU_COMPACTION": "0",
           "FLEXPART_GPU_SOFTWARE": "0", "FLEXPART_WG_AUTOTUNE_DISABLE_CACHE": "1"}


def sha(path):
    return oracle.sha256(Path(path))


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def command(args, log, *, env=None, cwd=REPO):
    """Retain exact argv, subprocess status, duration and complete output."""
    start = time.perf_counter()
    record = {"argv": list(map(str, args)), "log": str(log)}
    with Path(log).open("wb") as stream:
        try:
            result = subprocess.run(record["argv"], cwd=cwd, env=env,
                                    stdout=stream, stderr=subprocess.STDOUT, check=False)
            record["returncode"] = result.returncode
        except OSError as exc:
            record.update(returncode=127, error=str(exc))
            stream.write(str(exc).encode())
    record["wall_seconds"] = time.perf_counter() - start
    record["log_sha256"] = sha(log)
    return record


def checked(record):
    if record["returncode"]:
        raise RuntimeError(f"subprocess exited {record['returncode']}: {record['log']}")
    return record


def build_binary(record):
    """Use Cargo's artifact receipt, including configured target directories/triples."""
    checked(record)
    executables = set()
    for line in Path(record["log"]).read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            artifact = json.loads(line)
        except ValueError:
            continue
        if (isinstance(artifact, dict) and artifact.get("reason") == "compiler-artifact"
                and artifact.get("target", {}).get("name") == "bench-canonical"
                and artifact.get("executable")):
            executables.add(Path(artifact["executable"]).resolve())
    if len(executables) != 1 or not next(iter(executables)).is_file():
        raise ValueError("Cargo did not identify one existing bench-canonical executable")
    return executables.pop()


def freeze_manifest(source, output):
    """Bind parsing, provenance and every candidate run to the same source bytes."""
    payload = source.read_bytes()
    manifest = json.loads(payload)
    validate_manifest(manifest)
    output.write_bytes(payload)
    return manifest, sha(output)


def tree_hashes(root):
    return {p.relative_to(root).as_posix(): sha(p) for p in sorted(root.rglob("*")) if p.is_file()}


def distribution(values):
    if not values:
        return None
    if any(not isinstance(v, (int, float)) or not 0 < v < float("inf") for v in values):
        raise ValueError("timing must be finite and positive")
    return {"count": len(values), "mean": statistics.mean(values),
            "median": statistics.median(values), "min": min(values), "max": max(values),
            "stddev": statistics.stdev(values) if len(values) > 1 else None}


def require_candidate(manifest):
    # Host-only dependency: the frozen oracle worker runs Python 3.10.
    import tomllib
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=REPO, text=True).strip():
        raise ValueError("benchmark candidate must be committed and clean")
    subprocess.run(["git", "merge-base", "--is-ancestor", manifest["baseline_commit"], revision],
                   cwd=REPO, check=True)
    changed = subprocess.check_output(["git", "diff", "--name-only", manifest["baseline_commit"],
                                       revision, "--", *PRODUCTION_PATHS], cwd=REPO, text=True)
    if changed.strip():
        raise ValueError("Stage A production sources changed; use a new versioned series: " + changed)
    baseline_lock = subprocess.check_output(["git", "show", manifest["baseline_commit"] + ":Cargo.lock"], cwd=REPO, text=True)
    # Cargo may reorder identical lock entries when normalizing the existing file.
    def normalized_lock(text):
        value = tomllib.loads(text)
        value["package"] = sorted(value["package"], key=lambda p: (p["name"], p["version"]))
        return value
    if normalized_lock(baseline_lock) != normalized_lock((REPO / "Cargo.lock").read_text()):
        raise ValueError("Stage A dependencies changed; use a new versioned series")
    return revision


def validate_manifest(manifest):
    """Keep the finite Stage A campaign from passing an empty or scaled matrix."""
    counts = manifest["particle_counts"]
    if (manifest["schema"] != "flexpart-gpu.early-benchmark.v1"
            or manifest["series"] != "stage-a-pre-197-pre-198"
            or counts != [100000, 1000000] or manifest["repetitions"] < 3
            or not 2 <= manifest["warmup_seconds"] < float("inf")
            or manifest["candidate"]["steps"] <= 0 or manifest["candidate"]["dt_seconds"] <= 0):
        raise ValueError("invalid Stage A benchmark matrix")


def validate_candidate(data, count, manifest_hash, manifest):
    """Reject absent device/identity/output evidence, never infer a hardware pass."""
    scenario = manifest["candidate"]
    steps, dt = scenario["steps"], scenario["dt_seconds"]
    if (data.get("schema") != "flexpart-gpu.canonical-benchmark-run.v1"
            or data.get("regime") != "canonical-production" or data.get("calculation_path") != "wgsl_device"
            or data.get("adapter", {}).get("adapter_class") != "hardware_gpu"
            or data.get("particles") != count or data.get("measured_steps") != steps
            or data.get("manifest_sha256") != manifest_hash
            or data.get("invariants", {}).get("active_particles") != count
            or data.get("invariants", {}).get("no_boundary_contact") is not True
            or not data.get("canonical_identity") or len(data.get("steps", [])) != steps
            or not data.get("particle_output_sha256")):
        raise ValueError("missing/mismatched canonical hardware execution evidence")
    if data.get("configuration") != scenario or data.get("dt_seconds") != dt:
        raise ValueError("candidate configuration differs from frozen manifest")
    warmup_steps = data["warmup_steps"]
    start = data["measured_start_epoch_seconds"]
    if (type(warmup_steps) is not int or warmup_steps < 2
            or not manifest["warmup_seconds"] <= data["warmup_wall_seconds"] < float("inf")
            or start != 1704067200 + warmup_steps * dt
            or data["measured_end_epoch_seconds"] != start + steps * dt):
        raise ValueError("candidate warmup/measurement interval missing or inconsistent")
    invariants = data["invariants"]
    if (invariants.get("finite") is not True
            or invariants.get("mass_kg") != count * scenario["mass_kg_per_particle"]
            or not 0 < invariants["min_height_m"] <= invariants["max_height_m"] < invariants["top_m"] < float("inf")
            or type(data["particle_output_bytes"]) is not int or data["particle_output_bytes"] <= 0):
        raise ValueError("candidate particle invariant/artifact evidence incomplete")
    phase_names = ("surface_interpolation_ms", "pbl_preparation_upload_ms", "forcing_ms",
                   "deposition_upload_ms", "previous_wait_ms", "encode_submit_ms",
                   "submit_to_poll_ms", "reported_total_ms")
    for step in data["steps"]:
        distribution([step["wall_seconds"]])
        if any(not 0 <= step["phases"][key] < float("inf") for key in phase_names):
            raise ValueError("candidate phase timing missing or invalid")
    distribution([data["measured_wall_seconds"], data["particle_steps_per_second"]])


def candidate_run(binary, manifest_path, manifest_hash, manifest, count, folder, environment):
    report = folder / "candidate.json"
    process = command([binary, manifest_path, str(count), report], folder / "candidate.log", env=environment)
    record = {"regime": "canonical-production", "particles": count, "process": process, "state": "FAIL"}
    try:
        checked(process)
        data = json.loads(report.read_text())
        validate_candidate(data, count, manifest_hash, manifest)
        particle_path = report.with_suffix(".particles.bin")
        if (sha(manifest_path) != manifest_hash
                or sha(particle_path) != data["particle_output_sha256"]
                or particle_path.stat().st_size != data["particle_output_bytes"]):
            raise ValueError("frozen manifest or particle artifact identity mismatch")
        record.update(state="PASS", result=data, report_sha256=sha(report))
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as exc:
        record["error"] = str(exc)
    return record


def container_path(path):
    return "/workspace/flexpart-gpu/" + Path(path).resolve().relative_to(REPO).as_posix()


def docker_command(checkout, image, profile):
    args = ["docker", "run", "--rm", "--mount", f"type=bind,src={REPO},dst=/workspace/flexpart-gpu",
            "--mount", f"type=bind,src={checkout},dst=/workspace/flexpart,readonly"]
    for key, value in profile["runtime_environment"].items():
        args += ["-e", f"{key}={value}"]
    return args + [image]


def prepare_fortran(checkout, output, manifest):
    """Delegate build identity and scientific input derivation to existing owners."""
    oracle.require_pristine(checkout, REPO)
    status = output / "oracle-build-status.json"
    oracle.prepare(REPO, checkout, status, False)
    build_status = json.loads(status.read_text())
    build = json.loads(Path(build_status["metadata"]).read_text())
    shutil.copyfile(build_status["metadata"], output / "oracle-build.json")
    shutil.copyfile(build_status["log"], output / "oracle-build.log")
    reference = json.loads((REPO / manifest["fortran"]["reference_manifest"]).read_text())
    docker = docker_command(checkout, build_status["docker_image_id"], reference["execution_profile"])
    inputs = {}
    for count in manifest["particle_counts"]:
        case = json.loads((REPO / manifest["fortran"]["case"]).read_text())
        case["release"]["particle_count"] = count
        case["release"]["inventory"]["quantity_kg"] = count
        case["release"]["mass_kg_per_particle"] = 1
        case_path = output / f"fortran-case-{count}.json"
        write(case_path, case)
        normalized = fixtures.validate_and_normalize_case_for_generation(
            case["case_id"], case, case_file=str(case_path),
            tracer=checkout / "examples/Tracer/SPECIES/SPECIES_024", aerosol=None)
        root = output / f"inputs-{count}"
        fixtures.write_case_fixtures(normalized, root)
        fixture = root / case["case_id"]
        meteo = root / "meteo"
        generation = checked(command(docker + ["python3", "/workspace/flexpart-gpu/scripts/generate_synthetic_grib.py",
            "--output-dir", container_path(meteo), *shlex.split((fixture / "METEO_ARGS.txt").read_text())],
            output / f"meteorology-{count}.log"))
        inputs[count] = {"case":case, "fixture":fixture, "meteo":meteo,
                         "generation":generation, "case_sha256":sha(case_path),
                         "input_sha256":tree_hashes(root)}
    return {"build":build, "status":build_status, "reference":reference, "docker":docker, "inputs":inputs}


def fortran_run(prepared, checkout, count, folder):
    record = {"regime":"pinned-11.1-fortran", "particles":count, "state":"FAIL"}
    try:
        oracle.require_pristine(checkout, REPO)
        executable_hash = prepared["build"]["oracle_executable_sha256"]
        if sha(checkout / "src/FLEXPART") != executable_hash:
            raise ValueError("reference binary changed")
        inputs = prepared["inputs"][count]
        if tree_hashes(inputs["fixture"].parent) != inputs["input_sha256"]:
            raise ValueError("prepared reference inputs changed before execution")
        run = folder / "fortran"
        shutil.copytree(inputs["fixture"], run / "options")
        (run / "output").mkdir()
        for name in ["IGBP_int1.dat", "sfcdata.t", "sfcdepo.t", "PARTOPTIONS"]:
            source = checkout / "options" / name
            if source.is_file():
                shutil.copyfile(source, run / "options" / name)
        met = container_path(inputs["meteo"])
        (run / "pathnames").write_text(f"./options/\n./output/\n{met}/\n{met}/AVAILABLE\n", encoding="utf-8")
        consumed = {"options":tree_hashes(run / "options"), "pathnames":sha(run / "pathnames"),
                    "meteorology":tree_hashes(inputs["meteo"]), "executable":executable_hash}
        write(run / "consumed-inputs.json", consumed)
        process = command(prepared["docker"] + ["python3", "/workspace/flexpart-gpu/scripts/benchmark_early.py",
                           "--fortran-worker", container_path(run)], folder / "fortran.log")
        record["process"] = process
        checked(process)
        text = (folder / "fortran.log").read_text(errors="replace")
        if "CONGRATULATIONS" not in text:
            raise ValueError("Fortran completion marker missing")
        counts = fortran_counts(text, count, inputs["case"]["integration"]["total_s"])
        oracle.require_pristine(checkout, REPO)
        if (sha(checkout / "src/FLEXPART") != executable_hash
                or consumed["options"] != tree_hashes(run / "options")
                or consumed["pathnames"] != sha(run / "pathnames")
                or consumed["meteorology"] != tree_hashes(inputs["meteo"])
                or inputs["input_sha256"] != tree_hashes(inputs["fixture"].parent)):
            raise ValueError("reference binary/consumed inputs changed during execution")
        record["decode_process"] = checked(command([sys.executable, REPO / "scripts/corpus/decode_oracle_output.py",
            "--raw-dir", run / "output", "--releases", run / "options/RELEASES",
            "--output", run / "oracle-summary.json"], folder / "decode.log"))
        timing = json.loads((run / "timing.json").read_text())
        distribution([timing["model_wall_seconds"]])
        record.update(state="PASS", timing=timing, consumed=consumed,
            particle_count_observations=counts,
            reference_reported_timers=reference_timers(text),
            meteorology_generation=inputs["generation"], case_sha256=inputs["case_sha256"],
            raw_output_sha256=tree_hashes(run / "output"), decoded_sha256=sha(run / "oracle-summary.json"),
            decoded=json.loads((run / "oracle-summary.json").read_text()),
            reference_revision=prepared["reference"]["pinned_commit"],
            executable_sha256=executable_hash, image=prepared["status"]["docker_image_id"],
            particle_steps_per_second=None,
            throughput_limitation="actual adaptive particle integration count is not exposed; no assumed particle-step denominator")
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.CalledProcessError) as exc:
        record["error"] = str(exc)
    finally:
        try:
            oracle.require_pristine(checkout, REPO)
        except (OSError, ValueError, subprocess.CalledProcessError) as exc:
            record.update(state="FAIL", cleanliness_error=str(exc))
    return record


def fortran_counts(text, expected, total_seconds):
    """Audit actual spawned/live/terminated counters, without assuming substeps."""
    rows = [dict(zip(("time_seconds", "spawned", "alive", "terminated"), map(int, values)))
            for values in re.findall(r"Time:\s*(-?\d+) seconds\. Total spawned:\s*(\d+) alive:\s*(\d+) terminated:\s*(\d+)", text)]
    positive = [row for row in rows if row["time_seconds"] > 0]
    if (not positive or positive[-1]["time_seconds"] < total_seconds
            or any(row["spawned"] != expected or row["alive"] != expected or row["terminated"] != 0 for row in positive)):
        raise ValueError("Fortran actual release/count/fate evidence missing or mismatched")
    return rows


def reference_timers(text):
    """Retain pinned SYSTEM_CLOCK diagnostics; these overlapping scopes do not add."""
    timers = {}
    for label in ["Read wind fields", "Timemanager", "first timestep", "Write particle files", "Total running time"]:
        found = re.search(re.escape(label) + r":\s*([+\-\d.EeDd]+)\s*seconds", text)
        if not found:
            raise ValueError(f"missing reference diagnostic timer: {label}")
        value = float(found[1].replace("D", "E").replace("d", "e"))
        if not math.isfinite(value) or value < 0:
            raise ValueError(f"invalid reference diagnostic timer: {label}")
        timers[label] = value
    return {"seconds":timers,"basis":"pinned SYSTEM_CLOCK, default-real diagnostic precision",
            "source":"FLEXPART.f90:112-142; timemanager_mod.f90:209-210,381-400,714-715",
            "scope":"overlapping; particle file timer excludes other concentration serialization; not transport-only"}


def worker(run):
    """Time the genuine full executable inside the frozen runtime environment."""
    import resource
    profile = subprocess.run([sys.executable, str(REPO / "scripts/write_oracle_run_manifest.py"),
        "check-runtime-profile", "--oracle-manifest", str(REPO / "reference/flexpart-11.1.json")],
        check=True, capture_output=True)
    (run / "runtime-profile.json").write_bytes(profile.stdout)
    start = time.perf_counter()
    result = subprocess.run(["/workspace/flexpart/src/FLEXPART"], cwd=run, check=False)
    elapsed = time.perf_counter() - start
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    write(run / "timing.json", {"model_wall_seconds":elapsed, "returncode":result.returncode,
        "child_peak_rss_kib":usage.ru_maxrss, "output_included":True,
        "container_logical_cpus":os.cpu_count(), "openmp_environment":{
            k:v for k,v in os.environ.items() if k.startswith(("OMP_", "GOMP_", "KMP_"))},
        "transport_only_seconds":None, "decode_seconds":None})
    return result.returncode


def aggregate(records, counts):
    result = []
    for count in counts:
        for regime in ["canonical-production", "pinned-11.1-fortran"]:
            selected = [r for r in records if r["particles"] == count and r["regime"] == regime]
            passed = [r for r in selected if r["state"] == "PASS"]
            values = [r["result"]["measured_wall_seconds"] if regime == "canonical-production"
                      else r["timing"]["model_wall_seconds"] for r in passed]
            result.append({"particles":count,"regime":regime,"requested":len(selected),
                "passed":len(passed),"wall_seconds":distribution(values),
                "state":"PASS" if passed and len(passed) == len(selected) else "FAIL"})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=MANIFEST)
    parser.add_argument("--checkout", type=Path, default=REPO / "target/flexpart-reference")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--fortran-worker", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.fortran_worker:
        return worker(args.fortran_worker)
    output = (args.output or REPO / "target/early-benchmark" / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")).resolve()
    output.relative_to(REPO / "target")
    output.mkdir(parents=True, exist_ok=False)
    manifest_path = output / "scenario.json"
    manifest, manifest_hash = freeze_manifest(args.manifest, manifest_path)
    revision = require_candidate(manifest)
    environment = {k:v for k,v in os.environ.items() if not k.startswith(("FLEXPART_", "WGPU_"))}
    environment.update(GPU_ENV)
    build = command(["cargo", "build", "--release", "--bin", "bench-canonical",
                     "--message-format=json-render-diagnostics"], output / "candidate-build.log", env=environment)
    try:
        binary = build_binary(build)
    except (OSError, ValueError, RuntimeError) as exc:
        binary = None
        build["artifact_error"] = str(exc)
    provenance = {"revision":revision,"manifest_sha256":manifest_hash,"environment":GPU_ENV,
        "host":platform.platform(),"cpu":platform.processor(),"logical_cpus":os.cpu_count(),
        "build":build,"binary_path":str(binary) if binary else None,
        "binary_sha256":sha(binary) if binary else None,
        "rustc":subprocess.check_output(["rustc", "-Vv"], text=True),
        "reference_manifest_sha256":sha(REPO / manifest["fortran"]["reference_manifest"]),
        "source_tree":subprocess.check_output(["git","rev-parse","HEAD^{tree}"], cwd=REPO,text=True).strip(),
        "provenance_marker":"GPT6.1 Sol"}
    prepared = None
    prepare_error = None
    try:
        prepared = prepare_fortran(args.checkout.resolve(), output, manifest)
        provenance["oracle_build"] = {"metadata_sha256":sha(output / "oracle-build.json"),
                                      "log_sha256":sha(output / "oracle-build.log"),
                                      "record":prepared["build"]}
    except (OSError, ValueError, RuntimeError, SystemExit, subprocess.CalledProcessError) as exc:
        prepare_error = str(exc)
        print(json.dumps({"oracle_preparation_error":prepare_error}), flush=True)
    records = []
    for count in manifest["particle_counts"]:
        for repetition in range(manifest["repetitions"]):
            folder = output / f"n{count}-rep{repetition + 1}"
            folder.mkdir()
            order = ["candidate", "fortran"] if repetition % 2 == 0 else ["fortran", "candidate"]
            for engine in order:
                if engine == "candidate" and binary is not None:
                    record = candidate_run(binary, manifest_path, manifest_hash, manifest, count, folder, environment)
                elif engine == "fortran" and prepared is not None:
                    record = fortran_run(prepared, args.checkout.resolve(), count, folder)
                else:
                    record = {"regime":"canonical-production" if engine == "candidate" else "pinned-11.1-fortran",
                              "particles":count,"state":"BLOCKED", "error":prepare_error if engine == "fortran" else build["artifact_error"]}
                record.update(repetition=repetition + 1, cache_label="first-process" if repetition == 0 else "repeated-process")
                records.append(record)
                write(folder / f"{engine}-run.json", record)
                print(json.dumps({k:record[k] for k in ["regime","particles","repetition","state"]}), flush=True)
    aggregates = aggregate(records, manifest["particle_counts"])
    if (require_candidate(manifest) != revision
            or sha(manifest_path) != manifest_hash
            or sha(REPO / manifest["fortran"]["reference_manifest"]) != provenance["reference_manifest_sha256"]
            or (binary is not None and sha(binary) != provenance["binary_sha256"])):
        raise ValueError("campaign source, manifest or executable changed; retained runs are invalid")
    report = {"schema":"flexpart-gpu.early-benchmark-report.v1", "provenance":provenance,
        "series":manifest["series"],"scientific_verdict":"NON_COMPARABLE", "speedup":None,
        "blockers":manifest["blockers"], "oracle_preparation_error":prepare_error,
        "aggregates":aggregates,"runs":records,
        "state":"PASS" if all(r["state"] == "PASS" for r in records) else "FAIL",
        "limitations":["No paired scientific comparison or full FLEXPART parity verdict.",
            "GPU strict per-step readback/profile timings include synchronization and auditing.",
            "GPU warm measurement follows >=3 s same-driver warmup and continues its trajectories; Fortran repetitions restart from release.",
            "Cold refers to fresh process/device, not flushed OS/disk/driver caches.",
            "Fortran model wall includes ingestion/output; Docker wrapper and decoder separate.",
            "No isolated transport timer, GPU timestamp, measured VRAM peak or Fortran particle-step counter."]}
    write(output / "report.json", report)
    lines = ["# Early 11.1 benchmark (#199)", "", f"Candidate: `{revision}`. Series: `{manifest['series']}`.",
        "", "Scientific verdict: **NON_COMPARABLE**. No speedup is calculated.", "",
        "| Regime | Particles | Successful/requested | Mean wall s | Stddev s |", "|---|---:|---:|---:|---:|"]
    for row in aggregates:
        d = row["wall_seconds"]
        lines.append(f"| {row['regime']} | {row['particles']} | {row['passed']}/{row['requested']} | "
            + (f"{d['mean']:.6f} | {d['stddev']} |" if d else "unavailable | unavailable |"))
    lines += ["", *[f"- {b}" for b in manifest["blockers"] + report["limitations"]], "",
        f"Oracle preparation error: {prepare_error}", "", "Full provenance, phase timings, invariants, errors and hashes: `report.json`.",
        "Raw per-run logs, inputs and outputs are retained beside it. Provenance marker: GPT6.1 Sol."]
    (output / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps({"state":report["state"],"scientific_verdict":"NON_COMPARABLE","report":str(output / "report.json")}))
    return 0 if report["state"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
