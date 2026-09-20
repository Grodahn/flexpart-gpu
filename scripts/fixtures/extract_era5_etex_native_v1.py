#!/usr/bin/env python3
"""Extract the checked-in real ERA5 canonical meteorology fixture (fixture v1).

Issue: #29 / RISK-03.3G-10a.

This is a fixture-specific, one-shot extraction utility for
``fixtures/meteorology/era5-etex-native-v1.json``. It is intentionally NOT an
operational GRIB/ERA5 adapter: provider-specific decoding belongs to #32 in the
production path. Reproducibility is what matters here, so the script:

- verifies the source GRIB and surface archive against the checked-in manifests
  (```request.json```, ```surface-request.json```) before reading anything;
- takes exactly one ERA5 timestamp (1994-10-23 15:00 UTC);
- takes a fixed 2 x 2 horizontal slice (longitude -2.0/-1.75, latitude
  48.0/48.25 degrees);
- keeps all 137 native model levels with the native hybrid A/B coefficients;
- emits only canonical schema-v1 semantics (provider names, paramIds or the
  native 352..8 degree longitude encoding never leak into field content);
- fails loudly whenever the source geometry, coverage, finite-ness or
  monotonic vertical structure deviates from the verified manifests or from
  the invariants assumed by the canonical contract.

Provider-native vertical motion (```omega```, param 135, and eta-coordinate
velocity, param 77) and reconstructed 3-D ```pressure`` are deliberately NOT
written into the fixture: converting them to canonical semantics (upward-
positive m/s and a+b*p_s pressure) is hybrid-transform work owned by #30.
The surface pressure and the full hybrid metadata needed to describe the
native sigma-pressure coordinate ARE included.

The output ``.provenance.json`` sits beside the fixture and records the source
lineage, the selected slice, the normalization performed and the canonical
schema identity. No scientific parity claim is made by this artifact.

Usage:
    python scripts/fixtures/extract_era5_etex_native_v1.py [--native-mini-dir DIR]
"""

import argparse
import calendar
import hashlib
import json
import subprocess
import sys
from datetime import datetime
from pathlib import Path

import eccodes
import numpy as np

REPO = Path(__file__).resolve().parents[2]
DEFAULT_NATIVE_MINI = REPO / "fixtures" / "etex" / "native-mini"
DEFAULT_OUTPUT = REPO / "fixtures" / "meteorology" / "era5-etex-native-v1.json"

SCHEMA_ID = "flexpart-gpu.canonical-meteorology"
SCHEMA_VERSION = 1

# Native ERA5 model-level fields kept by the fixture (provider paramId -> canonical
# FieldId). param 135 (omega) and 77 (eta-dot) stay out; see module docstring.
LEVEL_PARAMS = {130: "temperature", 131: "wind_u", 132: "wind_v", 133: "specific_humidity"}

# Horizontal slice: 2 x 2 cells snapped to the 0.25-degree ERA5 grid. The slice
# brackets the ETEX-1 release point (48.058 N, -2.0 E) on the fixed source grid.
SELECTED_TIMESTAMP = "1994-10-23T15:00:00"
SELECTED_LONS_DEG = [-2.0, -1.75]
SELECTED_LATS_DEG = [48.0, 48.25]
NX, NY = 2, 2
NATIVE_LEVELS = 137
NATIVE_HALF_LEVELS = NATIVE_LEVELS + 1
PV_LEN = 2 * NATIVE_HALF_LEVELS  # a (Pa) and b (dimensionless) on half levels.

# Reference surface pressure used to expose explicit vertical coordinate values.
# FLEXPART uses 101325 Pa for the same purpose (conv_mod.f90:125) and derives the
# center-of-layer coefficients from the half-level (interface) coefficients by
# averaging (windfields_mod.f90:845-846).
REFERENCE_SURFACE_PRESSURE_PA = 101325.0

# Nine significant digits round-trip IEEE-754 binary32 uniquely; GRIB values and
# the PV coefficients are stored as binary32.
FLOAT_FORMAT = ".9g"


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_manifest(path):
    record = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(record.get("sha256"), str) or len(record.get("sha256", "")) != 64 \
            or not isinstance(record.get("bytes"), int) or record.get("bytes", 0) <= 0:
        raise ValueError(f"malformed manifest {path}")
    return record


