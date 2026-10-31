"""Canonical distributional metric extraction (Issue #55).

This module is the single extraction path for the versioned contract
``evaluation/metrics/distributional-metrics-v1.json``. It turns normalized
per-realization evidence (particle end states or gridded fields) into
machine-readable metric records with stable identifiers, explicit units,
weighting, representation, provenance and a typed status.

Ownership boundaries:

- Metric mathematics stays in :mod:`metrics` (center of mass, covariance,
  quantiles, field comparison, footprints). This module validates evidence,
  selects weights, normalizes layouts and wraps results; it adds only the
  argmax hotspot and the layer binning.
- Cell areas and volumes come from :func:`io_gpu.cell_volume_m3` (FLEXPART
  ``outgrid_mod.f90`` latitude zones); grid alignment from
  :data:`report.GRID_ABS_TOL`.
- Kilogram budgets are read from the #54 ``INV-MASS-CLOSURE`` record and the
  artifact's own reservoirs; no second ledger is computed.
- #52/#53/#54 are consumed, never re-decided: paired values exist only when
  the invariant aggregate is ``PASS`` and ``paired_scoring.status`` is
  ``ELIGIBLE``.

Particle-space and gridded-field metrics never share an identifier; a
concentration grid is never treated as particles, particle counts are never
labeled as mass, and no threshold, ensemble statistic or verdict is computed.
"""

import json
import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import invariants
import io_gpu
import metrics
import report as report_lib

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CONTRACT = REPO_ROOT / "evaluation" / "metrics" / "distributional-metrics-v1.json"
CONTRACT_ID = "flexpart-gpu.distributional-metrics"
CONTRACT_VERSION = 1

COMPUTED = "COMPUTED"
DEGENERATE = "DEGENERATE"
MISSING_EVIDENCE = "MISSING_EVIDENCE"
INCOMPATIBLE = "INCOMPATIBLE"
BLOCKED = "BLOCKED"
INTEGRITY_ERROR = "INTEGRITY_ERROR"
UNSUPPORTED = "UNSUPPORTED"
STATUSES = (COMPUTED, DEGENERATE, MISSING_EVIDENCE, INCOMPATIBLE, BLOCKED,
            INTEGRITY_ERROR, UNSUPPORTED)
STATUS_PRECEDENCE = (INTEGRITY_ERROR, INCOMPATIBLE, MISSING_EVIDENCE, UNSUPPORTED, BLOCKED,
                     DEGENERATE, COMPUTED)

# Reason code -> status emitted by this implementation; must equal the contract.
REASON_STATUS = {
    **{code: INTEGRITY_ERROR for code in (
        "NONFINITE_VALUE", "NEGATIVE_WEIGHT", "WEIGHT_UNIT_INVALID", "INVALID_COORDINATE",
        "INCOMPLETE_FIELD", "NEGATIVE_EIGENVALUE", "ARTIFACT_HASH_MISMATCH",
        "MIXED_EXECUTION_PROVENANCE", "LAYOUT_UNDECLARED")},
    **{code: MISSING_EVIDENCE for code in (
        "PARTICLE_DUMP_UNAVAILABLE", "PARTICLE_MASS_UNAVAILABLE", "GRID_FIELD_UNAVAILABLE",
        "DEPOSITION_RESERVOIR_UNAVAILABLE", "MASS_LEDGER_UNAVAILABLE",
        "LAYER_GEOMETRY_UNAVAILABLE", "VERTICAL_FIELD_UNAVAILABLE")},
    **{code: INCOMPATIBLE for code in (
        "GRID_MISMATCH", "UNIT_MISMATCH", "QUANTITY_MISMATCH", "SPECIES_MISMATCH",
        "SPECIES_UNDECLARED", "WINDOW_MISMATCH", "VERTICAL_REFERENCE_MISMATCH",
        "WEIGHTING_MISMATCH", "REGRIDDING_UNAVAILABLE", "NONCONSERVATIVE_TRANSFORMATION",
        "CONTRACT_VERSION_MISMATCH")},
    **{code: BLOCKED for code in (
        "INPUT_EQUIVALENCE_NOT_DEMONSTRATED", "PROVENANCE_NOT_BOUND", "INVARIANT_GATE_NOT_PASS")},
    **{code: DEGENERATE for code in (
        "ZERO_TOTAL_WEIGHT", "ZERO_VARIANCE", "ZERO_DENOMINATOR", "EMPTY_FOOTPRINT")},
    **{code: UNSUPPORTED for code in (
        "LONGITUDE_WRAP_UNSUPPORTED", "MIXING_HEIGHT_UNAVAILABLE",
        "FIELD_MASS_SCALE_UNDECLARED")},
}

WEIGHT_MASS = "particle_mass_kg"
WEIGHT_COUNT = "particle_count"
WEIGHT_CELL_MASS = "cell_mass_proportional"
WEIGHT_CELL_SAMPLE = "cell_sample_unweighted"
WEIGHT_NONE = "none"
PARTICLE_WEIGHTINGS = (WEIGHT_MASS, WEIGHT_COUNT)

QUANTITY_PARTICLES = "particle_ensemble"
QUANTITY_LEDGER = "mass_ledger"
QUANTITY_CONCENTRATION = "air_concentration"
QUANTITY_DRY = "dry_deposition"
QUANTITY_WET = "wet_deposition"
SURFACE_QUANTITIES = (QUANTITY_DRY, QUANTITY_WET)

LAYOUT_XYZ = "ix_iy_iz"
LAYOUT_X_FASTEST_2D = "x_fastest_2d"
TRANSFORM_IDENTITY = "identity"
TRANSFORM_CONSERVATIVE_REGRID = "conservative_regrid"

STOCHASTIC_CANDIDATE = "candidate_philox"
STOCHASTIC_ORACLE = "flexpart_oracle_internal"

# Implemented metric ids and their definition versions; must equal the contract.
IMPLEMENTED = {
    "particle.horizontal.center_of_mass": 1,
    "particle.vertical.mean_height": 1,
    "particle.horizontal.covariance": 1,
    "particle.horizontal.principal_axes": 1,
    "particle.horizontal.radial_containment": 1,
    "particle.vertical.quantiles": 1,
    "particle.vertical.layer_fraction": 1,
    "particle.vertical.fraction_above_mixing_height": 1,
    "grid.horizontal.center_of_mass": 1,
    "grid.vertical.mean_height": 1,
    "grid.horizontal.covariance": 1,
    "grid.horizontal.principal_axes": 1,
    "grid.horizontal.radial_containment": 1,
    "grid.horizontal.mass_footprint_area": 1,
    "grid.horizontal.hotspot": 1,
    "grid.vertical.layer_fraction": 1,
    "grid.field.integrated_total": 1,
    "paired.grid.field_statistics": 1,
    "paired.grid.shape_statistics": 1,
    "paired.grid.support_overlap": 1,
    "paired.grid.mass_footprint_overlap": 1,
    "paired.grid.center_separation": 1,
    "budget.mass_closure": 1,
    "budget.deposited_mass_total": 1,
}
PARTICLE_HORIZONTAL_IDS = ("particle.horizontal.center_of_mass",
                           "particle.horizontal.covariance",
                           "particle.horizontal.principal_axes",
                           "particle.horizontal.radial_containment")
PARTICLE_VERTICAL_IDS = ("particle.vertical.mean_height", "particle.vertical.quantiles",
                         "particle.vertical.layer_fraction")
PARTICLE_IDS = (PARTICLE_HORIZONTAL_IDS + PARTICLE_VERTICAL_IDS
                + ("particle.vertical.fraction_above_mixing_height",))
GRID_HORIZONTAL_IDS = ("grid.horizontal.center_of_mass", "grid.horizontal.covariance",
                       "grid.horizontal.principal_axes", "grid.horizontal.radial_containment",
                       "grid.horizontal.mass_footprint_area", "grid.horizontal.hotspot")
