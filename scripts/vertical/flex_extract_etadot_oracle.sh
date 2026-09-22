#!/usr/bin/env bash
set -euo pipefail

# ---------------------------------------------------------------------------
# calc_etadot oracle tier (#70): run the pinned flex_extract 7.1.2
# Calc_etadot installation example and compare the candidate
# eta_dot_to_pressure_velocity output against the oracle field.
#
# Fail-closed. Keeps the pinned flex_extract checkout pristine by building
# and running in scratch directories under the output dir; the checkout is
# never modified. Requires:
#   * Docker with the flex-extract image built from flexpart-fortran plus
#     libemos-dev/libemos-bin/libemos-data/libopenjp2-7-dev,
#   * a pinned, clean flex_extract checkout (reference/flex-extract.json).
#
# Usage:
#   scripts/vertical/flex_extract_etadot_oracle.sh \
#     --flex-extract-checkout <dir> [--output-dir <dir>]
# ---------------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

FLEXEXTRACT_CHECKOUT="${PROJECT_ROOT}/../flex_extract"
OUTPUT_DIR="${PROJECT_ROOT}/target/ci-gate/flex-extract-oracle"
HOST_PYTHON="python3"
if [ "${OS:-}" = "Windows_NT" ]; then
  HOST_PYTHON="python"
fi

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
log_info()  { echo -e "${GREEN}[ETADOT-ORACLE]${NC}  $*"; }
log_error() { echo -e "${RED}[ETADOT-ORACLE]${NC} $*"; }

usage() {
  cat <<'EOF'
Usage:
  scripts/vertical/flex_extract_etadot_oracle.sh [--flex-extract-checkout <dir>]

Options:
  --flex-extract-checkout <dir>  Pinned flex_extract checkout
                                 (default: ../flex_extract relative to the repo).
  --output-dir <dir>             Scratch/output directory
                                 (default: target/ci-gate/flex-extract-oracle).
  -h, --help                     Show this help.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --flex-extract-checkout) FLEXEXTRACT_CHECKOUT="$2"; shift 2 ;;
    --output-dir) OUTPUT_DIR="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) log_error "Unknown argument: $1"; usage; exit 2 ;;
  esac
done

# The compose service bind-mounts host-owned scratch directories. Match the
# invoking user's uid/gid so CI runners can write those bind mounts.
export DOCKER_UID="${DOCKER_UID:-$(id -u)}"
export DOCKER_GID="${DOCKER_GID:-$(id -g)}"

# The compose service bind-mounts the checkout via FLEXEXTRACT_DIR, so keep it
# in lockstep with the checkout actually verified (never silently verify one
# tree and mount another).
export FLEXEXTRACT_DIR="${FLEXEXTRACT_CHECKOUT}"

fail() {
  log_error "$*"
  exit 1
}

# Never reuse oracle products from an earlier invocation.
rm -rf "${OUTPUT_DIR}"
mkdir -p "${OUTPUT_DIR}"
MANIFEST="${PROJECT_ROOT}/reference/flex-extract.json"
PINNED_COMMIT="$("${HOST_PYTHON}" -c \
  "import json,sys; print(json.load(open(sys.argv[1]))['pinned_commit'])" \
  "${MANIFEST}")"

log_info "flex_extract checkout: ${FLEXEXTRACT_CHECKOUT} (pinned ${PINNED_COMMIT})"
log_info "output dir: ${OUTPUT_DIR}"

if [ ! -d "${FLEXEXTRACT_CHECKOUT}" ]; then
  fail "flex_extract checkout not found at ${FLEXEXTRACT_CHECKOUT}"
fi

# 1. Verify the pinned, clean checkout (same machinery as the FLEXPART tier).
if ! cargo run --quiet --bin reference-check -- verify \
  --checkout "${FLEXEXTRACT_CHECKOUT}" \
  --manifest "${MANIFEST}" 2>&1 | tee "${OUTPUT_DIR}/verify.log"; then
  fail "flex_extract checkout verification failed (must be pinned and clean)"
fi
if ! grep -q "reference checkout: OK" "${OUTPUT_DIR}/verify.log"; then
  fail "flex_extract verification log lacks OK marker"
fi

# 2. Compile calc_etadot and run the pinned installation example in scratch
#    directories so the checkout stays byte-for-byte pristine.
BUILD_DIR="${OUTPUT_DIR}/oracle-build"
RUN_DIR="${OUTPUT_DIR}/oracle-run"
rm -rf "${BUILD_DIR}" "${RUN_DIR}"
mkdir -p "${BUILD_DIR}" "${RUN_DIR}"

