"""Readers for Issue #6 test-corpus artifacts (point 2, parallel agent).

The corpus runner (``src/bin/corpus-run.rs``) writes one ``seed_*.json``
file per Philox seed under ``target/corpus/candidate/<CASE>/``. Each file
carries the case id, seed index, Philox key/counter, adapter identity,
candidate revision, per-particle records (``lon_deg``, ``lat_deg``, ``z_m``,
``mass_kg``) and the runner's own ``metrics`` block. Oracle outputs land
under ``target/corpus/oracle/<CASE>/`` in standard FLEXPART layout and are
read with :mod:`io_fortran`.

Unit note: the corpus runner reports horizontal covariance and eigenvalues
in m^2; this evaluation reports them in km^2 (see :mod:`metrics`). Convert
with 1 km^2 = 1e6 m^2 when cross-checking the two implementations.

These readers never modify fixtures or the scenario runner; missing files
raise instead of being interpolated.
"""

import json

M2_PER_KM2 = 1e6


def _require_keys(mapping, keys, label):
    missing = [key for key in keys if key not in mapping]
    if missing:
        raise ValueError(f"{label} lacks keys {missing}")


def read_seed_file(path):
    """Read one corpus candidate ``seed_*.json`` file."""
    with open(path, encoding="utf-8") as stream:
        data = json.load(stream)
    _require_keys(data, ("case_id", "seed_index", "philox_key", "philox_counter",
                         "adapter", "candidate_revision", "particles", "metrics"),
                  f"corpus seed file {path}")
    if not isinstance(data["particles"], list) or not data["particles"]:
        raise ValueError(f"corpus seed file has no particles: {path}")
    for record in data["particles"]:
        _require_keys(record, ("lon_deg", "lat_deg", "z_m", "mass_kg"),
                      f"particle record in {path}")
    return data


def seed_particles(seed_data):
    """Split one seed file into lon/lat/z/mass lists."""
    lons = [float(p["lon_deg"]) for p in seed_data["particles"]]
    lats = [float(p["lat_deg"]) for p in seed_data["particles"]]
    heights = [float(p["z_m"]) for p in seed_data["particles"]]
    masses = [float(p["mass_kg"]) for p in seed_data["particles"]]
    return lons, lats, heights, masses


def read_case_definition(path):
    """Read a canonical v2 corpus case input (``fixtures/corpus/cases/*.json``).

    Only ``schema_version`` 2 is accepted; legacy v1 (``seeds``/``version``)
    is frozen and rejected (see ``fixtures/corpus/cases/MIGRATION_NOTES.md``).
    """
    with open(path, encoding="utf-8") as stream:
        data = json.load(stream)
    if data.get("schema_version") != 2 or "version" in data:
        raise ValueError(
            f"corpus case {path} is not a canonical v2 document "
            "(schema_version 2 required; v1 is frozen)"
        )
    _require_keys(data, ("case_id", "release", "physics_switches", "stochastic", "domain"),
                  f"corpus case {path}")
    return data