GRID_VERTICAL_IDS = ("grid.vertical.mean_height", "grid.vertical.layer_fraction")
GRID_UNARY_IDS = GRID_HORIZONTAL_IDS + GRID_VERTICAL_IDS + ("grid.field.integrated_total",)
PAIRED_GRID_IDS = ("paired.grid.field_statistics", "paired.grid.shape_statistics",
                   "paired.grid.support_overlap", "paired.grid.mass_footprint_overlap",
                   "paired.grid.center_separation")

DEFINITION_KEYS = frozenset({
    "id", "version", "family", "classification", "representation", "meaning",
    "quantities", "weightings", "outputs", "required_artifacts", "aggregation_domain",
    "time_window", "normalization", "quantile_convention", "missing_data", "degenerate",
    "owner", "process_references",
})
OPTIONAL_DEFINITION_KEYS = frozenset({"parameters"})
CONTRACT_KEYS = frozenset({
    "contract_id", "contract_version", "date", "issue", "evaluator",
    "report_schema_version", "normative_oracle", "scope", "record_statuses",
    "status_precedence", "reason_codes", "weightings", "quantities", "field_layouts",
    "coordinate_references", "permitted_transformations", "prerequisites",
    "comparability_key", "metrics", "process_inventory", "downstream_handoff",
    "unimplemented", "change_policy",
})
REQUIRED_PROCESS_INVENTORY = ("#7", "#8", "#13", "#14", "#16")

# A minor covariance eigenvalue below -ROUNDOFF_REL*trace is not roundoff.
ROUNDOFF_REL = 1e-12
ISOTROPY_REL = 1e-9
# Axis separations below 1e-12 km^2 (1 mm^2) are coordinate roundoff, not an orientation.
ORIENTATION_FLOOR_KM2 = 1e-12
MAX_LONGITUDE_SPAN_DEG = 180.0


class MetricContractError(ValueError):
    """The distributional metric contract is invalid or mismatched."""


class _MetricStop(Exception):
    """Internal: one metric (or source) stops with a typed reason code."""

    def __init__(self, reason_code, detail):
        super().__init__(detail)
        self.reason_code = reason_code
        self.detail = detail


# ---------------------------------------------------------------------------
# Contract loading and validation
# ---------------------------------------------------------------------------

