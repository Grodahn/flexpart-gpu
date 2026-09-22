#!/usr/bin/env bash
set -euo pipefail

# ---------------------------------------------------------------------------
# Small deterministic technical CI gate (RISK-03.3G-01, Issue #6 point 4).
#
# Technical gate only. A TECHNICAL_PASS does not establish scientific parity
# and does not close Issue #6. Scientific thresholds are versioned separately
# and are only applied when inputs and metrics are demonstrably suitable
# (owned by the scientific-metrics track, not by this gate).
#
# What this gate does (fail-closed):
#   1. Verify the pinned, clean FLEXPART 11.1 oracle checkout.
#   2. Build the oracle Docker image and compile the Fortran executable.
#   3. Prove a real software-WGPU adapter via gpu-preflight (no skip allowed).
#   4. Run the analytical SW-WGPU-ADVECTION-001 case on the software adapter.
#   5. Run the small synthetic candidate smoke (fortran-validation, 1000
#      particles by default) on the software adapter.
#   6. Check inputs, outputs and provenance; write a machine-readable
#      ci-gate-report.json plus run_manifest.json and uploadable logs.
#
# Any missing adapter, skipped GPU test, missing oracle artifact, or failed
# comparison fails the job (non-zero exit). Unwired corpus cases are reported
# as NOT_WIRED, never as PASS.
#
# Local reproduction (Linux/Docker, full gate):
#   scripts/ci-gate.sh
#
# Local candidate-only smoke without Docker (oracle build skipped, gate then
# reports INCOMPLETE and exits non-zero by design):
#   scripts/ci-gate.sh --skip-oracle-build
#
# Larger corpus (local only, not part of this per-PR gate):
#   scripts/compare-fortran.sh compose validate
#   scripts/run-etex.sh mini
# See docs/ci-gates.md for prerequisites, runtime and artifact sizes.
#
# Coordination note: the extended test corpus (scripts/run-corpus.sh,
# docs/corpus-matrix.md) and the scientific metric format
# (scripts/evaluate/evaluate_case.py, docs/evaluation.md) are owned by two
# parallel tracks. This script only defines the stable extension point
# (allow-list + NOT_WIRED reporting) and does not implement their cases or
# thresholds.
# ---------------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

OUTPUT_DIR="${PROJECT_ROOT}/target/ci-gate"
PARTICLES="1000"
ORACLE_CHECKOUT="${PROJECT_ROOT}/../flexpart"
SKIP_ORACLE_BUILD="0"
CI_CASE_ALLOWLIST="SW-WGPU-ADVECTION-001 SYNTHETIC-UNIFORM-WIND-SMOKE"

HOST_PYTHON="python3"
if [ "${OS:-}" = "Windows_NT" ]; then
  HOST_PYTHON="python"
fi
CANDIDATE_BINARY="${PROJECT_ROOT}/target/release/fortran-validation"
if [ "${OS:-}" = "Windows_NT" ]; then
  CANDIDATE_BINARY="${CANDIDATE_BINARY}.exe"
fi

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
log_info()  { echo -e "${GREEN}[CI-GATE]${NC}  $*"; }
log_warn()  { echo -e "${YELLOW}[CI-GATE]${NC}  $*"; }
log_error() { echo -e "${RED}[CI-GATE]${NC} $*"; }

usage() {
  cat <<'EOF'
Usage:
  scripts/ci-gate.sh [--output-dir <dir>] [--particles <n>]
                     [--oracle-checkout <dir>] [--skip-oracle-build]

Options:
  --output-dir <dir>       Output directory (default: target/ci-gate).
  --particles <n>          Candidate smoke particle count (default: 1000).
  --oracle-checkout <dir>  Pinned FLEXPART checkout (default: ../flexpart).
  --skip-oracle-build      Skip Docker oracle build (local iteration only;
                           the gate then reports INCOMPLETE and fails).
  -h, --help               Show this help.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --output-dir) OUTPUT_DIR="$2"; shift 2 ;;
    --particles) PARTICLES="$2"; shift 2 ;;
    --oracle-checkout) ORACLE_CHECKOUT="$2"; shift 2 ;;
    --skip-oracle-build) SKIP_ORACLE_BUILD="1"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) log_error "Unknown argument: $1"; usage; exit 2 ;;
  esac
done

mkdir -p "${OUTPUT_DIR}"
GATE_LOG="${OUTPUT_DIR}/ci-gate.log"
exec > >(tee "${GATE_LOG}") 2>&1

log_info "Technical CI gate (no scientific parity verdict)."
log_info "Project root: ${PROJECT_ROOT}"
log_info "Output dir: ${OUTPUT_DIR}"
log_info "Particles: ${PARTICLES}"
log_info "Oracle checkout: ${ORACLE_CHECKOUT}"
log_info "Allow-listed CI cases: ${CI_CASE_ALLOWLIST}"

fail() {
  log_error "$*"
  "${HOST_PYTHON}" "${SCRIPT_DIR}/ci-gate-report.py" \
    --output "${OUTPUT_DIR}/ci-gate-report.json" \
    --project-root "${PROJECT_ROOT}" \
    --output-dir "${OUTPUT_DIR}" \
    --oracle-checkout "${ORACLE_CHECKOUT}" \
    --status "TECHNICAL_FAIL" \
    --failure "$*" || true
  log_error "CI gate: TECHNICAL_FAIL"
  exit 1
}

