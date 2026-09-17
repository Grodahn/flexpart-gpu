"""Readers for flexpart-gpu candidate artifacts.

Two candidate layouts are supported:

- ``fortran-validation`` single-window output: top-level ``grid``,
  ``window_start_epoch_seconds``/``window_end_epoch_seconds``,
  ``averaging_seconds``/``sampling_seconds``/``samples``/``endpoint_weight``,
  ``particle_count_per_cell``, ``concentration_mass_kg`` (kg, row-major
  ``((ix * ny) + iy) * nz + iz``) and optional ``particle_z_stats``.
- ``etex-run`` multi-window output: top-level ``grid``, ``averaging_seconds``,
  ``sampling_seconds`` and ``timesteps`` with per-window
  ``window_start_epoch_seconds``, ``epoch_seconds``, ``samples``,
  ``concentration_mass_kg`` and ``particle_count_per_cell``.

Mass-to-concentration conversion for ETEX surface fields adopts
``scripts/etex/compare_gpu_obs.py``: kilograms per cell divided by the
latitude-dependent cell volume, scaled to pg/m3. Bilinear station
interpolation adopts the same helper's boundary rule (None outside
``[0, n-1)``).
"""

import json
import math

EARTH_RADIUS_M = 6_371_000.0


def _require_keys(mapping, keys, label):
    missing = [key for key in keys if key not in mapping]
    if missing:
        raise ValueError(f"{label} lacks keys {missing}")


def read_fortran_validation_json(path):
    """Read a ``fortran-validation`` candidate JSON file."""
    with open(path, encoding="utf-8") as stream:
        data = json.load(stream)
    _require_keys(data, ("grid", "particle_count_per_cell", "concentration_mass_kg",
                         "window_start_epoch_seconds", "window_end_epoch_seconds",
                         "averaging_seconds", "sampling_seconds", "samples",
                         "endpoint_weight"), f"candidate {path}")
    grid = data["grid"]
    _require_keys(grid, ("nx", "ny", "nz", "dx", "dy", "xlon0", "ylat0", "heights_m"),
                  "candidate grid")
    expected = grid["nx"] * grid["ny"] * grid["nz"]
    if len(data["particle_count_per_cell"]) != expected:
        raise ValueError("candidate particle-count field has the wrong shape")
    if len(data["concentration_mass_kg"]) != expected:
        raise ValueError("candidate mass field has the wrong shape")
    return data


def read_etex_gpu_json(path):
    """Read an ``etex-run`` candidate JSON file with per-window fields."""
    with open(path, encoding="utf-8") as stream:
        data = json.load(stream)
    _require_keys(data, ("grid", "timesteps", "averaging_seconds", "sampling_seconds"),
                  f"ETEX candidate {path}")
    grid = data["grid"]
    _require_keys(grid, ("nx", "ny", "nz", "dx", "dy", "xlon0", "ylat0", "heights_m"),
                  "ETEX candidate grid")
    expected = grid["nx"] * grid["ny"] * grid["nz"]
    averaging = data["averaging_seconds"]
    sampling = data["sampling_seconds"]
    if averaging <= 0 or sampling <= 0 or averaging % sampling != 0:
        raise ValueError("invalid ETEX candidate averaging/sampling intervals")
    if not data["timesteps"]:
        raise ValueError("ETEX candidate output has no concentration windows")
    for step in data["timesteps"]:
        _require_keys(step, ("window_start_epoch_seconds", "epoch_seconds", "samples",
                             "concentration_mass_kg"), "ETEX candidate timestep")
        if step["epoch_seconds"] - step["window_start_epoch_seconds"] != averaging:
            raise ValueError("ETEX candidate concentration window is incomplete")
        if step["samples"] != averaging // sampling + 1:
            raise ValueError("ETEX candidate concentration window is incomplete")
        if len(step["concentration_mass_kg"]) != expected:
            raise ValueError("ETEX candidate concentration field has the wrong shape")
        for value in step["concentration_mass_kg"]:
            if not math.isfinite(value) or value < 0:
                raise ValueError("ETEX candidate field has non-finite or negative mass")
    return data


def cell_volume_m3(lat_deg, dx_deg, dy_deg, dz_m):
    """Volume from FLEXPART outgrid_mod.f90's latitude-zone area."""
    south = lat_deg - dy_deg / 2.0
    north = lat_deg + dy_deg / 2.0
    if dx_deg <= 0 or dy_deg <= 0 or dz_m <= 0 or south < -90 or north > 90:
        raise ValueError("output cell has invalid bounds or dimensions")
    zone_height = (math.radians(dy_deg) if south < 0 < north else
                   math.sin(math.radians(north)) - math.sin(math.radians(south)))
    return EARTH_RADIUS_M ** 2 * math.radians(dx_deg) * zone_height * dz_m


def layer_thicknesses_m(heights_m):
    """Output-layer thicknesses in metres from OUTHEIGHTS layer tops."""
    heights = [float(v) for v in heights_m]
    if any(h <= 0 for h in heights):
        raise ValueError("output heights must be positive")
    if any(b <= a for a, b in zip(heights, heights[1:])):
        raise ValueError("output heights must be strictly increasing")
    thicknesses = [heights[0]]
    thicknesses.extend(b - a for a, b in zip(heights, heights[1:]))
    return thicknesses


