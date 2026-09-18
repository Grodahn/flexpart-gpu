# FLEXPART 11.1 RNG/Seed Source Crosswalk (Issue #50)

This document traces the complete RNG initialization and consumption chain in the
pinned FLEXPART 11.1 sources (commit `c70586c2b7f5258850705325881c61f557ea9bd8`),
answering the mandatory questions from RISK-03.3G-01V-b issue #50.

## 1. RNG State Allocation and Initialization

**File:** `src/random_mod.f90`

- **Module:** `random_mod`
- **State arrays (per-thread, 0-indexed):**
  - `iseed1(0:num_threads-1)` — Knuth subtractive RNG seed stream 1
  - `iseed2(0:num_threads-1)` — Knuth subtractive RNG seed stream 2
  - `ma(55, 0:num_threads-1)` — Knuth subtractive RNG table (55 entries per thread)
  - `ran3_iff(0:num_threads-1)` — ran3 initialization flag
  - `ran3_inext/ran3_inextp(0:num_threads-1)` — ran3 table indices
  - `ran1_iv(ran1_ntab, 0:num_threads-1)` — ran1 shuffle table (32 entries)
  - `ran1_iy(0:num_threads-1)` — ran1 current value
  - `gasdev_iset/gasdev_gset(0:num_threads-1)` — Box-Muller state for Gaussian

- **Allocation:** `alloc_random(num_threads)` called once from `FLEXPART.f90:227`
- **Pristine hard-coded defaults (per thread `i`):**
  - `iseed1(i) = -7 - i`
  - `iseed2(i) = -88 - i`
  - `ran3_iff(i) = 0`
  - `ran1_iv(:, i) = 0`
  - `ran1_iy(i) = 0`
  - `gasdev_iset(i) = 0`
  - `gasdev_gset(i) = 0`

- **No external input path:** The seeds are never read from COMMAND namelist,
  environment variables, command line, or any file. The `get_environment_variable`
  intrinsic is not used anywhere in the pinned sources.

## 2. RNG Routines and Algorithms

| Routine | Algorithm | Purpose | Streams used |
|---------|-----------|---------|--------------|
| `ran3(idum, ithread)` | Knuth subtractive (mbig=1000000000, mseed=161803398) | Core uniform [0,1) | `iseed1`, `iseed2`, `ma` |
| `ran1(idum, ithread)` | Park-Miller LCG + Bays-Durham shuffle | Uniform [0,1) for release jitter | `ran1_iv`, `ran1_iy` |
| `gasdev(idum, ithread)` | Box-Muller over `ran3` | Gaussian N(0,1) | `gasdev_iset`, `gasdev_gset` |
| `gasdev1(idum, r1, r2)` | Box-Muller over `ran3(thread 0)` | Gaussian pair for precompute | `ran3` thread 0 |

**Precomputed Gaussian table:** `FLEXPART.f90:231-234` calls `gasdev1(idummy, ...)` with hard-coded
`idummy = -320` to fill `rannumb(1:maxrand+2)` once at startup. This table is then
consumed by all turbulence and release-jitter code via index `nrand`.

## 3. RNG Consumption Sites (Forward Path)

### Turbulence / PBL (per-thread streams)

- **`initialise_mod.f90:704`** — `nrand = int(ran3(iseed1(ithread), ithread) * (maxrand-1)) + 1`
  Picks starting index into `rannumb` table for each particle initialization.

- **`turbulence_mod.f90`** — Primary consumer of `rannumb(nrand)` for:
  - Horizontal velocities `u`, `v` (Ornstein-Uhlenbeck, lines 67, 73)
  - Vertical velocity `w` (lines 109, 124, 129, 137, 143, 150)
  - CBL skewed distributions (via `cbl_mod.f90`, lines 401, 404, 407)
  - Mesoscale turbulence (lines 247, 248, 253, 259)

- **`advance_mod.f90:160`** — Same `nrand` selection as initialise for each step.

- **`cbl_mod.f90`** — `ran3(idum, ithread)` and `gasdev(idum, ithread)` directly
  for CBL updraft/downdraft sampling (lines 282, 315, 323, 401, 404, 407).

