# ETEX-1 mini weather fixture

`era5-subset.zip` is a small, real ERA5 subset for a reproducible 12-hour
ETEX-1 smoke run. The archive is checked by `sha256.json` before extraction.
It contains 16 hourly fields from 1994-10-23 15:00 through 1994-10-24 06:00
UTC over 43–53° N and 8° W–8° E on a 0.25° grid. Five atmospheric fields are
stored at 14 pressure levels; 15 surface fields are also included. The
uncompressed arrays are about 15 MB. `metadata.json` inside the archive lists
the exact variables, shapes, levels, and source store.

Source: Google ARCO-ERA5, public Zarr store
`gs://gcp-public-data-arco-era5/ar/full_37-1h-0p25deg-chunk-1.zarr-v3`,
accessed 2026-09-16. The downloader is
`scripts/etex/download_era5_gcs.py`; it can reproduce the subset with:

```bash
python scripts/etex/download_era5_gcs.py --output-dir target/etex/mini_raw \
  --time-start 1994-10-23T15:00 --time-end 1994-10-24T06:00 \
  --lon-west -8 --lon-east 8 --lat-north 53 --lat-south 43
python scripts/etex/mini_fixture.py pack --source target/etex/mini_raw \
  --archive fixtures/etex/mini/era5-subset.zip \
  --manifest fixtures/etex/mini/sha256.json
```

Contains modified Copernicus Climate Change Service information 2026.
Neither the European Commission nor ECMWF is responsible for any use that may
be made of this information. Licence:
https://cds.climate.copernicus.eu/licences/licence-to-use-copernicus-products

Run `scripts/run-etex.sh mini` to unpack, prepare, execute both models, and
compare with the independent ETEX station data in `../data/`. This requires
the unmodified pinned FLEXPART checkout, Docker Compose, Cargo, and Python
with NumPy. Set `FLEXPART_GPU_SOFTWARE=1` for a software WGSL adapter.
`ETEX_PYTHON` may name a Python executable with NumPy installed.

**Interpretation limit:** ERA5 pressure-level fields are interpolated to an
approximate terrain-following coordinate for the Fortran input. The candidate
uses the original pressure-level subset and simplified level heights. These
are different meteorological representations. This mini fixture verifies the
data pipeline, model execution, provenance, output windows, and observation
pairing. Its concentration metrics are diagnostics, not a scientific parity
claim or a substitute for native FLEXPART model-level forcing.
