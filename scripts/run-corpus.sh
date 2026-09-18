#!/usr/bin/env bash
set -euo pipefail
# ===========================================================================
# Corpus runner (Issue #6, point 2).
#
# Reuses the existing Docker oracle, comparison and ETEX workflows; no second
# reference path is introduced.
#
# Usage:
#   scripts/run-corpus.sh candidate [CASE] [--seeds N]
#       Run the WGSL candidate for one case (default: all implemented synthetic
#       cases) with N Philox seeds (default 10). Writes raw per-seed outputs
#       under target/corpus/candidate/<CASE>/seed_*.json.
#   scripts/run-corpus.sh oracle [CASE]
#       Prepare synthetic GRIB + run the pinned FLEXPART 11.1 oracle in Docker
#       for one case (default: all). Writes raw outputs under
#       target/corpus/oracle/<CASE>/. Requires Docker and the pinned checkout
#       at ../flexpart (see docs/reference-environment.md).
#   scripts/run-corpus.sh oracle-repeatability [CASE] [REPS]
#       Issue #49 frozen single-thread repeatability: build the oracle once,
#       then run ADV-ANA-001 and WIND-UNI-002 REPS times each (default 5) with
#       isolated run/output directories under
#       target/corpus/oracle_repeatability/<CASE>/rep_XX/ and write
#       target/corpus/oracle_repeatability_report.json. Only the cases run by
#       the invocation are evaluated (a single CASE yields a valid single-case
#       report; canonical evidence uses the default: both cases). FLEXPART_DIR
#       may point to the pinned checkout when this repository is an isolated
#       worktree.
#   scripts/run-corpus.sh compare
#       Compute machine-readable metrics (mass, COM, covariance/eigenvalues,
#       vertical quantiles, overlap, field correlation, process budgets) into
#       target/corpus/comparison_report.json. Oracle pairing is diagnostic.
#   scripts/run-corpus.sh manifest
#       Write target/corpus/run_manifest.json (hashes, revisions, Fortran
#       build, adapter, seeds). Fails closed on missing artifacts or a
#       modified/unpinned oracle checkout.
#   scripts/run-corpus.sh all [--seeds N]
#       candidate + oracle + compare + manifest. Every step is required:
#       any failure aborts the workflow (fail-closed). For a candidate-only
#       run without the oracle, use the explicit `candidate` subcommand.
#   scripts/run-corpus.sh audit
#       Verify fixtures, pinned oracle commit and input hashes without running
#       simulations.
#
# Prerequisites:
#   - Rust toolchain (candidate), Python 3 without extra packages
#     (compare/manifest/decoder use the standard library only)
#   - Docker + docker-compose (oracle only)
#   - FLEXPART_GPU_SOFTWARE=1 on machines without a hardware GPU
#   - ETEX mini real-weather case stays under scripts/run-etex.sh mini
#
# Runtime (software WGSL adapter, Microsoft Basic Render Driver):
#   - CI deterministic subset (cargo test corpus_*): < 5 s
#   - candidate PBL-NEUTRAL-005, 10 seeds, 500 particles x 12 steps: ~1-2 min
#   - full synthetic candidate (8 cases x 10 seeds): ~10-20 min locally
#   - oracle per case in Docker: a few minutes plus FLEXPART build once
# ===========================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# FLEXPART_DIR may be exported to reuse one pinned oracle checkout from an
# isolated worktree; default preserves the historical sibling layout.
FLEXPART_DIR="${FLEXPART_DIR:-${PROJECT_ROOT}/../flexpart}"
FORTRAN_COMPOSE_FILE="${PROJECT_ROOT}/docker/docker-compose.fortran.yml"
HOST_PYTHON="${CORPUS_PYTHON:-python3}"
if [ "${OS:-}" = "Windows_NT" ]; then
  HOST_PYTHON="${CORPUS_PYTHON:-python}"
fi
CANDIDATE_DIR="${PROJECT_ROOT}/target/corpus/candidate"
ORACLE_DIR="${PROJECT_ROOT}/target/corpus/oracle"
REPORT="${PROJECT_ROOT}/target/corpus/comparison_report.json"
MANIFEST="${PROJECT_ROOT}/target/corpus/run_manifest.json"
CORPUS_INDEX="${PROJECT_ROOT}/fixtures/corpus/corpus.json"

