"""Canonical deterministic-invariant gate (Issue #54).

This module is the single evaluation path for the versioned contract
``evaluation/invariants/deterministic-invariants-v1.json``. It turns
normalized per-realization evidence (one corpus candidate seed file, or one
FLEXPART 11.1 oracle output directory) into machine-readable invariant
records with the verdicts ``PASS``, ``FAIL``, ``INTEGRITY_ERROR``,
``NOT_APPLICABLE`` and ``BLOCKED``.

Ownership boundaries:

- The #51 case manifest decides whether a process or output is declared;
  the contract's ``applies_when`` conditions read it, they never copy it.
- Mass-budget mathematics stays in :func:`metrics.mass_budget`; grid
  geometry comparison stays in :func:`report.check_grid_alignment`.
- Artifact hashes and execution identities are verified by the #53 library
  (``scripts/provenance/run_provenance.py``); this module only consumes the
  verified result.
- #52 input equivalence is never inferred from invariants; paired scoring
  eligibility is recorded separately by the evaluator.

Thresholds are referenced by the contract and read from their versioned
source files at load time; no threshold value is hard-coded here.
"""

import hashlib
import json
import math
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
# The #53 library is needed by provenance_context; make the module usable
# by downstream consumers that import it without evaluate_case.
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "provenance"))

import metrics
import report as report_lib

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CONTRACT = REPO_ROOT / "evaluation" / "invariants" / "deterministic-invariants-v1.json"
CONTRACT_ID = "flexpart-gpu.deterministic-invariants"
CONTRACT_VERSION = 1

PASS = "PASS"
FAIL = "FAIL"
INTEGRITY_ERROR = "INTEGRITY_ERROR"
NOT_APPLICABLE = "NOT_APPLICABLE"
BLOCKED = "BLOCKED"
VERDICTS = (PASS, FAIL, INTEGRITY_ERROR, NOT_APPLICABLE, BLOCKED)
PRECEDENCE = (INTEGRITY_ERROR, FAIL, BLOCKED, PASS, NOT_APPLICABLE)

ROLE_CANDIDATE = "candidate"
ROLE_ORACLE = "oracle"
KIND_PARTICLES = "particle_end_state"
KIND_GRIDDED = "gridded_output"

# Offender lists are truncated for report size; offender_count stays exact.
MAX_LISTED_OFFENDERS = 50

DEFINITION_KEYS = frozenset({
    "id", "version", "family", "meaning", "scope", "roles", "applies_when",
    "required_artifacts", "quantities", "normalization", "owner",
    "comparison", "pass", "fail", "missing_evidence", "integrity_error",
})
CONTRACT_KEYS = frozenset({
    "contract_id", "contract_version", "date", "issue", "evaluator",
    "report_schema_version", "scope", "verdicts", "aggregation",
    "threshold_sources", "error_ids", "invariants", "paired_scoring",
    "downstream_handoff", "change_policy",
})
CONDITION_OPS = ("equals", "present")

# Comparison operators the implementation applies to referenced criteria; a
# threshold file that changes the operator must not be applied silently.
REQUIRED_CRITERION_OPERATORS = {"INV-MASS-CLOSURE": "abs_lt"}

# Enabled-process -> artifact reservoir name used by the mass identity.
PROCESS_RESERVOIRS = (
    ("dry_deposition", "dry_deposited_kg"),
    ("wet_deposition", "wet_deposited_kg"),
    ("decay", "decayed_kg"),
)


class InvariantContractError(ValueError):
    """The invariant contract or its threshold references are invalid."""


class InvariantGateError(ValueError):
    """Fail-closed evaluation stop carrying a stable error id and gate block."""

    def __init__(self, error_id, message, block=None):
        super().__init__(f"[{error_id}] {message}")
        self.error_id = error_id
        self.block = block


# ---------------------------------------------------------------------------
# Contract loading and validation
# ---------------------------------------------------------------------------

