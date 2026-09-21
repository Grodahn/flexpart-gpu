#!/usr/bin/env python3
"""Generate versioned Fortran oracle inputs for the Issue #6 test corpus.

Every oracle release/grid setting is derived from the matching candidate
case JSON under fixtures/corpus/cases/*.json so that both programs start
from demonstrably equal inputs:

- OUTGRID (OUTLON0/OUTLAT0/NUMXGRID/NUMYGRID/DXOUT/DYOUT/OUTHEIGHTS) is
  driven exclusively by the case ``output_grid`` contract.
- RELEASES (LON/LAT/Z/PARTS) mirrors the case ``release`` exactly.
- RELEASES MASS is the case total mass converted from kilograms (candidate
  unit) to grams (FLEXPART unit): ``MASS_g = mass_kg * 1000``.
- COMMAND dates follow the case ``integration`` window; turbulence,
  convection and timestepping switches follow ``oracle_command_overrides``;
  direction (LDIRECT) and output timing (LOUTSTEP/LOUTAVER/LOUTSAMPLE)
  follow the required ``simulation_direction`` and ``output`` blocks. The
  raw Python path fails closed when any of these blocks is missing: the
  generator never supplies a hidden direction or output default.
- METEO_ARGS.txt records the exact synthetic-GRIB generator flags derived
  from the case wind/surface entries (single reference path:
  scripts/generate_synthetic_grib.py).

The inert tracer SPECIES is copied verbatim from the pinned upstream
checkout. The depositing SPECIES is derived from the upstream aerosol
example by dropping exactly the PNDIA key unknown to the v11.1 namelist
(see depositing_species_text), with provenance recorded per case. Each case
directory also records INPUT_DERIVATION.json with the source values and
conversion applied, and the generator re-parses every written file and
fails loudly when a derived value drifts from the case.

Usage:
    python3 scripts/corpus/generate_fortran_fixtures.py --flexpart-dir ../flexpart
"""

import argparse
import copy
import hashlib
import json
import math
import re
import shutil
from datetime import datetime, timedelta
from pathlib import Path

from validation_case_schema import ValidationCaseSchemaError, validate_case_document

REPO = Path(__file__).resolve().parents[2]
CORPUS = REPO / "fixtures" / "corpus"
CASES = CORPUS / "cases"
FORTRAN_OUT = CORPUS / "fortran"

# FLEXPART RELEASES MASS is in grams; candidate case masses are in kilograms.
KG_TO_G = 1000.0
MASS_CONSISTENCY_TOLERANCE_REL = 1e-6

ORACLE_EXECUTION_PROFILE_ID = "flexpart-11.1-single-thread"
ORACLE_EXECUTION_PROFILE_VERSION = 1
ORACLE_EXECUTION_PROFILE_PATH = "reference/flexpart-11.1.json"

SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_ID = "flexpart-synthetic-grib-v1"
SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_VERSION = 1
SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_PATH = "reference/oracle-meteorology/synthetic-grib-v1.json"
REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_ID = "real-weather-manifest-v1"
REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_VERSION = 1
REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_PATH = "reference/oracle-meteorology/real-weather-manifest-v1.json"

ORACLE_STOCHASTIC_STRATEGY_ID = "flexpart-oracle-validation-seed-offset"
ORACLE_STOCHASTIC_STRATEGY_VERSION = 1
ORACLE_STOCHASTIC_CONTRACT_PATH = "reference/oracle-stochastic-identity.json"
PHILOX_DERIVATION_WRAPPING_ADD_KEY0_V1 = "wrapping_add_key0_v1"
PHILOX_DERIVATION_REUSE_BASE_IDENTITY_V1 = "reuse_base_identity_v1"
SUPPORTED_PHILOX_DERIVATIONS = {
    PHILOX_DERIVATION_WRAPPING_ADD_KEY0_V1,
    PHILOX_DERIVATION_REUSE_BASE_IDENTITY_V1,
}

SPECIES_PHYSICS_CONTRACTS = {
    "species_024_inert_v1": {
        "species_id": "SPECIES_024",
        "id": "species-024-inert-v1",
        "version": 1,
        "path": "reference/species-physics/species-024-inert-v1.json",
        "git_blob_sha": "21dd5ccd2b616642be9e3989e9b7444774d97fd9",
        "dry_deposition": False,
        "wet_deposition": False,
        "decay": False,
    },
    "species_040_dry_constant_v1": {
        "species_id": "SPECIES_040",
        "id": "species-040-dry-constant-v1",
        "version": 1,
        "path": "reference/species-physics/species-040-dry-constant-v1.json",
        "git_blob_sha": "c050d6351244beaf2b1a57f661f2321101bda6f1",
        "dry_deposition": True,
        "wet_deposition": False,
        "decay": False,
    },
    "species_040_wet_aerosol_v1": {
        "species_id": "SPECIES_040",
        "id": "species-040-wet-aerosol-v1",
        "version": 1,
        "path": "reference/species-physics/species-040-wet-aerosol-v1.json",
        "git_blob_sha": "4f55d23294f320f0050581cfad14800b22bf141d",
        "dry_deposition": False,
        "wet_deposition": True,
        "decay": False,
    },
}

CANONICAL_UNIT_VALUES = {
    "wind": "m/s",
    "displacement": "m",
    "pressure": "Pa",
    "temperature": "K",
    "heat_flux": "W/m2",
    "height": "m",
    "mass": "kg",
    "time": "s",
    "shear": "1/s",
    "inv_obukhov": "1/m",
    "deposition_velocity": "m/s",
    "scavenging_coefficient": "1/s",
    "concentration": "kg/m3",
}

# Standard concentration output levels [m] shared by all synthetic corpus
# cases. The output grid is independent of the wind-field levels; one fixed
# set keeps oracle comparison grids identical across cases.
STANDARD_OUTHEIGHTS = [
    100.0, 250.0, 500.0, 750.0, 1000.0,
    1500.0, 2000.0, 2500.0, 3000.0, 5000.0,
]

RECEPTORS_ZERO = """*******************************************************************************
*                                                                             *
*  Input file for the Lagrangian particle dispersion model FLEXPART           *
*                        Please specify receptor points                       *
*******************************************************************************
                0
"""

RESTART_NOTE = (
    "Oracle-only restart illustration: rerun with IPIN=1 and LOUTRESTART set "
    "after a first run that wrote restart_* files (restart_mod.f90). "
    "No paired candidate restart exists (candidate has no restart API).\n"
)


def _timestamp14(value, case_id: str, field: str) -> str:
    if not isinstance(value, str) or len(value) != 14 or not value.isdigit():
        raise SystemExit(
            f"{case_id}: {field} must be YYYYMMDDHHMMSS (14 digits), got {value!r}"
        )
    try:
        datetime.strptime(value, "%Y%m%d%H%M%S")
    except ValueError as exc:
        raise SystemExit(
            f"{case_id}: {field} is not a valid Gregorian YYYYMMDDHHMMSS timestamp: "
            f"{value!r} ({exc})"
        ) from exc
    return value


def case_total_mass_kg(case_id: str, case: dict) -> float:
    release = _required_release(case_id, case)
    inventory = release.get("inventory")
    if not isinstance(inventory, dict):
        raise SystemExit(f"{case_id}: release.inventory must be an object, got {inventory!r}")
    if inventory.get("unit") != "kg":
        raise SystemExit(
            f"{case_id}: release.inventory.unit must be 'kg', got {inventory.get('unit')!r}"
        )
    quantity = _finite_number(
        inventory.get("quantity_kg"), case_id, "release.inventory.quantity_kg"
    )
    if quantity <= 0:
        raise SystemExit(
            f"{case_id}: release.inventory.quantity_kg must be finite and > 0, got {quantity!r}"
        )
    return quantity


def species_number_for_case(case_id: str, case: dict) -> int:
    release = _required_release(case_id, case)
    species = release.get("species")
    if not isinstance(species, dict):
        raise SystemExit(f"{case_id}: release.species must be an object, got {species!r}")
    species_id = species.get("id")
    if not isinstance(species_id, str) or not species_id.startswith("SPECIES_"):
        raise SystemExit(
            f"{case_id}: release.species.id {species_id!r} must match SPECIES_<NNN>"
        )
    digits = species_id[len("SPECIES_"):]
    if len(digits) != 3 or not digits.isdigit():
        raise SystemExit(
            f"{case_id}: release.species.id {species_id!r} must match SPECIES_<NNN>"
        )
    return int(digits)


def _validate_species_physics_contract(case_id: str, case: dict, physics: dict) -> str:
    release = _required_release(case_id, case)
    species = release.get("species")
    if not isinstance(species, dict):
        raise SystemExit(f"{case_id}: release.species must be an object")
    contract = species.get("physics_contract")
    if not isinstance(contract, dict):
        raise SystemExit(
            f"{case_id}: release.species.physics_contract must be an object"
        )
    profile = contract.get("profile")
    expected = SPECIES_PHYSICS_CONTRACTS.get(profile)
    if expected is None:
        raise SystemExit(
            f"{case_id}: unsupported release.species.physics_contract.profile {profile!r}"
        )
    for field in ("id", "version", "path", "git_blob_sha"):
        if contract.get(field) != expected[field]:
            raise SystemExit(
                f"{case_id}: release.species.physics_contract.{field} "
                f"{contract.get(field)!r} does not match canonical {profile} "
                f"value {expected[field]!r}"
            )
    if species.get("id") != expected["species_id"]:
        raise SystemExit(
            f"{case_id}: release.species.id {species.get('id')!r} conflicts with "
            f"{profile}, expected {expected['species_id']!r}"
        )
    for field in ("dry_deposition", "wet_deposition", "decay"):
        if physics[field] != expected[field]:
            raise SystemExit(
                f"{case_id}: physics_switches.{field}={physics[field]!r} conflicts "
                f"with species physics profile {profile} ({expected[field]!r})"
            )
    return profile


def _validate_deposition_contract(case_id: str, case: dict, physics: dict) -> None:
    active = physics["dry_deposition"] or physics["wet_deposition"]
    deposition = case.get("deposition")
    if not active:
        if deposition is not None:
            raise SystemExit(
                f"{case_id}: deposition block must be absent when dry/wet deposition are off"
            )
        return
    if not isinstance(deposition, dict):
        raise SystemExit(
            f"{case_id}: deposition block is required when dry/wet deposition is active"
        )
    dry_velocity = _finite_number(
        deposition.get("dry_deposition_velocity_m_s"),
        case_id,
        "deposition.dry_deposition_velocity_m_s",
    )
    wet_coeff = _finite_number(
        deposition.get("wet_scavenging_coefficient_s_inv"),
        case_id,
        "deposition.wet_scavenging_coefficient_s_inv",
    )
    wet_fraction = _finite_number(
        deposition.get("wet_precipitating_fraction"),
        case_id,
        "deposition.wet_precipitating_fraction",
    )
    if physics["dry_deposition"]:
        href = _finite_number(
            deposition.get("dry_reference_height_m"),
            case_id,
            "deposition.dry_reference_height_m",
        )
        if dry_velocity <= 0 or href <= 0:
            raise SystemExit(
                f"{case_id}: active dry deposition requires positive velocity and reference height"
            )
    elif dry_velocity != 0:
        raise SystemExit(
            f"{case_id}: dry_deposition_velocity_m_s must be 0 when dry deposition is disabled"
        )
    if physics["wet_deposition"]:
        if wet_coeff <= 0 or not (0 < wet_fraction <= 1):
            raise SystemExit(
                f"{case_id}: active wet deposition requires positive coefficient and precipitating fraction"
            )
    elif wet_coeff != 0 or wet_fraction != 0:
        raise SystemExit(
            f"{case_id}: wet deposition forcing must be zero when wet deposition is disabled"
        )


