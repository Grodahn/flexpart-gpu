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
mkdir -p "${OUT}/build/upstream"
git -C "${CHECKOUT}" archive HEAD:src | tar -x -C "${OUT}/build/upstream"
(
  cd "${OUT}/build/upstream"
  make -f makefile_gfortran FC=gfortran eta=no arch=x86-64 -j4
) > "${OUT}/full-build.log" 2>&1
export ORACLE_SRC="${OUT}/build/upstream"
export DRIVER="${ROOT}/scripts/interpolation/shared_height_oracle.f90"
# Reuse #80's genuine-object linking, symbol and executable call-site capture.
export OMP_NUM_THREADS=1 OMP_THREAD_LIMIT=1 OMP_DYNAMIC=FALSE OMP_NESTED=FALSE
export OMP_MAX_ACTIVE_LEVELS=1 OMP_SCHEDULE=static OMP_PROC_BIND=FALSE OMP_WAIT_POLICY=PASSIVE
cp "${ROOT}/fixtures/interpolation/shared-height-v1/input.txt" "${OUT}/input.txt"
bash "${ROOT}/scripts/interpolation/w_production_oracle.sh" "${OUT}/build" \
  "${OUT}/input.txt" "${OUT}/output.txt" > "${OUT}/driver-build-run.log" 2>&1
test -z "$(git -C "${CHECKOUT}" status --porcelain)"
python3 "${ROOT}/scripts/interpolation/prepare_shared_height_oracle.py" \
  --checkout "${CHECKOUT}" --run "${OUT}" --expected "${ROOT}/fixtures/interpolation/shared-height-v1"