### Release Jitter (ran1, thread 0 only)

- **`initialise_mod.f90`** — `ran1(idummy, 0)` for point release positions
  (lines 231, 238, 255, 267) with hard-coded `idummy = -7`.
- **`initialise_mod.f90`** — Domain fill paths use `ran1(idummy, 0/ithread)` with
  `idummy = -8, -11` (lines 508, 618, 922, 1093-1122, 1181, 1400, 1580-1859).
- **`initdomain_mod.f90`** — `ran1(idummy, 0)` with `idummy = -11` (lines 756-888).
- **`netcdf_output_mod.f90`** — `ran1(idummy, 0)` with `idummy = -8` (lines 2432, 2525, 2572, 2935).

**Note:** For WIND-UNI-002 (point release, single uncertainty class), all `xaux=yaux=zaux=0`
and `nclass=1`, so release-jitter `ran1` calls produce identical values for all
particles and do not affect trajectories.

### Convection (iseed2)

- **`conv_mod.f90:845`** — `ran3(iseed2(ithread-1), ithread-1)` for stochastic
  redistribution (unused when `LCONVECTION=0` as in WIND-UNI-002).

## 4. OpenMP / Thread Dependence

- **Thread indexing:** FLEXPART uses 1-based `ithread` for OpenMP (`OMP_GET_THREAD_NUM()+1`)
  in most modules, but `random_mod` arrays are 0-indexed. The mapping is consistent:
  module code passes `ithread` (0-based) to RNG routines.
- **Single-thread profile (#49):** `OMP_NUM_THREADS=1` means only `i=0` is ever used.
  All RNG state is confined to thread 0; no thread-scheduling non-determinism exists.
- **Restart:** `restart_mod.f90` does **not** persist RNG state. Restarted runs
  re-initialize from pristine defaults.

## 5. Native Seed Control — Exhausted Search

| Mechanism | Status | Evidence |
|-----------|--------|----------|
| COMMAND namelist seed variable | **Absent** | `readoptions_mod.f90` declares no seed field |
| `FLEXPART_VALIDATION_SEED` env var | **Absent** | No `get_environment_variable` calls in pinned sources |
| Command line / `get_command_argument` | **Absent** | No such calls found |
| `random_seed` / `random_number` intrinsics | **Absent** | Not used anywhere |
| File-based seed input | **Absent** | No seed file read in `readoptions_mod` |

**Conclusion:** Pristine FLEXPART 11.1 exposes **no approved external seed control**.
The only way to obtain independent stochastic realizations is a validation-only
initialization patch (implemented in issue #50).

## 6. Validation Patch (RISK-03.3G-01V-b)

**Artifact:** `reference/flexpart-11.1-seedable.patch` (SHA-256:
`554ec7d4aa291c5ded242c7342e22fdf3615964ea397abc85f738543f55be96c`)

**Scope:** Two files only — `src/random_mod.f90`, `src/FLEXPART.f90`
**Effect:** Adds `validation_seed_offset()` selector reading
`FLEXPART_VALIDATION_SEED` (canonical decimal `[1, 1000000000]`).
Applies offset `S` to:
- `iseed1(i) = -(7 + i + S)`
- `iseed2(i) = -(88 + i + S)`
- `rannumb` table `idummy = -(320 + S)`

**Default mode (unset/empty):** `S=0`, pristine bit-exact initialization preserved.
**Fail-closed:** Invalid/out-of-range/non-canonical seeds stop the Fortran run
with actionable error; no silent fallback.

## 7. Verification Evidence (Issue #50 Experiments)

| Experiment | Result |
|------------|--------|
| Pristine WIND-UNI-002 ×5 (frozen #49 profile) | Bitwise repeatable |
| Seedable default (unset) vs pristine | Bitwise identical |
| 10 identities (1..10) on WIND-UNI-002 | Pairwise distinct raw & decoded outputs |
| Identity 3 ×5 repetitions | Bitwise repeatable |
| Invalid seeds (0, 1000000001, abc) | All rejected with actionable error |
| Seedable ≠ pristine executable | Distinct SHA-256 confirmed |

All evidence recorded in `target/corpus/oracle_seed_identity_report.json`.