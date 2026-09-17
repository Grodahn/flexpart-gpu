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
FLEXPART_DIR="${PROJECT_ROOT}/../flexpart"
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
  *) echo "Usage: $0 [candidate|oracle|compare|manifest|audit|all] [CASE] [--seeds N]"; exit 2 ;;
esac
