#!/usr/bin/env bash
set -euo pipefail

# ---------------------------------------------------------------------------
# FLEXPART Fortran vs flexpart-gpu comparison
#
# Modes:
#   compose  — Docker containers (default, recommended)
#   nvidia   — Docker with NVIDIA GPU passthrough
#   local    — run directly on host (needs gfortran, eccodes, python3)
#
# Usage:
#   scripts/compare-fortran.sh [compose|nvidia|local] [setup|run|clean|all|validate|perf]
#
# Quick start:
#   scripts/compare-fortran.sh compose all
#
# Scientific validation (Fortran vs GPU concentration comparison):
#   scripts/compare-fortran.sh compose validate
#
# GPU performance profile (strict vs production + profiled timeloop):
#   scripts/compare-fortran.sh compose perf
# ---------------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
FLEXPART_DIR="${PROJECT_ROOT}/../flexpart"
# Optional override for the legacy external sibling layout
# (../flexpart-fortran-docker/docker-compose.yml). When unset, the in-fork
# oracle environment (docker/docker-compose.fortran.yml) is used.
FORTRAN_DOCKER_DIR="${FORTRAN_DOCKER_DIR:-}"
GPU_COMPOSE_FILE="${PROJECT_ROOT}/docker/docker-compose.yml"
GPU_NVIDIA_COMPOSE_FILE="${PROJECT_ROOT}/docker/docker-compose.nvidia.yml"
FORTRAN_COMPOSE_FILE="${PROJECT_ROOT}/docker/docker-compose.fortran.yml"

C_FLEXPART="/workspace/flexpart"
C_GPU="/workspace/flexpart-gpu"
C_DATA="/workspace/comparison"
CANDIDATE_BINARY="${PROJECT_ROOT}/target/release/fortran-validation"
HOST_PYTHON=python3
if [ "${OS:-}" = "Windows_NT" ]; then
  CANDIDATE_BINARY="${CANDIDATE_BINARY}.exe"
  HOST_PYTHON=python
fi

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
log_info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

require_fortran_stack() {
  if [ ! -d "${FLEXPART_DIR}" ] || [ ! -d "${FLEXPART_DIR}/src" ]; then
    log_error "Fortran checkout not found at ${FLEXPART_DIR}"
    log_error "Clone the pinned oracle as sibling directory (see docs/reference-environment.md)."
    return 1
  fi
  if [ -n "${FORTRAN_DOCKER_DIR}" ]; then
    if [ ! -f "${FORTRAN_DOCKER_DIR}/docker-compose.yml" ]; then
      log_error "FORTRAN_DOCKER_DIR is set but has no docker-compose.yml: ${FORTRAN_DOCKER_DIR}"
      return 1
    fi
    FORTRAN_COMPOSE_FILE="${FORTRAN_DOCKER_DIR}/docker-compose.yml"
  fi
  if [ ! -f "${FORTRAN_COMPOSE_FILE}" ]; then
    log_error "Fortran compose file not found at ${FORTRAN_COMPOSE_FILE}"
    return 1
  fi
  return 0
}

# Fail-closed check: the Fortran checkout must be an unmodified upstream
# tree at the commit pinned in reference/flexpart-11.1.json (RISK-03.3G-01).
require_pinned_fortran() {
  local manifest="${PROJECT_ROOT}/reference/flexpart-11.1.json"
  if [ ! -f "${manifest}" ]; then
    log_error "Oracle manifest not found at ${manifest}"
    return 1
  fi
  local pinned
  pinned="$(sed -n 's/^[[:space:]]*"pinned_commit": *"\([0-9a-f]*\)".*/\1/p' "${manifest}" | head -1)"
  if ! printf '%s' "${pinned}" | grep -qE '^[0-9a-f]{40}$'; then
    log_error "Could not read pinned_commit from ${manifest}"
    return 1
  fi
  local actual
  if ! actual="$(git -C "${FLEXPART_DIR}" rev-parse HEAD 2>/dev/null)"; then
    log_error "${FLEXPART_DIR} is not a git checkout"
    return 1
  fi
  if [ "${actual}" != "${pinned}" ]; then
    log_error "Fortran checkout is at ${actual}, expected pinned ${pinned}"
    log_error "Check out the exact pinned commit; see docs/reference-environment.md"
    return 1
  fi
  if [ -n "$(git -C "${FLEXPART_DIR}" status --porcelain)" ]; then
    log_error "Fortran checkout has uncommitted changes; the oracle must stay unmodified"
    return 1
  fi
  log_info "Fortran oracle pinned at ${pinned} (clean)"
  return 0
}

