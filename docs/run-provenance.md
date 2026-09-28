# Unified validation run provenance (issue #53)

Authoritative path binding every validation result to the exact execution
and immutable inputs/outputs that produced it:

`declared case/profile (#51) -> concrete execution instance -> immutable input/output bytes`

## Contract

- Schema: `schemas/run-manifest-v1.schema.json`
- Schema id: `flexpart-gpu.run-manifest`, version `1`.
- Library: `scripts/provenance/run_provenance.py` (standard library only).
- Regression: `scripts/provenance/test_run_provenance.py`.

The v1 envelope extends the existing #6 run-manifest/hash infrastructure
(`scripts/corpus/write_corpus_manifest.py`,
`scripts/write_oracle_run_manifest.py`). Legacy top-level keys are
preserved byte-for-byte for compatibility; the v1 keys (`schema`,
`run_id`, `cases`, `candidate`, `oracle`, `runtime`, `executions`,
`artifacts`, `attribution`, `layout`) are the authoritative path. No
second provenance family exists: both writers delegate hashing,
execution-identity derivation and verification to the library.

Legacy manifests without a v1 `schema` envelope are never silently
reinterpreted. `load_manifest` + `is_v1_manifest` dispatch explicitly;
`migrate_legacy_manifest` refuses blind byte migration. Regenerate with
the v1 writers instead.

## Execution identity

Each concrete candidate/oracle execution — and, for ensembles, each
realization — receives its own stable identity:

`execution_id = sha256(canonical_json(binding))`

The binding contains only immutable inputs: schema id/version, role
(`candidate`/`oracle`), case id, case-manifest SHA-256 and schema
version, realization (e.g. `seed_index` + Philox key/counter for
candidates, `requested_identity` + repetition for seedable oracle
runs), candidate revision, candidate/oracle executable hashes, oracle
kind/profile/strategy, runtime/adapter identity and the normalized
input/output artifact maps.

`run_id = sha256(canonical_json(sorted(execution_ids)))`.

Timestamps, pids, absolute working directories and other incidental
state are never identity inputs (`created_utc` is informational only).
`execution_run_dir(base, case_id, execution_id)` maps an identity to
`<base>/<case_id>/<short-id>/` where `short-id` is the first 16 hex
characters.

## Artifact layout

- Portable artifact keys are case-scoped relative paths
  (`<case_id>/<basename>`), never absolute paths and never bare
  basenames that collide across cases.
- `artifacts.inputs` / `artifacts.outputs` merge every execution's maps.
  A shared key with two hashes is a duplicate-identity error.
- Writers use `ensure_non_overwriting_write`: byte-identical reruns are
  idempotent; a new run with a different `run_id` refuses to overwrite
  an existing manifest (`--output` must point at a new
  execution-identity directory).
- `verify_run_manifest` resolves portable keys against caller-supplied
  `search_roots` (candidate, oracle, meteo, case-fixture directories).
  Reports must additionally call `verify_artifact_set` with the exact
  consumed `(label, path)` list before calculating any verdict.

## Attribution states

- `VERIFIED`: every recorded input/output hash matches the bytes on
  disk with complete coverage. Only this state may back a scientific
  verdict.
- `PARTIAL`: machine-readable gaps in `attribution.missing` (e.g. no
  candidate revision, no executable hash, no runtime identity, an
  execution without input/output hashes, a consumed artifact without
  manifest coverage, a placeholder case binding). Never promoted to
  verified.
- `INVALID`: raised as `ProvenanceError` (fail-closed, never a
  verdict). Triggered by output substitution after manifest creation,
  changed inputs, mismatched candidate revisions, mismatched oracle
  identity/build, stale reuse under a new run (`run_id` mismatch),
  missing artifacts, duplicate artifact identities and hash mismatches.

## Oracle distinction

`oracle.kind` is `pristine-oracle` or `seedable-validation-oracle`,
reusing the #50 contract vocabulary (`reference/oracle-stochastic-identity.json`,
`scripts/corpus/oracle_stochastic_identity.py`). The two are never
collapsed: seedable runs additionally carry the `strategy`
(`flexpart-oracle-validation-seed-offset` v1) and the requested
identity; pristine runs must not carry one. The RNG-state mapping
itself stays owned by #50; provenance only records the identity.

## Case binding (#51)

`cases[]` references the canonical #51 case files (`case_id`,
`case_manifest_sha256`, `case_schema_version: 2`, `manifest_path`).
Case semantics, execution-profile references, stochastic intent and
expected artifact classes are referenced, never copied into a second
schema. Without `--case-manifest` (oracle writer) or `--cases-dir`
(corpus writer) the binding stays an explicit `PARTIAL` gap.

## Verification

Focused provenance/hash checks plus the smallest representative
workflow — no expensive oracle/model reruns:

- `python scripts/provenance/test_run_provenance.py` (16 tests: valid
  verify, output substitution, input mutation, mixed revisions/builds,
  stale/missing/duplicate rejection, oracle-kind distinction, identity
  determinism).
- `python scripts/evaluate/test_evaluate.py` (evaluator, incl. legacy
  provenance paths).
- `python scripts/test_oracle_run_manifest.py` (oracle writer).
- Synthetic end-to-end writer check (see PR): focused corpus manifest
  for `ADV-ANA-001` verifies `VERIFIED`, then fails closed after output
  mutation.

`scripts/evaluate/evaluate_case.py` consumes v1 manifests through
`run_provenance.verify_artifact_set` (dispatched by explicit version
check; legacy manifests keep the legacy path). Reports verify consumed
artifacts before use; partial attribution is never promoted.

## Non-scope (owned elsewhere)

- #52: `INPUT_EQUIVALENT`, representation comparison, unit/sign
  semantics, meteorology-equivalence logic — never implemented here.
- #51: case semantics, execution-profile references, stochastic
  intent, expected artifact classes — referenced only.
- Scientific metrics/thresholds, ensemble aggregation and oracle
  execution redesign are out of scope.
