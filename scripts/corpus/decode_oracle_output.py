#!/usr/bin/env python3
"""Decode raw FLEXPART 11.1 oracle outputs into machine-readable facts.

Reads one corpus oracle case directory produced by scripts/run-corpus.sh
(``header``, ``dates`` and ``grid_conc_*`` preserved from the pinned oracle
run) and writes ``oracle_summary.json`` consumed by
scripts/corpus/compare_corpus.py. This script reports facts only; budget
closure and calibration live in the comparator next to the versioned
thresholds.

FLEXPART 11.1 writes binary concentration output only (``grid_conc_*``);
per-particle ``partposit`` dumps are NetCDF-only in v11.1
(``output_mod.f90:output_particles``), so particle-level oracle data does
not exist for these runs. Each ``grid_conc`` time slice holds, per release
and age class, a 2-D wet-deposition block, a 2-D dry-deposition block and
the 3-D concentration block. The dry/wet blocks accumulate removed mass
since release start (``drydepo_mod.f90``/``wetdepo_mod.f90`` accumulate into
the ``drydeposit``/``wetdeposit`` grids dumped by ``output_mod.f90``).

Mass-proportional fields: oracle concentration grids hold mass per volume,
so raw grid sums are NOT mass-conserving across windows when the plume
redistributes vertically (measured 20% drift on WIND-UNI-002 between two
windows with zero removal; volume-weighted sums coincide to 4e-08). The
decoder therefore persists per-level concentration sums; the comparator
weights them by layer thickness and cos(latitude) into mass-proportional
quantities. Deposition grids hold mass per area and enter with cos(latitude)
weighting. No mass-unit conversion is inferred anywhere.

Only the Python standard library is used so the decoder runs on any host
without extra prerequisites. Fails (non-zero exit) when the header or any
``grid_conc_*`` file is missing or unparseable.

Usage:
    python3 scripts/corpus/decode_oracle_output.py \
        --raw-dir target/corpus/oracle/ADV-ANA-001/raw \
        --releases fixtures/corpus/fortran/ADV-ANA-001/RELEASES \
        --output target/corpus/oracle/ADV-ANA-001/oracle_summary.json
"""

import argparse
import hashlib
import json
import math
import re
import struct
from pathlib import Path


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def read_record(source, endian: str):
    raw = source.read(4)
    if len(raw) < 4:
        return None
    (length,) = struct.unpack(f"{endian}i", raw)
    if length < 0:
        raise ValueError(f"negative record length {length}")
    data = source.read(length)
    if len(data) < length:
        raise ValueError(f"truncated record: need {length}, got {len(data)}")
    (trailer,) = struct.unpack(f"{endian}i", source.read(4))
    if trailer != length:
        raise ValueError(f"record length mismatch {length} vs {trailer}")
    return data


def read_header(path: Path, endian: str) -> dict:
    with path.open("rb") as source:
        rec = read_record(source, endian)
        ibdate, ibtime = struct.unpack(f"{endian}2i", rec[:8])
        rec = read_record(source, endian)
        loutstep, loutaver, loutsample = struct.unpack(f"{endian}3i", rec[:12])
        rec = read_record(source, endian)
        outlon0, outlat0 = struct.unpack(f"{endian}2f", rec[:8])
        numxgrid, numygrid = struct.unpack(f"{endian}2i", rec[8:16])
        dxout, dyout = struct.unpack(f"{endian}2f", rec[16:24])
        rec = read_record(source, endian)
        (numzgrid,) = struct.unpack(f"{endian}i", rec[:4])
        outheights = list(struct.unpack(f"{endian}{numzgrid}f", rec[4:4 + 4 * numzgrid]))
        rec = read_record(source, endian)  # release date
        rec = read_record(source, endian)  # 3*nspec, maxpointspec_act
        n3spec, maxpointspec_act = struct.unpack(f"{endian}2i", rec[:8])
        for _ in range(n3spec):
            read_record(source, endian)  # species names
        rec = read_record(source, endian)
        (numpoint,) = struct.unpack(f"{endian}i", rec[:4])
        nspec = n3spec // 3
        for _ in range(numpoint):
            read_record(source, endian)  # ireleasestart, ireleaseend, kindz
            read_record(source, endian)  # xp1, yp1, xp2, yp2, z1, z2
            read_record(source, endian)  # npart, 1
            read_record(source, endian)  # comment
            for _ in range(nspec):
                read_record(source, endian)  # xmass (wet)
                read_record(source, endian)  # xmass (dry)
                read_record(source, endian)  # xmass (conc)
        rec = read_record(source, endian)  # method, lsubgrid, ...
        rec = read_record(source, endian)  # nageclass, lage
        (nageclass,) = struct.unpack(f"{endian}i", rec[:4])
    return {
        "numxgrid": numxgrid,
        "numygrid": numygrid,
        "numzgrid": numzgrid,
        "outlon0": float(outlon0),
        "outlat0": float(outlat0),
        "dxout": float(dxout),
        "dyout": float(dyout),
        "outheights": [float(h) for h in outheights],
        "nspec": nspec,
        "maxpointspec_act": maxpointspec_act,
        "nageclass": nageclass,
        "loutstep": loutstep,
        "loutaver": loutaver,
        "loutsample": loutsample,
        "endian": endian,
    }


