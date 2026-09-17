"""Readers for pinned FLEXPART 11.1 oracle artifacts.

Only the unmodified oracle at the commit pinned in
``reference/flexpart-11.1.json`` may be used. These readers never modify the
oracle checkout; they fail with ``FileNotFoundError`` or ``ValueError`` on
missing or malformed artifacts instead of interpolating or substituting
values.

Sparse decoding adopts the value-sign run detection from
``scripts/compare_concentrations.py``: each run keeps a constant value sign
and physical values are ``abs(value)``. The run-index parity shortcut used
by older ETEX helpers is not used here.
"""

import struct
from pathlib import Path


def read_fortran_record(stream, endian="<"):
    """Read one Fortran unformatted sequential record or None at EOF."""
    raw = stream.read(4)
    if len(raw) < 4:
        return None
    (rec_len,) = struct.unpack(f"{endian}i", raw)
    if rec_len < 0:
        raise ValueError(f"negative record length {rec_len}; wrong endianness?")
    data = stream.read(rec_len)
    if len(data) < rec_len:
        raise ValueError(f"truncated record: expected {rec_len} bytes, got {len(data)}")
    trailer = stream.read(4)
    if len(trailer) < 4:
        raise ValueError("truncated record trailer")
    (suffix,) = struct.unpack(f"{endian}i", trailer)
    if suffix != rec_len:
        raise ValueError(f"record length mismatch: header={rec_len}, trailer={suffix}")
    return data


def read_header(header_path, endian="<"):
    """Parse the binary FLEXPART ``header`` file into grid metadata."""
    with open(header_path, "rb") as stream:
        rec = read_fortran_record(stream, endian)
        if rec is None or len(rec) < 8:
            raise ValueError("header record 1 is missing")
        ibdate, ibtime = struct.unpack(f"{endian}2i", rec[:8])

        rec = read_fortran_record(stream, endian)
        loutstep, loutaver, loutsample = struct.unpack(f"{endian}3i", rec[:12])

        rec = read_fortran_record(stream, endian)
        outlon0, outlat0 = struct.unpack(f"{endian}2f", rec[:8])
        numxgrid, numygrid = struct.unpack(f"{endian}2i", rec[8:16])
        dxout, dyout = struct.unpack(f"{endian}2f", rec[16:24])

        rec = read_fortran_record(stream, endian)
        (numzgrid,) = struct.unpack(f"{endian}i", rec[:4])
        import array

        outheights = list(struct.unpack(f"{endian}{numzgrid}f", rec[4:4 + numzgrid * 4]))

        rec = read_fortran_record(stream, endian)  # release date
        rec = read_fortran_record(stream, endian)
        n3spec, maxpointspec_act = struct.unpack(f"{endian}2i", rec[:8])
        nspec = n3spec // 3
        for _ in range(n3spec):
            read_fortran_record(stream, endian)
        rec = read_fortran_record(stream, endian)
        (numpoint,) = struct.unpack(f"{endian}i", rec[:4])
        for _ in range(numpoint):
            for _ in range(4 + 3 * nspec):
                read_fortran_record(stream, endian)
        read_fortran_record(stream, endian)  # method record
        rec = read_fortran_record(stream, endian)
        (nageclass,) = struct.unpack(f"{endian}i", rec[:4])
    del array
    return {
        "numxgrid": numxgrid,
        "numygrid": numygrid,
        "numzgrid": numzgrid,
        "outlon0": float(outlon0),
        "outlat0": float(outlat0),
        "dxout": float(dxout),
        "dyout": float(dyout),
        "outheights": [float(v) for v in outheights],
        "nspec": nspec,
        "maxpointspec_act": maxpointspec_act,
        "nageclass": nageclass,
        "loutstep": loutstep,
        "loutaver": loutaver,
        "loutsample": loutsample,
        "ibdate": ibdate,
        "ibtime": ibtime,
    }


def read_header_with_endian(header_path):
    """Try little-endian then big-endian header parsing."""
    last_error = None
    for endian in ("<", ">"):
        try:
            return read_header(header_path, endian), endian
        except (ValueError, struct.error) as exc:
            last_error = exc
            continue
    raise ValueError(f"could not parse Fortran header {header_path}: {last_error}")