def mass_to_concentration_kg_m3(mass_flat, nx, ny, nz, xlon0_deg, ylat0_deg,
                                dx_deg, dy_deg, heights_m):
    """Convert a candidate mass field (kg) to concentration (kg/m3).

    The flat input and output use row-major ``((ix * ny) + iy) * nz + iz``
    order. Cell volumes use latitude-dependent widths and OUTHEIGHTS layer
    thicknesses. Comparing concentration against concentration (rather than
    mass against concentration) removes grid-geometry weighting from the
    shape diagnostic; the mass budget in kilograms is evaluated separately.
    """
    mass = [float(v) for v in mass_flat]
    if len(mass) != nx * ny * nz:
        raise ValueError("candidate mass field has the wrong shape")
    thicknesses = layer_thicknesses_m(heights_m)
    concentration = [0.0] * len(mass)
    for ix in range(nx):
        for iy in range(ny):
            lat = ylat0_deg + (iy + 0.5) * dy_deg
            for iz in range(nz):
                volume = cell_volume_m3(lat, dx_deg, dy_deg, thicknesses[iz])
                flat = ((ix * ny) + iy) * nz + iz
                concentration[flat] = mass[flat] / volume
    for value in concentration:
        if not math.isfinite(value) or value < 0:
            raise ValueError("candidate concentration has non-finite or negative values")
    return concentration


def concentration_to_mass_per_cell(concentration_flat, nx, ny, nz,
                                   ylat0_deg, dx_deg, dy_deg, heights_m):
    """Reconstruct relative FLEXPART cell mass from concentration output."""
    concentration = [float(v) for v in concentration_flat]
    if len(concentration) != nx * ny * nz:
        raise ValueError("oracle concentration field has the wrong shape")
    thicknesses = layer_thicknesses_m(heights_m)
    mass = [0.0] * len(concentration)
    for ix in range(nx):
        for iy in range(ny):
            lat = ylat0_deg + (iy + 0.5) * dy_deg
            for iz in range(nz):
                flat = ((ix * ny) + iy) * nz + iz
                value = concentration[flat]
                if not math.isfinite(value) or value < 0:
                    raise ValueError("oracle concentration must be finite and non-negative")
                mass[flat] = value * cell_volume_m3(
                    lat, dx_deg, dy_deg, thicknesses[iz])
    return mass


def gpu_mass_to_surface_concentration_pg_m3(mass_flat, nx, ny, nz, dx_deg, dy_deg,
                                            xlon0_deg, ylat0_deg, heights_m):
    """Convert candidate mass (kg) to a surface concentration grid (pg/m3).

    Returns a ``[iy][ix]`` nested list for the lowest output level, whose
    depth is ``heights_m[0]`` metres. The flat input uses row-major
    ``((ix * ny) + iy) * nz + iz`` order.
    """
    mass = [float(v) for v in mass_flat]
    if len(mass) != nx * ny * nz:
        raise ValueError("candidate mass field has the wrong shape")
    dz_surface = float(heights_m[0])
    if dz_surface <= 0:
        raise ValueError("surface layer depth must be positive")
    surface = [[0.0 for _ in range(nx)] for _ in range(ny)]
    for iy in range(ny):
        lat = ylat0_deg + (iy + 0.5) * dy_deg
        volume = cell_volume_m3(lat, dx_deg, dy_deg, dz_surface)
        for ix in range(nx):
            flat = (ix * ny + iy) * nz + 0
            surface[iy][ix] = mass[flat] * 1e12 / volume
    for row in surface:
        for value in row:
            if not math.isfinite(value) or value < 0:
                raise ValueError("candidate surface field has non-finite or negative values")
    return surface


def bilinear_interpolate(grid_iy_ix, xlon0_deg, ylat0_deg, dx_deg, dy_deg,
                         nx, ny, lon_deg, lat_deg):
    """Bilinear interpolation of a ``[iy][ix]`` grid, or None outside."""
    ix = (lon_deg - xlon0_deg) / dx_deg
    iy = (lat_deg - ylat0_deg) / dy_deg
    if ix < 0 or ix >= nx - 1 or iy < 0 or iy >= ny - 1:
        return None
    ix0 = int(ix)
    iy0 = int(iy)
    fx = ix - ix0
    fy = iy - iy0
    return (
        (1 - fx) * (1 - fy) * grid_iy_ix[iy0][ix0]
        + fx * (1 - fy) * grid_iy_ix[iy0][ix0 + 1]
        + (1 - fx) * fy * grid_iy_ix[iy0 + 1][ix0]
        + fx * fy * grid_iy_ix[iy0 + 1][ix0 + 1]
    )


def read_measurements_json(path):
    """Read parsed ETEX measurements (``parse_measurements.py`` output)."""
    with open(path, encoding="utf-8") as stream:
        data = json.load(stream)
    if "measurements" not in data:
        raise ValueError(f"measurements file lacks records: {path}")
    return data