# ---------------------------------------------------------------------------
# Fortran Docker comes from this repository (docker/docker-compose.fortran.yml);
# a legacy external sibling layout can be selected via FORTRAN_DOCKER_DIR.
# GPU Docker is in this project
fortran_compose_cmd() { local m="$1"; shift
  docker compose -f "${FORTRAN_COMPOSE_FILE}" "$@"
}
gpu_compose_cmd() { local m="$1"; shift
  case "$m" in
    compose) docker compose -f "${GPU_COMPOSE_FILE}" "$@" ;;
    nvidia)  docker compose -f "${GPU_COMPOSE_FILE}" -f "${GPU_NVIDIA_COMPOSE_FILE}" "$@" ;;
  esac
}
fortran_exec() { local m="$1"; shift; fortran_compose_cmd "$m" run --rm flexpart-fortran "$@"; }
gpu_exec()     { local m="$1"; shift; gpu_compose_cmd "$m" run --rm flexpart-gpu "$@"; }

# ---------------------------------------------------------------------------
# SETUP
# ---------------------------------------------------------------------------
do_setup() {
  local mode="$1"

  mkdir -p "${PROJECT_ROOT}/target/comparison" "${PROJECT_ROOT}/target/etex"
  require_pinned_fortran
  log_info "Building Docker images..."
  fortran_compose_cmd "$mode" build
  gpu_compose_cmd "$mode" build

  log_info "Compiling FLEXPART Fortran..."
  fortran_exec "$mode" bash -c "
    set -euo pipefail
    cd ${C_FLEXPART}/src
    make -f makefile_gfortran clean >/dev/null 2>&1 || true
    FC=gfortran make -f makefile_gfortran eta=no arch=x86-64 -j4 2>&1 | tail -3
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

  log_info "Generating synthetic GRIB data..."
  fortran_exec "$mode" python3 ${C_GPU}/scripts/generate_synthetic_grib.py \
    --output-dir "${C_DATA}/meteo" \
    --nx 32 --ny 32 --nz 12 \
    --u-wind 0.5 --v-wind -0.3 --w-wind 0.0 \
    --start-date 20240101 --hours 6

  log_info "Setting up FLEXPART run directory..."
  fortran_exec "$mode" bash -c "
    mkdir -p ${C_DATA}/fortran_run/options/SPECIES
    mkdir -p ${C_DATA}/fortran_run/output

    # pathnames (4 lines: options, output, meteo_dir, available_path)
    cat > ${C_DATA}/fortran_run/pathnames <<'EOF'
./options/
./output/
../meteo/
../meteo/AVAILABLE
============================================
EOF

    # COMMAND
    cat > ${C_DATA}/fortran_run/options/COMMAND <<'EOF'
&COMMAND
 LDIRECT=               1,
 IBDATE=         20240101,
 IBTIME=           000000,
 IEDATE=         20240101,
 IETIME=           010000,
 LOUTSTEP=           3600,
 LOUTAVER=           3600,
 LOUTSAMPLE=          900,
 ITSPLIT=        99999999,
 LSYNCTIME=           900,
 CTL=          -5.0000000,
 IFINE=                 4,
 IOUT=                  1,
 IPOUT=                 0,
 LSUBGRID=              0,
 NXSHIFT=               0,
 LNETCDFOUT=            0,
 LCONVECTION=           0,
 LAGESPECTRA=           0,
 IPIN=                  0,
 IOUTPUTFOREACHRELEASE= 1,
 IFLUX=                 0,
 MDOMAINFILL=           0,
 IND_SOURCE=            1,
 IND_RECEPTOR=          1,
 MQUASILAG=             0,
 NESTED_OUTPUT=         0,
 LINIT_COND=            0,
 SURF_ONLY=             0,
 CBLFLAG=               0,
 OHFIELDS_PATH= \"../../flexin/\",
 /
EOF

    # RELEASES
    cat > ${C_DATA}/fortran_run/options/RELEASES <<'EOF'
&RELEASES_CTRL
 NSPEC      =           1,
 SPECNUM_REL=          24,
 /
&RELEASE
 IDATE1  =       20240101,
 ITIME1  =         000000,
 IDATE2  =       20240101,
 ITIME2  =         000000,
 LON1    =         10.000,
 LON2    =         10.000,
 LAT1    =         10.000,
 LAT2    =         10.000,
 Z1      =         50.000,
 Z2      =         50.000,
 ZKIND   =              1,
 MASS    =       1.0000E0,
 PARTS   =           1000,
 COMMENT =    \"COMPARISON\",
 /
EOF

    # OUTGRID
    cat > ${C_DATA}/fortran_run/options/OUTGRID <<'EOF'
&OUTGRID
 OUTLON0=      0.00,
 OUTLAT0=      0.00,
 NUMXGRID=       32,
 NUMYGRID=       32,
 DXOUT=        1.00,
 DYOUT=        1.00,
 OUTHEIGHTS=  100.0, 500.0, 1000.0,
 /
EOF

    # RECEPTORS (old format, 5 header lines + 0 receptors)
    cat > ${C_DATA}/fortran_run/options/RECEPTORS <<'EOF'
*******************************************************************************
*                                                                             *
*  Input file for the Lagrangian particle dispersion model FLEXPART           *
*                        Please specify receptor points                       *
*******************************************************************************
                0
EOF

    # AGECLASSES
    cat > ${C_DATA}/fortran_run/options/AGECLASSES <<'EOF'
&AGECLASS
 NAGECLASS= 1,
 LAGE= 3600,
 /
EOF

    # Copy static data (landuse, surface params)
    cp ${C_FLEXPART}/options/IGBP_int1.dat  ${C_DATA}/fortran_run/options/
    cp ${C_FLEXPART}/options/sfcdata.t     ${C_DATA}/fortran_run/options/
    cp ${C_FLEXPART}/options/sfcdepo.t     ${C_DATA}/fortran_run/options/
    cp ${C_FLEXPART}/options/PARTOPTIONS   ${C_DATA}/fortran_run/options/

    # Inert tracer species from the upstream Tracer example (all removal
    # disabled). v11.1 ships name-based SPECIES files, so copy the exact
    # numbered file the RELEASES (SPECNUM_REL) refers to.
    cp ${C_FLEXPART}/examples/Tracer/SPECIES/SPECIES_024 ${C_DATA}/fortran_run/options/SPECIES/
  "

  log_info "Setup complete."
}