SYNTHETIC_CASES="ADV-ANA-001 WIND-UNI-002 WIND-SHEAR-003 PBL-STABLE-004 PBL-NEUTRAL-005 PBL-UNSTABLE-006 DRY-007 WET-008 REPEAT-009"
# Issue #49: frozen single-thread oracle repeatability. Retained repetitions live
# outside the single-result oracle layout so #53 can own the global artifact
# redesign later. Build provenance is recorded once; every repetition reuses the
# same executable and shared meteorology.
REPEAT_CASES="ADV-ANA-001 WIND-UNI-002"
REPEAT_MINIMUM=5
REPEAT_DIR="${PROJECT_ROOT}/target/corpus/oracle_repeatability"
REPEAT_RUN_ROOT="${PROJECT_ROOT}/target/corpus/fortran_run_repeatability"
REPEAT_REPORT="${PROJECT_ROOT}/target/corpus/oracle_repeatability_report.json"

RED='\033[0;31m'; GREEN='\033[0;32m'; BLUE='\033[0;34m'; NC='\033[0m'
log_info() { echo -e "${GREEN}[INFO]${NC}  $*"; }
log_error() { echo -e "${RED}[ERROR]${NC}  $*" >&2; }
log_step() { echo -e "\n${BLUE}=== Step: $* ===${NC}"; }

require_pinned_fortran() {
  local manifest="${PROJECT_ROOT}/reference/flexpart-11.1.json"
  local pinned actual
  pinned="$(sed -n 's/^[[:space:]]*"pinned_commit": *"\([0-9a-f]*\)".*/\1/p' "${manifest}" | head -1)"
  if ! printf '%s' "${pinned}" | grep -qE '^[0-9a-f]{40}$'; then
    log_error "Could not read pinned_commit from ${manifest}"
    return 1
  fi
  if ! actual="$(git -C "${FLEXPART_DIR}" rev-parse HEAD 2>/dev/null)"; then
    log_error "${FLEXPART_DIR} is not a git checkout (see docs/reference-environment.md)"
    return 1
  fi
  if [ "${actual}" != "${pinned}" ]; then
    log_error "Fortran checkout is at ${actual}, expected pinned ${pinned}"
    return 1
  fi
  if [ -n "$(git -C "${FLEXPART_DIR}" status --porcelain)" ]; then
    log_error "Fortran checkout has uncommitted changes; the oracle must stay unmodified"
    return 1
  fi
  log_info "Fortran oracle pinned at ${pinned} (clean)"
}

# Issue #49: the v11.1 makefile stamps its git version into tracked
# src/FLEXPART.f90 plus untracked gitversion.txt. Restore both after every
# build so the normative checkout stays pristine; the git-ignored binary
# itself remains for the retained repetitions.
restore_oracle_checkout() {
  git -C "${FLEXPART_DIR}" checkout -- src/FLEXPART.f90 2>/dev/null || true
  rm -f "${FLEXPART_DIR}/src/gitversion.txt"
  if [ -n "$(git -C "${FLEXPART_DIR}" status --porcelain)" ]; then
    log_error "Oracle checkout is dirty after build/run; refusing to continue"
    git -C "${FLEXPART_DIR}" status --porcelain | head -20 >&2 || true
    return 1
  fi
}