docker compose -f "${PROJECT_ROOT}/docker/docker-compose.fortran.yml" run --rm \
  flex-extract bash -c "
    set -euo pipefail
    src=/workspace/flex_extract/Source/Fortran
    example=/workspace/flex_extract/Testing/Installation/Calc_etadot
    build=/workspace/target/ci-gate/flex-extract-oracle/oracle-build
    run=/workspace/target/ci-gate/flex-extract-oracle/oracle-run

    cp \"\$src\"/rwgrib2.f90 \"\$src\"/calc_etadot.f90 \"\$src\"/ftrafo.f90 \
       \"\$src\"/grphreal.f90 \"\$src\"/posnam.f90 \"\$src\"/phgrreal.f90 \
       \"\$src\"/makefile_fast \"\$build\"/
    cd \"\$build\"
    gfortran --version | sed -n '1p' > /workspace/target/ci-gate/flex-extract-oracle/compiler-version.txt
    make -f makefile_fast \
      LIB='-Bstatic -leccodes_f90_static -leccodes_static -Bdynamic -lemosR64 -lm -lpng -laec -lopenjp2' \
      INC='-I. -I/usr/lib/x86_64-linux-gnu/fortran/gfortran-mod-15' \
      calc_etadot_fast.out >/tmp/calc_etadot-make.log 2>&1
    test -x \"\$build\"/calc_etadot_fast.out

    cp \"\$example\"/fort.* \"\$run\"/
    cd \"\$run\"
    ln -sf \"\$build\"/calc_etadot_fast.out calc_etadot
    set +e
    ./calc_etadot >/tmp/calc_etadot-run.log 2>&1
    rc=\$?
    set -e
    cat /tmp/calc_etadot-run.log
    test \$rc -eq 0
    grep -q 'CONGRATULATIONS' /tmp/calc_etadot-run.log
    test -s \"\$run\"/fort.15

    python3 /workspace/flexpart-gpu/scripts/vertical/extract_calc_etadot_oracle.py \
      --example-dir \"\$run\" \
      --output-dir /workspace/target/ci-gate/flex-extract-oracle/oracle-json
  " 2>&1 | tee "${OUTPUT_DIR}/oracle-build-run.log" || \
  fail "calc_etadot oracle build/run/extract failed (see oracle-build-run.log)"

# Record machine-readable identity of the concrete oracle run: container image,
# compiler, executable and all consumed/produced fort.* payloads.
DOCKER_IMAGE_ID="$(docker image inspect flex-extract:latest --format '{{.Id}}' 2>/dev/null)" \
  || fail "could not inspect flex-extract:latest image identity"
if ! "${HOST_PYTHON}" - "${OUTPUT_DIR}" "${DOCKER_IMAGE_ID}" "${PROJECT_ROOT}" "${FLEXEXTRACT_CHECKOUT}" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

out = Path(sys.argv[1])
image_id = sys.argv[2]
project = Path(sys.argv[3])
checkout = Path(sys.argv[4])
upstream_example = checkout / "Testing" / "Installation" / "Calc_etadot"

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

compiler_file = out / "compiler-version.txt"
exe = out / "oracle-build" / "calc_etadot_fast.out"
run = out / "oracle-run"
required_inputs = ["fort.4", "fort.10", "fort.11", "fort.12", "fort.17", "fort.21"]
for path in [
    compiler_file,
    exe,
    *(run / name for name in required_inputs),
    *(upstream_example / name for name in required_inputs),
    run / "fort.15",
]:
    if not path.is_file():
        raise SystemExit(f"missing provenance input: {path}")
for name in required_inputs:
    if digest(run / name) != digest(upstream_example / name):
        raise SystemExit(f"oracle input mutated during run: {name}")

payload = {
    "schema": "flexpart-gpu.etadot-oracle-run-provenance.v1",
    "docker_image": {"name": "flex-extract:latest", "id": image_id},
    "compiler": {"version": compiler_file.read_text(encoding="utf-8").strip()},
    "oracle_executable": {
        "path": str(exe),
        "sha256": digest(exe),
    },
    "build_inputs": {
        "dockerfile_sha256": digest(project / "docker" / "Dockerfile.flex-extract"),
        "compose_sha256": digest(project / "docker" / "docker-compose.fortran.yml"),
    },
    "oracle_inputs": {
        name: {
            "path": str(run / name),
            "sha256": digest(run / name),
            "upstream_path": str(upstream_example / name),
            "upstream_sha256": digest(upstream_example / name),
        }
        for name in required_inputs
    },
    "oracle_outputs": {
        "fort.15": {"path": str(run / "fort.15"), "sha256": digest(run / "fort.15")}
    },
}
(out / "run-provenance.json").write_text(
    json.dumps(payload, indent=2) + "\n", encoding="utf-8"
)
PY
then
  fail "failed to write calc_etadot run provenance"
fi

