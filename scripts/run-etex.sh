#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "mini" ]; then
    ETEX_PROFILE=mini
    set -- all
fi
ETEX_PROFILE="${ETEX_PROFILE:-full}"

# ===========================================================================
# ETEX-1 Real Data Validation Pipeline
#
# Paired workflow: prepare both models from the same independent ERA5 arrays,
# run the pinned FLEXPART 11.1 oracle and the WGSL candidate, then compare
# complete three-hour concentration windows with ETEX station measurements.
#
# Usage:
#   scripts/run-etex.sh [step]
#
# Steps:
#   all              Run both models and the paired observation comparison
#   all-with-fortran Alias for all
#   download         Download public ARCO-ERA5 arrays
#   prepare          Prepare both model inputs from ERA5
#   parse        Parse ETEX measurements
#   fortran      Run Fortran FLEXPART
#   audit        Check paired mini-run inputs and list unresolved differences
#   gpu          Run flexpart-gpu
#   compare      Compare outputs against observations
#   report       Print final report
#   status       Show current pipeline status
#
# Prerequisites:
#   - Python 3 with eccodes, numpy, xarray, gcsfs and zarr
#   - Rust toolchain (for flexpart-gpu)
#   - Docker + docker-compose for the FLEXPART 11.1 oracle
# ===========================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
FLEXPART_DIR="${PROJECT_ROOT}/../flexpart"
# Optional override for the legacy external sibling layout
# (../flexpart-fortran-docker/docker-compose.yml). When unset, the in-fork
# oracle environment (docker/docker-compose.fortran.yml) is used.
FORTRAN_DOCKER_DIR="${FORTRAN_DOCKER_DIR:-}"
FORTRAN_COMPOSE_FILE="${PROJECT_ROOT}/docker/docker-compose.fortran.yml"
if [ -n "${FORTRAN_DOCKER_DIR}" ]; then
    FORTRAN_COMPOSE_FILE="${FORTRAN_DOCKER_DIR}/docker-compose.yml"
fi

ETEX_DIR="${PROJECT_ROOT}/target/etex"
if [ "${ETEX_PROFILE}" = "mini" ]; then
    ETEX_DIR="${ETEX_DIR}/mini"
fi
ERA5_RAW="${ETEX_DIR}/era5_raw"
if [ "${ETEX_PROFILE}" = "mini" ]; then
    ERA5_RAW="${PROJECT_ROOT}/fixtures/etex/native-mini"
fi
METEO_DIR="${ETEX_DIR}/meteo"
FORTRAN_RUN="${ETEX_DIR}/fortran_run"
GPU_OUTPUT="${ETEX_DIR}/gpu_output.json"
GPU_BINARY="${PROJECT_ROOT}/target/release/etex-run"
if [ "${OS:-}" = "Windows_NT" ]; then
    GPU_BINARY="${GPU_BINARY}.exe"
fi
HOST_PYTHON="${ETEX_PYTHON:-python3}"
if [ "${OS:-}" = "Windows_NT" ]; then
    HOST_PYTHON="${ETEX_PYTHON:-python}"
fi
MEASUREMENTS="${ETEX_DIR}/measurements.json"
REPORT="${ETEX_DIR}/comparison_report.json"
DATA_DIR="${PROJECT_ROOT}/fixtures/etex/data"
CONFIG_DIR="${PROJECT_ROOT}/fixtures/etex/real/config"
if [ "${ETEX_PROFILE}" = "mini" ]; then
    CONFIG_DIR="${PROJECT_ROOT}/fixtures/etex/mini/config"
fi

C_FLEXPART="/workspace/flexpart"
C_ETEX="/workspace/etex"
if [ "${ETEX_PROFILE}" = "mini" ]; then
    C_ETEX="${C_ETEX}/mini"
fi

RED='\033[0;31m'; GREEN='\033[0;32m'; BLUE='\033[0;34m'; NC='\033[0m'
log_info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }
log_step()  { echo -e "\n${BLUE}=== Step: $* ===${NC}"; }

fortran_run_succeeded() {
    [ -f "${ETEX_DIR}/fortran.log" ] && grep -q "CONGRATULATIONS" "${ETEX_DIR}/fortran.log" 2>/dev/null
}

