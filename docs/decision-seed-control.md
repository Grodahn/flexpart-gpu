# Decision Record: Native vs Validation-Patch Seed Control (Issue #50)

## Context

Issue #50 requires establishing a scientifically defensible stochastic identity
strategy for independent FLEXPART 11.1 oracle realizations. The first mandatory
step is to exhaust native seed-control options before any patching.

## Investigation Summary

**Pinned revision:** `c70586c2b7f5258850705325881c61f557ea9bd8` (FLEXPART 11.1 v11.1 tag)

**RNG subsystem:** `src/random_mod.f90` — Numerical Recipes `ran3` (Knuth subtractive),
`ran1` (Park-Miller + Bays-Durham), `gasdev`/`gasdev1` (Box-Muller).

**Initialization:** `alloc_random(num_threads)` called once at startup
(`FLEXPART.f90:227`). Hard-coded per-thread seeds:
- `iseed1(i) = -7 - i`
- `iseed2(i) = -88 - i`

**Consumption:** Three distinct RNG families drive the forward path:
1. `ran3`/`gasdev` → `rannumb` table + CBL/vertical turbulence (per thread)
2. `ran1` (thread 0) → release position jitter (point/domain fill)
3. `ran3` on `iseed2` → convection redistribution (unused in WIND-UNI-002)

**OpenMP:** Single-thread profile (`OMP_NUM_THREADS=1`) freezes all state to thread 0.

## Exhaustive Search for Native Seed Control

| Candidate | Searched | Found | Location |
|-----------|----------|-------|----------|
| COMMAND namelist seed field | Full `readoptions_mod.f90` | ❌ | No seed variable in `/command/`, `/releases/`, `/species_params/`, `/outgrid/`, `/receptors/`, `/partoptions/` namelists |
| Environment variable | All `get_environment_variable` calls | ❌ | Zero occurrences in pinned source tree |
| Command line arguments | `get_command_argument`, `command_argument_count` | ❌ | Not used |
| Fortran 2018 `random_seed`/`random_number` | Intrinsic calls | ❌ | Not used |
| Seed file input | File reads in `readoptions_mod.f90` | ❌ | No seed file referenced |

**No native mechanism exists** to request an independent stochastic identity.

## Decision

**Use a validation-only RNG initialization patch** as the seed control mechanism.

**Rationale:**
1. Native control is completely absent — no input path reaches `alloc_random`.
2. The patch is minimal (two files, ~40 lines added), touching only initialization.
3. RNG algorithms, distributions, call order, OpenMP semantics, and physics are untouched.
4. Default mode (unset `FLEXPART_VALIDATION_SEED`) preserves pristine bit-exact behavior.
5. The patched executable is a **separately identified validation instrument**
   (`seedable-validation-oracle`), never the normative `pristine-oracle`.
6. Fail-closed: invalid seeds abort with actionable error; no silent fallback.
7. Distinct identities proven via exact algorithm replica of `ran3` initialization.

## Artifacts Produced

| Artifact | Path | Purpose |
|----------|------|---------|
| Patch | `reference/flexpart-11.1-seedable.patch` | Versioned initialization-only diff |
| Contract | `reference/oracle-stochastic-identity.json` | Machine-readable identity semantics |
| Library | `scripts/corpus/oracle_stochastic_identity.py` | Mapping, validation, replica |
| Comparator | `scripts/corpus/compare_oracle_seed_identities.py` | Evidence report generator |
| Tests | `scripts/corpus/test_oracle_stochastic_identity.py` | Fail-closed regression suite |
| Tests | `scripts/corpus/test_compare_oracle_seed_identities.py` | Comparator regression suite |
| Docs | `docs/oracle-stochastic-identity.md` | Source crosswalk |
| Runner | `scripts/run-corpus.sh oracle-seed-identities` | Reusable evidence pipeline |

## Verification Results

| Criterion | Result |
|-----------|--------|
| Pristine 5× WIND-UNI-002 | Bitwise repeatable |
| Seedable default ≡ pristine | Bitwise identical |
| 10 identities distinct | Pairwise distinct SHA-256 (raw & decoded) |
| Identity 3 ×5 repeat | Bitwise repeatable |
| Invalid seeds rejected | Exit 1 + actionable fragment |
| Seedable ≠ pristine binary | Distinct SHA-256 |
| Pristine checkout clean | `git status --porcelain` empty |

## Handoff

- **#51 (case manifest):** Stochastic identity fields use separate candidate/oracle namespaces.
- **#57 (oracle ensemble runner):** Consumes this contract to orchestrate multi-identity runs.
- **#58/#59 (aggregation/verdicts):** Use the evidence but own statistical adequacy.