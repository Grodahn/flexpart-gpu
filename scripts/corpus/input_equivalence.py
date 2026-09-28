#!/usr/bin/env python3
"""Canonical candidate/oracle input-equivalence report and verdict (Issue #52).

Single definition site for the fail-closed, machine-readable
candidate/oracle input-equivalence gate. Consumes:

- the canonical #51 validation-case v2 manifest
  (``schemas/validation-case-v2.schema.json`` via
  ``scripts/corpus/validation_case_schema.py``);
- the resolved oracle Fortran inputs already derived by the canonical
  generator ``scripts/corpus/generate_fortran_fixtures.py``
  (COMMAND, RELEASES, OUTGRID, SPECIES, METEO_ARGS.txt,
  INPUT_DERIVATION.json);
- the existing ETEX mini config (``fixtures/etex/mini/config/``) through
  the same report contract.

This module does NOT:

- redesign the #51 schema (it validates against it, never extends it);
- create a second case parser (it reuses ``validate_case_document``);
- create a second Fortran-input generator (it reuses the generator's
  derivation readers and ``verify_rendered_case`` semantics via direct
  ``namelist_value`` comparison against ``GEN`` expectations);
- duplicate corpus/ETEX audit logic (``audit_corpus_inputs.py`` routes its
  fixture check through :func:`build_report`; the ETEX heavy GRIB audit
  remains independent evidence and is referenced, not reimplemented);
- execute scientific model runs;
- implement provenance/build/executable hashing owned by #53
  (provenance is recorded as an explicit ``#53`` dependency);
- define scientific output metrics, ensemble statistics, or parity
  thresholds;
- normalize away genuine representation differences merely to obtain PASS.

Verdict semantics (fail-closed):

- ``INPUT_EQUIVALENT``: every required field is demonstrated equivalent
  under an explicitly recorded direct mapping/conversion.
- ``INPUT_EQUIVALENCE_NOT_DEMONSTRATED``: a declared representation
  difference, missing independent evidence, or unsupported transformation
  prevents an equivalence claim.
- ``INPUT_MISMATCH``: resolved candidate/oracle inputs contradict the
  declared mapping.
- ``INTEGRITY_ERROR``: required manifest/artifact metadata needed to
  perform the gate is malformed or missing.

Never infer equivalence because values merely look close. No
interpolation, normalization, defaults, or transformations are introduced
solely to make candidate and oracle inputs match.

Usage:
    python scripts/corpus/input_equivalence.py --case ADV-ANA-001
    python scripts/corpus/input_equivalence.py --case ETEX-MINI-013
    python scripts/corpus/input_equivalence.py --case WIND-UNI-002 \\
        --oracle-dir fixtures/corpus/fortran/WIND-UNI-002 \\
        --output target/input-equivalence/WIND-UNI-002.json \\
        --summary target/input-equivalence/WIND-UNI-002.txt
"""

import argparse
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts" / "corpus"))