def verify_manifest(path, record):
    if path.stat().st_size != record["bytes"]:
        raise ValueError(f"source byte count differs from manifest: {path}")
    if sha256(path) != record["sha256"]:
        raise ValueError(f"source SHA-256 differs from manifest: {path}")


def git_revision():
    try:
        completed = subprocess.run(
            ["git", "-C", str(REPO), "rev-parse", "--short=12", "HEAD"],
            check=True, capture_output=True, text=True)
    except (OSError, subprocess.CalledProcessError):
        return None
    revision = completed.stdout.strip()
    return revision or None


def float_text(value):
    return format(float(value), FLOAT_FORMAT)


def emit_json(obj, pad=""):
    """Deterministic JSON emitter with compact single-line numeric arrays.

    Numeric arrays are rendered inline with nine significant digits (an exact
    IEEE-754 binary32 round-trip). Everything else is laid out at two-space
    indentation so the artifact stays reviewable in Git.
    """
    if isinstance(obj, dict):
        if not obj:
            return "{}"
        entries = [f"{pad}  {json.dumps(key)}: {emit_json(value, pad + '  ')}"
                   for key, value in obj.items()]
        return "{\n" + ",\n".join(entries) + "\n" + pad + "}"
    if isinstance(obj, list):
        if all(isinstance(value, (int, float)) and not isinstance(value, bool)
               for value in obj):
            return "[" + ", ".join(
                float_text(value) if isinstance(value, float) else repr(value)
                for value in obj) + "]"
        entries = [emit_json(value, pad + "  ") for value in obj]
        return "[\n" + ",\n".join(f"{pad}  {entry}" for entry in entries) + "\n" + pad + "]"
    if isinstance(obj, bool):
        return "true" if obj else "false"
    if isinstance(obj, float):
        return float_text(obj)
    if isinstance(obj, int):
        return repr(obj)
    return json.dumps(obj)


def assemble(payload):
    """Render the canonical fixture as deterministic, compact JSON text."""
    return emit_json(payload)


