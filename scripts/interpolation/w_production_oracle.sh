#!/usr/bin/env bash
# Build and run the issue #80 pristine FLEXPART 11.1 eta=no W production oracle.
set -euo pipefail

BUILD="${1:?usage: w_production_oracle.sh <build-dir> <input-file> <output-file>}"
INPUT="${2:?missing oracle input file}"
OUTPUT="${3:?missing oracle output file}"
ORACLE_SRC="${ORACLE_SRC:-/workspace/flexpart/src}"
DRIVER="${DRIVER:-/workspace/flexpart-gpu/scripts/interpolation/w_production_oracle.f90}"
BINARY="${BUILD}/w-production-oracle"

mkdir -p "${BUILD}"
objects=$(find "${ORACLE_SRC}" -maxdepth 1 -type f -name '*.o' ! -name 'FLEXPART.o' -print | sort)
if [ -z "${objects}" ]; then
  echo "no pristine FLEXPART objects found in ${ORACLE_SRC}" >&2
  exit 1
fi

compile_driver() {
  set -euo pipefail
  (
  cd "${BUILD}"
  gfortran -O0 -I"${ORACLE_SRC}" -fopenmp -mcmodel=large -c "${DRIVER}" \
    -o w-production-oracle-driver.o
  # The cross-reference table retains which object references each pristine
  # symbol, complementing the symbol-presence and runtime-output checks. Using
  # build-relative names keeps the retained evidence independent of its parent
  # artifact directory.
  # shellcheck disable=SC2046
  gfortran -fopenmp -mcmodel=large w-production-oracle-driver.o ${objects} \
    -L/usr/lib/x86_64-linux-gnu -Wl,-rpath=/usr/lib/x86_64-linux-gnu \
    -Wl,-Map=w-production-oracle.link-map,--cref \
    -leccodes -leccodes_f90 -lm -lnetcdff -o w-production-oracle

  gfortran --version | sed -n '1p' > compiler-identity.txt
  printf '%s\n' ${objects} | sed "s#^${ORACLE_SRC}/##" > linked-objects.txt
  nm w-production-oracle > w-production-oracle.nm
  {
    objdump -d --disassemble=MAIN__ w-production-oracle
    objdump -d --disassemble=__interpol_mod_MOD_interpol_wind w-production-oracle
  } > w-production-oracle.call-sites
  )
}
export BUILD ORACLE_SRC DRIVER objects
export -f compile_driver
python3 /workspace/flexpart-gpu/scripts/oracle_build_cache.py cached-command \
  --metadata "${BUILD}/build.json" --input "${BASH_SOURCE[0]}" --input "${DRIVER}" \
  --artifact "${BINARY}" --artifact "${BUILD}/w-production-oracle-driver.o" \
  --artifact "${BUILD}/w-production-oracle.link-map" --artifact "${BUILD}/compiler-identity.txt" \
  --artifact "${BUILD}/linked-objects.txt" --artifact "${BUILD}/w-production-oracle.nm" \
  --artifact "${BUILD}/w-production-oracle.call-sites" --command bash -c compile_driver

grep -q '__verttransform_mod_MOD_verttransform_ecmwf_heights' "${BUILD}/w-production-oracle.nm"
grep -q '__verttransform_mod_MOD_verttransform_ecmwf_windfields' "${BUILD}/w-production-oracle.nm"
grep -q '__interpol_mod_MOD_interpol_wind' "${BUILD}/w-production-oracle.nm"
grep -q '__interpol_mod_MOD_interpol_wind_meter' "${BUILD}/w-production-oracle.nm"

"${BINARY}" "${INPUT}" "${OUTPUT}"