# ---------------------------------------------------------------------------
# RUN
# ---------------------------------------------------------------------------
do_run() {
  local mode="$1"
  mkdir -p "${PROJECT_ROOT}/target"

  log_info "Running flexpart-gpu CPU vs GPU tests..."
  gpu_exec "$mode" cargo test --test cpu_gpu_comparison -- --nocapture 2>&1 \
    | tee "${PROJECT_ROOT}/target/gpu-comparison.log" || true

  log_info "Running FLEXPART Fortran..."
  fortran_exec "$mode" bash -c "
    cd ${C_DATA}/fortran_run && ${C_FLEXPART}/src/FLEXPART
  " 2>&1 | tee "${PROJECT_ROOT}/target/fortran-comparison.log" || true

  echo ""
  echo "================================================================"
  echo "  FLEXPART Fortran vs flexpart-gpu — Results"
  echo "================================================================"

  # GPU tests
  if grep -q "test result: ok" "${PROJECT_ROOT}/target/gpu-comparison.log" 2>/dev/null; then
    echo -e "  GPU tests:    ${GREEN}PASSED${NC}"
    grep -E "^test " "${PROJECT_ROOT}/target/gpu-comparison.log" | sed 's/^/    /'
  else
    echo -e "  GPU tests:    ${RED}FAILED or NOT RUN${NC}"
  fi
  echo ""

  # Fortran
  if grep -q "CONGRATULATIONS" "${PROJECT_ROOT}/target/fortran-comparison.log" 2>/dev/null; then
    echo -e "  Fortran run:  ${GREEN}COMPLETED${NC}"
    grep -E "Simulated|Particles|CONGRATULATIONS" "${PROJECT_ROOT}/target/fortran-comparison.log" | sed 's/^/    /'
  else
    echo -e "  Fortran run:  ${RED}FAILED${NC}"
    tail -5 "${PROJECT_ROOT}/target/fortran-comparison.log" 2>/dev/null | sed 's/^/    /'
  fi

  echo ""
  echo "  Logs:"
  echo "    target/gpu-comparison.log"
  echo "    target/fortran-comparison.log"
  echo "================================================================"
}

