#!/usr/bin/env python3
"""Stochastic identity contract for the FLEXPART 11.1 oracle (issue #50).

Implements the versioned ``flexpart-oracle-validation-seed-offset`` strategy:
a canonical integer identity ``S`` in ``[1, 1000000000]`` selects the initial
RNG state of a separately built ``seedable-validation-oracle`` executable
through the environment variable ``FLEXPART_VALIDATION_SEED``. An unset or
empty variable keeps the pristine default initialization bit-exactly.

Everything here uses the Python standard library only. All validation entry
points fail closed with :class:`StochasticIdentityError`; no code path falls
back silently from a requested seed to the FLEXPART default seed.

RNG namespaces: a candidate Philox seed ``N`` and an oracle identity ``N``
are different generators and must never be equated numerically.
"""

import hashlib
import json
from pathlib import Path

STRATEGY = "flexpart-oracle-validation-seed-offset"
STRATEGY_VERSION = 1
SEED_ENV_VAR = "FLEXPART_VALIDATION_SEED"
IDENTITY_MIN = 1
IDENTITY_MAX = 1000000000

PRISTINE_ORACLE = "pristine-oracle"
SEEDABLE_ORACLE = "seedable-validation-oracle"

PROFILE_ID = "flexpart-11.1-single-thread"
PROFILE_VERSION = 1

# Pristine hard-coded initialization (pinned FLEXPART 11.1 sources).
PRISTINE_ISEED1_BASE = 7
PRISTINE_ISEED2_BASE = 88
PRISTINE_RANNUMB_IDUMMY = -320
PRISTINE_RANNUMB_IDUMMY_ABS = 320

# Files the validation-only patch is allowed to touch (relative to the
# FLEXPART checkout root). Anything else is a scope violation.
PATCH_ALLOWED_FILES = frozenset({"src/random_mod.f90", "src/FLEXPART.f90"})

# Non-blank lines the patch is allowed to remove. The patch may only restate
# the two pristine seed assignments with the validation offset applied.
PATCH_ALLOWED_REMOVED_CODE = frozenset({
    "iseed1(i) = -7-i",
    "iseed2(i) = -88-i",
})

# Tripwire substrings for added code lines (comments excluded). This is a
# guardrail, not a proof: the patch SHA-256 in the versioned contract pins
# the exact content. Every state-touching line must mention the validation
# offset or the seed variables it shifts.
PATCH_ADDED_CODE_MARKERS = (
    "validation_seed",
    "validation_offset",
    "iseed1",
    "iseed2",
    "idummy",
    "error stop",
    "offset",
    "implicit none",
    "integer",
    "character",
    "get_environment",
    "adjustl",
    "len_trim",
    "value_length",
    "digits",
    "read(",
    "return",
    "end function",
    "do k",
    "end do",
    "if (",
)


class StochasticIdentityError(ValueError):
    """Fail-closed rejection of a stochastic identity, record, or artifact."""


def parse_requested_identity(raw) -> int:
    """Parse a canonical requested identity to int, rejecting anything else.

    Only plain decimal spellings of integers in ``[1, 1000000000]`` are
    accepted. ``0``, negative values, out-of-range values, blanks, signs,
    decimal points, surrounding whitespace, leading zeros, and non-numeric
    text are all rejected so a missing or ambiguous identity can never
    become a run.
    """
    if raw is None:
        raise StochasticIdentityError(
            "missing stochastic identity: an independent realization was "
            "requested but no identity was provided"
        )
    if isinstance(raw, bool):
        raise StochasticIdentityError(
            f"unsupported stochastic identity: {raw!r}")
    if isinstance(raw, int):
        identity = raw
    elif isinstance(raw, str):
        if not raw:
            raise StochasticIdentityError(
                f"unsupported stochastic identity: {raw!r}; expected a "
                f"canonical decimal integer in [{IDENTITY_MIN}, {IDENTITY_MAX}]"
            )
        if any(c < "0" or c > "9" for c in raw):
            raise StochasticIdentityError(
                f"unsupported stochastic identity: {raw!r}; expected a "
                f"canonical decimal integer in [{IDENTITY_MIN}, {IDENTITY_MAX}]"
            )
        # Reject leading zeros (canonical decimal representation)
        if len(raw) > 1 and raw[0] == "0":
            raise StochasticIdentityError(
                f"unsupported stochastic identity: {raw!r}; leading zeros not allowed"
            )
        try:
            identity = int(raw, 10)
        except ValueError:
            raise StochasticIdentityError(
                f"unsupported stochastic identity: {raw!r}") from None
    else:
        raise StochasticIdentityError(
            f"unsupported stochastic identity: {raw!r}")
    if identity < IDENTITY_MIN or identity > IDENTITY_MAX:
        raise StochasticIdentityError(
            f"stochastic identity {identity} out of supported range "
            f"[{IDENTITY_MIN}, {IDENTITY_MAX}]"
        )
    return identity


