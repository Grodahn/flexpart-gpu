#!/usr/bin/env python3
"""Authoritative validation run-provenance path (issue #53).

Single provenance family binding::

    declared case/profile (#51) -> concrete execution instance
        -> immutable input/output bytes

This module consolidates the pre-existing #6 run-manifest/hash
infrastructure (``scripts/corpus/write_corpus_manifest.py`` and
``scripts/write_oracle_run_manifest.py``) into one versioned contract
(``schemas/run-manifest-v1.schema.json``). Writers delegate hashing,
execution-identity derivation and verification here instead of
maintaining parallel provenance shapes.

Scope (issue #53 owns execution attribution only):

- assign a stable, content-derived identity to each concrete
  candidate/oracle execution and, for ensembles, each realization;
- bind that identity to the exact case-manifest hash/version,
  executable/build/runtime identity and all consumed/produced bytes;
- verify hashes before reports consume artifacts;
- keep artifact directories non-overwriting and stale/mixed-run safe;
- distinguish pristine-oracle from seedable-validation-oracle
  executions by reusing the #49/#50 identities (never reimplemented).

Explicit non-scope: #51 case semantics and expected-artifact classes
are referenced, never copied; #52 ``INPUT_EQUIVALENT`` / representation
/ meteorology-equivalence logic is never implemented here; no
scientific metrics, thresholds or ensemble aggregation live here.

Only the Python standard library is used.
"""

from __future__ import annotations

import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

SCHEMA_ID = "flexpart-gpu.run-manifest"
SCHEMA_VERSION = 1
SCHEMA_PATH = "schemas/run-manifest-v1.schema.json"

ATTRIBUTION_VERIFIED = "VERIFIED"
ATTRIBUTION_PARTIAL = "PARTIAL"
ATTRIBUTION_INVALID = "INVALID"

ORACLE_PRISTINE = "pristine-oracle"
ORACLE_SEEDABLE = "seedable-validation-oracle"

ORACLE_PROFILE_ID = "flexpart-11.1-single-thread"
ORACLE_PROFILE_VERSION = 1

ORACLE_STRATEGY_ID = "flexpart-oracle-validation-seed-offset"
ORACLE_STRATEGY_VERSION = 1

CASE_SCHEMA_VERSION = 2

LAYOUT_VERSION = 1

_HEX64 = frozenset("0123456789abcdef")


class ProvenanceError(ValueError):
    """Fail-closed rejection of a provenance record or artifact set."""


def digest(path: Path | str) -> str:
    """Return the SHA-256 hex digest of a file."""
    sha = hashlib.sha256()
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            sha.update(chunk)
    return sha.hexdigest()


def hash_bytes(data: bytes) -> str:
    """Return the SHA-256 hex digest of in-memory bytes."""
    return hashlib.sha256(data).hexdigest()


def canonical_json(value) -> bytes:
    """Encode a value as canonical JSON (sorted keys, compact separators)."""
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      allow_nan=False).encode("utf-8")


def utc_now_iso() -> str:
    """Return an informational UTC timestamp (never an identity input)."""
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace(
        "+00:00", "Z")


def _require_hex64(value: str, field: str) -> str:
    if not isinstance(value, str) or len(value) != 64 or any(
            c not in _HEX64 for c in value):
        raise ProvenanceError(f"{field} must be 64 lowercase hex characters")
    return value


def case_manifest_identity(case_path: Path | str) -> dict:
    """Bind a declared #51 case file to its immutable identity.

    Returns ``case_id``, ``case_manifest_sha256``, ``case_schema_version``
    and the repository-relative-or-absolute ``manifest_path``. The case
    document itself is never reinterpreted here; only its identity and
    schema version are recorded so provenance references #51 instead of
    copying its semantics.
    """
    path = Path(case_path)
    if not path.is_file():
        raise ProvenanceError(f"case manifest missing: {path}")
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ProvenanceError(f"case manifest unreadable: {path}: {exc}") from None
    if not isinstance(document, dict):
        raise ProvenanceError(f"case manifest is not a JSON object: {path}")
    if document.get("schema_version") != CASE_SCHEMA_VERSION or "version" in document:
        raise ProvenanceError(
            f"case {path} is not a canonical v2 document "
            f"(schema_version {CASE_SCHEMA_VERSION} required)")
    case_id = document.get("case_id")
    if not isinstance(case_id, str) or not case_id:
        raise ProvenanceError(f"case {path} lacks a case_id")
    return {
        "case_id": case_id,
        "case_manifest_sha256": digest(path),
        "case_schema_version": CASE_SCHEMA_VERSION,
        "manifest_path": str(path),
    }


