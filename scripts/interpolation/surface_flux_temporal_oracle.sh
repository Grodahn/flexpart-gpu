#!/usr/bin/env bash
# Build and run the #120 direct surface-flux temporal oracle inside the
# established Fortran image. The authoritative checkouts are mounted read-only.
set -euo pipefail
CHECKOUT="${1:?usage: surface_flux_temporal_oracle.sh <pristine-flexpart> <pristine-flex_extract> <new-output-dir>}"
FLEX_EXTRACT="${2:?missing pristine flex_extract checkout}"
OUT="${3:?missing new output directory}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FIXTURE="${ROOT}/fixtures/interpolation/surface-flux-temporal-v1"
PIN=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pinned_commit"])' \
  "${ROOT}/reference/flexpart-11.1.json")
git config --global --add safe.directory "${CHECKOUT}"
git config --global --add safe.directory "${FLEX_EXTRACT}"
test "$(git -C "${CHECKOUT}" rev-parse HEAD)" = "${PIN}" || { echo 'Wrong pinned oracle revision' >&2; exit 1; }
test -z "$(git -C "${CHECKOUT}" status --porcelain)" || { echo 'Dirty oracle checkout' >&2; exit 1; }
test ! -e "${OUT}" || { echo 'Output directory already exists' >&2; exit 1; }
mkdir -p "${OUT}/build"
# Compile a scratch copy so the upstream build cannot modify the authoritative
# checkout. No copied routine is edited.
WORK=$(mktemp -d /tmp/flexpart-surface-flux.XXXXXXXX)
trap 'rm -rf -- "${WORK}"' EXIT
mkdir -p "${WORK}/upstream"
git -C "${CHECKOUT}" archive HEAD:src | tar -x -C "${WORK}/upstream"
(
  cd "${WORK}/upstream"
  make -f makefile_gfortran FC=gfortran eta=no arch=x86-64 -j4
) > "${OUT}/full-build.log" 2>&1
cp "${FIXTURE}/input.txt" "${OUT}/input.txt"
export OMP_NUM_THREADS=1 OMP_THREAD_LIMIT=1 OMP_DYNAMIC=FALSE OMP_NESTED=FALSE
export OMP_MAX_ACTIVE_LEVELS=1 OMP_SCHEDULE=static OMP_PROC_BIND=FALSE OMP_WAIT_POLICY=PASSIVE
{
  set -euo pipefail
  objects=$(find "${WORK}/upstream" -maxdepth 1 -type f -name '*.o' ! -name 'FLEXPART.o' -print | sort)
  test -n "${objects}" || { echo 'no pristine FLEXPART objects' >&2; exit 1; }
  cd "${WORK}"
  gfortran -O0 -I"${WORK}/upstream" -fopenmp -mcmodel=large -c \
    "${ROOT}/scripts/interpolation/surface_flux_temporal_oracle.f90" \
    -o surface-flux-temporal-oracle-driver.o
  # shellcheck disable=SC2086
  gfortran -fopenmp -mcmodel=large surface-flux-temporal-oracle-driver.o ${objects} \
    -L/usr/lib/x86_64-linux-gnu -Wl,-rpath=/usr/lib/x86_64-linux-gnu \
    -Wl,-Map=surface-flux-temporal-oracle.link-map,--cref \
    -leccodes -leccodes_f90 -lm -lnetcdff -o surface-flux-temporal-oracle
  gfortran --version | sed -n '1p' > compiler-identity.txt
  printf '%s\n' ${objects} | sed "s#^${WORK}/upstream/##" > linked-objects.txt
  nm surface-flux-temporal-oracle > surface-flux-temporal-oracle.nm
  objdump -d --disassemble=MAIN__ surface-flux-temporal-oracle > surface-flux-temporal-oracle.call-sites
  ./surface-flux-temporal-oracle "${OUT}/input.txt" "${OUT}/output.txt"
} > "${OUT}/driver-build-run.log" 2>&1
cp -a "${WORK}/upstream" "${OUT}/build/upstream"
cp -a "${WORK}"/surface-flux-temporal-oracle* "${WORK}/compiler-identity.txt" \
  "${WORK}/linked-objects.txt" "${OUT}/build/"
test -z "$(git -C "${CHECKOUT}" status --porcelain)"
python3 "${ROOT}/scripts/interpolation/prepare_surface_flux_temporal_oracle.py" \
  --checkout "${CHECKOUT}" --flex-extract "${FLEX_EXTRACT}" --run "${OUT}" --expected "${FIXTURE}"
