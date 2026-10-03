#!/usr/bin/env bash
# Build pristine sources in retained scratch, link the direct driver, and run it.
set -euo pipefail
root=/workspace/flexpart-gpu
build="${root}/target/settling-oracle/${1:?missing build identity}"
mkdir -p "${build}/source"
if [ ! -f "${build}/source/src/FLEXPART" ]; then
  tar -xf "${root}/target/settling-oracle/source.tar" -C "${build}/source"
  make -C "${build}/source/src" -f makefile_gfortran FC=gfortran eta=no arch=x86-64 -j4
  # The upstream build stamps only its main program, which is excluded below.
  tar -xf "${root}/target/settling-oracle/source.tar" -C "${build}/source" src/FLEXPART.f90
elif [ -f "${build}/linked-objects.txt" ]; then
  sha256sum -c "${build}/linked-objects.txt"
else
  echo "retained oracle build lacks its object identity record" >&2
  exit 1
fi
src="${build}/source/src"
mapfile -t objects < <(find "${src}" -maxdepth 1 -name '*.o' ! -name FLEXPART.o | sort)
test "${#objects[@]}" -gt 0
gfortran -O3 -march=x86-64 -fopenmp -mcmodel=large -I"${src}" \
  "${root}/oracle/settling_oracle.f90" "${objects[@]}" \
  -L/usr/lib/x86_64-linux-gnu -Wl,-rpath=/usr/lib/x86_64-linux-gnu \
  -leccodes -leccodes_f90 -lm -lnetcdff -o "${build}/settling_oracle"
nm "${build}/settling_oracle" > "${build}/symbols.txt"
grep -q '__settling_mod_MOD_get_settling' "${build}/symbols.txt"
grep -q '__drydepo_mod_MOD_part0' "${build}/symbols.txt"
gfortran --version > "${build}/compiler.txt"
sha256sum "${objects[@]}" > "${build}/linked-objects.txt"
"${build}/settling_oracle" "${root}/target/settling-oracle/inputs.txt" "${build}/outputs.txt"
