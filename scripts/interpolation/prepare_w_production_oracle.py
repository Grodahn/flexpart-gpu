#!/usr/bin/env python3
"""Prepare and freeze the issue #80 pristine FLEXPART W production oracle."""

import argparse
import hashlib
import json
import math
import re
import subprocess
from pathlib import Path


ABSOLUTE_TOLERANCE_M_S = 1.0e-6
RELATIVE_TOLERANCE = 1.0e-5
EXPECTED_SYMBOLS = {
    "verttransform_ecmwf_heights": "__verttransform_mod_MOD_verttransform_ecmwf_heights",
    "verttransform_ecmwf_windfields": "__verttransform_mod_MOD_verttransform_ecmwf_windfields",
    "interpol_wind": "__interpol_mod_MOD_interpol_wind",
    "interpol_wind_meter": "__interpol_mod_MOD_interpol_wind_meter",
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(checkout: Path, *arguments: str) -> str:
    safe_checkout = checkout.resolve().as_posix()
    return subprocess.run(
        ["git", "-c", f"safe.directory={safe_checkout}", "-C", str(checkout), *arguments],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def canonical_field(snapshot: dict, field_id: str) -> list[float]:
    matches = [field["values"] for field in snapshot["fields"] if field["id"] == field_id]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one {field_id} field")
    return matches[0]


def validate_sources(snapshot: dict, motion: dict) -> None:
    grid = snapshot["horizontal_grid"]
    vertical = snapshot["vertical_coordinate"]
    if (grid["nx"], grid["ny"]) != (1, 1):
        raise ValueError("#80 synthetic source must be a single canonical column")
    if vertical["kind"] != "hybrid_sigma_pressure":
        raise ValueError("#80 requires the #30 hybrid-sigma pressure boundary")
    if vertical["ordering"] != "increasing" or vertical["reference"] != "model_native":
        raise ValueError("#80 requires canonical top-to-surface model-native ordering")
    expected_motion = {
        "kind": "pressure_velocity_omega",
        "unit": "pascal_per_second",
        "sign": "positive_pressure_increasing",
        "vertical_staggering": "level_interface",
        "profile_shape": "deliberately_non_linear",
    }
    for key, expected in expected_motion.items():
        if motion.get(key) != expected:
            raise ValueError(f"motion {key} must be {expected!r}")
    levels = vertical["level_values"]
    if len(motion["values"]) != len(levels) + 1:
        raise ValueError("interface omega requires native_nz + 1 values")
    if any(not math.isfinite(value) for value in motion["values"]):
        raise ValueError("interface omega contains a non-finite value")
    second_differences = [
        motion["values"][index + 2]
        - 2.0 * motion["values"][index + 1]
        + motion["values"][index]
        for index in range(len(motion["values"]) - 2)
    ]
    if not any(abs(value) > 1.0e-6 for value in second_differences):
        raise ValueError("the #80 omega fixture must be deliberately non-linear")


def emit_input(snapshot_path: Path, motion_path: Path, output_path: Path) -> None:
    snapshot = json.loads(snapshot_path.read_text(encoding="utf-8"))
    motion = json.loads(motion_path.read_text(encoding="utf-8"))
    validate_sources(snapshot, motion)
    vertical = snapshot["vertical_coordinate"]
    native_nz = len(vertical["level_values"])
    surface_pressure = canonical_field(snapshot, "surface_pressure")
    temperature_2m = canonical_field(snapshot, "temperature2m")
    dewpoint_2m = canonical_field(snapshot, "dewpoint2m")
    temperature = canonical_field(snapshot, "temperature")
    humidity = canonical_field(snapshot, "specific_humidity")
    if any(len(values) != 1 for values in (surface_pressure, temperature_2m, dewpoint_2m)):
        raise ValueError("surface fields must contain one value")
    if len(temperature) != native_nz or len(humidity) != native_nz:
        raise ValueError("model-level fields must match the vertical coordinate")

    lines = [
        str(native_nz),
        f"{surface_pressure[0]} {temperature_2m[0]} {dewpoint_2m[0]}",
    ]
    lines.extend(
        f"{a} {b}"
        for a, b in zip(
            vertical["hybrid_a_interface_pa"], vertical["hybrid_b_interface"], strict=True
        )
    )
    lines.extend(
        f"{temp} {q}" for temp, q in zip(temperature, humidity, strict=True)
    )
    lines.extend(str(value) for value in motion["values"])
    # kind, bottom-to-top model-height bracket, strict fraction. Boundaries
    # use kind 0/2; kind 1 records three strict-interior particle heights.
    queries = [(0, 1, 0.0), (1, 1, 0.25), (1, 2, 0.5), (1, 3, 0.75), (2, native_nz, 1.0)]
    lines.append(str(len(queries)))
    lines.extend(f"{kind} {level} {fraction}" for kind, level, fraction in queries)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def parse_oracle_output(path: Path) -> dict:
    lines = path.read_text(encoding="utf-8").splitlines()
    if not lines or lines[0] != "FLEXPART_W_PRODUCTION_ORACLE_V1":
        raise ValueError("unexpected #80 oracle output header")
    cursor = 1

    def count(label: str) -> int:
        nonlocal cursor
        tokens = lines[cursor].split()
        cursor += 1
        if len(tokens) != 2 or tokens[0] != label:
            raise ValueError(f"expected {label} record")
        return int(tokens[1])

    native_levels = count("NATIVE_LEVELS")
    model_count = count("MODEL_LEVELS")
    model_grid = []
    for _ in range(model_count):
        index, height, value = lines[cursor].split()
        cursor += 1
        model_grid.append({"index_bottom_to_top": int(index), "height_m_agl": float(height), "w_m_s": float(value)})
    interface_count = count("INTERFACES")
    interfaces = []
    for _ in range(interface_count):
        index, height, converted, omega = lines[cursor].split()
        cursor += 1
        interfaces.append(
            {
                "index_bottom_to_top": int(index),
                "height_m_agl": float(height),
                "w_m_s": float(converted),
                "omega_pa_s": float(omega),
            }
        )
    query_count = count("QUERIES")
    queries = []
    query_classifications = {
        0: "lower_boundary",
        1: "strict_interior",
        2: "upper_flexpart_height_boundary",
    }
    for _ in range(query_count):
        query, kind, level, fraction, height, pristine = lines[cursor].split()
        cursor += 1
        parsed_kind = int(kind)
        if parsed_kind not in query_classifications:
            raise ValueError(f"unknown oracle query kind {parsed_kind}")
        queries.append(
            {
                "query": int(query),
                "kind": parsed_kind,
                "classification": query_classifications[parsed_kind],
                "model_bracket_bottom_to_top": int(level),
                "fraction": float(fraction),
                "particle_height_m_agl": float(height),
                "pristine_w_m_s": float(pristine),
            }
        )
    if cursor != len(lines):
        raise ValueError("unexpected trailing #80 oracle records")
    return {
        "native_levels": native_levels,
        "model_grid": model_grid,
        "interface_grid": interfaces,
        "queries": queries,
    }


def direct_interface_sample(interfaces: list[dict], height: float) -> float:
    if height <= interfaces[0]["height_m_agl"]:
        return interfaces[0]["w_m_s"]
    if height >= interfaces[-1]["height_m_agl"]:
        return interfaces[-1]["w_m_s"]
    for lower, upper in zip(interfaces, interfaces[1:]):
        if height <= upper["height_m_agl"]:
            span = upper["height_m_agl"] - lower["height_m_agl"]
            fraction = (height - lower["height_m_agl"]) / span
            return lower["w_m_s"] + fraction * (upper["w_m_s"] - lower["w_m_s"])
    raise AssertionError("bounded direct interpolation did not find a bracket")


def compare(parsed: dict) -> tuple[list[dict], str, float]:
    results = []
    maximum_difference = 0.0
    all_equivalent = True
    for query in parsed["queries"]:
        direct = direct_interface_sample(parsed["interface_grid"], query["particle_height_m_agl"])
        difference = query["pristine_w_m_s"] - direct
        tolerance = ABSOLUTE_TOLERANCE_M_S + RELATIVE_TOLERANCE * max(
            abs(query["pristine_w_m_s"]), abs(direct)
        )
        equivalent = abs(difference) <= tolerance
        all_equivalent &= equivalent
        maximum_difference = max(maximum_difference, abs(difference))
        results.append(
            {
                **query,
                "direct_interface_w_m_s": direct,
                "pristine_minus_direct_m_s": difference,
                "tolerance_m_s": tolerance,
                "equivalent": equivalent,
            }
        )
    return results, "equivalent" if all_equivalent else "not_equivalent", maximum_difference


def verify_call_edges(nm_text: str, call_sites_text: str) -> list[dict]:
    symbols = {}
    for line in nm_text.splitlines():
        tokens = line.split()
        if len(tokens) == 3:
            symbols[tokens[2]] = int(tokens[0], 16)

    def function_text(symbol: str) -> str:
        marker = f"<{symbol}>:"
        start = call_sites_text.find(marker)
        if start < 0:
            raise ValueError(f"call-site evidence lacks {symbol}")
        next_header = re.search(r"\n[0-9a-f]+ <[^>]+>:\n", call_sites_text[start + len(marker):])
        end = len(call_sites_text) if next_header is None else start + len(marker) + next_header.start()
        return call_sites_text[start:end]

    def has_indirect_call(caller: str, callee: str) -> bool:
        body = function_text(caller)
        base_immediate = re.search(r"movabs \$0x([0-9a-f]+),%r11", body)
        base_anchor = re.search(r"lea\s+[^#]+# ([0-9a-f]+) <", body)
        if base_immediate is None or base_anchor is None:
            raise ValueError(f"cannot resolve large-model call base for {caller}")
        call_base = int(base_immediate.group(1), 16) + int(base_anchor.group(1), 16)
        displacement = (symbols[callee] - call_base) % (1 << 64)
        pattern = re.compile(
            rf"movabs \$0x{displacement:x},%rax(?:(?!movabs).){{0,500}}?call\s+\*%rax",
            re.DOTALL,
        )
        return pattern.search(body) is not None

    edges = [
        ("MAIN__", "__verttransform_mod_MOD_verttransform_ecmwf_heights"),
        ("MAIN__", "__verttransform_mod_MOD_verttransform_ecmwf_windfields"),
        ("MAIN__", "__interpol_mod_MOD_interpol_wind"),
        ("__interpol_mod_MOD_interpol_wind", "__interpol_mod_MOD_interpol_wind_meter"),
    ]
    evidence = []
    for caller, callee in edges:
        if caller not in symbols or callee not in symbols:
            raise ValueError(f"nm evidence lacks call edge endpoint {caller} -> {callee}")
        if not has_indirect_call(caller, callee):
            raise ValueError(f"linked executable lacks call edge {caller} -> {callee}")
        evidence.append(
            {
                "caller": caller,
                "callee": callee,
                "caller_address": f"0x{symbols[caller]:x}",
                "callee_address": f"0x{symbols[callee]:x}",
                "verified_in_linked_executable": True,
            }
        )
    return evidence


def build_report(args: argparse.Namespace) -> dict:
    manifest = json.loads(args.reference_manifest.read_text(encoding="utf-8"))
    actual_commit = git(args.oracle_checkout, "rev-parse", "HEAD")
    if actual_commit != manifest["pinned_commit"]:
        raise ValueError(f"oracle checkout {actual_commit} != pinned {manifest['pinned_commit']}")
    if git(args.oracle_checkout, "status", "--porcelain"):
        raise ValueError("oracle checkout is not pristine")
    parsed = parse_oracle_output(args.oracle_output)
    comparisons, verdict, maximum_difference = compare(parsed)
    nm_text = args.nm_output.read_text(encoding="utf-8")
    missing_symbols = [name for name, symbol in EXPECTED_SYMBOLS.items() if symbol not in nm_text]
    if missing_symbols:
        raise ValueError(f"oracle executable is missing symbols: {', '.join(missing_symbols)}")
    call_edges = verify_call_edges(nm_text, args.call_sites.read_text(encoding="utf-8"))
    linked_objects = args.linked_objects.read_text(encoding="utf-8").splitlines()
    if linked_objects != sorted(set(linked_objects)) or not linked_objects:
        raise ValueError("linked FLEXPART object list must be non-empty, sorted, and unique")
    compiler = args.compiler_identity.read_text(encoding="utf-8").strip()
    source_files = [
        "src/verttransform_mod.f90",
        "src/interpol_mod.f90",
        "src/windfields_mod.f90",
    ]
    direct_objects = [
        {
            "path": source,
            "source_sha256": sha256(args.oracle_checkout / source),
            "object": Path(source).with_suffix(".o").name,
            "object_sha256": sha256(args.oracle_checkout / "src" / Path(source).with_suffix(".o").name),
        }
        for source in source_files
    ]
    real_fixture = json.loads(args.real_fixture.read_text(encoding="utf-8"))
    real_motion_fields = [
        field["id"] for field in real_fixture["fields"]
        if field["id"] in {"vertical_velocity", "omega", "eta_dot"}
    ]
    return {
        "schema": {"id": "flexpart-gpu.w-production-oracle", "version": 1},
        "issue": 80,
        "conclusion": verdict,
        "conclusion_statement": (
            "Direct interpolation on the #30 W/interface runtime geometry differs from "
            "pristine FLEXPART's verttransform_ecmwf_windfields -> height[] -> "
            "interpol_wind_meter result."
            if verdict == "not_equivalent"
            else
            "Direct interpolation on the #30 W/interface runtime geometry is equivalent "
            "to the supported pristine FLEXPART 11.1 two-stage production path within tolerance."
        ),
        "oracle_path": [
            "pressure velocity / interface W",
            "verttransform_mod::verttransform_ecmwf_heights",
            "verttransform_mod::verttransform_ecmwf_windfields",
            "windfields_mod::ww on height[]",
            "interpol_mod::interpol_wind (eta=no)",
            "interpol_mod::interpol_wind_meter",
            "final sampled W",
        ],
        "tolerances": {
            "formula": "abs(pristine-direct) <= absolute_m_s + relative*max(abs(pristine),abs(direct))",
            "absolute_m_s": ABSOLUTE_TOLERANCE_M_S,
            "relative": RELATIVE_TOLERANCE,
        },
        "maximum_absolute_difference_m_s": maximum_difference,
        "synthetic_case": {
            "profile": "deliberately_non_linear_interface_omega",
            "source_snapshot": "fixtures/vertical/synthetic-column-v1.json",
            "source_motion": "fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json",
            "input_sha256": sha256(args.oracle_input),
            "snapshot_sha256": sha256(args.snapshot),
            "motion_sha256": sha256(args.motion),
            **parsed,
            "comparisons": comparisons,
        },
        "real_data_obligation": {
            "canonical_fixture": "fixtures/meteorology/era5-etex-native-v1.json",
            "fixture_sha256": sha256(args.real_fixture),
            "vertical_motion_fields_present": real_motion_fields,
            "status": "not_available_in_checked_in_canonical_fixture" if not real_motion_fields else "available",
            "scope_result": (
                "The checked-in #29/#30 canonical ERA5/ETEX fixture contains thermodynamic "
                "fields but no vertical-motion field; #80 therefore retains synthetic direct-oracle "
                "proof and does not add provider decoding or eta-dot preprocessing."
                if not real_motion_fields else
                "The checked-in canonical fixture exposes vertical motion."
            ),
        },
        "provenance": {
            "oracle": {
                "name": manifest["name"],
                "version": manifest["version"],
                "pinned_commit": actual_commit,
                "checkout_clean": True,
            },
            "build": {
                "compiler_identity": compiler,
                "oracle_build_profile": manifest["execution_profile"]["build"],
                "driver_compile": {
                    "compile_flags": ["-O0", "-fopenmp", "-mcmodel=large"],
                    "link_strategy": "all pristine src/*.o except FLEXPART.o",
                    "coordinate_mode": "eta=no",
                },
                "driver_source": "scripts/interpolation/w_production_oracle.f90",
                "driver_sha256": sha256(args.driver),
                "harness_source": "scripts/interpolation/w_production_oracle.sh",
                "harness_sha256": sha256(args.harness),
                "packer_source": "scripts/interpolation/prepare_w_production_oracle.py",
                "packer_sha256": sha256(Path(__file__).resolve()),
                "reference_manifest_sha256": sha256(args.reference_manifest),
                "binary_sha256": sha256(args.binary),
                "oracle_output_sha256": sha256(args.oracle_output),
            },
            "linked_flexpart": {
                "strategy": "all pristine src/*.o except FLEXPART.o",
                "objects": linked_objects,
                "object_list_sha256": sha256(args.linked_objects),
                "direct_routine_objects": direct_objects,
                "required_symbols": EXPECTED_SYMBOLS,
                "verified_call_edges": call_edges,
                "nm_sha256": sha256(args.nm_output),
                "call_sites_sha256": sha256(args.call_sites),
                "link_map_sha256": sha256(args.link_map),
            },
        },
        "handoff_to_issue_73": {
            "status": "blocked_pending_review_and_merge_of_issue_80",
            "required_behavior": (
                "Reproduce the frozen pristine two-stage production result or document an "
                "intentional canonical divergence with separate candidate/oracle expectations."
                if verdict == "not_equivalent" else
                "Direct #30-interface sampling may use this tolerance contract after #80 is merged."
            ),
            "interface_vertical_motion_block_removed": False,
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--snapshot", type=Path, required=True)
    parser.add_argument("--motion", type=Path, required=True)
    parser.add_argument("--oracle-input", type=Path, required=True)
    parser.add_argument("--oracle-output", type=Path)
    parser.add_argument("--emit-input-only", action="store_true")
    parser.add_argument("--oracle-checkout", type=Path)
    parser.add_argument("--reference-manifest", type=Path)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--driver", type=Path)
    parser.add_argument("--harness", type=Path)
    parser.add_argument("--nm-output", type=Path)
    parser.add_argument("--call-sites", type=Path)
    parser.add_argument("--link-map", type=Path)
    parser.add_argument("--linked-objects", type=Path)
    parser.add_argument("--compiler-identity", type=Path)
    parser.add_argument("--real-fixture", type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    emit_input(args.snapshot, args.motion, args.oracle_input)
    if args.emit_input_only:
        return
    required = [
        "oracle_output", "oracle_checkout", "reference_manifest", "binary", "driver",
        "harness", "nm_output", "call_sites", "link_map", "linked_objects",
        "compiler_identity", "real_fixture", "report",
    ]
    for name in required:
        if getattr(args, name) is None:
            parser.error(f"--{name.replace('_', '-')} is required unless --emit-input-only is used")
    report = build_report(args)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
