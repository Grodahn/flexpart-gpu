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

# Keep the complete link-input set available on every invocation. CI reuses the
# same oracle binary across all fixture cases, so reuse is allowed only when the
# harness, driver, compiler and every linked FLEXPART object are byte-identical.
objects=$(find "${ORACLE_SRC}" -maxdepth 1 -type f -name '*.o' ! -name 'FLEXPART.o' -print | sort)
if [ -z "${objects}" ]; then
  echo "no FLEXPART objects found in ${ORACLE_SRC}; build the oracle first" >&2
  exit 1
fi

COMPILER_VERSION="$(gfortran --version | sed -n '1p')"
FINGERPRINT_OUTPUT="${BUILD}/interpolation-oracle.build-fingerprint.txt"
build_fingerprint="$(
  {
    printf 'harness '
    sha256sum "${BASH_SOURCE[0]}"
    printf 'driver '
    sha256sum "${DRIVER}"
    printf 'compiler %s\n' "${COMPILER_VERSION}"
    printf '%s\n' "${objects}" | while IFS= read -r object; do
      sha256sum "${object}"
    done
  } | sha256sum | awk '{print $1}'
)"
stored_fingerprint=""
if [ -f "${FINGERPRINT_OUTPUT}" ]; then
  stored_fingerprint="$(cat "${FINGERPRINT_OUTPUT}")"
fi

if [ ! -x "${BUILD}/interpolation-oracle" ] || [ "${stored_fingerprint}" != "${build_fingerprint}" ]; then
  tmp_binary="${BUILD}/interpolation-oracle.tmp"
  rm -f "${tmp_binary}"
  # shellcheck disable=SC2046
  gfortran -O0 -I"${ORACLE_SRC}" -fopenmp -mcmodel=large "${DRIVER}" ${objects} \
    -L/usr/lib/x86_64-linux-gnu -Wl,-rpath=/usr/lib/x86_64-linux-gnu \
    -leccodes -leccodes_f90 -lm -lnetcdff \
    -o "${tmp_binary}"
  mv "${tmp_binary}" "${BUILD}/interpolation-oracle"
  printf '%s\n' "${build_fingerprint}" > "${FINGERPRINT_OUTPUT}"
fi

COMPILER_VERSION_OUTPUT="${BUILD}/interpolation-oracle.compiler-version.txt"
LINKED_OBJECTS_OUTPUT="${BUILD}/interpolation-oracle.linked-objects.txt"
printf '%s\n' "${COMPILER_VERSION}" > "${COMPILER_VERSION_OUTPUT}"
printf '%s\n' ${objects} | sed "s#^${ORACLE_SRC}/##" > "${LINKED_OBJECTS_OUTPUT}"

NM_OUTPUT="${BUILD}/interpolation-oracle.nm"
nm "${BUILD}/interpolation-oracle" > "${NM_OUTPUT}"
grep -q '__interpol_mod_MOD_interpol_rain' "${NM_OUTPUT}"
grep -q '__interpol_mod_MOD_find_vert_vars' "${NM_OUTPUT}"
grep -q '__point_mod_MOD_coordtrafo' "${NM_OUTPUT}"

"${BUILD}/interpolation-oracle" "${INPUT}" "${OUTPUT}"