# ---------------------------------------------------------------------------
check_prereqs() {
    if ! command -v "${HOST_PYTHON}" &>/dev/null; then
        log_error "${HOST_PYTHON} not found"
        return 1
    fi
    local modules="eccodes, numpy, xarray, gcsfs, zarr"
    if [ "${ETEX_PROFILE}" = "mini" ]; then
        modules="numpy"
    fi
    if ! "${HOST_PYTHON}" -c "import ${modules}" 2>/dev/null; then
        log_error "Python needs ${modules}"
        return 1
    fi
    if ! command -v docker &>/dev/null; then
        log_error "Docker is required for the pinned FLEXPART 11.1 run"
        return 1
    fi
    if ! command -v cargo &>/dev/null; then
        log_error "Cargo is required for the WGSL candidate run"
        return 1
    fi
}

# ---------------------------------------------------------------------------
step_download() {
    log_step "Download ERA5 data"
    mkdir -p "${ERA5_RAW}"

    if [ "${ETEX_PROFILE}" = "mini" ]; then
        docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm \
            flexpart-fortran python3 \
            /workspace/flexpart-gpu/scripts/etex/verify_native_model_levels.py
        return 0
    fi

    if [ -f "${ERA5_RAW}/metadata.json" ] && [ -f "${ERA5_RAW}/times.npy" ] \
        && [ -f "${ERA5_RAW}/u_component_of_wind.npy" ]; then
        log_info "ERA5 data already downloaded. Skipping."
        return 0
    fi

    if ! "${HOST_PYTHON}" -c "import numpy, xarray, gcsfs, zarr" 2>/dev/null; then
        log_error "ERA5 download needs numpy, xarray, gcsfs and zarr"
        return 1
    fi

    "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/etex/download_era5_gcs.py" \
        --output-dir "${ERA5_RAW}"
}

# ---------------------------------------------------------------------------
step_prepare() {
    log_step "Prepare FLEXPART input"
    mkdir -p "${METEO_DIR}"

    if [ "${ETEX_PROFILE}" != "mini" ] && \
        { [ ! -f "${ERA5_RAW}/metadata.json" ] || [ ! -f "${ERA5_RAW}/times.npy" ]; }; then
        log_error "ERA5 data not downloaded. Run: scripts/run-etex.sh download"
        return 1
    fi

    if [ "${ETEX_PROFILE}" = "mini" ]; then
        docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm \
            flexpart-fortran python3 \
            /workspace/flexpart-gpu/scripts/etex/prepare_native_era5.py \
            --native-dir /workspace/flexpart-gpu/fixtures/etex/native-mini \
            --meteo-dir "${C_ETEX}/meteo" \
            --gpu-dir "${C_ETEX}/gpu_meteo"
    else
        "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/etex/prepare_flexpart_input_from_npy.py" \
            --era5-dir "${ERA5_RAW}" --output-dir "${METEO_DIR}"
        "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/etex/prepare_gpu_meteo.py" \
            --era5-dir "${ERA5_RAW}" --output-dir "${ETEX_DIR}/gpu_meteo"
    fi
}

# ---------------------------------------------------------------------------
step_parse() {
    log_step "Parse ETEX-1 measurements"

    if [ ! -f "${DATA_DIR}/meas-t1.txt" ]; then
        log_error "ETEX measurement data not found at ${DATA_DIR}/meas-t1.txt"
        return 1
    fi

    "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/etex/parse_measurements.py" \
        --data-dir "${DATA_DIR}" \
        --output "${MEASUREMENTS}"
}

