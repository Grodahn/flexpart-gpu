#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Run genuine pinned FLEXPART 11.1 gridcheck_ecmwf + readwind_ecmwf on the
# flex_extract-prepared GRIB1 and GRIB2 input files (issue #211 evidence).
#
# Executed inside the pinned Fortran oracle container (flexpart-fortran). The
# authoritative checkout at /workspace/flexpart must be the clean pinned commit;
# objects are built from a `git archive` scratch copy so the checkout is never
# modified. The harness links every FLEXPART object except FLEXPART.o (the
# model main program), as scripts/interpolation/direct_oracle.sh does.
#
# Usage:
#   grib2_cp_reader_oracle.sh <trace-dir> <out-dir>
# where <trace-dir> holds prepared-grib1/ and prepared-grib2/ written by
# grib2_cp_units_trace.py.
# ---------------------------------------------------------------------------
set -euo pipefail

TRACE="${1:?usage: grib2_cp_reader_oracle.sh <trace-dir> <out-dir>}"
OUT="${2:?missing output directory}"
ROOT=/workspace/flexpart-gpu
CHECKOUT=/workspace/flexpart
DRIVER="${ROOT}/scripts/precipitation/grib2_cp_reader_oracle.f90"
PIN=$(python3 -c 'import json; print(json.load(open("/workspace/flexpart-gpu/reference/flexpart-11.1.json"))["pinned_commit"])')
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0="${CHECKOUT}"

check_checkout() {
  test "$(git -C "${CHECKOUT}" rev-parse HEAD)" = "${PIN}"
  test -z "$(git -C "${CHECKOUT}" status --porcelain)"
  { git -C "${CHECKOUT}" rev-parse HEAD; git -C "${CHECKOUT}" status --porcelain; } > "$1"
}

UPSTREAM="${OUT}/build/upstream"
mkdir -p "${UPSTREAM}" "${OUT}/runs"
check_checkout "${OUT}/checkout-before.txt"
if [ ! -f "${UPSTREAM}/windfields_mod.o" ]; then
  git -C "${CHECKOUT}" archive HEAD:src | tar -x -C "${UPSTREAM}"
  (cd "${UPSTREAM}" && make -f makefile_gfortran FC=gfortran eta=no arch=x86-64 -j4) \
    > "${OUT}/full-build.log" 2>&1
fi
# A reused build must still come from the pinned sources: compare every file of
# a fresh archive with the scratch copy before linking its objects.
PRISTINE=$(mktemp -d)
git -C "${CHECKOUT}" archive HEAD:src | tar -x -C "${PRISTINE}"
(cd "${PRISTINE}" && find . -type f -print0 | sort -z | xargs -0 sha256sum) > "${OUT}/pristine-src.sha256"
(cd "${UPSTREAM}" && sha256sum --quiet -c "${OUT}/pristine-src.sha256")
rm -rf "${PRISTINE}"

objects=$(find "${UPSTREAM}" -maxdepth 1 -type f -name '*.o' ! -name 'FLEXPART.o' -print | sort)
BINARY="${OUT}/build/grib2-cp-reader-oracle"
# shellcheck disable=SC2086
gfortran -O0 -I"${UPSTREAM}" -fopenmp -mcmodel=large "${DRIVER}" ${objects} \
  -L/usr/lib/x86_64-linux-gnu -Wl,-rpath=/usr/lib/x86_64-linux-gnu \
  -leccodes -leccodes_f90 -lm -lnetcdff -o "${BINARY}"
printf '%s\n' ${objects} | sed "s#^${UPSTREAM}/##" > "${OUT}/build/linked-objects.txt"
nm "${BINARY}" > "${OUT}/build/grib2-cp-reader-oracle.nm"
grep -q '__windfields_mod_MOD_readwind_ecmwf' "${OUT}/build/grib2-cp-reader-oracle.nm"
grep -q '__windfields_mod_MOD_gridcheck_ecmwf' "${OUT}/build/grib2-cp-reader-oracle.nm"

export OMP_NUM_THREADS=1 OMP_THREAD_LIMIT=1 OMP_DYNAMIC=FALSE OMP_NESTED=FALSE \
  OMP_MAX_ACTIVE_LEVELS=1 OMP_SCHEDULE=static OMP_PROC_BIND=FALSE OMP_WAIT_POLICY=PASSIVE
# prepared-grib1, prepared-grib2 and any prepared-grib2-eccodes-<version> variant.
# A reader stop is evidence, not a script failure: its exit status is kept per run.
for prepared in "${TRACE}"/prepared-*/; do
  edition=$(basename "${prepared}")
  edition=${edition#prepared-}
  for input in "${prepared}"EN*; do
    name=$(basename "${input}")
    status=0
    "${BINARY}" "${prepared}" "${name}" "${OUT}/runs/${edition}-${name}-output.txt" \
      > "${OUT}/runs/${edition}-${name}-reader.log" 2>&1 || status=$?
    echo "${status}" > "${OUT}/runs/${edition}-${name}-exit.txt"
  done
done

check_checkout "${OUT}/checkout-after.txt"
gfortran --version | sed -n '1p' > "${OUT}/compiler-version.txt"
dpkg-query -W -f='${Package} ${Version}\n' libeccodes0 libeccodes-dev > "${OUT}/eccodes-version.txt"
(cd "${OUT}" && sha256sum build/grib2-cp-reader-oracle runs/* \
  "${UPSTREAM}/windfields_mod.f90" "${UPSTREAM}/com_mod.f90" "${UPSTREAM}/par_mod.f90" \
  "${DRIVER}" > hashes.txt)
sha256sum "${TRACE}"/prepared-*/EN* >> "${OUT}/hashes.txt"