# ---------------------------------------------------------------------------
# VALIDATE — scientific comparison of concentration fields
# ---------------------------------------------------------------------------

# Shared parameters (must match src/bin/fortran-validation.rs constants)
V_U_WIND=5.0
V_V_WIND=-3.0
V_W_WIND=0.0
V_PARTICLES=10000
V_OUTLON0=9.50
V_OUTLAT0=8.50
V_NX=32
V_NY=32
V_DX=0.10
V_DY=0.10

do_validate_setup() {
  local mode="$1"

  mkdir -p "${PROJECT_ROOT}/target/comparison" "${PROJECT_ROOT}/target/etex"
  require_pinned_fortran
  log_info "Building Docker images..."
  fortran_compose_cmd "$mode" build

  log_info "Compiling FLEXPART Fortran..."
  fortran_exec "$mode" bash -c "
    set -euo pipefail
    cd ${C_FLEXPART}/src
    make -f makefile_gfortran clean >/dev/null 2>&1 || true
    FC=gfortran make -f makefile_gfortran eta=no arch=x86-64 -j4 2>&1 | tail -3
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

  log_info "Generating synthetic GRIB data (u=${V_U_WIND}, v=${V_V_WIND})..."
  fortran_exec "$mode" python3 ${C_GPU}/scripts/generate_synthetic_grib.py \
    --output-dir "${C_DATA}/meteo" \
    --nx 32 --ny 32 --nz 12 \
    --u-wind "${V_U_WIND}" --v-wind "${V_V_WIND}" --w-wind "${V_W_WIND}" \
    --start-date 20240101 --hours 6

  log_info "Setting up Fortran validation run directory..."
  fortran_exec "$mode" bash -c "
    rm -rf ${C_DATA}/validate_run
    mkdir -p ${C_DATA}/validate_run/options/SPECIES
    mkdir -p ${C_DATA}/validate_run/output

    cat > ${C_DATA}/validate_run/pathnames <<'EOF'
./options/
./output/
../meteo/
../meteo/AVAILABLE
============================================
EOF

    cat > ${C_DATA}/validate_run/options/COMMAND <<'EOF'
&COMMAND
 LDIRECT=               1,
 IBDATE=         20240101,
 IBTIME=           000000,
 IEDATE=         20240101,
 IETIME=           060000,
 LOUTSTEP=           1800,
 LOUTAVER=           1800,
 LOUTSAMPLE=          900,
 ITSPLIT=        99999999,
 LSYNCTIME=           900,
 CTL=          -5.0000000,
 IFINE=                 4,
 IOUT=                  1,
 IPOUT=                 2,
 LSUBGRID=              0,
 NXSHIFT=               0,
 LNETCDFOUT=            0,
 LCONVECTION=           0,
 LAGESPECTRA=           0,
 IPIN=                  0,
 IOUTPUTFOREACHRELEASE= 1,
 IFLUX=                 0,
 MDOMAINFILL=           0,
 IND_SOURCE=            1,
 IND_RECEPTOR=          1,
 MQUASILAG=             0,
 NESTED_OUTPUT=         0,
 LINIT_COND=            0,
 SURF_ONLY=             0,
 CBLFLAG=               0,
 OHFIELDS_PATH= \"../../flexin/\",
 /
EOF

    cat > ${C_DATA}/validate_run/options/RELEASES <<'EOF'
&RELEASES_CTRL
 NSPEC      =           1,
 SPECNUM_REL=          24,
 /
&RELEASE
 IDATE1  =       20240101,
 ITIME1  =         000000,
 IDATE2  =       20240101,
 ITIME2  =         000000,
 LON1    =         10.000,
 LON2    =         10.000,
 LAT1    =         10.000,
 LAT2    =         10.000,
 Z1      =         50.000,
 Z2      =         50.000,
 ZKIND   =              1,
 MASS    =       1.0000E0,
 PARTS   =         ${V_PARTICLES},
 COMMENT =    \"VALIDATION\",
 /
EOF

    cat > ${C_DATA}/validate_run/options/OUTGRID <<EOF
&OUTGRID
 OUTLON0=      ${V_OUTLON0},
 OUTLAT0=      ${V_OUTLAT0},
 NUMXGRID=       ${V_NX},
 NUMYGRID=       ${V_NY},
 DXOUT=        ${V_DX},
 DYOUT=        ${V_DY},
 OUTHEIGHTS=  100.0, 250.0, 500.0, 750.0, 1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 5000.0,
 /
EOF

    cat > ${C_DATA}/validate_run/options/RECEPTORS <<'EOF'
*******************************************************************************
*                                                                             *
*  Input file for the Lagrangian particle dispersion model FLEXPART           *
*                        Please specify receptor points                       *
*******************************************************************************
                0
EOF

    cat > ${C_DATA}/validate_run/options/AGECLASSES <<'EOF'
&AGECLASS
 NAGECLASS= 1,
 LAGE= 21600,
 /
EOF

    cp ${C_FLEXPART}/options/IGBP_int1.dat  ${C_DATA}/validate_run/options/
    cp ${C_FLEXPART}/options/sfcdata.t     ${C_DATA}/validate_run/options/
    cp ${C_FLEXPART}/options/sfcdepo.t     ${C_DATA}/validate_run/options/
    cp ${C_FLEXPART}/options/PARTOPTIONS   ${C_DATA}/validate_run/options/
    cp ${C_FLEXPART}/examples/Tracer/SPECIES/SPECIES_024 ${C_DATA}/validate_run/options/SPECIES/
  "

  log_info "Validation setup complete."
}