def derive_execution_id(binding: dict) -> str:
    """Derive a stable execution identity from its immutable binding.

    The binding must already contain only immutable inputs
    (case identity/hash, role, realization, revisions, executable and
    runtime identities, artifact hashes). Timestamps, pids, absolute
    working directories and other incidental state must never be part
    of the binding; artifact maps use portable case-scoped relative
    keys so the identity is stable across machines.
    """
    if not isinstance(binding, dict) or not binding:
        raise ProvenanceError("execution binding must be a non-empty object")
    return hash_bytes(canonical_json(binding))


def derive_run_id(execution_ids: list[str]) -> str:
    """Derive a deterministic run identity from sorted execution identities."""
    if not execution_ids:
        raise ProvenanceError("a run needs at least one execution identity")
    for value in execution_ids:
        _require_hex64(value, "execution_id")
    return hash_bytes(canonical_json(sorted(execution_ids)))


def short_id(execution_id: str) -> str:
    """Return the 16-character directory-safe prefix of an execution identity."""
    _require_hex64(execution_id, "execution_id")
    return execution_id[:16]


def execution_run_dir(base: Path | str, case_id: str, execution_id: str) -> Path:
    """Return the canonical non-overwriting run directory for one execution.

    Layout: ``<base>/<case_id>/<short-execution-id>/``. The directory name
    carries only the stable execution identity, never timestamps or pids.
    """
    if (not isinstance(case_id, str) or not case_id
            or "/" in case_id or "\\" in case_id
            or any(part in ("", ".", "..") for part in case_id.split("/"))):
        raise ProvenanceError(f"invalid case_id for run directory: {case_id!r}")
    return Path(base) / case_id / short_id(execution_id)


def ensure_non_overwriting_write(path: Path | str, content: bytes) -> None:
    """Write ``content`` to ``path`` without overwriting prior evidence.

    A missing parent is created. An existing file with byte-identical
    content is left untouched (idempotent rerun). An existing file with
    different bytes raises :class:`ProvenanceError` so one execution can
    never silently replace another execution's evidence.
    """
    target = Path(path)
    if target.is_file():
        if target.read_bytes() == content:
            return
        raise ProvenanceError(
            f"refusing to overwrite prior evidence: {target} "
            "(use an execution-identity run directory for the new run)")
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(content)


def _normalize_artifact_map(entries: dict) -> dict:
    """Normalize an artifact map to portable case-scoped relative keys.

    Absolute manifest keys from legacy writers are reduced to portable
    relative keys while preserving one parent scope level (typically the
    case directory) so ``<case-a>/header`` and ``<case-b>/header`` stay
    distinct. Already-relative keys are kept as written. Genuine
    collisions (same portable key, different hashes) raise
    :class:`ProvenanceError` (duplicate artifact identity) instead of
    silently merging.
    """
    normalized: dict[str, str] = {}
    if entries is None:
        entries = {}
    if not isinstance(entries, dict):
        raise ProvenanceError("artifact map must be an object")
    for key, value in entries.items():
        if not isinstance(value, str):
            raise ProvenanceError(
                f"artifact hash for {key!r} must be a hex string, got {type(value).__name__}")
        raw = str(key).replace("\\", "/")
        is_absolute = (Path(raw).is_absolute() or raw.startswith("/")
                       or (len(raw) > 2 and raw[1] == ":"))
        if is_absolute:
            parts = [p for p in raw.split("/") if p not in ("", ".")]
            if len(parts) >= 2:
                name = "/".join(parts[-2:])
            else:
                name = parts[-1] if parts else raw
        else:
            name = raw
        if name in normalized and normalized[name] != value:
            raise ProvenanceError(
                f"duplicate artifact identity: {name!r} maps to two hashes")
        normalized[name] = value
    return normalized


def normalize_artifact_map(entries: dict) -> dict:
    """Return portable artifact identities for writer-supplied path/hash maps."""
    return _normalize_artifact_map(entries)


