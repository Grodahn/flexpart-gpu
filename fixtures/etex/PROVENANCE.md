# ETEX-1 fixture provenance

All measurement, source-term, and meteorological inputs used for ETEX
validation are **independent published data**. Nothing under `fixtures/etex/`
is derived from `flexpart-gpu` outputs.

## Experiment

First European Tracer Experiment (ETEX-1), coordinated by the Joint Research
Centre (JRC) Ispra. Canonical references:

- Nodop, K., Connolly, R., Girardi, F., 1998. The field campaigns of the
  European Tracer Experiment (ETEX): overview and results. Atmospheric
  Environment 32, 4095–4108.
- Girardi, F., et al., 1998. The European Tracer Experiment. EUR 18143 EN,
  Joint Research Centre, Ispra.

## Source term (`real/config/RELEASES`)

| Quantity | Fixture value | Published ETEX-1 value |
| --- | --- | --- |
| Release site | 48.058° N, 2.000° W (Monterfil, Brittany) | 48°03′ N, 2°00′ W (Monterfil) |
| Start | 23 Oct 1994, 16:00 UTC | 23 Oct 1994, 16:00 UTC |
| End | 24 Oct 1994, 03:40 UTC (~11.7 h) | ~12 h release duration |
| Tracer | PMCH (`ETEX1_PMCH`) | PMCH (perfluoromethylcyclohexane) |
| Mass | 3.4E+05 g (FLEXPART `MASS` is in grams) | 340 kg |

Any future change to these values must re-verify them against the references
above and record the deviation here. Model-specific tuning of the source term
to fit `flexpart-gpu` output is not permitted in this directory.

## Measurements (`data/mea95t1.txt`, `data/stations.txt`)

Ground-level ETEX-1 station observations in the original exchange format
(`year mn dy shr dur lat lon pm40 stn`; concentrations in pg/m³ per the
bundled `readme.txt`). The files are consumed read-only by
`scripts/etex/parse_measurements.py`. They are never written by any
`flexpart-gpu` binary, and no comparison script may overwrite them.
The DATEM `dur` field is HHMM, so `0300` denotes a three-hour sample.

## Meteorology (not bundled)

Meteorology is deliberately not vendored here. `scripts/run-etex.sh` downloads
public ARCO-ERA5 arrays from Google Cloud Storage using
`scripts/etex/download_era5_gcs.py`. It prepares Fortran GRIB input with
`prepare_flexpart_input_from_npy.py` and candidate binary input with
`prepare_gpu_meteo.py` from those same arrays. ERA5 is independent of both
models. The downloader records its request parameters, and the comparison
report hashes the downloaded arrays, generated model outputs, manifests, and
observations. It also records the pinned oracle and candidate revisions.

## Reference outputs (not bundled)

FLEXPART Fortran oracle outputs are produced at validation time by the pinned
reference environment (`reference/flexpart-11.1.json`,
`docs/reference-environment.md`) and are never stored as fixtures.

## Guard rails

- The validation harness rejects candidate-derived references for any claimed
  dataset (`EtexValidationError::DerivedReferenceWithDataset`); scaffold runs
  stay explicitly labeled `fixture_scaffold`.
- The `scaffold/` directory holds synthetic plumbing fixtures only and must
  not be cited as ETEX evidence.
- `scripts/run-etex.sh all` requires both the pinned FLEXPART 11.1 run and the
  WGSL candidate run. Missing output or incomplete time coverage is an error.
  The paired report is diagnostic and cannot by itself establish parity.