def derive_state(identity: int, num_threads: int = 1) -> dict:
    """Map a requested identity to FLEXPART initialization state.

    Mirrors ``validation_seed_offset()`` in the versioned patch exactly::

        iseed1(i) = -(7 + i + S)
        iseed2(i) = -(88 + i + S)
        rannumb_table_idummy = -(320 + S)
    """
    identity = parse_requested_identity(identity)
    if num_threads < 1:
        raise StochasticIdentityError(
            f"unsupported thread count for seed derivation: {num_threads}")
    return {
        "requested_identity": identity,
        "offset": identity,
        "iseed1": [-(PRISTINE_ISEED1_BASE + i + identity) for i in range(num_threads)],
        "iseed2": [-(PRISTINE_ISEED2_BASE + i + identity) for i in range(num_threads)],
        "rannumb_table_idummy": -(PRISTINE_RANNUMB_IDUMMY_ABS + identity),
    }


def default_state(num_threads: int = 1) -> dict:
    """Pristine default initialization state (offset 0, never a requested identity)."""
    if num_threads < 1:
        raise StochasticIdentityError(
            f"unsupported thread count for seed derivation: {num_threads}")
    return {
        "requested_identity": None,
        "offset": 0,
        "iseed1": [-(PRISTINE_ISEED1_BASE + i) for i in range(num_threads)],
        "iseed2": [-(PRISTINE_ISEED2_BASE + i) for i in range(num_threads)],
        "rannumb_table_idummy": PRISTINE_RANNUMB_IDUMMY,
    }


def digest(path: Path) -> str:
    """SHA-256 of a file."""
    sha = hashlib.sha256()
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            sha.update(chunk)
    return sha.hexdigest()