def decode_sparse_block(records, idx: int, endian: str):
    (count_i,) = struct.unpack(f"{endian}i", records[idx][:4])
    idx += 1
    if count_i > 0:
        indices = list(struct.unpack(f"{endian}{count_i}i", records[idx][: 4 * count_i]))
    else:
        indices = []
    idx += 1
    (count_r,) = struct.unpack(f"{endian}i", records[idx][:4])
    idx += 1
    if count_r > 0:
        values = list(struct.unpack(f"{endian}{count_r}f", records[idx][: 4 * count_r]))
    else:
        values = []
    idx += 1
    return indices, values, idx


def reconstruct_cells(indices, values, total_cells: int):
    """Reconstruct per-cell magnitudes from sparse sign-run encoding."""
    cells = [0.0] * total_cells
    if not indices or not values:
        return cells
    offset = 0
    for run, start in enumerate(indices):
        end = indices[run + 1] if run + 1 < len(indices) else total_cells
        if offset >= len(values):
            break
        sign = 1.0 if values[offset] >= 0 else -1.0
        cell = start
        while offset < len(values) and cell < end:
            if (values[offset] >= 0) != (sign >= 0):
                break
            cells[cell] = abs(values[offset])
            offset += 1
            cell += 1
    return cells


def read_slice(path: Path, header: dict, endian: str) -> dict:
    """Decode one grid_conc time slice into reservoir sums and level sums."""
    nx, ny, nz = header["numxgrid"], header["numygrid"], header["numzgrid"]
    nkp, nage = header["maxpointspec_act"], header["nageclass"]
    with path.open("rb") as source:
        records = []
        while True:
            rec = read_record(source, endian)
            if rec is None:
                break
            records.append(rec)
    idx = 1  # skip itime record
    # Fortran flat layouts: 3-D index ix + jy*nx + kz*nx*ny with kz in 1..nz;
    # 2-D deposition index ix + jy*nx.
    conc_cells = [0.0] * ((nz + 1) * nx * ny)
    dry_cells = [0.0] * (nx * ny)
    wet_cells = [0.0] * (nx * ny)
    for _ in range(nkp):
        for _ in range(nage):
            w_idx, w_val, idx = decode_sparse_block(records, idx, endian)
            d_idx, d_val, idx = decode_sparse_block(records, idx, endian)
            c_idx, c_val, idx = decode_sparse_block(records, idx, endian)
            wet_part = reconstruct_cells(w_idx, w_val, nx * ny)
            dry_part = reconstruct_cells(d_idx, d_val, nx * ny)
            conc_part = reconstruct_cells(c_idx, c_val, (nz + 1) * nx * ny)
            for k, v in enumerate(wet_part):
                wet_cells[k] += v
            for k, v in enumerate(dry_part):
                dry_cells[k] += v
            for k, v in enumerate(conc_part):
                conc_cells[k] += v
    level_sums = []
    for kz in range(1, nz + 1):
        level_sums.append(
            sum(conc_cells[ix + jy * nx + kz * nx * ny] for jy in range(ny) for ix in range(nx))
        )
    return {
        "level_sums_native": level_sums,
        "dry_cells": dry_cells,
        "wet_cells": wet_cells,
        "conc_cells": conc_cells,
    }


def weighted_sums(slice_data: dict, header: dict) -> dict:
    """Mass-proportional reservoir scalars for one decoded slice.

    Concentration cells (mass per volume) are weighted by layer thickness
    and cos(latitude); deposition cells (mass per area) by cos(latitude).
    The uniform horizontal grid spacing cancels in same-grid ratios; the
    per-row cos(latitude) factor does not and is applied exactly. All three
    scalars share one proportionality constant per run, so their sum closes
    against a calibrated scale (see compare_corpus.py).
    """
    nx, ny, nz = header["numxgrid"], header["numygrid"], header["numzgrid"]
    heights = header["outheights"]
    thick = [heights[0]] + [heights[i] - heights[i - 1] for i in range(1, nz)]
    cos_rows = [
        math.cos(math.radians(header["outlat0"] + (jy + 0.5) * header["dyout"]))
        for jy in range(ny)
    ]
    air = dry = wet = 0.0
    for ix in range(nx):
        for jy in range(ny):
            cos_lat = cos_rows[jy]
            dry += slice_data["dry_cells"][ix + jy * nx] * cos_lat
            wet += slice_data["wet_cells"][ix + jy * nx] * cos_lat
            for iz in range(nz):
                air += (
                    slice_data["conc_cells"][ix + jy * nx + (iz + 1) * nx * ny]
                    * thick[iz]
                    * cos_lat
                )
    return {"airborne": air, "dry_deposited": dry, "wet_deposited": wet}