def read_native_fields(grib_path, expected):
    """Read the selected ERA5 model-level timestamp; returns per-level arrays.

    Returns (fields[name][level] 41x65 float arrays, hybrid_pv 276-vector).
    expected: dict paramId -> canonical field name. Only the selected
    timestamp (date/time from SELECTED_TIMESTAMP) and the 65 x 41 area of the
    checked-in manifest are accepted; anything else fails closed.
    """
    fields = {name: {} for name in expected.values()}
    pv = None
    wanted_date = int(SELECTED_TIMESTAMP[:10].replace("-", ""))
    wanted_time = 1500
    count = 0
    with grib_path.open("rb") as source:
        while (message := eccodes.codes_grib_new_from_file(source)) is not None:
            try:
                param = eccodes.codes_get_long(message, "paramId")
                level = eccodes.codes_get_long(message, "level")
                date = eccodes.codes_get_long(message, "dataDate")
                hhmm = eccodes.codes_get_long(message, "dataTime")
                if param not in expected or date != wanted_date or hhmm != wanted_time:
                    continue
                if level < 1 or level > NATIVE_LEVELS:
                    raise ValueError(f"unexpected model level {level}")
                if eccodes.codes_get(message, "gridType") != "regular_ll" \
                        or eccodes.codes_get_long(message, "Ni") != 65 \
                        or eccodes.codes_get_long(message, "Nj") != 41 \
                        or eccodes.codes_get_long(message, "NV") != PV_LEN \
                        or eccodes.codes_get_long(message, "PVPresent") != 1 \
                        or eccodes.codes_get_double(message, "latitudeOfFirstGridPointInDegrees") != 53.0 \
                        or eccodes.codes_get_double(message, "latitudeOfLastGridPointInDegrees") != 43.0 \
                        or eccodes.codes_get_double(message, "longitudeOfFirstGridPointInDegrees") != 352.0 \
                        or eccodes.codes_get_double(message, "longitudeOfLastGridPointInDegrees") != 8.0:
                    raise ValueError(f"unexpected grid geometry at {param}/{level}")
                values = np.asarray(eccodes.codes_get_values(message)).reshape(41, 65)
                if not np.all(np.isfinite(values)):
                    raise ValueError(f"non-finite native field {param}/{level}")
                name = expected[param]
                if level in fields[name]:
                    raise ValueError(f"duplicate native field {param}/{level}")
                fields[name][level] = values
                this_pv = np.asarray(eccodes.codes_get_array(message, "pv"))
                if len(this_pv) != PV_LEN:
                    raise ValueError("native hybrid coefficients are missing")
                if pv is None:
                    pv = this_pv
                elif not np.array_equal(pv, this_pv):
                    raise ValueError("inconsistent hybrid coefficients across levels")
                count += 1
            finally:
                eccodes.codes_release(message)
    expected_count = len(expected) * NATIVE_LEVELS
    if count != expected_count:
        raise ValueError(f"native field coverage: {count} fields, expected {expected_count}")
    return fields, pv


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-mini-dir", type=Path, default=DEFAULT_NATIVE_MINI,
                        help="checked-in fixtures/etex/native-mini source directory")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()

    base = args.native_mini_dir
    if not base.is_dir():
        print(f"source directory not found: {base}", file=sys.stderr)
        sys.exit(2)

    grib_path = base / "era5-19941023-151821-ml.grib"
    surface_path = base / "era5-surface-19941023-24.npz"
    request = load_manifest(base / "request.json")
    surface_request = load_manifest(base / "surface-request.json")

    request_identity = request["request"]
    surface_identity = {key: surface_request[key]
                        for key in ("source", "dataset", "retrieved_utc", "extraction")}
    for path, record, label in ((grib_path, request, "native GRIB"),
                                (surface_path, surface_request, "surface archive")):
        verify_manifest(path, record)
        print(f"verified {label}: {path.name} sha256 {record['sha256']}")

    # The timestamp must be the first surface snapshot.
    if surface_request["times"][0] != SELECTED_TIMESTAMP:
        raise ValueError("surface archive does not start at the selected timestamp")

    fields, pv = read_native_fields(grib_path, LEVEL_PARAMS)
    ts = datetime.fromisoformat(SELECTED_TIMESTAMP)
    epoch_seconds = calendar.timegm(ts.utctimetuple())

    # Resolve the horizontal slice against the source grid and fail closed on drift.
    with np.load(surface_path) as archive:
        source_times = [value.decode() for value in archive["times"]]
        if source_times[0] != SELECTED_TIMESTAMP:
            raise ValueError("surface snapshot times do not match the source manifest")
        if not np.array_equal(archive["latitudes"], np.arange(53, 42.99, -0.25)) \
                or not np.array_equal(archive["longitudes"], np.arange(-8, 8.01, 0.25)):
            raise ValueError("surface latitude/longitude grid mismatch")
        for lon in SELECTED_LONS_DEG:
            if not np.isclose(np.min(np.abs(archive["longitudes"] - lon)), 0.0):
                raise ValueError(f"selected longitude {lon} is off the source grid")
        for lat in SELECTED_LATS_DEG:
            if not np.isclose(np.min(np.abs(archive["latitudes"] - lat)), 0.0):
                raise ValueError(f"selected latitude {lat} is off the source grid")
        # Canonical x follows longitude, canonical y follows latitude ascending;
        # the source stores latitude descending, so the two canonical y slots are
        # the descending grid rows from (NATIVE lat base) downward.
        lon_indices = [int(np.flatnonzero(np.isclose(archive["longitudes"], lon))[0])
                       for lon in SELECTED_LONS_DEG]
        lat_desc_indices = [int(np.flatnonzero(np.isclose(archive["latitudes"], lat))[0])
                            for lat in SELECTED_LATS_DEG]
        # Canonical y index 0 is the smallest latitude; source rows run
        # north-to-south, so canonical y maps to source rows descending by one.
        source_desc_rows = [lat_desc_indices[0] - y for y in range(NY)]
        if source_desc_rows != lat_desc_indices:
            raise ValueError("selected latitudes are not contiguous native grid rows")
        surface_pressure = archive["surface_pressure"][0]

    # Full-level (center-of-layer) hybrid coefficients follow FLEXPART
    # windfields_mod.f90:845-846: akz/bkz = 0.5 * (akm/bkm(k) + akm/bkm(k+1)).
    a_half, b_half = pv[:NATIVE_HALF_LEVELS], pv[NATIVE_HALF_LEVELS:]
    a_full = 0.5 * (a_half[:-1] + a_half[1:])
    b_full = 0.5 * (b_half[:-1] + b_half[1:])
    level_values = a_full + b_full * REFERENCE_SURFACE_PRESSURE_PA
    interface_values = a_half + b_half * REFERENCE_SURFACE_PRESSURE_PA
    for label, values in (("level", level_values), ("interface", interface_values)):
        if not np.all(np.isfinite(values)) or np.any(np.diff(values) <= 0):
            raise ValueError(f"non-monotonic {label} reference pressures")

    vertical_coordinate = {
        "kind": "hybrid_sigma_pressure",
        "reference": "model_native",
        "ordering": "increasing",
        "level_values": [float(value) for value in level_values],
        "interface_values": [float(value) for value in interface_values],
        "hybrid_a_pa": [float(value) for value in a_full],
        "hybrid_b": [float(value) for value in b_full],
        "surface_pressure_dependency": "surface_pressure",
    }

    field_definitions = {
        "wind_u": ("meter_per_second", "positive_eastward"),
        "wind_v": ("meter_per_second", "positive_northward"),
        "temperature": ("kelvin", "signed_scalar"),
        "specific_humidity": ("kilogram_per_kilogram", "non_negative"),
        "surface_pressure": ("pascal", "non_negative"),
    }
    field_time = {
        "calendar": "gregorian",
        "kind": "instantaneous",
        "valid_time_epoch_seconds": epoch_seconds,
    }

    fields_payload = []
    for name, (unit, sign) in field_definitions.items():
        if name == "surface_pressure":
            values = [surface_pressure[source_desc_rows[y], lon_indices[x]]
                      for y in range(NY) for x in range(NX)]
            shape = [NX, NY]
            axes = ["x", "y"]
            vertical_staggering = "not_applicable"
        else:
            values = [fields[name][level][source_desc_rows[y], lon_indices[x]]
                      for level in range(1, NATIVE_LEVELS + 1)
                      for y in range(NY) for x in range(NX)]
            shape = [NX, NY, NATIVE_LEVELS]
            axes = ["x", "y", "z"]
            vertical_staggering = "level_center"
        if not np.all(np.isfinite(values)):
            raise ValueError(f"non-finite canonical values for {name}")
        fields_payload.append({
            "id": name,
            "shape": shape,
            "axis_order": axes,
            "unit": unit,
            "sign": sign,
            "horizontal_staggering": "cell_center",
            "vertical_staggering": vertical_staggering,
            "time": field_time,
            "values": [float(value) for value in values],
        })

    payload = {
        "schema": {"id": SCHEMA_ID, "version": SCHEMA_VERSION},
        "horizontal_grid": {
            "nx": NX,
            "ny": NY,
            "xlon0_deg": SELECTED_LONS_DEG[0],
            "ylat0_deg": SELECTED_LATS_DEG[0],
            "dx_deg": 0.25,
            "dy_deg": 0.25,
            "longitude_domain": "minus180_to180",
        },
        "vertical_coordinate": vertical_coordinate,
        "fields": fields_payload,
    }

    args.output.parent.mkdir(parents=True, exist_ok=True)
    fixture_text = assemble(payload)
    fixture_bytes = fixture_text.encode("utf-8") + b"\n"
    args.output.write_bytes(fixture_bytes)

    provenance = {
        "schema": {"id": SCHEMA_ID, "version": SCHEMA_VERSION},
        "artifact": {
            "path": str(args.output.relative_to(REPO)).replace("\\", "/"),
            "sha256": hashlib.sha256(fixture_bytes).hexdigest(),
            "bytes": len(fixture_bytes),
            "generated_by": "scripts/fixtures/extract_era5_etex_native_v1.py",
            "generated_at_git_revision": git_revision(),
            "generator_environment": {
                "python": sys.version.split()[0],
                "eccodes": str(eccodes.codes_get_api_version()),
                "numpy": np.__version__,
            },
        },
        "source": {
            "path": "fixtures/etex/native-mini",
            "request_identity": request_identity,
            "surface_request": surface_identity,
            "surface_manifest_times": surface_request["times"],
            "native_grib": {
                "path": "fixtures/etex/native-mini/era5-19941023-151821-ml.grib",
                "bytes": request["bytes"],
                "sha256": request["sha256"],
            },
            "surface_archive": {
                "path": "fixtures/etex/native-mini/era5-surface-19941023-24.npz",
                "bytes": surface_request["bytes"],
                "sha256": surface_request["sha256"],
            },
            "source_archive_sha256": surface_request.get("source_archive_sha256"),
        },
        "slice": {
            "timestamp": "1994-10-23T15:00:00Z",
            "epoch_seconds": epoch_seconds,
            "horizontal": {
                "nx": NX,
                "ny": NY,
                "xlon0_deg": SELECTED_LONS_DEG[0],
                "ylat0_deg": SELECTED_LATS_DEG[0],
                "longitudes_deg": SELECTED_LONS_DEG,
                "latitudes_deg": SELECTED_LATS_DEG,
                "source_lon_indices": lon_indices,
                "source_lat_desc_indices": lat_desc_indices,
                "source_lat_asc_indices": [40 - index for index in lat_desc_indices],
                "longitude_domain": "minus180_to180",
            },
            "vertical": {
                "native_levels": NATIVE_LEVELS,
                "selected_levels": "1..137 (all native levels)",
                "coordinate": "native ERA5 hybrid sigma-pressure, half-level a (Pa)/b plus full-level a/b as 0.5*(half(k)+half(k+1)) following windfields_mod.f90:845-846",
                "reference_surface_pressure_pa": REFERENCE_SURFACE_PRESSURE_PA,
            },
        },
        "represented_fields": ["wind_u", "wind_v", "temperature",
                               "specific_humidity", "surface_pressure"],
        "omitted_provider_fields": {
            "omega_param_135": "pressure velocity; conversion to canonical upward-positive m/s is eigen to #30 and is not represented",
            "eta_velocity_param_77": "eta-coordinate velocity; conversion to canonical upward-positive m/s is eigen to #30 and is not represented",
            "pressure_3d": "full-level pressure reconstruction a+b*p_s is the #30 hybrid transform and is not represented",
        },
        "normalization": [
            "native ERA5 longitude encoding 352..8 degrees normalized to the [-180,180) canonical domain (-8..8 convention of the ETEX mini fixture); snapshot xlon0=-2.0",
            "native north-to-south latitude order reordered to canonical south-to-north; Y index 0 is ylat0=48.0",
            "provider paramId (130/131/132/133) mapped to canonical temperature/wind_u/wind_v/specific_humidity; surface pressure read from the checked-in surface archive",
            "no numeric unit conversion: ERA5 model-level temperature is K, u/v are m/s, specific humidity is kg/kg, surface pressure from the ARCO-ERA5 archive is Pa",
            "vertical metadata taken verbatim from the native PV records (a in Pa, b dimensionless); full-level coefficients derived by half-level averaging; level_values/interface_values are the explicit reference pressures a+b*101325 Pa",
            "time expressed as UTC epoch seconds; calendar gregorian; temporal kind instantaneous",
        ],
        "oracle_reference": {
            "name": "FLEXPART",
            "version": "11.1",
            "pinned_commit": "c70586c2b7f5258850705325881c61f557ea9bd8",
            "manifest": "reference/flexpart-11.1.json",
        },
        "notes": [
            "This artifact demonstrates canonical normalization and reproducibility only; it makes no scientific parity claim.",
            "The fixture is validated against flexpart-gpu::meteorology::Requirements::real_data_native_levels (wind_u, wind_v, temperature, specific_humidity, surface_pressure); it intentionally does not satisfy Requirements::advection().",
        ],
    }
    provenance_path = args.output.with_name(args.output.name.replace(".json", ".provenance.json"))
    provenance_path.write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")

    print(f"wrote fixture {args.output} ({len(fixture_bytes)} bytes)")
    print(f"wrote provenance {provenance_path}")
    print(f"artifact sha256 {provenance['artifact']['sha256']}")
    print("slice 1994-10-23 15:00 UTC; 2 x 2 cells; 137 native levels; "
          "fields wind_u wind_v temperature specific_humidity surface_pressure")


if __name__ == "__main__":
    main()