# Build and identify the frozen oracle executable once. Repetitions must reuse
# this binary; rebuilding per repetition would mix build reproducibility with
# run repeatability.
oracle_build_once() {
  require_pinned_fortran
  log_info "Building oracle image and compiling pinned FLEXPART once..."
  docker compose -f "${FORTRAN_COMPOSE_FILE}" build flexpart-fortran
  docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm flexpart-fortran bash -c "
    set -euo pipefail
    cd /workspace/flexpart/src
    make -f makefile_gfortran clean >/dev/null 2>&1 || true
    FC=gfortran make -f makefile_gfortran eta=no arch=x86-64 -j4 2>&1 | tail -5
    test -x FLEXPART
    rm -f gitversion.txt
  "
  # The makefile stamps tracked src/FLEXPART.f90; only the host git restores it
  # (the container user cannot own the Windows bind mount for git).
  restore_oracle_checkout
  # Host check is existence only: Windows checkouts via MSYS2 lack the exec
  # bit; executability is proven inside the Linux container (test -x).
  if [ ! -f "${FLEXPART_DIR}/src/FLEXPART" ]; then
    log_error "Oracle executable missing after build: ${FLEXPART_DIR}/src/FLEXPART"
    return 1
  fi
  ORACLE_EXE_SHA="$("${HOST_PYTHON}" -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "${FLEXPART_DIR}/src/FLEXPART")"
  if [ "${#ORACLE_EXE_SHA}" != 64 ]; then
    log_error "Could not hash oracle executable"
    return 1
  fi
  # Build identity for this experiment: every retained repetition is tied to
  # these values via per-case experiment.json and per-rep records.
  ORACLE_IMAGE_ID="$(docker image inspect flexpart-fortran:latest --format '{{.Id}}')"
  ORACLE_COMPILER="$(docker run --rm flexpart-fortran:latest gfortran --version | head -1)"
  ORACLE_MAKEFILE_SHA="$("${HOST_PYTHON}" -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "${FLEXPART_DIR}/src/makefile_gfortran")"
  if [ -z "${ORACLE_IMAGE_ID}" ] || [ -z "${ORACLE_COMPILER}" ] || [ "${#ORACLE_MAKEFILE_SHA}" != 64 ]; then
    log_error "Could not record oracle build provenance"
    return 1
  fi
  log_info "Oracle executable SHA-256: ${ORACLE_EXE_SHA}"
  log_info "Oracle image: ${ORACLE_IMAGE_ID} (${ORACLE_COMPILER})"
}

# Record the experiment a case's retained repetitions belong to. The case
# directory is recreated fresh so repetitions from an older experiment can
# never enter the new report silently.
oracle_write_case_experiment() {
  local case="$1"
  local reps="$2"
  local case_dir="${REPEAT_DIR}/${case}"
  rm -rf "${case_dir}"
  mkdir -p "${case_dir}"
  "${HOST_PYTHON}" - "${PROJECT_ROOT}/reference/flexpart-11.1.json" "${case}" "${reps}" \
    "${ORACLE_EXE_SHA}" "${ORACLE_IMAGE_ID}" "${ORACLE_COMPILER}" "${ORACLE_MAKEFILE_SHA}" \
    "${case_dir}/experiment.json" <<'PYEOF'
import datetime
import json
import sys
(manifest_path, case_id, reps, exe_sha, image_id, compiler, makefile_sha, output) = sys.argv[1:9]
reference = json.load(open(manifest_path, encoding="utf-8"))
profile = reference["execution_profile"]
experiment = {
    "execution_profile": {"id": profile["id"], "version": profile["version"]},
    "case": case_id,
    "classification": profile["repeatability_cases"][case_id],
    "repetitions_requested": int(reps),
    "experiment_started_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "oracle_pinned_commit": reference["pinned_commit"],
    "oracle_executable_sha256": exe_sha,
    "docker_image": profile["docker"]["image"],
    "docker_image_id": image_id,
    "compiler": compiler,
    "make_arguments": profile["build"]["make_arguments"],
    "makefile_sha256": makefile_sha,
    "external_seed_control": profile["external_seed_control"],
}
open(output, "w", encoding="utf-8").write(json.dumps(experiment, indent=2) + "\n")
print(f"Experiment record: {output}")
PYEOF
}

oracle_assert_same_executable() {
  local current
  current="$("${HOST_PYTHON}" -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "${FLEXPART_DIR}/src/FLEXPART")"
  if [ "${current}" != "${ORACLE_EXE_SHA}" ]; then
    log_error "Oracle executable changed during repeatability experiment (expected ${ORACLE_EXE_SHA}, found ${current})"
    return 1
  fi
}

