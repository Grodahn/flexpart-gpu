# Native ERA5 meteorology for the ETEX-1 mini run

This checked-in fixture contains real ERA5 meteorology for the ETEX-1 release
window. The native model-level GRIB files were retrieved from ERA5 Complete
through the CDS API on 2026-09-16. The surface archive contains six matching
snapshots selected from independently downloaded ARCO-ERA5 hourly arrays.
The request and SHA-256 records are in `request.json`,
`request-next-day.json`, `request-etadot.json` and `surface-request.json`.

| Property | Value |
| --- | --- |
| Times | 1994-10-23 15/18/21 and 1994-10-24 00/03/06 UTC |
| Region | 43–53° N, 8° W–8° E |
| Horizontal grid | 0.25°, 65 × 41 points |
| Vertical coordinate | All 137 ERA5 hybrid model levels, with 276 PV coefficients |
| Native fields | Temperature (130), u (131), v (132), humidity (133), pressure velocity (135), eta-coordinate velocity (77) |
| Surface fields | 15 fields including pressure, near-surface weather, PBL height, fluxes and precipitation |

The GRIB files express the western longitude as 352°, equivalent to −8° in
the surface archive. The converter checks both grids and normalizes the GPU
domain to −8°–8°. Run `scripts/etex/verify_native_model_levels.py` in the
Fortran container to check source hashes and complete field coverage before
preparing a run.

To reproduce the native retrieval, use
`scripts/etex/download_native_model_levels.py` with a personal CDS API token
after accepting the ERA5 Complete terms. To reproduce the surface archive,
download the same 16 hourly ARCO-ERA5 subset with
`scripts/etex/download_era5_gcs.py`, then select six times using
`scripts/etex/build_native_surface_fixture.py`. Keep credentials outside this
repository. The former pressure-level archive hash is recorded in
`surface-request.json` for source lineage; that archive is no longer needed by
the run.

Contains modified Copernicus Climate Change Service information 2026.
Neither the European Commission nor ECMWF is responsible for any use that may
be made of this information. ERA5 Complete:
https://cds.climate.copernicus.eu/datasets/reanalysis-era5-complete