def load_contract(contract_path: Path) -> dict:
    """Load the versioned stochastic identity contract, failing closed."""
    try:
        contract = json.loads(Path(contract_path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise StochasticIdentityError(
            f"cannot load stochastic identity contract {contract_path}: {exc}") from None
    if contract.get("strategy") != STRATEGY or contract.get("version") != STRATEGY_VERSION:
        raise StochasticIdentityError(
            "unsupported stochastic identity strategy "
            f"{contract.get('strategy')!r} version {contract.get('version')!r}; "
            f"this tooling implements {STRATEGY!r} version {STRATEGY_VERSION}"
        )
    canonical = contract.get("canonical_identity", {})
    if canonical.get("minimum") != IDENTITY_MIN or canonical.get("maximum") != IDENTITY_MAX:
        raise StochasticIdentityError(
            "contract canonical identity range does not match this tooling")
    profile = contract.get("execution_profile", {})
    if profile.get("id") != PROFILE_ID or profile.get("version") != PROFILE_VERSION:
        raise StochasticIdentityError(
            "contract execution profile does not reference the frozen "
            f"{PROFILE_ID} version {PROFILE_VERSION} profile")
    return contract


def verify_patch_artifact(contract: dict, repo_root: Path) -> dict:
    """Verify the versioned patch artifact against the contract.

    Checks the recorded SHA-256, the touched-file set, and the initialization-only
    scope tripwires. Returns provenance details for run records and reports.
    """
    patch_info = contract.get("validation_patch", {})
    patch_path = Path(repo_root) / patch_info.get("file", "")
    if not patch_path.is_file():
        raise StochasticIdentityError(
            f"validation patch artifact missing: {patch_path}")
    actual_sha = digest(patch_path)
    if actual_sha != patch_info.get("sha256"):
        raise StochasticIdentityError(
            f"validation patch SHA-256 mismatch: expected "
            f"{patch_info.get('sha256')}, found {actual_sha}")
    if set(patch_info.get("touched_files", [])) != set(PATCH_ALLOWED_FILES):
        raise StochasticIdentityError(
            "contract patch file set does not match the initialization-only scope")
    text = patch_path.read_text(encoding="utf-8")
    touched, removed, added = set(), set(), []
    for line in text.splitlines():
        if line.startswith("--- a/"):
            continue
        if line.startswith("+++ b/"):
            touched.add(line[len("+++ b/"):])
        elif line.startswith("--- "):
            raise StochasticIdentityError("unexpected diff header in patch artifact")
        elif line.startswith("-") and not line.startswith("---"):
            stripped = line[1:].strip()
            if stripped:
                removed.add(stripped)
        elif line.startswith("+") and not line.startswith("+++"):
            code = line[1:].split("!", 1)[0].strip()
            if code:
                added.append(code)
    if touched != set(PATCH_ALLOWED_FILES):
        raise StochasticIdentityError(
            f"patch touches {sorted(touched)}; only {sorted(PATCH_ALLOWED_FILES)} "
            "are within the initialization-only scope")
    if removed != set(PATCH_ALLOWED_REMOVED_CODE):
        raise StochasticIdentityError(
            f"patch removes {sorted(removed)}; only the two pristine seed "
            "assignments may be restated")
    for code in added:
        if not any(marker in code for marker in PATCH_ADDED_CODE_MARKERS):
            raise StochasticIdentityError(
                f"patch adds out-of-scope code line: {code!r}")
    return {
        "file": patch_info.get("file"),
        "sha256": actual_sha,
        "touched_files": sorted(touched),
    }


def build_identity_record(*, requested_env_value, oracle_kind: str,
                          executable_sha256: str, patch_sha256: str,
                          case: str, execution_profile: dict) -> dict:
    """Build the machine-readable identity record stored with every run.

    ``requested_env_value`` is the exact ``FLEXPART_VALIDATION_SEED`` string
    passed to the run, or ``None`` for default (unset) mode.
    """
    if oracle_kind not in (PRISTINE_ORACLE, SEEDABLE_ORACLE):
        raise StochasticIdentityError(f"unknown oracle kind: {oracle_kind!r}")
    if requested_env_value is None:
        state = default_state()
        record = {
            "strategy": STRATEGY,
            "strategy_version": STRATEGY_VERSION,
            "oracle_kind": oracle_kind,
            "requested_env_value": None,
            "requested_identity": None,
            "default_mode": True,
            "derived_state": state,
            "executable_sha256": executable_sha256,
            "patch_sha256": patch_sha256 if oracle_kind == SEEDABLE_ORACLE else None,
            "case": case,
            "execution_profile": execution_profile,
        }
    else:
        identity = parse_requested_identity(requested_env_value)
        if oracle_kind != SEEDABLE_ORACLE:
            raise StochasticIdentityError(
                f"a requested stochastic identity ({identity}) cannot be served by "
                f"{oracle_kind}: independent realizations require the "
                f"{SEEDABLE_ORACLE} executable")
        state = derive_state(identity)
        if state["iseed1"] == default_state()["iseed1"]:
            raise StochasticIdentityError(
                "silent fallback: requested identity reproduces the pristine "
                "default state")
        record = {
            "strategy": STRATEGY,
            "strategy_version": STRATEGY_VERSION,
            "oracle_kind": oracle_kind,
            "requested_env_value": requested_env_value if isinstance(
                requested_env_value, str) else str(requested_env_value),
            "requested_identity": identity,
            "default_mode": False,
            "derived_state": state,
            "executable_sha256": executable_sha256,
            "patch_sha256": patch_sha256,
            "case": case,
            "execution_profile": execution_profile,
        }
    return record


def validate_run_record(record: dict, *, contract: dict,
                        seedable_executable_sha256: str,
                        pristine_executable_sha256: str) -> dict:
    """Fail-closed validation of one stored identity record.

    Rejects unknown oracle kinds, mislabeled executables, unprovable seeds,
    unknown builds, and silent fallbacks. Returns the applied state on success.
    """
    if record.get("strategy") != STRATEGY or record.get("strategy_version") != STRATEGY_VERSION:
        raise StochasticIdentityError("identity record uses an unsupported strategy version")
    profile = record.get("execution_profile", {})
    if profile != {"id": PROFILE_ID, "version": PROFILE_VERSION}:
        raise StochasticIdentityError(
            f"identity record execution profile {profile} does not match the "
            f"frozen {PROFILE_ID} version {PROFILE_VERSION} profile")
    if record.get("patch_sha256") and record["patch_sha256"] != contract["validation_patch"]["sha256"]:
        raise StochasticIdentityError("identity record cites an unknown patch build")
    kind = record.get("oracle_kind")
    exe = record.get("executable_sha256")
    if kind not in (PRISTINE_ORACLE, SEEDABLE_ORACLE):
        raise StochasticIdentityError(f"unknown oracle kind: {kind!r}")
    if exe not in (seedable_executable_sha256, pristine_executable_sha256):
        raise StochasticIdentityError(
            "identity record ties to an executable of unknown build identity")
    if seedable_executable_sha256 == pristine_executable_sha256:
        raise StochasticIdentityError(
            "seedable and pristine executables are identical; the validation "
            "instrument is not distinguished from the normative oracle")
    if kind == PRISTINE_ORACLE and exe != pristine_executable_sha256:
        raise StochasticIdentityError(
            "provenance labels a patched executable as the pristine oracle")
    if kind == SEEDABLE_ORACLE and record.get("requested_identity") is not None \
            and exe != seedable_executable_sha256:
        raise StochasticIdentityError(
            "seeded run does not tie to the seedable validation executable")
    if record.get("default_mode"):
        if record.get("requested_identity") is not None or record.get("requested_env_value") is not None:
            raise StochasticIdentityError(
                "ambiguous identity: default mode must not carry a requested identity")
        expected = default_state()
        if record.get("derived_state") != expected:
            raise StochasticIdentityError(
                "cannot prove which seed/state a default-mode run applied")
        return expected
    identity = parse_requested_identity(record.get("requested_identity"))
    env_value = record.get("requested_env_value")
    if env_value is None or parse_requested_identity(env_value) != identity:
        raise StochasticIdentityError(
            "cannot prove which seed/state was actually applied: requested "
            "identity and recorded environment value disagree")
    expected = derive_state(identity)
    if record.get("derived_state") != expected:
        raise StochasticIdentityError(
            "cannot prove which seed/state was actually applied: recorded "
            "derivation does not match the canonical mapping")
    if expected["iseed1"] == default_state()["iseed1"]:
        raise StochasticIdentityError(
            "silent fallback from requested seed to FLEXPART default seed")
    return expected


# --- Algorithm-level distinctness model ------------------------------------
# Exact integer replica of the Numerical Recipes ran3 initialization in
# random_mod.f90 (mseed/mbig constants, 55-entry table, four warmup passes).
# Used to justify that distinct requested identities initialize distinct RNG
# states according to the traced algorithm. It models initialization only,
# not realized trajectories; trajectory distinctness is proven empirically.

_RAN3_MBIG = 1000000000
_RAN3_MSEED = 161803398


def ran3_initial_table(idum: int) -> list:
    """Replicate the ran3 ma(55) table after initialization for seed ``idum``."""
    if not isinstance(idum, int) or isinstance(idum, bool) or idum >= 0:
        raise StochasticIdentityError(
            f"ran3 initialization requires a negative integer seed, got {idum!r}")
    table = [0] * 55
    mj = (_RAN3_MSEED - abs(idum)) % _RAN3_MBIG
    table[54] = mj
    mk = 1
    for i in range(1, 55):
        # Fortran ii=mod(21*i,55) is in 1..54 here (21 and 55 are coprime and
        # i never reaches 55), so the 0-based index ii-1 is always in range.
        ii = (21 * i) % 55
        table[ii - 1] = mk
        mk = mj - mk
        if mk < 0:
            mk += _RAN3_MBIG
        mj = table[ii - 1]
    for _ in range(4):
        for i in range(1, 56):
            value = table[i - 1] - table[(i + 30) % 55]
            if value < 0:
                value += _RAN3_MBIG
            table[i - 1] = value
    return table


def initial_tables_distinct(requested_identities) -> dict:
    """Prove distinct initialization states for requested identities.

    Covers both seed families driven by the mapping (the iseed1 index stream
    and the rannumb-table idummy stream) plus the pristine default. Returns
    the evidence payload embedded in validation reports.
    """
    identities = [parse_requested_identity(s) for s in requested_identities]
    if len(set(identities)) != len(identities):
        raise StochasticIdentityError("requested identities are not unique")
    index_tables = {}
    table_tables = {}
    for identity in [None] + identities:
        state = default_state() if identity is None else derive_state(identity)
        label = "default" if identity is None else str(identity)
        index_tables[label] = ran3_initial_table(state["iseed1"][0])
        table_tables[label] = ran3_initial_table(state["rannumb_table_idummy"])
    all_tables = [(f"iseed1:{k}", v) for k, v in index_tables.items()]
    all_tables += [(f"idummy:{k}", v) for k, v in table_tables.items()]
    seen = {}
    for label, table in all_tables:
        key = tuple(table)
        if key in seen:
            raise StochasticIdentityError(
                f"initialization states collide: {label} matches {seen[key]}")
        seen[key] = label
    digests = {}
    for label, table in all_tables:
        digest_value = hashlib.sha256(
            ",".join(str(v) for v in table).encode("ascii")).hexdigest()
        digests[label] = digest_value
    return {
        "method": "exact integer replica of random_mod.f90 ran3 initialization "
                  "(mseed=161803398, mbig=1000000000, 55-entry table, four warmup passes)",
        "families": ["iseed1 thread-0 index stream", "rannumb-table idummy stream"],
        "identities": [str(s) for s in identities],
        "pairwise_distinct": True,
        "table_digests": digests,
    }
