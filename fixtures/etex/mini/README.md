# ETEX-1 mini run

`scripts/run-etex.sh mini` runs the pinned FLEXPART 11.1 oracle and the WGSL
candidate from the six native-level ERA5 snapshots in `../native-mini/`.
The meteorology covers 1994-10-23 15:00 through 1994-10-24 06:00 UTC, which
brackets the 16:00–04:00 simulation. The old pressure-level weather archive
has been retired. The independent ETEX station observations remain in
`../data/`.

`scripts/etex/verify_native_model_levels.py` checks all ERA5 input hashes,
grids, fields and times. `scripts/etex/prepare_native_era5.py` writes Fortran
GRIB files and the GPU binary meteorology from these inputs. The Fortran files
retain the 137 native hybrid model levels and include ERA5 eta-coordinate
velocity (`etadot`); the GPU receives 16 fixed AGL levels sampled from the
native fields and uses ERA5 pressure velocity (`omega`) converted to m/s.
Both runs use the same ERA5 dates, area and surface fields, but their vertical
representations and velocity coordinates still require a quantitative
equivalence check before claiming concentration parity.

Run `scripts/run-etex.sh mini` with Docker Compose, Cargo, NumPy and a software
WGSL adapter (`FLEXPART_GPU_SOFTWARE=1` where needed). The paired report and
raw outputs are written under `target/etex/mini/`; they are diagnostic data,
not a scientific parity verdict.
