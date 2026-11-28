# ERA5/ETEX native GRIB decoder

[#237](https://github.com/Grodahn/flexpart-gpu/issues/237) defines profile
`era5-etex-native-model-levels-v1`. The ecCodes-backed API reads the two
[ERA5 Complete native fixtures](../fixtures/etex/native-mini/README.md). Each
file contains 2,055 GRIB2 messages for three analysis times, five parameters
and 137 model levels. A model-only decode returns four full-grid fields and
a separate raw omega diagnostic in the `model_levels_only_diagnostic` envelope. A snapshot request without the
pressure file returns `SurfacePressureUnavailable`. [#253](https://github.com/Grodahn/flexpart-gpu/issues/253)
adds profile `era5-etex-native-lnsp-pair-v1` for publication with the two
verified ERA5 Complete log surface pressure files.

## Accepted raw bytes and keys

| Model-level file | Valid UTC on stated date | SHA-256 |
| --- | --- | --- |
| `era5-19941023-151821-ml.grib` | 1994-10-23 15:00, 18:00, 21:00 | `0e059410504dc3d1b2ba13628372a0be81054ff30a75017cf5b06cf9144b5023` |
| `era5-19941024-000306-ml.grib` | 1994-10-24 00:00, 03:00, 06:00 | `893c39cbf0b70129895588aa5c8b43dc819abb129973604147e621c05d5a472b` |

The decoder checks the chosen file's 13,624,650 bytes against the pinned
[day-one request](../fixtures/etex/native-mini/request.json) or
[day-two request](../fixtures/etex/native-mini/request-next-day.json) before
passing those same bytes to ecCodes. Filenames do not establish identity. The
profile requires complete 6,630-byte GRIB2 frames and all expected records
across the file, including times the caller did not select. It rejects other
retrievals, partial or reordered source files with different hashes, forecasts,
unsupported calendar dates, GRIB1, additional packing templates, inconsistent
metadata and bitmaps. The GRIB2 date fields encode Gregorian dates. No alternative
calendar or guessed epoch is accepted.

All messages identify ECMWF `ecmf/0`, MARS `ea/oper/an` experiment `0001`,
local definition 1, generating process 145, product template 0 and an
instantaneous analysis at zero lead. The reference and valid dates and times
must agree. Parameters use discipline 0, WMO table 5 and local table 0.
Hybrid fixed surface type 105 selects levels 1â€“137. Every message carries the
same 276 PV coefficients and is packed with template 5.0 (`grid_simple`,
16 bits, no bitmap). The decoder validates the complete floating-point PV
vector against its little-endian binary64 digest and compares it on every
message. The exact checked ecCodes keys and parameter map are included in
the diagnostic provenance.

| Parameter ID | Category/number | ecCodes units | Diagnostic field |
| --- | --- | --- | --- |
| 130 | 0/0 | `K` | `temperature` |
| 131 | 2/2 | `m s**-1` | `wind_u` (positive eastward) |
| 132 | 2/3 | `m s**-1` | `wind_v` (positive northward) |
| 133 | 1/0 | `kg kg**-1` | `specific_humidity` |
| 135 | 2/8 | `Pa s**-1` | Raw full-level omega diagnostic (positive toward increasing pressure) |

The 65 Ã— 41 regular latitude-longitude grid runs eastward from 352Â° to 8Â°
and southward from 53Â° to 43Â° at 0.25Â° spacing. Scan mode 0 stores X
contiguously and separates latitude rows. The decoder writes a western origin
of âˆ’8Â°, reverses the latitude rows and stores model levels in order 1â€“137.
Values are narrowed from ecCodes binary64 to canonical binary32. This changes
precision only. Cell values retain their physical units and signs. Missing
cells, nonfinite values and invalid thermodynamic domains fail closed.
No interpolation or extrapolation is applied.

All 138 interfaces retain their original A (Pa) and B (dimensionless)
coefficients at canonical precision. As required by the
[canonical schema](meteorology-contract.md), serialized reference pressures
use the fixture's explicit 101325 Pa reference. Full-level reference
pressures average adjacent interface pressures following
`windfields_mod.f90:845â€“846` in the pinned FLEXPART reference. These arrays
describe the coordinate and supply no local atmospheric pressure or height.

## Surface pressure and handoff

ERA5 Complete archives `lnsp` as a two-dimensional field stored at model
level 1. The matching CDS requests retain the original dates, hours, grid,
area, stream, analysis type and GRIB format. They set `levtype=ml`,
`levelist=1` and `param=152`. ECMWF identifies the ERA5 Complete archive
as MARS class `ea`, experiment `0001`. [ERA5 archive organisation](https://confluence.ecmwf.int/pages/viewpage.action?pageId=235634513)
and [ECMWF lnsp retrieval and interpretation](https://confluence.ecmwf.int/pages/viewpage.action?pageId=226496386).

| Original CDS GRIB2 output | Valid UTC on stated date | SHA-256 |
| --- | --- | --- |
| `era5-19941023-151821-lnsp.grib` | 1994-10-23 15:00, 18:00, 21:00 | `5ed2501645d4bbc3adea25bb54d10462d87f6ada8fbc3a3c55e5dc2f9cb0d03d` |
| `era5-19941024-000306-lnsp.grib` | 1994-10-24 00:00, 03:00, 06:00 | `78973555cb774d831925f70252de582d602cce1e12fa4e2a04205d176dfaee4a` |

Each 19,890-byte file holds exactly three messages. The [23 October request](../fixtures/etex/native-mini/era5-19941023-151821-lnsp.request.json)
and [24 October request](../fixtures/etex/native-mini/era5-19941024-000306-lnsp.request.json)
bind the original CDS responses to the original ML request hashes and raw ML
SHA-256. Both original ML requests were replayed through CDS and returned files
identical to the committed ML bytes. The [replay record](../fixtures/etex/native-mini/era5-etex-model-replay-v1.json)
retains the requests and resulting checksums. The join checks the actual
`ecmf/0` origin, `ea/oper/an` experiment
`0001`, generating process 145, local definition 1, analysis reference and
validity date and hour, and instantaneous step zero. It also verifies the full
grid and scan metadata and all raw hybrid coefficients. The independent audit
compares the exact GRIB grid-section bytes by SHA-256 and the binary64 PV digest
at each timestamp. These checks establish the shared ECMWF analysis identity for
this pinned pair.

The pressure messages identify discipline 0, category 3, number 25, parameter
152. ecCodes 2.48.0 labels its units `Numeric`, and 2.28.0 uses `~` for the
same coded parameter. The archived value is the natural
logarithm of pressure in Pa. The decoder exponentiates in binary64, checks
the inclusive 50,000â€“110,000 Pa sanity envelope for this regional fixture,
then narrows to binary32. The surface field is cell-centred X,Y with the same
longitude normalization, row reversal and X-fastest ordering as the four
model-level volumes. Exactly five fields satisfy
`Requirements::real_data_native_levels()`, with 137 levels and 138 interfaces.

The historical [surface NPZ manifest](../fixtures/etex/native-mini/surface-request.json)
pins a 531,362-byte ARCO-ERA5 derivative with SHA-256
`88cd4dcb5e8ff89aeae6c065e3f2c2f75ad09c125f5d15290bfe7a56b059668d`.
The offline audit checks that its six times and normalized grid coincide and
that its pressure array is present and positive. The archive carries a
variable name and time labels, with no original ECMWF product, reference
cycle or packing record tying its pressure to these model-level analyses.
It remains a rejected cross-source control. The snapshot API requires the
exact pressure GRIB bytes and returns a typed source-identity error for ARCO.

## Native omega decision (#257)

ERA5 Complete parameter 135 in these archives is the material pressure
tendency `Dp/Dt` in Pa/s. Positive values indicate increasing parcel
pressure. The sign comes from the ECMWF definition of this pressure
tendency, with no separate sign key in these records. GRIB2 identifies it as discipline/category/number `0/2/8`. The
`hybrid` records carry samples on model levels 1–137, from top to bottom.
Their 138 A/B pairs describe the bounding interfaces. The record count
and the full-level label do not supply values on those interfaces.
See [ERA5 Table 12](https://confluence.ecmwf.int/pages/viewpage.action?pageId=611555221)
and [ECMWF IFS dynamics, §§2.1–2.2](https://www.ecmwf.int/sites/default/files/elibrary/2015/74327-ifs-documentation-cy41r1-part-iii-dynamics-and-numerical-procedures_1.pdf).

The pinned FLEXPART 11.1 reader recognizes W in GRIB2 through category/number
`2/32` or `128/77`. The GRIB1 parameter-77 alias to internal code 135
does not apply to these GRIB2 records. The reader places accepted W in
`wwh(nlev_ec-k+1)` without a sign flip. Other 3-D fields use
`nlev_ec-k+2`. W slot 1 is the surface, and the full-column reader sets
the additional top slot `nlev_ec+1` to zero. The transform samples the W
slots at interface heights, including the zero-height surface, and
multiplies by `pinmconv = dz/dp` before height interpolation. The negative
gradient gives the geometric result its positive-upward convention.
See pinned
[`windfields_mod.f90`](https://gitlab.phaidra.org/flexpart/flexpart/-/blob/c70586c2b7f5258850705325881c61f557ea9bd8/src/windfields_mod.f90#L2096)
and
[`verttransform_mod.f90`](https://gitlab.phaidra.org/flexpart/flexpart/-/blob/c70586c2b7f5258850705325881c61f557ea9bd8/src/verttransform_mod.f90#L1977).

Pristine `flex_extract` 7.1.2 reads raw parameter 135 from `fort.19`
under `MOMEGA` and copies its values to diagnostic `fort.25`. The
regular-grid preprocessing profile pinned by #70 instead reads raw
eta-dot (77) from `fort.21`. With `MDPDETA=1` it applies the hybrid
pressure scaling and top-to-bottom recurrence at lines 545–557, then
writes the prepared `ETA` values into `fort.15`. The production
assembly consumes `fort.15` and writes the raw omega diagnostics
separately. The chain rule distinguishes the quantities:
`Dp/Dt = (dp/deta) Deta/Dt + (partial_t + u·grad_eta) p`, where the
second term includes local and horizontal changes in hybrid pressure
at fixed eta. The archive therefore supplies neither the verified
hybrid-coordinate tendency scaled to Pa/s nor the interface placement
required by the pinned reader. Relabeling or interpolating the 135
records alone does not establish the handoff. See pinned
[`calc_etadot.f90`](https://gitlab.phaidra.org/flexpart/flex_extract/-/blob/e0005c99ac81d12faa45a8ff799debbd592b0dc0/Source/Fortran/calc_etadot.f90#L508)
and
[`EcFlexpart.py`](https://gitlab.phaidra.org/flexpart/flex_extract/-/blob/e0005c99ac81d12faa45a8ff799debbd592b0dc0/Source/Python/Classes/EcFlexpart.py#L1757).

The typed result is `OmegaHandoff::RequiresPreprocessing`. Inspect
`ModelLevelsDiagnostic::omega_pa_s()` or the CLI's `omega_pa_s`
array for the independently checked 65 × 41 × 137 volume. The API
does not construct a #30 `NativeVerticalMotion` or expose a canonical
geometric W field. Each result retains the analysis time, raw source and
request SHA-256, PV identity, binary32 value digest and hash of the
[decision record](../fixtures/meteorology/era5-etex-native-omega-v1.decision.json).
The decision pins the source commits, Git blobs, SHA-256 of the canonical
source bytes and line ranges. The
#253 snapshot payload still contains the same five canonical fields.
It fails full advection requirements. #32-D needs independently proven
interface W, T2m, Td2m and orography, plus temporal and paired production
evidence. The #70 eta-dot production lock still applies.

## Run and reproduce

On Linux, install ecCodes development headers and pkg-config, libclang for the
existing Rust ecCodes binding, and the Rust toolchain. From the repository root:

```sh
cargo run --locked --features eccodes --bin era5-etex-decode -- \
  diagnostic fixtures/etex/native-mini/era5-19941023-151821-ml.grib \
  782924400 > era5-etex-19941023T15.diagnostic.json

cargo run --locked --features eccodes --bin era5-etex-decode -- \
  snapshot fixtures/etex/native-mini/era5-19941023-151821-ml.grib \
  782924400 fixtures/etex/native-mini/era5-19941023-151821-lnsp.grib \
  > era5-etex-19941023T15.snapshot.json
```

The CLI writes a diagnostic envelope. Its `model_levels` payload uses
canonical metadata with the mandatory surface dependency unresolved. The
`snapshot` mode writes a `snapshot` payload and sibling `provenance` only after
both raw inputs and the canonical payload pass validation. Rust callers can
use `load_snapshot_with_surface` and extract the validated `Snapshot`. The
provenance records raw and request-record SHA-256, tested GRIB keys, PV hash,
output schema, transformations, linked ecCodes API version, decoder-source
and Cargo dependency-lock SHA-256, and the SHA-256 of the compact Serde JSON
bytes of the payload.

```sh
python3 -I -m unittest discover -s scripts/etex -p test_retrieve_etex_lnsp.py
python3 -I -m unittest discover -s scripts/fixtures -p test_era5_etex_native_values.py
python3 -I scripts/fixtures/check_era5_etex_native_values.py
cargo test --locked --features eccodes --lib io::era5_etex
cargo test --locked --features eccodes --test era5_etex_decoder
```

The Python check uses ecCodes coordinates to sort raw cells and regenerates
[24 volume digests](../fixtures/meteorology/era5-etex-native-v1.value-digests.json)
and [six pressure-grid digests](../fixtures/meteorology/era5-etex-native-lnsp-v1.value-digests.json)
independently of the Rust normalization. The pressure audit applies Python
`math.exp` before binary32 rounding and records each raw GRIB message's SHA-256,
analysis metadata, grid and PV hashes, and pressure extrema. The six grids
span 74,612.28125â€“101,580.0703125 Pa. Rust verifies every cell across all six
times and all levels by binary32 digest and checks the
[canonical payload SHA-256](../fixtures/meteorology/era5-etex-native-lnsp-v1.snapshot-digests.json)
for each time. At the first time it also compares
the four fields to the checked-in [2 Ã— 2 canonical reference](../fixtures/meteorology/era5-etex-native-v1.json)
at all levels and checks both complete PV arrays and reference pressure arrays.
The [omega evidence](../fixtures/meteorology/era5-etex-native-omega-v1.value-digests.json)
binds all 822 raw omega messages to the verified #253 grid, PV and
analysis identities and records binary64 and binary32 digests, samples,
extrema and sign counts. The six omega volumes contain 2,190,630 samples.
Rust and the separately executed Python ecCodes audit agree bit-for-bit
at binary32 precision. The maximum ecCodes binary64 to binary32 rounding
difference is 1.1920928955078125e-7 Pa/s. Binary32 values range from
−2.110884428 to +2.703836203 Pa/s, with no missing or nonfinite cells.
The direct ecCodes payload and PV digests require exact equality. The frozen
#29 JSON differs from the direct binary32 decode by at most one ULP in the
four field slices and reference pressure arrays. Sixteen of its A coefficients
differ from the bytes stored in the GRIB by at most 0.000244141 Pa, so the
reference comparison bounds A at 0.00025 Pa and requires exact B values.
The reference is read only after decoding for comparison and is absent from
both the Rust decoder input and the direct-ecCodes audit. The checks provide
source-decoding evidence and do not establish FLEXPART simulation parity.

After accepting the ERA5 Complete terms, install `cdsapi` and configure the
personal access token in `~/.cdsapirc` or `CDS_API`. From the repository root,
run the original MARS requests through CDS in fresh output directories.

```sh
python scripts/etex/retrieve_etex_lnsp.py 23 target/etex-lnsp
python scripts/etex/retrieve_etex_lnsp.py 24 target/etex-lnsp
python scripts/etex/retrieve_etex_lnsp.py 23 target/etex-replay --replay-model
python scripts/etex/retrieve_etex_lnsp.py 24 target/etex-replay --replay-model
```

The recipe uses the official CDS API with TLS verification. It publishes the
response and receipt only after the download matches the checked-in byte
count and SHA-256. A mismatched response stays in the `.part` file. Retrieval
failures report the client error class and any HTTP status. Offline verification
requires no CDS credentials. See the
[fixture README](../fixtures/etex/native-mini/README.md) for the Copernicus notice.