def build_execution_record(
    *,
    role: str,
    case: dict,
    realization: dict | None = None,
    candidate_revision: str | None = None,
    candidate_executable_sha256: str | None = None,
    oracle_kind: str | None = None,
    oracle_revision: str | None = None,
    oracle_executable_sha256: str | None = None,
    oracle_profile: dict | None = None,
    oracle_strategy: dict | None = None,
    runtime_adapter: str | None = None,
    cpu_runtime: str | None = None,
    inputs_sha256: dict | None = None,
    outputs_sha256: dict | None = None,
) -> dict:
    """Build one execution record with its stable execution identity.

    ``case`` is a :func:`case_manifest_identity` record (referenced, not
    copied). ``realization`` distinguishes ensemble members, e.g.
    ``{"seed_index": 3, "philox_key": [...]}`` for candidates or
    ``{"requested_identity": "3", "repetition": 1}`` for seedable
    oracle runs. Every realization receives its own unambiguous
    ``execution_id`` derived from the full immutable binding.
    """
    if role not in ("candidate", "oracle"):
        raise ProvenanceError(f"unknown execution role: {role!r}")
    if not isinstance(case, dict) or not case.get("case_id"):
        raise ProvenanceError("execution record needs a case identity")
    if oracle_kind is not None and oracle_kind not in (ORACLE_PRISTINE, ORACLE_SEEDABLE):
        raise ProvenanceError(f"unknown oracle kind: {oracle_kind!r}")
    binding = {
        "schema": {"id": SCHEMA_ID, "version": SCHEMA_VERSION},
        "role": role,
        "case_id": case["case_id"],
        "case_manifest_sha256": case["case_manifest_sha256"],
        "case_schema_version": case.get("case_schema_version", CASE_SCHEMA_VERSION),
        "realization": realization or {},
        "candidate_revision": candidate_revision,
        "candidate_executable_sha256": candidate_executable_sha256,
        "oracle_kind": oracle_kind,
        "oracle_revision": oracle_revision,
        "oracle_executable_sha256": oracle_executable_sha256,
        "oracle_profile": oracle_profile,
        "oracle_strategy": oracle_strategy,
        "runtime_adapter": runtime_adapter,
        "cpu_runtime": cpu_runtime,
        "inputs_sha256": _normalize_artifact_map(inputs_sha256 or {}),
        "outputs_sha256": _normalize_artifact_map(outputs_sha256 or {}),
    }
    execution_id = derive_execution_id(binding)
    record = {
        "execution_id": execution_id,
        "role": role,
        "case_id": case["case_id"],
        "case_manifest_sha256": case["case_manifest_sha256"],
        "case_schema_version": case.get("case_schema_version", CASE_SCHEMA_VERSION),
        "realization": realization or {},
        "candidate_revision": candidate_revision,
        "candidate_executable_sha256": candidate_executable_sha256,
        "oracle_kind": oracle_kind,
        "oracle_revision": oracle_revision,
        "oracle_executable_sha256": oracle_executable_sha256,
        "oracle_profile": oracle_profile,
        "oracle_strategy": oracle_strategy,
        "runtime_adapter": runtime_adapter,
        "cpu_runtime": cpu_runtime,
        "inputs_sha256": dict(binding["inputs_sha256"]),
        "outputs_sha256": dict(binding["outputs_sha256"]),
        "attribution": ATTRIBUTION_VERIFIED,
    }
    return record


def _execution_binding(record: dict) -> dict:
    """Reconstruct the immutable binding represented by an execution record."""
    required = (
        "role",
        "case_id",
        "case_manifest_sha256",
        "case_schema_version",
        "realization",
        "candidate_revision",
        "candidate_executable_sha256",
        "oracle_kind",
        "oracle_revision",
        "oracle_executable_sha256",
        "oracle_profile",
        "oracle_strategy",
        "runtime_adapter",
        "cpu_runtime",
        "inputs_sha256",
        "outputs_sha256",
    )
    missing = [field for field in required if field not in record]
    if missing:
        raise ProvenanceError(
            "execution record lacks identity fields: " + ", ".join(missing))
    return {
        "schema": {"id": SCHEMA_ID, "version": SCHEMA_VERSION},
        **{field: record[field] for field in required},
    }


def _verify_execution_identities(executions: list[dict]) -> None:
    """Reject execution IDs that do not bind the record's immutable fields."""
    seen: set[str] = set()
    for record in executions:
        execution_id = record.get("execution_id", "")
        _require_hex64(execution_id, "execution_id")
        if execution_id in seen:
            raise ProvenanceError(
                f"duplicate artifact identity: execution {execution_id}")
        seen.add(execution_id)
        expected = derive_execution_id(_execution_binding(record))
        if execution_id != expected:
            raise ProvenanceError(
                f"execution identity mismatch for {record.get('role')}:"
                f"{record.get('case_id')}: recorded immutable binding does not "
                "derive the recorded execution_id")


def _merged_execution_artifacts(executions: list[dict]) -> tuple[dict, dict]:
    """Return exact merged input/output maps owned by execution records."""
    merged_inputs: dict[str, str] = {}
    merged_outputs: dict[str, str] = {}
    for record in executions:
        for section, merged in (("inputs_sha256", merged_inputs),
                                ("outputs_sha256", merged_outputs)):
            execution_map = record.get(section)
            if not isinstance(execution_map, dict):
                raise ProvenanceError(
                    f"execution {record.get('execution_id', '')[:16]} "
                    f"lacks a {section} object")
            for name, value in execution_map.items():
                _require_hex64(value, f"{section}.{name}")
                if name in merged and merged[name] != value:
                    raise ProvenanceError(
                        f"duplicate artifact identity: {name!r} maps to two hashes")
                merged[name] = value
    return merged_inputs, merged_outputs


