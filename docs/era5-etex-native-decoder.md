# ERA5/ETEX native GRIB decoder

[#237](https://github.com/Grodahn/flexpart-gpu/issues/237) defines profile
`era5-etex-native-model-levels-v1`. The ecCodes-backed API reads the two
[ERA5 Complete native fixtures](../fixtures/etex/native-mini/README.md). Each
file contains 2,055 GRIB2 messages for three analysis times, five parameters
and 137 model levels. The selected time returns four full-grid fields as an
explicit `model_levels_only_diagnostic`. A request for a validated hybrid
`meteorology::Snapshot` returns `SurfacePressureUnavailable` after successful
model-level decoding. Schema v1 requires actual surface pressure for hybrid
snapshots.

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
Hybrid fixed surface type 105 selects levels 1–137. Every message carries the
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
| 135 | 2/8 | `Pa s**-1` | Checked for source coverage and omitted |

The 65 × 41 regular latitude-longitude grid runs eastward from 352° to 8°
and southward from 53° to 43° at 0.25° spacing. Scan mode 0 stores X
contiguously and separates latitude rows. The decoder writes a western origin
of −8°, reverses the latitude rows and stores model levels in order 1–137.
Values are narrowed from ecCodes binary64 to canonical binary32. This changes
precision only. Cell values retain their physical units and signs. Missing
cells, nonfinite values and invalid thermodynamic domains fail closed.
No interpolation or extrapolation is applied.

All 138 interfaces retain their original A (Pa) and B (dimensionless)
coefficients at canonical precision. As required by the
[canonical schema](meteorology-contract.md), serialized reference pressures
use the fixture's explicit 101325 Pa reference. Full-level reference
pressures average adjacent interface pressures following
`windfields_mod.f90:845–846` in the pinned FLEXPART reference. These arrays
describe the coordinate and supply no local atmospheric pressure or height.

## Surface pressure and handoff

The [surface NPZ manifest](../fixtures/etex/native-mini/surface-request.json)
pins a 531,362-byte ARCO-ERA5 derivative with SHA-256
`88cd4dcb5e8ff89aeae6c065e3f2c2f75ad09c125f5d15290bfe7a56b059668d`.
The offline audit checks that its six times and normalized grid coincide and
that its pressure array is present and positive. The archive carries a
variable name and time labels, with no original ECMWF product, reference
cycle or packing record tying its surface pressure to these model-level
analyses. Matching valid times and spatial coordinates alone cannot verify
that join. The decoder consumes no ancillary bytes. Its provenance states this
and records the missing surface-pressure decision.

For #32-C, admit a checksum-pinned surface-pressure source and prove its
analysis/cycle, grid points and scan order, parameter/unit and valid-time
identity against the model source. Any alternative or logarithmic
representation needs its own verified semantics and explicit preprocessing.
The diagnostic exposes typed canonical field and coordinate metadata for
testing that handoff. For #32-D, first obtain validated snapshots with the
mandatory surface and applicable ancillary fields. Then validate the actual
#30 geometry and vertical motion, #31 interpolation and #76 composition
through the production path and its governing gates.

## Run and reproduce

On Linux, install ecCodes development headers and pkg-config, libclang for the
existing Rust ecCodes binding, and the Rust toolchain. From the repository root:

```sh
cargo run --locked --features eccodes --bin era5-etex-decode -- \
  diagnostic fixtures/etex/native-mini/era5-19941023-151821-ml.grib \
  782924400 > era5-etex-19941023T15.diagnostic.json
```

The CLI writes a diagnostic envelope. Its `model_levels` payload uses
canonical metadata, with the mandatory surface dependency unresolved. The
`snapshot` mode exits nonzero and produces no JSON for the existing inputs.
Provenance includes raw and request-record SHA-256, the tested keys, the PV
hash, output schema, transformations, linked ecCodes API version, decoder
source and Cargo dependency-lock SHA-256, and SHA-256 over the compact Serde
JSON bytes of the actual `model_levels` payload. Local execution may also
retain `rustc -vV` and `pkg-config --modversion eccodes` with the report.

```sh
python3 -I scripts/fixtures/check_era5_etex_native_values.py
cargo test --locked --features eccodes --lib io::era5_etex
cargo test --locked --features eccodes --test era5_etex_decoder
```

The Python check uses ecCodes coordinates to sort raw cells and regenerates
[24 volume digests](../fixtures/meteorology/era5-etex-native-v1.value-digests.json)
independently of the Rust normalization. Rust verifies every cell across all
six times and all levels by binary32 digest. At the first time it also compares
the four fields to the checked-in [2 × 2 canonical reference](../fixtures/meteorology/era5-etex-native-v1.json)
at all levels and checks both complete PV arrays and reference pressure arrays.
The direct ecCodes payload and PV digests require exact equality. The frozen
#29 JSON differs from the direct binary32 decode by at most one ULP in the
four field slices and reference pressure arrays. Sixteen of its A coefficients
differ from the bytes stored in the GRIB by at most 0.000244141 Pa, so the
reference comparison bounds A at 0.00025 Pa and requires exact B values.
The reference is read only after decoding for comparison and is absent from
both the Rust decoder input and the direct-ecCodes audit. The checks provide
source-decoding evidence and do not establish FLEXPART simulation parity.

See the [fixture README](../fixtures/etex/native-mini/README.md) for the
Copernicus notice, licence information and retrieval procedure. Offline
verification uses the committed data and requires no CDS credentials.