# Generate shared synthetic meteorology once per case so all repetitions of
# that case consume bit-identical inputs.
oracle_generate_meteo_once() {
  local case="$1"
  local fixture="${PROJECT_ROOT}/fixtures/corpus/fortran/${case}"
  local meteodir="${PROJECT_ROOT}/target/corpus/meteo/${case}"
  mkdir -p "${meteodir}"
  log_info "Meteo args for ${case}: $(cat "${fixture}/METEO_ARGS.txt")"
  # shellcheck disable=SC2086
  docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm flexpart-fortran \
    python3 //workspace/flexpart-gpu/scripts/generate_synthetic_grib.py \
    --output-dir "//workspace/corpus/meteo/${case}" \
    $(cat "${fixture}/METEO_ARGS.txt")
  test -f "${meteodir}/AVAILABLE"
}

# Prepare one isolated run directory from versioned fixtures. The flat
# <case>_rep_NN layout keeps ../../meteo/<case> resolving to the shared
# target/corpus/meteo/<case> exactly like the single-result oracle path.
oracle_prepare_rep_rundir() {
  local case="$1"
  local rundir="$2"
  local fixture="${PROJECT_ROOT}/fixtures/corpus/fortran/${case}"
  rm -rf "${rundir}"
  mkdir -p "${rundir}/options/SPECIES" "${rundir}/output"
  cp "${fixture}/COMMAND" "${rundir}/options/COMMAND"
  cp "${fixture}/RELEASES" "${rundir}/options/RELEASES"
  cp "${fixture}/OUTGRID" "${rundir}/options/OUTGRID"
  cp "${fixture}/AGECLASSES" "${rundir}/options/AGECLASSES"
  cp "${fixture}/RECEPTORS" "${rundir}/options/RECEPTORS"
  cp "${fixture}"/SPECIES/SPECIES_* "${rundir}/options/SPECIES/"
  cp "${FLEXPART_DIR}/options/IGBP_int1.dat" "${rundir}/options/" 2>/dev/null || true
  cp "${FLEXPART_DIR}/options/sfcdata.t" "${rundir}/options/" 2>/dev/null || true
  cp "${FLEXPART_DIR}/options/sfcdepo.t" "${rundir}/options/" 2>/dev/null || true
  cp "${FLEXPART_DIR}/options/PARTOPTIONS" "${rundir}/options/" 2>/dev/null || true
  cat > "${rundir}/pathnames" <<PATHEOF
./options/
./output/
../../meteo/${case}/
../../meteo/${case}/AVAILABLE
============================================
PATHEOF
}