def _verify_merged_artifacts(document: dict, executions: list[dict]) -> None:
    """Require global artifact indexes to equal execution-owned artifacts."""
    artifacts = document.get("artifacts")
    if not isinstance(artifacts, dict):
        raise ProvenanceError("run manifest lacks an artifacts object")
    recorded_inputs = artifacts.get("inputs")
    recorded_outputs = artifacts.get("outputs")
    if not isinstance(recorded_inputs, dict) or not isinstance(recorded_outputs, dict):
        raise ProvenanceError("run manifest artifact sections must be objects")
    expected_inputs, expected_outputs = _merged_execution_artifacts(executions)
    if recorded_inputs != expected_inputs:
        raise ProvenanceError(
            "merged input artifact map differs from execution-owned provenance")
    if recorded_outputs != expected_outputs:
        raise ProvenanceError(
            "merged output artifact map differs from execution-owned provenance")


def _verify_summary_bindings(document: dict, executions: list[dict]) -> None:
    """Reject mutable summary fields that disagree with execution bindings."""
    cases = document.get("cases")
    if not isinstance(cases, list) or not cases:
        raise ProvenanceError("run manifest cases must be a non-empty array")
    case_bindings = {
        (case.get("case_id"), case.get("case_manifest_sha256"),
         case.get("case_schema_version"))
        for case in cases if isinstance(case, dict)
    }
    if len(case_bindings) != len(cases):
        raise ProvenanceError("run manifest contains duplicate or invalid case bindings")
    for record in executions:
        binding = (record.get("case_id"), record.get("case_manifest_sha256"),
                   record.get("case_schema_version"))
        if binding not in case_bindings:
            raise ProvenanceError(
                f"execution {record['execution_id'][:16]} disagrees with cases summary")

    candidate = document.get("candidate")
    oracle = document.get("oracle")
    if not isinstance(candidate, dict) or not isinstance(oracle, dict):
        raise ProvenanceError("run manifest candidate/oracle summaries must be objects")
    candidate_records = [record for record in executions
                         if record.get("role") == "candidate"]
    oracle_records = [record for record in executions
                      if record.get("role") == "oracle"]
    for record in candidate_records:
        if record.get("candidate_revision") != candidate.get("revision"):
            raise ProvenanceError(
                "candidate summary revision disagrees with execution binding")
        if (record.get("candidate_executable_sha256") !=
                candidate.get("executable_sha256")):
            raise ProvenanceError(
                "candidate summary executable disagrees with execution binding")
    for record in oracle_records:
        if record.get("oracle_kind") != oracle.get("kind"):
            raise ProvenanceError(
                "oracle summary kind disagrees with execution binding")
        if record.get("oracle_revision") != oracle.get("pinned_commit"):
            raise ProvenanceError(
                "oracle summary revision disagrees with execution binding")
        if (record.get("oracle_executable_sha256") !=
                oracle.get("executable_sha256")):
            raise ProvenanceError(
                "oracle summary executable disagrees with execution binding")
        if record.get("oracle_profile") != oracle.get("execution_profile"):
            raise ProvenanceError(
                "oracle summary profile disagrees with execution binding")