# ---------------------------------------------------------------------------
# 0. Build environment and revisions (provenance, always recorded).
# ---------------------------------------------------------------------------
log_info "Recording build environment..."
CANDIDATE_REVISION="$(git -C "${PROJECT_ROOT}" rev-parse HEAD 2>/dev/null || echo "unknown")"
CANDIDATE_DIRTY="0"
if [ -n "$(git -C "${PROJECT_ROOT}" status --porcelain 2>/dev/null || true)" ]; then
  CANDIDATE_DIRTY="1"
fi
PINNED_COMMIT="$("${HOST_PYTHON}" -c 'import json; print(json.load(open("reference/flexpart-11.1.json"))["pinned_commit"])' 2>/dev/null || true)"
if [ -z "${PINNED_COMMIT}" ]; then
  # Fallback when invoked outside the project root.
  PINNED_COMMIT="$("${HOST_PYTHON}" -c 'import json; print(json.load(open("'"${PROJECT_ROOT}"'/reference/flexpart-11.1.json"))["pinned_commit"])')"
fi
if ! printf '%s' "${PINNED_COMMIT}" | grep -qE '^[0-9a-f]{40}$'; then
  fail "Could not read pinned_commit from reference/flexpart-11.1.json"
fi
log_info "Candidate revision: ${CANDIDATE_REVISION} (dirty=${CANDIDATE_DIRTY})"
log_info "Pinned oracle commit: ${PINNED_COMMIT}"
{
  echo "candidate_revision=${CANDIDATE_REVISION}"
  echo "candidate_dirty=${CANDIDATE_DIRTY}"
  echo "pinned_commit=${PINNED_COMMIT}"
  echo "github_run_id=${GITHUB_RUN_ID:-local}"
  echo "github_sha=${GITHUB_SHA:-${CANDIDATE_REVISION}}"
  rustc --version 2>/dev/null || echo "rustc=unknown"
  cargo --version 2>/dev/null || echo "cargo=unknown"
  docker --version 2>/dev/null || echo "docker=missing"
  "${HOST_PYTHON}" --version 2>&1 || echo "python=unknown"
  uname -a 2>/dev/null || echo "uname=unknown"
} | tee "${OUTPUT_DIR}/build-env.txt"

# ---------------------------------------------------------------------------
# 1. Verify the pinned, clean oracle checkout (fail-closed).
# ---------------------------------------------------------------------------
log_info "Step 1/6: verify pinned clean oracle checkout..."
if [ ! -d "${ORACLE_CHECKOUT}" ]; then
  fail "Oracle checkout not found at ${ORACLE_CHECKOUT}; clone the pinned oracle as documented in docs/reference-environment.md"
fi
if ! cargo run --bin reference-check -- verify --checkout "${ORACLE_CHECKOUT}" 2>&1 | tee "${OUTPUT_DIR}/oracle-verify.log"; then
  fail "Oracle checkout verification failed (must be pinned ${PINNED_COMMIT} and clean)"
fi
if ! grep -q "reference checkout: OK" "${OUTPUT_DIR}/oracle-verify.log"; then
  fail "Oracle verification log lacks OK marker"
fi
ORACLE_ACTUAL="$(git -C "${ORACLE_CHECKOUT}" rev-parse HEAD 2>/dev/null || echo "unknown")"
if [ "${ORACLE_ACTUAL}" != "${PINNED_COMMIT}" ]; then
  fail "Oracle checkout is at ${ORACLE_ACTUAL}, expected pinned ${PINNED_COMMIT}"
fi
if [ -n "$(git -C "${ORACLE_CHECKOUT}" status --porcelain 2>/dev/null || true)" ]; then
  fail "Oracle checkout has uncommitted changes; the oracle must stay unmodified"
fi
log_info "Oracle pinned at ${ORACLE_ACTUAL} (clean)."

# ---------------------------------------------------------------------------
# 2. Build the oracle Docker image and Fortran executable (fail-closed).
# ---------------------------------------------------------------------------
ORACLE_BUILD_STATUS="PASS"
ORACLE_EXECUTABLE="${ORACLE_CHECKOUT}/src/FLEXPART"
if [ "${SKIP_ORACLE_BUILD}" = "1" ]; then
  ORACLE_BUILD_STATUS="NOT_RUN"
  log_warn "--skip-oracle-build was given; oracle build is NOT_RUN and the gate will report INCOMPLETE."
  echo "NOT_RUN (skipped by --skip-oracle-build)" > "${OUTPUT_DIR}/oracle-build.log"