# Run FLEXPART once with the frozen runtime guard and preserve raw plus decoded
# artifacts in an isolated repetition directory. Reuses the single-result
# decode path; no second oracle implementation.
oracle_run_one_repetition() {
  local case="$1"
  local rundir="$2"
  local repdir="$3"
  # Container rundir uses a double leading slash so MSYS2/Git Bash on Windows
  # leaves it untouched (POSIX collapses // to / inside Linux).
  local container_rundir="//workspace/corpus/fortran_run_repeatability/$(basename "${rundir}")"
  rm -rf "${repdir}"
  mkdir -p "${repdir}/raw"
  # Tie this repetition to the experiment executable and prove which inputs
  # the invocation consumes. Both records are written BEFORE FLEXPART runs:
  # post-run fixture copies alone cannot prove what was consumed.
  printf '%s' "${ORACLE_EXE_SHA}" > "${repdir}/oracle_executable.sha256"
  "${HOST_PYTHON}" - "${rundir}" "${PROJECT_ROOT}/target/corpus/meteo/${case}" "${repdir}/consumed_inputs.json" <<'PYEOF'
import hashlib
import json
import sys
from pathlib import Path
rundir, meteodir, output = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()
options = rundir / "options"
consumed = {"options": {}, "meteo": {}}
for path in sorted(p for p in options.rglob("*") if p.is_file()):
    consumed["options"][str(path.relative_to(rundir)).replace("\\", "/")] = digest(path)
for required in ("options/COMMAND", "options/RELEASES", "options/OUTGRID"):
    if required not in consumed["options"]:
        raise SystemExit(f"prepared run directory lacks {required}")
consumed["pathnames_sha256"] = digest(rundir / "pathnames")
consumed["pathnames_text"] = (rundir / "pathnames").read_text(encoding="utf-8")
if not (meteodir / "AVAILABLE").is_file():
    raise SystemExit(f"shared meteorology lacks AVAILABLE: {meteodir}")
for path in sorted(p for p in meteodir.rglob("*") if p.is_file()):
    consumed["meteo"][str(path.relative_to(meteodir)).replace("\\", "/")] = digest(path)
Path(output).write_text(json.dumps(consumed, indent=2) + "\n", encoding="utf-8")
print(f"Consumed inputs: {output}")
PYEOF
  docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm flexpart-fortran bash -c "
    set -euo pipefail
    python3 /workspace/flexpart-gpu/scripts/write_oracle_run_manifest.py check-runtime-profile \
      --oracle-manifest /workspace/flexpart-gpu/reference/flexpart-11.1.json \
      > /workspace/corpus/oracle_repeatability/${case}/$(basename "${repdir}")/runtime_profile.json
    cd ${container_rundir} && /workspace/flexpart/src/FLEXPART
  " 2>&1 | tee "${repdir}/fortran.log"
  if ! grep -q "CONGRATULATIONS" "${repdir}/fortran.log"; then
    log_error "Oracle repetition failed for ${case} $(basename "${repdir}"); see ${repdir}/fortran.log"
    return 1
  fi
  cp "${rundir}/output/header" "${rundir}/output/dates" "${repdir}/raw/"
  cp "${rundir}"/output/grid_conc_* "${repdir}/raw/"
  for partposit in "${rundir}"/output/partposit_*; do
    [ -e "${partposit}" ] || continue
    cp "${partposit}" "${repdir}/raw/"
  done
  cp "${PROJECT_ROOT}/fixtures/corpus/fortran/${case}/COMMAND" "${PROJECT_ROOT}/fixtures/corpus/fortran/${case}/RELEASES" "${PROJECT_ROOT}/fixtures/corpus/fortran/${case}/OUTGRID" "${repdir}/"
  "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/corpus/decode_oracle_output.py" \
    --raw-dir "${repdir}/raw" \
    --releases "${repdir}/RELEASES" \
    --output "${repdir}/oracle_summary.json"
}

step_oracle_repeatability() {
  local cases="${1:-${REPEAT_CASES}}"
  local reps="${2:-${REPEAT_MINIMUM}}"
  if [ "${cases}" = "all" ]; then
    cases="${REPEAT_CASES}"
  fi
  case " ${cases} " in
    *"REPEAT-009"*) log_error "REPEAT-009 is candidate-side only and is not the oracle test case for #49"; return 1 ;;
  esac
  mkdir -p "${REPEAT_DIR}" "${REPEAT_RUN_ROOT}"
  oracle_build_once
  ran_cases=""
  for case in ${cases}; do
    case " ${REPEAT_CASES} " in
      *" ${case} "*) ;;
      *) log_error "Case ${case} is not in the frozen #49 repeatability set (${REPEAT_CASES})"; return 1 ;;
    esac
    log_step "Oracle repeatability ${case} x${reps} (shared exe ${ORACLE_EXE_SHA})"
    # Fresh case directory: repetitions from an older experiment must never
    # enter the new report silently. Only cases run below are evaluated.
    oracle_write_case_experiment "${case}" "${reps}"
    oracle_generate_meteo_once "${case}"
    for i in $(seq 1 "${reps}"); do
      rep="$(printf 'rep_%02d' "${i}")"
      oracle_assert_same_executable
      oracle_prepare_rep_rundir "${case}" "${REPEAT_RUN_ROOT}/${case}_rep_${rep}"
      oracle_run_one_repetition "${case}" "${REPEAT_RUN_ROOT}/${case}_rep_${rep}" "${REPEAT_DIR}/${case}/${rep}"
    done
    require_pinned_fortran
    ran_cases="${ran_cases} ${case}"
  done
  restore_oracle_checkout
  log_step "Oracle repeatability comparison (cases:${ran_cases})"
  "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/corpus/compare_oracle_repeatability.py" \
    --repeat-dir "${REPEAT_DIR}" \
    --output "${REPEAT_REPORT}" \
    --oracle-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --oracle-checkout "${FLEXPART_DIR}" \
    --oracle-exe "${FLEXPART_DIR}/src/FLEXPART" \
    --meteo-dir "${PROJECT_ROOT}/target/corpus/meteo" \
    --fixtures-dir "${PROJECT_ROOT}/fixtures/corpus/fortran" \
    --cases "${ran_cases}"
  log_info "Repeatability evidence under ${REPEAT_DIR}/ and ${REPEAT_REPORT}"
}

