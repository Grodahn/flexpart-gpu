#!/usr/bin/env python3
"""Write machine-readable provenance for the direct FLEXPART vertical oracle driver."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--driver-source", type=Path, required=True)
    parser.add_argument("--oracle-checkout", type=Path, required=True)
    parser.add_argument("--reference-manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    manifest = json.loads(args.reference_manifest.read_text(encoding="utf-8"))
    pinned = manifest["pinned_commit"]
    actual = subprocess.run(
        ["git", "-C", str(args.oracle_checkout), "rev-parse", "HEAD"],
        check=True, capture_output=True, text=True
    ).stdout.strip()
    if actual != pinned:
        raise ValueError(f"oracle checkout {actual} != pinned {pinned}")
    if subprocess.run(
        ["git", "-C", str(args.oracle_checkout), "status", "--porcelain"],
        check=True, capture_output=True, text=True
    ).stdout.strip():
        raise ValueError("oracle checkout is dirty")

    src = args.oracle_checkout / "src"
    vert_source = src / "verttransform_mod.f90"
    wind_source = src / "windfields_mod.f90"
    vert_object = src / "verttransform_mod.o"
    wind_object = src / "windfields_mod.o"
    for path in (args.binary, args.driver_source, vert_source, wind_source, vert_object, wind_object):
        if not path.is_file():
            raise ValueError(f"missing direct-oracle provenance input: {path}")

    nm = subprocess.run(
        ["nm", str(args.binary)], check=True, capture_output=True, text=True
    ).stdout
    symbol = "__verttransform_mod_MOD_verttransform_ecmwf_heights"
    if symbol not in nm:
        raise ValueError(
            "direct oracle binary does not contain the real "
            "verttransform_ecmwf_heights module symbol"
        )

    report = {
        "schema": "flexpart-gpu.vertical-direct-oracle-provenance.v1",
        "pinned_commit": pinned,
        "checkout_clean": True,
        "source_path": "src/verttransform_mod.f90",
        "source_sha256": sha256(vert_source),
        "routine": "verttransform_ecmwf_heights",
        "oracle": {
            "name": manifest["name"],
            "version": manifest["version"],
            "pinned_commit": pinned,
            "checkout_clean": True,
        },
        "driver": {
            "source_path": str(args.driver_source),
            "source_sha256": sha256(args.driver_source),
            "binary_path": str(args.binary),
            "binary_sha256": sha256(args.binary),
        },
        "linked_flexpart": {
            "entrypoint": "verttransform_mod::verttransform_ecmwf_heights",
            "entrypoint_symbol": symbol,
            "symbol_present_in_binary": True,
            "verttransform_source_sha256": sha256(vert_source),
            "windfields_source_sha256": sha256(wind_source),
            "verttransform_object_sha256": sha256(vert_object),
            "windfields_object_sha256": sha256(wind_object),
        },
        "scope": (
            "The driver initializes the windfields_mod vertical-coordinate state "
            "from the canonical column fixture, then calls the compiled pristine "
            "FLEXPART 11.1 verttransform_ecmwf_heights routine directly."
        ),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