def release_window_datetimes(case_id: str, case: dict) -> tuple:
    release = _required_release(case_id, case)
    timing = release.get("timing")
    if not isinstance(timing, dict):
        raise SystemExit(f"{case_id}: release.timing must be an object, got {timing!r}")

    integration = _required_integration(case_id, case)
    sim_start = datetime.strptime(integration["start"], "%Y%m%d%H%M%S")
    sim_end = sim_start + timedelta(seconds=integration["total_s"])

    kind = timing.get("kind")
    if kind == "instant":
        stamp = _timestamp14(timing.get("at"), case_id, "release.timing.at")
        release_start = release_end = datetime.strptime(stamp, "%Y%m%d%H%M%S")
        start = end = stamp
    elif kind == "window":
        start = _timestamp14(timing.get("start"), case_id, "release.timing.start")
        end = _timestamp14(timing.get("end"), case_id, "release.timing.end")
        release_start = datetime.strptime(start, "%Y%m%d%H%M%S")
        release_end = datetime.strptime(end, "%Y%m%d%H%M%S")
        if release_end < release_start:
            raise SystemExit(
                f"{case_id}: release.timing.end {end} must be >= start {start}"
            )
    else:
        raise SystemExit(f"{case_id}: unknown release.timing.kind {kind!r}")

    if release_start < sim_start or release_end > sim_end:
        raise SystemExit(
            f"{case_id}: release timing lies outside simulation window "
            f"{integration['start']} + {integration['total_s']}s"
        )
    return start, end


def flexpart_datetime(stamp: str) -> tuple:
    return int(stamp[0:8]), int(stamp[8:14])


def release_vertical(case_id: str, case: dict) -> tuple:
    release = _required_release(case_id, case)
    if release.get("vertical_ref") != "agl":
        raise SystemExit(
            f"{case_id}: vertical_ref {release.get('vertical_ref')!r} has no "
            "FLEXPART ZKIND mapping (only agl -> ZKIND=1 is established)"
        )
    geometry = release.get("geometry")
    if not isinstance(geometry, dict):
        raise SystemExit(f"{case_id}: release.geometry must be an object, got {geometry!r}")
    kind = geometry.get("kind")
    if kind == "point":
        z = _finite_number(geometry.get("z_m"), case_id, "release.geometry.z_m")
        if z < 0:
            raise SystemExit(f"{case_id}: release.geometry.z_m must be >= 0 for AGL")
        return z, z, 1
    if kind == "box":
        z_min = _finite_number(geometry.get("z_min_m"), case_id, "release.geometry.z_min_m")
        z_max = _finite_number(geometry.get("z_max_m"), case_id, "release.geometry.z_max_m")
        if z_min < 0 or z_max < 0:
            raise SystemExit(f"{case_id}: release box heights must be >= 0 for AGL")
        if z_max < z_min:
            raise SystemExit(
                f"{case_id}: release.geometry.z_max_m {z_max} must be >= z_min_m {z_min}"
            )
        return z_min, z_max, 1
    raise SystemExit(f"{case_id}: unknown release.geometry.kind {kind!r}")


def _checked_lon(value, case_id: str, field: str) -> float:
    lon = _finite_number(value, case_id, field)
    if not -180.0 <= lon <= 360.0:
        raise SystemExit(f"{case_id}: {field} must be in [-180, 360], got {lon}")
    return lon


def _checked_lat(value, case_id: str, field: str) -> float:
    lat = _finite_number(value, case_id, field)
    if not -90.0 <= lat <= 90.0:
        raise SystemExit(f"{case_id}: {field} must be in [-90, 90], got {lat}")
    return lat


def release_lonlat(case_id: str, case: dict) -> tuple:
    release = _required_release(case_id, case)
    geometry = release.get("geometry")
    if not isinstance(geometry, dict):
        raise SystemExit(f"{case_id}: release.geometry must be an object, got {geometry!r}")
    kind = geometry.get("kind")
    if kind == "point":
        lon = _checked_lon(geometry.get("lon_deg"), case_id, "release.geometry.lon_deg")
        lat = _checked_lat(geometry.get("lat_deg"), case_id, "release.geometry.lat_deg")
        return lon, lon, lat, lat
    if kind == "box":
        lon_min = _checked_lon(geometry.get("lon_min_deg"), case_id, "release.geometry.lon_min_deg")
        lon_max = _checked_lon(geometry.get("lon_max_deg"), case_id, "release.geometry.lon_max_deg")
        lat_min = _checked_lat(geometry.get("lat_min_deg"), case_id, "release.geometry.lat_min_deg")
        lat_max = _checked_lat(geometry.get("lat_max_deg"), case_id, "release.geometry.lat_max_deg")
        if lon_max < lon_min:
            raise SystemExit(f"{case_id}: release.geometry.lon_max_deg must be >= lon_min_deg")
        if lat_max < lat_min:
            raise SystemExit(f"{case_id}: release.geometry.lat_max_deg must be >= lat_min_deg")
        return lon_min, lon_max, lat_min, lat_max
    raise SystemExit(f"{case_id}: unknown release.geometry.kind {kind!r}")


def _validate_release_contract(case_id: str, case: dict, domain: dict) -> dict:
    release = _required_release(case_id, case)
    count = _required_particle_count(case_id, release)
    total = case_total_mass_kg(case_id, case)
    species_number_for_case(case_id, case)
    release_window_datetimes(case_id, case)
    lon1, lon2, lat1, lat2 = release_lonlat(case_id, case)
    z1, z2, _ = release_vertical(case_id, case)

    per_particle = release.get("mass_kg_per_particle")
    if per_particle is not None:
        per_particle = _finite_number(
            per_particle, case_id, "release.mass_kg_per_particle"
        )
        if per_particle <= 0:
            raise SystemExit(
                f"{case_id}: release.mass_kg_per_particle must be finite and > 0"
            )
        implied = per_particle * count
        if abs((implied - total) / total) > MASS_CONSISTENCY_TOLERANCE_REL:
            raise SystemExit(
                f"{case_id}: release.mass_kg_per_particle x particle_count implies "
                f"{implied}, inconsistent with inventory {total}"
            )

    if "require_source_containment" not in case:
        raise SystemExit(
            f"{case_id}: require_source_containment is required explicitly"
        )
    containment = case["require_source_containment"]
    if not isinstance(containment, bool):
        raise SystemExit(
            f"{case_id}: require_source_containment must be a boolean, got {containment!r}"
        )
    if containment:
        heights = domain.get("wind_heights_m")
        if not isinstance(heights, list) or not heights:
            raise SystemExit(
                f"{case_id}: domain.wind_heights_m must be a non-empty array for source containment"
            )
        height_values = [
            _finite_number(v, case_id, "domain.wind_heights_m") for v in heights
        ]
        lon_min = float(domain["xlon0_deg"])
        lat_min = float(domain["ylat0_deg"])
        dx = float(domain["dx_deg"])
        dy = float(domain["dy_deg"])
        max_grid_x_exclusive = int(domain["nx"]) - 1
        max_grid_y_exclusive = int(domain["ny"]) - 1
        lon_max_exclusive = lon_min + max_grid_x_exclusive * dx
        lat_max_exclusive = lat_min + max_grid_y_exclusive * dy
        height_min, height_max = height_values[0], height_values[-1]
        eps = 1e-6
        for label, lon, lat, z in (("min", lon1, lat1, z1), ("max", lon2, lat2, z2)):
            grid_x = (lon - lon_min) / dx
            if grid_x < 0 or grid_x >= max_grid_x_exclusive:
                raise SystemExit(
                    f"{case_id}: release geometry {label} longitude {lon} outside runtime "
                    f"domain [{lon_min}, {lon_max_exclusive})"
                )
            grid_y = (lat - lat_min) / dy
            if grid_y < 0 or grid_y >= max_grid_y_exclusive:
                raise SystemExit(
                    f"{case_id}: release geometry {label} latitude {lat} outside runtime "
                    f"domain [{lat_min}, {lat_max_exclusive})"
                )
            if not height_min - eps <= z <= height_max + eps:
                raise SystemExit(
                    f"{case_id}: release geometry {label} height {z} outside domain levels "
                    f"[{height_min}, {height_max}]"
                )
    return release


def sim_end_date(start: str, total_s: int) -> tuple:
    """Derive (IEDATE, IETIME) from a validated Gregorian start plus seconds."""
    start_dt = datetime.strptime(start, "%Y%m%d%H%M%S")
    end_dt = start_dt + timedelta(seconds=total_s)
    return int(end_dt.strftime("%Y%m%d")), int(end_dt.strftime("%H%M%S"))


def _required_output_grid(case_id: str, case: dict):
    """Validate the explicit concentration/comparison grid.

    Every v2 case declares this block. No synthetic fallback to ``domain`` or
    process-local output-height list is permitted.
    """
    grid = case.get("output_grid")
    if grid is None:
        raise SystemExit(f"{case_id}: output_grid is required explicitly")
    if not isinstance(grid, dict):
        raise SystemExit(f"{case_id}: output_grid must be an object, got {grid!r}")
    for field in ("nx", "ny", "nz"):
        value = grid.get(field)
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise SystemExit(
                f"{case_id}: output_grid.{field} must be a positive integer, got {value!r}"
            )
    for field in ("xlon0_deg", "ylat0_deg", "dx_deg", "dy_deg"):
        value = _finite_number(grid.get(field), case_id, f"output_grid.{field}")
        if field in ("dx_deg", "dy_deg") and value <= 0:
            raise SystemExit(
                f"{case_id}: output_grid.{field} must be > 0, got {value!r}"
            )
    if grid.get("horizontal_ref") != "geographic_lon_lat_degrees":
        raise SystemExit(
            f"{case_id}: output_grid.horizontal_ref must be 'geographic_lon_lat_degrees'"
        )
    if grid.get("heights_ref") != "agl":
        raise SystemExit(
            f"{case_id}: output_grid.heights_ref must be 'agl' in schema v2; ASL conversion is not implemented"
        )
    heights = grid.get("heights_m")
    if not isinstance(heights, list) or len(heights) != grid["nz"]:
        raise SystemExit(
            f"{case_id}: output_grid.heights_m length must equal output_grid.nz"
        )
    values = [_finite_number(v, case_id, "output_grid.heights_m") for v in heights]
    if any(v < 0 for v in values) or any(b <= a for a, b in zip(values, values[1:])):
        raise SystemExit(
            f"{case_id}: output_grid.heights_m must be finite, >= 0 and strictly increasing"
        )
    return grid


def outgrid_text(case: dict) -> str:
    """Render FLEXPART OUTGRID exclusively from explicit output_grid."""
    grid = case.get("output_grid")
    if not isinstance(grid, dict):
        raise SystemExit("output_grid is required explicitly before rendering OUTGRID")
    heights_values = grid["heights_m"]
    heights = ", ".join(f"{h:6.1f}" for h in heights_values) + ","
    return (
        "&OUTGRID\n"
        f" OUTLON0=   {grid['xlon0_deg']:7.2f},\n"
        f" OUTLAT0=   {grid['ylat0_deg']:7.2f},\n"
        f" NUMXGRID=   {grid['nx']:7d},\n"
        f" NUMYGRID=   {grid['ny']:7d},\n"
        f" DXOUT=     {grid['dx_deg']:7.2f},\n"
        f" DYOUT=     {grid['dy_deg']:7.2f},\n"
        f" OUTHEIGHTS=  {heights}\n"
        " /\n"
    )