# ---------------------------------------------------------------------------
step_fortran() {
    log_step "Run FLEXPART Fortran"

    if ! command -v docker &>/dev/null; then
        log_error "docker not found. Fortran step requires Docker."
        return 1
    fi
    if ! docker compose version &>/dev/null; then
        log_error "docker compose is not available."
        return 1
    fi

    if [ ! -d "${FLEXPART_DIR}" ] || [ ! -d "${FLEXPART_DIR}/src" ]; then
        log_error "Fortran checkout not found at ${FLEXPART_DIR}"
        return 1
    fi
    # Fail closed on unpinned or modified oracle sources (RISK-03.3G-01).
    local manifest="${PROJECT_ROOT}/reference/flexpart-11.1.json"
    local pinned actual
    pinned="$(sed -n 's/^[[:space:]]*"pinned_commit": *"\([0-9a-f]*\)".*/\1/p' "${manifest}" | head -1)"
    if ! printf '%s' "${pinned}" | grep -qE '^[0-9a-f]{40}$'; then
        log_error "Could not read pinned_commit from ${manifest}"
        return 1
    fi
    if ! actual="$(git -C "${FLEXPART_DIR}" rev-parse HEAD 2>/dev/null)" \
        || [ "${actual}" != "${pinned}" ] \
        || [ -n "$(git -C "${FLEXPART_DIR}" status --porcelain)" ]; then
        log_error "Fortran checkout is not the pinned unmodified oracle (see docs/reference-environment.md)"
        return 1
    fi
    if [ ! -f "${FORTRAN_COMPOSE_FILE}" ]; then
        log_error "Fortran compose file not found at ${FORTRAN_COMPOSE_FILE}"
        return 1
    fi

    if [ ! -d "${METEO_DIR}" ] || [ ! -f "${METEO_DIR}/AVAILABLE" ]; then
        log_error "Meteorological data not prepared. Run: scripts/run-etex.sh prepare"
        return 1
    fi

    mkdir -p "${FORTRAN_RUN}/options/SPECIES"
    rm -rf "${FORTRAN_RUN}/output"
    mkdir -p "${FORTRAN_RUN}/output"
    rm -f "${ETEX_DIR}/fortran.log"

    cat > "${FORTRAN_RUN}/pathnames" <<'PATHEOF'
./options/
./output/
../meteo/
../meteo/AVAILABLE
============================================
PATHEOF

    cp "${CONFIG_DIR}/COMMAND"     "${FORTRAN_RUN}/options/"
    cp "${CONFIG_DIR}/RELEASES"    "${FORTRAN_RUN}/options/"
    cp "${CONFIG_DIR}/OUTGRID"     "${FORTRAN_RUN}/options/"
    cp "${CONFIG_DIR}/AGECLASSES"  "${FORTRAN_RUN}/options/"
    cp "${CONFIG_DIR}/RECEPTORS"   "${FORTRAN_RUN}/options/"
    cp "${FLEXPART_DIR}/options/PARTOPTIONS" "${FORTRAN_RUN}/options/"

    # Inert tracer species from the upstream Tracer example (all removal
    # disabled; PMCH analog for ETEX-1 dispersion comparison).
    cp "${FLEXPART_DIR}/examples/Tracer/SPECIES/SPECIES_024" "${FORTRAN_RUN}/options/SPECIES/"
    for f in IGBP_int1.dat sfcdata.t sfcdepo.t; do
        if [ -f "${FLEXPART_DIR}/options/${f}" ]; then
            cp "${FLEXPART_DIR}/options/${f}" "${FORTRAN_RUN}/options/"
        fi
    done

    log_info "Building Docker images..."
    cd "${PROJECT_ROOT}"
    docker compose -f "${FORTRAN_COMPOSE_FILE}" build flexpart-fortran

    log_info "Compiling FLEXPART Fortran..."
    docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm \
        flexpart-fortran bash -c "
            set -euo pipefail
            cd ${C_FLEXPART}/src
            make -f makefile_gfortran clean >/dev/null 2>&1 || true
            FC=gfortran make -f makefile_gfortran eta=no arch=x86-64 -j4 2>&1 | tail -5
            test -x FLEXPART
            rm -f gitversion.txt
        "
    # Restore oracle checkout: the makefile modifies tracked src/FLEXPART.f90
    # and creates untracked gitversion.txt. Restore both so the pinned checkout
    # stays clean for subsequent verification steps.
    git -C "${FLEXPART_DIR}" checkout -- src/FLEXPART.f90
    rm -f "${FLEXPART_DIR}/src/gitversion.txt"
    # Verify cleanliness (fail closed, no || true)
    if [ -n "$(git -C "${FLEXPART_DIR}" status --porcelain)" ]; then
        log_error "Oracle checkout not clean after build"
        git -C "${FLEXPART_DIR}" status --porcelain
        exit 1
    fi

    log_info "Running FLEXPART Fortran (ETEX-1)..."
    docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm \
        flexpart-fortran bash -c "
            set -euo pipefail
            cd ${C_ETEX}/fortran_run && ${C_FLEXPART}/src/FLEXPART
        " 2>&1 | tee "${ETEX_DIR}/fortran.log"

    if grep -q "CONGRATULATIONS" "${ETEX_DIR}/fortran.log" 2>/dev/null; then
        log_info "Fortran run completed successfully"
    else
        log_error "Fortran run failed. Check ${ETEX_DIR}/fortran.log"
        return 1
    fi
    test -s "${FORTRAN_RUN}/output/header_txt"
    test -s "${FORTRAN_RUN}/output/dates"
    compgen -G "${FORTRAN_RUN}/output/grid_conc_*_001" > /dev/null
}