step_candidate() {
  local case="${1:-all}"
  local seeds="${2:-10}"
  log_step "Candidate corpus run (case=${case}, seeds=${seeds})"
  mkdir -p "${CANDIDATE_DIR}"
  if [ "${case}" = "all" ]; then
    cargo run --release --bin corpus-run -- --all --seeds "${seeds}" --out-dir "${CANDIDATE_DIR}"
  else
    if [ "${case}" = "ADV-ANA-001" ]; then
      cargo run --release --bin corpus-run -- --case "${case}" --seeds 1 --out-dir "${CANDIDATE_DIR}"
    elif [ "${case}" = "REPEAT-009" ]; then
      cargo run --release --bin corpus-run -- --case "${case}" --seeds 2 --out-dir "${CANDIDATE_DIR}"
    else
      cargo run --release --bin corpus-run -- --case "${case}" --seeds "${seeds}" --out-dir "${CANDIDATE_DIR}"
    fi
  fi
}

step_oracle_case() {
  local case="$1"
  local fixture="${PROJECT_ROOT}/fixtures/corpus/fortran/${case}"
  local rundir="${PROJECT_ROOT}/target/corpus/fortran_run/${case}"
  local meteodir="${PROJECT_ROOT}/target/corpus/meteo/${case}"
  local oracledir="${ORACLE_DIR}/${case}"
  log_step "Oracle run ${case}"
  if [ ! -d "${fixture}" ]; then
    log_error "Missing oracle fixture: ${fixture}"
    return 1
  fi
  mkdir -p "${rundir}/options/SPECIES" "${rundir}/output" "${meteodir}" "${oracledir}/raw"
  # Synthetic meteorology from the single generator path. Flags come from the
  # versioned METEO_ARGS.txt written by generate_fortran_fixtures.py (derived
  # from the case JSON), so no wind/surface value is duplicated in this script.
  # target/corpus is bind-mounted as /workspace/corpus (see
  # docker/docker-compose.fortran.yml); without that mount the GRIB files
  # would disappear with the container.
  log_info "Meteo args: $(cat "${fixture}/METEO_ARGS.txt")"
  # METEO_ARGS.txt is read from the host fixture path (command substitution
  # runs on the host). Bare container paths use a double leading slash so
  # MSYS2/Git Bash on Windows leaves them untouched (POSIX collapses // to /
  # inside the Linux container); single-slash absolute args would be
  # rewritten to a Windows MSYS root and break the container command.
  # shellcheck disable=SC2086
  docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm flexpart-fortran \
    python3 //workspace/flexpart-gpu/scripts/generate_synthetic_grib.py \
    --output-dir "//workspace/corpus/meteo/${case}" \
    $(cat "${fixture}/METEO_ARGS.txt")
  test -f "${meteodir}/AVAILABLE"
  cp "${fixture}/COMMAND" "${rundir}/options/COMMAND"
  cp "${fixture}/RELEASES" "${rundir}/options/RELEASES"
  cp "${fixture}/OUTGRID" "${rundir}/options/OUTGRID"
  cp "${fixture}/AGECLASSES" "${rundir}/options/AGECLASSES"
  cp "${fixture}/RECEPTORS" "${rundir}/options/RECEPTORS"
  cp "${fixture}"/SPECIES/SPECIES_* "${rundir}/options/SPECIES/"
  cp "${FLEXPART_DIR}/options/IGBP_int1.dat" "${rundir}/options/" 2>/dev/null || true
  cp "${FLEXPART_DIR}/options/sfcdata.t" "${rundir}/options/" 2>/dev/null || true
  cp "${FLEXPART_DIR}/options/sfcdepo.t" "${rundir}/options/" 2>/dev/null || true
  cp "${FLEXPART_DIR}/options/PARTOPTIONS" "${rundir}/options/" 2>/dev/null || true
  # pathnames entries are relative to the run directory
  # target/corpus/fortran_run/<case>, so the per-case meteorology at
  # target/corpus/meteo/<case> is ../../meteo/<case>.
  cat > "${rundir}/pathnames" <<PATHEOF
./options/
./output/
../../meteo/${case}/
../../meteo/${case}/AVAILABLE
============================================
PATHEOF
  # The meteo bind-mount layout mirrors scripts/compare-fortran.sh validate.
  log_info "Building oracle image (once) and compiling pinned FLEXPART..."
  docker compose -f "${FORTRAN_COMPOSE_FILE}" build flexpart-fortran
  docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm flexpart-fortran bash -c "
    set -euo pipefail
    cd /workspace/flexpart/src
    make -f makefile_gfortran clean >/dev/null 2>&1 || true
    FC=gfortran make -f makefile_gfortran eta=no arch=x86-64 -j4 2>&1 | tail -5
    test -x FLEXPART
    rm -f gitversion.txt
  "
  log_info "Running pinned oracle for ${case}..."
  docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm flexpart-fortran bash -c "
    set -euo pipefail
    python3 /workspace/flexpart-gpu/scripts/write_oracle_run_manifest.py check-runtime-profile \
      --oracle-manifest /workspace/flexpart-gpu/reference/flexpart-11.1.json \
      > /workspace/corpus/oracle/${case}/runtime_profile.json
    cd /workspace/corpus/fortran_run/${case} && /workspace/flexpart/src/FLEXPART
  " 2>&1 | tee "${oracledir}/fortran.log"
  if ! grep -q "CONGRATULATIONS" "${oracledir}/fortran.log"; then
    log_error "Oracle run failed for ${case}; see ${oracledir}/fortran.log"
    return 1
  fi
  # Preserve the raw oracle outputs inside the hashed oracle directory:
  # header, dates and every grid_conc_* slice, plus any partposit dumps.
  # FLEXPART 11.1 writes binary concentration output only; per-particle
  # partposit dumps are NetCDF-only in v11.1, so grid_conc_* is required
  # while partposit_* is optional.
  cp "${rundir}/output/header" "${rundir}/output/dates" "${oracledir}/raw/"
  cp "${rundir}"/output/grid_conc_* "${oracledir}/raw/"
  for partposit in "${rundir}"/output/partposit_*; do
    [ -e "${partposit}" ] || continue
    cp "${partposit}" "${oracledir}/raw/"
  done
  cp "${fixture}/COMMAND" "${fixture}/RELEASES" "${fixture}/OUTGRID" "${oracledir}/"
  # Decode the raw outputs into the machine-readable summary consumed by
  # compare_corpus.py. Fails when required artifacts are absent.
  "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/corpus/decode_oracle_output.py" \
    --raw-dir "${oracledir}/raw" \
    --releases "${oracledir}/RELEASES" \
    --output "${oracledir}/oracle_summary.json"
  log_info "Oracle output for ${case} under ${oracledir}/"
}

