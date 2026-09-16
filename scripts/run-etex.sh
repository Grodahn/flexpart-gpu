#!/usr/bin/env bash
set -euo pipefail

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
ERA5_RAW="${ETEX_DIR}/era5_raw"
METEO_DIR="${ETEX_DIR}/meteo"
FORTRAN_RUN="${ETEX_DIR}/fortran_run"
GPU_OUTPUT="${ETEX_DIR}/gpu_output.json"
MEASUREMENTS="${ETEX_DIR}/measurements.json"
REPORT="${ETEX_DIR}/comparison_report.json"
DATA_DIR="${PROJECT_ROOT}/fixtures/etex/data"
CONFIG_DIR="${PROJECT_ROOT}/fixtures/etex/real/config"

C_FLEXPART="/workspace/flexpart"

RED='\033[0;31m'; GREEN='\033[0;32m'; BLUE='\033[0;34m'; NC='\033[0m'
log_info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }
log_step()  { echo -e "\n${BLUE}=== Step: $* ===${NC}"; }

fortran_run_succeeded() {
    [ -f "${ETEX_DIR}/fortran.log" ] && grep -q "CONGRATULATIONS" "${ETEX_DIR}/fortran.log" 2>/dev/null
}

# ---------------------------------------------------------------------------
check_prereqs() {
    if ! command -v python3 &>/dev/null; then
        log_error "python3 not found"
        return 1
    fi
    if ! python3 -c "import eccodes, numpy, xarray, gcsfs, zarr" 2>/dev/null; then
        log_error "Python needs eccodes, numpy, xarray, gcsfs and zarr"
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

    if [ -f "${ERA5_RAW}/metadata.json" ] && [ -f "${ERA5_RAW}/times.npy" ] \
        && [ -f "${ERA5_RAW}/u_component_of_wind.npy" ]; then
        log_info "ERA5 data already downloaded. Skipping."
        return 0
    fi

    if ! python3 -c "import numpy, xarray, gcsfs, zarr" 2>/dev/null; then
        log_error "ERA5 download needs numpy, xarray, gcsfs and zarr"
        return 1
    fi

    python3 "${PROJECT_ROOT}/scripts/etex/download_era5_gcs.py" \
        --output-dir "${ERA5_RAW}"
}

# ---------------------------------------------------------------------------
step_prepare() {
    log_step "Prepare FLEXPART input"
    mkdir -p "${METEO_DIR}"

    if [ ! -f "${ERA5_RAW}/metadata.json" ] || [ ! -f "${ERA5_RAW}/times.npy" ]; then
        log_error "ERA5 data not downloaded. Run: scripts/run-etex.sh download"
        return 1
    fi

    python3 "${PROJECT_ROOT}/scripts/etex/prepare_flexpart_input_from_npy.py" \
        --era5-dir "${ERA5_RAW}" \
        --output-dir "${METEO_DIR}"
    python3 "${PROJECT_ROOT}/scripts/etex/prepare_gpu_meteo.py" \
        --era5-dir "${ERA5_RAW}" \
        --output-dir "${ETEX_DIR}/gpu_meteo"
}

# ---------------------------------------------------------------------------
step_parse() {
    log_step "Parse ETEX-1 measurements"

    if [ ! -f "${DATA_DIR}/meas-t1.txt" ]; then
        log_error "ETEX measurement data not found at ${DATA_DIR}/meas-t1.txt"
        return 1
    fi

    python3 "${PROJECT_ROOT}/scripts/etex/parse_measurements.py" \
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
        -v "${ETEX_DIR}:/workspace/etex" \
        flexpart-fortran bash -c "
            set -euo pipefail
            cd ${C_FLEXPART}/src
            make -f makefile_gfortran clean >/dev/null 2>&1 || true
            FC=gfortran make -f makefile_gfortran eta=no -j\"$(nproc)\" 2>&1 | tail -5
            test -x FLEXPART
            rm -f gitversion.txt
        "

    log_info "Running FLEXPART Fortran (ETEX-1)..."
    docker compose -f "${FORTRAN_COMPOSE_FILE}" run --rm \
        -v "${ETEX_DIR}:/workspace/etex" \
        flexpart-fortran bash -c "
            set -euo pipefail
            cd /workspace/etex/fortran_run && ${C_FLEXPART}/src/FLEXPART
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
    RUST_LOG=info \
        "${PROJECT_ROOT}/target/release/etex-run" 2>&1 \
        | tee "${ETEX_DIR}/gpu.log"

    test -s "${GPU_OUTPUT}"
    log_info "GPU output: ${GPU_OUTPUT}"
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
    if [ -n "$(git -C "${PROJECT_ROOT}" status --porcelain)" ]; then
        candidate_dirty_args=(--candidate-dirty)
    fi

    python3 "${PROJECT_ROOT}/scripts/etex/compare_oracle_observations.py" \
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
    check_file "${ERA5_RAW}/metadata.json"                      "ERA5 source metadata"
    check_file "${ERA5_RAW}/times.npy"                          "ERA5 time axis"
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

    echo ""

    echo ""
}

# ---------------------------------------------------------------------------
step_report() {
    log_step "Validation Report"
    if [ -f "${REPORT}" ]; then
        python3 -c "
import json, sys
with open('${REPORT}') as f:
    r = json.load(f)
print(json.dumps({key: value for key, value in r.items() if key != 'pairs'}, indent=2))
"
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
    gpu)      step_gpu ;;
    compare)  step_compare ;;
    report)   step_report ;;
    all|all-with-fortran)
        check_prereqs
        step_parse
        step_download
        step_prepare
        step_fortran
        step_gpu
        step_compare
        step_report
        ;;
    *)
        echo "Usage: scripts/run-etex.sh [all|all-with-fortran|status|download|prepare|parse|fortran|gpu|compare|report]"
        exit 2
        ;;
esac
