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

gfortran -O0 -I"${ORACLE_SRC}" -fopenmp -mcmodel=large -c "${DRIVER}" \
  -o "${BUILD}/w-production-oracle-driver.o"
# The cross-reference table retains which object references each pristine
# symbol, complementing the symbol-presence and runtime-output checks.
# shellcheck disable=SC2046
gfortran -fopenmp -mcmodel=large "${BUILD}/w-production-oracle-driver.o" ${objects} \
  -L/usr/lib/x86_64-linux-gnu -Wl,-rpath=/usr/lib/x86_64-linux-gnu \
  -Wl,-Map="${BUILD}/w-production-oracle.link-map",--cref \
  -leccodes -leccodes_f90 -lm -lnetcdff -o "${BINARY}"

gfortran --version | sed -n '1p' > "${BUILD}/compiler-identity.txt"
printf '%s\n' ${objects} | sed "s#^${ORACLE_SRC}/##" > "${BUILD}/linked-objects.txt"
nm "${BINARY}" > "${BUILD}/w-production-oracle.nm"
{
  objdump -d --disassemble=MAIN__ "${BINARY}"
  objdump -d --disassemble=__interpol_mod_MOD_interpol_wind "${BINARY}"
} > "${BUILD}/w-production-oracle.call-sites"

grep -q '__verttransform_mod_MOD_verttransform_ecmwf_heights' "${BUILD}/w-production-oracle.nm"
grep -q '__verttransform_mod_MOD_verttransform_ecmwf_windfields' "${BUILD}/w-production-oracle.nm"
grep -q '__interpol_mod_MOD_interpol_wind' "${BUILD}/w-production-oracle.nm"
grep -q '__interpol_mod_MOD_interpol_wind_meter' "${BUILD}/w-production-oracle.nm"

"${BINARY}" "${INPUT}" "${OUTPUT}"
