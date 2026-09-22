#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Build (once) and run the direct FLEXPART 11.1 interpolation oracle driver
# (RISK-03.3G-10c1, issue #71).
#
# This script is executed inside the pinned Fortran oracle container
# (docker/docker-compose.fortran.yml); see scripts/ci-gate.sh for the wiring.
# The driver is linked against the object files of the pinned full FLEXPART
# build produced by that container. Only FLEXPART.o is excluded because it
# defines the model main program.
#
# Usage:
#   direct_oracle.sh <build-dir> <input-file> <output-file>
# ---------------------------------------------------------------------------
set -euo pipefail

BUILD="${1:?usage: direct_oracle.sh <build-dir> <input-file> <output-file>}"
INPUT="${2:?missing oracle input file}"
OUTPUT="${3:?missing oracle output file}"
ORACLE_SRC="${ORACLE_SRC:-/workspace/flexpart/src}"
DRIVER="${DRIVER:-/workspace/flexpart-gpu/scripts/interpolation/direct_interpolation_oracle.f90}"

mkdir -p "${BUILD}"

if [ ! -x "${BUILD}/interpolation-oracle" ]; then
  # shellcheck disable=SC2046
  objects=$(find "${ORACLE_SRC}" -maxdepth 1 -type f -name '*.o' ! -name 'FLEXPART.o' -print | sort)
  if [ -z "${objects}" ]; then
    echo "no FLEXPART objects found in ${ORACLE_SRC}; build the oracle first" >&2
    exit 1
  fi
  gfortran -O0 -I"${ORACLE_SRC}" -fopenmp -mcmodel=large "${DRIVER}" ${objects} \
    -L/usr/lib/x86_64-linux-gnu -Wl,-rpath=/usr/lib/x86_64-linux-gnu \
    -leccodes -leccodes_f90 -lm -lnetcdff \
    -o "${BUILD}/interpolation-oracle"
fi

NM_OUTPUT="${BUILD}/interpolation-oracle.nm"
nm "${BUILD}/interpolation-oracle" > "${NM_OUTPUT}"
grep -q '__interpol_mod_MOD_interpol_rain' "${NM_OUTPUT}"
grep -q '__interpol_mod_MOD_find_vert_vars' "${NM_OUTPUT}"
grep -q '__point_mod_MOD_coordtrafo' "${NM_OUTPUT}"

"${BUILD}/interpolation-oracle" "${INPUT}" "${OUTPUT}"