# ---------------------------------------------------------------------------
step_gpu() {
    log_step "Run flexpart-gpu"

    if [ ! -f "${ETEX_DIR}/gpu_meteo/manifest.json" ]; then
        log_error "Prepared GPU meteorology is missing. Run: scripts/run-etex.sh prepare"
        return 1
    fi
    log_info "Building GPU ETEX binary..."
    cd "${PROJECT_ROOT}"
    cargo build --release --bin etex-run 2>&1 | tail -3

    log_info "Running flexpart-gpu (ETEX-1)..."
    rm -f "${GPU_OUTPUT}"
    OUTPUT_PATH="${GPU_OUTPUT}" \
    ETEX_MANIFEST="${ETEX_DIR}/gpu_meteo/manifest.json" \
    RUST_LOG=flexpart_gpu=info \
        "${GPU_BINARY}" 2>&1 \
        | tee "${ETEX_DIR}/gpu.log"

    test -s "${GPU_OUTPUT}"
    log_info "GPU output: ${GPU_OUTPUT}"
}

# ---------------------------------------------------------------------------
step_audit() {
    if [ "${ETEX_PROFILE}" != "mini" ]; then
        log_error "The input equivalence audit currently supports only the native ERA5 mini profile"
        return 2
    fi
    log_step "Audit paired ETEX inputs"
    if [ ! -f "${FORTRAN_RUN}/options/SPECIES/SPECIES_024" ] || \
        [ ! -f "${ETEX_DIR}/gpu_meteo/manifest.json" ]; then
        log_error "Prepare inputs and run the Fortran step before auditing"
        return 1
    fi
    docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm flexpart-fortran python3 \
        /workspace/flexpart-gpu/scripts/etex/audit_input_equivalence.py \
        --native-dir /workspace/flexpart-gpu/fixtures/etex/native-mini \
        --meteo-dir "${C_ETEX}/meteo" \
        --gpu-dir "${C_ETEX}/gpu_meteo" \
        --config-dir "${C_ETEX}/fortran_run/options" \
        --thresholds /workspace/flexpart-gpu/fixtures/etex/mini/input-equivalence-thresholds.json \
        --output "${C_ETEX}/input_equivalence_report.json"
}

# ---------------------------------------------------------------------------
step_compare() {
    log_step "Compare with observations"

    if [ ! -f "${MEASUREMENTS}" ]; then
        log_error "Measurements not parsed. Run: scripts/run-etex.sh parse"
        return 1
    fi

    if ! fortran_run_succeeded || [ ! -d "${FORTRAN_RUN}/output" ]; then
        log_error "Pinned FLEXPART 11.1 output is missing or incomplete"
        return 1
    fi
    if [ ! -s "${GPU_OUTPUT}" ]; then
        log_error "GPU ETEX output is missing"
        return 1
    fi
    local pinned actual candidate_revision
    pinned="$(sed -n 's/^[[:space:]]*"pinned_commit": *"\([0-9a-f]*\)".*/\1/p' "${PROJECT_ROOT}/reference/flexpart-11.1.json" | head -1)"
    actual="$(git -C "${FLEXPART_DIR}" rev-parse HEAD 2>/dev/null)" || return 1
    if [ "${actual}" != "${pinned}" ] || [ -n "$(git -C "${FLEXPART_DIR}" status --porcelain)" ]; then
        log_error "Current FLEXPART checkout is not the pinned unmodified oracle"
        return 1
    fi
    candidate_revision="$(git -C "${PROJECT_ROOT}" rev-parse HEAD)"
    local -a candidate_dirty_args=()
    local -a audit_artifact_args=()
    if [ -n "$(git -C "${PROJECT_ROOT}" status --porcelain)" ]; then
        candidate_dirty_args=(--candidate-dirty)
    fi
    if [ "${ETEX_PROFILE}" = "mini" ]; then
        test -s "${ETEX_DIR}/input_equivalence_report.json"
        audit_artifact_args=(--artifact "${ETEX_DIR}/input_equivalence_report.json")
    fi

    "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/etex/compare_oracle_observations.py" \
        --measurements "${MEASUREMENTS}" \
        --fortran-output "${FORTRAN_RUN}/output" \
        --gpu-output "${GPU_OUTPUT}" \
        --era5-dir "${ERA5_RAW}" \
        --gpu-manifest "${ETEX_DIR}/gpu_meteo/manifest.json" \
        --gpu-log "${ETEX_DIR}/gpu.log" \
        --fortran-log "${ETEX_DIR}/fortran.log" \
        --oracle-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
        --candidate-revision "${candidate_revision}" \
        "${candidate_dirty_args[@]}" \
        --output "${REPORT}"

    "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/write_oracle_run_manifest.py" \
        --output "${ETEX_DIR}/run_manifest.json" \
        --scenario ETEX-1 \
        --oracle-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
        --oracle-checkout "${FLEXPART_DIR}" \
        --oracle-executable "${FLEXPART_DIR}/src/FLEXPART" \
        --candidate-checkout "${PROJECT_ROOT}" \
        --candidate-executable "${GPU_BINARY}" \
        --candidate-log "${ETEX_DIR}/gpu.log" \
        --input "${ERA5_RAW}" \
        --input "${METEO_DIR}" \
        --input "${ETEX_DIR}/gpu_meteo" \
        --input "${CONFIG_DIR}" \
        --input "${DATA_DIR}" \
        --artifact "${FORTRAN_RUN}/output" \
        --artifact "${GPU_OUTPUT}" \
        --artifact "${REPORT}" \
        "${audit_artifact_args[@]}" \
        --artifact "${ETEX_DIR}/fortran.log" \
        --artifact "${ETEX_DIR}/gpu.log"
}