def create_run_manifest(
    *,
    cases: list[dict],
    candidate: dict,
    oracle: dict,
    runtime: dict,
    executions: list[dict],
    base: str = "target/corpus",
    run_dir: str | None = None,
    notes: list[str] | None = None,
) -> dict:
    """Assemble a versioned v1 run manifest from execution records.

    Missing required identity/hash information is recorded
    machine-readably in ``attribution.missing`` and the manifest state
    stays ``PARTIAL``; it is never promoted to ``VERIFIED``. Structural
    contradictions (duplicate execution identities, mixed candidate
    revisions, mixed oracle builds) raise :class:`ProvenanceError`
    immediately so callers fail closed instead of writing bad evidence.
    """
    if not cases:
        raise ProvenanceError("a run manifest needs at least one case identity")
    if not executions:
        raise ProvenanceError("a run manifest needs at least one execution record")

    seen: dict[str, str] = {}
    for record in executions:
        execution_id = record.get("execution_id", "")
        _require_hex64(execution_id, "execution_id")
        label = f"{record.get('role')}:{record.get('case_id')}"
        if execution_id in seen:
            raise ProvenanceError(
                f"duplicate artifact identity: execution {execution_id} "
                f"claimed by {seen[execution_id]} and {label}")
        seen[execution_id] = label

    candidate_revisions = {
        record.get("candidate_revision")
        for record in executions
        if record.get("role") == "candidate"
    } - {None}
    usable_revisions = {r for r in candidate_revisions if r != "unknown"}
    if len(usable_revisions) > 1:
        raise ProvenanceError(
            f"mixed candidate revisions in one run: {sorted(usable_revisions)}")

    oracle_builds = {
        (record.get("oracle_kind"), record.get("oracle_executable_sha256"))
        for record in executions
        if record.get("role") == "oracle"
    } - {(None, None)}
    exe_hashes = {exe for _, exe in oracle_builds if exe}
    if len(exe_hashes) > 1:
        kinds = sorted({kind for kind, _ in oracle_builds if kind})
        raise ProvenanceError(
            f"mixed oracle builds in one run (kinds {kinds}); "
            "one run manifest covers a single oracle build only")

    if oracle.get("kind") not in (ORACLE_PRISTINE, ORACLE_SEEDABLE):
        raise ProvenanceError(
            f"oracle kind must distinguish {ORACLE_PRISTINE} from "
            f"{ORACLE_SEEDABLE}; generic oracle identity is rejected")

    merged_inputs: dict[str, str] = {}
    merged_outputs: dict[str, str] = {}
    for record in executions:
        for section, merged in (("inputs_sha256", merged_inputs),
                                ("outputs_sha256", merged_outputs)):
            for name, value in (record.get(section) or {}).items():
                if name in merged and merged[name] != value:
                    raise ProvenanceError(
                        f"duplicate artifact identity: {name!r} maps to two hashes")
                merged[name] = value

    missing: list[dict] = []
    roles = {record.get("role") for record in executions}
    for role in ("candidate", "oracle"):
        if role not in roles:
            missing.append({
                "name": f"executions.{role}",
                "reason": f"run records no {role} execution; paired attribution is incomplete",
            })
    if not candidate.get("revision") or candidate.get("revision") == "unknown":
        missing.append({
            "name": "candidate.revision",
            "reason": "no candidate revision tied to the executed artifacts",
        })
    if candidate.get("worktree_dirty"):
        missing.append({
            "name": "candidate.worktree_clean",
            "reason": "candidate checkout was dirty at execution time",
        })
    if not candidate.get("executable_sha256"):
        missing.append({
            "name": "candidate.executable_sha256",
            "reason": "no candidate executable hash tied to the executed artifacts",
        })
    if not oracle.get("executable_sha256"):
        missing.append({
            "name": "oracle.executable_sha256",
            "reason": "no oracle executable hash tied to the executed artifacts",
        })
    if oracle.get("worktree_dirty"):
        missing.append({
            "name": "oracle.worktree_clean",
            "reason": "oracle checkout was dirty at execution time",
        })
    if not runtime.get("adapter") and not runtime.get("cpu_runtime"):
        missing.append({
            "name": "runtime.identity",
            "reason": "neither adapter nor CPU runtime identity is recorded",
        })
    for record in executions:
        if not record.get("inputs_sha256"):
            missing.append({
                "name": f"execution.{record['execution_id'][:16]}.inputs",
                "reason": "execution records no input artifact hashes",
            })
        if not record.get("outputs_sha256"):
            missing.append({
                "name": f"execution.{record['execution_id'][:16]}.outputs",
                "reason": "execution records no output artifact hashes",
            })

    run_id = derive_run_id([record["execution_id"] for record in executions])
    resolved_run_dir = run_dir or f"{base}/runs/{run_id[:16]}"
    state = ATTRIBUTION_VERIFIED if not missing else ATTRIBUTION_PARTIAL
    manifest = {
        "schema": {"id": SCHEMA_ID, "version": SCHEMA_VERSION},
        "run_id": run_id,
        "created_utc": utc_now_iso(),
        "cases": cases,
        "candidate": candidate,
        "oracle": oracle,
        "runtime": runtime,
        "executions": executions,
        "artifacts": {"inputs": merged_inputs, "outputs": merged_outputs},
        "attribution": {
            "state": state,
            "missing": missing,
            "notes": list(notes or []),
        },
        "layout": {
            "version": LAYOUT_VERSION,
            "base": base,
            "run_dir": resolved_run_dir,
            "non_overwriting": True,
        },
    }
    return manifest


def write_run_manifest(manifest: dict, path: Path | str) -> Path:
    """Write a v1 manifest without overwriting prior evidence."""
    payload = (json.dumps(manifest, indent=2, sort_keys=False) + "\n").encode("utf-8")
    ensure_non_overwriting_write(path, payload)
    return Path(path)


def is_v1_manifest(document: dict) -> bool:
    """Return True only for an explicit v1 schema envelope."""
    schema = (document or {}).get("schema")
    return (isinstance(schema, dict) and schema.get("id") == SCHEMA_ID
            and schema.get("version") == SCHEMA_VERSION)