step_oracle() {
  local case="${1:-all}"
  require_pinned_fortran
  mkdir -p "${ORACLE_DIR}"
  if [ "${case}" = "all" ]; then
    for c in ${SYNTHETIC_CASES}; do
      if [ "${c}" = "REPEAT-009" ]; then
        log_info "Skipping oracle for REPEAT-009 (no oracle seed control; see docs/corpus-matrix.md)"
        continue
      fi
      step_oracle_case "${c}"
    done
  else
    step_oracle_case "${case}"
  fi
}

step_compare() {
  log_step "Corpus comparison"
  mkdir -p "$(dirname "${REPORT}")"
  # Input equality is asserted before any output is compared: unequal
  # release/grid inputs must fail here, never surface as model differences.
  "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/corpus/audit_corpus_inputs.py" \
    --fixtures "${PROJECT_ROOT}/fixtures/corpus" \
    --candidate-dir "${CANDIDATE_DIR}" \
    --oracle-dir "${ORACLE_DIR}"
  "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/corpus/compare_corpus.py" \
    --candidate-dir "${CANDIDATE_DIR}" \
    --oracle-dir "${ORACLE_DIR}" \
    --output "${REPORT}"
}

step_manifest() {
  log_step "Corpus provenance manifest"
  require_pinned_fortran
  local candidate_exe="${PROJECT_ROOT}/target/release/corpus-run"
  if [ "${OS:-}" = "Windows_NT" ]; then
    candidate_exe="${candidate_exe}.exe"
  fi
  "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/corpus/write_corpus_manifest.py" \
    --output "${MANIFEST}" \
    --corpus-index "${CORPUS_INDEX}" \
    --oracle-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --oracle-checkout "${FLEXPART_DIR}" \
    --candidate-checkout "${PROJECT_ROOT}" \
    --candidate-dir "${CANDIDATE_DIR}" \
    --oracle-dir "${ORACLE_DIR}" \
    --report "${REPORT}" \
    --cases-dir "${PROJECT_ROOT}/fixtures/corpus/cases" \
    --fortran-fixtures "${PROJECT_ROOT}/fixtures/corpus/fortran" \
    --thresholds "${PROJECT_ROOT}/fixtures/corpus/thresholds.json" \
    --meteo-dir "${PROJECT_ROOT}/target/corpus/meteo" \
    --candidate-exe "${candidate_exe}" \
    --oracle-exe "${FLEXPART_DIR}/src/FLEXPART"
}