def release_mass_g(releases_path: Path) -> float:
    text = re.sub(r"!.*", "", releases_path.read_text(encoding="utf-8"))
    match = re.search(r"\bMASS\s*=\s*([^,/\n]+)", text, re.IGNORECASE)
    if not match:
        raise ValueError(f"MASS not found in {releases_path}")
    return float(match.group(1).strip().replace("D", "E"))


def center_of_mass(cells, header: dict):
    nx, ny, nz = header["numxgrid"], header["numygrid"], header["numzgrid"]
    total = sum(cells)
    if total <= 0:
        return None
    com_lon = com_lat = com_z = 0.0
    for kz in range(1, nz + 1):
        for jy in range(ny):
            for ix in range(nx):
                w = cells[ix + jy * nx + kz * nx * ny]
                if w > 0:
                    com_lon += w * (header["outlon0"] + (ix + 0.5) * header["dxout"])
                    com_lat += w * (header["outlat0"] + (jy + 0.5) * header["dyout"])
                    com_z += w * header["outheights"][kz - 1]
    return {
        "lon_deg": com_lon / total,
        "lat_deg": com_lat / total,
        "z_m": com_z / total,
    }


def mass_proportional_fractions(conc_cells, header: dict):
    """Normalized mass-proportional field in the shared comparison layout.

    Weights each concentration cell by layer thickness and cos(latitude);
    uniform horizontal spacing cancels in the normalization. Layout is
    (ix * ny + iy) * nz + iz with iz = 0..nz-1 for kz = 1..nz, matching the
    candidate binning in compare_corpus.py.
    """
    nx, ny, nz = header["numxgrid"], header["numygrid"], header["numzgrid"]
    heights = header["outheights"]
    thick = [heights[0]] + [heights[i] - heights[i - 1] for i in range(1, nz)]
    weighted = []
    for ix in range(nx):
        for jy in range(ny):
            lat = header["outlat0"] + (jy + 0.5) * header["dyout"]
            cos_lat = math.cos(math.radians(lat))
            for iz in range(nz):
                weighted.append(
                    conc_cells[ix + jy * nx + (iz + 1) * nx * ny] * thick[iz] * cos_lat
                )
    total = sum(weighted) or 1.0
    return [v / total for v in weighted]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--raw-dir", required=True)
    parser.add_argument("--releases", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    raw_dir = Path(args.raw_dir)
    header_path = raw_dir / "header"
    if not header_path.is_file():
        raise SystemExit(f"oracle header missing: {header_path}")
    conc_files = sorted(raw_dir.glob("grid_conc_*"))
    if not conc_files:
        raise SystemExit(f"no grid_conc_* files in {raw_dir}")

    header = None
    for endian in ("<", ">"):
        try:
            header = read_header(header_path, endian)
            break
        except (ValueError, struct.error):
            continue
    if header is None:
        raise SystemExit(f"could not parse oracle header: {header_path}")

    first = read_slice(conc_files[0], header, header["endian"])
    last = read_slice(conc_files[-1], header, header["endian"])
    first_weighted = weighted_sums(first, header)
    last_weighted = weighted_sums(last, header)
    mass_g = release_mass_g(Path(args.releases))

    summary = {
        "status": "ORACLE_DECODED_NO_PARITY_VERDICT",
        "header": header,
        "raw_files": sorted(p.name for p in raw_dir.iterdir() if p.is_file()),
        "raw_sha256": {p.name: sha256(p) for p in sorted(raw_dir.iterdir()) if p.is_file()},
        "first_file": conc_files[0].name,
        "last_file": conc_files[-1].name,
        "file_count": len(conc_files),
        "release_mass_g": mass_g,
        "first_slice": {
            "level_sums_native": first["level_sums_native"],
            "dry_sum_native": sum(first["dry_cells"]),
            "wet_sum_native": sum(first["wet_cells"]),
            "weighted": first_weighted,
        },
        "last_slice": {
            "level_sums_native": last["level_sums_native"],
            "dry_sum_native": sum(last["dry_cells"]),
            "wet_sum_native": sum(last["wet_cells"]),
            "weighted": last_weighted,
        },
        "weighted_note": "Mass-proportional reservoir scalars: concentration "
        "weighted by layer thickness and cos(latitude), deposition by "
        "cos(latitude). Shared proportionality per run; same-grid ratios "
        "cancel the grid spacing.",
        "mass_proportional_fractions": mass_proportional_fractions(
            last["conc_cells"], header
        ),
        "mass_proportional_layout": "(ix * ny + iy) * nz + iz",
        "mass_proportional_note": "Last-slice concentration weighted by layer "
        "thickness and cos(latitude), normalized to sum 1. Raw grid sums are "
        "not mass-conserving across windows when the plume redistributes "
        "vertically; always use these weighted fractions.",
        "vertical_profile_native": last["level_sums_native"],
        "center_of_mass": center_of_mass(last["conc_cells"], header),
        "units_note": "Native FLEXPART grid sums (f32 sparse packing). "
        "Deposition cells hold mass per area, concentration cells mass per "
        "volume; the comparator weights both by cos(latitude) (plus layer "
        "thickness for concentration) into mass-proportional quantities.",
    }
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(f"Oracle summary: {output}")


if __name__ == "__main__":
    main()