# ---------------------------------------------------------------------------
step_status() {
    echo ""
    echo "ETEX-1 Validation Pipeline Status"
    echo "=================================="
    echo ""

    check_file() {
        if [ -e "$1" ]; then
            echo -e "  ${GREEN}[OK]${NC}  $2"
        else
            echo -e "  ${RED}[--]${NC}  $2"
        fi
    }

    check_file "${DATA_DIR}/meas-t1.txt"                        "ETEX measurements (DATEM)"
    check_file "${DATA_DIR}/stations.txt"                        "ETEX stations (DATEM)"
    if [ "${ETEX_PROFILE}" = "mini" ]; then
        check_file "${ERA5_RAW}/era5-19941023-151821-ml.grib"   "ERA5 model levels, day 1"
        check_file "${ERA5_RAW}/era5-19941024-000306-ml.grib"   "ERA5 model levels, day 2"
        check_file "${ERA5_RAW}/era5-19941023-151821-etadot.grib" "ERA5 eta velocity, day 1"
        check_file "${ERA5_RAW}/era5-19941024-000306-etadot.grib" "ERA5 eta velocity, day 2"
        check_file "${ERA5_RAW}/era5-surface-19941023-24.npz"  "ERA5 surface fields"
    else
        check_file "${ERA5_RAW}/metadata.json"                  "ERA5 source metadata"
        check_file "${ERA5_RAW}/times.npy"                      "ERA5 time axis"
    fi
    check_file "${METEO_DIR}/AVAILABLE"                          "FLEXPART input prepared"
    check_file "${ETEX_DIR}/gpu_meteo/manifest.json"             "GPU input prepared"
    check_file "${MEASUREMENTS}"                                 "Measurements parsed"
    if fortran_run_succeeded; then
        echo -e "  ${GREEN}[OK]${NC}  Fortran FLEXPART run"
    else
        echo -e "  ${RED}[--]${NC}  Fortran FLEXPART run"
    fi
    check_file "${GPU_OUTPUT}"                                   "GPU run"
    check_file "${REPORT}"                                       "Comparison report"
    if [ "${ETEX_PROFILE}" = "mini" ]; then
        check_file "${ETEX_DIR}/input_equivalence_report.json"   "Input equivalence audit"
    fi
    check_file "${ETEX_DIR}/run_manifest.json"                     "Run provenance manifest"

    echo ""

    echo ""
}

# ---------------------------------------------------------------------------
step_report() {
    log_step "Validation Report"
    if [ -f "${REPORT}" ]; then
        local report_path="${REPORT}"
        if [ "${OS:-}" = "Windows_NT" ]; then
            report_path="$(cygpath -w "${REPORT}")"
        fi
        "${HOST_PYTHON}" -c '
import json, sys
with open(sys.argv[1]) as f:
    r = json.load(f)
print(json.dumps({key: value for key, value in r.items() if key != "pairs"}, indent=2))
' "${report_path}"
    else
        log_error "No report found. Run: scripts/run-etex.sh compare"
    fi
}

# ---------------------------------------------------------------------------
STEP="${1:-all}"

case "${STEP}" in
    status)   step_status ;;
    download) step_download ;;
    prepare)  step_prepare ;;
    parse)    step_parse ;;
    fortran)  step_fortran ;;
    audit)    step_audit ;;
    gpu)      step_gpu ;;
    compare)  step_compare ;;
    report)   step_report ;;
    all|all-with-fortran)
        check_prereqs
        step_parse
        step_download
        step_prepare
        step_fortran
        if [ "${ETEX_PROFILE}" = "mini" ]; then step_audit; fi
        step_gpu
        step_compare
        step_report
        ;;
    *)
        echo "Usage: scripts/run-etex.sh [all|all-with-fortran|status|download|prepare|parse|fortran|audit|gpu|compare|report]"
        exit 2
        ;;
esac