else
  log_info "Step 2/6: build oracle image and compile FLEXPART..."
  if ! command -v docker >/dev/null 2>&1; then
    fail "Docker is required for the oracle build but was not found"
  fi
  if [ ! -f "${PROJECT_ROOT}/docker/docker-compose.fortran.yml" ]; then
    fail "Fortran compose file not found at docker/docker-compose.fortran.yml"
  fi
  # The oracle compose file defaults to user 1000:1000, but GitHub runners
  # use UID 1001. Override the run user so the bind-mounted checkout stays
  # writable (bash UID is readonly, so pass --user instead of exporting UID).
  DOCKER_USER_ARGS=""
  if command -v id >/dev/null 2>&1; then
    DOCKER_USER_ARGS="--user $(id -u):$(id -g)"
    # shellcheck disable=SC2086
    log_info "Docker run user override: ${DOCKER_USER_ARGS}"
  fi
  {
    echo "=== docker compose build ==="
    docker compose -f "${PROJECT_ROOT}/docker/docker-compose.fortran.yml" build flexpart-fortran
    echo "=== fortran compile ==="
    # shellcheck disable=SC2086
    docker compose -f "${PROJECT_ROOT}/docker/docker-compose.fortran.yml" run --rm \
      ${DOCKER_USER_ARGS} \
      flexpart-fortran bash -c "
        set -euo pipefail
        cd /workspace/flexpart/src
        make -f makefile_gfortran clean >/dev/null 2>&1 || true
        FC=gfortran make -f makefile_gfortran eta=no arch=x86-64 -j4 2>&1 | tail -5
        test -x FLEXPART
        rm -f gitversion.txt
        git checkout -- src/FLEXPART.f90 2>/dev/null || git checkout -- FLEXPART.f90
        rm -f gitversion.txt
      "
    echo "=== compiler and image provenance ==="
    docker run --rm flexpart-fortran:latest gfortran --version | head -1
    docker image inspect flexpart-fortran:latest --format '{{.Id}}'
  } 2>&1 | tee "${OUTPUT_DIR}/oracle-build.log"
  if [ ! -x "${ORACLE_EXECUTABLE}" ]; then
    fail "Oracle executable missing after build: ${ORACLE_EXECUTABLE}"
  fi
  # The v11.1 makefile stamps the git version into the tracked
  # src/FLEXPART.f90 (sed gitversion_tmp) plus an untracked gitversion.txt.
  # Restore both on the host so the oracle stays pristine; the compiled
  # binary itself is git-ignored and remains.
  git -C "${ORACLE_CHECKOUT}" checkout -- src/FLEXPART.f90 2>/dev/null || true
  rm -f "${ORACLE_CHECKOUT}/src/gitversion.txt"
  # The build must leave the oracle checkout clean.
  ORACLE_POST_STATUS="$(git -C "${ORACLE_CHECKOUT}" status --porcelain 2>/dev/null || true)"
  if [ -n "${ORACLE_POST_STATUS}" ]; then
    log_error "Oracle status after build:"
    echo "${ORACLE_POST_STATUS}" | head -20
    # Untracked build leftovers that upstream does not ignore fail closed,
    # but list them explicitly for debugging.
    fail "Oracle checkout is dirty after build; the build must remove generated stamps (see docs/reference-environment.md)"
  fi
  "${HOST_PYTHON}" -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' \
    "${ORACLE_EXECUTABLE}" > "${OUTPUT_DIR}/oracle-executable.sha256"
  test -s "${OUTPUT_DIR}/oracle-executable.sha256" || fail "Could not hash oracle executable"
  log_info "Oracle executable: ${ORACLE_EXECUTABLE} (sha256 recorded)."
fi