def load_manifest(path: Path | str) -> dict:
    """Load a manifest file, failing closed on unreadable content."""
    manifest_path = Path(path)
    if not manifest_path.is_file():
        raise ProvenanceError(f"run manifest missing: {manifest_path}")
    try:
        document = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ProvenanceError(
            f"run manifest unreadable: {manifest_path}: {exc}") from None
    if not isinstance(document, dict):
        raise ProvenanceError(
            f"run manifest is not a JSON object: {manifest_path}")
    return document


def _resolve_artifact_file(name: str, search_roots: list[Path]) -> Path | None:
    candidate = Path(name)
    if candidate.is_file():
        return candidate
    for root in search_roots:
        direct = root / name
        if direct.is_file():
            return direct
        # Case-scoped keys such as "<case>/header" also match when the
        # search root is already the case directory: fall back to the
        # basename in that layout.
        fallback = root / Path(name).name
        if fallback.is_file() and "/" in name.replace("\\", "/"):
            return fallback
        matches = sorted(root.rglob(Path(name).name)) if root.is_dir() else []
        existing = [p for p in matches if p.is_file()]
        # Prefer a suffix match on the full portable key when several
        # same-basename files exist under different cases.
        suffix = name.replace("\\", "/")
        scoped = [p for p in existing
                  if "/".join(p.parts[-2:]).replace("\\", "/") == suffix
                  or p.name == suffix]
        if len(scoped) == 1:
            return scoped[0]
        if len(scoped) > 1:
            raise ProvenanceError(
                f"duplicate artifact identity: {name!r} matches "
                f"{len(scoped)} files under {root}")
        if len(existing) == 1:
            return existing[0]
        if len(existing) > 1:
            raise ProvenanceError(
                f"duplicate artifact identity: {name!r} matches "
                f"{len(existing)} files under {root}")
    return None


def verify_manifest_integrity(document: dict) -> None:
    """Verify a v1 manifest's internal consistency without touching disk.

    Checks the recorded ``run_id`` against the execution identities, the
    merged artifact maps against every execution-recorded hash, mixed
    candidate revisions, mixed oracle builds and an explicit INVALID
    attribution state. Raises :class:`ProvenanceError` on any violation;
    returns None when the manifest is internally consistent.
    """
    if not is_v1_manifest(document):
        raise ProvenanceError(
            f"not a {SCHEMA_ID} v{SCHEMA_VERSION} manifest")
    executions = document.get("executions")
    if not isinstance(executions, list) or not executions:
        raise ProvenanceError("run manifest executions must be a non-empty array")
    for record in executions:
        if not isinstance(record, dict) or not isinstance(record.get("execution_id"), str):
            raise ProvenanceError("run manifest execution records must be objects "
                                  "with an execution_id")
    _verify_execution_identities(executions)
    expected_run = derive_run_id(
        [record["execution_id"] for record in executions])
    if expected_run != document.get("run_id"):
        raise ProvenanceError(
            "stale artifact reused under a new run: run_id does not match "
            "the recorded execution identities")

    _verify_merged_artifacts(document, executions)

    candidate_revisions = {
        record.get("candidate_revision")
        for record in executions
        if record.get("role") == "candidate"
    } - {None}
    if len({r for r in candidate_revisions if r != "unknown"}) > 1:
        raise ProvenanceError(
            f"mismatched candidate revision: {sorted(candidate_revisions)}")
    oracle_builds = {
        (record.get("oracle_kind"), record.get("oracle_executable_sha256"))
        for record in executions
        if record.get("role") == "oracle"
    }
    exe_hashes = {exe for _, exe in oracle_builds if exe}
    if len(exe_hashes) > 1:
        raise ProvenanceError(
            f"mismatched oracle identity/build: {sorted(exe_hashes)}")
    _verify_summary_bindings(document, executions)
    if (document.get("attribution") or {}).get("state") == ATTRIBUTION_INVALID:
        raise ProvenanceError("run manifest is explicitly marked INVALID")


