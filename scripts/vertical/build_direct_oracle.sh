#!/usr/bin/env bash
# Compile #30's direct driver and conformance harness; execution stays in ci-gate.sh.
set -euo pipefail
build="${1:?build directory required}"
oracle_src=/workspace/flexpart/src
mkdir -p "${build}"
cd "${build}"
objects=$(find "$oracle_src" -maxdepth 1 -type f -name '*.o' ! -name 'FLEXPART.o' -print | sort | tr '\n' ' ')
test -n "$objects"
test -f "$oracle_src/verttransform_mod.o"
test -f "$oracle_src/windfields_mod.o"

gfortran -O0 -I"$oracle_src" -fopenmp -mcmodel=large \
  /workspace/flexpart-gpu/scripts/vertical/direct_oracle_driver.f90 \
  $objects \
  -L/usr/lib/x86_64-linux-gnu -Wl,-rpath=/usr/lib/x86_64-linux-gnu \
  -leccodes -leccodes_f90 -lm -lnetcdff \
  -o "$build/flexpart-vertical-routine-oracle"

nm "$build/flexpart-vertical-routine-oracle" > "$build/flexpart-vertical-routine-oracle.symbols"
grep -q '__verttransform_mod_MOD_verttransform_ecmwf_heights' \
  "$build/flexpart-vertical-routine-oracle.symbols"
sha256sum "$oracle_src/verttransform_mod.o" > "$build/verttransform_mod.o.sha256"
sha256sum "$oracle_src/windfields_mod.o" > "$build/windfields_mod.o.sha256"

gfortran -O0 -J"$build" -I"$build" \
  "$oracle_src/par_mod.f90" \
  "$oracle_src/qvsat_mod.f90" \
  /workspace/flexpart-gpu/scripts/vertical/oracle_column.f90 \
  -o "$build/vertical-conformance-harness"