do_validate() {
  local mode="$1"
  mkdir -p "${PROJECT_ROOT}/target/validation"

  # Step 1: Setup
  do_validate_setup "$mode"

  # Step 2: Run Fortran FLEXPART
  log_info "Running FLEXPART Fortran (validation)..."
  fortran_exec "$mode" bash -c "
    set -euo pipefail
    cd ${C_DATA}/validate_run && ${C_FLEXPART}/src/FLEXPART
  " 2>&1 | tee "${PROJECT_ROOT}/target/validation/fortran.log"

  if grep -q "CONGRATULATIONS" "${PROJECT_ROOT}/target/validation/fortran.log" 2>/dev/null; then
    log_info "Fortran run completed successfully"
  else
    log_error "Fortran run failed"
    tail -10 "${PROJECT_ROOT}/target/validation/fortran.log"
    exit 1
  fi

  # Step 3: Build and run GPU validation binary
  log_info "Building GPU validation binary..."
  cargo build --release --bin fortran-validation 2>&1 \
    | tee "${PROJECT_ROOT}/target/validation/gpu-build.log"

  log_info "Running GPU validation..."
  OUTPUT_PATH="${PROJECT_ROOT}/target/validation/gpu_concentration.json" \
    PARTICLES=${V_PARTICLES} \
    RUST_LOG=info \
    cargo run --release --bin fortran-validation 2>&1 \
    | tee "${PROJECT_ROOT}/target/validation/gpu.log"

  # Step 4: Fortran output is now at the bind-mount path
  local FORTRAN_OUTPUT="${PROJECT_ROOT}/target/comparison/validate_run/output"
  if [ ! -d "${FORTRAN_OUTPUT}" ]; then
    log_error "Fortran output not found at ${FORTRAN_OUTPUT}. Check volume mounts."
    exit 1
  fi
  log_info "Fortran output at ${FORTRAN_OUTPUT}"
  ls -la "${FORTRAN_OUTPUT}/" | head -20

  # Step 5: Run comparison
  log_info "Comparing concentration fields..."
  if [ "$mode" = "local" ]; then
    "${HOST_PYTHON}" "${SCRIPT_DIR}/compare_concentrations.py" \
      --fortran-output "${FORTRAN_OUTPUT}" \
      --gpu-output "${PROJECT_ROOT}/target/validation/gpu_concentration.json" \
      --output-json "${PROJECT_ROOT}/target/validation/comparison_report.json" \
      --verbose 2>&1 | tee "${PROJECT_ROOT}/target/validation/comparison.log"
  else
    fortran_exec "$mode" python3 "${C_GPU}/scripts/compare_concentrations.py" \
      --fortran-output "${C_DATA}/validate_run/output" \
      --gpu-output "${C_GPU}/target/validation/gpu_concentration.json" \
      --output-json "${C_GPU}/target/validation/comparison_report.json" \
      --verbose 2>&1 | tee "${PROJECT_ROOT}/target/validation/comparison.log"
  fi

  if [ "$mode" != "local" ]; then
    "${HOST_PYTHON}" "${SCRIPT_DIR}/write_oracle_run_manifest.py" \
      --output "${PROJECT_ROOT}/target/validation/run_manifest.json" \
      --scenario synthetic-uniform-wind \
      --oracle-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
      --oracle-checkout "${FLEXPART_DIR}" \
      --oracle-executable "${FLEXPART_DIR}/src/FLEXPART" \
      --candidate-checkout "${PROJECT_ROOT}" \
      --candidate-executable "${CANDIDATE_BINARY}" \
      --candidate-log "${PROJECT_ROOT}/target/validation/gpu.log" \
      --input "${PROJECT_ROOT}/target/comparison/meteo" \
      --input "${PROJECT_ROOT}/target/comparison/validate_run/options" \
      --artifact "${FORTRAN_OUTPUT}" \
      --artifact "${PROJECT_ROOT}/target/validation/gpu_concentration.json" \
      --artifact "${PROJECT_ROOT}/target/validation/comparison_report.json" \
      --artifact "${PROJECT_ROOT}/target/validation/fortran.log" \
      --artifact "${PROJECT_ROOT}/target/validation/gpu.log"
  fi

  log_info "Validation complete. Results in target/validation/"
}