# Canonical (lowercase) Oracle override fields. The generated Fortran
# namelist keeps uppercase spelling; only the JSON/Rust representation is
# canonical lowercase. Legacy uppercase JSON keys are frozen and rejected
# (see fixtures/corpus/cases/MIGRATION_NOTES.md): every checked-in case is
# migrated to v2. `turbulence_formulation` is the typed Issue #67 contract
# decision: FLEXPART derives the dispersion method and Markov-chain
# formulation from the sign/magnitude of CTL; schema v2 records the actual
# closed mode explicitly instead of inferring it from a numeric threshold.
CANONICAL_ORACLE_FIELDS = (
    "turbulence_formulation",
    "lturbulence",
    "lconvection",
    "ctl",
    "ifine",
    "lsynctime_s",
)
LEGACY_ORACLE_FIELDS = {
    "LTURBULENCE": "lturbulence",
    "LCONVECTION": "lconvection",
    "CTL": "ctl",
    "IFINE": "ifine",
    "LSYNCTIME": "lsynctime_s",
}
REQUIRED_ORACLE_FIELDS = (
    "turbulence_formulation",
    "lturbulence",
    "lconvection",
    "ctl",
    "ifine",
    "lsynctime_s",
)
FLAG_ORACLE_FIELDS = ("lturbulence", "lconvection")

# Only real FLEXPART COMMAND switches are cross-checked here. Dry/wet
# deposition and decay are species/release physics, not COMMAND namelist keys.
PHYSICS_AGREEMENT = (
    ("lturbulence", "turbulence"),
    ("lconvection", "convection"),
)

# Turbulence/integration formulation contract (Issue #67), pinned to
# reference/flexpart-11.1.json (commit c70586c). FLEXPART derives two coupled
# behaviours from the COMMAND CTL value:
#
# - dispersion method (readoptions_mod.f90:786-795): CTL > 0 selects the
#   adaptive particle-timestep method (method=1, mintime=minstep); CTL <= 0
#   selects the fixed-timestep method (method=0, mintime=lsynctime);
# - Markov-chain formulation (readoptions_mod.f90:626,645-650): CTL >= 0.1
#   selects the w/sigw formulation (turbswitch=.true.); CTL < 0.1 silently
#   selects the w formulation and forces ifine=1.
#
# Schema v2 supports the two formulations actually present in the checked-in
# corpus: synthetic cases use adaptive w/sigw, while ETEX-MINI-013 preserves
# its historical fixed-timestep / w configuration. The declared formulation
# must agree with CTL; no mode is inferred by case ID.
SUPPORTED_TURBULENCE_FORMULATIONS = frozenset(
    {"adaptive_w_sigw", "fixed_sync_w"}
)

# CTL >= this value keeps the pinned oracle's w/sigw Markov formulation
# (turbswitch=.true., readoptions_mod.f90:645-650); below it the oracle
# silently switches to the w formulation and forces ifine=1. Single documented
# constant shared with the Rust validator (CTL_W_SIGW_FORMULATION_THRESHOLD).
CTL_FORMULATION_THRESHOLD = 0.1

# Simulation direction contract (Issue #51 / #57): the manifest carries the
# typed semantic value; the generator maps it to the FLEXPART numeric key
# (readoptions_mod.f90: `ldirect contains direction of time forward (1) or
# backward(-1)`). The raw LDIRECT value is a derived namelist artifact, never
# a manifest input, so no numeric default exists on the manifest side.
SIMULATION_DIRECTIONS = frozenset({"forward", "backward"})
LDIRECT_FORWARD = 1
LDIRECT_BACKWARD = -1

# Schema-v2 deliberately supports only forward concentration runs. FLEXPART
# backward runs are valid, but IOUT=1 then represents source-receptor /
# residence-time semantics rather than the concentration quantity modeled by
# OutputQuantity. Keep the enum value recognizable so the error can name the
# unsupported mode, but fail closed before rendering a backward COMMAND.
SUPPORTED_SIMULATION_DIRECTIONS = frozenset({"forward"})

# Closed scientific output semantics mirrored by Rust OutputQuantity.
SUPPORTED_OUTPUT_QUANTITIES = frozenset(
    {"time_averaged_mass_concentration_kg_m3"}
)

# Output timing contract (Issue #51 / #57): every COMMAND output key is
# derived from the manifest `output` block (Interval_s -> LOUTSTEP,
# Averaging_window_s -> LOUTAVER, Sampling_interval_s -> LOUTSAMPLE).
# `_required_output` is the single reading/validation path.


# Physics switches are mandatory in the canonical contract (the Rust
# `PhysicsSwitches` struct is a required member, not an `Option`). The raw
# Python path enforces the same shape so Oracle/physics agreement checks
# never degrade into a skipped comparison on malformed input.
PHYSICS_SWITCH_FIELDS = (
    "turbulence",
    "convection",
    "dry_deposition",
    "wet_deposition",
    "decay",
)

# Wind profiles recognized by the synthetic-GRIB flag builder (Rust
# `WindSpec` is a required tagged union, not a defaultable block).
WIND_PROFILES = frozenset({"uniform", "linear_shear", "real_weather"})

# Synthetic-GRIB surface defaults, applied ONLY when surface data is
# legitimately absent (surface: null with turbulence and deposition declared
# off, mirroring the Rust validate_physics_consistency contract). Named
# constants so a truthiness collapse can never silently reintroduce them.
ANALYTIC_DEFAULT_SENSIBLE_HEAT_FLUX_W_M2 = 40.0
ANALYTIC_DEFAULT_MIXING_HEIGHT_M = 1500.0
ANALYTIC_DEFAULT_PRECIP_LARGE_SCALE_MM_H = 0.0
ANALYTIC_DEFAULT_PRECIP_CONVECTIVE_MM_H = 0.0


