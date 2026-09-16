# Native ERA5 model-level mini extract

This fixture contains a small ERA5 Complete GRIB extract for the first ETEX-1
day. It is **real reanalysis data**, retrieved through the Copernicus Climate
Data Store API on 2026-09-16, and is independent of `flexpart-gpu` output.
`request.json` records the exact request, byte count, and SHA-256 digest.

| Property | Value |
| --- | --- |
| Times | 1994-10-23 15:00, 18:00, 21:00 UTC |
| Region | 43–53° N, 8° W–8° E (the existing ETEX mini grid) |
| Horizontal grid | 0.25°, 65 × 41 points |
| Vertical coordinate | All 137 native ERA5 hybrid model levels, with GRIB PV coefficients |
| Fields | Temperature (130), east/west wind (131), north/south wind (132), specific humidity (133), vertical velocity (135) |
| Size | 13,624,650 bytes |

Reproduce the extract with `python scripts/etex/download_native_model_levels.py`
after configuring a personal CDS API token and accepting the ERA5 Complete
dataset terms. The script uses `CDS_API` if provided; otherwise it uses the
standard `cdsapi` configuration. Keep the token outside the repository.

The surface fields for these times and the same area are already in the
SHA-256-checked `../mini/era5-subset.zip`. The native GRIB uses 352°–8° for
longitude, equivalent to −8°–8° in the existing mini arrays. A downstream
converter must normalize this convention and use the GRIB hybrid coefficients
with the surface pressure field. The current ETEX runner still consumes the
older pressure-level fixture; **adding this extract does not yet establish
equivalent forcing or scientific parity**. It is the independent input needed
to implement and test that conversion. Its three timestamps support a short
meteorological comparison, not the existing 12-hour ETEX run.

Contains modified Copernicus Climate Change Service information 2026.
Neither the European Commission nor ECMWF is responsible for any use that may
be made of this information. Dataset and licence:
https://cds.climate.copernicus.eu/datasets/reanalysis-era5-complete