# ---------------------------------------------------------------------------
# PERF — GPU-only performance profile (strict vs production)
# ---------------------------------------------------------------------------
do_perf() {
  local mode="$1"
  mkdir -p "${PROJECT_ROOT}/target/validation"

  local particles="${PERF_PARTICLES:-1000000}"
  local warmup_steps="${PERF_WARMUP_STEPS:-5}"
  local measure_steps="${PERF_MEASURE_STEPS:-30}"

  log_info "GPU performance profile (mode=${mode}, particles=${particles})"
  log_info "Output directory: target/validation/"

  log_info "Run 1/3: strict GPU baseline (SYNC_READBACK=1)"
  gpu_exec "$mode" bash -c "
    OUTPUT_PATH=${C_GPU}/target/validation/perf_gpu_${particles}_strict.json \
    PARTICLES=${particles} \
    SYNC_READBACK=1 \
    /usr/bin/time -f 'REAL_SECONDS=%e' \
    cargo run --release --bin fortran-validation
  " 2>&1 | tee "${PROJECT_ROOT}/target/validation/perf_gpu_${particles}_strict.log"

  log_info "Run 2/3: production GPU baseline (SYNC_READBACK=0)"
  gpu_exec "$mode" bash -c "
    OUTPUT_PATH=/dev/null \
    PARTICLES=${particles} \
    SYNC_READBACK=0 \
    /usr/bin/time -f 'REAL_SECONDS=%e' \
    cargo run --release --bin fortran-validation
  " 2>&1 | tee "${PROJECT_ROOT}/target/validation/perf_gpu_${particles}_prod.log"

  log_info "Run 3/3: profiled timeloop benchmark"
  gpu_exec "$mode" bash -c "
    FLEXPART_GPU_PROFILE=1 \
    PARTICLES=${particles} \
    WARMUP_STEPS=${warmup_steps} \
    MEASURE_STEPS=${measure_steps} \
    cargo run --release --bin bench-timeloop
  " 2>&1 | tee "${PROJECT_ROOT}/target/validation/perf_gpu_${particles}_timeloop_profile.log"

  log_info "Performance profile complete."
  echo "Generated logs:"
  echo "  target/validation/perf_gpu_${particles}_strict.log"
  echo "  target/validation/perf_gpu_${particles}_prod.log"
  echo "  target/validation/perf_gpu_${particles}_timeloop_profile.log"
}