ORACLE_JSON="${OUTPUT_DIR}/oracle-json"
test -s "${ORACLE_JSON}/snapshot.json" || fail "oracle snapshot missing"
test -s "${ORACLE_JSON}/motion.json" || fail "oracle motion missing"
test -s "${ORACLE_JSON}/oracle.json" || fail "oracle reference missing"

# 3. Run the candidate on the extracted snapshot + motion.
if ! cargo run --quiet --bin eta-dot-column-report -- \
  "${ORACLE_JSON}/snapshot.json" \
  "${ORACLE_JSON}/motion.json" \
  "${OUTPUT_DIR}/candidate.json"; then
  fail "Candidate eta-dot->Pa/s transform failed"
fi

# 4. Compare the candidate field against the oracle field (fail-closed).
if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/vertical/compare_calc_etadot_oracle.py" \
  --candidate "${OUTPUT_DIR}/candidate.json" \
  --oracle "${ORACLE_JSON}/oracle.json" \
  --flex-extract-checkout "${FLEXEXTRACT_CHECKOUT}" \
  --reference-manifest "${MANIFEST}" \
  --source-snapshot "${ORACLE_JSON}/snapshot.json" \
  --source-motion "${ORACLE_JSON}/motion.json" \
  --run-provenance "${OUTPUT_DIR}/run-provenance.json" \
  --output "${OUTPUT_DIR}/comparison-report.json"; then
  fail "calc_etadot oracle field comparison failed"
fi
test -s "${OUTPUT_DIR}/comparison-report.json" \
  || fail "comparison report missing"

if ! "${HOST_PYTHON}" -c '
import json, sys
report = json.load(open(sys.argv[1]))
assert report["status"] == "PASS", report["status"]
assert report["oracle"]["calc_etadot_f90_sha256"] == "160F267F8741F23D13FDBA2F7A88F110BB131AA84AD7894FA43605258E55B0D9"
assert report["oracle"]["calc_etadot_f90_git_blob"] == "741eba91eab049df23a560219d0f2656a6cc9881"
print("eta-dot oracle comparison: PASS")
' "${OUTPUT_DIR}/comparison-report.json"; then
  fail "comparison report failed structural assertion"
fi

# 5. Run the same pinned calc_etadot executable on one complete real ERA5
#    native-level case: 137/137 eta-dot levels over the checked-in 65x41 ETEX
#    native-mini grid at 1994-10-23 15:00 UTC. This closes #70's real-column
#    proof obligation without candidate-derived inputs or zero-filled levels.
REAL_ROOT="${OUTPUT_DIR}/real-era5-137"
REAL_RUN="${REAL_ROOT}/oracle-run"
REAL_JSON="${REAL_ROOT}/oracle-json"
rm -rf "${REAL_ROOT}"
mkdir -p "${REAL_RUN}" "${REAL_JSON}"

if ! docker compose -f "${PROJECT_ROOT}/docker/docker-compose.fortran.yml" run --rm \
  flex-extract bash -c "
    set -euo pipefail
    root=/workspace/target/ci-gate/flex-extract-oracle
    real=\$root/real-era5-137
    run=\$real/oracle-run
    json=\$real/oracle-json
    exe=\$root/oracle-build/calc_etadot_fast.out

    python3 /workspace/flexpart-gpu/scripts/vertical/prepare_real_era5_etadot_oracle.py \
      --native-dir /workspace/flexpart-gpu/fixtures/etex/native-mini \
      --output-dir \$run \
      --timestamp 1994-10-23T15:00:00

    cd \$run
    ln -sf \$exe calc_etadot
    set +e
    ./calc_etadot >\$real/calc-etadot.log 2>&1
    rc=\$?
    set -e
    cat \$real/calc-etadot.log
    test \$rc -eq 0
    grep -q 'CONGRATULATIONS' \$real/calc-etadot.log
    test -s \$run/fort.15

    python3 /workspace/flexpart-gpu/scripts/vertical/extract_calc_etadot_oracle.py \
      --example-dir \$run \
      --output-dir \$json \
      --expected-levels 1/to/137 \
      --fixture-classification real_era5_native_model_level_full_column \
      --fixture-origin 'fixtures/etex/native-mini ERA5 Complete 1994-10-23T15:00:00Z' \
      --source-provenance \$run/source-provenance.json
  " 2>&1 | tee "${REAL_ROOT}/oracle-build-run.log"; then
  fail "full 137-level real ERA5 calc_etadot oracle run failed"
fi

# Bind the real ERA5 run to the same concrete image/compiler/executable identity
# and to every generated fort.* input plus the independent source provenance.
if ! "${HOST_PYTHON}" - "${OUTPUT_DIR}" "${DOCKER_IMAGE_ID}" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