# ---------------------------------------------------------------------------
# 2b. Vertical-coordinate oracle column (#30).
# ---------------------------------------------------------------------------
if [ "${SKIP_ORACLE_BUILD}" != "1" ]; then
  log_info "Step 2b/6: direct FLEXPART-11.1 vertical routine oracle comparison..."
  VERTICAL_DIR="${OUTPUT_DIR}/vertical-column"
  VERTICAL_BUILD_DIR="${VERTICAL_DIR}/oracle-build"
  mkdir -p "${VERTICAL_DIR}" "${VERTICAL_BUILD_DIR}"

  if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/vertical/prepare_oracle_column.py" \
    --snapshot "${PROJECT_ROOT}/fixtures/vertical/synthetic-column-v1.json" \
    --motion "${PROJECT_ROOT}/fixtures/vertical/synthetic-omega-interface-v1.json" \
    --output "${VERTICAL_DIR}/oracle-input.txt"; then
    fail "Preparing the #30 vertical oracle input failed"
  fi

  if ! cargo run --quiet --bin vertical-column-report -- \
    "${PROJECT_ROOT}/fixtures/vertical/synthetic-column-v1.json" \
    "${VERTICAL_DIR}/candidate.json" \
    "${PROJECT_ROOT}/fixtures/vertical/synthetic-omega-interface-v1.json"; then
    fail "Candidate #30 vertical-column transform failed"
  fi

  # FLEXPART was compiled in Step 2. Link the focused driver against those exact
  # pristine objects. Exclude only FLEXPART.o because it defines the model's
  # main program; verttransform_mod.o, windfields_mod.o and all transitive module
  # dependencies are the objects produced by the pinned full FLEXPART build.
  if ! docker compose -f "${PROJECT_ROOT}/docker/docker-compose.fortran.yml" run --rm \
    ${DOCKER_USER_ARGS} \
    flexpart-fortran bash -c "
      set -euo pipefail
      build=/workspace/target/ci-gate/vertical-column/oracle-build
      oracle_src=/workspace/flexpart/src
      mkdir -p \"\$build\"
      cd \"\$build\"

      objects=\$(find \"\$oracle_src\" -maxdepth 1 -type f -name '*.o' ! -name 'FLEXPART.o' -print | sort | tr '\\n' ' ')
      test -n \"\$objects\"
      test -f \"\$oracle_src/verttransform_mod.o\"
      test -f \"\$oracle_src/windfields_mod.o\"

      gfortran -O0 -I\"\$oracle_src\" -fopenmp -mcmodel=large \
        /workspace/flexpart-gpu/scripts/vertical/direct_oracle_driver.f90 \
        \$objects \
        -L/usr/lib/x86_64-linux-gnu -Wl,-rpath=/usr/lib/x86_64-linux-gnu \
        -leccodes -leccodes_f90 -lm -lnetcdff \
        -o \"\$build/flexpart-vertical-routine-oracle\"

      nm \"\$build/flexpart-vertical-routine-oracle\" > \
        \"\$build/flexpart-vertical-routine-oracle.nm\"
      grep -q '__verttransform_mod_MOD_verttransform_ecmwf_heights' \
        \"\$build/flexpart-vertical-routine-oracle.nm\"
      sha256sum \"\$oracle_src/verttransform_mod.o\" > \"\$build/verttransform_mod.o.sha256\"
      sha256sum \"\$oracle_src/windfields_mod.o\" > \"\$build/windfields_mod.o.sha256\"

      gfortran -O0 -J\"\$build\" -I\"\$build\" \
        \"\$oracle_src/par_mod.f90\" \
        \"\$oracle_src/qvsat_mod.f90\" \
        /workspace/flexpart-gpu/scripts/vertical/oracle_column.f90 \
        -o \"\$build/vertical-conformance-harness\"

      \"\$build/flexpart-vertical-routine-oracle\" \
        /workspace/target/ci-gate/vertical-column/oracle-input.txt \
        /workspace/target/ci-gate/vertical-column/routine-oracle-output.txt

      \"\$build/vertical-conformance-harness\" \
        /workspace/target/ci-gate/vertical-column/oracle-input.txt \
        /workspace/target/ci-gate/vertical-column/conformance-output.txt
    " 2>&1 | tee "${VERTICAL_DIR}/oracle-build-run.log"; then
    fail "Direct pinned FLEXPART #30 vertical routine oracle build/run failed"
  fi

  if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/vertical/write_direct_oracle_provenance.py" \
    --binary "${VERTICAL_BUILD_DIR}/flexpart-vertical-routine-oracle" \
    --driver-source "${PROJECT_ROOT}/scripts/vertical/direct_oracle_driver.f90" \
    --oracle-checkout "${ORACLE_CHECKOUT}" \
    --reference-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --output "${VERTICAL_DIR}/routine-oracle-provenance.json"; then
    fail "Writing direct FLEXPART routine-oracle provenance failed"
  fi

  if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/vertical/compare_oracle_column.py" \
    --candidate "${VERTICAL_DIR}/candidate.json" \
    --oracle "${VERTICAL_DIR}/routine-oracle-output.txt" \
    --oracle-checkout "${ORACLE_CHECKOUT}" \
    --reference-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --source-snapshot "${PROJECT_ROOT}/fixtures/vertical/synthetic-column-v1.json" \
    --source-motion "${PROJECT_ROOT}/fixtures/vertical/synthetic-omega-interface-v1.json" \
    --expect-execution-mode pinned_routine \
    --oracle-provenance "${VERTICAL_DIR}/routine-oracle-provenance.json" \
    --output "${VERTICAL_DIR}/comparison-report.json"; then
    fail "FLEXPART-11.1 direct-routine synthetic vertical comparison failed"
  fi

  if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/vertical/compare_oracle_column.py" \
    --candidate "${VERTICAL_DIR}/candidate.json" \
    --oracle "${VERTICAL_DIR}/conformance-output.txt" \
    --oracle-checkout "${ORACLE_CHECKOUT}" \
    --reference-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --source-snapshot "${PROJECT_ROOT}/fixtures/vertical/synthetic-column-v1.json" \
    --source-motion "${PROJECT_ROOT}/fixtures/vertical/synthetic-omega-interface-v1.json" \
    --expect-execution-mode conformance_harness \
    --output "${VERTICAL_DIR}/conformance-comparison-report.json"; then
    fail "Secondary #30 vertical conformance comparison failed"
  fi

  test -s "${VERTICAL_DIR}/comparison-report.json" \
    || fail "Normative vertical-column comparison report is missing"
  test -s "${VERTICAL_DIR}/routine-oracle-provenance.json" \
    || fail "Direct FLEXPART routine provenance is missing"

  # Repeat the same two checks on one real 137-level ERA5/ETEX column.
  if ! docker compose -f "${PROJECT_ROOT}/docker/docker-compose.fortran.yml" run --rm \
    ${DOCKER_USER_ARGS} \
    flexpart-fortran python3 /workspace/flexpart-gpu/scripts/vertical/extract_real_etex_column.py \
      --canonical /workspace/flexpart-gpu/fixtures/meteorology/era5-etex-native-v1.json \
      --canonical-provenance /workspace/flexpart-gpu/fixtures/meteorology/era5-etex-native-v1.provenance.json \
      --surface-archive /workspace/flexpart-gpu/fixtures/etex/native-mini/era5-surface-19941023-24.npz \
      --output /workspace/target/ci-gate/vertical-column/real-column-snapshot.json \
      --provenance-output /workspace/target/ci-gate/vertical-column/real-column-fixture-provenance.json \
      2>&1 | tee "${VERTICAL_DIR}/real-column-extract.log"; then
    fail "Extracting the real #30 ERA5/ETEX vertical column failed"
  fi

  if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/vertical/prepare_oracle_column.py" \
    --snapshot "${VERTICAL_DIR}/real-column-snapshot.json" \
    --output "${VERTICAL_DIR}/real-oracle-input.txt"; then
    fail "Preparing the real #30 vertical oracle input failed"
  fi

  if ! cargo run --quiet --bin vertical-column-report -- \
    "${VERTICAL_DIR}/real-column-snapshot.json" \
    "${VERTICAL_DIR}/real-candidate.json"; then
    fail "Candidate real #30 vertical-column transform failed"
  fi

  if ! docker compose -f "${PROJECT_ROOT}/docker/docker-compose.fortran.yml" run --rm \
    ${DOCKER_USER_ARGS} \
    flexpart-fortran bash -c "
      set -euo pipefail
      build=/workspace/target/ci-gate/vertical-column/oracle-build
      \"\$build/flexpart-vertical-routine-oracle\" \
        /workspace/target/ci-gate/vertical-column/real-oracle-input.txt \
        /workspace/target/ci-gate/vertical-column/real-routine-oracle-output.txt
      \"\$build/vertical-conformance-harness\" \
        /workspace/target/ci-gate/vertical-column/real-oracle-input.txt \
        /workspace/target/ci-gate/vertical-column/real-conformance-output.txt
    " 2>&1 | tee "${VERTICAL_DIR}/real-oracle-run.log"; then
    fail "Direct pinned FLEXPART real #30 vertical routine oracle failed"
  fi

  if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/vertical/compare_oracle_column.py" \
    --candidate "${VERTICAL_DIR}/real-candidate.json" \
    --oracle "${VERTICAL_DIR}/real-routine-oracle-output.txt" \
    --oracle-checkout "${ORACLE_CHECKOUT}" \
    --reference-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --source-snapshot "${VERTICAL_DIR}/real-column-snapshot.json" \
    --expect-execution-mode pinned_routine \
    --oracle-provenance "${VERTICAL_DIR}/routine-oracle-provenance.json" \
    --output "${VERTICAL_DIR}/real-comparison-report.json"; then
    fail "FLEXPART-11.1 direct-routine real vertical-column comparison failed"
  fi

  if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/vertical/compare_oracle_column.py" \
    --candidate "${VERTICAL_DIR}/real-candidate.json" \
    --oracle "${VERTICAL_DIR}/real-conformance-output.txt" \
    --oracle-checkout "${ORACLE_CHECKOUT}" \
    --reference-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --source-snapshot "${VERTICAL_DIR}/real-column-snapshot.json" \
    --expect-execution-mode conformance_harness \
    --output "${VERTICAL_DIR}/real-conformance-comparison-report.json"; then
    fail "Secondary real #30 vertical conformance comparison failed"
  fi

  test -s "${VERTICAL_DIR}/real-comparison-report.json" \
    || fail "Real normative vertical-column comparison report is missing"
  test -s "${VERTICAL_DIR}/real-column-fixture-provenance.json" \
    || fail "Real vertical-column fixture provenance is missing"

  log_info "Direct pinned FLEXPART-11.1 synthetic/real vertical routine comparisons and secondary conformance checks passed."
fi

# ---------------------------------------------------------------------------
# 2c. Interpolation oracle contract fixture (#71, RISK-03.3G-10c1).
# ---------------------------------------------------------------------------
# The interpolation contract (docs/interpolation-contract.md) freezes the
# behaviour of find_grid_indices / find_grid_distances / find_z_level_meters /
# find_vert_vars / hor_interpol_4d / temporal_interpolation / vert_interpol /
# interpol_rain on canonical synthetic grids. This step proves the committed
# fixture pack (fixtures/interpolation/contract-v1.json + provenance) is
# reproducible: regenerate inputs from the case definitions, run the direct
# oracle in the pinned container, re-pack the fixture, and require the
# regenerated goldens/provenance to match the committed ones. The committed
# goldens are additionally re-verified analytically by the Rust test
# tests/interpolation_contract.rs (cargo test runs it).
if [ "${SKIP_ORACLE_BUILD}" != "1" ]; then
  log_info "Step 2c/6: regenerate and verify the interpolation oracle contract fixture (#71)..."
  INTERPOL_DIR="${OUTPUT_DIR}/interpolation"
  INTERPOL_BUILD_DIR="${INTERPOL_DIR}/oracle-build"
  mkdir -p "${INTERPOL_DIR}" "${INTERPOL_BUILD_DIR}" "${INTERPOL_DIR}/oracle-output"

  if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/interpolation/prepare_interpolation_fixtures.py" \
    --emit-inputs-only \
    --input-dir "${INTERPOL_DIR}/oracle-input" \
    --output-dir "${INTERPOL_DIR}/oracle-output" \
    --binary "${INTERPOL_BUILD_DIR}/interpolation-oracle" \
    --oracle-checkout "${ORACLE_CHECKOUT}" \
    --reference-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --vertical-routine-oracle-output "${OUTPUT_DIR}/vertical-column/routine-oracle-output.txt" \
    --real-vertical-routine-oracle-output "${OUTPUT_DIR}/vertical-column/real-routine-oracle-output.txt" \
    --out-fixture "${INTERPOL_DIR}/contract-v1.json"; then
    fail "Emitting the #71 interpolation oracle case inputs failed"
  fi

  # Compile the direct driver against the pristine objects produced in Step 2 and
  # run all sampling cases inside the pinned container.
  if ! docker compose -f "${PROJECT_ROOT}/docker/docker-compose.fortran.yml" run --rm \
    ${DOCKER_USER_ARGS} \
    flexpart-fortran bash -c '
      set -euo pipefail
      for name in horizontal-interior horizontal-geographic-interior horizontal-periodic-wrap vertical-model-levels vertical-interface-wzlev temporal-bilinear rain-layer-fields real-era5-etex-temperature-column; do
        bash /workspace/flexpart-gpu/scripts/interpolation/direct_oracle.sh \
          /workspace/target/ci-gate/interpolation/oracle-build \
          "/workspace/target/ci-gate/interpolation/oracle-input/${name}/${name}.txt" \
          "/workspace/target/ci-gate/interpolation/oracle-output/${name}.out"
      done
    ' 2>&1 | tee "${INTERPOL_DIR}/oracle-build-run.log"; then
    fail "Direct pinned FLEXPART #71 interpolation oracle build/run failed"
  fi

  # Re-pack the fixture and provenance from the fresh oracle outputs (also
  # re-verifies the pinned clean checkout and the nm symbol entry points).
  if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/interpolation/prepare_interpolation_fixtures.py" \
    --offline \
    --input-dir "${INTERPOL_DIR}/oracle-input" \
    --output-dir "${INTERPOL_DIR}/oracle-output" \
    --binary "${INTERPOL_BUILD_DIR}/interpolation-oracle" \
    --oracle-checkout "${ORACLE_CHECKOUT}" \
    --reference-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --vertical-routine-oracle-output "${OUTPUT_DIR}/vertical-column/routine-oracle-output.txt" \
    --real-vertical-routine-oracle-output "${OUTPUT_DIR}/vertical-column/real-routine-oracle-output.txt" \
    --out-fixture "${INTERPOL_DIR}/contract-v1.json" \
    --out-provenance "${INTERPOL_DIR}/contract-v1.provenance.json"; then
    fail "Repacking the #71 interpolation fixture/provenance failed"
  fi

  # The committed goldens and provenance sub-fields must be reproducible. Paths
  # and binary hashes are local build locations and may legitimately differ;
  # every semantic field (goldens, per-case output hashes, object hashes,
  # routines, pinning, cleanliness) must match exactly.
  if ! "${HOST_PYTHON}" -c '
import hashlib, json, math, sys

def normalize_json(value):
    if isinstance(value, dict):
        return {key: normalize_json(item) for key, item in value.items()}
    if isinstance(value, list):
        return [normalize_json(item) for item in value]
    if isinstance(value, float):
        assert math.isfinite(value), "non-finite contract number"
        if value.is_integer():
            return int(value)
    return value

def canonical_json_sha256(path):
    value = normalize_json(json.load(open(path)))
    canonical = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()

committed = json.load(open(sys.argv[1]))
regenerated = json.load(open(sys.argv[2]))
assert normalize_json(committed) == normalize_json(regenerated), "full contract semantic drift"
assert committed["schema"] == regenerated["schema"], "schema drift"
assert committed["oracle_output_version"] == regenerated["oracle_output_version"]
assert committed["pinned_flexpart"]["pinned_commit"] == regenerated["pinned_flexpart"]["pinned_commit"]
assert committed["real_data_samples"] == regenerated["real_data_samples"], "real-data sample descriptor drifted"
assert committed["cases"] == regenerated["cases"], "golden values drifted"
p = json.load(open(sys.argv[3]))
q = json.load(open(sys.argv[4]))
for key in (
    "schema", "pinned_commit", "checkout_clean", "entrypoint_present",
    "fixture_artifact", "generator_source", "driver_source",
    "oracle_harness_source", "real_extraction_source", "reference_manifest",
    "cases", "real_data_samples", "interface_vertical_source",
    "real_vertical_source",
):
    assert p[key] == q[key], f"provenance drift in {key}"
assert p["fixture_artifact"]["hash_kind"] == "normalized_canonical_json_sha256"
assert p["fixture_artifact"]["sha256"] == canonical_json_sha256(sys.argv[1]), "committed contract hash mismatch"
assert q["fixture_artifact"]["sha256"] == canonical_json_sha256(sys.argv[2]), "regenerated contract hash mismatch"
assert p["linked_flexpart"]["routines"] == q["linked_flexpart"]["routines"], "routine list drifted"
pa = {o["object"]: o["object_sha256"] for o in p["linked_flexpart"]["objects"]}
qa = {o["object"]: o["object_sha256"] for o in q["linked_flexpart"]["objects"]}
assert pa == qa, "linked object hashes drifted"
assert p["driver_source"]["sha256"] == q["driver_source"]["sha256"], "driver source drifted"
print("interpolation contract fixture/provenance reproduced: OK")
' \
    "${PROJECT_ROOT}/fixtures/interpolation/contract-v1.json" \
    "${INTERPOL_DIR}/contract-v1.json" \
    "${PROJECT_ROOT}/fixtures/interpolation/contract-v1.provenance.json" \
    "${INTERPOL_DIR}/contract-v1.provenance.json" 2>&1 | tee "${INTERPOL_DIR}/reproducibility-check.log"; then
    fail "Regenerated #71 interpolation fixture/provenance does not match the committed pack"
  fi

  if ! cargo test --test interpolation_contract 2>&1 | tee "${INTERPOL_DIR}/fixture-validation.log"; then
    fail "Rust #71 interpolation contract validation tests failed"
  fi
  if ! grep -q "test result: ok" "${INTERPOL_DIR}/fixture-validation.log"; then
    fail "Interpolation contract validation log lacks 'test result: ok'"
  fi
  log_info "Interpolation contract fixture reproduced and goldens re-verified analytically."
fi
# ---------------------------------------------------------------------------
# 3. Prove a real software-WGPU adapter (fail-closed, no skip allowed).
# ---------------------------------------------------------------------------
log_info "Step 3/6: gpu-preflight on the software adapter..."
export FLEXPART_GPU_SOFTWARE="1"
export WGPU_BACKEND="${WGPU_BACKEND:-vulkan}"
export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"
if ! cargo run --bin gpu-preflight -- --software 2>&1 | tee "${OUTPUT_DIR}/gpu-preflight.log"; then
  fail "gpu-preflight on the software adapter failed (missing adapter or smoke failure)"
fi
if ! grep -q "software adapter: true" "${OUTPUT_DIR}/gpu-preflight.log"; then
  fail "Preflight did not select a software adapter (expected 'software adapter: true')"
fi
if ! grep -q "smoke test: PASS" "${OUTPUT_DIR}/gpu-preflight.log"; then
  fail "Preflight smoke test did not PASS (skipped smoke tests fail this gate)"
fi
ADAPTER_LINE="$(grep -E "adapter:" "${OUTPUT_DIR}/gpu-preflight.log" | head -1 || true)"
if [ -z "${ADAPTER_LINE}" ]; then
  fail "Preflight log lacks an adapter record"
fi
log_info "Software adapter proven: ${ADAPTER_LINE}"

# ---------------------------------------------------------------------------
# 4. Analytical case SW-WGPU-ADVECTION-001 (fail-closed, no skip allowed).
# ---------------------------------------------------------------------------
log_info "Step 4/6: analytical case SW-WGPU-ADVECTION-001..."
if ! cargo test --test integration \
  software_advection::test_sw_wgpu_advection_001_constant_wind_displacement \
  -- --exact --nocapture 2>&1 | tee "${OUTPUT_DIR}/sw-wgpu-advection.log"; then
  fail "SW-WGPU-ADVECTION-001 failed"
fi
if ! grep -q "test result: ok" "${OUTPUT_DIR}/sw-wgpu-advection.log"; then
  fail "SW-WGPU log lacks 'test result: ok' (skipped or failed tests fail this gate)"
fi
if ! grep -qE "1 passed" "${OUTPUT_DIR}/sw-wgpu-advection.log"; then
  fail "SW-WGPU log does not show exactly 1 passed test (missing test fails this gate)"
fi
if grep -qE "0 passed|FAILED|failed" "${OUTPUT_DIR}/sw-wgpu-advection.log" | grep -qv "0 failed" ; then
  # Defensive: any FAILED marker fails, but "0 failed" is expected.
  if grep -q "FAILED" "${OUTPUT_DIR}/sw-wgpu-advection.log"; then
    fail "SW-WGPU log contains FAILED"
  fi
fi
if ! grep -q "SW-WGPU-ADVECTION-001" "${OUTPUT_DIR}/sw-wgpu-advection.log"; then
  fail "SW-WGPU log lacks the analytical case marker"
fi
log_info "SW-WGPU-ADVECTION-001 passed on the software adapter."

# ---------------------------------------------------------------------------
# 5. Small synthetic candidate smoke on the software adapter (fail-closed).
# ---------------------------------------------------------------------------
log_info "Step 5/6: synthetic candidate smoke (fortran-validation, ${PARTICLES} particles)..."
CANDIDATE_OUTPUT="${OUTPUT_DIR}/candidate-output.json"
CANDIDATE_LOG="${OUTPUT_DIR}/candidate-run.log"
rm -f "${CANDIDATE_OUTPUT}"
if ! OUTPUT_PATH="${CANDIDATE_OUTPUT}" \
  PARTICLES="${PARTICLES}" \
  SYNC_READBACK=1 \
  RUST_LOG=info \
  FLEXPART_GPU_SOFTWARE=1 \
  cargo run --release --bin fortran-validation 2>&1 | tee "${CANDIDATE_LOG}"; then
  fail "Candidate smoke run (fortran-validation) failed"
fi
if [ ! -s "${CANDIDATE_OUTPUT}" ]; then
  fail "Candidate output missing or empty: ${CANDIDATE_OUTPUT}"
fi
ADAPTER_COUNT="$(grep -c "wgpu adapter" "${CANDIDATE_LOG}" || true)"
if [ "${ADAPTER_COUNT}" != "1" ]; then
  fail "Candidate log must contain exactly one wgpu adapter record (found ${ADAPTER_COUNT})"
fi
if ! grep -q "software fallback requested" "${CANDIDATE_LOG}"; then
  fail "Candidate did not run on the requested software fallback adapter"
fi
# Structural output checks (technical only, no scientific thresholds here).
if ! "${HOST_PYTHON}" -c '
import json, sys
data = json.load(open(sys.argv[1]))
assert data["averaging_seconds"] == 1800, "averaging window"
assert data["sampling_seconds"] == 900, "sampling window"
assert data["samples"] == 3, "sample count"
assert data["endpoint_weight"] == 0.5, "endpoint weight"
assert data["total_particles_active"] > 0, "active particles"
assert data["total_steps"] == 24, "expected 24 steps for 6h at 900s"
assert len(data["concentration_mass_kg"]) == data["grid"]["nx"] * data["grid"]["ny"] * data["grid"]["nz"], "grid shape"
print("candidate output structure: OK")
' "${CANDIDATE_OUTPUT}" 2>&1 | tee "${OUTPUT_DIR}/candidate-output-check.log"; then
  fail "Candidate output failed structural checks (inputs/outputs/provenance check)"
fi
if [ -x "${CANDIDATE_BINARY}" ]; then
  "${HOST_PYTHON}" -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' \
    "${CANDIDATE_BINARY}" > "${OUTPUT_DIR}/candidate-executable.sha256" || true
fi
log_info "Candidate smoke produced ${CANDIDATE_OUTPUT}."

# ---------------------------------------------------------------------------
# 6. Provenance manifests and machine-readable gate report (fail-closed).
# ---------------------------------------------------------------------------
log_info "Step 6/6: provenance manifests and gate report..."
REPORT_STATUS="TECHNICAL_PASS"
if [ "${SKIP_ORACLE_BUILD}" = "1" ]; then
  REPORT_STATUS="TECHNICAL_FAIL"
fi
if [ "${ORACLE_BUILD_STATUS}" != "PASS" ] && [ "${SKIP_ORACLE_BUILD}" != "1" ]; then
  fail "Oracle build did not pass"
fi

# Best-effort oracle run manifest via the shared provenance writer.
# Requires the Docker image and both executables; missing artifacts fail.
if [ "${SKIP_ORACLE_BUILD}" != "1" ]; then
  if ! "${HOST_PYTHON}" "${SCRIPT_DIR}/write_oracle_run_manifest.py" \
    --output "${OUTPUT_DIR}/run-manifest.json" \
    --scenario synthetic-uniform-wind-smoke \
    --oracle-manifest "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --oracle-checkout "${ORACLE_CHECKOUT}" \
    --oracle-executable "${ORACLE_EXECUTABLE}" \
    --candidate-checkout "${PROJECT_ROOT}" \
    --candidate-executable "${CANDIDATE_BINARY}" \
    --candidate-log "${CANDIDATE_LOG}" \
    --input "${PROJECT_ROOT}/reference/flexpart-11.1.json" \
    --input "${PROJECT_ROOT}/src/bin/fortran-validation.rs" \
    --artifact "${CANDIDATE_OUTPUT}" \
    --artifact "${CANDIDATE_LOG}" \
    --artifact "${OUTPUT_DIR}/sw-wgpu-advection.log" \
    --artifact "${OUTPUT_DIR}/gpu-preflight.log" \
    --artifact "${OUTPUT_DIR}/vertical-column/comparison-report.json" \
    --artifact "${OUTPUT_DIR}/vertical-column/conformance-comparison-report.json" \
    --artifact "${OUTPUT_DIR}/vertical-column/routine-oracle-provenance.json" \
    --artifact "${OUTPUT_DIR}/vertical-column/real-comparison-report.json" \
    --artifact "${OUTPUT_DIR}/vertical-column/real-conformance-comparison-report.json" \
    --artifact "${OUTPUT_DIR}/vertical-column/real-column-fixture-provenance.json" \
    --artifact "${OUTPUT_DIR}/interpolation/contract-v1.json" \
    --artifact "${OUTPUT_DIR}/interpolation/contract-v1.provenance.json" \
    --artifact "${OUTPUT_DIR}/interpolation/oracle-output/real-era5-etex-temperature-column.out" \
    --artifact "${OUTPUT_DIR}/vertical-column/real-routine-oracle-output.txt" \
    --artifact "${OUTPUT_DIR}/interpolation/reproducibility-check.log" 2>&1 | tee "${OUTPUT_DIR}/run-manifest.log"; then
    fail "Provenance manifest generation failed (missing artifact or unpinned oracle)"
  fi
  test -s "${OUTPUT_DIR}/run-manifest.json" || fail "Provenance manifest missing: ${OUTPUT_DIR}/run-manifest.json"
  if ! "${HOST_PYTHON}" -c '
import json, sys
data = json.load(open(sys.argv[1]))
contract = data["meteorology_contract"]
assert contract["schema_id"] == "flexpart-gpu.canonical-meteorology"
assert contract["schema_version"] == 1
assert len(contract["identity_source_sha256"]) == 64
binding = data["meteorology_input"]
assert binding["status"] == "NOT_BOUND_TO_RUN"
assert binding["inputs"] == {}
print("meteorology contract provenance: OK (no canonical runtime input for this smoke)")
' "${OUTPUT_DIR}/run-manifest.json" 2>&1 | tee "${OUTPUT_DIR}/meteorology-provenance-check.log"; then
    fail "Run manifest lacks valid canonical meteorology schema provenance"
  fi
else
  echo "oracle build skipped; run-manifest not generated" > "${OUTPUT_DIR}/run-manifest.log"
fi

# Machine-readable gate report (stable schema v1, see docs/ci-gates.md).
if ! "${HOST_PYTHON}" "${SCRIPT_DIR}/ci-gate-report.py" \
  --output "${OUTPUT_DIR}/ci-gate-report.json" \
  --project-root "${PROJECT_ROOT}" \
  --output-dir "${OUTPUT_DIR}" \
  --oracle-checkout "${ORACLE_CHECKOUT}" \
  --status "${REPORT_STATUS}" \
  --particles "${PARTICLES}" \
  --allowlist "${CI_CASE_ALLOWLIST}" 2>&1 | tee "${OUTPUT_DIR}/ci-gate-report.log"; then
  fail "Gate report generation failed"
fi
test -s "${OUTPUT_DIR}/ci-gate-report.json" || fail "Gate report missing"
if ! "${HOST_PYTHON}" -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["status"]=="TECHNICAL_PASS", d["status"]; assert d["scientific_verdict"]=="NOT_EVALUATED"' \
  "${OUTPUT_DIR}/ci-gate-report.json"; then
  if [ "${REPORT_STATUS}" = "TECHNICAL_PASS" ]; then
    fail "Gate report does not show TECHNICAL_PASS"
  fi
  # When --skip-oracle-build was requested, a TECHNICAL_FAIL report is the
  # expected fail-closed outcome for local iteration.
  log_warn "Gate report status is not TECHNICAL_PASS (expected with --skip-oracle-build)."
  log_error "CI gate: TECHNICAL_FAIL (see ${OUTPUT_DIR}/ci-gate-report.json)"
  exit 1
fi

log_info "CI gate: TECHNICAL_PASS (no scientific parity verdict; Issue #6 stays open)."
log_info "Artifacts in ${OUTPUT_DIR}: ci-gate-report.json, run-manifest.json, logs, candidate-output.json."