def verify_run_manifest(
    manifest_path: Path | str,
    *,
    search_roots: list[Path | str] | None = None,
) -> dict:
    """Verify every consumed artifact against a v1 manifest before use.

    Returns ``{"state", "verified", "missing", "notes"}``. ``VERIFIED``
    is returned only when every recorded input and output hash matches
    the bytes on disk with complete coverage. ``PARTIAL`` reports
    machine-readable gaps. Any substitution, mutation, mixed revision /
    mixed build, stale reuse, missing artifact, duplicate identity or
    hash mismatch raises :class:`ProvenanceError` (``INVALID``); callers
    must treat that as fail-closed and never compute a scientific
    verdict from the artifacts.
    """
    document = load_manifest(manifest_path)
    if not is_v1_manifest(document):
        raise ProvenanceError(
            f"{manifest_path} is not a {SCHEMA_ID} v{SCHEMA_VERSION} manifest; "
            "legacy manifests are never silently reinterpreted "
            "(migrate them explicitly first)")

    roots = [Path(r) for r in (search_roots or [])]
    manifest_dir = Path(manifest_path).parent
    if manifest_dir not in roots:
        roots.append(manifest_dir)

    for field in ("run_id", "cases", "candidate", "oracle", "runtime",
                  "executions", "artifacts", "attribution", "layout"):
        if field not in document:
            raise ProvenanceError(f"run manifest lacks required field: {field}")
    _require_hex64(document["run_id"], "run_id")

    executions = document["executions"]
    if not isinstance(executions, list) or not executions:
        raise ProvenanceError("run manifest executions must be a non-empty array")
    for record in executions:
        if not isinstance(record, dict) or not isinstance(record.get("execution_id"), str):
            raise ProvenanceError("run manifest execution records must be objects "
                                  "with an execution_id")

    verify_manifest_integrity(document)

    verified: list[str] = []
    for section in ("inputs", "outputs"):
        for name, expected in document["artifacts"][section].items():
            _require_hex64(expected, f"artifacts.{section}.{name}")
            found = _resolve_artifact_file(name, roots)
            if found is None:
                raise ProvenanceError(
                    f"missing required artifact: {name!r} "
                    f"(expected sha256 {expected})")
            actual = digest(found)
            if actual != expected:
                kind = "output substituted after manifest creation" if section == "outputs" \
                    else "changed input after manifest creation"
                raise ProvenanceError(
                    f"artifact hash mismatch ({kind}): {name!r} "
                    f"expected {expected}, found {actual}")
            verified.append(f"{section}/{name}")

    candidate_revisions = {
        record.get("candidate_revision")
        for record in document["executions"]
        if record.get("role") == "candidate"
    } - {None}
    if len({r for r in candidate_revisions if r != "unknown"}) > 1:
        raise ProvenanceError(
            f"mismatched candidate revision: {sorted(candidate_revisions)}")
    oracle_builds = {
        (record.get("oracle_kind"), record.get("oracle_executable_sha256"))
        for record in document["executions"]
        if record.get("role") == "oracle"
    }
    exe_hashes = {exe for _, exe in oracle_builds if exe}
    if len(exe_hashes) > 1:
        raise ProvenanceError(
            f"mismatched oracle identity/build: {sorted(exe_hashes)}")

    if document["attribution"].get("state") == ATTRIBUTION_INVALID:
        raise ProvenanceError("run manifest is explicitly marked INVALID")

    missing = list(document["attribution"].get("missing", []))
    notes = list(document["attribution"].get("notes", []))
    if missing:
        return {"state": ATTRIBUTION_PARTIAL, "verified": verified,
                "missing": missing, "notes": notes}
    if document["attribution"].get("state") != ATTRIBUTION_VERIFIED:
        return {"state": ATTRIBUTION_PARTIAL, "verified": verified,
                "missing": [{"name": "attribution.state",
                             "reason": "manifest does not claim VERIFIED attribution"}],
                "notes": notes}
    return {"state": ATTRIBUTION_VERIFIED, "verified": verified,
            "missing": [], "notes": notes}


def verify_artifact_set(
    manifest_path: Path | str,
    labeled_paths: list[tuple[str, Path | str] | tuple[str, Path | str, str]],
    *,
    search_roots: list[Path | str] | None = None,
) -> dict:
    """Verify an explicit consumed-artifact set against a v1 manifest.

    ``labeled_paths`` holds ``(label, path)`` or ``(label, path, role)``
    tuples actually consumed by a report. When a role is supplied, the
    artifact must be owned by an execution of that role; an oracle output
    cannot establish candidate coverage or vice versa. Every consumed file
    must be covered by the manifest with a matching hash; uncovered files
    are reported machine-readably and the result stays ``PARTIAL``. Hash mismatches raise
    :class:`ProvenanceError`. Reports must call this before calculating
    any verdict.
    """
    document = load_manifest(manifest_path)
    if not is_v1_manifest(document):
        raise ProvenanceError(
            f"{manifest_path} is not a {SCHEMA_ID} v{SCHEMA_VERSION} manifest")
    verify_manifest_integrity(document)
    recorded: dict[str, str] = {}
    for section in ("inputs", "outputs"):
        recorded.update(document["artifacts"].get(section, {}))
    outputs_by_role: dict[str, dict[str, str]] = {}
    for record in document["executions"]:
        role_outputs = outputs_by_role.setdefault(record["role"], {})
        for name, value in record["outputs_sha256"].items():
            if name in role_outputs and role_outputs[name] != value:
                raise ProvenanceError(
                    f"duplicate artifact identity for role {record['role']}: {name!r}")
            role_outputs[name] = value
    roots = [Path(r) for r in (search_roots or [])]
    verified: list[str] = []
    uncovered: list[dict] = []
    for item in labeled_paths:
        if len(item) == 2:
            label, path = item
            role = None
        elif len(item) == 3:
            label, path, role = item
            if role not in ("candidate", "oracle"):
                raise ProvenanceError(f"unknown consumed-artifact role: {role!r}")
        else:
            raise ProvenanceError("consumed artifact tuple must have 2 or 3 items")
        actual = digest(path)
        eligible = outputs_by_role.get(role, {}) if role else recorded
        expected = _resolve_consumed_artifact(eligible, Path(path), roots)
        if expected is None:
            uncovered.append({"name": f"provenance.{label}",
                              "reason": "artifact is not covered by the run manifest"})
        elif expected != actual:
            raise ProvenanceError(
                f"artifact {label} hash differs from run manifest "
                f"(expected {expected}, found {actual})")
        else:
            verified.append(label)
    if uncovered:
        return {"state": ATTRIBUTION_PARTIAL, "verified": verified,
                "missing": uncovered,
                "notes": [f"{len(uncovered)} consumed artifacts lack manifest coverage"]}
    return {"state": ATTRIBUTION_VERIFIED, "verified": verified,
            "missing": [], "notes": [f"{len(verified)} consumed artifacts hash-verified"]}