step_audit() {
  log_step "Corpus audit (no simulations)"
  require_pinned_fortran
  "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/corpus/generate_fortran_fixtures.py" --flexpart-dir "${FLEXPART_DIR}" >/dev/null
  if [ -n "$(git -C "${PROJECT_ROOT}" status --porcelain -- fixtures/corpus/fortran/)" ]; then
    log_error "Generated Fortran fixtures differ from checked-in version; run the generator and commit the result"
    return 1
  fi
  "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/corpus/audit_corpus_inputs.py" \
    --fixtures "${PROJECT_ROOT}/fixtures/corpus"
  "${HOST_PYTHON}" -c "import json; d=json.load(open('${CORPUS_INDEX}')); assert d['version']==1; print(f\"corpus v{d['version']}: {len(d['cases'])} cases indexed\")"
  log_info "INPUT_EQUIVALENCE_NOT_DEMONSTRATED remains in force for ETEX mini (see fixtures/etex/mini/README.md)"
  log_info "Audit OK"
}

CMD="${1:-all}"
CASE="${2:-all}"
SEEDS="${3:-10}"
# Allow: scripts/run-corpus.sh candidate WIND-UNI-002 --seeds 2
if [ "${CASE}" = "--seeds" ]; then
  SEEDS="${3:-10}"
  CASE="all"
fi
if [ "${2:-}" = "--seeds" ]; then
  SEEDS="${3:-10}"
  CASE="all"
fi

case "${CMD}" in
  candidate) step_candidate "${CASE}" "${SEEDS}" ;;
  oracle) step_oracle "${CASE}" ;;
  oracle-repeatability) step_oracle_repeatability "${CASE:-all}" "${3:-${REPEAT_MINIMUM}}" ;;
  compare) step_compare ;;
  manifest) step_manifest ;;
  audit) step_audit ;;
  all)
    if ! command -v docker >/dev/null 2>&1; then
      log_error "Docker is required for the paired oracle workflow; use 'candidate' for a candidate-only run"
      exit 1
    fi
    if [ ! -d "${FLEXPART_DIR}/src" ]; then
      log_error "Fortran checkout not found at ${FLEXPART_DIR}; use 'candidate' for a candidate-only run"
      exit 1
    fi
    step_candidate "all" "${SEEDS}"
    step_oracle "all"
    step_compare
    step_manifest
    ;;
  *) echo "Usage: $0 [candidate|oracle|oracle-repeatability|compare|manifest|audit|all] [CASE] [--seeds N]"; exit 2 ;;
esac