def _decode_sparse_block(records, idx, endian="<"):
    rec = records[idx]
    (sp_count_i,) = struct.unpack(f"{endian}i", rec[:4])
    idx += 1
    rec = records[idx]
    if sp_count_i > 0:
        count = min(sp_count_i, len(rec) // 4)
        indices = list(struct.unpack(f"{endian}{count}i", rec[:count * 4]))
    else:
        indices = []
    idx += 1
    rec = records[idx]
    (sp_count_r,) = struct.unpack(f"{endian}i", rec[:4])
    idx += 1
    rec = records[idx]
    if sp_count_r > 0:
        count = min(sp_count_r, len(rec) // 4)
        values = list(struct.unpack(f"{endian}{count}f", rec[:count * 4]))
    else:
        values = []
    idx += 1
    return indices, values, idx


def reconstruct_grid_from_sparse(indices, values, total_cells):
    """Reconstruct a flat field from FLEXPART sparse run encoding."""
    grid = [0.0] * total_cells
    if not indices or not values:
        return grid
    val_offset = 0
    for run_idx, start_cell in enumerate(indices):
        next_start = indices[run_idx + 1] if run_idx + 1 < len(indices) else total_cells
        if val_offset >= len(values):
            break
        run_sign = 1.0 if values[val_offset] >= 0 else -1.0
        cell = int(start_cell)
        while val_offset < len(values) and cell < next_start:
            current_sign = 1.0 if values[val_offset] >= 0 else -1.0
            if current_sign != run_sign:
                break
            if 0 <= cell < total_cells:
                grid[cell] = abs(float(values[val_offset]))
            val_offset += 1
            cell += 1
    return grid


def read_grid_conc(filepath, header, endian="<"):
    """Read a ``grid_conc_*`` file into an (nx, ny, nz) nested list.

    The returned layout is ``grid[ix][iy][iz]`` with ``kz >= 1`` mapped to
    ``iz = kz - 1``. Only the concentration field is returned; wet and dry
    deposition blocks are skipped.
    """
    nx = header["numxgrid"]
    ny = header["numygrid"]
    nz = header["numzgrid"]
    nkp = header["maxpointspec_act"]
    nage = header["nageclass"]
    total_flat = (nz + 1) * nx * ny
    with open(filepath, "rb") as stream:
        records = []
        while True:
            rec = read_fortran_record(stream, endian)
            if rec is None:
                break
            records.append(rec)
    if not records:
        raise ValueError(f"empty concentration file: {filepath}")
    idx = 1  # skip itime record
    conc_flat = [0.0] * total_flat
    for _ in range(nkp):
        for _ in range(nage):
            _, _, idx = _decode_sparse_block(records, idx, endian)  # wet
            _, _, idx = _decode_sparse_block(records, idx, endian)  # dry
            conc_indices, conc_values, idx = _decode_sparse_block(records, idx, endian)
            block = reconstruct_grid_from_sparse(conc_indices, conc_values, total_flat)
            for i, v in enumerate(block):
                conc_flat[i] += v
    grid = [[[0.0 for _ in range(nz)] for _ in range(ny)] for _ in range(nx)]
    for kz in range(1, nz + 1):
        for jy in range(ny):
            for ix in range(nx):
                flat_idx = ix + jy * nx + kz * nx * ny
                grid[ix][jy][kz - 1] = conc_flat[flat_idx]
    return grid


def flatten_grid_nx_ny_nz(grid):
    """Flatten an (nx, ny, nz) nested list to row-major flat order."""
    flat = []
    for ix in range(len(grid)):
        for iy in range(len(grid[ix])):
            for iz in range(len(grid[ix][iy])):
                flat.append(float(grid[ix][iy][iz]))
    return flat


def find_grid_conc_files(output_dir):
    """Sorted ``grid_conc_*`` paths in a Fortran output directory."""
    directory = Path(output_dir)
    files = sorted(p for p in directory.glob("grid_conc_*") if p.is_file())
    if not files:
        raise FileNotFoundError(f"no grid_conc_* files in {output_dir}")
    return files


def read_partposit(filepath, endian="<"):
    """Read a Fortran ``partposit_*`` dump into lon/lat/z lists."""
    lons, lats, zs = [], [], []
    with open(filepath, "rb") as stream:
        first = read_fortran_record(stream, endian)
        if first is None:
            return [], [], []
        while True:
            rec = read_fortran_record(stream, endian)
            if rec is None:
                break
            if len(rec) < 16:
                break
            (npoint,) = struct.unpack(f"{endian}i", rec[:4])
            if npoint <= 0:
                break
            xlon, ylat, z = struct.unpack(f"{endian}3f", rec[4:16])
            lons.append(float(xlon))
            lats.append(float(ylat))
            zs.append(float(z))
    return lons, lats, zs


def find_partposit_files(output_dir):
    """Sorted ``partposit_*`` paths; empty list when the oracle wrote none."""
    directory = Path(output_dir)
    return sorted(p for p in directory.glob("partposit_*") if p.is_file())


def parse_header_txt(path):
    """Parse a FLEXPART ``header_txt`` file into grid metadata."""
    with open(path, encoding="utf-8") as stream:
        lines = [line.strip() for line in stream]
    info = {}
    for i, line in enumerate(lines):
        if "outlon0" in line and i + 1 < len(lines):
            parts = lines[i + 1].split()
            info["outlon0"] = float(parts[0])
            info["outlat0"] = float(parts[1])
            info["nx"] = int(parts[2])
            info["ny"] = int(parts[3])
            info["dx"] = float(parts[4])
            info["dy"] = float(parts[5])
        if "numzgrid, outheight" in line and i + 1 < len(lines):
            parts = lines[i + 1].split()
            info["nz"] = int(parts[0])
            info["outheights"] = [float(x) for x in parts[1:]]
        if "interval, averaging" in line and i + 1 < len(lines):
            parts = lines[i + 1].split()
            info["interval_s"] = int(parts[0])
            info["averaging_s"] = int(parts[1])
            info["sampling_s"] = int(parts[2])
        if "ibdate" in line and i + 1 < len(lines):
            parts = lines[i + 1].split()
            info["ibdate"] = parts[0]
            info["ibtime"] = parts[1]
    required = ("outlon0", "outlat0", "nx", "ny", "nz", "dx", "dy",
                "outheights", "interval_s", "averaging_s", "sampling_s")
    missing = [key for key in required if key not in info]
    if missing:
        raise ValueError(f"header_txt lacks {missing}: {path}")
    return info


def read_grid_conc_txt_format(path, nx, ny, nz):
    """Read a sparse ``grid_conc`` file in the header_txt (ETEX) layout."""
    with open(path, "rb") as stream:
        data = stream.read()
    if len(data) < 20:
        raise ValueError(f"concentration file is incomplete: {path}")

    def record(offset, kind):
        (rec_len,) = struct.unpack_from("<i", data, offset)
        offset += 4
        count = rec_len // 4
        if count > 0:
            fmt = f"<{count}i" if kind == "i" else f"<{count}f"
            values = list(struct.unpack_from(fmt, data, offset))
        else:
            values = []
        return values, offset + rec_len + 4

    offset = 0
    _, offset = record(offset, "i")  # timestamp
    for _ in range(2):  # wet + dry deposition blocks
        _, offset = record(offset, "i")
        _, offset = record(offset, "i")
        _, offset = record(offset, "i")
        _, offset = record(offset, "f")
    _, offset = record(offset, "i")
    indices, offset = record(offset, "i")
    _, offset = record(offset, "i")
    values, offset = record(offset, "f")

    total_size = (nz + 1) * ny * nx
    flat = [0.0] * total_size
    val_idx = 0
    for run_idx, run_start in enumerate(indices):
        pos = int(run_start)
        expected_positive = (run_idx % 2 == 0)
        while val_idx < len(values):
            v = values[val_idx]
            if (v > 0) != expected_positive and not (v == 0 and expected_positive):
                if v != 0:
                    break
            if 0 <= pos < total_size:
                flat[pos] = abs(float(v))
            pos += 1
            val_idx += 1
            if val_idx < len(values):
                nxt = values[val_idx]
                if (nxt > 0) != expected_positive and nxt != 0:
                    break
    # Reshape to [ix][iy][iz] with kz >= 1.
    grid = [[[0.0 for _ in range(nz)] for _ in range(ny)] for _ in range(nx)]
    for kz in range(1, nz + 1):
        base = kz * nx * ny
        for jy in range(ny):
            for ix in range(nx):
                grid[ix][jy][kz - 1] = flat[base + jy * nx + ix]
    return grid


def read_dates(dates_path):
    """Read a FLEXPART ``dates`` file into a list of timestamp strings."""
    with open(dates_path, encoding="utf-8") as stream:
        dates = [line.strip() for line in stream if line.strip()]
    if not dates:
        raise ValueError(f"dates file is empty: {dates_path}")
    return dates
