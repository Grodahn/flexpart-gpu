#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Run genuine pinned FLEXPART 11.1 interpol_rain on flex_extract-prepared
# precipitation members (issue #202 research evidence).
#
# Executed inside the pinned Fortran oracle container (flexpart-fortran). The
# authoritative checkout at /workspace/flexpart must be the clean pinned commit;
# objects are built from a `git archive` scratch copy so the checkout is never
# modified, then linked through the unchanged scripts/interpolation/direct_oracle.sh.
#
# Usage:
#   member_mapping_oracle.sh <trace-dir> <out-dir>
# where <trace-dir> holds rain-inputs/ written by flex_extract_member_trace.py.
# ---------------------------------------------------------------------------
set -euo pipefail

TRACE="${1:?usage: member_mapping_oracle.sh <trace-dir> <out-dir>}"
OUT="${2:?missing output directory}"
ROOT=/workspace/flexpart-gpu
CHECKOUT=/workspace/flexpart
PIN=$(python3 -c 'import json; print(json.load(open("/workspace/flexpart-gpu/reference/flexpart-11.1.json"))["pinned_commit"])')
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0="${CHECKOUT}"

check_checkout() {
  test "$(git -C "${CHECKOUT}" rev-parse HEAD)" = "${PIN}"
  test -z "$(git -C "${CHECKOUT}" status --porcelain)"
  { git -C "${CHECKOUT}" rev-parse HEAD; git -C "${CHECKOUT}" status --porcelain; } > "$1"
}

mkdir -p "${OUT}/build/upstream" "${OUT}/runs"
check_checkout "${OUT}/checkout-before.txt"
if [ ! -f "${OUT}/build/upstream/interpol_mod.o" ]; then
  git -C "${CHECKOUT}" archive HEAD:src | tar -x -C "${OUT}/build/upstream"
  (cd "${OUT}/build/upstream" && make -f makefile_gfortran FC=gfortran eta=no arch=x86-64 -j4) \
    > "${OUT}/full-build.log" 2>&1
fi

export ORACLE_SRC="${OUT}/build/upstream"
export OMP_NUM_THREADS=1 OMP_THREAD_LIMIT=1 OMP_DYNAMIC=FALSE OMP_NESTED=FALSE \
  OMP_MAX_ACTIVE_LEVELS=1 OMP_SCHEDULE=static OMP_PROC_BIND=FALSE OMP_WAIT_POLICY=PASSIVE
for input in "${TRACE}"/rain-inputs/rain-*.txt; do
  name=$(basename "${input}" .txt)
  bash "${ROOT}/scripts/interpolation/direct_oracle.sh" "${OUT}/build" "${input}" \
    "${OUT}/runs/${name}-output.txt" > "${OUT}/runs/${name}-driver.log" 2>&1
done

check_checkout "${OUT}/checkout-after.txt"
gfortran --version | sed -n '1p' > "${OUT}/compiler-version.txt"
(cd "${OUT}" && sha256sum build/interpolation-oracle runs/*-output.txt \
  "${ORACLE_SRC}/interpol_mod.f90" "${ORACLE_SRC}/windfields_mod.f90" \
  "${ORACLE_SRC}/getfields_mod.f90" "${ORACLE_SRC}/par_mod.f90" \
  "${ROOT}/scripts/interpolation/direct_interpolation_oracle.f90" > hashes.txt)
sha256sum "${TRACE}"/rain-inputs/rain-*.txt >> "${OUT}/hashes.txt"