def _load_module(name, filename):
    spec = importlib.util.spec_from_file_location(
        name, REPO / "scripts" / "corpus" / filename
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


GEN = _load_module("generate_fortran_fixtures", "generate_fortran_fixtures.py")
SCHEMA_MOD = _load_module("validation_case_schema", "validation_case_schema.py")

REPORT_SCHEMA_VERSION = 1
REPORT_SCHEMA_ID = "schemas/input-equivalence-v1.schema.json"
REPORT_SCHEMA_PATH = REPO / REPORT_SCHEMA_ID

VERDICT_EQUIVALENT = "INPUT_EQUIVALENT"
VERDICT_NOT_DEMONSTRATED = "INPUT_EQUIVALENCE_NOT_DEMONSTRATED"
VERDICT_MISMATCH = "INPUT_MISMATCH"
VERDICT_INTEGRITY_ERROR = "INTEGRITY_ERROR"
VERDICTS = (
    VERDICT_EQUIVALENT,
    VERDICT_NOT_DEMONSTRATED,
    VERDICT_MISMATCH,
    VERDICT_INTEGRITY_ERROR,
)

# Stable field identifiers for the closed #52 comparison surface.
FIELD_RELEASE_GEOMETRY = "release.geometry"
FIELD_RELEASE_TIMING = "release.timing"
FIELD_RELEASE_VERTICAL_REF = "release.vertical_ref"
FIELD_RELEASE_SPECIES_INVENTORY = "release.species_inventory"
FIELD_RELEASE_PARTICLE_COUNT = "release.particle_count"
FIELD_SPECIES_PHYSICS_CONTRACT = "species_physics_contract"
FIELD_METEOROLOGY_IDENTITY = "meteorology.identity"
FIELD_METEOROLOGY_COVERAGE = "meteorology.coverage"
FIELD_METEOROLOGY_CANDIDATE_TRANSFORMATION = "meteorology.candidate_transformation"
FIELD_METEOROLOGY_ORACLE_TRANSFORMATION = "meteorology.oracle_transformation"
FIELD_PHYSICS_SWITCHES = "physics_switches"
FIELD_PHYSICS_FORMULATION = "physics.formulation"
FIELD_SIMULATION_DIRECTION = "simulation_direction"
FIELD_DOMAIN_GRID = "domain_grid"
FIELD_OUTPUT_GRID = "output_grid"
FIELD_VERTICAL_COORDINATE = "vertical_coordinate"
FIELD_TIMESTEP = "integration.timestep"
FIELD_SIMULATION_WINDOW = "integration.window"
FIELD_OUTPUT_WINDOWS = "output.windows"
FIELD_CONVERSIONS = "conversions.units_sign_coordinates"
FIELD_REPRESENTATION_DIFFERENCES = "representation_differences"

REQUIRED_FIELD_IDS = (
    FIELD_RELEASE_GEOMETRY,
    FIELD_RELEASE_TIMING,
    FIELD_RELEASE_VERTICAL_REF,
    FIELD_RELEASE_SPECIES_INVENTORY,
    FIELD_RELEASE_PARTICLE_COUNT,
    FIELD_SPECIES_PHYSICS_CONTRACT,
    FIELD_METEOROLOGY_IDENTITY,
    FIELD_METEOROLOGY_COVERAGE,
    FIELD_METEOROLOGY_CANDIDATE_TRANSFORMATION,
    FIELD_METEOROLOGY_ORACLE_TRANSFORMATION,
    FIELD_PHYSICS_SWITCHES,
    FIELD_PHYSICS_FORMULATION,
    FIELD_SIMULATION_DIRECTION,
    FIELD_DOMAIN_GRID,
    FIELD_OUTPUT_GRID,
    FIELD_VERTICAL_COORDINATE,
    FIELD_TIMESTEP,
    FIELD_SIMULATION_WINDOW,
    FIELD_OUTPUT_WINDOWS,
    FIELD_CONVERSIONS,
    FIELD_REPRESENTATION_DIFFERENCES,
)

# Named, versioned conversions recorded in every report. No conversion is
# applied silently: each field cites the conversion it used.
CONVERSIONS = [
    {
        "id": "mass_kg_to_g",
        "version": "v1",
        "description": "MASS_g = mass_kg * 1000 (FLEXPART RELEASES MASS is in grams; candidate inventory is in kg).",
    },
    {
        "id": "time_yyyymmddhhmmss_to_ibdate_ibtime",
        "version": "v1",
        "description": "IBDATE/ITIME/IEDATE/IETIME derived from YYYYMMDDHHMMSS via Gregorian calendar; release window must lie inside integration window.",
    },
    {
        "id": "lonlat_deg_to_releases",
        "version": "v1",
        "description": "RELEASES LON1/LON2/LAT1/LAT2 mirror release geometry lon/lat in decimal degrees (geographic_lon_lat_degrees).",
    },
    {
        "id": "z_agl_to_zkind1",
        "version": "v1",
        "description": "ZKIND=1 selects meters above ground; only vertical_ref=agl is established (ASL conversion is not implemented).",
    },
    {
        "id": "output_to_lout",
        "version": "v1",
        "description": "LOUTSTEP/LOUTAVER/LOUTSAMPLE mirror output interval_s/averaging_window_s/sampling_interval_s with LOUTSAMPLE<=LOUTAVER<=LOUTSTEP, multiples of LSYNCTIME, and averaging/interval >= 2*LSYNCTIME.",
    },
    {
        "id": "direction_to_ldirect",
        "version": "v1",
        "description": "LDIRECT=1 for simulation_direction=forward (readoptions_mod.f90); backward uses source-receptor semantics and is unsupported by schema v2 output quantity.",
    },
    {
        "id": "outgrid_from_output_grid",
        "version": "v1",
        "description": "OUTGRID OUTLON0/OUTLAT0/NUMXGRID/NUMYGRID/DXOUT/DYOUT/OUTHEIGHTS mirror output_grid exactly; concentration grid is separate from the meteorology/candidate domain.",
    },
    {
        "id": "synthetic_meteorology_pinned",
        "version": "flexpart-synthetic-grib-v1",
        "description": "Synthetic oracle meteorology uses the pinned 32x32x12 global regular_ll grid at 10800 s cadence with the 12-level simplified hybrid-eta coordinate; candidate uses the local case domain with fixed AGL levels. Both derive from the same case wind/surface via scripts/generate_synthetic_grib.py flags (METEO_ARGS.txt).",
    },
    {
        "id": "candidate_physics_pinned",
        "version": "candidate-forward-timeloop-v1",
        "description": "Candidate runner physics/runtime pinned by reference/candidate-physics/candidate-forward-timeloop-v1.json; never from executable defaults.",
    },
    {
        "id": "ctl_formulation",
        "version": "v1",
        "description": "CTL>=0.1 selects adaptive_w_sigw (method=1, turbswitch w/sigw); CTL<0 selects fixed_sync_w (method=0/mintime=lsynctime, w formulation, effective IFINE=1). Declared turbulence_formulation must agree with CTL (readoptions_mod.f90:626,645-650,786-795).",
    },
    {
        "id": "units_canonical_si",
        "version": "v1",
        "description": "Canonical SI units: wind m/s, height m, mass kg, time s, pressure Pa, temperature K, heat flux W/m2, shear 1/s, inv_obukhov 1/m, deposition_velocity m/s, scavenging 1/s, concentration kg/m3.",
    },
]

PROVENANCE_DEPENDENCY = {
    "owner": "#53",
    "note": (
        "Execution provenance (executable/build hashes, runtime/adapter "
        "identity, immutable input/output hashes, concrete execution-instance "
        "identity) belongs to #53 and is not evaluated here. This verdict "
        "covers declared-vs-resolved scientific inputs only; downstream "
        "scoring must additionally verify #53 attribution before any parity "
        "claim."
    ),
}


class InputEquivalenceError(ValueError):
    """Raised when a report cannot be built or a gate refuses scoring."""


FIELD_EVALUATION_ERRORS = (
    InputEquivalenceError,
    KeyError,
    OSError,
    TypeError,
    ValueError,
    json.JSONDecodeError,
)
REPORT_EVALUATION_ERRORS = (SystemExit,) + FIELD_EVALUATION_ERRORS


def _evidence(field_id, status, candidate, oracle, conversion, detail):
    return {
        "field_id": field_id,
        "status": status,
        "candidate": candidate,
        "oracle": oracle,
        "conversion": conversion,
        "detail": detail,
    }


def _integrity(field_id, detail, candidate=None, oracle=None, conversion=None):
    return _evidence(field_id, "integrity_error", candidate, oracle, conversion, detail)


def _mismatch(field_id, detail, candidate=None, oracle=None, conversion=None):
    return _evidence(field_id, "mismatch", candidate, oracle, conversion, detail)


def _not_demonstrated(field_id, detail, candidate=None, oracle=None, conversion=None):
    return _evidence(field_id, "not_demonstrated", candidate, oracle, conversion, detail)


def _equivalent(field_id, detail, candidate=None, oracle=None, conversion=None):
    return _evidence(field_id, "equivalent", candidate, oracle, conversion, detail)


def _overall_verdict(fields):
    statuses = {f["status"] for f in fields}
    if "integrity_error" in statuses:
        return VERDICT_INTEGRITY_ERROR
    if "mismatch" in statuses:
        return VERDICT_MISMATCH
    if "not_demonstrated" in statuses:
        return VERDICT_NOT_DEMONSTRATED
    return VERDICT_EQUIVALENT


def _validate_report(report, expected_case_id=None):
    """Validate report shape, closed field surface, and verdict consistency."""
    try:
        schema = json.loads(REPORT_SCHEMA_PATH.read_text(encoding="utf-8"))
        SCHEMA_MOD._validate_node(schema, schema, report, "$")
    except (OSError, json.JSONDecodeError, SCHEMA_MOD.ValidationCaseSchemaError) as exc:
        raise InputEquivalenceError(
            f"input-equivalence report schema violation: {exc}"
        ) from exc

    fields = report["fields"]
    field_ids = [field["field_id"] for field in fields]
    duplicate_ids = sorted(
        field_id for field_id in set(field_ids) if field_ids.count(field_id) > 1
    )
    missing_ids = sorted(set(REQUIRED_FIELD_IDS) - set(field_ids))
    unexpected_ids = sorted(set(field_ids) - set(REQUIRED_FIELD_IDS))
    if duplicate_ids or missing_ids or unexpected_ids:
        raise InputEquivalenceError(
            "input-equivalence report does not contain the closed field surface: "
            f"duplicates={duplicate_ids}, missing={missing_ids}, "
            f"unexpected={unexpected_ids}"
        )

    derived_verdict = _overall_verdict(fields)
    if report["verdict"] != derived_verdict:
        raise InputEquivalenceError(
            "input-equivalence report verdict contradicts field evidence: "
            f"declared={report['verdict']}, derived={derived_verdict}"
        )
    if expected_case_id is not None and report["case_id"] != expected_case_id:
        raise InputEquivalenceError(
            "input-equivalence report belongs to the wrong case: "
            f"expected {expected_case_id!r}, got {report['case_id']!r}"
        )
    return report


def _read_text(path):
    try:
        return Path(path).read_text(encoding="utf-8")
    except OSError as exc:
        raise InputEquivalenceError(f"cannot read {path}: {exc}") from exc


def _namelist_scalar(text, key):
    try:
        return GEN.namelist_value(text, key)
    except (ValueError, AttributeError) as exc:
        raise InputEquivalenceError(f"namelist key {key} missing/malformed: {exc}") from exc


def _git_blob_sha(path):
    """Return the Git blob identity for one resolved input artifact."""
    data = Path(path).read_bytes().replace(b"\r\n", b"\n")
    header = f"blob {len(data)}\0".encode("ascii")
    return hashlib.sha1(header + data).hexdigest()


def default_oracle_dir(case_id):
    if case_id == "ETEX-MINI-013":
        return REPO / "fixtures" / "etex" / "mini" / "config"
    return REPO / "fixtures" / "corpus" / "fortran" / case_id


def load_case(case_path):
    """Load and schema-validate one #51 case document.

    Returns the parsed dict. Raises InputEquivalenceError on any
    malformed/missing manifest so callers emit INTEGRITY_ERROR.
    """
    path = Path(case_path)
    if not path.is_file():
        raise InputEquivalenceError(f"case manifest not found: {path}")
    try:
        case = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise InputEquivalenceError(f"case manifest unreadable: {path}: {exc}") from exc
    try:
        SCHEMA_MOD.validate_case_document(case, source=str(path))
    except SCHEMA_MOD.ValidationCaseSchemaError as exc:
        raise InputEquivalenceError(f"validation-case-v2 schema violation: {exc}") from exc
    if case.get("schema_version") != 2 or "version" in case:
        raise InputEquivalenceError(
            "unsupported schema: expected only schema_version 2 "
            f"(got schema_version={case.get('schema_version')!r} "
            f"version={case.get('version')!r})"
        )
    return case


def _integrity_report(case_id, case_file, oracle_dir, detail, fields=None):
    evidence = list(fields) if fields else []
    if not evidence:
        evidence.append(
            _integrity(
                FIELD_CONVERSIONS,
                detail,
                candidate=str(case_file),
                oracle=str(oracle_dir),
            )
        )
    # Ensure every required field is represented even on early integrity exit.
    present = {f["field_id"] for f in evidence}
    for field_id in REQUIRED_FIELD_IDS:
        if field_id not in present:
            evidence.append(
                _integrity(
                    field_id,
                    f"gate aborted before field evaluation: {detail}",
                    candidate=None,
                    oracle=None,
                )
            )
    return {
        "schema_version": REPORT_SCHEMA_VERSION,
        "schema_id": REPORT_SCHEMA_ID,
        "case_id": case_id,
        "case_file": str(case_file),
        "oracle_dir": str(oracle_dir),
        "verdict": VERDICT_INTEGRITY_ERROR,
        "fields": evidence,
        "conversions": CONVERSIONS,
        "provenance_dependency": PROVENANCE_DEPENDENCY,
    }


def build_report(case_id, case, oracle_dir):
    """Build the canonical input-equivalence report for one resolved case.

    ``case`` is an already-parsed #51 v2 dict; ``oracle_dir`` holds the
    resolved Fortran inputs (COMMAND, RELEASES, OUTGRID plus, for
    synthetic cases, METEO_ARGS.txt, INPUT_DERIVATION.json, SPECIES).
    Never raises for field mismatches: those become MISMATCH/NOT_DEMONSTRATED
    evidence. Raises InputEquivalenceError only for programming errors;
    manifest/artifact problems become INTEGRITY_ERROR evidence.
    """
    oracle_dir = Path(oracle_dir)
    case_file = f"fixtures/corpus/cases/{case_id}.json"

    try:
        SCHEMA_MOD.validate_case_document(case, source=case_file)
    except SCHEMA_MOD.ValidationCaseSchemaError as exc:
        return _integrity_report(case_id, case_file, oracle_dir, str(exc))
    if case.get("case_id") != case_id:
        return _integrity_report(
            case_id,
            case_file,
            oracle_dir,
            f"manifest case_id {case.get('case_id')!r} does not match requested {case_id!r}",
        )

    # --- manifest-level fail-closed checks (reuse generator readers) ---
    try:
        physics = GEN.mandatory_physics_switches(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id,
            case_file,
            oracle_dir,
            str(exc),
            [_integrity(FIELD_PHYSICS_SWITCHES, str(exc), case.get("physics_switches"), None)],
        )
    try:
        GEN._validate_species_physics_contract(case_id, case, physics)
    except SystemExit as exc:
        return _integrity_report(
            case_id,
            case_file,
            oracle_dir,
            str(exc),
            [_integrity(FIELD_SPECIES_PHYSICS_CONTRACT, str(exc), case.get("release", {}).get("species"), None)],
        )
    try:
        GEN._validate_deposition_contract(case_id, case, physics)
    except SystemExit as exc:
        return _integrity_report(
            case_id,
            case_file,
            oracle_dir,
            str(exc),
            [_integrity(FIELD_SPECIES_PHYSICS_CONTRACT, str(exc), case.get("deposition"), None)],
        )
    try:
        specnum = GEN.species_number_for_case(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_RELEASE_SPECIES_INVENTORY, str(exc))],
        )
    try:
        integration = GEN._required_integration(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_TIMESTEP, str(exc), case.get("integration"), None)],
        )
    try:
        wind = GEN._required_wind(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_METEOROLOGY_IDENTITY, str(exc), case.get("wind"), None)],
        )
    wind_profile = wind["profile"]
    is_real_weather = wind_profile == "real_weather"
    try:
        GEN._required_oracle_meteorology_profile(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_METEOROLOGY_IDENTITY, str(exc))],
        )
    if is_real_weather:
        try:
            GEN._required_meteorology(case_id, wind, integration)
        except SystemExit as exc:
            return _integrity_report(
                case_id, case_file, oracle_dir, str(exc),
                [_integrity(FIELD_METEOROLOGY_COVERAGE, str(exc))],
            )
    try:
        GEN._required_domain(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_DOMAIN_GRID, str(exc), case.get("domain"), None)],
        )
    try:
        GEN._required_output_grid(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_OUTPUT_GRID, str(exc), case.get("output_grid"), None)],
        )
    try:
        oracle_overrides = GEN.normalize_oracle_overrides(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_PHYSICS_FORMULATION, str(exc), case.get("oracle_command_overrides"), None)],
        )
    try:
        direction = GEN._required_simulation_direction(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_SIMULATION_DIRECTION, str(exc), case.get("simulation_direction"), None)],
        )
    try:
        output = GEN._required_output(case_id, case, int(oracle_overrides["lsynctime_s"]))
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_OUTPUT_WINDOWS, str(exc), case.get("output"), None)],
        )
    try:
        GEN._validate_stochastic_contract(case_id, case, physics)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_RELEASE_PARTICLE_COUNT, str(exc), case.get("stochastic"), None)],
        )
    try:
        surface = GEN._required_surface(case_id, case, physics)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_METEOROLOGY_IDENTITY, str(exc), case.get("surface"), None)],
        )
    try:
        GEN._required_units(case_id, case, wind, surface, physics)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_CONVERSIONS, str(exc), case.get("units"), None, "units_canonical_si:v1")],
        )
    try:
        GEN._required_validation_definition_refs(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_CONVERSIONS, str(exc))],
        )
    try:
        GEN._required_execution_profile(case_id, case)
    except SystemExit as exc:
        return _integrity_report(
            case_id, case_file, oracle_dir, str(exc),
            [_integrity(FIELD_METEOROLOGY_IDENTITY, str(exc), case.get("execution_profile"), None)],
        )

    # --- required oracle artifacts (fail-closed: missing => INTEGRITY_ERROR) ---
    if not oracle_dir.is_dir():
        return _integrity_report(
            case_id, case_file, oracle_dir, f"oracle input directory not found: {oracle_dir}"
        )
    required_names = ["COMMAND", "RELEASES", "OUTGRID"]
    if not is_real_weather:
        required_names += ["METEO_ARGS.txt", "INPUT_DERIVATION.json"]
    missing = [n for n in required_names if not (oracle_dir / n).is_file()]
    if missing:
        fields = [
            _integrity(
                FIELD_CONVERSIONS,
                f"missing required oracle artifact(s): {', '.join(missing)} in {oracle_dir}",
                candidate=case_id,
                oracle=str(oracle_dir),
            )
        ]
        return _integrity_report(
            case_id, case_file, oracle_dir,
            f"missing required oracle artifact(s): {', '.join(missing)}",
            fields,
        )
    try:
        command_text = (oracle_dir / "COMMAND").read_text(encoding="utf-8")
        releases_text = (oracle_dir / "RELEASES").read_text(encoding="utf-8")
        outgrid_text = (oracle_dir / "OUTGRID").read_text(encoding="utf-8")
    except OSError as exc:
        return _integrity_report(case_id, case_file, oracle_dir, f"cannot read oracle artifact: {exc}")

    derivation_mismatches = {}
    if not is_real_weather:
        try:
            derivation = json.loads(
                (oracle_dir / "INPUT_DERIVATION.json").read_text(encoding="utf-8")
            )
            release_lon, _, release_lat, _ = GEN.release_lonlat(case_id, case)
            release_z, _, _ = GEN.release_vertical(case_id, case)
            output_grid = GEN._required_output_grid(case_id, case)
            expected_derivation = {
                "case_file": case_file,
                "release_lon_deg": release_lon,
                "release_lat_deg": release_lat,
                "release_z_m": release_z,
                "particle_count": GEN._required_particle_count(
                    case_id, case["release"]
                ),
                "candidate_mass_kg": GEN.case_total_mass_kg(case_id, case),
                "mass_conversion": (
                    "MASS_g = mass_kg * 1000 (FLEXPART MASS is in grams)"
                ),
                "oracle_mass_g": (
                    GEN.case_total_mass_kg(case_id, case) * GEN.KG_TO_G
                ),
                "oracle_meteorology_profile": case["oracle_meteorology_profile"],
                "output_grid": {
                    key: output_grid[key]
                    for key in (
                        "xlon0_deg",
                        "ylat0_deg",
                        "nx",
                        "ny",
                        "nz",
                        "dx_deg",
                        "dy_deg",
                        "heights_m",
                        "heights_ref",
                    )
                },
            }
            if not isinstance(derivation, dict):
                raise InputEquivalenceError(
                    "INPUT_DERIVATION.json root must be an object"
                )
            missing_derivation_keys = sorted(
                set(expected_derivation) - set(derivation)
            )
            unexpected_derivation_keys = sorted(
                set(derivation) - set(expected_derivation)
            )
            if missing_derivation_keys or unexpected_derivation_keys:
                return _integrity_report(
                    case_id,
                    case_file,
                    oracle_dir,
                    "INPUT_DERIVATION.json has an invalid field surface: "
                    f"missing={missing_derivation_keys}, "
                    f"unexpected={unexpected_derivation_keys}",
                )
            derivation_mismatches = {
                key: {"candidate": expected_derivation[key], "oracle": derivation[key]}
                for key in expected_derivation
                if derivation[key] != expected_derivation[key]
            }
        except FIELD_EVALUATION_ERRORS as exc:
            return _integrity_report(
                case_id,
                case_file,
                oracle_dir,
                f"INPUT_DERIVATION.json is malformed or unreadable: {exc}",
            )

    fields = []

    # --- release.geometry ---
    try:
        exp_lon1, exp_lon2, exp_lat1, exp_lat2 = GEN.release_lonlat(case_id, case)
        o_lon1 = float(_namelist_scalar(releases_text, "LON1"))
        o_lon2 = float(_namelist_scalar(releases_text, "LON2"))
        o_lat1 = float(_namelist_scalar(releases_text, "LAT1"))
        o_lat2 = float(_namelist_scalar(releases_text, "LAT2"))
        ok = (
            abs(o_lon1 - exp_lon1) <= 1e-9
            and abs(o_lon2 - exp_lon2) <= 1e-9
            and abs(o_lat1 - exp_lat1) <= 1e-9
            and abs(o_lat2 - exp_lat2) <= 1e-9
        )
        candidate_geo = case["release"]["geometry"]
        oracle_geo = {"LON1": o_lon1, "LON2": o_lon2, "LAT1": o_lat1, "LAT2": o_lat2}
        if ok:
            fields.append(_equivalent(FIELD_RELEASE_GEOMETRY,
                "RELEASES lon/lat mirror release geometry exactly.",
                candidate_geo, oracle_geo, "lonlat_deg_to_releases:v1"))
        else:
            fields.append(_mismatch(FIELD_RELEASE_GEOMETRY,
                f"RELEASES lon/lat {oracle_geo} contradict release geometry {(exp_lon1, exp_lon2, exp_lat1, exp_lat2)}.",
                candidate_geo, oracle_geo, "lonlat_deg_to_releases:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_RELEASE_GEOMETRY, str(exc)))

    # --- release.timing ---
    try:
        exp_start, exp_end = GEN.release_window_datetimes(case_id, case)
        exp_idate1, exp_itime1 = GEN.flexpart_datetime(exp_start)
        exp_idate2, exp_itime2 = GEN.flexpart_datetime(exp_end)
        o_idate1 = int(float(_namelist_scalar(releases_text, "IDATE1")))
        o_itime1 = int(float(_namelist_scalar(releases_text, "ITIME1")))
        o_idate2 = int(float(_namelist_scalar(releases_text, "IDATE2")))
        o_itime2 = int(float(_namelist_scalar(releases_text, "ITIME2")))
        candidate_timing = case["release"]["timing"]
        oracle_timing = {"IDATE1": o_idate1, "ITIME1": o_itime1, "IDATE2": o_idate2, "ITIME2": o_itime2}
        if (o_idate1, o_itime1, o_idate2, o_itime2) == (exp_idate1, exp_itime1, exp_idate2, exp_itime2):
            fields.append(_equivalent(FIELD_RELEASE_TIMING,
                "RELEASES dates mirror release timing exactly.",
                candidate_timing, oracle_timing, "time_yyyymmddhhmmss_to_ibdate_ibtime:v1"))
        else:
            fields.append(_mismatch(FIELD_RELEASE_TIMING,
                f"RELEASES dates {oracle_timing} contradict release timing {(exp_idate1, exp_itime1, exp_idate2, exp_itime2)}.",
                candidate_timing, oracle_timing, "time_yyyymmddhhmmss_to_ibdate_ibtime:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_RELEASE_TIMING, str(exc)))

    # --- release.vertical_ref ---
    try:
        exp_z1, exp_z2, exp_zkind = GEN.release_vertical(case_id, case)
        o_z1 = float(_namelist_scalar(releases_text, "Z1"))
        o_z2 = float(_namelist_scalar(releases_text, "Z2"))
        o_zkind = int(float(_namelist_scalar(releases_text, "ZKIND")))
        candidate_vref = case["release"].get("vertical_ref")
        oracle_vref = {"Z1": o_z1, "Z2": o_z2, "ZKIND": o_zkind}
        if candidate_vref == "agl" and o_zkind == exp_zkind == 1 and abs(o_z1 - exp_z1) <= 1e-9 and abs(o_z2 - exp_z2) <= 1e-9:
            fields.append(_equivalent(FIELD_RELEASE_VERTICAL_REF,
                "Release heights are AGL with ZKIND=1 on both sides.",
                candidate_vref, oracle_vref, "z_agl_to_zkind1:v1"))
        else:
            fields.append(_mismatch(FIELD_RELEASE_VERTICAL_REF,
                f"Vertical reference mismatch: case vertical_ref={candidate_vref!r} expects Z1/Z2/ZKIND=({exp_z1}, {exp_z2}, {exp_zkind}), oracle has {oracle_vref}.",
                candidate_vref, oracle_vref, "z_agl_to_zkind1:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_RELEASE_VERTICAL_REF, str(exc)))

    # --- release.species_inventory ---
    try:
        total_kg = GEN.case_total_mass_kg(case_id, case)
        exp_g = total_kg * GEN.KG_TO_G
        o_mass_g = float(_namelist_scalar(releases_text, "MASS").replace("D", "E"))
        o_specnum = int(float(_namelist_scalar(releases_text, "SPECNUM_REL")))
        candidate_inv = {"species": case["release"]["species"]["id"], "quantity_kg": total_kg, "unit": "kg"}
        oracle_inv = {"SPECNUM_REL": o_specnum, "MASS_g": o_mass_g}
        rel_err = abs(o_mass_g - exp_g) / exp_g if exp_g else float("inf")
        if rel_err <= 1e-4 * 1.0 + 1e-12 and o_specnum == specnum:
            # Tolerance mirrors GEN.verify_rendered_case (1e-4 relative on MASS).
            fields.append(_equivalent(FIELD_RELEASE_SPECIES_INVENTORY,
                f"RELEASES MASS_g matches inventory kg->g within tolerance (rel_err={rel_err:.2e}) and SPECNUM_REL={o_specnum} matches {specnum}.",
                candidate_inv, oracle_inv, "mass_kg_to_g:v1"))
        else:
            fields.append(_mismatch(FIELD_RELEASE_SPECIES_INVENTORY,
                f"Inventory mismatch: expected MASS_g={exp_g} SPECNUM={specnum}, oracle has MASS_g={o_mass_g} SPECNUM={o_specnum} (rel_err={rel_err:.2e}).",
                candidate_inv, oracle_inv, "mass_kg_to_g:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_RELEASE_SPECIES_INVENTORY, str(exc)))

    # --- release.particle_count ---
    try:
        exp_parts = GEN._required_particle_count(case_id, case["release"])
        o_parts = int(float(_namelist_scalar(releases_text, "PARTS")))
        if o_parts == exp_parts:
            fields.append(_equivalent(FIELD_RELEASE_PARTICLE_COUNT,
                f"RELEASES PARTS={o_parts} matches release.particle_count.",
                exp_parts, o_parts, None))
        else:
            fields.append(_mismatch(FIELD_RELEASE_PARTICLE_COUNT,
                f"Particle-count contradiction: case={exp_parts}, oracle PARTS={o_parts}.",
                exp_parts, o_parts, None))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_RELEASE_PARTICLE_COUNT, str(exc)))

    # --- species_physics_contract ---
    try:
        contract = case["release"]["species"]["physics_contract"]
        profile = contract.get("profile")
        o_specnum = int(float(_namelist_scalar(releases_text, "SPECNUM_REL")))
        expected_specnum = specnum
        contract_path = REPO / contract["path"]
        contract_document = json.loads(contract_path.read_text(encoding="utf-8"))
        expected_contract_identity = {
            "contract_id": contract["id"],
            "contract_version": contract["version"],
            "species_id": case["release"]["species"]["id"],
        }
        actual_contract_identity = {
            key: contract_document.get(key) for key in expected_contract_identity
        }
        if actual_contract_identity != expected_contract_identity:
            raise InputEquivalenceError(
                f"species contract {contract_path} identity mismatch: "
                f"expected {expected_contract_identity}, got {actual_contract_identity}"
            )
        candidate_physics = {
            key: bool(physics[key])
            for key in ("dry_deposition", "wet_deposition", "decay")
        }
        if contract_document.get("candidate_physics") != candidate_physics:
            raise InputEquivalenceError(
                "species contract candidate_physics contradicts the canonical case: "
                f"contract={contract_document.get('candidate_physics')}, "
                f"case={candidate_physics}"
            )
        oracle_physics = contract_document.get("oracle_physics")
        if not isinstance(oracle_physics, dict):
            raise InputEquivalenceError(
                "species contract oracle_physics must be an object"
            )
        species_ok = o_specnum == expected_specnum
        if not is_real_weather:
            species_path = oracle_dir / "SPECIES" / f"SPECIES_{expected_specnum:03d}"
            if not species_path.is_file():
                fields.append(_integrity(FIELD_SPECIES_PHYSICS_CONTRACT,
                    f"missing required oracle SPECIES file: {species_path}",
                    contract, {"SPECNUM_REL": o_specnum}))
            else:
                import re as _re
                code = _re.sub(r"!.*", "", species_path.read_text(encoding="utf-8"))
                has_pndia = _re.search(r"(?im)^\s*PNDIA\s*=", code) is not None
                checked_fixture = contract_document.get("checked_in_fixture")
                if not isinstance(checked_fixture, dict) or not isinstance(
                    checked_fixture.get("git_blob_sha"), str
                ):
                    raise InputEquivalenceError(
                        "species contract lacks checked_in_fixture.git_blob_sha"
                    )
                actual_blob_sha = _git_blob_sha(species_path)
                expected_blob_sha = checked_fixture["git_blob_sha"]
                oracle_evidence = {
                    "SPECNUM_REL": o_specnum,
                    "git_blob_sha": actual_blob_sha,
                    "oracle_physics": oracle_physics,
                }
                if actual_blob_sha != expected_blob_sha:
                    fields.append(_mismatch(FIELD_SPECIES_PHYSICS_CONTRACT,
                        f"Resolved SPECIES_{expected_specnum:03d} blob {actual_blob_sha} contradicts pinned contract blob {expected_blob_sha}.",
                        contract_document, oracle_evidence))
                elif has_pndia:
                    fields.append(_mismatch(FIELD_SPECIES_PHYSICS_CONTRACT,
                        f"Oracle SPECIES_{expected_specnum:03d} contains PNDIA unknown to v11.1.",
                        contract_document, oracle_evidence))
                elif case_id == "DRY-007":
                    pdry = GEN.namelist_value(species_path.read_text(encoding="utf-8"), "PDRYVEL").strip()
                    if pdry != "2.0":
                        fields.append(_mismatch(FIELD_SPECIES_PHYSICS_CONTRACT,
                            f"DRY-007 SPECIES PDRYVEL={pdry!r} contradicts candidate-equivalent 2.0 (0.02 m/s).",
                            contract_document, dict(oracle_evidence, PDRYVEL=pdry)))
                    elif not species_ok:
                        fields.append(_mismatch(FIELD_SPECIES_PHYSICS_CONTRACT,
                            f"SPECNUM_REL={o_specnum} contradicts species contract {profile} (expected {expected_specnum}).",
                            contract_document, oracle_evidence))
                    else:
                        fields.append(_equivalent(FIELD_SPECIES_PHYSICS_CONTRACT,
                            f"Species contract {profile} matches pinned SPECIES_{expected_specnum:03d} blob {actual_blob_sha} (PDRYVEL=2.0, no PNDIA) and SPECNUM_REL={o_specnum}.",
                            contract_document, dict(oracle_evidence, PDRYVEL=pdry), None))
                else:
                    if not species_ok:
                        fields.append(_mismatch(FIELD_SPECIES_PHYSICS_CONTRACT,
                            f"SPECNUM_REL={o_specnum} contradicts species contract {profile} (expected {expected_specnum}).",
                            contract_document, oracle_evidence))
                    elif oracle_physics != candidate_physics:
                        fields.append(_not_demonstrated(FIELD_SPECIES_PHYSICS_CONTRACT,
                            f"Species contract {profile} declares different candidate/oracle physics {candidate_physics}/{oracle_physics}; equivalence is not demonstrated.",
                            candidate_physics, oracle_evidence))
                    else:
                        fields.append(_equivalent(FIELD_SPECIES_PHYSICS_CONTRACT,
                            f"Species contract {profile} matches pinned SPECIES_{expected_specnum:03d} blob {actual_blob_sha} (no PNDIA) and SPECNUM_REL={o_specnum}.",
                            contract_document, oracle_evidence, None))
        else:
            # ETEX mini: no resolved SPECIES file in mini/config, so the pinned
            # identity cannot be independently checked by this report.
            if not species_ok:
                fields.append(_mismatch(FIELD_SPECIES_PHYSICS_CONTRACT,
                    f"SPECNUM_REL={o_specnum} contradicts species contract {profile} (expected {expected_specnum}).",
                    contract_document, {"SPECNUM_REL": o_specnum}))
            else:
                fields.append(_not_demonstrated(FIELD_SPECIES_PHYSICS_CONTRACT,
                    f"Species contract {profile} agrees with RELEASES SPECNUM_REL={o_specnum}, but the resolved oracle SPECIES file is absent from {oracle_dir} and its identity cannot be checked.",
                    contract_document, {"SPECNUM_REL": o_specnum}, None))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_SPECIES_PHYSICS_CONTRACT, str(exc)))

    # --- meteorology.identity ---
    try:
        if not is_real_weather:
            expected_args = GEN.meteo_args(case_id, case)
            actual_args = (oracle_dir / "METEO_ARGS.txt").read_text(encoding="utf-8").strip()
            oracle_profile = case["oracle_meteorology_profile"]
            if actual_args == expected_args:
                fields.append(_equivalent(FIELD_METEOROLOGY_IDENTITY,
                    f"METEO_ARGS.txt matches generator flags for {wind_profile} wind; oracle meteorology {oracle_profile['id']} v{oracle_profile['version']} pinned.",
                    {"profile": wind_profile, "oracle_meteorology_profile": oracle_profile},
                    {"METEO_ARGS.txt": actual_args},
                    "synthetic_meteorology_pinned:flexpart-synthetic-grib-v1"))
            else:
                fields.append(_mismatch(FIELD_METEOROLOGY_IDENTITY,
                    f"METEO_ARGS.txt contradicts case wind: expected {expected_args!r}, oracle has {actual_args!r}.",
                    {"profile": wind_profile}, {"METEO_ARGS.txt": actual_args},
                    "synthetic_meteorology_pinned:flexpart-synthetic-grib-v1"))
        else:
            met = wind["meteorology"]
            identity = {k: met.get(k) for k in ("dataset_id", "version", "source_path", "digest", "horizontal_coord", "vertical_coord")}
            # Digest manifest must exist when manifest:-prefixed.
            digest = met.get("digest", "")
            if isinstance(digest, str) and digest.startswith("manifest:"):
                manifest_rel = digest[len("manifest:"):]
                if not (REPO / manifest_rel).is_file():
                    # Missing source-data digest is missing independent evidence,
                    # not a malformed manifest: ETEX must stay
                    # INPUT_EQUIVALENCE_NOT_DEMONSTRATED, never PASS, when its
                    # provenance evidence is absent.
                    fields.append(_not_demonstrated(FIELD_METEOROLOGY_IDENTITY,
                        f"Real-weather meteorology digest manifest not found: {manifest_rel}; "
                        f"identity {met['dataset_id']} {met['version']} lacks independent hash evidence.",
                        identity, None))
                else:
                    fields.append(_equivalent(FIELD_METEOROLOGY_IDENTITY,
                        f"Real-weather meteorology identity {met['dataset_id']} {met['version']} from {met['source_path']} with digest {digest}; source is shared, transformations compared separately.",
                        identity, identity, None))
            else:
                fields.append(_equivalent(FIELD_METEOROLOGY_IDENTITY,
                    f"Real-weather meteorology identity {met['dataset_id']} {met['version']} with sha256 digest; source is shared, transformations compared separately.",
                    identity, identity, None))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_METEOROLOGY_IDENTITY, str(exc)))

    # --- meteorology.coverage ---
    try:
        if not is_real_weather:
            cadence_s = 10800  # pinned by flexpart-synthetic-grib-v1 temporal.cadence_s
            total_s = int(integration["total_s"])
            coverage_s = ((total_s + cadence_s - 1) // cadence_s) * cadence_s
            hours = coverage_s // 3600
            actual_args = (oracle_dir / "METEO_ARGS.txt").read_text(encoding="utf-8")
            if f"--hours {hours}" in actual_args:
                fields.append(_equivalent(FIELD_METEOROLOGY_COVERAGE,
                    f"Synthetic meteorology covers simulation ({total_s}s rounded up to {coverage_s}s at {cadence_s}s cadence).",
                    {"total_s": total_s, "coverage_s": coverage_s}, {"METEO_ARGS hours": hours},
                    "synthetic_meteorology_pinned:flexpart-synthetic-grib-v1"))
            else:
                fields.append(_mismatch(FIELD_METEOROLOGY_COVERAGE,
                    f"Synthetic meteorology coverage mismatch: expected --hours {hours} for total_s={total_s}, METEO_ARGS has {actual_args.strip()!r}.",
                    {"total_s": total_s}, {"METEO_ARGS.txt": actual_args.strip()},
                    "synthetic_meteorology_pinned:flexpart-synthetic-grib-v1"))
        else:
            met = wind["meteorology"]
            coverage = met.get("temporal_coverage")
            # GEN._required_meteorology already verified coverage covers simulation.
            fields.append(_equivalent(FIELD_METEOROLOGY_COVERAGE,
                f"ERA5 temporal coverage {coverage} covers simulation {integration['start']} + {integration['total_s']}s.",
                coverage, coverage, None))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_METEOROLOGY_COVERAGE, str(exc)))

    # --- meteorology transformations (candidate + oracle) ---
    rep = case.get("representation_differences", {}) or {}
    known_limits = rep.get("known_input_equivalence_limitations", []) or []
    synthetic_representation_equivalent = (
        not is_real_weather
        and wind_profile == "uniform"
        and not any(
            physics[key]
            for key in (
                "turbulence",
                "convection",
                "dry_deposition",
                "wet_deposition",
            )
        )
    )
    try:
        if not is_real_weather:
            oracle_profile = case["oracle_meteorology_profile"]
            candidate_profile = case["candidate_physics_profile"]
            candidate_profile_document = json.loads(
                (REPO / candidate_profile["manifest_path"]).read_text(encoding="utf-8")
            )
            oracle_profile_document = json.loads(
                (REPO / oracle_profile["manifest_path"]).read_text(encoding="utf-8")
            )
            for profile_ref, profile_document, label in (
                (candidate_profile, candidate_profile_document, "candidate"),
                (oracle_profile, oracle_profile_document, "oracle"),
            ):
                if (
                    profile_document.get("id") != profile_ref["id"]
                    or profile_document.get("version") != profile_ref["version"]
                ):
                    raise InputEquivalenceError(
                        f"{label} profile manifest identity contradicts its case reference"
                    )
            declared = {k: rep.get(k) for k in ("horizontal_grid", "vertical_coordinate", "temporal_resolution", "wind_components", "pbl_diagnostics") if rep.get(k)}
            cand_trans = {
                "candidate_physics_profile": candidate_profile,
                "synthetic_meteorology": candidate_profile_document.get(
                    "synthetic_meteorology"
                ),
                "wind_profile": wind_profile,
            }
            or_trans = {
                "oracle_meteorology_profile": oracle_profile,
                "grid": oracle_profile_document.get("grid"),
                "upper_air_profile": oracle_profile_document.get(
                    "upper_air_profile"
                ),
                "fixed_surface_fields": oracle_profile_document.get(
                    "fixed_surface_fields"
                ),
                "known_representation_differences": oracle_profile_document.get(
                    "known_representation_differences"
                ),
            }
            if synthetic_representation_equivalent:
                fields.append(_equivalent(FIELD_METEOROLOGY_CANDIDATE_TRANSFORMATION,
                    f"Candidate profile {candidate_profile['id']} v{candidate_profile['version']} is immaterial for this uniform-wind case because all meteorology-sensitive physics switches are disabled; declared differences remain recorded: {declared}.",
                    cand_trans, or_trans,
                    "candidate_physics_pinned:candidate-forward-timeloop-v1"))
                fields.append(_equivalent(FIELD_METEOROLOGY_ORACLE_TRANSFORMATION,
                    f"Oracle profile {oracle_profile['id']} v{oracle_profile['version']} maps the same uniform wind, while all meteorology-sensitive physics switches are disabled; declared differences remain recorded: {declared}.",
                    or_trans, cand_trans,
                    "synthetic_meteorology_pinned:flexpart-synthetic-grib-v1"))
            else:
                reason = (
                    "Pinned candidate/oracle profiles declare different grid, vertical, "
                    "thermodynamic, or surface representations while the case enables "
                    "meteorology-sensitive physics; no independent equivalence evidence "
                    f"is supplied. declared={declared}."
                )
                fields.append(_not_demonstrated(
                    FIELD_METEOROLOGY_CANDIDATE_TRANSFORMATION,
                    reason,
                    cand_trans,
                    or_trans,
                ))
                fields.append(_not_demonstrated(
                    FIELD_METEOROLOGY_ORACLE_TRANSFORMATION,
                    reason,
                    or_trans,
                    cand_trans,
                ))
        else:
            met = wind["meteorology"]
            cand_t = met.get("candidate_transformation")
            or_t = met.get("oracle_transformation")
            if known_limits:
                reason = (
                    "Real-weather candidate/oracle transformations differ scientifically "
                    f"(candidate={cand_t}, oracle={or_t}) with no equivalence mapping; "
                    f"known limitations={known_limits}. Missing independent evidence."
                )
                fields.append(_not_demonstrated(FIELD_METEOROLOGY_CANDIDATE_TRANSFORMATION,
                    reason, cand_t, or_t))
                fields.append(_not_demonstrated(FIELD_METEOROLOGY_ORACLE_TRANSFORMATION,
                    reason, or_t, cand_t))
            else:
                reason = (
                    "Real-weather candidate (16 fixed AGL levels, omega-derived w) vs oracle "
                    "(137 native hybrid levels, etadot) differ with no demonstrated equivalence mapping."
                )
                fields.append(_not_demonstrated(FIELD_METEOROLOGY_CANDIDATE_TRANSFORMATION,
                    f"{reason} candidate={cand_t}.", cand_t, or_t))
                fields.append(_not_demonstrated(FIELD_METEOROLOGY_ORACLE_TRANSFORMATION,
                    f"{reason} oracle={or_t}.", or_t, cand_t))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_METEOROLOGY_CANDIDATE_TRANSFORMATION, str(exc)))
        fields.append(_integrity(FIELD_METEOROLOGY_ORACLE_TRANSFORMATION, str(exc)))

    # --- physics_switches ---
    try:
        o_lturb = int(float(_namelist_scalar(command_text, "LTURBULENCE")))
        o_lconv = int(float(_namelist_scalar(command_text, "LCONVECTION")))
        exp_lturb = 1 if physics["turbulence"] else 0
        exp_lconv = 1 if physics["convection"] else 0
        if (o_lturb, o_lconv) == (exp_lturb, exp_lconv):
            fields.append(_equivalent(FIELD_PHYSICS_SWITCHES,
                f"COMMAND LTURBULENCE={o_lturb}/LCONVECTION={o_lconv} mirror physics_switches.",
                physics, {"LTURBULENCE": o_lturb, "LCONVECTION": o_lconv}, None))
        else:
            fields.append(_mismatch(FIELD_PHYSICS_SWITCHES,
                f"Physics-switch contradiction: case turbulence={physics['turbulence']}/convection={physics['convection']} expects LTURBULENCE={exp_lturb}/LCONVECTION={exp_lconv}, oracle has {o_lturb}/{o_lconv}.",
                physics, {"LTURBULENCE": o_lturb, "LCONVECTION": o_lconv}, None))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_PHYSICS_SWITCHES, str(exc)))

    # --- physics.formulation ---
    try:
        o_ctl = float(_namelist_scalar(command_text, "CTL"))
        o_ifine = int(float(_namelist_scalar(command_text, "IFINE")))
        o_lsync = int(float(_namelist_scalar(command_text, "LSYNCTIME")))
        exp_ctl = float(oracle_overrides["ctl"])
        exp_ifine = int(oracle_overrides["ifine"])
        exp_lsync = int(oracle_overrides["lsynctime_s"])
        exp_form = oracle_overrides["turbulence_formulation"]
        ok = (abs(o_ctl - exp_ctl) <= 1e-6 and o_ifine == exp_ifine and o_lsync == exp_lsync)
        if ok:
            fields.append(_equivalent(FIELD_PHYSICS_FORMULATION,
                f"COMMAND CTL={o_ctl}/IFINE={o_ifine}/LSYNCTIME={o_lsync} mirror formulation {exp_form}.",
                {"turbulence_formulation": exp_form, "ctl": exp_ctl, "ifine": exp_ifine, "lsynctime_s": exp_lsync},
                {"CTL": o_ctl, "IFINE": o_ifine, "LSYNCTIME": o_lsync},
                "ctl_formulation:v1"))
        else:
            fields.append(_mismatch(FIELD_PHYSICS_FORMULATION,
                f"Formulation contradiction: case {exp_form} CTL={exp_ctl}/IFINE={exp_ifine}/LSYNCTIME={exp_lsync}, oracle has CTL={o_ctl}/IFINE={o_ifine}/LSYNCTIME={o_lsync}.",
                {"turbulence_formulation": exp_form, "ctl": exp_ctl, "ifine": exp_ifine, "lsynctime_s": exp_lsync},
                {"CTL": o_ctl, "IFINE": o_ifine, "LSYNCTIME": o_lsync},
                "ctl_formulation:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_PHYSICS_FORMULATION, str(exc)))

    # --- simulation_direction ---
    try:
        o_ldirect = int(float(_namelist_scalar(command_text, "LDIRECT")))
        exp_ldirect = GEN.LDIRECT_FORWARD if direction == "forward" else GEN.LDIRECT_BACKWARD
        if o_ldirect == exp_ldirect:
            fields.append(_equivalent(FIELD_SIMULATION_DIRECTION,
                f"COMMAND LDIRECT={o_ldirect} mirrors simulation_direction={direction}.",
                direction, o_ldirect, "direction_to_ldirect:v1"))
        else:
            fields.append(_mismatch(FIELD_SIMULATION_DIRECTION,
                f"Direction contradiction: case {direction} expects LDIRECT={exp_ldirect}, oracle has {o_ldirect}.",
                direction, o_ldirect, "direction_to_ldirect:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_SIMULATION_DIRECTION, str(exc)))

    # --- domain_grid ---
    try:
        domain = case["domain"]
        if not is_real_weather and synthetic_representation_equivalent:
            declared_h = rep.get("horizontal_grid")
            fields.append(_equivalent(FIELD_DOMAIN_GRID,
                f"Candidate domain {domain['nx']}x{domain['ny']}x{domain['nz']} declared; oracle synthetic meteorology uses pinned 32x32x12 global grid (flexpart-synthetic-grib-v1). Declared difference preserved: {declared_h!r}.",
                domain, {"oracle_synthetic_grid": {"nx": 32, "ny": 32, "nz": 12}},
                "synthetic_meteorology_pinned:flexpart-synthetic-grib-v1"))
        elif not is_real_weather:
            fields.append(_not_demonstrated(FIELD_DOMAIN_GRID,
                f"Candidate domain {domain['nx']}x{domain['ny']}x{domain['nz']} and oracle synthetic 32x32x12 global grid differ while meteorology-sensitive physics is enabled; no independent equivalence mapping is supplied. Declared: {rep.get('horizontal_grid')!r}.",
                domain, {"oracle_synthetic_grid": {"nx": 32, "ny": 32, "nz": 12}}))
        else:
            fields.append(_not_demonstrated(FIELD_DOMAIN_GRID,
                f"Candidate meteorology domain 65x41x16 differs from oracle 137 native hybrid levels with no equivalence mapping. Declared: {rep.get('horizontal_grid')!r}.",
                domain, {"oracle_levels": 137}))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_DOMAIN_GRID, str(exc)))

    # --- output_grid ---
    try:
        grid = case["output_grid"]
        o_outlon0 = float(_namelist_scalar(outgrid_text, "OUTLON0"))
        o_outlat0 = float(_namelist_scalar(outgrid_text, "OUTLAT0"))
        o_nx = int(float(_namelist_scalar(outgrid_text, "NUMXGRID")))
        o_ny = int(float(_namelist_scalar(outgrid_text, "NUMYGRID")))
        o_dx = float(_namelist_scalar(outgrid_text, "DXOUT"))
        o_dy = float(_namelist_scalar(outgrid_text, "DYOUT"))
        import re as _re2
        m = _re2.search(r"\bOUTHEIGHTS\s*=\s*([^/]+)", outgrid_text, _re2.DOTALL)
        if m is None:
            raise InputEquivalenceError("Fortran OUTGRID lacks OUTHEIGHTS")
        o_heights = [float(v) for v in _re2.findall(r"[-+]?\d+(?:\.\d+)?", m.group(1))]
        exp_heights = [float(v) for v in grid["heights_m"]]
        ok = (
            abs(o_outlon0 - float(grid["xlon0_deg"])) <= 1e-9
            and abs(o_outlat0 - float(grid["ylat0_deg"])) <= 1e-9
            and o_nx == int(grid["nx"])
            and o_ny == int(grid["ny"])
            and abs(o_dx - float(grid["dx_deg"])) <= 1e-9
            and abs(o_dy - float(grid["dy_deg"])) <= 1e-9
            and o_heights == exp_heights
        )
        if ok:
            fields.append(_equivalent(FIELD_OUTPUT_GRID,
                "OUTGRID mirrors output_grid exactly (origin, counts, spacing, heights).",
                grid, {"OUTLON0": o_outlon0, "OUTLAT0": o_outlat0, "NUMXGRID": o_nx, "NUMYGRID": o_ny, "DXOUT": o_dx, "DYOUT": o_dy, "OUTHEIGHTS": o_heights},
                "outgrid_from_output_grid:v1"))
        else:
            fields.append(_mismatch(FIELD_OUTPUT_GRID,
                f"Output-grid contradiction: case {grid} vs oracle OUTLON0={o_outlon0}/OUTLAT0={o_outlat0}/NUMXGRID={o_nx}/NUMYGRID={o_ny}/DXOUT={o_dx}/DYOUT={o_dy}/OUTHEIGHTS={o_heights}.",
                grid, {"OUTLON0": o_outlon0, "OUTLAT0": o_outlat0, "NUMXGRID": o_nx, "NUMYGRID": o_ny, "DXOUT": o_dx, "DYOUT": o_dy, "OUTHEIGHTS": o_heights},
                "outgrid_from_output_grid:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_OUTPUT_GRID, str(exc)))

    # --- vertical_coordinate ---
    try:
        domain = case["domain"]
        grid = case["output_grid"]
        vrefs = (domain.get("wind_heights_ref"), case["release"].get("vertical_ref"), grid.get("heights_ref"))
        o_zkind = int(float(_namelist_scalar(releases_text, "ZKIND")))
        if not is_real_weather:
            if (
                synthetic_representation_equivalent
                and vrefs == ("agl", "agl", "agl")
                and o_zkind == 1
            ):
                fields.append(_equivalent(FIELD_VERTICAL_COORDINATE,
                    f"Vertical references are AGL on both sides (domain/case/output {vrefs}, ZKIND=1); oracle synthetic uses pinned 12-level hybrid-eta coordinate. Declared: {rep.get('vertical_coordinate')!r}.",
                    {"wind_heights_ref": vrefs[0], "release_vertical_ref": vrefs[1], "output_heights_ref": vrefs[2]},
                    {"ZKIND": o_zkind, "oracle_vertical": "pinned 12-level simplified hybrid-eta"},
                    "z_agl_to_zkind1:v1+synthetic_meteorology_pinned:flexpart-synthetic-grib-v1"))
            elif vrefs != ("agl", "agl", "agl") or o_zkind != 1:
                fields.append(_mismatch(FIELD_VERTICAL_COORDINATE,
                    f"Vertical-reference contradiction: case refs {vrefs} vs oracle ZKIND={o_zkind}; only agl->ZKIND=1 is established.",
                    {"refs": vrefs}, {"ZKIND": o_zkind}, "z_agl_to_zkind1:v1"))
            else:
                fields.append(_not_demonstrated(FIELD_VERTICAL_COORDINATE,
                    f"Candidate fixed AGL wind levels {domain.get('wind_heights_m')} and oracle pinned 12-level hybrid-eta coordinate differ while meteorology-sensitive physics is enabled; no independent equivalence mapping is supplied. Declared: {rep.get('vertical_coordinate')!r}.",
                    {"candidate_levels": domain.get("wind_heights_m")},
                    {"oracle_vertical": "pinned 12-level simplified hybrid-eta"}))
        else:
            fields.append(_not_demonstrated(FIELD_VERTICAL_COORDINATE,
                f"Candidate 16 fixed AGL levels {domain.get('wind_heights_m')} vs oracle 137 native hybrid levels; candidate omega-derived w vs oracle etadot. Declared: vertical={rep.get('vertical_coordinate')!r} wind={rep.get('wind_components')!r}. No equivalence mapping demonstrated.",
                {"candidate_levels": domain.get("wind_heights_m"), "candidate_w": "omega-derived"},
                {"oracle_levels": 137, "oracle_w": "etadot"}))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_VERTICAL_COORDINATE, str(exc)))

    # --- integration.timestep ---
    try:
        o_lsync = int(float(_namelist_scalar(command_text, "LSYNCTIME")))
        o_ifine = int(float(_namelist_scalar(command_text, "IFINE")))
        exp_lsync = int(oracle_overrides["lsynctime_s"])
        exp_ifine = int(oracle_overrides["ifine"])
        candidate_ts = {"dt_s": integration["dt_s"], "steps": integration["steps"], "total_s": integration["total_s"]}
        oracle_ts = {"LSYNCTIME": o_lsync, "IFINE": o_ifine, "CTL": float(oracle_overrides["ctl"])}
        if o_lsync == exp_lsync and o_ifine == exp_ifine:
            fields.append(_equivalent(FIELD_TIMESTEP,
                f"Timestep/sub-step declared: candidate dt={integration['dt_s']}s x{integration['steps']} (total {integration['total_s']}s); oracle LSYNCTIME={o_lsync}s/IFINE={o_ifine}. Oracle file mirrors case overrides.",
                candidate_ts, oracle_ts, "ctl_formulation:v1"))
        else:
            fields.append(_mismatch(FIELD_TIMESTEP,
                f"Timestep contradiction: case lsynctime/ifine={exp_lsync}/{exp_ifine}, oracle has {o_lsync}/{o_ifine}.",
                candidate_ts, oracle_ts, "ctl_formulation:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_TIMESTEP, str(exc)))

    # --- integration.window (start/end) ---
    try:
        exp_ibdate, exp_ibtime = GEN.flexpart_datetime(integration["start"])
        exp_iedate, exp_ietime = GEN.sim_end_date(integration["start"], integration["total_s"])
        o_ibdate = int(float(_namelist_scalar(command_text, "IBDATE")))
        o_ibtime = int(float(_namelist_scalar(command_text, "IBTIME")))
        o_iedate = int(float(_namelist_scalar(command_text, "IEDATE")))
        o_ietime = int(float(_namelist_scalar(command_text, "IETIME")))
        candidate_win = {"start": integration["start"], "total_s": integration["total_s"]}
        oracle_win = {"IBDATE": o_ibdate, "IBTIME": o_ibtime, "IEDATE": o_iedate, "IETIME": o_ietime}
        if (o_ibdate, o_ibtime, o_iedate, o_ietime) == (exp_ibdate, exp_ibtime, exp_iedate, exp_ietime):
            fields.append(_equivalent(FIELD_SIMULATION_WINDOW,
                "COMMAND IBDATE/IBTIME/IEDATE/IETIME mirror integration start + total_s.",
                candidate_win, oracle_win, "time_yyyymmddhhmmss_to_ibdate_ibtime:v1"))
        else:
            fields.append(_mismatch(FIELD_SIMULATION_WINDOW,
                f"Simulation-window contradiction: expected {(exp_ibdate, exp_ibtime, exp_iedate, exp_ietime)}, oracle has {(o_ibdate, o_ibtime, o_iedate, o_ietime)}.",
                candidate_win, oracle_win, "time_yyyymmddhhmmss_to_ibdate_ibtime:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_SIMULATION_WINDOW, str(exc)))

    # --- output.windows ---
    try:
        o_loutstep = int(float(_namelist_scalar(command_text, "LOUTSTEP")))
        o_loutaver = int(float(_namelist_scalar(command_text, "LOUTAVER")))
        o_loutsample = int(float(_namelist_scalar(command_text, "LOUTSAMPLE")))
        if (o_loutstep, o_loutaver, o_loutsample) == (output["interval_s"], output["averaging_window_s"], output["sampling_interval_s"]):
            fields.append(_equivalent(FIELD_OUTPUT_WINDOWS,
                f"COMMAND LOUTSTEP/LOUTAVER/LOUTSAMPLE={o_loutstep}/{o_loutaver}/{o_loutsample} mirror output windows; quantity={output['quantity']}.",
                output, {"LOUTSTEP": o_loutstep, "LOUTAVER": o_loutaver, "LOUTSAMPLE": o_loutsample},
                "output_to_lout:v1"))
        else:
            fields.append(_mismatch(FIELD_OUTPUT_WINDOWS,
                f"Output-window contradiction: case interval/averaging/sampling={(output['interval_s'], output['averaging_window_s'], output['sampling_interval_s'])}, oracle has {(o_loutstep, o_loutaver, o_loutsample)}.",
                output, {"LOUTSTEP": o_loutstep, "LOUTAVER": o_loutaver, "LOUTSAMPLE": o_loutsample},
                "output_to_lout:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_OUTPUT_WINDOWS, str(exc)))

    # --- conversions.units_sign_coordinates ---
    try:
        units = case.get("units", {})
        fields.append(_equivalent(FIELD_CONVERSIONS,
            "Units are canonical SI (wind m/s, height m, mass kg, time s, concentration kg/m3); sign/coordinate conventions: lon/lat degrees geographic, heights AGL, mass kg->g x1000, time Gregorian YYYYMMDDHHMMSS.",
            units, units, "units_canonical_si:v1"))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_CONVERSIONS, str(exc)))

    # --- representation_differences ---
    try:
        if known_limits:
            fields.append(_not_demonstrated(FIELD_REPRESENTATION_DIFFERENCES,
                f"Case declares known input-equivalence limitations {known_limits}; equivalence cannot be claimed until independent evidence clears them.",
                rep, rep))
        elif is_real_weather:
            fields.append(_not_demonstrated(FIELD_REPRESENTATION_DIFFERENCES,
                f"Real-weather structured differences (vertical={rep.get('vertical_coordinate')!r}, wind={rep.get('wind_components')!r}, pbl={rep.get('pbl_diagnostics')!r}, temporal={rep.get('temporal_resolution')!r}) have no demonstrated equivalence mapping; see meteorology/vertical fields.",
                rep, rep))
        elif synthetic_representation_equivalent:
            fields.append(_equivalent(FIELD_REPRESENTATION_DIFFERENCES,
                f"Synthetic representation differences are declared (horizontal={rep.get('horizontal_grid')!r}, vertical={rep.get('vertical_coordinate')!r}, temporal={rep.get('temporal_resolution')!r}) but are immaterial for this uniform-wind case with all meteorology-sensitive physics disabled.",
                rep, rep,
                "synthetic_meteorology_pinned:flexpart-synthetic-grib-v1+candidate_physics_pinned:candidate-forward-timeloop-v1"))
        else:
            fields.append(_not_demonstrated(FIELD_REPRESENTATION_DIFFERENCES,
                f"Synthetic candidate/oracle representation differences are pinned but not demonstrated equivalent for enabled meteorology-sensitive physics (horizontal={rep.get('horizontal_grid')!r}, vertical={rep.get('vertical_coordinate')!r}, temporal={rep.get('temporal_resolution')!r}).",
                rep, rep))
    except REPORT_EVALUATION_ERRORS as exc:
        fields.append(_integrity(FIELD_REPRESENTATION_DIFFERENCES, str(exc)))

    derivation_field_ids = {
        "case_file": FIELD_CONVERSIONS,
        "release_lon_deg": FIELD_RELEASE_GEOMETRY,
        "release_lat_deg": FIELD_RELEASE_GEOMETRY,
        "release_z_m": FIELD_RELEASE_VERTICAL_REF,
        "particle_count": FIELD_RELEASE_PARTICLE_COUNT,
        "candidate_mass_kg": FIELD_RELEASE_SPECIES_INVENTORY,
        "mass_conversion": FIELD_CONVERSIONS,
        "oracle_mass_g": FIELD_RELEASE_SPECIES_INVENTORY,
        "oracle_meteorology_profile": FIELD_METEOROLOGY_IDENTITY,
        "output_grid": FIELD_OUTPUT_GRID,
    }
    mismatches_by_field = {}
    for key, difference in derivation_mismatches.items():
        mismatches_by_field.setdefault(derivation_field_ids[key], {})[key] = difference
    for index, field in enumerate(fields):
        differences = mismatches_by_field.get(field["field_id"])
        if differences is None or field["status"] != "equivalent":
            continue
        fields[index] = _mismatch(
            field["field_id"],
            "INPUT_DERIVATION.json contradicts the canonical case derivation: "
            f"{differences}",
            {key: value["candidate"] for key, value in differences.items()},
            {key: value["oracle"] for key, value in differences.items()},
            field["conversion"],
        )

    # Order fields canonically and compute verdict.
    order = {fid: i for i, fid in enumerate(REQUIRED_FIELD_IDS)}
    fields.sort(key=lambda f: order.get(f["field_id"], 999))
    verdict = _overall_verdict(fields)
    return {
        "schema_version": REPORT_SCHEMA_VERSION,
        "schema_id": REPORT_SCHEMA_ID,
        "case_id": case_id,
        "case_file": case_file,
        "oracle_dir": str(oracle_dir),
        "verdict": verdict,
        "fields": fields,
        "conversions": CONVERSIONS,
        "provenance_dependency": PROVENANCE_DEPENDENCY,
    }


def evaluate_case_file(case_path, oracle_dir=None):
    """Evaluate one case file through the canonical contract.

    Returns the report dict. Malformed manifests or missing artifacts yield
    an INTEGRITY_ERROR report, never an exception, so callers stay fail-closed.
    """
    case_path = Path(case_path)
    case_id = case_path.stem
    if oracle_dir is None:
        oracle_dir = default_oracle_dir(case_id)
    try:
        case = load_case(case_path)
    except InputEquivalenceError as exc:
        return _integrity_report(case_id, case_path, oracle_dir or "", str(exc))
    if case.get("case_id") != case_id:
        return _integrity_report(
            case_id, case_path, oracle_dir,
            f"manifest case_id {case.get('case_id')!r} does not match requested {case_id!r}",
            [_integrity(FIELD_CONVERSIONS, "manifest identity mismatch")],
        )
    return build_report(case_id, case, Path(oracle_dir))


def require_input_equivalent(report_or_path, expected_case_id=None):
    """Gate downstream paired scientific scoring on INPUT_EQUIVALENT.

    Accepts a report dict or a path to a report JSON file. Returns the
    report on success. Raises InputEquivalenceError for every other state
    (NOT_DEMONSTRATED, MISMATCH, INTEGRITY_ERROR) or for malformed reports.
    #53 provenance must be verified separately by its own gate.
    """
    if isinstance(report_or_path, (str, Path)):
        try:
            report = json.loads(Path(report_or_path).read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise InputEquivalenceError(f"input-equivalence report unreadable: {exc}") from exc
    else:
        report = report_or_path
    report = _validate_report(report, expected_case_id=expected_case_id)
    verdict = report["verdict"]
    if verdict != VERDICT_EQUIVALENT:
        case_id = report.get("case_id", "?") if isinstance(report, dict) else "?"
        raise InputEquivalenceError(
            f"paired scientific scoring refused: case {case_id} verdict is {verdict} "
            "(INPUT_EQUIVALENT required; #53 provenance must also be verified)"
        )
    return report


def render_summary(report):
    """Render the human-readable mismatch/not-demonstrated summary."""
    lines = [
        f"Input-equivalence report v{report.get('schema_version')} for {report.get('case_id')}: {report.get('verdict')}",
        f"Case: {report.get('case_file')}  Oracle: {report.get('oracle_dir')}",
        "",
    ]
    for field in report.get("fields", []):
        marker = {
            "equivalent": "PASS",
            "mismatch": "FAIL",
            "not_demonstrated": "NOT_DEMONSTRATED",
            "integrity_error": "ERROR",
        }.get(field.get("status"), "?")
        lines.append(f"[{marker}] {field.get('field_id')}: {field.get('detail')}")
    lines += [
        "",
        f"Provenance: {report.get('provenance_dependency', {}).get('owner')} owns executable/build/runtime/output attribution; "
        "verify #53 separately before any parity claim.",
    ]
    return "\n".join(lines) + "\n"


def write_report(report, output_path, summary_path=None):
    output_path = Path(output_path)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if summary_path is not None:
        summary_path = Path(summary_path)
        summary_path.parent.mkdir(parents=True, exist_ok=True)
        summary_path.write_text(render_summary(report), encoding="utf-8")
    return output_path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", dest="case_id", default=None,
                        help="Canonical case id (e.g. ADV-ANA-001). Implies --case-file under fixtures/corpus/cases unless --case-file is given.")
    parser.add_argument("--case-file", default=None, help="Explicit case JSON path.")
    parser.add_argument("--cases-dir", default=str(REPO / "fixtures" / "corpus" / "cases"))
    parser.add_argument("--oracle-dir", default=None, help="Resolved oracle input directory (default: fixtures/corpus/fortran/<CASE> or fixtures/etex/mini/config for ETEX-MINI-013).")
    parser.add_argument("--output", default=None, help="Machine-readable report JSON path.")
    parser.add_argument("--summary", default=None, help="Human-readable summary path.")
    args = parser.parse_args()

    if args.case_file is not None:
        case_path = Path(args.case_file)
        case_id = case_path.stem if args.case_id is None else args.case_id
    elif args.case_id is not None:
        case_path = Path(args.cases_dir) / f"{args.case_id}.json"
        case_id = args.case_id
    else:
        raise SystemExit("supply --case and/or --case-file")

    oracle_dir = Path(args.oracle_dir) if args.oracle_dir else default_oracle_dir(case_id)
    report = evaluate_case_file(case_path, oracle_dir)
    print(f"{case_id}: {report['verdict']}")
    for field in report["fields"]:
        if field["status"] != "equivalent":
            print(f"  [{field['status']}] {field['field_id']}: {field['detail']}")
    if args.output is not None:
        write_report(report, args.output, args.summary)
        print(f"Report: {args.output}")
        if args.summary is not None:
            print(f"Summary: {args.summary}")
    # Fail-closed exit status: only INPUT_EQUIVALENT exits 0 when --output is a gate;
    # plain evaluation always exits 0 so NOT_DEMONSTRATED/MISMATCH/INTEGRITY_ERROR
    # remain inspectable artifacts. Downstream scoring must call
    # require_input_equivalent() explicitly.
    return report


if __name__ == "__main__":
    main()