def _canonical_digest(document):
    """SHA-256 of canonical JSON, independent of checkout line endings."""
    canonical = json.dumps(document, sort_keys=True, separators=(",", ":"),
                           ensure_ascii=False, allow_nan=False)
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def load_contract(path=DEFAULT_CONTRACT, repo_root=REPO_ROOT):
    """Load and validate the invariant contract; resolve threshold references."""
    path = Path(path)
    try:
        raw = path.read_bytes()
        document = json.loads(raw.decode("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise InvariantContractError(f"invariant contract unreadable: {path}: {exc}") from None
    validate_contract(document)
    digest = _canonical_digest(document)
    document["_path"] = str(path)
    document["_sha256"] = digest
    document["_resolved"] = _resolve_thresholds(document, Path(repo_root))
    document["_by_id"] = {d["id"]: d for d in document["invariants"]}
    return document


def validate_contract(document):
    """Validate the closed contract surface against the implemented checks."""
    if not isinstance(document, dict):
        raise InvariantContractError("invariant contract must be a JSON object")
    keys = set(document)
    if keys != CONTRACT_KEYS:
        raise InvariantContractError(
            f"invariant contract keys differ: missing={sorted(CONTRACT_KEYS - keys)}, "
            f"unexpected={sorted(keys - CONTRACT_KEYS)}")
    if document["contract_id"] != CONTRACT_ID:
        raise InvariantContractError(f"unexpected contract_id {document['contract_id']!r}")
    if document["contract_version"] != CONTRACT_VERSION:
        raise InvariantContractError(
            f"unsupported contract_version {document['contract_version']!r}")
    if document["report_schema_version"] != report_lib.SCHEMA_VERSION:
        raise InvariantContractError(
            f"contract targets report schema {document['report_schema_version']}, "
            f"evaluator writes {report_lib.SCHEMA_VERSION}")
    if set(document["verdicts"]) != set(VERDICTS):
        raise InvariantContractError("contract verdict set differs from the evaluator")
    aggregation = document["aggregation"]
    if tuple(aggregation.get("precedence", ())) != PRECEDENCE:
        raise InvariantContractError("contract aggregation precedence differs from the evaluator")
    if aggregation.get("empty_set") != BLOCKED:
        raise InvariantContractError("an empty invariant set must aggregate to BLOCKED")
    error_ids = document["error_ids"]
    if not isinstance(error_ids, dict) or not error_ids:
        raise InvariantContractError("contract error_ids must be a non-empty object")
    for error_id, verdict in error_ids.items():
        if verdict not in (FAIL, INTEGRITY_ERROR, BLOCKED):
            raise InvariantContractError(
                f"error id {error_id} maps to {verdict!r}; only FAIL, INTEGRITY_ERROR and BLOCKED carry ids")
    definitions = document["invariants"]
    if not isinstance(definitions, list) or not definitions:
        raise InvariantContractError("contract invariants must be a non-empty array")
    seen = set()
    for definition in definitions:
        _validate_definition(definition, document)
        if definition["id"] in seen:
            raise InvariantContractError(f"duplicate invariant id {definition['id']}")
        seen.add(definition["id"])
    implemented = set(IMPLEMENTED)
    if seen != implemented:
        raise InvariantContractError(
            f"contract and evaluator invariant sets differ: "
            f"undefined={sorted(implemented - seen)}, unimplemented={sorted(seen - implemented)}")
    for definition in definitions:
        if definition["version"] != IMPLEMENTED[definition["id"]]:
            raise InvariantContractError(
                f"{definition['id']} definition version {definition['version']} differs "
                f"from implemented version {IMPLEMENTED[definition['id']]}")


def _validate_definition(definition, document):
    if not isinstance(definition, dict):
        raise InvariantContractError("invariant definition must be an object")
    keys = set(definition)
    if keys != DEFINITION_KEYS:
        raise InvariantContractError(
            f"invariant {definition.get('id')!r} keys differ: "
            f"missing={sorted(DEFINITION_KEYS - keys)}, unexpected={sorted(keys - DEFINITION_KEYS)}")
    if not isinstance(definition["id"], str) or not definition["id"].startswith("INV-"):
        raise InvariantContractError(f"invalid invariant id {definition['id']!r}")
    version = definition["version"]
    if not isinstance(version, int) or isinstance(version, bool) or version < 1:
        raise InvariantContractError(f"{definition['id']} version must be a positive integer")
    if definition["scope"] not in ("realization", "ensemble"):
        raise InvariantContractError(f"{definition['id']} has unknown scope {definition['scope']!r}")
    roles = definition["roles"]
    if not roles or not set(roles) <= {ROLE_CANDIDATE, ROLE_ORACLE}:
        raise InvariantContractError(f"{definition['id']} has invalid roles {roles!r}")
    for condition in definition["applies_when"]:
        if (not isinstance(condition, dict) or condition.get("op") not in CONDITION_OPS
                or not isinstance(condition.get("path"), str)):
            raise InvariantContractError(f"{definition['id']} has malformed condition {condition!r}")
        if condition["op"] == "equals" and "value" not in condition:
            raise InvariantContractError(f"{definition['id']} equals-condition lacks a value")
    comparison = definition["comparison"]
    if not isinstance(comparison, dict) or "policy" not in comparison:
        raise InvariantContractError(f"{definition['id']} comparison lacks a policy")
    sources = document["threshold_sources"]
    for reference in _threshold_references(comparison):
        if reference.get("source") not in sources:
            raise InvariantContractError(
                f"{definition['id']} references unknown threshold source {reference.get('source')!r}")


def _threshold_references(comparison):
    references = []
    if isinstance(comparison.get("threshold"), dict):
        references.append(comparison["threshold"])
    for reference in (comparison.get("thresholds") or {}).values():
        references.append(reference)
    return references


def _resolve_thresholds(document, repo_root):
    """Read every referenced threshold from its versioned source file."""
    loaded = {}
    for name, source in document["threshold_sources"].items():
        path = repo_root / source["path"]
        try:
            raw = path.read_bytes()
            body = json.loads(raw.decode("utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise InvariantContractError(f"threshold source {name} unreadable: {exc}") from None
        if body.get("version") != source["version"]:
            raise InvariantContractError(
                f"threshold source {name} version {body.get('version')!r} differs from "
                f"contract-declared {source['version']!r}")
        loaded[name] = (body, _canonical_digest(body), source)

    def resolve(reference):
        body, digest, source = loaded[reference["source"]]
        base = {"source": reference["source"], "path": source["path"],
                "version": source["version"], "source_sha256": digest}
        if "criterion" in reference:
            for criterion in body.get("criteria", []):
                if criterion.get("id") == reference["criterion"]:
                    return dict(base, id=criterion["id"], value=criterion["value"],
                                operator=criterion["operator"], unit=criterion.get("unit"))
            raise InvariantContractError(
                f"threshold criterion {reference['criterion']} missing from {source['path']}")
        if "key" in reference:
            entry = (body.get("thresholds") or {}).get(reference["key"])
            if not isinstance(entry, dict) or "value" not in entry:
                raise InvariantContractError(
                    f"threshold key {reference['key']} missing from {source['path']}")
            return dict(base, id=reference["key"], value=entry["value"], unit=entry.get("unit"))
        raise InvariantContractError(f"threshold reference lacks criterion/key: {reference!r}")

    resolved = {}
    for definition in document["invariants"]:
        comparison = definition["comparison"]
        if isinstance(comparison.get("threshold"), dict):
            resolved[definition["id"]] = resolve(comparison["threshold"])
            required = REQUIRED_CRITERION_OPERATORS.get(definition["id"])
            if required and resolved[definition["id"]].get("operator") != required:
                raise InvariantContractError(
                    f"{definition['id']} implements operator {required}, threshold source "
                    f"declares {resolved[definition['id']].get('operator')!r}")
        if comparison.get("thresholds"):
            resolved[definition["id"]] = {
                component: resolve(reference)
                for component, reference in comparison["thresholds"].items()}
    return resolved


def contract_identity(contract):
    """Return the identity fields a report must carry for its contract."""
    return {"contract_id": contract["contract_id"],
            "contract_version": contract["contract_version"],
            "contract_path": _repo_relative(contract["_path"]),
            "contract_sha256": contract["_sha256"]}


def _repo_relative(path):
    try:
        return Path(path).resolve().relative_to(REPO_ROOT).as_posix()
    except ValueError:
        return str(path)


# ---------------------------------------------------------------------------
# Evidence builders
# ---------------------------------------------------------------------------

def particle_evidence(role, realization_id, *, case_id, particles, particle_count=None,
                      active_particles=None, declared_initial_mass_kg=None,
                      reservoirs=None, seed_index=None, artifacts=None):
    """Normalize one particle end-state realization (corpus seed file).

    ``particles`` maps ``lon_deg``/``lat_deg``/``z_m``/``mass_kg`` to lists.
    ``reservoirs`` holds only the ledgers the artifact actually carries
    (absent keys are missing evidence, never zero).
    """
    return {"role": role, "realization_id": realization_id, "kind": KIND_PARTICLES,
            "case_id": case_id, "seed_index": seed_index, "particles": particles,
            "particle_count": particle_count, "active_particles": active_particles,
            "declared_initial_mass_kg": declared_initial_mass_kg,
            "reservoirs": dict(reservoirs or {}), "artifacts": list(artifacts or [])}


def gridded_evidence(role, realization_id, *, case_id, fields, grid, chronology,
                     particle_positions=None, artifacts=None):
    """Normalize one gridded realization (FLEXPART header + grid_conc files).

    ``fields`` is a list of ``{"field_id", "kind", "unit", "values",
    "expected_cells"}`` with kind ``concentration``, ``dry_deposition`` or
    ``wet_deposition``. ``grid`` uses report keys ``nx/ny/nz/xlon0/ylat0/
    dx/dy/heights_m``. ``chronology`` holds ``start_epoch_s``,
    ``interval_s``, ``averaging_s``, ``sampling_s`` and
    ``timestamps_epoch_s``.
    """
    return {"role": role, "realization_id": realization_id, "kind": KIND_GRIDDED,
            "case_id": case_id, "seed_index": None, "fields": list(fields),
            "grid": grid, "chronology": chronology,
            "particle_positions": particle_positions,
            "artifacts": list(artifacts or [])}


def _particle_fields(evidence):
    fields = [{"field_id": "particles/mass_kg", "kind": "particle_mass", "unit": "kg",
               "values": (evidence.get("particles") or {}).get("mass_kg"),
               "expected_cells": evidence.get("active_particles")}]
    for name, value in sorted(evidence["reservoirs"].items()):
        fields.append({"field_id": f"reservoir/{name}", "kind": "reservoir",
                       "unit": "kg", "values": [value], "expected_cells": 1})
    if evidence.get("declared_initial_mass_kg") is not None:
        fields.append({"field_id": "declared/initial_mass_kg", "kind": "declared_mass",
                       "unit": "kg", "values": [evidence["declared_initial_mass_kg"]],
                       "expected_cells": 1})
    return fields


def _evidence_fields(evidence):
    if evidence["kind"] == KIND_PARTICLES:
        return _particle_fields(evidence)
    return evidence["fields"]


# ---------------------------------------------------------------------------
# Record helpers
# ---------------------------------------------------------------------------

def json_number(value):
    """Return a JSON-safe representation of a possibly non-finite number."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return repr(value) if value is not None else None
    if math.isnan(value):
        return "NaN"
    if math.isinf(value):
        return "Infinity" if value > 0 else "-Infinity"
    return value


def _is_number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool)


class _Context:
    """Per-evaluation state shared by the checks of one realization."""

    def __init__(self, contract, case_def, provenance):
        self.contract = contract
        self.case_def = case_def
        self.provenance = provenance or {"mode": "none"}
        self.bad_fields = set()
        self.records = {}


def _record(ctx, invariant_id, verdict, *, error_id=None, detail="",
            applicability="APPLICABLE", applicability_reason="", **extra):
    definition = ctx.contract["_by_id"][invariant_id]
    if verdict not in VERDICTS:
        raise InvariantContractError(f"unknown verdict {verdict!r}")
    if verdict in (PASS, NOT_APPLICABLE):
        if error_id is not None:
            raise InvariantContractError(f"{verdict} records carry no error id")
    else:
        expected = ctx.contract["error_ids"].get(error_id)
        if expected != verdict:
            raise InvariantContractError(
                f"error id {error_id!r} is declared {expected!r}, emitted as {verdict}")
    record = {"invariant_id": invariant_id, "definition_version": definition["version"],
              "family": definition["family"], "verdict": verdict, "error_id": error_id,
              "applicability": {"status": applicability, "reason": applicability_reason},
              "detail": detail}
    record.update(extra)
    return record


def _lookup(document, dotted):
    """Return (found, value); traversal through an explicit null is found=None-valued."""
    node = document
    for part in dotted.split("."):
        if node is None:
            return True, None
        if not isinstance(node, dict) or part not in node:
            return False, None
        node = node[part]
    return True, node


def _same_value(actual, expected):
    if expected is None:
        return actual is None
    if isinstance(expected, bool) or isinstance(actual, bool):
        return isinstance(actual, bool) and isinstance(expected, bool) and actual == expected
    return actual == expected


def _applicability(ctx, invariant_id):
    """Return (verdict_or_None, error_id, reason); None means applicable."""
    conditions = ctx.contract["_by_id"][invariant_id]["applies_when"]
    if not conditions:
        return None, None, "unconditional"
    if ctx.case_def is None:
        return BLOCKED, "CASE_MANIFEST_UNAVAILABLE", "no validated #51 case manifest"
    for condition in conditions:
        found, value = _lookup(ctx.case_def, condition["path"])
        if not found:
            return BLOCKED, "CASE_FIELD_UNAVAILABLE", f"case lacks {condition['path']}"
        if condition["op"] == "present" and value is None:
            return NOT_APPLICABLE, None, f"case declares {condition['path']} null"
        if condition["op"] == "equals" and not _same_value(value, condition["value"]):
            return (NOT_APPLICABLE, None,
                    f"case declares {condition['path']}={value!r} "
                    f"(applies when {condition['value']!r})")
    return None, None, "all case conditions hold"


def _gated(ctx, invariant_id):
    """Return a terminal record when applicability is not established."""
    verdict, error_id, reason = _applicability(ctx, invariant_id)
    if verdict is None:
        return None, reason
    status = "NOT_APPLICABLE" if verdict == NOT_APPLICABLE else "UNDETERMINED"
    return _record(ctx, invariant_id, verdict, error_id=error_id, detail=reason,
                   applicability=status, applicability_reason=reason), reason


def _case_value(ctx, dotted):
    found, value = _lookup(ctx.case_def or {}, dotted)
    return value if found else None


def _flexpart_stamp_epoch(stamp):
    return int(datetime.strptime(str(stamp), "%Y%m%d%H%M%S")
               .replace(tzinfo=timezone.utc).timestamp())


# ---------------------------------------------------------------------------
# Family 1/2: finite and non-negative fields
# ---------------------------------------------------------------------------

def check_field_finite(ctx, evidence):
    fields = _evidence_fields(evidence)
    if not fields:
        return _record(ctx, "INV-FIELD-FINITE", BLOCKED, error_id="MASS_LEDGER_UNAVAILABLE",
                       detail="realization carries no mass or concentration field")
    offenders, malformed, cells = [], [], 0
    for field in fields:
        values = field.get("values")
        expected = field.get("expected_cells")
        if not isinstance(values, list):
            malformed.append({"field": field["field_id"], "reason": "values are not an array"})
            ctx.bad_fields.add(field["field_id"])
            continue
        if expected is not None and len(values) != expected:
            malformed.append({"field": field["field_id"],
                              "reason": f"{len(values)} cells, declared {expected}"})
            ctx.bad_fields.add(field["field_id"])
        for index, value in enumerate(values):
            cells += 1
            if not _is_number(value):
                malformed.append({"field": field["field_id"], "cell": index,
                                  "reason": f"non-numeric value {value!r}"})
                ctx.bad_fields.add(field["field_id"])
            elif not math.isfinite(value):
                offenders.append({"field": field["field_id"], "cell": index,
                                  "unit": field.get("unit"), "value": json_number(value)})
                ctx.bad_fields.add(field["field_id"])
    extra = {"fields_evaluated": [f["field_id"] for f in fields], "cells_evaluated": cells,
             "offender_count": len(offenders) + len(malformed),
             "offenders": (malformed + offenders)[:MAX_LISTED_OFFENDERS]}
    if malformed:
        return _record(ctx, "INV-FIELD-FINITE", INTEGRITY_ERROR, error_id="MALFORMED_FIELD",
                       detail=f"{len(malformed)} malformed field entries", **extra)
    if offenders:
        return _record(ctx, "INV-FIELD-FINITE", INTEGRITY_ERROR, error_id="NONFINITE_VALUE",
                       detail=f"{len(offenders)} non-finite cells", **extra)
    return _record(ctx, "INV-FIELD-FINITE", PASS,
                   detail=f"{cells} cells finite across {len(fields)} fields", **extra)


def check_field_nonnegative(ctx, evidence):
    fields = _evidence_fields(evidence)
    threshold = ctx.contract["_resolved"].get("INV-FIELD-NONNEGATIVE")
    if not fields:
        return _record(ctx, "INV-FIELD-NONNEGATIVE", BLOCKED, error_id="MASS_LEDGER_UNAVAILABLE",
                       detail="realization carries no mass or concentration field",
                       threshold=threshold)
    offenders = []
    for field in fields:
        values = field.get("values")
        if not isinstance(values, list):
            continue
        for index, value in enumerate(values):
            if _is_number(value) and math.isfinite(value) and value < 0:
                offenders.append({"field": field["field_id"], "cell": index,
                                  "unit": field.get("unit"), "value": value})
    extra = {"offender_count": len(offenders), "offenders": offenders[:MAX_LISTED_OFFENDERS],
             "threshold": threshold}
    if offenders:
        for item in offenders:
            ctx.bad_fields.add(item["field"])
        return _record(ctx, "INV-FIELD-NONNEGATIVE", INTEGRITY_ERROR, error_id="NEGATIVE_VALUE",
                       detail=f"{len(offenders)} negative cells (v1 MASS_POSITIVITY: integrity error)",
                       **extra)
    if ctx.bad_fields:
        return _record(ctx, "INV-FIELD-NONNEGATIVE", INTEGRITY_ERROR,
                       error_id="UPSTREAM_FIELD_INTEGRITY",
                       detail=f"fields {sorted(ctx.bad_fields)} failed INV-FIELD-FINITE", **extra)
    return _record(ctx, "INV-FIELD-NONNEGATIVE", PASS, detail="no negative value", **extra)


# ---------------------------------------------------------------------------
# Family 3: mass and reservoir closure
# ---------------------------------------------------------------------------

def check_mass_closure(ctx, evidence):
    invariant = "INV-MASS-CLOSURE"
    gated, reason = _gated(ctx, invariant)
    if gated:
        return gated
    threshold = ctx.contract["_resolved"][invariant]
    if ctx.bad_fields:
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="UPSTREAM_FIELD_INTEGRITY",
                       detail=f"fields {sorted(ctx.bad_fields)} failed INV-FIELD-*",
                       applicability_reason=reason, threshold=threshold)
    if evidence["kind"] != KIND_PARTICLES:
        return _record(ctx, invariant, BLOCKED, error_id="MASS_LEDGER_UNAVAILABLE",
                       detail="FLEXPART 11.1 binary output carries no kilogram mass ledger; "
                              "mass-proportional grid sums need a calibration that is not a ledger",
                       applicability_reason=reason, threshold=threshold)
    released = _case_value(ctx, "release.inventory.quantity_kg")
    if not _is_number(released) or released <= 0:
        return _record(ctx, invariant, BLOCKED, error_id="CASE_FIELD_UNAVAILABLE",
                       detail="case lacks a positive release.inventory.quantity_kg",
                       applicability_reason=reason, threshold=threshold)
    declared = evidence.get("declared_initial_mass_kg")
    if declared is not None and abs(declared - released) / released >= float(threshold["value"]):
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="INITIAL_MASS_CONTRADICTS_CASE",
                       detail=f"artifact initial mass {declared} kg contradicts case inventory "
                              f"{released} kg", applicability_reason=reason, threshold=threshold)
    masses = evidence["particles"]["mass_kg"]
    count, active = evidence.get("particle_count"), evidence.get("active_particles")
    if count is None or active is None:
        return _record(ctx, invariant, BLOCKED, error_id="PARTICLE_LEDGER_UNAVAILABLE",
                       detail="particle_count/active_particles not recorded; terminated mass "
                              "cannot be excluded", applicability_reason=reason,
                       threshold=threshold)
    if len(masses) != active or active > count:
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="PARTICLE_COUNT_INCONSISTENT",
                       detail=f"{len(masses)} records, active {active}, released {count}",
                       applicability_reason=reason, threshold=threshold)
    if active < count:
        return _record(ctx, invariant, BLOCKED, error_id="MISSING_TERMINATION_RESERVOIR",
                       detail=f"{count - active} particles were deactivated without a "
                              "termination ledger; termination is not deposition",
                       applicability_reason=reason, threshold=threshold)
    terms, excluded = {}, {}
    for process, reservoir in PROCESS_RESERVOIRS:
        switch = _case_value(ctx, f"physics_switches.{process}")
        if not isinstance(switch, bool):
            return _record(ctx, invariant, BLOCKED, error_id="CASE_FIELD_UNAVAILABLE",
                           detail=f"case lacks boolean physics_switches.{process}",
                           applicability_reason=reason, threshold=threshold)
        present = reservoir in evidence["reservoirs"]
        if switch and not present:
            return _record(ctx, invariant, BLOCKED, error_id="MISSING_RESERVOIR",
                           detail=f"{process} is enabled but the artifact has no {reservoir} "
                                  "ledger; it is never treated as zero",
                           applicability_reason=reason, threshold=threshold,
                           missing_reservoir=reservoir)
        if switch:
            terms[reservoir] = float(evidence["reservoirs"][reservoir])
        elif present:
            excluded[reservoir] = float(evidence["reservoirs"][reservoir])
    airborne = math.fsum(float(m) for m in masses)
    budget = metrics.mass_budget(released, airborne,
                                 dry_deposited_kg=terms.get("dry_deposited_kg", 0.0),
                                 wet_deposited_kg=terms.get("wet_deposited_kg", 0.0),
                                 decayed_kg=terms.get("decayed_kg", 0.0))
    identity = "released = airborne" + "".join(f" + {name}" for name in terms)
    extra = {"identity": identity, "measured": budget, "included_reservoirs": sorted(terms),
             "excluded_disabled_reservoirs": excluded, "residual_kg": budget["absolute_error_kg"],
             "residual_relative": budget["relative_error"], "unit": "kg",
             "threshold": threshold, "applicability_reason": reason}
    if abs(budget["relative_error"]) < float(threshold["value"]):
        return _record(ctx, invariant, PASS, detail=f"{identity} closes", **extra)
    error_id = "MASS_DEFICIT" if budget["absolute_error_kg"] < 0 else "MASS_EXCESS"
    return _record(ctx, invariant, FAIL, error_id=error_id,
                   detail=f"{identity} residual {budget['relative_error']:+.3e} relative", **extra)


# ---------------------------------------------------------------------------
# Family 4: disabled-physics no-effect
# ---------------------------------------------------------------------------

DISABLED_CHECKS = (
    ("INV-DISABLED-DRY-DEPOSITION-ZERO", "dry_deposited_kg", "dry_deposition"),
    ("INV-DISABLED-WET-DEPOSITION-ZERO", "wet_deposited_kg", "wet_deposition"),
    ("INV-DISABLED-DECAY-NO-LOSS", "decayed_kg", None),
)


def check_disabled_process(ctx, evidence, invariant, reservoir, grid_kind):
    gated, reason = _gated(ctx, invariant)
    if gated:
        return gated
    if evidence["kind"] == KIND_GRIDDED and grid_kind is not None:
        fields = [f for f in evidence["fields"] if f.get("kind") == grid_kind]
        if fields:
            bad = [f["field_id"] for f in fields if f["field_id"] in ctx.bad_fields]
            if bad:
                return _record(ctx, invariant, INTEGRITY_ERROR, error_id="UPSTREAM_FIELD_INTEGRITY",
                               detail=f"fields {bad} failed INV-FIELD-*",
                               applicability_reason=reason)
            offenders = [{"field": f["field_id"], "cell": i, "value": v}
                         for f in fields for i, v in enumerate(f["values"]) if v != 0.0]
            extra = {"evidence_mode": "exact_ledger", "fields_evaluated": [f["field_id"] for f in fields],
                     "offender_count": len(offenders), "offenders": offenders[:MAX_LISTED_OFFENDERS],
                     "applicability_reason": reason}
            if offenders:
                return _record(ctx, invariant, FAIL, error_id="DISABLED_PROCESS_NONZERO_RESERVOIR",
                               detail=f"{len(offenders)} non-zero {grid_kind} cells", **extra)
            return _record(ctx, invariant, PASS, detail=f"every {grid_kind} cell is exactly zero",
                           **extra)
    if evidence["kind"] == KIND_PARTICLES and reservoir in evidence["reservoirs"]:
        field_id = f"reservoir/{reservoir}"
        if field_id in ctx.bad_fields:
            return _record(ctx, invariant, INTEGRITY_ERROR, error_id="UPSTREAM_FIELD_INTEGRITY",
                           detail=f"{field_id} failed INV-FIELD-*", applicability_reason=reason)
        value = evidence["reservoirs"][reservoir]
        extra = {"evidence_mode": "exact_ledger", "measured": {reservoir: value},
                 "expected": {reservoir: 0.0}, "unit": "kg", "applicability_reason": reason}
        if value != 0.0:
            return _record(ctx, invariant, FAIL, error_id="DISABLED_PROCESS_NONZERO_RESERVOIR",
                           detail=f"disabled process reports {reservoir}={value}", **extra)
        return _record(ctx, invariant, PASS, detail=f"{reservoir} is exactly zero", **extra)
    closure = ctx.records.get("INV-MASS-CLOSURE")
    extra = {"evidence_mode": "closure_bound", "applicability_reason": reason,
             "closure_verdict": closure["verdict"] if closure else None,
             "closure_error_id": closure.get("error_id") if closure else None}
    if closure is None or closure["verdict"] in (BLOCKED, NOT_APPLICABLE):
        return _record(ctx, invariant, BLOCKED, error_id="CLOSURE_UNAVAILABLE",
                       detail=f"no {reservoir} ledger and no evaluated mass closure", **extra)
    if closure["verdict"] == INTEGRITY_ERROR:
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="UPSTREAM_FIELD_INTEGRITY",
                       detail="mass closure inputs are invalid", **extra)
    if closure["verdict"] == PASS:
        return _record(ctx, invariant, PASS,
                       detail=f"no {reservoir} ledger; closure without that term bounds removal "
                              "within MASS_BUDGET_CLOSE", **extra)
    if closure.get("error_id") == "MASS_DEFICIT":
        return _record(ctx, invariant, FAIL, error_id="UNEXPLAINED_MASS_LOSS",
                       detail="closure reports an unexplained mass deficit", **extra)
    return _record(ctx, invariant, BLOCKED, error_id="CLOSURE_UNAVAILABLE",
                   detail="closure reports MASS_EXCESS; the deficit bound is unusable", **extra)


# ---------------------------------------------------------------------------
# Family 5: deterministic analytic and process cases
# ---------------------------------------------------------------------------

def check_analytic_uniform_advection(ctx, evidence):
    invariant = "INV-ANALYTIC-UNIFORM-ADVECTION"
    gated, reason = _gated(ctx, invariant)
    if gated:
        return gated
    comparison = ctx.contract["_by_id"][invariant]["comparison"]
    thresholds = ctx.contract["_resolved"][invariant]
    case_id = ctx.case_def.get("case_id")
    if case_id not in comparison["scope_case_ids"]:
        return _record(ctx, invariant, BLOCKED, error_id="NO_VERSIONED_THRESHOLD",
                       detail=f"{case_id} qualifies but the analytic bounds are justified only "
                              f"for {comparison['scope_case_ids']}",
                       applicability_reason=reason, threshold=thresholds)
    if evidence["kind"] == KIND_PARTICLES:
        positions = evidence["particles"]
    else:
        positions = evidence.get("particle_positions")
    if not positions or not positions.get("lon_deg"):
        return _record(ctx, invariant, BLOCKED, error_id="PARTICLE_POSITIONS_UNAVAILABLE",
                       detail="no particle end positions (FLEXPART 11.1 partposit is NetCDF-only)",
                       applicability_reason=reason, threshold=thresholds)
    needed = ("release.geometry.lon_deg", "release.geometry.lat_deg", "release.geometry.z_m",
              "wind.u_m_s", "wind.v_m_s", "wind.w_m_s", "integration.start",
              "integration.total_s", "release.timing.at")
    values = {key: _case_value(ctx, key) for key in needed}
    missing = [key for key, value in values.items() if value is None]
    if missing:
        return _record(ctx, invariant, BLOCKED, error_id="CASE_FIELD_UNAVAILABLE",
                       detail=f"case lacks {missing}", applicability_reason=reason)
    lon0, lat0, z0 = (float(values["release.geometry.lon_deg"]),
                      float(values["release.geometry.lat_deg"]),
                      float(values["release.geometry.z_m"]))
    end = _flexpart_stamp_epoch(values["integration.start"]) + int(values["integration.total_s"])
    elapsed = end - _flexpart_stamp_epoch(values["release.timing.at"])
    expected = {"east_m": float(values["wind.u_m_s"]) * elapsed,
                "north_m": float(values["wind.v_m_s"]) * elapsed,
                "vertical_m": float(values["wind.w_m_s"]) * elapsed}
    m_per_deg_lat = metrics.EARTH_RADIUS_M * math.pi / 180.0
    m_per_deg_lon = m_per_deg_lat * math.cos(math.radians(lat0))
    lons, lats, zs = positions["lon_deg"], positions["lat_deg"], positions["z_m"]
    if not (len(lons) == len(lats) == len(zs)):
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="MALFORMED_FIELD",
                       detail="position arrays differ in length", applicability_reason=reason)
    worst = {component: {"residual_m": 0.0, "particle": None, "actual_m": None}
             for component in expected}
    for index, (lon, lat, z) in enumerate(zip(lons, lats, zs)):
        if not all(_is_number(v) for v in (lon, lat, z)):
            return _record(ctx, invariant, INTEGRITY_ERROR, error_id="MALFORMED_FIELD",
                           detail=f"particle {index} position is not numeric",
                           offenders=[{"particle": index, "value": [json_number(lon),
                                       json_number(lat), json_number(z)]}],
                           applicability_reason=reason)
        if not all(math.isfinite(v) for v in (lon, lat, z)):
            return _record(ctx, invariant, INTEGRITY_ERROR, error_id="NONFINITE_VALUE",
                           detail=f"particle {index} position is not finite",
                           offenders=[{"particle": index, "value": [json_number(lon),
                                       json_number(lat), json_number(z)]}],
                           applicability_reason=reason)
        actual = {"east_m": (lon - lon0) * m_per_deg_lon,
                  "north_m": (lat - lat0) * m_per_deg_lat,
                  "vertical_m": z - z0}
        for component, value in actual.items():
            residual = abs(value - expected[component])
            if worst[component]["particle"] is None or residual > worst[component]["residual_m"]:
                worst[component] = {"residual_m": residual, "particle": index, "actual_m": value}
    violations = [{"component": component, "particle": data["particle"],
                   "actual_m": data["actual_m"], "expected_m": expected[component],
                   "residual_m": data["residual_m"], "bound_m": thresholds[component]["value"]}
                  for component, data in worst.items()
                  if data["residual_m"] > float(thresholds[component]["value"])]
    extra = {"expected": expected, "worst_particle": worst, "unit": "m",
             "elapsed_s": elapsed, "particles_evaluated": len(lons),
             "threshold": thresholds, "operator": comparison["operator"],
             "offenders": violations, "offender_count": len(violations),
             "applicability_reason": reason}
    if violations:
        return _record(ctx, invariant, FAIL, error_id="ANALYTIC_DISPLACEMENT_EXCEEDED",
                       detail="analytic displacement exceeded for "
                              f"{[v['component'] for v in violations]}", **extra)
    return _record(ctx, invariant, PASS,
                   detail=f"all {len(lons)} particles within the analytic bounds", **extra)


def check_analytic_decay(ctx, evidence):
    invariant = "INV-ANALYTIC-DECAY"
    gated, reason = _gated(ctx, invariant)
    if gated:
        return gated
    return _record(ctx, invariant, BLOCKED, error_id="DECAY_SEMANTICS_UNAVAILABLE",
                   detail="decay is enabled but no species contract binds a half-life, the "
                          "artifact carries no decay ledger and no decay threshold is versioned "
                          "(follow-up: DECAY-011)", applicability_reason=reason)


def check_deterministic_repeat(contract, case_def, evidences, realization_records):
    """Ensemble-level exact-repeat invariant over candidate realizations."""
    ctx = _Context(contract, case_def, None)
    invariant = "INV-DETERMINISTIC-REPEAT"
    gated, reason = _gated(ctx, invariant)
    candidates = [(e, r) for e, r in zip(evidences, realization_records)
                  if e["role"] == ROLE_CANDIDATE and e["kind"] == KIND_PARTICLES]
    if gated:
        gated["scope"] = "ensemble"
        return gated
    ids = [e["realization_id"] for e, _ in candidates]
    if len(candidates) < 2:
        record = _record(ctx, invariant, BLOCKED, error_id="INSUFFICIENT_REPEATS",
                         detail=f"{len(candidates)} realization(s); a repeat needs two",
                         applicability_reason=reason, realizations=ids)
        record["scope"] = "ensemble"
        return record
    invalid = [r["realization_id"] for _, r in candidates
               if any(rec["invariant_id"].startswith("INV-FIELD-") and rec["verdict"] != PASS
                      for rec in r["invariants"])]
    if invalid:
        record = _record(ctx, invariant, INTEGRITY_ERROR, error_id="UPSTREAM_FIELD_INTEGRITY",
                         detail=f"realizations {invalid} failed INV-FIELD-*",
                         applicability_reason=reason, realizations=ids)
        record["scope"] = "ensemble"
        return record
    reference = candidates[0][0]
    difference = None
    for evidence, _ in candidates[1:]:
        difference = _first_difference(reference, evidence)
        if difference:
            difference["realization"] = evidence["realization_id"]
            break
    extra = {"realizations": ids, "reference_realization": reference["realization_id"],
             "applicability_reason": reason}
    if difference:
        record = _record(ctx, invariant, FAIL, error_id="REPEAT_NOT_IDENTICAL",
                         detail=f"{difference['realization']} differs from "
                                f"{reference['realization_id']}", offenders=[difference],
                         offender_count=1, **extra)
    else:
        record = _record(ctx, invariant, PASS,
                         detail=f"{len(candidates)} realizations are bit-identical", **extra)
    record["scope"] = "ensemble"
    return record


def _first_difference(left, right):
    for component in ("lon_deg", "lat_deg", "z_m", "mass_kg"):
        a = left["particles"].get(component) or []
        b = right["particles"].get(component) or []
        if len(a) != len(b):
            return {"component": component, "reason": f"{len(a)} vs {len(b)} particles"}
        for index, (x, y) in enumerate(zip(a, b)):
            if x != y:
                return {"component": component, "particle": index,
                        "reference": json_number(x), "value": json_number(y)}
    if left["reservoirs"] != right["reservoirs"]:
        return {"component": "reservoirs", "reference": left["reservoirs"],
                "value": right["reservoirs"]}
    return None


# ---------------------------------------------------------------------------
# Family 6: grid, time-window and output consistency
# ---------------------------------------------------------------------------

def check_case_identity(ctx, evidence):
    invariant = "INV-CASE-IDENTITY"
    if ctx.case_def is None:
        return _record(ctx, invariant, BLOCKED, error_id="CASE_MANIFEST_UNAVAILABLE",
                       detail="no #51 case manifest to bind against")
    artifact_case = evidence.get("case_id")
    if artifact_case is None:
        return _record(ctx, invariant, BLOCKED, error_id="PROVENANCE_MISSING",
                       detail="neither the artifact nor a run manifest binds a case id")
    if artifact_case != ctx.case_def.get("case_id"):
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="CASE_ID_MISMATCH",
                       detail=f"artifact belongs to {artifact_case}, case manifest is "
                              f"{ctx.case_def.get('case_id')}")
    return _record(ctx, invariant, PASS, detail=f"artifact bound to {artifact_case}")


def check_output_units(ctx, evidence):
    invariant = "INV-OUTPUT-UNITS"
    if ctx.case_def is None:
        return _record(ctx, invariant, BLOCKED, error_id="CASE_MANIFEST_UNAVAILABLE",
                       detail="no #51 case manifest declares units")
    expected = ctx.contract["_by_id"][invariant]["comparison"]["expected"]
    mismatches, missing = [], []
    for path, unit in expected.items():
        found, value = _lookup(ctx.case_def, path)
        if not found or value is None:
            missing.append(path)
        elif value != unit:
            mismatches.append({"path": path, "declared": value, "consumed": unit})
    if mismatches:
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="UNIT_MISMATCH",
                       detail=f"declared units differ from consumed units: {mismatches}",
                       offenders=mismatches, offender_count=len(mismatches))
    if missing:
        return _record(ctx, invariant, BLOCKED, error_id="CASE_FIELD_UNAVAILABLE",
                       detail=f"case lacks unit declarations {missing}")
    return _record(ctx, invariant, PASS, detail="declared units match consumed units",
                   expected=expected)


def check_particle_count(ctx, evidence):
    invariant = "INV-PARTICLE-COUNT"
    if evidence["kind"] != KIND_PARTICLES:
        return _record(ctx, invariant, BLOCKED, error_id="PARTICLE_LEDGER_UNAVAILABLE",
                       detail="gridded realization carries no particle ledger")
    particles = evidence["particles"]
    lengths = {k: len(particles.get(k) or []) for k in ("lon_deg", "lat_deg", "z_m", "mass_kg")}
    count, active = evidence.get("particle_count"), evidence.get("active_particles")
    measured = {"records": lengths, "particle_count": count, "active_particles": active}
    problems = []
    if len(set(lengths.values())) != 1:
        problems.append(f"particle component arrays differ in length {lengths}")
    if active is not None and lengths["mass_kg"] != active:
        problems.append(f"{lengths['mass_kg']} records but active_particles={active}")
    if active is not None and count is not None and active > count:
        problems.append(f"active_particles={active} exceeds particle_count={count}")
    declared = _case_value(ctx, "release.particle_count")
    if count is not None and declared is not None and count != declared:
        problems.append(f"particle_count={count} differs from case release.particle_count={declared}")
    if problems:
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="PARTICLE_COUNT_INCONSISTENT",
                       detail="; ".join(problems), measured=measured)
    if count is None or active is None:
        return _record(ctx, invariant, BLOCKED, error_id="PARTICLE_LEDGER_UNAVAILABLE",
                       detail="particle_count/active_particles not recorded", measured=measured)
    if ctx.case_def is None:
        return _record(ctx, invariant, BLOCKED, error_id="CASE_MANIFEST_UNAVAILABLE",
                       detail="internal counts agree; no case release.particle_count to bind",
                       measured=measured)
    return _record(ctx, invariant, PASS, detail="particle ledgers agree", measured=measured,
                   expected={"particle_count": declared})


def check_output_grid(ctx, evidence):
    invariant = "INV-OUTPUT-GRID"
    gated, reason = _gated(ctx, invariant)
    if gated:
        return gated
    if evidence["kind"] != KIND_GRIDDED or evidence.get("grid") is None:
        return _record(ctx, invariant, BLOCKED, error_id="GRIDDED_OUTPUT_UNAVAILABLE",
                       detail="artifact carries no gridded output geometry",
                       applicability_reason=reason)
    try:
        case_grid = ctx.case_def["output_grid"]
        declared = {"outlon0": case_grid["xlon0_deg"], "outlat0": case_grid["ylat0_deg"],
                    "nx": case_grid["nx"], "ny": case_grid["ny"], "nz": case_grid["nz"],
                    "dx": case_grid["dx_deg"], "dy": case_grid["dy_deg"],
                    "outheights": case_grid["heights_m"]}
    except (KeyError, TypeError) as exc:
        return _record(ctx, invariant, BLOCKED, error_id="CASE_FIELD_UNAVAILABLE",
                       detail=f"case output_grid lacks {exc}", applicability_reason=reason)
    try:
        match, checks = report_lib.check_grid_alignment(declared, evidence["grid"])
    except (KeyError, TypeError, ValueError) as exc:
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="OUTPUT_GRID_MISMATCH",
                       detail=f"artifact grid is malformed: {exc}", applicability_reason=reason)
    if match:
        return _record(ctx, invariant, PASS, detail="output grid equals case output_grid",
                       checks=checks, applicability_reason=reason)
    failed = {c["check"] for c in checks if not c["match"]}
    shape_keys = {"grid.nx", "grid.ny", "grid.nz", "grid.nz_heights_length"}
    error_id = "GRID_SHAPE_MISMATCH" if failed & shape_keys else "OUTPUT_GRID_MISMATCH"
    return _record(ctx, invariant, INTEGRITY_ERROR, error_id=error_id,
                   detail=f"output grid differs from case: {sorted(failed)}", checks=checks,
                   applicability_reason=reason)


def check_output_chronology(ctx, evidence):
    invariant = "INV-OUTPUT-CHRONOLOGY"
    gated, reason = _gated(ctx, invariant)
    if gated:
        return gated
    if evidence["kind"] != KIND_GRIDDED or evidence.get("chronology") is None:
        return _record(ctx, invariant, BLOCKED, error_id="GRIDDED_OUTPUT_UNAVAILABLE",
                       detail="artifact carries no output windows",
                       applicability_reason=reason)
    direction = ctx.case_def.get("simulation_direction")
    if direction != "forward":
        return _record(ctx, invariant, BLOCKED, error_id="DIRECTION_UNSUPPORTED",
                       detail=f"chronology for {direction!r} runs is not supported",
                       applicability_reason=reason)
    try:
        integration, output = ctx.case_def["integration"], ctx.case_def["output"]
        start = _flexpart_stamp_epoch(integration["start"])
        end = start + int(integration["total_s"])
        interval = int(output["interval_s"])
        expected = {"start_epoch_s": start, "interval_s": interval,
                    "averaging_s": int(output["averaging_window_s"]),
                    "sampling_s": int(output["sampling_interval_s"])}
    except (KeyError, TypeError, ValueError) as exc:
        return _record(ctx, invariant, BLOCKED, error_id="CASE_FIELD_UNAVAILABLE",
                       detail=f"case chronology incomplete: {exc}", applicability_reason=reason)
    expected_times = list(range(start + interval, end + 1, interval)) if interval > 0 else []
    expected["timestamps_epoch_s"] = expected_times
    actual = evidence["chronology"]
    mismatches = [{"field": key, "expected": value, "actual": actual.get(key)}
                  for key, value in expected.items() if actual.get(key) != value]
    extra = {"expected": expected, "measured": actual, "unit": "s",
             "applicability_reason": reason}
    if mismatches:
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="OUTPUT_WINDOW_MISMATCH",
                       detail=f"output chronology differs from case: "
                              f"{[m['field'] for m in mismatches]}",
                       offenders=mismatches, offender_count=len(mismatches), **extra)
    return _record(ctx, invariant, PASS, detail=f"{len(expected_times)} output windows match",
                   **extra)


# ---------------------------------------------------------------------------
# Family 7: evidence requirements (#53 binding)
# ---------------------------------------------------------------------------

def check_provenance_binding(ctx, evidence):
    invariant = "INV-PROVENANCE-BINDING"
    provenance = ctx.provenance
    mode = provenance.get("mode", "none")
    if mode == "none":
        return _record(ctx, invariant, BLOCKED, error_id="PROVENANCE_MISSING",
                       detail="no #53 run manifest supplied")
    if mode != "v1":
        return _record(ctx, invariant, BLOCKED, error_id="EXECUTION_IDENTITY_UNAVAILABLE",
                       detail="legacy run manifest records no execution identities")
    labels = [artifact["label"] for artifact in evidence["artifacts"]]
    if not labels:
        return _record(ctx, invariant, BLOCKED, error_id="ARTIFACT_NOT_COVERED",
                       detail="realization lists no consumed artifacts")
    uncovered = [label for label in labels if label not in provenance.get("verified", ())]
    if uncovered:
        return _record(ctx, invariant, BLOCKED, error_id="ARTIFACT_NOT_COVERED",
                       detail=f"artifacts {uncovered} are not hash-verified by the run manifest")
    execution_ids = set()
    for label in labels:
        bound = provenance.get("bindings", {}).get(label, [])
        if len(bound) != 1:
            return _record(ctx, invariant, INTEGRITY_ERROR, error_id="EXECUTION_IDENTITY_AMBIGUOUS",
                           detail=f"artifact {label} binds {len(bound)} executions")
        execution_ids.add(bound[0]["execution_id"])
    if len(execution_ids) != 1:
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="EXECUTION_IDENTITY_AMBIGUOUS",
                       detail=f"realization artifacts bind {len(execution_ids)} executions")
    execution = provenance["bindings"][labels[0]][0]
    problems = []
    if execution.get("role") != evidence["role"]:
        problems.append(f"execution role {execution.get('role')} != {evidence['role']}")
    if ctx.case_def is not None and execution.get("case_id") != ctx.case_def.get("case_id"):
        problems.append(f"execution case {execution.get('case_id')} != {ctx.case_def.get('case_id')}")
    case_sha = provenance.get("case_manifest_sha256")
    if case_sha and execution.get("case_manifest_sha256") != case_sha:
        problems.append("execution case_manifest_sha256 differs from the evaluated case file")
    realized = (execution.get("realization") or {}).get("seed_index")
    if evidence.get("seed_index") is not None and realized is not None \
            and realized != evidence["seed_index"]:
        problems.append(f"execution seed_index {realized} != artifact {evidence['seed_index']}")
    identity = {"execution_id": execution.get("execution_id"), "role": execution.get("role"),
                "case_id": execution.get("case_id"),
                "case_manifest_sha256": execution.get("case_manifest_sha256"),
                "realization": execution.get("realization"),
                "candidate_revision": execution.get("candidate_revision"),
                "oracle_kind": execution.get("oracle_kind"),
                "oracle_revision": execution.get("oracle_revision")}
    if problems:
        return _record(ctx, invariant, INTEGRITY_ERROR, error_id="EXECUTION_BINDING_MISMATCH",
                       detail="; ".join(problems), execution_identity=identity)
    return _record(ctx, invariant, PASS, detail=f"bound to execution {identity['execution_id'][:16]}",
                   execution_identity=identity)


# Implemented checks and their definition versions; must equal the contract.
IMPLEMENTED = {
    "INV-FIELD-FINITE": 1,
    "INV-FIELD-NONNEGATIVE": 1,
    "INV-MASS-CLOSURE": 1,
    "INV-DISABLED-DRY-DEPOSITION-ZERO": 1,
    "INV-DISABLED-WET-DEPOSITION-ZERO": 1,
    "INV-DISABLED-DECAY-NO-LOSS": 1,
    "INV-ANALYTIC-UNIFORM-ADVECTION": 1,
    "INV-ANALYTIC-DECAY": 1,
    "INV-DETERMINISTIC-REPEAT": 1,
    "INV-CASE-IDENTITY": 1,
    "INV-OUTPUT-UNITS": 1,
    "INV-PARTICLE-COUNT": 1,
    "INV-OUTPUT-GRID": 1,
    "INV-OUTPUT-CHRONOLOGY": 1,
    "INV-PROVENANCE-BINDING": 1,
}


# ---------------------------------------------------------------------------
# Realization, ensemble and gate assembly
# ---------------------------------------------------------------------------

def evaluate_realization(contract, case_def, evidence, provenance=None):
    """Evaluate every realization-scoped invariant for one realization."""
    ctx = _Context(contract, case_def, provenance)
    steps = [
        ("INV-FIELD-FINITE", lambda: check_field_finite(ctx, evidence)),
        ("INV-FIELD-NONNEGATIVE", lambda: check_field_nonnegative(ctx, evidence)),
        ("INV-CASE-IDENTITY", lambda: check_case_identity(ctx, evidence)),
        ("INV-OUTPUT-UNITS", lambda: check_output_units(ctx, evidence)),
        ("INV-PARTICLE-COUNT", lambda: check_particle_count(ctx, evidence)),
        ("INV-MASS-CLOSURE", lambda: check_mass_closure(ctx, evidence)),
    ]
    for invariant, reservoir, grid_kind in DISABLED_CHECKS:
        steps.append((invariant, lambda i=invariant, r=reservoir, g=grid_kind:
                      check_disabled_process(ctx, evidence, i, r, g)))
    steps += [
        ("INV-ANALYTIC-UNIFORM-ADVECTION", lambda: check_analytic_uniform_advection(ctx, evidence)),
        ("INV-ANALYTIC-DECAY", lambda: check_analytic_decay(ctx, evidence)),
        ("INV-OUTPUT-GRID", lambda: check_output_grid(ctx, evidence)),
        ("INV-OUTPUT-CHRONOLOGY", lambda: check_output_chronology(ctx, evidence)),
        ("INV-PROVENANCE-BINDING", lambda: check_provenance_binding(ctx, evidence)),
    ]
    records = []
    for invariant, run in steps:
        definition = contract["_by_id"][invariant]
        if evidence["role"] not in definition["roles"]:
            continue
        record = run()
        ctx.records[invariant] = record
        records.append(record)
    execution = next((r.get("execution_identity") for r in records
                      if r["invariant_id"] == "INV-PROVENANCE-BINDING"), None)
    realization = {"realization_id": evidence["realization_id"], "role": evidence["role"],
                   "kind": evidence["kind"], "case_id": evidence.get("case_id"),
                   "seed_index": evidence.get("seed_index"),
                   "artifacts": evidence["artifacts"], "execution_identity": execution,
                   "invariants": records}
    realization.update(_summarize(records, realization_id=evidence["realization_id"]))
    return realization


def _summarize(records, realization_id):
    def listing(verdict):
        return [{"realization_id": realization_id or r.get("_realization"),
                 "invariant_id": r["invariant_id"], "error_id": r.get("error_id"),
                 "detail": r.get("detail")}
                for r in records if r["verdict"] == verdict]
    verdicts = [r["verdict"] for r in records]
    return {"verdict": aggregate_verdicts(verdicts),
            "counts": {v: verdicts.count(v) for v in VERDICTS},
            "failed": listing(FAIL), "integrity_errors": listing(INTEGRITY_ERROR),
            "missing_evidence": listing(BLOCKED)}


def aggregate_verdicts(verdicts):
    """Deterministic precedence: INTEGRITY_ERROR > FAIL > BLOCKED > PASS > NOT_APPLICABLE."""
    verdicts = list(verdicts)
    if not verdicts:
        return BLOCKED
    for verdict in verdicts:
        if verdict not in VERDICTS:
            return INTEGRITY_ERROR
    for verdict in PRECEDENCE:
        if verdict in verdicts:
            return verdict
    return BLOCKED


def _guard_duplicate_executions(contract, realizations):
    """Two realizations may never claim the same execution identity."""
    owners = {}
    for realization in realizations:
        identity = realization.get("execution_identity") or {}
        if identity.get("execution_id"):
            owners.setdefault(identity["execution_id"], []).append(realization)
    for execution_id, claimed in owners.items():
        if len(claimed) < 2:
            continue
        for realization in claimed:
            ctx = _Context(contract, None, None)
            replacement = _record(
                ctx, "INV-PROVENANCE-BINDING", INTEGRITY_ERROR,
                error_id="EXECUTION_IDENTITY_AMBIGUOUS",
                detail=f"execution {execution_id[:16]} is claimed by "
                       f"{[r['realization_id'] for r in claimed]}")
            realization["invariants"] = [
                replacement if r["invariant_id"] == "INV-PROVENANCE-BINDING" else r
                for r in realization["invariants"]]
            realization.update(_summarize(realization["invariants"],
                                          realization["realization_id"]))


def build_gate(contract, case_def, evidences, *, provenance=None, paired_scoring=None,
               case_manifest_sha256=None):
    """Evaluate all realizations plus ensemble invariants into one gate block."""
    realizations = [evaluate_realization(contract, case_def, e, provenance) for e in evidences]
    _guard_duplicate_executions(contract, realizations)
    ensemble = [check_deterministic_repeat(contract, case_def, evidences, realizations)]
    return assemble_gate(contract, case_def, realizations, ensemble,
                         paired_scoring=paired_scoring,
                         case_manifest_sha256=case_manifest_sha256)


def assemble_gate(contract, case_def, realizations, ensemble, *, paired_scoring=None,
                  case_manifest_sha256=None):
    aggregate = aggregate_realizations({"realizations": realizations, "ensemble": ensemble})
    block = dict(contract_identity(contract))
    block.update({
        "case_id": (case_def or {}).get("case_id"),
        "case_manifest_sha256": case_manifest_sha256,
        "verdict": aggregate["verdict"],
        "precedence": list(PRECEDENCE),
        "counts": aggregate["counts"],
        "failed": aggregate["failed"],
        "integrity_errors": aggregate["integrity_errors"],
        "blocked": aggregate["blocked"],
        "realization_count": len(realizations),
        "realizations": realizations,
        "ensemble": ensemble,
        "paired_scoring": paired_scoring or {"status": "NOT_REQUESTED",
                                             "reason": "no oracle artifact supplied"},
    })
    block["summary"] = render_gate_summary(block)
    return block


def aggregate_realizations(source):
    """Fail-closed aggregation for downstream ensemble consumers (#55-#59).

    ``source`` is a gate block, a list of gate blocks, or a list of
    realization records. Every record of every realization is kept: the
    verdict is the precedence maximum and all FAIL/INTEGRITY_ERROR/BLOCKED
    identifiers are listed. A missing block or record is BLOCKED; an
    unknown verdict is an INTEGRITY_ERROR. Nothing is averaged.
    """
    blocks = source if isinstance(source, list) else [source]
    realizations, ensemble, missing = [], [], 0
    for item in blocks:
        if item is None:
            missing += 1
        elif isinstance(item, dict) and "realizations" in item:
            realizations.extend(item.get("realizations") or [])
            ensemble.extend(item.get("ensemble") or [])
        elif isinstance(item, dict) and "invariants" in item:
            realizations.append(item)
        else:
            missing += 1
    records = []
    for realization in realizations:
        for record in realization.get("invariants") or []:
            records.append(dict(record, _realization=realization.get("realization_id")))
        if not realization.get("invariants"):
            missing += 1
    for record in ensemble:
        records.append(dict(record, _realization="ensemble"))
    verdicts = [r.get("verdict") for r in records] + [BLOCKED] * missing
    summary = _summarize(records, None)
    for key in ("failed", "integrity_errors", "missing_evidence"):
        for entry in summary[key]:
            entry.setdefault("realization_id", None)
    unknown = [r for r in records if r.get("verdict") not in VERDICTS]
    result = {"verdict": aggregate_verdicts(verdicts),
              "counts": {v: verdicts.count(v) for v in VERDICTS},
              "failed": summary["failed"],
              "integrity_errors": summary["integrity_errors"] + [
                  {"realization_id": r.get("_realization"), "invariant_id": r.get("invariant_id"),
                   "error_id": "EVALUATION_INTEGRITY_ERROR",
                   "detail": f"unknown verdict {r.get('verdict')!r}"} for r in unknown],
              "blocked": summary["missing_evidence"] + (
                  [{"realization_id": None, "invariant_id": None, "error_id": None,
                    "detail": f"{missing} realization(s) without invariant evidence"}]
                  if missing else []),
              "realization_count": len(realizations)}
    return result


def integrity_block(contract, error_id, message, *, case_def=None, paired_scoring=None):
    """Gate block for an evaluation stopped before per-realization records exist."""
    identity = (contract_identity(contract) if contract and "_path" in contract
                else {"contract_id": CONTRACT_ID, "contract_version": CONTRACT_VERSION,
                      "contract_path": None, "contract_sha256": None})
    entry = {"realization_id": None, "invariant_id": None, "error_id": error_id,
             "detail": message}
    block = dict(identity)
    block.update({"case_id": (case_def or {}).get("case_id"), "case_manifest_sha256": None,
                  "verdict": INTEGRITY_ERROR, "precedence": list(PRECEDENCE),
                  "counts": {v: (1 if v == INTEGRITY_ERROR else 0) for v in VERDICTS},
                  "failed": [], "integrity_errors": [entry], "blocked": [],
                  "realization_count": 0, "realizations": [], "ensemble": [],
                  "paired_scoring": paired_scoring or {"status": "NOT_REQUESTED",
                                                       "reason": "evaluation stopped"}})
    block["summary"] = render_gate_summary(block)
    return block


def render_gate_summary(block):
    lines = [f"Deterministic invariants ({block['contract_id']} v{block['contract_version']}): "
             f"{block['verdict']}"]
    for key, label in (("integrity_errors", "INTEGRITY_ERROR"), ("failed", "FAIL"),
                       ("blocked", "BLOCKED")):
        for entry in block.get(key, []):
            lines.append(f"  {label} {entry.get('realization_id')} {entry.get('invariant_id')} "
                         f"[{entry.get('error_id')}] {entry.get('detail')}")
    paired = block.get("paired_scoring") or {}
    lines.append(f"  Paired scoring: {paired.get('status')} ({paired.get('reason', '')})")
    return lines


# ---------------------------------------------------------------------------
# #53 provenance context
# ---------------------------------------------------------------------------

def provenance_context(manifest_document, verify_result, labeled_paths, *,
                       case_manifest_sha256=None):
    """Build the per-evaluation provenance context from verified #53 results.

    ``manifest_document`` is None (no manifest), a legacy document or a v1
    document already verified by ``run_provenance``; ``verify_result`` is
    the ``verify_artifact_set`` result for ``labeled_paths``. Execution
    bindings are looked up with ``run_provenance.executions_for_artifact``.
    """
    if manifest_document is None:
        return {"mode": "none"}
    import run_provenance as provenance
    if not provenance.is_v1_manifest(manifest_document):
        return {"mode": "legacy"}
    verified = set((verify_result or {}).get("verified", []))
    bindings = {}
    for label, path, role in labeled_paths:
        if label in verified:
            bindings[label] = provenance.executions_for_artifact(manifest_document, path, role)
    return {"mode": "v1", "verified": verified, "bindings": bindings,
            "case_manifest_sha256": case_manifest_sha256}
