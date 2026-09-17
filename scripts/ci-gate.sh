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
    --artifact "${OUTPUT_DIR}/gpu-preflight.log" 2>&1 | tee "${OUTPUT_DIR}/run-manifest.log"; then
    fail "Provenance manifest generation failed (missing artifact or unpinned oracle)"
  fi
  test -s "${OUTPUT_DIR}/run-manifest.json" || fail "Provenance manifest missing: ${OUTPUT_DIR}/run-manifest.json"
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