def _finite_number(value, case_id: str, field: str) -> float:
    """Fail-closed JSON number reader: a real number, never a bool, finite."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise SystemExit(f"{case_id}: {field} must be a finite number, got {value!r}")
    number = float(value)
    if not math.isfinite(number):
        raise SystemExit(f"{case_id}: {field} must be finite, got {value!r}")
    return number


def _normalized_repo_path(value, case_id: str, field: str, *, allow_trailing_slash=False) -> str:
    if not isinstance(value, str) or not value:
        raise SystemExit(f"{case_id}: {field} must be a non-empty repository-relative path")
    if (
        value.startswith("/")
        or value.startswith(".")
        or "\\" in value
        or "//" in value
        or any(part in (".", "..") for part in value.split("/"))
        or (value.endswith("/") and not allow_trailing_slash)
    ):
        raise SystemExit(
            f"{case_id}: {field} must be a normalized repository-relative path, got {value!r}"
        )
    return value


def _required_execution_profile(case_id: str, case: dict) -> dict:
    profile = case.get("execution_profile")
    if not isinstance(profile, dict):
        raise SystemExit(f"{case_id}: execution_profile must be an object")
    expected = {
        "id": ORACLE_EXECUTION_PROFILE_ID,
        "version": ORACLE_EXECUTION_PROFILE_VERSION,
        "manifest_path": ORACLE_EXECUTION_PROFILE_PATH,
    }
    if profile != expected:
        raise SystemExit(
            f"{case_id}: execution_profile must reference frozen #49 profile "
            f"{expected!r}, got {profile!r}"
        )
    _normalized_repo_path(
        profile["manifest_path"], case_id, "execution_profile.manifest_path"
    )
    return profile


def _required_release(case_id: str, case: dict) -> dict:
    """Fail-closed release reader: the normalized release block must exist."""
    release = case.get("release")
    if not isinstance(release, dict):
        raise SystemExit(f"{case_id}: release must be an object, got {release!r}")
    return release


def _required_wind(case_id: str, case: dict) -> dict:
    """Fail-closed wind reader: an object with a recognized profile.

    Rust `WindSpec` declares a required tagged union; `json.loads` alone must
    not treat a missing/non-object wind block as an implicit uniform wind.
    """
    wind = case.get("wind")
    if not isinstance(wind, dict):
        raise SystemExit(f"{case_id}: wind must be an object, got {wind!r}")
    profile = wind.get("profile")
    if not isinstance(profile, str) or profile not in WIND_PROFILES:
        raise SystemExit(
            f"{case_id}: unsupported wind.profile {profile!r}; supported: "
            f"{', '.join(sorted(WIND_PROFILES))}"
        )
    return wind


def _required_integration(case_id: str, case: dict) -> dict:
    """Fail-closed integration reader shared by COMMAND, RELEASES and met coverage."""
    integration = case.get("integration")
    if not isinstance(integration, dict):
        raise SystemExit(f"{case_id}: integration must be an object, got {integration!r}")

    start = _timestamp14(integration.get("start"), case_id, "integration.start")
    dt_s = _finite_number(integration.get("dt_s"), case_id, "integration.dt_s")
    if dt_s <= 0:
        raise SystemExit(f"{case_id}: integration.dt_s must be > 0, got {dt_s!r}")

    steps = integration.get("steps")
    if isinstance(steps, bool) or not isinstance(steps, int) or steps <= 0:
        raise SystemExit(
            f"{case_id}: integration.steps must be a positive integer, got {steps!r}"
        )

    total_s = _finite_number(integration.get("total_s"), case_id, "integration.total_s")
    if total_s != int(total_s):
        raise SystemExit(
            f"{case_id}: integration.total_s must be whole seconds, got {total_s!r}"
        )
    if total_s <= 0:
        raise SystemExit(
            f"{case_id}: integration.total_s must be positive, got {total_s!r}"
        )
    expected_total = dt_s * steps
    if not math.isfinite(expected_total) or abs(total_s - expected_total) > 1e-6:
        raise SystemExit(
            f"{case_id}: integration.total_s {total_s} must equal "
            f"dt_s * steps ({dt_s} * {steps} = {expected_total})"
        )
    try:
        datetime.strptime(start, "%Y%m%d%H%M%S") + timedelta(seconds=int(total_s))
    except OverflowError as exc:
        raise SystemExit(
            f"{case_id}: simulation end exceeds Gregorian datetime range for "
            f"start={start!r} total_s={total_s!r}"
        ) from exc
    return {
        "start": start,
        "dt_s": dt_s,
        "steps": steps,
        "total_s": int(total_s),
    }


def _required_simulation_direction(case_id: str, case: dict) -> str:
    """Fail-closed simulation direction reader (Issue #51 / #57).

    The manifest carries the typed semantic value (`forward`/`backward`);
    the FLEXPART numeric `LDIRECT` is derived from it (readoptions_mod.f90).
    A missing or unknown direction is rejected: no direction may be
    substituted in the generator.
    """
    direction = case.get("simulation_direction")
    if direction not in SIMULATION_DIRECTIONS:
        raise SystemExit(
            f"{case_id}: simulation_direction must be one of "
            f"{', '.join(sorted(SIMULATION_DIRECTIONS))}, got {direction!r} "
            "(the FLEXPART LDIRECT numeric key is derived, never defaulted)"
        )
    if direction not in SUPPORTED_SIMULATION_DIRECTIONS:
        raise SystemExit(
            f"{case_id}: simulation_direction={direction!r} is a valid FLEXPART "
            "mode but is deliberately unsupported by schema v2: the current "
            "output.quantity models forward time-averaged mass concentration, "
            "while backward IOUT=1 uses source-receptor/residence-time semantics"
        )
    return direction


def _required_output(case_id: str, case: dict, sync_s: int) -> dict:
    """Fail-closed output timing validated against the declared LSYNCTIME."""
    output = case.get("output")
    if not isinstance(output, dict):
        raise SystemExit(
            f"{case_id}: output must be an object, got {output!r} "
            "(no hidden output default substituted)"
        )

    def seconds(name: str) -> int:
        value = output.get(name)
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise SystemExit(
                f"{case_id}: output.{name} must be a whole positive number of "
                f"seconds, got {value!r}"
            )
        number = float(value)
        if not math.isfinite(number):
            raise SystemExit(
                f"{case_id}: output.{name} must be finite, got {value!r}"
            )
        if number != int(number) or number <= 0:
            raise SystemExit(
                f"{case_id}: output.{name} must be a whole positive number of "
                f"seconds, got {value!r}"
            )
        return int(number)

    interval_s = seconds("interval_s")
    averaging_window_s = seconds("averaging_window_s")
    sampling_interval_s = seconds("sampling_interval_s")

    if sampling_interval_s > averaging_window_s:
        raise SystemExit(
            f"{case_id}: output.sampling_interval_s ({sampling_interval_s}s) "
            f"must not exceed output.averaging_window_s ({averaging_window_s}s); "
            "FLEXPART LOUTSAMPLE <= LOUTAVER"
        )
    if averaging_window_s > interval_s:
        raise SystemExit(
            f"{case_id}: output.averaging_window_s ({averaging_window_s}s) "
            f"must not exceed output.interval_s ({interval_s}s); "
            "FLEXPART LOUTAVER <= LOUTSTEP"
        )

    if isinstance(sync_s, bool) or not isinstance(sync_s, int) or sync_s <= 0:
        raise SystemExit(
            f"{case_id}: oracle_command_overrides.lsynctime_s must be a positive integer, "
            f"got {sync_s!r}"
        )
    for name, value in (
        ("interval_s", interval_s),
        ("averaging_window_s", averaging_window_s),
        ("sampling_interval_s", sampling_interval_s),
    ):
        if value % sync_s != 0:
            raise SystemExit(
                f"{case_id}: output.{name} ({value}s) must be a multiple of "
                f"the declared FLEXPART LSYNCTIME={sync_s}s"
            )
    if averaging_window_s < 2 * sync_s:
        raise SystemExit(
            f"{case_id}: output.averaging_window_s ({averaging_window_s}s) "
            f"must be at least 2*LSYNCTIME ({2 * sync_s}s)"
        )
    if interval_s < 2 * sync_s:
        raise SystemExit(
            f"{case_id}: output.interval_s ({interval_s}s) must be at least "
            f"2*LSYNCTIME ({2 * sync_s}s)"
        )

    quantity = output.get("quantity")
    if quantity not in SUPPORTED_OUTPUT_QUANTITIES:
        raise SystemExit(
            f"{case_id}: unsupported output.quantity {quantity!r}; supported: "
            f"{', '.join(sorted(SUPPORTED_OUTPUT_QUANTITIES))}"
        )
    return {
        "interval_s": interval_s,
        "averaging_window_s": averaging_window_s,
        "sampling_interval_s": sampling_interval_s,
        "quantity": quantity,
    }


def _validate_stochastic_contract(case_id: str, case: dict, physics: dict) -> None:
    stochastic = case.get("stochastic")
    if not isinstance(stochastic, dict):
        raise SystemExit(f"{case_id}: stochastic must be an object")
    for field in ("candidate_philox", "oracle_seed"):
        if field not in stochastic:
            raise SystemExit(
                f"{case_id}: stochastic.{field} is required explicitly; use null "
                "when that model has no stochastic identity"
            )

    candidate = stochastic["candidate_philox"]
    if candidate is not None:
        if not isinstance(candidate, dict):
            raise SystemExit(f"{case_id}: stochastic.candidate_philox must be an object or null")

        def require_u32_array(field, length):
            value = candidate.get(field)
            if (
                not isinstance(value, list)
                or len(value) != length
                or any(
                    isinstance(item, bool)
                    or not isinstance(item, int)
                    or item < 0
                    or item >= 2**32
                    for item in value
                )
            ):
                raise SystemExit(
                    f"{case_id}: stochastic.candidate_philox.{field} must contain "
                    f"exactly {length} unsigned 32-bit integers"
                )
            return value

        require_u32_array("base_key", 2)
        require_u32_array("base_counter", 4)
        count = candidate.get("count")
        if isinstance(count, bool) or not isinstance(count, int) or count <= 0:
            raise SystemExit(f"{case_id}: stochastic.candidate_philox.count must be > 0")
        derivation = candidate.get("derivation")
        if derivation not in SUPPORTED_PHILOX_DERIVATIONS:
            raise SystemExit(
                f"{case_id}: unsupported stochastic.candidate_philox.derivation "
                f"{derivation!r}; supported={sorted(SUPPORTED_PHILOX_DERIVATIONS)}"
            )
        if "identical_repeats" in candidate:
            raise SystemExit(
                f"{case_id}: legacy stochastic.candidate_philox.identical_repeats "
                "is forbidden; encode semantics in derivation"
            )

    oracle = stochastic["oracle_seed"]
    if oracle is not None:
        if not isinstance(oracle, dict):
            raise SystemExit(f"{case_id}: stochastic.oracle_seed must be an object or null")
        allowed = {"kind", "strategy", "mode", "seed", "repetitions"}
        unknown = sorted(set(oracle) - allowed)
        if unknown:
            raise SystemExit(
                f"{case_id}: unknown stochastic.oracle_seed field(s): {', '.join(unknown)}"
            )
        for field in ("kind", "strategy", "mode", "seed", "repetitions"):
            if field not in oracle:
                raise SystemExit(
                    f"{case_id}: stochastic.oracle_seed.{field} is required explicitly; "
                    "null is distinct from omission"
                )
        repetitions = oracle["repetitions"]
        if isinstance(repetitions, bool) or not isinstance(repetitions, int) or repetitions <= 0:
            raise SystemExit(f"{case_id}: stochastic.oracle_seed.repetitions must be > 0")
        kind = oracle["kind"]
        mode = oracle["mode"]
        seed = oracle["seed"]
        strategy = oracle["strategy"]
        if kind == "pristine-oracle":
            if strategy is not None:
                raise SystemExit(f"{case_id}: pristine-oracle requires strategy=null")
            if mode != "default":
                raise SystemExit(f"{case_id}: pristine-oracle requires mode='default'")
            if seed is not None:
                raise SystemExit(f"{case_id}: pristine-oracle default mode requires seed=null")
        elif kind == "seedable-validation-oracle":
            expected = {
                "strategy": ORACLE_STOCHASTIC_STRATEGY_ID,
                "version": ORACLE_STOCHASTIC_STRATEGY_VERSION,
                "contract_path": ORACLE_STOCHASTIC_CONTRACT_PATH,
            }
            if strategy != expected:
                raise SystemExit(
                    f"{case_id}: seedable-validation-oracle requires the exact #50 strategy "
                    f"reference {expected!r}, got {strategy!r}"
                )
            if mode == "default":
                if seed is not None:
                    raise SystemExit(
                        f"{case_id}: seedable-validation-oracle mode='default' requires seed=null"
                    )
            elif mode == "requested_identity":
                if (
                    isinstance(seed, bool)
                    or not isinstance(seed, int)
                    or not 1 <= seed <= 1_000_000_000
                ):
                    raise SystemExit(
                        f"{case_id}: mode='requested_identity' requires "
                        "stochastic.oracle_seed.seed in [1, 1000000000]"
                    )
            else:
                raise SystemExit(
                    f"{case_id}: stochastic.oracle_seed.mode must be 'default' or "
                    f"'requested_identity', got {mode!r}"
                )
        else:
            raise SystemExit(f"{case_id}: unsupported stochastic.oracle_seed.kind {kind!r}")

    if physics["turbulence"] and candidate is None:
        raise SystemExit(
            f"{case_id}: physics_switches.turbulence=true requires "
            "stochastic.candidate_philox; oracle_seed is a separate RNG namespace "
            "and cannot satisfy the candidate requirement"
        )


def _required_units(case_id: str, case: dict, wind: dict, surface, physics: dict) -> dict:
    units = case.get("units")
    if not isinstance(units, dict):
        raise SystemExit(f"{case_id}: units must be an object")
    unknown = sorted(set(units) - set(CANONICAL_UNIT_VALUES))
    if unknown:
        raise SystemExit(f"{case_id}: unknown units field(s): {', '.join(unknown)}")

    required = {"wind", "height", "mass", "time", "concentration"}
    if surface is not None:
        required.update({"pressure", "temperature", "heat_flux", "inv_obukhov"})
    if wind["profile"] == "linear_shear":
        required.add("shear")
    if physics["dry_deposition"]:
        required.add("deposition_velocity")
    if physics["wet_deposition"]:
        required.add("scavenging_coefficient")

    for field in required:
        expected = CANONICAL_UNIT_VALUES[field]
        if units.get(field) != expected:
            raise SystemExit(
                f"{case_id}: units.{field} must be {expected!r}, got {units.get(field)!r}"
            )
    for field, actual in units.items():
        expected = CANONICAL_UNIT_VALUES[field]
        if actual != expected:
            raise SystemExit(
                f"{case_id}: units.{field} must be {expected!r}, got {actual!r}"
            )
    return units


def _required_validation_definition_refs(case_id: str, case: dict) -> dict:
    refs = case.get("validation_definition_refs")
    if not isinstance(refs, dict):
        raise SystemExit(f"{case_id}: validation_definition_refs must be an object")
    if set(refs) != {"metric_contracts", "threshold_contracts"}:
        raise SystemExit(
            f"{case_id}: validation_definition_refs must contain exactly "
            "metric_contracts and threshold_contracts"
        )
    for field in ("metric_contracts", "threshold_contracts"):
        values = refs.get(field)
        if not isinstance(values, list) or not values:
            raise SystemExit(f"{case_id}: validation_definition_refs.{field} must be non-empty")
        for ref in values:
            if not isinstance(ref, dict) or set(ref) != {"id", "version", "path"}:
                raise SystemExit(
                    f"{case_id}: validation_definition_refs.{field} entries require id/version/path"
                )
            for key in ("id", "version", "path"):
                if not isinstance(ref[key], str) or not ref[key]:
                    raise SystemExit(
                        f"{case_id}: validation_definition_refs.{field}.{key} must be non-empty"
                    )
            if ref["path"].startswith("/") or ".." in ref["path"].split("/"):
                raise SystemExit(
                    f"{case_id}: validation definition path must be repository-relative "
                    f"without '..': {ref['path']!r}"
                )
    return refs


def mandatory_physics_switches(case_id: str, case: dict) -> dict:
    """Fail-closed physics_switches reader used by the raw Python path.

    The Python generator does not run the Rust ValidationCaseManifest
    validator, so it independently enforces the canonical schema-v2 physics
    contract: one object, exactly the five switch fields, every value a
    strict boolean. A malformed or missing block raises instead of collapsing
    to None, so Oracle-vs-physics agreement checks are never skipped on input
    type alone.
    """
    physics = case.get("physics_switches")
    if not isinstance(physics, dict):
        raise SystemExit(
            f"{case_id}: physics_switches must be an object with "
            f"{', '.join(PHYSICS_SWITCH_FIELDS)}, got {physics!r}"
        )
    unknown = sorted(set(physics).difference(PHYSICS_SWITCH_FIELDS))
    if unknown:
        raise SystemExit(
            f"{case_id}: unknown physics_switches field(s) "
            f"{', '.join(repr(key) for key in unknown)}; a schema-v2 physics "
            "switch block declares exactly turbulence, convection, "
            "dry_deposition, wet_deposition, decay"
        )
    for field in PHYSICS_SWITCH_FIELDS:
        value = physics.get(field)
        if not isinstance(value, bool):
            raise SystemExit(
                f"{case_id}: physics_switches.{field} must be a boolean, got {value!r}"
            )
    return physics


def _required_particle_count(case_id: str, release: dict) -> int:
    """Fail-closed release.particle_count reader: a positive strict integer."""
    value = release.get("particle_count")
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise SystemExit(
            f"{case_id}: release.particle_count must be a positive integer, got {value!r}"
        )
    return value


def _required_domain(case_id: str, case: dict) -> dict:
    """Fail-closed domain reader for the OUTGRID/INPUT_DERIVATION fields.

    OUTGRID consumes ``xlon0_deg``, ``ylat0_deg``, ``nx``, ``ny``,
    ``dx_deg``, ``dy_deg`` via format specifiers; validating them here (once,
    in the preflight) keeps a missing or mistyped field from surfacing as a
    crash mid-write.
    """
    domain = case.get("domain")
    if not isinstance(domain, dict):
        raise SystemExit(f"{case_id}: domain must be an object, got {domain!r}")
    for field in ("xlon0_deg", "ylat0_deg", "dx_deg", "dy_deg"):
        _finite_number(domain.get(field), case_id, f"domain.{field}")
    for field in ("nx", "ny"):
        value = domain.get(field)
        if isinstance(value, bool) or not isinstance(value, int) or value < 2:
            raise SystemExit(
                f"{case_id}: domain.{field} must be an integer >= 2, got {value!r}"
            )
    value = domain.get("nz")
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise SystemExit(
            f"{case_id}: domain.nz must be a positive integer, got {value!r}"
        )
    if domain.get("wind_heights_ref") != "agl":
        raise SystemExit(
            f"{case_id}: domain.wind_heights_ref must be 'agl' in schema v2; ASL conversion is not implemented"
        )
    heights = domain.get("wind_heights_m")
    if not isinstance(heights, list) or len(heights) != domain["nz"]:
        raise SystemExit(f"{case_id}: domain.wind_heights_m length must equal domain.nz")
    parsed = [_finite_number(v, case_id, "domain.wind_heights_m") for v in heights]
    if any(b <= a for a, b in zip(parsed, parsed[1:])):
        raise SystemExit(f"{case_id}: domain.wind_heights_m must be strictly increasing")
    return domain


def _required_meteorology(case_id: str, wind: dict, integration: dict) -> dict:
    """Fail-closed reader for the real-weather meteorology METEO.txt metadata.

    The real-weather write path consumes ``dataset_id``, ``source_path``,
    ``version`` and both transformation ``script`` entries; if any is missing
    or mistyped the case must fail in preflight, not while writing fixtures.
    """
    meteorology = wind.get("meteorology")
    if not isinstance(meteorology, dict):
        raise SystemExit(
            f"{case_id}: wind.meteorology must be an object for real_weather, got {meteorology!r}"
        )
    for field in ("dataset_id", "source_path", "version", "digest"):
        value = meteorology.get(field)
        if not isinstance(value, str) or not value:
            raise SystemExit(
                f"{case_id}: wind.meteorology.{field} must be a non-empty string, got {value!r}"
            )
    _normalized_repo_path(
        meteorology["source_path"],
        case_id,
        "wind.meteorology.source_path",
        allow_trailing_slash=True,
    )
    digest = meteorology["digest"]
    if len(digest) == 64:
        if any(ch not in "0123456789abcdefABCDEF" for ch in digest):
            raise SystemExit(
                f"{case_id}: wind.meteorology.digest sha256 must be hexadecimal"
            )
    elif digest.startswith("manifest:"):
        manifest_path = digest[len("manifest:"):]
        _normalized_repo_path(
            manifest_path, case_id, "wind.meteorology.digest manifest path"
        )
    else:
        raise SystemExit(
            f"{case_id}: wind.meteorology.digest must be 64-char sha256 or "
            "manifest:<normalized repository-relative file path>"
        )
    for field in ("candidate_transformation", "oracle_transformation"):
        transformation = meteorology.get(field)
        if not isinstance(transformation, dict):
            raise SystemExit(
                f"{case_id}: wind.meteorology.{field} must be an object, got {transformation!r}"
            )
        script = transformation.get("script")
        if not isinstance(script, str) or not script:
            raise SystemExit(
                f"{case_id}: wind.meteorology.{field}.script must be a non-empty string, got {script!r}"
            )
    coverage = meteorology.get("temporal_coverage")
    if not isinstance(coverage, list) or len(coverage) != 2:
        raise SystemExit(
            f"{case_id}: wind.meteorology.temporal_coverage must contain exactly two timestamps"
        )
    coverage_start_s = _timestamp14(
        coverage[0], case_id, "wind.meteorology.temporal_coverage[0]"
    )
    coverage_end_s = _timestamp14(
        coverage[1], case_id, "wind.meteorology.temporal_coverage[1]"
    )
    coverage_start = datetime.strptime(coverage_start_s, "%Y%m%d%H%M%S")
    coverage_end = datetime.strptime(coverage_end_s, "%Y%m%d%H%M%S")
    if coverage_end < coverage_start:
        raise SystemExit(
            f"{case_id}: wind.meteorology.temporal_coverage end must be >= start"
        )
    sim_start = datetime.strptime(integration["start"], "%Y%m%d%H%M%S")
    sim_end = sim_start + timedelta(seconds=integration["total_s"])
    if coverage_start > sim_start or coverage_end < sim_end:
        raise SystemExit(
            f"{case_id}: wind.meteorology.temporal_coverage "
            f"{coverage_start_s}..{coverage_end_s} must cover the full simulation "
            f"{integration['start']} + {integration['total_s']}s"
        )
    return meteorology


def _required_surface(case_id: str, case: dict, physics: dict):
    """Validate surface semantics and return the surface block (dict or None).

    Single source for the surface contract shared by the preflight and the
    synthetic-GRIB flag builder:

    - surface must be ``null`` or an object; a non-null falsey value
      (``false``, ``0``, ``[]``, ``""``) is rejected, never normalized to ``{}``.
    - ``surface: null`` is accepted when analytic physics do not require it
      or when ``wind.profile=real_weather`` supplies time-varying surface
      fields through the meteorology contract.
    """
    real_weather = (
        isinstance(case.get("wind"), dict)
        and case["wind"].get("profile") == "real_weather"
    )
    surface_required = (
        physics["turbulence"] or physics["dry_deposition"] or physics["wet_deposition"]
    ) and not real_weather
    surface = case.get("surface")
    if surface is not None and not isinstance(surface, dict):
        raise SystemExit(
            f"{case_id}: surface must be null or an object, got {surface!r}"
        )
    if surface is None and surface_required:
        raise SystemExit(
            f"{case_id}: surface is null but the declared physics "
            "(physics_switches.turbulence/dry_deposition/wet_deposition) "
            "require surface data; refusing to substitute analytic defaults"
        )
    return surface


def real_weather_meteo_txt(meteorology: dict) -> str:
    """METEO.txt provenance header for a validated real-weather case."""
    return (
        f"# Real-weather case: meteorology from native ERA5 fixture at {meteorology['source_path']}\n"
        f"# Dataset: {meteorology['dataset_id']} version {meteorology['version']}\n"
        f"# Candidate transformation: {meteorology['candidate_transformation']['script']}\n"
        f"# Oracle transformation: {meteorology['oracle_transformation']['script']}\n"
    )


def normalize_oracle_overrides(case_id: str, case: dict) -> dict:
    """Canonical v2 Oracle override reader (no hidden defaults, no v1).

    Reads the canonical lowercase fields. A document declaring both a legacy
    uppercase key and the canonical lowercase form is ambiguous and rejected;
    a legacy-only spelling is frozen and rejected (see MIGRATION_NOTES.md), as
    are unknown keys, missing required overrides, out-of-range values, Oracle
    flags conflicting with ``physics_switches``, and any ``ctl`` value that
    contradicts the declared ``turbulence_formulation``. Never substitutes
    physics-altering defaults.
    """
    raw = case.get("oracle_command_overrides")
    if raw is None:
        raise SystemExit(
            f"{case_id}: missing oracle_command_overrides; "
            "turbulence_formulation, lturbulence, lconvection, ctl and ifine "
            "must be declared explicitly"
        )
    if not isinstance(raw, dict):
        raise SystemExit(f"{case_id}: oracle_command_overrides must be an object")
    for legacy, canonical in LEGACY_ORACLE_FIELDS.items():
        if legacy in raw and canonical in raw:
            raise SystemExit(
                f"{case_id}: ambiguous oracle override: both {legacy} and "
                f"{canonical} present; keep only the canonical lowercase form"
            )
    for key in raw:
        if key in LEGACY_ORACLE_FIELDS:
            raise SystemExit(
                f"{case_id}: legacy uppercase oracle override {key!r} is frozen "
                f"and unsupported; use the canonical lowercase form "
                f"{LEGACY_ORACLE_FIELDS[key]!r} (see MIGRATION_NOTES.md)"
            )
    normalized: dict = {}
    for field in CANONICAL_ORACLE_FIELDS:
        if field in raw:
            normalized[field] = raw[field]
    for key in raw:
        if key not in CANONICAL_ORACLE_FIELDS:
            raise SystemExit(f"{case_id}: unknown oracle override {key!r}")
    for field in REQUIRED_ORACLE_FIELDS:
        if field not in normalized:
            raise SystemExit(
                f"{case_id}: missing required oracle override {field}; "
                "no hidden default substituted"
            )
    formulation = normalized["turbulence_formulation"]
    if (
        not isinstance(formulation, str)
        or formulation not in SUPPORTED_TURBULENCE_FORMULATIONS
    ):
        raise SystemExit(
            f"{case_id}: unsupported oracle override turbulence_formulation "
            f"{formulation!r}; supported: "
            f"{', '.join(sorted(SUPPORTED_TURBULENCE_FORMULATIONS))}"
        )
    for field in FLAG_ORACLE_FIELDS:
        if field in normalized:
            value = normalized[field]
            if isinstance(value, bool) or not isinstance(value, int):
                raise SystemExit(
                    f"{case_id}: oracle override {field} must be exactly 0 or 1, "
                    f"got {value!r}"
                )
            if value not in (0, 1):
                raise SystemExit(
                    f"{case_id}: oracle override {field} must be exactly 0 or 1, "
                    f"got {value!r}"
                )
    ctl = normalized["ctl"]
    if isinstance(ctl, bool) or not isinstance(ctl, (int, float)):
        raise SystemExit(
            f"{case_id}: oracle override ctl must be a finite number, got {ctl!r}"
        )
    ctl_f = float(ctl)
    if not math.isfinite(ctl_f):
        raise SystemExit(
            f"{case_id}: oracle override ctl must be finite, got {ctl!r}"
        )
    if ctl_f == 0.0:
        raise SystemExit(
            f"{case_id}: oracle override ctl must be non-zero: the oracle computes "
            "ctl = 1./ctl unconditionally (readoptions_mod.f90:653) and sizes "
            "particle time steps from it (advance_mod.f90:557-568), "
            f"got {ctl!r}"
        )
    if formulation == "adaptive_w_sigw":
        if ctl_f < CTL_FORMULATION_THRESHOLD:
            raise SystemExit(
                f"{case_id}: oracle override ctl={ctl!r} contradicts "
                "turbulence_formulation='adaptive_w_sigw': CTL must be >= "
                f"{CTL_FORMULATION_THRESHOLD}; CTL <= 0 selects fixed-timestep "
                "method=0/mintime=lsynctime and values below the threshold select "
                "the w formulation and force effective ifine=1 "
                "(readoptions_mod.f90:645-650,786-795)"
            )
    elif formulation == "fixed_sync_w":
        if ctl_f >= 0.0:
            raise SystemExit(
                f"{case_id}: oracle override ctl={ctl!r} contradicts "
                "turbulence_formulation='fixed_sync_w': CTL must be < 0 so "
                "FLEXPART selects method=0 with mintime=lsynctime; CTL < 0.1 "
                "also selects the w formulation and forces effective ifine=1 "
                "(readoptions_mod.f90:645-650,786-795)"
            )
    ifine = normalized["ifine"]
    if isinstance(ifine, bool) or not isinstance(ifine, int):
        raise SystemExit(
            f"{case_id}: oracle override ifine must be an integer in 1..=10, "
            f"got {ifine!r}"
        )
    if not 1 <= ifine <= 10:
        raise SystemExit(
            f"{case_id}: oracle override ifine must be in 1..=10, got {ifine!r}"
        )
    lsynctime_s = normalized["lsynctime_s"]
    if isinstance(lsynctime_s, bool) or not isinstance(lsynctime_s, int) or lsynctime_s <= 0:
        raise SystemExit(
            f"{case_id}: oracle override lsynctime_s must be a positive integer, "
            f"got {lsynctime_s!r}"
        )
    physics = mandatory_physics_switches(case_id, case)
    for field, physics_key in PHYSICS_AGREEMENT:
        if field not in normalized:
            continue
        declared = physics[physics_key]
        if declared != (normalized[field] == 1):
            raise SystemExit(
                f"{case_id}: physics_switches.{physics_key}={declared} conflicts "
                f"with oracle {field}={normalized[field]}; refusing to generate"
            )
    return normalized


def command_text(case_id: str, case: dict) -> str:
    """COMMAND namelist derived from case integration and switch overrides.

    Direction and output timing are the only values taken from the
    ``simulation_direction`` and ``output`` manifest blocks; every field is
    required, so a document missing one fails closed here.
    """
    integration = _required_integration(case_id, case)
    overrides = normalize_oracle_overrides(case_id, case)
    ibdate, ibtime = flexpart_datetime(integration["start"])
    iedate, ietime = sim_end_date(integration["start"], integration["total_s"])
    ctl = float(overrides["ctl"])
    ifine = int(overrides["ifine"])
    lturbulence = int(overrides["lturbulence"])
    lconvection = int(overrides["lconvection"])
    direction = _required_simulation_direction(case_id, case)
    ldirect = LDIRECT_FORWARD if direction == "forward" else LDIRECT_BACKWARD
    output = _required_output(case_id, case, int(overrides["lsynctime_s"]))
    return (
        "&COMMAND\n"
        f" LDIRECT= {ldirect:>15},\n"
        f" IBDATE=         {ibdate},\n"
        f" IBTIME=           {ibtime:06d},\n"
        f" IEDATE=         {iedate},\n"
        f" IETIME=           {ietime:06d},\n"
        f" LOUTSTEP={output['interval_s']:>15},\n"
        f" LOUTAVER={output['averaging_window_s']:>15},\n"
        f" LOUTSAMPLE={output['sampling_interval_s']:>14},\n"
        " ITSPLIT=        99999999,\n"
        f" LSYNCTIME={int(overrides['lsynctime_s']):>15},\n"
        f" CTL=            {ctl:.7f},\n"
        f" IFINE=                 {ifine},\n"
        " IOUT=                  1,\n"
        " IPOUT=                 2,\n"
        " LSUBGRID=              0,\n"
        " NXSHIFT=               0,\n"
        " LNETCDFOUT=            0,\n"
        f" LCONVECTION=           {lconvection},\n"
        " LAGESPECTRA=           0,\n"
        " IPIN=                  0,\n"
        " IOUTPUTFOREACHRELEASE= 1,\n"
        " IFLUX=                 0,\n"
        " MDOMAINFILL=           0,\n"
        " IND_SOURCE=            1,\n"
        " IND_RECEPTOR=          1,\n"
        " MQUASILAG=             0,\n"
        " NESTED_OUTPUT=         0,\n"
        " LINIT_COND=            0,\n"
        " SURF_ONLY=             0,\n"
        " CBLFLAG=               0,\n"
        ' OHFIELDS_PATH= "../../flexin/",\n'
        f" LTURBULENCE=           {lturbulence},\n"
        " /\n"
    )


def releases_text(case_id: str, case: dict, specnum: int) -> str:
    """RELEASES namelist derived from the normalized release block.

    FLEXPART MASS is in grams; the candidate works in kilograms.
    """
    release = _required_release(case_id, case)
    mass_g = case_total_mass_kg(case_id, case) * KG_TO_G
    start_stamp, end_stamp = release_window_datetimes(case_id, case)
    idate1, itime1 = flexpart_datetime(start_stamp)
    idate2, itime2 = flexpart_datetime(end_stamp)
    lon1, lon2, lat1, lat2 = release_lonlat(case_id, case)
    z1, z2, zkind = release_vertical(case_id, case)
    particles = _required_particle_count(case_id, release)
    return (
        "&RELEASES_CTRL\n"
        " NSPEC      =           1,\n"
        f" SPECNUM_REL=          {specnum},\n"
        " /\n"
        "&RELEASE\n"
        f" IDATE1  =       {idate1},\n"
        f" ITIME1  =         {itime1:06d},\n"
        f" IDATE2  =       {idate2},\n"
        f" ITIME2  =         {itime2:06d},\n"
        f" LON1    =     {lon1:8.3f},\n"
        f" LON2    =     {lon2:8.3f},\n"
        f" LAT1    =     {lat1:8.3f},\n"
        f" LAT2    =     {lat2:8.3f},\n"
        f" Z1      =     {z1:9.3f},\n"
        f" Z2      =     {z2:9.3f},\n"
        f" ZKIND   =              {zkind},\n"
        f" MASS    =       {mass_g:.4E},\n"
        f" PARTS   =       {particles:10d},\n"
        f' COMMENT =    "{case_id}",\n'
        " /\n"
    )


def ageclass_text(case_id: str, case: dict) -> str:
    """Single age class covering the full integration window."""
    integration = _required_integration(case_id, case)
    return f"&AGECLASS\n NAGECLASS= 1,\n LAGE= {integration['total_s']},\n /\n"


def _required_oracle_meteorology_profile(case_id: str, case: dict) -> dict:
    ref = case.get("oracle_meteorology_profile")
    if not isinstance(ref, dict):
        raise SystemExit(
            f"{case_id}: oracle_meteorology_profile must be an explicit object"
        )
    wind = _required_wind(case_id, case)
    if wind["profile"] == "real_weather":
        expected = (
            REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_ID,
            REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_VERSION,
            REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_PATH,
        )
    else:
        expected = (
            SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_ID,
            SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_VERSION,
            SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_PATH,
        )
    actual = (ref.get("id"), ref.get("version"), ref.get("manifest_path"))
    if actual != expected:
        raise SystemExit(
            f"{case_id}: oracle_meteorology_profile must reference "
            f"{expected[0]} v{expected[1]} at {expected[2]}, got {actual!r}"
        )
    profile_path = REPO / expected[2]
    try:
        profile = json.loads(profile_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise SystemExit(
            f"{case_id}: cannot load oracle meteorology profile {profile_path}: {exc}"
        ) from exc
    if profile.get("id") != expected[0] or profile.get("version") != expected[1]:
        raise SystemExit(
            f"{case_id}: oracle meteorology profile file identity does not match case reference"
        )
    return profile


def meteo_args(case_id: str, case: dict) -> str:
    """Exact synthetic-GRIB generator flags derived from case wind/surface.

    Fail-closed: the raw Python path (``json.loads``, no Rust validator) must
    not be weaker than the canonical schema-v2 contract for the fields it
    consumes, so malformed blocks are rejected instead of decaying to defaults:

    - `wind` must be an object with a recognized `profile` (Rust `WindSpec`).
    - For `uniform`/`linear_shear` every consumed component must be present
      and a finite number; a missing component is never defaulted (e.g. an
      implicit 5.0 m/s u-wind).
    - `surface` must be `null` or an object. A non-null falsey value
      (`false`, `0`, `[]`, `""`) is rejected, not normalized to `{}`.
    - `surface: null` is accepted only when the declared physics do not
      require surface data (turbulence and deposition off), matching the Rust
      `validate_physics_consistency` contract. Analytic defaults are applied
      only in that legitimately-absent case.
    - For the `real_weather` profile the meteorology comes from native ERA5
      fixture data (fixtures/etex/native-mini/) and no synthetic GRIB is
      generated; this function returns an empty string.
    """
    wind = _required_wind(case_id, case)
    profile = wind["profile"]
    oracle_meteo = _required_oracle_meteorology_profile(case_id, case)
    if profile == "real_weather":
        return ""
    if profile == "uniform":
        u = _finite_number(wind.get("u_m_s"), case_id, "wind.u_m_s")
        v = _finite_number(wind.get("v_m_s"), case_id, "wind.v_m_s")
        w = _finite_number(wind.get("w_m_s"), case_id, "wind.w_m_s")
        shear = 0.0
    elif profile == "linear_shear":
        u = _finite_number(wind.get("u0_m_s"), case_id, "wind.u0_m_s")
        v = _finite_number(wind.get("v_m_s"), case_id, "wind.v_m_s")
        w = _finite_number(wind.get("w_m_s"), case_id, "wind.w_m_s")
        shear = _finite_number(wind.get("u_shear_per_s"), case_id, "wind.u_shear_per_s")
    else:
        raise SystemExit(
            f"{case_id}: unsupported wind.profile {profile!r}; supported: "
            f"{', '.join(sorted(WIND_PROFILES))}"
        )

    physics = mandatory_physics_switches(case_id, case)
    surface = _required_surface(case_id, case, physics)
    if surface is None:
        sshf = ANALYTIC_DEFAULT_SENSIBLE_HEAT_FLUX_W_M2
        blh = ANALYTIC_DEFAULT_MIXING_HEIGHT_M
        lsp = ANALYTIC_DEFAULT_PRECIP_LARGE_SCALE_MM_H
        cp = ANALYTIC_DEFAULT_PRECIP_CONVECTIVE_MM_H
    else:
        sshf = _finite_number(
            surface.get("sensible_heat_flux_w_m2"),
            case_id,
            "surface.sensible_heat_flux_w_m2",
        )
        blh = _finite_number(surface.get("mixing_height_m"), case_id, "surface.mixing_height_m")
        lsp = _finite_number(
            surface.get("precip_large_scale_mm_h"), case_id, "surface.precip_large_scale_mm_h"
        )
        cp = _finite_number(
            surface.get("precip_convective_mm_h"), case_id, "surface.precip_convective_mm_h"
        )

    grid = oracle_meteo.get("grid")
    temporal = oracle_meteo.get("temporal")
    if not isinstance(grid, dict) or not isinstance(temporal, dict):
        raise SystemExit(f"{case_id}: synthetic oracle meteorology profile lacks grid/temporal")
    integration = _required_integration(case_id, case)
    start = integration["start"]
    if temporal.get("start_time_policy") == "simulation_start_date_midnight" and start[8:] != "000000":
        raise SystemExit(
            f"{case_id}: synthetic meteorology profile requires midnight simulation start, got {start}"
        )
    cadence_s = temporal.get("cadence_s")
    if not isinstance(cadence_s, int) or cadence_s <= 0 or cadence_s % 3600 != 0:
        raise SystemExit(f"{case_id}: oracle meteorology cadence_s must be a positive whole hour")
    total_s = int(integration["total_s"])
    coverage_s = ((total_s + cadence_s - 1) // cadence_s) * cadence_s
    hours = coverage_s // 3600
    ref_path = case["oracle_meteorology_profile"]["manifest_path"]
    return (
        f"--profile {ref_path} "
        f"--nx {int(grid['nx'])} --ny {int(grid['ny'])} --nz {int(grid['nz'])} "
        f"--u-wind {u} --v-wind {v} --w-wind {w} "
        f"--u-shear-per-m {shear} --sshf {sshf} --blh {blh} "
        f"--lsp {lsp} --cp {cp} --start-date {start[:8]} --hours {hours}"
    )


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def set_namelist_key(text: str, key: str, value: str) -> tuple:
    """Replace the assignment of KEY in namelist text; fail if absent/ambiguous."""
    pattern = re.compile(rf"(?im)^(?P<indent>\s*){key}\s*=\s*[^,/\n]+(?P<tail>,?)")
    matches = list(pattern.finditer(text))
    if len(matches) != 1:
        raise SystemExit(
            f"expected exactly one {key} assignment, found {len(matches)}; "
            "upstream example changed, re-derive deliberately"
        )
    match = matches[0]
    return (
        text[: match.start()] + f"{match.group('indent')}{key}={value}{match.group('tail')}" + text[match.end():],
        match.group(0).strip(),
    )


def species_for_case(case_id: str, profile: str, tracer: Path, aerosol: Path) -> tuple:
    """Derive the oracle SPECIES file for a deposition case.

    - DRY-007 starts from the inert tracer example (no wet removal, no
      aerosol settling) and sets ``PDRYVEL=2.0``. FLEXPART converts PDRYVEL
      from cm/s to m/s (``readoptions_mod.f90``: ``dryvel*0.01``), giving a
      constant 0.02 m/s dry deposition velocity with the same
      ``exp(-vdep*|dt|/(2*href))`` removal law and sub-``2*href`` layer gate
      as the candidate (``drydepo_mod.f90:860``,
      ``physics/deposition.rs:dry_deposition_probability_step``,
      ``href = 15 m`` both sides). This equates the dry process intensity
      by experimental design; the comparison stays diagnostic.
    - WET-008 starts from the upstream aerosol example (wet scavenging
      parameters active) and drops exactly the ``PNDIA`` key that the pinned
      v11.1 ``species_params`` namelist does not declare (the oracle aborts
      with ``SPECIES file not in NAMELIST format`` otherwise). FLEXPART
      computes scavenging rates dynamically from precipitation and cloud
      fields, so no single-coefficient equivalence with the candidate
      uniform ``lambda`` exists; the wet comparison stays diagnostic, and
      the oracle aerosol additionally dry-deposits (reported separately in
      the budget, never equated with the candidate dry-zero path).

    Returns (file_text, provenance_text).
    """
    if profile == "species_040_dry_constant_v1":
        base = tracer.read_text(encoding="utf-8")
        text, old = set_namelist_key(base, "PDRYVEL", "2.0")
        provenance = (
            f"Derived from upstream examples/Tracer/SPECIES/{tracer.name} "
            f"at sha256 {sha256_file(tracer)} "
            "(pinned oracle reference/flexpart-11.1.json).\n"
            f"Edited assignment: `{old}` -> `PDRYVEL=2.0` (2.0 cm/s = 0.02 m/s, "
            "exactly the candidate DRY-007 dry_deposition_velocity_m_s; wet "
            "removal stays disabled as in the tracer).\n"
        )
        header = (
            "! Corpus-derived dry-deposition SPECIES for DRY-007.\n"
            "! See SPECIES_040.PROVENANCE.txt for the exact upstream source and edit.\n"
        )
        return header + text, provenance
    if profile != "species_040_wet_aerosol_v1":
        raise SystemExit(f"{case_id}: profile {profile!r} has no depositing species derivation")
    raw_lines = aerosol.read_text(encoding="utf-8").splitlines(keepends=True)
    kept, removed = [], []
    for line in raw_lines:
        code = re.sub(r"!.*", "", line)
        if re.match(r"\s*PNDIA\s*=", code, re.IGNORECASE):
            removed.append(line)
        else:
            kept.append(line)
    if not removed:
        raise SystemExit(
            f"{aerosol}: expected PNDIA line not found; upstream example changed, "
            "re-derive the depositing SPECIES deliberately"
        )
    provenance = (
        f"Derived from upstream examples/Aerosol/SPECIES/{aerosol.name} "
        f"at sha256 {sha256_file(aerosol)} "
        "(pinned oracle reference/flexpart-11.1.json).\n"
        "Removed lines (PNDIA is not a member of the v11.1 species_params "
        "namelist; the oracle aborts on unknown keys):\n"
        + "".join(f"  {line}" if line.endswith("\n") else f"  {line}\n" for line in removed)
    )
    header = (
        "! Corpus-derived depositing SPECIES for WET-008.\n"
        "! See SPECIES_040.PROVENANCE.txt for the exact upstream source and edit.\n"
    )
    return header + "".join(kept), provenance


def namelist_value(text: str, key: str) -> str:
    """Extract one raw value for KEY from namelist text (comments stripped)."""
    stripped = re.sub(r"!.*", "", text)
    match = re.search(
        rf"\b{re.escape(key)}\s*=\s*([^,/\n]+)", stripped, re.IGNORECASE
    )
    if not match:
        raise ValueError(f"key {key} not found in namelist")
    return match.group(1).strip().strip("\"'")


def verify_rendered_case(case_id: str, case: dict, files: dict, specnum: int) -> None:
    """Verify rendered scientific fixture content in memory before any write."""
    release = case["release"]
    domain = case["domain"]
    failures = []

    def check(name: str, actual, expected=True, tolerance: float = 0.0) -> None:
        if isinstance(expected, float):
            ok = abs(actual - expected) <= tolerance
        else:
            ok = actual == expected
        if not ok:
            failures.append(f"{name}: rendered fixture has {actual!r}, case needs {expected!r}")

    releases = files["RELEASES"]
    lon1, lon2, lat1, lat2 = release_lonlat(case_id, case)
    z1, z2, zkind = release_vertical(case_id, case)
    start_stamp, end_stamp = release_window_datetimes(case_id, case)
    idate1, itime1 = flexpart_datetime(start_stamp)
    idate2, itime2 = flexpart_datetime(end_stamp)
    check("RELEASES LON1", float(namelist_value(releases, "LON1")), lon1, 1e-9)
    check("RELEASES LON2", float(namelist_value(releases, "LON2")), lon2, 1e-9)
    check("RELEASES LAT1", float(namelist_value(releases, "LAT1")), lat1, 1e-9)
    check("RELEASES LAT2", float(namelist_value(releases, "LAT2")), lat2, 1e-9)
    check("RELEASES Z1", float(namelist_value(releases, "Z1")), z1, 1e-9)
    check("RELEASES Z2", float(namelist_value(releases, "Z2")), z2, 1e-9)
    check("RELEASES ZKIND", int(namelist_value(releases, "ZKIND")), zkind)
    check("RELEASES IDATE1", int(namelist_value(releases, "IDATE1")), idate1)
    check("RELEASES ITIME1", int(namelist_value(releases, "ITIME1")), itime1)
    check("RELEASES IDATE2", int(namelist_value(releases, "IDATE2")), idate2)
    check("RELEASES ITIME2", int(namelist_value(releases, "ITIME2")), itime2)
    check("RELEASES PARTS", int(namelist_value(releases, "PARTS")), int(release["particle_count"]))
    check("RELEASES SPECNUM_REL", int(namelist_value(releases, "SPECNUM_REL")), specnum)
    expected_g = case_total_mass_kg(case_id, case) * KG_TO_G
    actual_g = float(namelist_value(releases, "MASS").replace("D", "E"))
    check("RELEASES MASS_g", actual_g, expected_g, 1e-4 * expected_g)

    outgrid = files["OUTGRID"]
    output_grid = case.get("output_grid") or domain
    check("OUTGRID OUTLON0", float(namelist_value(outgrid, "OUTLON0")), float(output_grid["xlon0_deg"]), 1e-9)
    check("OUTGRID OUTLAT0", float(namelist_value(outgrid, "OUTLAT0")), float(output_grid["ylat0_deg"]), 1e-9)
    check("OUTGRID NUMXGRID", int(namelist_value(outgrid, "NUMXGRID")), int(output_grid["nx"]))
    check("OUTGRID NUMYGRID", int(namelist_value(outgrid, "NUMYGRID")), int(output_grid["ny"]))
    check("OUTGRID DXOUT", float(namelist_value(outgrid, "DXOUT")), float(output_grid["dx_deg"]), 1e-9)
    check("OUTGRID DYOUT", float(namelist_value(outgrid, "DYOUT")), float(output_grid["dy_deg"]), 1e-9)

    expected_meteo_args = meteo_args(case_id, case)
    actual_meteo_args = files.get("METEO_ARGS.txt", "").strip()
    check("METEO_ARGS", actual_meteo_args, expected_meteo_args)

    command = files["COMMAND"]
    overrides = normalize_oracle_overrides(case_id, case)
    check("COMMAND LTURBULENCE", int(namelist_value(command, "LTURBULENCE")), int(overrides["lturbulence"]))
    check("COMMAND LCONVECTION", int(namelist_value(command, "LCONVECTION")), int(overrides["lconvection"]))
    check("COMMAND CTL", float(namelist_value(command, "CTL")), float(overrides["ctl"]), 1e-6)
    check("COMMAND IFINE", int(namelist_value(command, "IFINE")), int(overrides["ifine"]))
    direction = _required_simulation_direction(case_id, case)
    expected_ldirect = LDIRECT_FORWARD if direction == "forward" else LDIRECT_BACKWARD
    check("COMMAND LDIRECT", int(namelist_value(command, "LDIRECT")), expected_ldirect)
    output = _required_output(case_id, case, int(overrides["lsynctime_s"]))
    check("COMMAND LOUTSTEP", int(namelist_value(command, "LOUTSTEP")), output["interval_s"])
    check("COMMAND LOUTAVER", int(namelist_value(command, "LOUTAVER")), output["averaging_window_s"])
    check("COMMAND LOUTSAMPLE", int(namelist_value(command, "LOUTSAMPLE")), output["sampling_interval_s"])
    check(
        "COMMAND LSYNCTIME",
        int(namelist_value(command, "LSYNCTIME")),
        int(overrides["lsynctime_s"]),
    )

    species_text = files["SPECIES"]
    if not isinstance(species_text, str):
        failures.append(f"SPECIES_{specnum:03d}: rendered content is missing")
    else:
        code = re.sub(r"!.*", "", species_text)
        check(
            f"SPECIES_{specnum:03d} has no PNDIA (unknown to v11.1)",
            re.search(r"(?im)^\s*PNDIA\s*=", code) is None,
        )
        if specnum == 40:
            check(
                "SPECIES_040.PROVENANCE.txt rendered",
                isinstance(files.get("SPECIES_PROVENANCE"), str)
                and bool(files["SPECIES_PROVENANCE"]),
            )
        if case_id == "DRY-007":
            check(
                "DRY SPECIES PDRYVEL=2.0",
                float(namelist_value(species_text, "PDRYVEL")) == 2.0,
            )

    if failures:
        raise SystemExit(
            f"{case_id}: rendered fixtures drift from case JSON before write:\n"
            + "\n".join(failures)
        )


def verify_case(case_id: str, case: dict, outdir: Path, specnum: int) -> None:
    """Optional post-hoc disk audit; generation itself verifies before writing."""
    species_path = outdir / "SPECIES" / f"SPECIES_{specnum:03d}"
    provenance_path = outdir / "SPECIES" / f"SPECIES_{specnum:03d}.PROVENANCE.txt"
    files = {
        "COMMAND": (outdir / "COMMAND").read_text(encoding="utf-8"),
        "RELEASES": (outdir / "RELEASES").read_text(encoding="utf-8"),
        "OUTGRID": (outdir / "OUTGRID").read_text(encoding="utf-8"),
        "METEO_ARGS.txt": (outdir / "METEO_ARGS.txt").read_text(encoding="utf-8"),
        "SPECIES": species_path.read_text(encoding="utf-8"),
        "SPECIES_PROVENANCE": (
            provenance_path.read_text(encoding="utf-8")
            if provenance_path.is_file()
            else None
        ),
    }
    verify_rendered_case(case_id, case, files, specnum)


def is_real_weather(case_id: str, case: dict) -> bool:
    """Check if the case uses real-weather meteorology (RealWeather profile)."""
    return _required_wind(case_id, case)["profile"] == "real_weather"


def _load_case(case_id: str) -> tuple:
    """Load the schema-v2 case document and the source file name for it.

    RESTART-010 has no dedicated document: it reuses the neutral release/grid
    shape from PBL-NEUTRAL-005 (oracle-only restart illustration) while the
    recorded ``case_file`` name stays RESTART-010.json.
    """
    case_path = CASES / f"{case_id}.json"
    if case_path.is_file():
        return json.loads(case_path.read_text(encoding="utf-8")), case_path
    case = json.loads((CASES / "PBL-NEUTRAL-005.json").read_text(encoding="utf-8"))
    return case, case_path


def validate_and_normalize_case_for_generation(
    case_id: str, case: dict, case_file, tracer, aerosol
) -> dict:
    """Complete fail-closed preflight of one schema-v2 case.

    Runs every validation the generation path depends on and computes all
    output content *before* any filesystem write, so a malformed input can
    never leave a partially updated fixture directory:

        validate once -> write many

    Fields validated here (each raising SystemExit naming the case and the
    offending field): schema version; Oracle command overrides including the
    Oracle/physics agreement; integration; release (geometry, timing,
    particle count, species id, total mass); wind profile; physics switches;
    surface semantics; domain (OUTGRID fields); species identity and upstream
    source availability; deposition/species selection for SPECIES_040;
    simulation direction; output timing semantics; and the real-weather
    meteorology metadata. The returned normalized dict is
    the only input the write phase consumes.
    """
    try:
        validate_case_document(case, source=str(case_file))
    except ValidationCaseSchemaError as exc:
        raise SystemExit(f"{case_id}: validation-case-v2 schema violation: {exc}") from None

    if case.get("schema_version") != 2 or "version" in case:
        raise SystemExit(
            f"{case_id}: unsupported schema: expected only schema_version 2, "
            f"got schema_version={case.get('schema_version')!r} "
            f"version={case.get('version')!r} (v1 frozen, see MIGRATION_NOTES.md)"
        )

    oracle = normalize_oracle_overrides(case_id, case)
    _required_execution_profile(case_id, case)

    integration = _required_integration(case_id, case)
    direction = _required_simulation_direction(case_id, case)
    output = _required_output(case_id, case, int(oracle["lsynctime_s"]))
    domain = _required_domain(case_id, case)

    release = _validate_release_contract(case_id, case, domain)
    mass_kg = case_total_mass_kg(case_id, case)
    release_window_datetimes(case_id, case)
    lon1, lon2, lat1, lat2 = release_lonlat(case_id, case)
    z1, z2, _zkind = release_vertical(case_id, case)
    specnum = species_number_for_case(case_id, case)

    wind = _required_wind(case_id, case)
    profile = wind["profile"]
    oracle_meteorology = _required_oracle_meteorology_profile(case_id, case)
    meteorology = (
        _required_meteorology(case_id, wind, integration)
        if profile == "real_weather"
        else None
    )
    output_grid = _required_output_grid(case_id, case)

    physics = mandatory_physics_switches(case_id, case)
    species_profile = _validate_species_physics_contract(case_id, case, physics)
    _validate_deposition_contract(case_id, case, physics)
    surface = _required_surface(case_id, case, physics)
    _validate_stochastic_contract(case_id, case, physics)
    _required_units(case_id, case, wind, surface, physics)
    _required_validation_definition_refs(case_id, case)

    if species_profile in ("species_040_dry_constant_v1", "species_040_wet_aerosol_v1"):
        if species_profile == "species_040_wet_aerosol_v1" and (
            aerosol is None or not aerosol.is_file()
        ):
            raise SystemExit(f"{case_id}: upstream aerosol species not found: {aerosol}")
        species_text, species_provenance = species_for_case(
            case_id, species_profile, tracer, aerosol
        )
        species_source = None
    elif species_profile == "species_024_inert_v1" and specnum == 24:
        if tracer is None or not tracer.is_file():
            raise SystemExit(f"{case_id}: upstream tracer species not found: {tracer}")
        species_text = tracer.read_text(encoding="utf-8")
        species_provenance = None
        species_source = str(tracer)
    else:
        raise SystemExit(
            f"{case_id}: no established SPECIES derivation for SPECIES_{specnum:03d} "
            "(corpus uses SPECIES_024 and SPECIES_040)"
        )

    meteo_args_line = meteo_args(case_id, case)
    if profile == "real_weather":
        meteo_args_txt = "\n"
        meteo_txt = real_weather_meteo_txt(meteorology)
    else:
        meteo_args_txt = meteo_args_line + "\n"
        meteo_txt = (
            "python3 scripts/generate_synthetic_grib.py "
            f"--output-dir target/corpus/meteo/{case_id} {meteo_args_line}\n"
        )

    derivation = {
        "case_file": f"fixtures/corpus/cases/{Path(case_file).name}",
        "release_lon_deg": lon1,
        "release_lat_deg": lat1,
        "release_z_m": z1,
        "particle_count": release["particle_count"],
        "candidate_mass_kg": mass_kg,
        "mass_conversion": "MASS_g = mass_kg * 1000 (FLEXPART MASS is in grams)",
        "oracle_mass_g": mass_kg * KG_TO_G,
        "oracle_meteorology_profile": copy.deepcopy(case["oracle_meteorology_profile"]),
        "output_grid": (
            {
                key: output_grid[key]
                for key in (
                    "xlon0_deg", "ylat0_deg", "nx", "ny", "nz",
                    "dx_deg", "dy_deg", "heights_m", "heights_ref"
                )
            }
            if output_grid is not None
            else {
                "xlon0_deg": domain["xlon0_deg"],
                "ylat0_deg": domain["ylat0_deg"],
                "nx": domain["nx"],
                "ny": domain["ny"],
                "nz": len(STANDARD_OUTHEIGHTS),
                "dx_deg": domain["dx_deg"],
                "dy_deg": domain["dy_deg"],
                "heights_m": STANDARD_OUTHEIGHTS,
                "heights_ref": "agl",
                "policy": "legacy synthetic shared output grid",
            }
        ),
    }
    files = {
        "COMMAND": command_text(case_id, case),
        "RELEASES": releases_text(case_id, case, specnum),
        "OUTGRID": outgrid_text(case),
        "AGECLASSES": ageclass_text(case_id, case),
        "RECEPTORS": RECEPTORS_ZERO,
        "SPECIES": species_text,
        "SPECIES_PROVENANCE": species_provenance,
        "SPECIES_SOURCE": species_source,
        "METEO_ARGS.txt": meteo_args_txt,
        "METEO.txt": meteo_txt,
        "INPUT_DERIVATION.json": json.dumps(derivation, indent=2) + "\n",
        "RESTART-NOTE.txt": RESTART_NOTE if case_id == "RESTART-010" else None,
    }
    verify_rendered_case(case_id, case, files, specnum)

    return {
        "case_id": case_id,
        "case": copy.deepcopy(case),
        "schema_version": 2,
        "specnum": specnum,
        "integration": integration,
        "simulation_direction": direction,
        "output": output,
        "oracle": oracle,
        "physics": physics,
        "wind_profile": profile,
        "oracle_meteorology": oracle_meteorology,
        "meteorology": meteorology,
        "files": files,
    }


def prepare_cases(desired, tracer, aerosol) -> list:
    """Preflight the whole generation set before any filesystem write.

    Loads and fully validates every case (schema, oracle overrides,
    integration, release, wind, physics, surface, domain, species, and
    real-weather meteorology where applicable). Returns one normalized dict
    per case, ready for write_case_fixtures(). A single malformed case aborts
    here with every fixture directory untouched, because nothing has been
    written yet.
    """
    prepared = []
    for case_id in desired:
        case, case_path = _load_case(case_id)
        prepared.append(
            validate_and_normalize_case_for_generation(
                case_id,
                case,
                case_file=str(case_path),
                tracer=tracer,
                aerosol=aerosol,
            )
        )
    return prepared


def write_case_fixtures(normalized: dict, out_root: Path = None) -> None:
    """Persist one fully preflighted and in-memory-verified case.

    No scientific/semantic validation occurs here. Failures in this phase are
    operational filesystem errors only.
    """
    if out_root is None:
        out_root = FORTRAN_OUT
    case_id = normalized["case_id"]
    specnum = normalized["specnum"]
    files = normalized["files"]
    outdir = out_root / case_id
    (outdir / "SPECIES").mkdir(parents=True, exist_ok=True)

    (outdir / "SPECIES" / f"SPECIES_{specnum:03d}").write_text(
        files["SPECIES"], encoding="utf-8"
    )
    if files["SPECIES_PROVENANCE"] is not None:
        (outdir / "SPECIES" / f"SPECIES_{specnum:03d}.PROVENANCE.txt").write_text(
            files["SPECIES_PROVENANCE"], encoding="utf-8"
        )

    for name in (
        "COMMAND",
        "RELEASES",
        "OUTGRID",
        "AGECLASSES",
        "RECEPTORS",
        "METEO_ARGS.txt",
        "METEO.txt",
        "INPUT_DERIVATION.json",
    ):
        (outdir / name).write_text(files[name], encoding="utf-8")
    if files["RESTART-NOTE.txt"] is not None:
        (outdir / "RESTART-NOTE.txt").write_text(
            files["RESTART-NOTE.txt"], encoding="utf-8"
        )
    print(f"wrote preflight-verified {outdir}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--flexpart-dir", default=str(REPO.parent / "flexpart"))
    args = parser.parse_args()
    flexpart = Path(args.flexpart_dir)
    tracer = flexpart / "examples" / "Tracer" / "SPECIES" / "SPECIES_024"
    aerosol = flexpart / "examples" / "Aerosol" / "SPECIES" / "SPECIES_040"
    if not tracer.is_file():
        raise SystemExit(f"upstream tracer species not found: {tracer}")

    desired = [
        "ADV-ANA-001",
        "WIND-UNI-002",
        "WIND-SHEAR-003",
        "PBL-STABLE-004",
        "PBL-NEUTRAL-005",
        "PBL-UNSTABLE-006",
        "DRY-007",
        "WET-008",
        "RESTART-010",
    ]

    # Preflight every case before the first write: one malformed schema-v2
    # case aborts the whole run with all fixture directories untouched.
    prepared = prepare_cases(desired, tracer, aerosol)
    for normalized in prepared:
        write_case_fixtures(normalized)
    print("Fortran corpus fixtures generated and input-equal to case JSON.")


if __name__ == "__main__":
    main()
