#!/usr/bin/env bash
# Research-only direct oracle. Run inside the established Fortran image.
set -euo pipefail
CHECKOUT="${1:?usage: shared_height_oracle.sh <pristine-checkout> <new-output-dir>}"
OUT="${2:?missing new output directory}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PIN=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pinned_commit"])' \
  "${ROOT}/reference/flexpart-11.1.json")
git -C "${CHECKOUT}" config --global --add safe.directory "${CHECKOUT}"
test "$(git -C "${CHECKOUT}" rev-parse HEAD)" = "${PIN}" || { echo 'Wrong pinned oracle revision' >&2; exit 1; }
test -z "$(git -C "${CHECKOUT}" status --porcelain)" || { echo 'Dirty oracle checkout' >&2; exit 1; }
test ! -e "${OUT}" || { echo 'Output directory already exists' >&2; exit 1; }
mkdir -p "${OUT}/build"
# Compile a scratch copy: even the upstream makefile's main-program stamp
# cannot modify the authoritative checkout. No copied routine is edited.
WORK=$(mktemp -d /tmp/flexpart-height-research.XXXXXXXX)
trap 'rm -rf -- "${WORK}"' EXIT
mkdir -p "${WORK}/build/upstream"
git -C "${CHECKOUT}" rev-parse HEAD > "${OUT}/checkout-before.txt"
git -C "${CHECKOUT}" status --porcelain >> "${OUT}/checkout-before.txt"
git -C "${CHECKOUT}" archive HEAD:src | tar -x -C "${WORK}/build/upstream"
(
  cd "${WORK}/build/upstream"
  make -f makefile_gfortran FC=gfortran eta=no arch=x86-64 -j4
) > "${OUT}/full-build.log" 2>&1
export ORACLE_SRC="${WORK}/build/upstream"
KIND="${3:-shared-height}"
case "${KIND}" in
  shared-height) DRIVER_NAME=shared_height_oracle; FIXTURE=shared-height-v1; AUDITOR=prepare_shared_height_oracle ;;
  interior-w) DRIVER_NAME=interior_w_oracle; FIXTURE=interior-w-v1; AUDITOR=prepare_interior_w_oracle ;;
  *) echo 'Unknown research kind' >&2; exit 1 ;;
esac
export DRIVER="${ROOT}/scripts/interpolation/${DRIVER_NAME}.f90"
# Reuse #80's genuine-object linking, symbol and executable call-site capture.
export OMP_NUM_THREADS=1 OMP_THREAD_LIMIT=1 OMP_DYNAMIC=FALSE OMP_NESTED=FALSE
export OMP_MAX_ACTIVE_LEVELS=1 OMP_SCHEDULE=static OMP_PROC_BIND=FALSE OMP_WAIT_POLICY=PASSIVE
cp "${ROOT}/fixtures/interpolation/${FIXTURE}/input.txt" "${OUT}/input.txt"
bash "${ROOT}/scripts/interpolation/w_production_oracle.sh" "${WORK}/build" \
  "${OUT}/input.txt" "${OUT}/output.txt" > "${OUT}/driver-build-run.log" 2>&1
cp -a "${WORK}/build/." "${OUT}/build/"
test -z "$(git -C "${CHECKOUT}" status --porcelain)"
git -C "${CHECKOUT}" rev-parse HEAD > "${OUT}/checkout-after.txt"
git -C "${CHECKOUT}" status --porcelain >> "${OUT}/checkout-after.txt"
python3 "${ROOT}/scripts/interpolation/${AUDITOR}.py" \
  --checkout "${CHECKOUT}" --run "${OUT}" --expected "${ROOT}/fixtures/interpolation/${FIXTURE}"
