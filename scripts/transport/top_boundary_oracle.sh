#!/usr/bin/env bash
# #180 direct transport experiment, using #118/#80's pristine build/link conventions.
set -euo pipefail
CHECKOUT="${1:?pristine checkout required}"
OUT="${2:?new output required}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PIN=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pinned_commit"])' "$ROOT/reference/flexpart-11.1.json")
git config --global --add safe.directory "$CHECKOUT"
test "$(git -C "$CHECKOUT" rev-parse HEAD)" = "$PIN"
test -z "$(git -C "$CHECKOUT" status --porcelain)"
test ! -e "$OUT"
mkdir -p "$OUT/build"
WORK=$(mktemp -d /tmp/flexpart-top-research.XXXXXXXX)
trap 'rm -rf -- "$WORK"' EXIT
mkdir -p "$WORK/build/upstream"
git -C "$CHECKOUT" rev-parse HEAD > "$OUT/checkout-before.txt"
git -C "$CHECKOUT" archive HEAD:src | tar -x -C "$WORK/build/upstream"
(cd "$WORK/build/upstream"; make -f makefile_gfortran FC=gfortran eta=no arch=x86-64 -j4) > "$OUT/full-build.log" 2>&1
export ORACLE_SRC="$WORK/build/upstream"
# Private procedure access changes ELF symbol visibility only, never source or code.
# Preserve the original object and both .text byte streams for independent auditing.
cp "$ORACLE_SRC/advance_mod.o" "$WORK/build/advance_mod.original.o"
objcopy --dump-section .text="$WORK/build/advance-original.text" "$ORACLE_SRC/advance_mod.o"
objcopy --globalize-symbol=__advance_mod_MOD_pushpartdown "$ORACLE_SRC/advance_mod.o"
objcopy --dump-section .text="$WORK/build/advance-exported.text" "$ORACLE_SRC/advance_mod.o"
cmp "$WORK/build/advance-original.text" "$WORK/build/advance-exported.text"
export DRIVER="$ROOT/scripts/transport/top_boundary_oracle.f90"
export OMP_NUM_THREADS=1 OMP_THREAD_LIMIT=1 OMP_DYNAMIC=FALSE OMP_NESTED=FALSE
export OMP_MAX_ACTIVE_LEVELS=1 OMP_SCHEDULE=static OMP_PROC_BIND=FALSE OMP_WAIT_POLICY=PASSIVE
cp "$ROOT/fixtures/interpolation/shared-height-v1/input.txt" "$OUT/input.txt"
bash "$ROOT/scripts/interpolation/w_production_oracle.sh" "$WORK/build" "$OUT/input.txt" "$OUT/output.txt" > "$OUT/driver-build-run.log" 2>&1
objdump -dr "$WORK/build/advance_mod.original.o" > "$WORK/build/advance-original.disassembly"
objdump -dr "$ORACLE_SRC/advance_mod.o" > "$WORK/build/advance-exported.disassembly"
objdump -d "$WORK/build/w-production-oracle" > "$WORK/build/executable.disassembly"
readelf -a "$WORK/build/advance_mod.original.o" > "$WORK/build/advance-original.elf"
readelf -a "$ORACLE_SRC/advance_mod.o" > "$WORK/build/advance-exported.elf"
cp -a "$WORK/build/." "$OUT/build/"
test -z "$(git -C "$CHECKOUT" status --porcelain)"
git -C "$CHECKOUT" rev-parse HEAD > "$OUT/checkout-after.txt"
python3 "$ROOT/scripts/transport/verify_top_boundary_oracle.py" --checkout "$CHECKOUT" --run "$OUT" "${@:3}"