def _normalize_key(key: str) -> str:
    return Path(key).name or str(key)


def _consumed_suffixes(path: Path, search_roots: list[Path]) -> list[str]:
    """Return portable lookup suffixes for a consumed filesystem path.

    Order is most-specific first: search-root-relative paths, then the
    case-scoped two-part suffix (``<case>/<basename>``), then the bare
    basename. Callers try each against the recorded portable keys so a
    case-scoped path disambiguates multi-case runs instead of colliding
    on the basename.
    """
    suffixes: list[str] = []
    try:
        resolved = path.resolve()
    except OSError:
        resolved = path
    for root in search_roots:
        try:
            relative = resolved.relative_to(root.resolve())
        except (OSError, ValueError):
            continue
        relative_posix = relative.as_posix()
        if relative_posix and relative_posix not in suffixes:
            suffixes.append(relative_posix)
    parts = resolved.as_posix().replace("\\", "/").split("/")
    parts = [p for p in parts if p not in ("", ".")]
    if len(parts) >= 2:
        two_part = "/".join(parts[-2:])
        if two_part not in suffixes:
            suffixes.append(two_part)
    basename = resolved.name
    if basename and basename not in suffixes:
        suffixes.append(basename)
    return suffixes


def _resolve_consumed_artifact(
    recorded: dict[str, str], path: Path, search_roots: list[Path]
) -> str | None:
    """Resolve one consumed file to its expected manifest hash.

    Exact absolute-path keys win first (legacy writers). Otherwise the
    most-specific portable suffix wins, so ``.../case-a/header``
    matches ``case-a/header`` even when ``case-b/header`` with
    different bytes exists. A bare basename shared by several entries
    with different hashes and no disambiguating scope raises
    :class:`ProvenanceError` (fail-closed); a basename with no entry
    returns None (uncovered, machine-readable PARTIAL).
    """
    try:
        resolved = str(path.resolve())
    except OSError:
        resolved = str(path)
    if resolved in recorded:
        return recorded[resolved]
    suffixes = _consumed_suffixes(path, search_roots)
    for suffix in suffixes:
        if suffix in recorded:
            return recorded[suffix]
    basename = Path(resolved).name or _normalize_key(resolved)
    same_name = [(key, value) for key, value in recorded.items()
                 if _normalize_key(key) == basename]
    if not same_name:
        return None
    hashes = {value for _, value in same_name}
    if len(hashes) == 1:
        return same_name[0][1]
    raise ProvenanceError(
        f"duplicate artifact identity: {basename!r} maps to "
        f"{len(same_name)} hashes in the run manifest and "
        f"{resolved!r} carries no disambiguating case scope")


def migrate_legacy_manifest(document: dict, *, source: str = "legacy") -> dict:
    """Deterministically upgrade a legacy manifest envelope to v1 shape.

    Legacy corpus/oracle manifests carry no ``schema`` envelope, use
    absolute-path artifact keys and have no execution identities. This
    helper refuses to guess: it requires the caller to supply the case
    identities, oracle kind and runtime identity explicitly (see
    ``scripts/corpus/write_corpus_manifest.py`` and
    ``scripts/write_oracle_run_manifest.py`` which now emit v1
    directly). Direct silent reinterpretation of an old schema version
    is rejected; use the writers to regenerate instead.
    """
    raise ProvenanceError(
        f"legacy manifest from {source} has no {SCHEMA_ID} v{SCHEMA_VERSION} "
        "envelope and cannot be silently reinterpreted; regenerate it with "
        "the v1 writers instead of migrating bytes blindly")
