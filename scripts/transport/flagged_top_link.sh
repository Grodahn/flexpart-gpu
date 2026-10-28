#!/usr/bin/env bash
# Same #80/#180 compiler/link recipe, with separately compiled observers.
set -euo pipefail
BUILD="${1:?build required}"
INPUT="${2:?input required}"
OUTPUT="${3:?output required}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$BUILD"
mapfile -t objects < <(find "$ORACLE_SRC" -maxdepth 1 -type f -name '*.o' ! -name FLEXPART.o | sort)
test "${#objects[@]}" -gt 20
gfortran -O0 -I"$ORACLE_SRC" -fopenmp -mcmodel=large -c \
  "$ROOT/scripts/transport/flagged_top_observer.f90" -o flagged-top-observer.o
gfortran -O0 -I"$ORACLE_SRC" -fopenmp -mcmodel=large -c "$DRIVER" -o w-production-oracle-driver.o
symbols=(__interpol_mod_MOD_interpol_wind __interpol_mod_MOD_interpol_wind_short \
         __particle_mod_MOD_update_z_sp __particle_mod_MOD_set_z_sp)
wrap=(); aliases=()
for symbol in "${symbols[@]}"; do
  wrap+=("-Wl,--wrap=$symbol")
  aliases+=("-Wl,--defsym=__real_$symbol=$symbol")
done
link() {
  local binary="$1"; shift
  gfortran -fopenmp -mcmodel=large w-production-oracle-driver.o flagged-top-observer.o "${objects[@]}" \
    -L/usr/lib/x86_64-linux-gnu -Wl,-rpath=/usr/lib/x86_64-linux-gnu \
    "-Wl,-Map=$binary.link-map,--cref" "$@" -leccodes -leccodes_f90 -lm -lnetcdff -o "$binary"
}
link w-production-oracle "${wrap[@]}"
# An unwrapped execution of the same public routine falsifies observer side effects.
link unwrapped-control "${aliases[@]}"
gfortran --version | sed -n '1p' > compiler-identity.txt
printf '%s\n' "${objects[@]}" | sed "s#^${ORACLE_SRC}/##" > linked-objects.txt
nm w-production-oracle > w-production-oracle.nm
objdump -d --disassemble=MAIN__ w-production-oracle > w-production-oracle.call-sites
./w-production-oracle "$INPUT" "$OUTPUT"
./unwrapped-control "$INPUT" "${OUTPUT%.txt}-unwrapped.txt"