out = Path(sys.argv[1])
image_id = sys.argv[2]
real = out / "real-era5-137"
run = real / "oracle-run"
exe = out / "oracle-build" / "calc_etadot_fast.out"
compiler = out / "compiler-version.txt"
required = ["fort.4", "fort.10", "fort.11", "fort.12", "fort.17", "fort.21"]

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

for path in [exe, compiler, run / "fort.15", run / "source-provenance.json",
             *(run / name for name in required)]:
    if not path.is_file():
        raise SystemExit(f"missing real ERA5 provenance artifact: {path}")

source = json.loads((run / "source-provenance.json").read_text(encoding="utf-8"))
if source.get("schema") != "flexpart-gpu.etadot-real-era5-case.v1":
    raise SystemExit("real ERA5 source provenance schema mismatch")
if source.get("vertical", {}).get("model_levels") != 137:
    raise SystemExit("real ERA5 source provenance is not a 137-level case")
for name in required:
    generated = source["generated_inputs"].get(name, {}).get("sha256")
    actual = digest(run / name)
    if generated != actual:
        raise SystemExit(f"generated real ERA5 input hash changed: {name}")

payload = {
    "schema": "flexpart-gpu.etadot-oracle-run-provenance.v1",
    "docker_image": {"name": "flex-extract:latest", "id": image_id},
    "compiler": {"version": compiler.read_text(encoding="utf-8").strip()},
    "oracle_executable": {"path": str(exe), "sha256": digest(exe)},
    "source_provenance": {
        "path": str(run / "source-provenance.json"),
        "sha256": digest(run / "source-provenance.json"),
    },
    "oracle_inputs": {
        name: {"path": str(run / name), "sha256": digest(run / name)}
        for name in required
    },
    "oracle_outputs": {
        "fort.15": {"path": str(run / "fort.15"), "sha256": digest(run / "fort.15")}
    },
}
(real / "run-provenance.json").write_text(
    json.dumps(payload, indent=2) + "\n", encoding="utf-8"
)
PY
then
  fail "failed to write full real ERA5 calc_etadot provenance"
fi

test -s "${REAL_JSON}/snapshot.json" || fail "real ERA5 oracle snapshot missing"
test -s "${REAL_JSON}/motion.json" || fail "real ERA5 oracle motion missing"
test -s "${REAL_JSON}/oracle.json" || fail "real ERA5 oracle reference missing"

if ! cargo run --quiet --bin eta-dot-column-report -- \
  "${REAL_JSON}/snapshot.json" \
  "${REAL_JSON}/motion.json" \
  "${REAL_ROOT}/candidate.json"; then
  fail "Candidate full real ERA5 eta-dot->Pa/s transform failed"
fi

if ! "${HOST_PYTHON}" "${PROJECT_ROOT}/scripts/vertical/compare_calc_etadot_oracle.py" \
  --candidate "${REAL_ROOT}/candidate.json" \
  --oracle "${REAL_JSON}/oracle.json" \
  --flex-extract-checkout "${FLEXEXTRACT_CHECKOUT}" \
  --reference-manifest "${MANIFEST}" \
  --source-snapshot "${REAL_JSON}/snapshot.json" \
  --source-motion "${REAL_JSON}/motion.json" \
  --run-provenance "${REAL_ROOT}/run-provenance.json" \
  --output "${REAL_ROOT}/comparison-report.json"; then
  fail "full 137-level real ERA5 calc_etadot field comparison failed"
fi

if ! "${HOST_PYTHON}" -c '
import json, sys
report = json.load(open(sys.argv[1]))
coverage = report["coverage"]
assert report["status"] == "PASS", report["status"]
assert coverage["complete_real_native_column"] is True
assert coverage["levels_compared"] == 137
assert coverage["points_per_level"] == 65 * 41
assert coverage["comparisons"] == 137 * 65 * 41
assert coverage["failure_count"] == 0
assert report["source_fixture"]["level_coverage"]["complete_native_column"] is True
print(
    "full real ERA5 eta-dot oracle comparison: PASS "
    f"({coverage['comparisons']} values)"
)
' "${REAL_ROOT}/comparison-report.json"; then
  fail "full real ERA5 comparison report failed structural assertion"
fi

# 6. The build/run must leave the pinned checkout pristine.
POST_STATUS="$(git -C "${FLEXEXTRACT_CHECKOUT}" status --porcelain 2>/dev/null || true)"
if [ -n "${POST_STATUS}" ]; then
  log_error "flex_extract status after run:"
  echo "${POST_STATUS}" | head -20
  fail "flex_extract checkout is dirty after the oracle run"
fi

log_info "calc_etadot oracle tier: PASS (upstream fixture + complete 137-level real ERA5 case)"