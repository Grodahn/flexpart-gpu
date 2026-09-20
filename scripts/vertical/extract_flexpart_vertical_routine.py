#!/usr/bin/env python3
"""Extract the exact pinned FLEXPART vertical-height routine for oracle execution.

The pinned checkout stays pristine. The generated Fortran module wraps the exact
`verttransform_ecmwf_heights` subroutine text from the pinned FLEXPART source
with only its directly required constants/state, so CI executes the real routine
instead of a reimplementation of its equations.
"""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

ROUTINE = "verttransform_ecmwf_heights"
EXPECTED_TOKENS = (
    "tvold=tt2_tmp(ix,jy)*(1.+0.378*ew(td2_tmp(ix,jy),ps_tmp(ix,jy))/",
    "pint=akz(kz)+bkz(kz)*ps_tmp(ix,jy)",
    "tv=tth_tmp(ix,jy,kz)*(1.+0.608*qvh_tmp(ix,jy,kz))",
    "wzlev(ix,jy,kz)=(uvzlev(ix,jy,kz+1)+uvzlev(ix,jy,kz))*0.5",
    "pinmconv(ix,jy,kz)=(uvzlev(ix,jy,kz+1)-uvzlev(ix,jy,kz-1))/",
)


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def git(checkout, *args):
    return subprocess.run(
        ["git", "-C", str(checkout), *args],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def extract_routine(source):
    lines = source.splitlines(keepends=True)
    start_re = re.compile(
        r"^\s*subroutine\s+verttransform_ecmwf_heights\b", re.IGNORECASE
    )
    end_re = re.compile(
        r"^\s*end\s+subroutine\s+verttransform_ecmwf_heights\b", re.IGNORECASE
    )
    starts = [i for i, line in enumerate(lines) if start_re.search(line)]
    if len(starts) != 1:
        raise ValueError(
            f"expected exactly one {ROUTINE} start, found {len(starts)}"
        )
    start = starts[0]
    ends = [
        i
        for i, line in enumerate(lines[start:], start=start)
        if end_re.search(line)
    ]
    if len(ends) != 1:
        raise ValueError(
            f"expected exactly one {ROUTINE} end after start, found {len(ends)}"
        )

    routine = "".join(lines[start : ends[0] + 1])
    missing = [token for token in EXPECTED_TOKENS if token not in routine]
    if missing:
        raise ValueError(
            "pinned routine does not match expected #30 contract: "
            + repr(missing)
        )
    return routine


def render_module(routine):
    return (
        "module flexpart_vertical_oracle_routine_mod\n"
        "  use par_mod, only: r_air, ga\n"
        "  use qvsat_mod, only: ew\n"
        "  use vertical_oracle_state_mod, only: nuvzmax, nzmax, nuvz, nwz, nz, &\n"
        "       akz, bkz, aknew, bknew\n"
        "  implicit none\n"
        "contains\n"
        + routine
        + ("\n" if not routine.endswith("\n") else "")
        + "end module flexpart_vertical_oracle_routine_mod\n"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--oracle-checkout", type=Path, required=True)
    parser.add_argument("--reference-manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--provenance-output", type=Path, required=True)
    args = parser.parse_args()

    manifest = json.loads(args.reference_manifest.read_text(encoding="utf-8"))
    pinned = manifest["pinned_commit"]
    actual = git(args.oracle_checkout, "rev-parse", "HEAD")
    if actual != pinned:
        raise ValueError(f"oracle checkout {actual} != pinned {pinned}")
    if git(args.oracle_checkout, "status", "--porcelain"):
        raise ValueError("oracle checkout is dirty")

    source_path = args.oracle_checkout / "src" / "verttransform_mod.f90"
    source_bytes = source_path.read_bytes()
    source = source_bytes.decode("utf-8")
    routine = extract_routine(source)
    rendered = render_module(routine)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(rendered, encoding="utf-8")

    provenance = {
        "schema": "flexpart-gpu.flexpart-vertical-routine-oracle.v1",
        "oracle_name": manifest.get("name", "FLEXPART"),
        "oracle_version": manifest.get("version", "11.1"),
        "pinned_commit": pinned,
        "checkout_clean": True,
        "source_path": "src/verttransform_mod.f90",
        "source_sha256": sha256_bytes(source_bytes),
        "routine": ROUTINE,
        "routine_sha256": sha256_bytes(routine.encode("utf-8")),
        "generated_module_sha256": sha256_bytes(rendered.encode("utf-8")),
        "extraction": (
            "exact contiguous source slice from subroutine declaration through "
            "matching end subroutine; wrapper only supplies original imports/state"
        ),
    }
    args.provenance_output.write_text(
        json.dumps(provenance, indent=2) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()