# ---------------------------------------------------------------------------
# CLEAN
# ---------------------------------------------------------------------------
do_clean() {
  local mode="$1"
  log_info "Cleaning up..."
  fortran_compose_cmd "$mode" down -v --remove-orphans 2>/dev/null || true
  gpu_compose_cmd "$mode" down -v --remove-orphans 2>/dev/null || true
  rm -f "${PROJECT_ROOT}/target/gpu-comparison.log"
  rm -f "${PROJECT_ROOT}/target/fortran-comparison.log"
  log_info "Done."
}

# ---------------------------------------------------------------------------
# MAIN
# ---------------------------------------------------------------------------
MODE="${1:-compose}"
ACTION="${2:-all}"

case "$MODE" in
  compose|nvidia)
    cd "${PROJECT_ROOT}"
    case "$ACTION" in
      setup|run|all|validate) require_fortran_stack ;;
    esac
    case "$ACTION" in
      setup)    do_setup    "$MODE" ;;
      run)      do_run      "$MODE" ;;
      clean)    do_clean    "$MODE" ;;
      validate) do_validate "$MODE" ;;
      perf)     do_perf     "$MODE" ;;
      all)      do_setup "$MODE"; do_run "$MODE" ;;
      *)        echo "Usage: $0 $MODE [setup|run|clean|all|validate|perf]"; exit 2 ;;
    esac
    ;;
  local)
    echo "Local mode: ensure gfortran, eccodes, python3 are installed."
    echo "Then adapt the paths in this script. Docker mode is recommended."
    exit 2
    ;;
  *)
    echo "Usage: $0 [compose|nvidia|local] [setup|run|clean|all]"
    exit 2
    ;;
esac