def load_contract(path=DEFAULT_CONTRACT):
    """Load and validate the metric contract; attach its canonical digest."""
    path = Path(path)
    try:
        document = json.loads(path.read_bytes().decode("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise MetricContractError(f"metric contract unreadable: {path}: {exc}") from None
    validate_contract(document)
    digest = invariants._canonical_digest(document)
    document["_path"] = str(path)
    document["_sha256"] = digest
    document["_by_id"] = {d["id"]: d for d in document["metrics"]}
    return document


def validate_contract(document):
    """Validate the closed contract surface against the implemented extractors."""
    if not isinstance(document, dict):
        raise MetricContractError("metric contract must be a JSON object")
    keys = set(document)
    if keys != CONTRACT_KEYS:
        raise MetricContractError(
            f"metric contract keys differ: missing={sorted(CONTRACT_KEYS - keys)}, "
            f"unexpected={sorted(keys - CONTRACT_KEYS)}")
    if document["contract_id"] != CONTRACT_ID or document["contract_version"] != CONTRACT_VERSION:
        raise MetricContractError(
            f"contract identity {document['contract_id']} v{document['contract_version']} "
            f"is not the implemented {CONTRACT_ID} v{CONTRACT_VERSION}")
    if document["report_schema_version"] != report_lib.SCHEMA_VERSION:
        raise MetricContractError(
            f"contract targets report schema {document['report_schema_version']}, "
            f"evaluator writes {report_lib.SCHEMA_VERSION}")
    if set(document["record_statuses"]) != set(STATUSES) or \
            tuple(document["status_precedence"]) != STATUS_PRECEDENCE:
        raise MetricContractError("record statuses or precedence differ from the implementation")
    if document["reason_codes"] != REASON_STATUS:
        raise MetricContractError(
            "contract reason codes differ from the implementation: "
            f"{sorted(set(document['reason_codes'].items()) ^ set(REASON_STATUS.items()))}")
    for name in REQUIRED_PROCESS_INVENTORY:
        if name not in document["process_inventory"]:
            raise MetricContractError(f"process inventory lacks {name}")
    seen = {}
    for definition in document["metrics"]:
        _validate_definition(definition, document)
        if definition["id"] in seen:
            raise MetricContractError(f"duplicate metric id {definition['id']}")
        seen[definition["id"]] = definition["version"]
    if seen != IMPLEMENTED:
        raise MetricContractError(
            f"contract metrics differ from the implementation: contract={sorted(seen.items())}, "
            f"implemented={sorted(IMPLEMENTED.items())}")
    for name, inventory in document["process_inventory"].items():
        for metric_id in inventory.get("metric_ids", []):
            if metric_id not in seen:
                raise MetricContractError(f"process inventory {name} names unknown {metric_id}")


def _validate_definition(definition, document):
    if not isinstance(definition, dict):
        raise MetricContractError("metric definition must be an object")
    keys = set(definition)
    missing = DEFINITION_KEYS - keys
    unexpected = keys - DEFINITION_KEYS - OPTIONAL_DEFINITION_KEYS
    if missing or unexpected:
        raise MetricContractError(
            f"metric {definition.get('id')} keys differ: missing={sorted(missing)}, "
            f"unexpected={sorted(unexpected)}")
    metric_id = definition["id"]
    prefix = {"particle": "particle.", "grid": ("grid.", "paired.grid."),
              "ledger": "budget."}.get(definition["representation"])
    if prefix is None or not metric_id.startswith(prefix):
        raise MetricContractError(
            f"metric {metric_id} representation {definition['representation']!r} "
            "does not match its identifier")
    expected_class = "paired" if metric_id.startswith("paired.") else "unary"
    if definition["classification"] != expected_class:
        raise MetricContractError(f"metric {metric_id} must be classified {expected_class}")
    for weighting in definition["weightings"]:
        if weighting not in document["weightings"]:
            raise MetricContractError(f"metric {metric_id} names unknown weighting {weighting}")
    for quantity in definition["quantities"]:
        if quantity not in document["quantities"]:
            raise MetricContractError(f"metric {metric_id} names unknown quantity {quantity}")
    text = json.dumps(definition).lower()
    for forbidden in ("threshold_value", "pass_if", "fail_if", "margin", "confidence"):
        if forbidden in text:
            raise MetricContractError(
                f"metric {metric_id} embeds verdict semantics ({forbidden}); thresholds are "
                "owned by the physics issues and #59")
    if not isinstance(definition["outputs"], dict) or not definition["outputs"]:
        raise MetricContractError(f"metric {metric_id} declares no outputs")


def contract_identity(contract):
    """Identity fields every record and block must carry."""
    return {"contract_id": contract["contract_id"],
            "contract_version": contract["contract_version"],
            "contract_path": invariants._repo_relative(contract["_path"]),
            "contract_sha256": contract["_sha256"]}


# ---------------------------------------------------------------------------
# Evidence descriptors
# ---------------------------------------------------------------------------

def particle_source(role, realization_id, *, lons, lats, heights, masses, mass_unit,
                    vertical_reference, artifacts, species=None, reservoirs=None,
                    stochastic_identity=None, end_epoch_s=None):
    """Normalize one particle end-state realization for extraction.

    ``masses`` is None when the artifact carries no decoded masses (oracle
    ``partposit``); then only ``particle_count`` metrics are extracted.
    ``reservoirs`` holds only ledgers the artifact actually carries.
    """
    return {"representation": "particle", "role": role, "realization_id": realization_id,
            "lons": lons, "lats": lats, "heights": heights, "masses": masses,
            "mass_unit": mass_unit, "vertical_reference": vertical_reference,
            "artifacts": list(artifacts or []), "species": species,
            "reservoirs": None if reservoirs is None else dict(reservoirs),
            "stochastic_identity": stochastic_identity, "end_epoch_s": end_epoch_s}


def grid_field(role, realization_id, *, field_id, quantity, measure, unit, values, layout,
               grid, window, artifacts, species=None, mass_scale_to_kg=None,
               transformation=TRANSFORM_IDENTITY):
    """Normalize one gridded field for extraction.

    ``grid`` uses ``nx/ny/nz/xlon0/ylat0/dx/dy/heights_m/heights_ref`` (``nz``
    is 1 and heights are ignored for surface quantities). ``measure`` is
    ``per_volume``, ``per_area`` or ``per_cell``. ``window`` declares
    ``kind`` (``time_average``, ``accumulated_since_start``,
    ``instantaneous``) with ``start_epoch_s``/``end_epoch_s``.
    """
    return {"representation": "grid", "role": role, "realization_id": realization_id,
            "field_id": field_id, "quantity": quantity, "measure": measure, "unit": unit,
            "values": values, "layout": layout, "grid": dict(grid or {}),
            "window": dict(window or {}), "artifacts": list(artifacts or []),
            "species": species, "mass_scale_to_kg": mass_scale_to_kg,
            "transformation": transformation}


class ExtractionContext:
    """Per-evaluation context: contract, #54 gate, case identity and layers."""

    def __init__(self, contract, gate, *, case_id, case_manifest_sha256=None,
                 layer_heights_m=None, layer_reference=None, verify_hashes=True):
        self.contract = contract
        self.gate = gate
        self.case_id = case_id
        self.case_manifest_sha256 = case_manifest_sha256
        self.layer_heights_m = layer_heights_m
        self.layer_reference = layer_reference
        self.verify_hashes = verify_hashes
        self._hash_cache = {}
        self._by_realization = {r["realization_id"]: r
                                for r in (gate or {}).get("realizations") or []}

    @property
    def gate_verdict(self):
        return (self.gate or {}).get("verdict", invariants.BLOCKED)

    @property
    def paired_scoring(self):
        return (self.gate or {}).get("paired_scoring") or {"status": "NOT_REQUESTED"}

    @property
    def scoring_eligible(self):
        return (self.gate_verdict == invariants.PASS
                and self.paired_scoring.get("status") == "ELIGIBLE")

    def realization_identity(self, source):
        realization = self._by_realization.get(source["realization_id"]) or {}
        stochastic = source.get("stochastic_identity")
        namespace = STOCHASTIC_CANDIDATE if source["role"] == "candidate" else STOCHASTIC_ORACLE
        return {"realization_id": source["realization_id"], "role": source["role"],
                "seed_index": realization.get("seed_index"),
                "execution_identity": realization.get("execution_identity"),
                "invariant_verdict": realization.get("verdict", invariants.BLOCKED),
                "stochastic_identity": {"namespace": namespace, "identity": stochastic}}

    def artifact_problem(self, artifacts):
        """Return a hash-mismatch detail for mutated or vanished artifacts, else None."""
        if not self.verify_hashes:
            return None
        for artifact in artifacts:
            path, expected = artifact.get("path"), artifact.get("sha256")
            if not path or not expected:
                return f"artifact {artifact.get('label')} lacks a path or recorded sha256"
            if path not in self._hash_cache:
                file_path = Path(path)
                self._hash_cache[path] = (report_lib.sha256_file(file_path)
                                          if file_path.is_file() else None)
            actual = self._hash_cache[path]
            if actual != expected:
                return (f"artifact {artifact.get('label')} hash changed after #53/#54 "
                        f"verification (recorded {str(expected)[:16]}, now {str(actual)[:16]})")
        return None


# ---------------------------------------------------------------------------
# Record construction
# ---------------------------------------------------------------------------

def _finite_tree(value):
    if isinstance(value, bool) or value is None or isinstance(value, str):
        return True
    if isinstance(value, (int, float)):
        return math.isfinite(value)
    if isinstance(value, dict):
        return all(_finite_tree(v) for v in value.values())
    if isinstance(value, (list, tuple)):
        return all(_finite_tree(v) for v in value)
    return False


def comparability_key(metric_id, version, representation, weighting, quantity, species,
                      vertical_reference, parameters):
    """Stable grouping key for #58: values are comparable only within one key."""
    return "|".join([metric_id, f"v{version}", representation, str(weighting), str(quantity),
                     str(species), str(vertical_reference),
                     json.dumps(parameters, sort_keys=True, separators=(",", ":"))])


def make_record(ctx, metric_id, *, role, value=None, reason_code=None, detail="",
                weighting, quantity, species=None, vertical_reference=None,
                realization=None, candidate=None, oracle=None, artifacts=None,
                time_window=None, domain=None, field_ids=None):
    """Build one contract record; non-finite values become INTEGRITY_ERROR."""
    definition = ctx.contract["_by_id"][metric_id]
    if reason_code is None and not _finite_tree(value):
        reason_code, detail, value = "NONFINITE_VALUE", "metric produced a non-finite value", None
    if reason_code is None:
        status = COMPUTED
    else:
        status = ctx.contract["reason_codes"].get(reason_code)
        if status is None:
            raise MetricContractError(f"reason code {reason_code!r} is not declared")
        value = None
    if weighting not in definition["weightings"]:
        raise MetricContractError(f"{metric_id} does not allow weighting {weighting}")
    if quantity is not None and quantity not in definition["quantities"]:
        raise MetricContractError(f"{metric_id} does not allow quantity {quantity}")
    parameters = definition.get("parameters") or {}
    paired = definition["classification"] == "paired"
    record = {
        "metric_id": metric_id,
        "definition_version": definition["version"],
        "contract_version": ctx.contract["contract_version"],
        "classification": "paired" if paired else "diagnostic",
        "representation": definition["representation"],
        "role": role,
        "status": status,
        "reason_code": reason_code,
        "reason": detail if reason_code else "",
        "value": value,
        "units": definition["outputs"],
        "weighting": weighting,
        "normalization": definition["normalization"],
        "quantity": quantity,
        "species": species,
        "vertical_reference": vertical_reference,
        "parameters": parameters,
        "case": {"case_id": ctx.case_id, "case_manifest_sha256": ctx.case_manifest_sha256},
        "time_window": time_window,
        "aggregation_domain": domain,
        "field_ids": field_ids,
        "source_artifacts": [{"label": a.get("label"), "sha256": a.get("sha256")}
                             for a in (artifacts or [])],
        "comparability_key": comparability_key(
            metric_id, definition["version"], definition["representation"], weighting,
            quantity, species, vertical_reference, parameters),
    }
    if paired:
        record["realizations"] = {"candidate": candidate, "oracle": oracle}
        paired_scoring = ctx.paired_scoring
        record["input_equivalence"] = {
            "verdict": paired_scoring.get("input_equivalence"),
            "report_sha256": paired_scoring.get("input_equivalence_report_sha256"),
            "paired_scoring_status": paired_scoring.get("status")}
    else:
        record["realization"] = realization
    record["eligible_for_paired_scoring"] = bool(status == COMPUTED and ctx.scoring_eligible)
    return record


def missing_records(ctx, metric_ids, *, role, reason_code, detail, quantities=None,
                    weightings=None, species=None, vertical_reference=None, realization=None):
    """Explicit records for metrics whose evidence is absent (never omitted)."""
    records = []
    for metric_id in metric_ids:
        definition = ctx.contract["_by_id"][metric_id]
        for quantity in (quantities or definition["quantities"]):
            if quantity not in definition["quantities"]:
                continue
            for weighting in (weightings or definition["weightings"]):
                if weighting not in definition["weightings"]:
                    continue
                records.append(make_record(
                    ctx, metric_id, role=role, reason_code=reason_code, detail=detail,
                    weighting=weighting, quantity=quantity, species=species,
                    vertical_reference=vertical_reference, realization=realization))
    return records


# ---------------------------------------------------------------------------
# Particle-space extraction
# ---------------------------------------------------------------------------

def _validate_horizontal(lons, lats):
    if lons is None or lats is None or len(lons) != len(lats):
        raise _MetricStop("PARTICLE_DUMP_UNAVAILABLE", "particle lon/lat arrays are absent")
    for lon, lat in zip(lons, lats):
        if not invariants._is_number(lon) or not invariants._is_number(lat) \
                or not math.isfinite(lon) or not math.isfinite(lat) or not -90.0 <= lat <= 90.0:
            raise _MetricStop("INVALID_COORDINATE", f"invalid particle position ({lon}, {lat})")
    if lons and max(lons) - min(lons) > MAX_LONGITUDE_SPAN_DEG:
        raise _MetricStop("LONGITUDE_WRAP_UNSUPPORTED",
                          "particle longitudes span more than 180 degrees; the local-tangent "
                          "projection and naive longitude averaging are invalid across the wrap")


def _validate_heights(heights, reference, count):
    if heights is None:
        raise _MetricStop("PARTICLE_DUMP_UNAVAILABLE", "particle heights are absent")
    if reference not in ("agl", "asl"):
        raise _MetricStop("VERTICAL_REFERENCE_MISMATCH",
                          f"particle vertical reference {reference!r} is not declared agl/asl")
    if len(heights) != count:
        raise _MetricStop("INCOMPLETE_FIELD", "height array length differs from positions")
    for z in heights:
        if not invariants._is_number(z) or not math.isfinite(z) or (reference == "agl" and z < 0):
            raise _MetricStop("INVALID_COORDINATE", f"invalid particle height {z} m {reference}")


def _particle_weights(source, weighting):
    count = len(source["lons"] or [])
    if weighting == WEIGHT_COUNT:
        return [1.0] * count
    masses = source.get("masses")
    if masses is None:
        raise _MetricStop("PARTICLE_MASS_UNAVAILABLE",
                          "artifact carries no decoded particle masses; only particle_count "
                          "weighting is supported")
    if source.get("mass_unit") != "kg":
        raise _MetricStop("WEIGHT_UNIT_INVALID",
                          f"particle_mass_kg weighting needs masses in kg, got "
                          f"{source.get('mass_unit')!r}")
    if len(masses) != count:
        raise _MetricStop("INCOMPLETE_FIELD", "mass array length differs from positions")
    for mass in masses:
        if not invariants._is_number(mass) or not math.isfinite(mass):
            raise _MetricStop("NONFINITE_VALUE", f"non-finite particle mass {mass!r}")
        if mass < 0:
            raise _MetricStop("NEGATIVE_WEIGHT", f"negative particle mass {mass} kg")
    return [float(m) for m in masses]


def _positive(values, weights):
    pairs = [(v, w) for v, w in zip(values, weights) if w > 0]
    return [v for v, _ in pairs], [w for _, w in pairs]


def _layer_fractions(heights, weights, layer_tops):
    """Weight fraction per half-open layer [bottom, top) plus at/above the top."""
    total = math.fsum(weights)
    sums = [0.0] * len(layer_tops)
    above = 0.0
    for z, w in zip(heights, weights):
        bottom = 0.0
        for index, top in enumerate(layer_tops):
            if bottom <= z < top:
                sums[index] += w
                break
            bottom = top
        else:
            above += w
    return {"layer_top_m": [float(t) for t in layer_tops],
            "layer_fraction": [s / total for s in sums],
            "fraction_above_top": above / total}


def _particle_horizontal_values(lons, lats, weights):
    covariance = metrics.horizontal_covariance(lons, lats, weights)
    center = {"lon_deg": covariance["lon_mean_deg"], "lat_deg": covariance["lat_mean_deg"],
              "total_weight": math.fsum(weights)}
    return center, covariance


def _covariance_value(covariance):
    (xx, xy), (_, yy) = covariance["covariance_km2"]
    return {"cov_east_east_km2": xx, "cov_north_north_km2": yy, "cov_east_north_km2": xy,
            "sigma_east_km": covariance["sigma_east_km"],
            "sigma_north_km": covariance["sigma_north_km"]}


def _principal_value(covariance):
    try:
        return metrics.principal_axes(covariance["covariance_km2"], ROUNDOFF_REL, ISOTROPY_REL,
                                      ORIENTATION_FLOOR_KM2)
    except ValueError as exc:
        raise _MetricStop("NEGATIVE_EIGENVALUE", str(exc)) from None


def _radial_value(lons, lats, weights, center_lon, center_lat, probabilities):
    lon_kept, w_kept = _positive(lons, weights)
    lat_kept, _ = _positive(lats, weights)
    xs, ys = metrics.local_tangent_offsets_km(lon_kept, lat_kept, center_lon, center_lat)
    radii = [math.hypot(x, y) for x, y in zip(xs, ys)]
    result = metrics.weighted_quantiles(radii, w_kept, probabilities)
    return {"radius_km": result["quantiles"], "center_lon_deg": center_lon,
            "center_lat_deg": center_lat}


def _run(builder):
    """Evaluate one metric builder, mapping typed stops to (None, code, detail)."""
    try:
        return builder(), None, ""
    except _MetricStop as stop:
        return None, stop.reason_code, stop.detail


def extract_particle_records(ctx, source):
    """Unary particle-space records for one realization, for both weightings."""
    realization = ctx.realization_identity(source)
    count = len(source.get("lons") or [])
    common = {"role": source["role"], "quantity": QUANTITY_PARTICLES,
              "species": source.get("species"), "realization": realization,
              "artifacts": source["artifacts"],
              "time_window": {"kind": "instantaneous_end_state",
                              "end_epoch_s": source.get("end_epoch_s")},
              "domain": {"kind": "particle_ensemble", "active_particles": count}}
    source_stop = None
    problem = ctx.artifact_problem(source["artifacts"])
    if problem:
        source_stop = _MetricStop("ARTIFACT_HASH_MISMATCH", problem)
    horizontal_stop = vertical_stop = source_stop
    if source_stop is None:
        try:
            _validate_horizontal(source.get("lons"), source.get("lats"))
        except _MetricStop as stop:
            horizontal_stop = stop
        try:
            _validate_heights(source.get("heights"), source.get("vertical_reference"), count)
        except _MetricStop as stop:
            vertical_stop = stop
    records = []
    for weighting in PARTICLE_WEIGHTINGS:
        records.extend(_particle_records_for_weighting(
            ctx, source, weighting, common, horizontal_stop, vertical_stop))
    return records


def _particle_records_for_weighting(ctx, source, weighting, common, horizontal_stop,
                                    vertical_stop):
    vertical_reference = source.get("vertical_reference")
    weight_stop = None
    weights = None
    try:
        weights = _particle_weights(source, weighting)
        if math.fsum(weights) <= 0:
            raise _MetricStop("ZERO_TOTAL_WEIGHT", "total particle weight is zero")
    except _MetricStop as stop:
        weight_stop = stop
    records = []

    def emit(metric_id, stop, builder, vref=None):
        if stop is not None:
            records.append(make_record(ctx, metric_id, reason_code=stop.reason_code,
                                       detail=stop.detail, weighting=weighting,
                                       vertical_reference=vref, **common))
            return
        value, code, detail = _run(builder)
        records.append(make_record(ctx, metric_id, value=value, reason_code=code, detail=detail,
                                   weighting=weighting, vertical_reference=vref, **common))

    lons, lats, heights = source.get("lons"), source.get("lats"), source.get("heights")
    h_stop = _precedence(horizontal_stop, weight_stop)
    v_stop = _precedence(vertical_stop, weight_stop)
    cache = {}

    def horizontal():
        if "h" not in cache:
            cache["h"] = _particle_horizontal_values(lons, lats, weights)
        return cache["h"]

    emit("particle.horizontal.center_of_mass", h_stop, lambda: horizontal()[0])
    emit("particle.horizontal.covariance", h_stop, lambda: _covariance_value(horizontal()[1]))
    emit("particle.horizontal.principal_axes", h_stop, lambda: _principal_value(horizontal()[1]))
    probabilities = ctx.contract["_by_id"]["particle.horizontal.radial_containment"][
        "parameters"]["probabilities"]
    emit("particle.horizontal.radial_containment", h_stop,
         lambda: _radial_value(lons, lats, weights, horizontal()[0]["lon_deg"],
                               horizontal()[0]["lat_deg"], probabilities))

    def mean_height():
        com = metrics.center_of_mass_particles([0.0] * len(heights), [0.0] * len(heights),
                                               heights, weights)
        return {"z_m": com["z_m"], "total_weight": com["total_mass_kg"]}

    def quantiles():
        kept_h, kept_w = _positive(heights, weights)
        probs = ctx.contract["_by_id"]["particle.vertical.quantiles"]["parameters"][
            "probabilities"]
        result = metrics.weighted_quantiles(kept_h, kept_w, probs)["quantiles"]
        return {"height_m": result,
                "interquartile_range_m": result["0.75"] - result["0.25"],
                "p10_p90_range_m": result["0.9"] - result["0.1"]}

    def layer_fraction():
        if not ctx.layer_heights_m:
            raise _MetricStop("LAYER_GEOMETRY_UNAVAILABLE",
                              "no declared output layer tops for particle binning")
        if ctx.layer_reference != vertical_reference:
            raise _MetricStop("VERTICAL_REFERENCE_MISMATCH",
                              f"layer reference {ctx.layer_reference!r} differs from particle "
                              f"reference {vertical_reference!r}")
        return _layer_fractions(heights, weights, ctx.layer_heights_m)

    emit("particle.vertical.mean_height", v_stop, mean_height, vertical_reference)
    emit("particle.vertical.quantiles", v_stop, quantiles, vertical_reference)
    emit("particle.vertical.layer_fraction", v_stop, layer_fraction, vertical_reference)
    emit("particle.vertical.fraction_above_mixing_height",
         _precedence(v_stop, _MetricStop(
             "MIXING_HEIGHT_UNAVAILABLE",
             "no per-particle mixing height in the artifact; follow-up owned by #64")),
         lambda: None, vertical_reference)
    return records


def _precedence(*stops):
    """The highest-precedence stop under STATUS_PRECEDENCE, or None."""
    present = [s for s in stops if s is not None]
    if not present:
        return None
    rank = {status: i for i, status in enumerate(STATUS_PRECEDENCE)}
    return min(present, key=lambda s: rank[REASON_STATUS[s.reason_code]])


# ---------------------------------------------------------------------------
# Gridded-field extraction
# ---------------------------------------------------------------------------

def _grid_shape(field):
    grid = field["grid"]
    try:
        nx, ny = int(grid["nx"]), int(grid["ny"])
        nz = 1 if field["quantity"] in SURFACE_QUANTITIES else int(grid["nz"])
        geometry = (float(grid["xlon0"]), float(grid["ylat0"]), float(grid["dx"]),
                    float(grid["dy"]))
    except (KeyError, TypeError, ValueError):
        raise _MetricStop("INCOMPLETE_FIELD", "grid geometry is incomplete") from None
    if nx <= 0 or ny <= 0 or nz <= 0 or geometry[2] <= 0 or geometry[3] <= 0:
        raise _MetricStop("INCOMPLETE_FIELD", "grid dimensions must be positive")
    return nx, ny, nz, geometry


def _normalized_field(field):
    """Validate one field and return (values in ix_iy_iz order, nx, ny, nz, geometry)."""
    nx, ny, nz, geometry = _grid_shape(field)
    values = field.get("values")
    if values is None:
        raise _MetricStop("GRID_FIELD_UNAVAILABLE", f"field {field.get('field_id')} has no values")
    if len(values) != nx * ny * nz:
        raise _MetricStop("INCOMPLETE_FIELD",
                          f"field {field.get('field_id')} has {len(values)} cells, "
                          f"expected {nx * ny * nz}")
    for value in values:
        if value is None:
            raise _MetricStop("INCOMPLETE_FIELD", "field contains a missing cell; missing "
                                                  "cells are never treated as zero")
        if not invariants._is_number(value) or not math.isfinite(value):
            raise _MetricStop("NONFINITE_VALUE", f"non-finite cell value {value!r}")
        if value < 0:
            raise _MetricStop("NEGATIVE_WEIGHT", f"negative cell value {value}")
    layout = field.get("layout")
    if layout == LAYOUT_XYZ:
        ordered = [float(v) for v in values]
    elif layout == LAYOUT_X_FASTEST_2D and nz == 1:
        ordered = [float(values[ix + iy * nx]) for ix in range(nx) for iy in range(ny)]
    else:
        raise _MetricStop("LAYOUT_UNDECLARED",
                          f"field layout {layout!r} is not a declared layout for nz={nz}")
    return ordered, nx, ny, nz, geometry


def _cell_masses(field, ordered, nx, ny, nz, geometry):
    """Cell weights proportional to mass, and the column areas in m^2."""
    _xlon0, ylat0, dx, dy = geometry
    measure = field.get("measure")
    allowed = ("per_volume", "per_cell") if field["quantity"] == QUANTITY_CONCENTRATION \
        else ("per_area", "per_cell")
    if measure not in allowed:
        raise _MetricStop("WEIGHT_UNIT_INVALID",
                          f"measure {measure!r} is not valid for {field['quantity']}")
    try:
        areas = [io_gpu.cell_volume_m3(ylat0 + (iy + 0.5) * dy, dx, dy, 1.0)
                 for _ix in range(nx) for iy in range(ny)]
    except ValueError as exc:
        raise _MetricStop("INVALID_COORDINATE", str(exc)) from None
    if measure == "per_cell":
        return ordered, areas
    if measure == "per_area":
        return [v * a for v, a in zip(ordered, areas)], areas
    heights = field["grid"].get("heights_m")
    if not heights or len(heights) != nz:
        raise _MetricStop("LAYER_GEOMETRY_UNAVAILABLE",
                          "per_volume field needs OUTHEIGHTS for cell volumes")
    try:
        return io_gpu.concentration_to_mass_per_cell(
            ordered, nx, ny, nz, ylat0, dx, dy, heights), areas
    except ValueError as exc:
        raise _MetricStop("INVALID_COORDINATE", str(exc)) from None


def _prepare_grid(ctx, field):
    """Validate and weight one field; raises _MetricStop for the whole field."""
    problem = ctx.artifact_problem(field["artifacts"])
    if problem:
        raise _MetricStop("ARTIFACT_HASH_MISMATCH", problem)
    if field.get("transformation") == TRANSFORM_CONSERVATIVE_REGRID:
        raise _MetricStop("REGRIDDING_UNAVAILABLE",
                          "field declares conservative regridding, but no validated "
                          "conservative mapping exists in the evaluator")
    if field.get("transformation") != TRANSFORM_IDENTITY:
        raise _MetricStop("NONCONSERVATIVE_TRANSFORMATION",
                          f"field transformation {field.get('transformation')!r} is not permitted")
    ordered, nx, ny, nz, geometry = _normalized_field(field)
    masses, areas = _cell_masses(field, ordered, nx, ny, nz, geometry)
    columns, ix_list, iy_list = metrics.grid_column_weights(masses, nx, ny, nz)
    xlon0, ylat0, dx, dy = geometry
    return {"values": ordered, "masses": masses, "columns": columns, "areas": areas,
            "column_lons": [xlon0 + (ix + 0.5) * dx for ix in ix_list],
            "column_lats": [ylat0 + (iy + 0.5) * dy for iy in iy_list],
            "ix": ix_list, "iy": iy_list, "nx": nx, "ny": ny, "nz": nz,
            "geometry": geometry, "total": math.fsum(masses)}


def _horizontal_wrap_stop(field):
    grid = field["grid"]
    try:
        span = float(grid["nx"]) * float(grid["dx"])
    except (KeyError, TypeError, ValueError):
        return None
    if span > MAX_LONGITUDE_SPAN_DEG:
        return _MetricStop("LONGITUDE_WRAP_UNSUPPORTED",
                           f"output grid spans {span} degrees of longitude")
    return None


def _grid_window(field):
    return dict(field.get("window") or {})


def _grid_domain(field):
    grid = field["grid"]
    nz = 1 if field["quantity"] in SURFACE_QUANTITIES else grid.get("nz")
    return {"kind": "output_grid", "nx": grid.get("nx"), "ny": grid.get("ny"), "nz": nz,
            "xlon0_deg": grid.get("xlon0"), "ylat0_deg": grid.get("ylat0"),
            "dx_deg": grid.get("dx"), "dy_deg": grid.get("dy")}


def extract_grid_records(ctx, field):
    """Unary gridded-field records for one field (concentration or deposition)."""
    realization = ctx.realization_identity(field)
    surface = field["quantity"] in SURFACE_QUANTITIES
    vertical_reference = None if surface else field["grid"].get("heights_ref")
    common = {"role": field["role"], "quantity": field["quantity"],
              "species": field.get("species"), "realization": realization,
              "artifacts": field["artifacts"], "time_window": _grid_window(field),
              "domain": _grid_domain(field), "field_ids": [field.get("field_id")]}
    field_stop, prepared = None, None
    try:
        prepared = _prepare_grid(ctx, field)
        if prepared["total"] <= 0:
            field_stop = _MetricStop("ZERO_TOTAL_WEIGHT", "field total is zero (all-zero field)")
    except _MetricStop as stop:
        field_stop = stop
    h_stop = _precedence(field_stop, _horizontal_wrap_stop(field))
    records = []

    def emit(metric_id, stop, builder, weighting=WEIGHT_CELL_MASS, vref=None):
        if stop is not None:
            records.append(make_record(ctx, metric_id, reason_code=stop.reason_code,
                                       detail=stop.detail, weighting=weighting,
                                       vertical_reference=vref, **common))
            return
        value, code, detail = _run(builder)
        records.append(make_record(ctx, metric_id, value=value, reason_code=code,
                                   detail=detail, weighting=weighting,
                                   vertical_reference=vref, **common))

    cache = {}

    def covariance():
        if "cov" not in cache:
            cache["cov"] = metrics.horizontal_covariance(
                prepared["column_lons"], prepared["column_lats"], prepared["columns"])
        return cache["cov"]

    emit("grid.horizontal.center_of_mass", h_stop,
         lambda: {"lon_deg": covariance()["lon_mean_deg"],
                  "lat_deg": covariance()["lat_mean_deg"]})
    emit("grid.horizontal.covariance", h_stop, lambda: _covariance_value(covariance()))
    emit("grid.horizontal.principal_axes", h_stop, lambda: _principal_value(covariance()))
    definition = ctx.contract["_by_id"]
    emit("grid.horizontal.radial_containment", h_stop,
         lambda: _radial_value(prepared["column_lons"], prepared["column_lats"],
                               prepared["columns"], covariance()["lon_mean_deg"],
                               covariance()["lat_mean_deg"],
                               definition["grid.horizontal.radial_containment"]["parameters"][
                                   "probabilities"]))
    emit("grid.horizontal.mass_footprint_area", h_stop, lambda: _footprint_value(
        prepared, definition["grid.horizontal.mass_footprint_area"]["parameters"]["fractions"]))
    emit("grid.horizontal.hotspot", h_stop, lambda: _hotspot_value(prepared))
    if not surface:
        v_stop = field_stop
        if v_stop is None and field["grid"].get("heights_ref") not in ("agl", "asl"):
            v_stop = _MetricStop("VERTICAL_REFERENCE_MISMATCH",
                                 "OUTHEIGHTS reference is not declared agl/asl")
        emit("grid.vertical.mean_height", v_stop, lambda: _grid_mean_height(field, prepared),
             vref=vertical_reference)
        emit("grid.vertical.layer_fraction", v_stop, lambda: _grid_layers(field, prepared),
             vref=vertical_reference)
    total_stop = field_stop if field_stop is not None and field_stop.reason_code != \
        "ZERO_TOTAL_WEIGHT" else None
    emit("grid.field.integrated_total", total_stop, lambda: _integrated_total(field, prepared),
         weighting=WEIGHT_NONE)
    return records


def _footprint_value(prepared, fractions):
    result = {"area_km2": {}, "cell_count": {}, "achieved_fraction": {}}
    for fraction in fractions:
        footprint = metrics.mass_containment_footprint(prepared["columns"], prepared["areas"],
                                                       fraction)
        result["area_km2"][str(fraction)] = footprint["area_km2"]
        result["cell_count"][str(fraction)] = footprint["cell_count"]
        result["achieved_fraction"][str(fraction)] = footprint["achieved_fraction"]
    return result


def _hotspot_value(prepared):
    densities = [c / a for c, a in zip(prepared["columns"], prepared["areas"])]
    peak = max(densities)
    index = densities.index(peak)
    return {"lon_deg": prepared["column_lons"][index], "lat_deg": prepared["column_lats"][index],
            "ix": prepared["ix"][index], "iy": prepared["iy"][index],
            "mass_fraction": prepared["columns"][index] / prepared["total"],
            "tied_cells": sum(1 for d in densities if d == peak)}


def _grid_mean_height(field, prepared):
    xlon0, ylat0, dx, dy = prepared["geometry"]
    com = metrics.center_of_mass_grid(prepared["masses"], prepared["nx"], prepared["ny"],
                                      prepared["nz"], xlon0, ylat0, dx, dy,
                                      field["grid"]["heights_m"])
    return {"z_m": com["z_m"]}


def _grid_layers(field, prepared):
    nx, ny, nz = prepared["nx"], prepared["ny"], prepared["nz"]
    sums = [math.fsum(prepared["masses"][((ix * ny) + iy) * nz + iz]
                      for ix in range(nx) for iy in range(ny)) for iz in range(nz)]
    return {"layer_top_m": [float(h) for h in field["grid"]["heights_m"]],
            "layer_fraction": [s / prepared["total"] for s in sums]}


def _integrated_total(field, prepared):
    scale = field.get("mass_scale_to_kg")
    if scale is None:
        raise _MetricStop("FIELD_MASS_SCALE_UNDECLARED",
                          f"field {field.get('field_id')} declares no kilogram scale "
                          f"(unit {field.get('unit')!r})")
    return {"total_kg": prepared["total"] * float(scale)}


# ---------------------------------------------------------------------------
# Paired gridded-field extraction
# ---------------------------------------------------------------------------

def _compatibility_stop(candidate, oracle, *, require_same_unit):
    if candidate["quantity"] != oracle["quantity"]:
        return _MetricStop("QUANTITY_MISMATCH",
                           f"quantity {candidate['quantity']} != {oracle['quantity']}")
    if candidate.get("species") is None or oracle.get("species") is None:
        return _MetricStop("SPECIES_UNDECLARED", "paired fields must declare their species")
    if candidate["species"] != oracle["species"]:
        return _MetricStop("SPECIES_MISMATCH",
                           f"species {candidate['species']} != {oracle['species']}")
    if (candidate.get("window") or {}) != (oracle.get("window") or {}):
        return _MetricStop("WINDOW_MISMATCH",
                           f"windows differ: candidate {candidate.get('window')}, "
                           f"oracle {oracle.get('window')}")
    mismatched = _grid_mismatches(candidate, oracle)
    if mismatched:
        return _MetricStop("GRID_MISMATCH",
                           f"grids differ in {mismatched}; no validated conservative mapping "
                           "is available (REGRIDDING_UNAVAILABLE)")
    if candidate["quantity"] == QUANTITY_CONCENTRATION and \
            candidate["grid"].get("heights_ref") != oracle["grid"].get("heights_ref"):
        return _MetricStop("VERTICAL_REFERENCE_MISMATCH",
                           f"OUTHEIGHTS reference {candidate['grid'].get('heights_ref')} != "
                           f"{oracle['grid'].get('heights_ref')}")
    if require_same_unit and (candidate.get("unit") != oracle.get("unit")
                              or candidate.get("measure") != oracle.get("measure")):
        return _MetricStop("UNIT_MISMATCH",
                           f"absolute statistics need one unit: candidate {candidate.get('unit')!r} "
                           f"({candidate.get('measure')}), oracle {oracle.get('unit')!r} "
                           f"({oracle.get('measure')})")
    return None


def _grid_mismatches(candidate, oracle):
    surface = candidate["quantity"] in SURFACE_QUANTITIES
    keys = ("nx", "ny", "xlon0", "ylat0", "dx", "dy") + (() if surface else ("nz",))
    mismatched = []
    for key in keys:
        a, b = candidate["grid"].get(key), oracle["grid"].get(key)
        if a is None or b is None or abs(float(a) - float(b)) > report_lib.GRID_ABS_TOL:
            mismatched.append(key)
    if not surface:
        a = candidate["grid"].get("heights_m") or []
        b = oracle["grid"].get("heights_m") or []
        if len(a) != len(b) or any(abs(float(x) - float(y)) > report_lib.GRID_ABS_TOL
                                   for x, y in zip(a, b)):
            mismatched.append("heights_m")
    return mismatched


def _mixed_provenance(candidate_identity, oracle_identity):
    cand = candidate_identity.get("execution_identity") or {}
    orac = oracle_identity.get("execution_identity") or {}
    if not cand or not orac:
        return None
    for key in ("case_id", "case_manifest_sha256"):
        if cand.get(key) != orac.get(key):
            return (f"candidate execution {key} {cand.get(key)} differs from oracle execution "
                    f"{orac.get(key)}")
    return None


def _eligibility_stop(ctx):
    if ctx.gate_verdict != invariants.PASS:
        return _MetricStop("INVARIANT_GATE_NOT_PASS",
                           f"#54 deterministic-invariant aggregate is {ctx.gate_verdict}; paired "
                           "metrics need PASS (a FAIL is never offset by distributional values)")
    paired = ctx.paired_scoring
    if paired.get("input_equivalence") != "INPUT_EQUIVALENT":
        return _MetricStop("INPUT_EQUIVALENCE_NOT_DEMONSTRATED",
                           f"#52 input equivalence is {paired.get('input_equivalence')}: "
                           f"{paired.get('reason', '')}")
    if paired.get("status") != "ELIGIBLE":
        return _MetricStop("PROVENANCE_NOT_BOUND",
                           f"paired scoring is {paired.get('status')}: {paired.get('reason', '')}")
    return None


def extract_paired_grid_records(ctx, candidate, oracle):
    """Paired candidate/oracle records for two gridded fields of one quantity."""
    cand_identity = ctx.realization_identity(candidate)
    oracle_identity = ctx.realization_identity(oracle)
    quantity = oracle["quantity"]
    common = {"role": "paired", "quantity": quantity, "species": oracle.get("species"),
              "candidate": cand_identity, "oracle": oracle_identity,
              "artifacts": candidate["artifacts"] + oracle["artifacts"],
              "time_window": _grid_window(oracle), "domain": _grid_domain(oracle),
              "field_ids": [candidate.get("field_id"), oracle.get("field_id")]}
    vertical_reference = None if quantity in SURFACE_QUANTITIES else \
        oracle["grid"].get("heights_ref")
    integrity, prepared = None, {}
    for label, field in (("candidate", candidate), ("oracle", oracle)):
        try:
            prepared[label] = _prepare_grid(ctx, field)
        except _MetricStop as stop:
            integrity = _precedence(integrity, _MetricStop(stop.reason_code,
                                                           f"{label}: {stop.detail}"))
    mixed = _mixed_provenance(cand_identity, oracle_identity)
    if mixed:
        integrity = _precedence(integrity, _MetricStop("MIXED_EXECUTION_PROVENANCE", mixed))
    eligibility = _eligibility_stop(ctx)
    degenerate = None
    if integrity is None and (prepared["candidate"]["total"] <= 0
                              or prepared["oracle"]["total"] <= 0):
        degenerate = _MetricStop("ZERO_TOTAL_WEIGHT", "a paired field is all zero")
    records = []
    builders = {
        "paired.grid.field_statistics": (True, WEIGHT_CELL_SAMPLE,
                                         lambda: _field_statistics(prepared)),
        "paired.grid.shape_statistics": (False, WEIGHT_CELL_MASS,
                                         lambda: _shape_statistics(prepared, degenerate)),
        "paired.grid.support_overlap": (False, WEIGHT_CELL_SAMPLE,
                                        lambda: _support_overlap(prepared)),
        "paired.grid.mass_footprint_overlap": (False, WEIGHT_CELL_MASS,
                                               lambda: _footprint_overlap(ctx, prepared,
                                                                          degenerate)),
        "paired.grid.center_separation": (False, WEIGHT_CELL_MASS,
                                          lambda: _center_separation(prepared, degenerate)),
    }
    for metric_id, (same_unit, weighting, builder) in builders.items():
        stop = _precedence(integrity, _compatibility_stop(candidate, oracle,
                                                          require_same_unit=same_unit),
                           eligibility)
        if metric_id == "paired.grid.center_separation":
            stop = _precedence(stop, _horizontal_wrap_stop(oracle))
        if stop is not None:
            records.append(make_record(ctx, metric_id, reason_code=stop.reason_code,
                                       detail=stop.detail, weighting=weighting,
                                       vertical_reference=vertical_reference, **common))
            continue
        value, code, detail = _run(builder)
        records.append(make_record(ctx, metric_id, value=value, reason_code=code, detail=detail,
                                   weighting=weighting, vertical_reference=vertical_reference,
                                   **common))
    return records


def _field_statistics(prepared):
    oracle_values = prepared["oracle"]["values"]
    candidate_values = prepared["candidate"]["values"]
    base = metrics.field_comparison_metrics(oracle_values, candidate_values)
    chang_hanna = metrics.etex_station_metrics(oracle_values, candidate_values)
    n = base["sample_count"]
    mean_oracle = base["sum_observed"] / n
    mean_candidate = base["sum_modeled"] / n
    undefined = {}
    relative = None
    if mean_oracle > 0:
        relative = (mean_candidate - mean_oracle) / mean_oracle
    else:
        undefined["relative_mean_bias"] = "ZERO_DENOMINATOR: oracle mean is zero"
    for key, reason in (("normalized_rmse", "ZERO_DENOMINATOR: both fields are all zero"),
                        ("correlation", "ZERO_VARIANCE: a field is constant")):
        if base[key] is None:
            undefined[key] = reason
    if chang_hanna["fractional_bias"] is None:
        undefined["fractional_bias"] = "ZERO_DENOMINATOR: mean_o + mean_c is zero"
    if chang_hanna["nmse"] is None:
        undefined["nmse"] = "ZERO_DENOMINATOR: a field mean is zero"
    return {"mean_bias": base["bias"], "mae": base["mae"], "rmse": base["rmse"],
            "normalized_rmse": base["normalized_rmse"], "relative_mean_bias": relative,
            "fractional_bias": chang_hanna["fractional_bias"], "nmse": chang_hanna["nmse"],
            "pearson_correlation": base["correlation"], "sample_count": n,
            "undefined": undefined}


def _unit_sum(values, total):
    return [v / total for v in values]


def _shape_statistics(prepared, degenerate):
    if degenerate:
        raise degenerate
    norm_o = _unit_sum(prepared["oracle"]["masses"], prepared["oracle"]["total"])
    norm_c = _unit_sum(prepared["candidate"]["masses"], prepared["candidate"]["total"])
    base = metrics.field_comparison_metrics(norm_o, norm_c)
    return {"pearson_correlation": base["correlation"],
            "normalized_rmse": base["normalized_rmse"],
            "normalized_mass_overlap": metrics.normalized_mass_overlap(norm_o, norm_c)}


def _support_overlap(prepared):
    overlap = metrics.footprint_overlap(prepared["oracle"]["values"],
                                        prepared["candidate"]["values"], threshold=0.0)
    if overlap["union_cells"] == 0:
        raise _MetricStop("EMPTY_FOOTPRINT", "both supports are empty")
    return {"candidate_active_cells": overlap["modeled_active_cells"],
            "oracle_active_cells": overlap["observed_active_cells"],
            "intersection_cells": overlap["intersection_cells"],
            "union_cells": overlap["union_cells"],
            "figure_of_merit_in_space": overlap["figure_of_merit_in_space"]}


def _footprint_overlap(ctx, prepared, degenerate):
    if degenerate:
        raise degenerate
    fractions = ctx.contract["_by_id"]["paired.grid.mass_footprint_overlap"]["parameters"][
        "fractions"]
    result = {"area_overlap_ratio": {}, "intersection_km2": {}, "union_km2": {}}
    areas = prepared["oracle"]["areas"]
    for fraction in fractions:
        cand = metrics.mass_containment_footprint(prepared["candidate"]["columns"], areas,
                                                  fraction)
        orac = metrics.mass_containment_footprint(prepared["oracle"]["columns"], areas, fraction)
        overlap = metrics.area_weighted_set_overlap(cand["cells"], orac["cells"], areas)
        key = str(fraction)
        result["area_overlap_ratio"][key] = overlap["area_overlap_ratio"]
        result["intersection_km2"][key] = overlap["intersection_km2"]
        result["union_km2"][key] = overlap["union_km2"]
    return result


def _center_separation(prepared, degenerate):
    if degenerate:
        raise degenerate
    centers = {}
    for label in ("candidate", "oracle"):
        cov = metrics.horizontal_covariance(prepared[label]["column_lons"],
                                            prepared[label]["column_lats"],
                                            prepared[label]["columns"])
        centers[label] = (cov["lon_mean_deg"], cov["lat_mean_deg"])
    xs, ys = metrics.local_tangent_offsets_km([centers["candidate"][0]],
                                              [centers["candidate"][1]], *centers["oracle"])
    return {"separation_km": math.hypot(xs[0], ys[0]), "east_offset_km": xs[0],
            "north_offset_km": ys[0]}


# ---------------------------------------------------------------------------
# Ledger records (#54 authoritative)
# ---------------------------------------------------------------------------

BUDGET_OUTPUTS = ("initial_mass_kg", "remaining_mass_kg", "dry_deposited_kg",
                  "wet_deposited_kg", "decayed_kg", "accounted_mass_kg", "absolute_error_kg",
                  "relative_error")
RESERVOIR_BY_QUANTITY = {QUANTITY_DRY: "dry_deposited_kg", QUANTITY_WET: "wet_deposited_kg"}


def budget_records(ctx, source, *, species=None, quantities=SURFACE_QUANTITIES):
    """Ledger records for one realization from its #54 record and own reservoirs.

    ``quantities`` limits the reservoir records to the deposition processes in
    scope (the case enables them); a disabled process gets no reservoir record.
    """
    identity = ctx.realization_identity(source)
    realization = ctx._by_realization.get(source["realization_id"]) or {}
    closure = next((r for r in realization.get("invariants") or []
                    if r.get("invariant_id") == "INV-MASS-CLOSURE"), None)
    common = {"role": source["role"], "species": species, "realization": identity,
              "artifacts": source["artifacts"], "weighting": WEIGHT_NONE,
              "time_window": {"kind": "simulation", "end_epoch_s": source.get("end_epoch_s")},
              "domain": {"kind": "realization"}}
    records = []
    if closure is not None and closure.get("measured"):
        value = {key: closure["measured"].get(key) for key in BUDGET_OUTPUTS}
        value["invariant_verdict"] = closure.get("verdict")
        records.append(make_record(ctx, "budget.mass_closure", value=value,
                                   quantity=QUANTITY_LEDGER, **common))
    else:
        detail = (f"#54 INV-MASS-CLOSURE {closure.get('verdict')} "
                  f"[{closure.get('error_id')}]: {closure.get('detail')}") if closure \
            else "no #54 INV-MASS-CLOSURE record for this realization"
        records.append(make_record(ctx, "budget.mass_closure",
                                   reason_code="MASS_LEDGER_UNAVAILABLE", detail=detail,
                                   quantity=QUANTITY_LEDGER, **common))
    reservoirs = source.get("reservoirs")
    for quantity, name in RESERVOIR_BY_QUANTITY.items():
        if quantity not in quantities:
            continue
        if reservoirs is None or name not in reservoirs:
            reason = ("the artifact carries no kilogram reservoir ledger"
                      if reservoirs is None else f"the artifact carries no {name} ledger")
            records.append(make_record(ctx, "budget.deposited_mass_total",
                                       reason_code="DEPOSITION_RESERVOIR_UNAVAILABLE",
                                       detail=f"{reason}; never inferred from particle mass loss",
                                       quantity=quantity, **common))
            continue
        total = reservoirs[name]
        if not invariants._is_number(total) or not math.isfinite(total):
            code, detail = "NONFINITE_VALUE", f"{name} is {total!r}"
        elif total < 0:
            code, detail = "NEGATIVE_WEIGHT", f"{name} is negative ({total} kg)"
        else:
            code, detail = None, ""
        records.append(make_record(ctx, "budget.deposited_mass_total",
                                   value=None if code else {"total_kg": float(total)},
                                   reason_code=code, detail=detail, quantity=quantity,
                                   **common))
    return records


# ---------------------------------------------------------------------------
# Report block and downstream handoff
# ---------------------------------------------------------------------------

def build_block(ctx, records, *, extraction_scope, notes=None):
    """Assemble the ``distributional_metrics`` report block."""
    counts = {status: 0 for status in STATUSES}
    for record in records:
        counts[record["status"]] += 1
    block = dict(contract_identity(ctx.contract))
    block.update({
        "case_id": ctx.case_id,
        "case_manifest_sha256": ctx.case_manifest_sha256,
        "extraction_scope": extraction_scope,
        "deterministic_invariants_verdict": ctx.gate_verdict,
        "paired_scoring_status": ctx.paired_scoring.get("status"),
        "eligible_for_paired_scoring": ctx.scoring_eligible,
        "eligibility_rule": ctx.contract["prerequisites"]["scoring_eligibility"],
        "counts": counts,
        "record_count": len(records),
        "records": records,
        "notes": list(notes or []),
    })
    return block


def collect_for_ensemble(reports, contract=None):
    """Fail-closed per-realization handoff for #58 (no aggregation of values).

    ``reports`` is a list of ``(path, report_dict)``. Every source must carry a
    block from the same contract identity and case; the #54 aggregate over all
    sources comes from :func:`invariants.aggregate_realizations`, so a failed
    seed can never be dropped. ``eligible`` is true only when that aggregate
    is PASS and every source block is eligible for paired scoring.
    """
    contract = contract or load_contract()
    identity = contract_identity(contract)
    records, case_ids, blocked = [], set(), []
    for path, document in reports:
        block = document.get("distributional_metrics")
        if block is None:
            # Not extracted (the source evaluation stopped); its #54 block carries why.
            blocked.append(str(path))
            continue
        if not isinstance(block, dict):
            raise MetricContractError(f"report {path} has a malformed distributional_metrics block")
        for key in ("contract_id", "contract_version", "contract_sha256"):
            if block.get(key) != identity[key]:
                raise MetricContractError(
                    f"report {path} uses {key}={block.get(key)!r}, current contract has "
                    f"{identity[key]!r} (CONTRACT_VERSION_MISMATCH)")
        case_ids.add(block.get("case_id"))
        if not block.get("eligible_for_paired_scoring"):
            blocked.append(str(path))
        for record in block.get("records") or []:
            records.append(dict(record, source_report=str(path)))
    if len(case_ids) > 1:
        raise MetricContractError(f"reports mix case ids {sorted(map(str, case_ids))}")
    gate_summary = invariants.aggregate_realizations(
        [document.get("deterministic_invariants") for _, document in reports])
    groups = {}
    for index, record in enumerate(records):
        groups.setdefault(f"{record['role']}::{record['comparability_key']}", []).append(index)
    eligible = gate_summary["verdict"] == invariants.PASS and not blocked and bool(reports)
    return {"contract": identity, "case_id": next(iter(case_ids), None),
            "deterministic_invariants": gate_summary, "eligible": eligible,
            "ineligible_reports": blocked, "records": records, "groups": groups,
            "pairing_rule": contract["downstream_handoff"]["#58"]}
