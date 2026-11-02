# IFS deterministic Open Data provider audit (#238)

Research decision for [#238](https://github.com/Grodahn/flexpart-gpu/issues/238),
parent [#32](https://github.com/Grodahn/flexpart-gpu/issues/32), from main
`daee70a8dca1c446193a2ca7ddb55a760035ed69`. No decoder, physics, canonical
schema or GPU execution is added. No FLEXPART parity or operational readiness
is demonstrated. Evidence level: real-source metadata/value decoding and a
provider-compatibility decision against current contracts.

## Decision and consumer scope

**NO_GO** for every existing production 3-D dispersion consumer supplied solely
by the audited `ifs / oper / fc / 0p1` product, and separately **NO_GO** for full
Europe 48-hour P0 physics and the approximately 5–8 km target. There is **no
production-capable consumer subset** under the present contracts.

Individual surface quantities (SP, 10 m wind, 2 m temperature/dewpoint, land
fraction, snow water equivalent, TCC) have compatible decoded units. They could
support a future separately verified diagnostic adapter: **CONDITIONAL_GO for
surface diagnostics only**. This is not a claim that such an adapter exists or
that these fields satisfy a physics requirement set. Even quantity identities
need regional extraction, scan normalization and canonical validation.

The finite [30-field matrix](../../fixtures/meteorology/ifs-open-audit-v1/provider-contract.json)
records source parameter identifiers, levels, units, time meaning, availability,
transformations, original consumer traces, evidence, fatal behavior and blocker
owners. Supported status describes an individual quantity, not complete input
admission. Missing status is bounded to the captured product/cycle, not all IFS
or licensed ECMWF data. P0 includes FLEXPART-derived diagnostics, which must not
be confused with missing independent provider fields.

[Original-source provenance](../../fixtures/meteorology/ifs-open-audit-v1/original-source.json)
binds the clean FLEXPART 11.1 checkout at `c70586c2b7f5258850705325881c61f557ea9bd8`.
Inspected `windfields_mod.f90:819–846,2105–2175` requires native hybrid
coefficients and field/level identities; `verttransform_mod.f90:1976–2044`
reconstructs pressure, height and W geometry. These are source-contract traces,
not a new original-routine execution claim.

## Authority and observed availability

Official sources read on 1 November 2026:

- [ECMWF Open Data](https://www.ecmwf.int/en/forecasts/datasets/open-data):
  0.1° introduced 20 October 2026; same approximately 9 km forecast exported
  onto regular lat/lon grids; 0.1° approximately two hours later than 0.25°;
  rolling retention of 12 runs, approximately 2–3 days. Cloud mirrors are listed.
- [IFS/AIFS access overview](https://confluence.ecmwf.int/spaces/DAC/pages/272310539/ECMWF+open+data+real-time+forecasts+from+IFS+and+AIFS):
  `oper/fc` deterministic IFS; three-hour forecast steps through +48 hours.
- [New 0.1° tier](https://confluence.ecmwf.int/spaces/DAC/pages/738041087/ECMWF+open+data+what+s+new+at+0.1%C2%B0):
  mentions additional multi/model-level initialization fields. This wording
  alone is insufficient evidence of native hybrid geometry.
- [File naming/formats](https://confluence.ecmwf.int/spaces/DAC/pages/738041084/ECMWF+open+data+file+naming+and+formats):
  0.1° `-sfc`/`-pl`, 0.25° combined, GRIB2; recommends ecCodes >=2.42.0.
- [Packing changes](https://confluence.ecmwf.int/spaces/DAC/pages/738041093/ECMWF+open+data+bits+per+value):
  per-parameter precision changed on 20 October; its CP categorization as
  AIFS-only does not match the captured deterministic IFS inventory.
- [Official client](https://github.com/ecmwf/ecmwf-opendata): index/range
  extraction and explicit model, resolution, date, stream and step selection.
- [Licence/terms](https://apps.ecmwf.int/datasets/licences/general/): CC BY 4.0
  redistribution with attribution; [fixture attribution](../../fixtures/meteorology/ifs-open-audit-v1/ATTRIBUTION.md).

Response hashes, URLs, timestamps and headers are recorded in
[source-authority.json](../../fixtures/meteorology/ifs-open-audit-v1/source-authority.json).
Documentation response bodies are not required to reproduce the byte audit.
The captured official product-directory listing contains `sfc` and `pl` files,
with no separate `ml` file. All 51 selected official indexes returned HTTP 200:
17 steps each for 0.1° sfc/pl and 0.25° combined. Every selected range returned
206 with the requested range and length. A separate AWS mirror index request
returned HTTP 503; mirror availability is **not verified** by this audit.

Initialization is **2026-11-01T00:00:00Z**. Captured lead times are 0, 3, 6,
24 and 48 hours for precipitation; upper-air T also has +0/+3. Valid times are
reference time plus lead, including 2026-11-03T00:00:00Z at +48.
No AIFS, ENS, ERA5 or licensed full-product bytes substitute for this cycle.
The index Last-Modified for +0 0.1° sfc is 09:34 UTC: an observed server header,
not a measured delivery SLA. Retention/latency claims above are documented,
not established by a multi-cycle operational monitor.

## Preserved evidence and decoding

The [manifest](../../fixtures/meteorology/ifs-open-audit-v1/retrieval-manifest.json)
binds exact URL, selector/index row, offset/length, response headers, retrieval
time, byte count and SHA-256. All **34 unmodified messages (53,026,786 bytes)**
and **51 indexes (1,416,904 bytes)** are committed. Individual message ranges
are global fields; an index selects messages, not geographical tiles. No full
global forecast archive was downloaded. The fetcher caps individual messages
at 20 MB, total message requests at 160 MB, and each index at 2 MB; it rejects
an ignored Range response. Source bytes remain available when live URLs expire.

| Decoder | Evidence and conclusion |
| --- | --- |
| ecCodes 2.48.0, Windows | All metadata and source quantities decoded; [full report](../../fixtures/meteorology/ifs-open-audit-v1/decoded-2.48.0.json) |
| Existing `flexpart-fortran:latest`, ecCodes 2.24.2 | Numerically decodes the same bytes; CP, EWSS, NSSS, SF and TCC are unrecognized parameter labels. This is not semantic compatibility; [diagnostic report](../../fixtures/meteorology/ifs-open-audit-v1/decoded-2.24.2.json) |
| Independent WMO octet reader + direct libaec | All values compared against ecCodes, not just a few cells; zero maximum difference. No ecCodes parameter definitions are used by this independent unpacking path |

[Decoder environment](../../fixtures/meteorology/ifs-open-audit-v1/decoder-environment.json)
records the existing container image ID and local decoder/library identities.
The old image is `sha256:324bb63d7311b13d9700906f05c98babaa69967c1dbfed124abb135855f80d84`.
The legacy command is explicitly numeric diagnostic mode and retains unknown
labels. It borrows modern labels only for a bounded validation step after
matching raw hashes; it does not reclassify the old environment as supported.
Raw discipline/category/number, fixed-surface codes and statistical templates
are recorded independently. This confirms the official minimum-version advice
matters even when the codec itself works. The oracle container is unchanged.

Messages use GRIB edition 2, centre 98 (`ecmf`), `class=od`, `stream=oper`,
`type=fc`, regular latitude/longitude template 3.0 and CCSDS template 5.42.
0.1° has 3600 × 1801 points, first point 90° N, 180° E; last point 90° S,
179.9° E. Scan mode 0 is east-fastest and north-to-south, wrapping longitude;
`uvRelativeToGrid=0`. 0.25° has 1440 × 721 points, last longitude 179.75° E.
These are cell-centered export quantities, not native staggered hybrid fields.
All preserved messages have no bitmap and zero missing values. That does not
prove pressure levels below terrain are meaningful atmospheric observations.
Any missing/corrupt or mismatched required input must fail closed.

Observed precision, including the post-change parameter-specific bit counts:

| Example | bits | quantum in decoded units |
| --- | ---: | ---: |
| 2T +0 | 9 | 0.25 K |
| T 850 hPa +0 | 9 | 0.125 K |
| Q 850 hPa +0 | 9 | 0.00006103515625 kg/kg |
| U/V 850 hPa +0 | 11 | 0.0625 m/s |
| Omega 850 hPa +0 | 12 | 0.0078125 Pa/s |
| Surface pressure +0 | 12 | 16 Pa |
| TP +3 / +6 | 12 | 0.061035 / 0.122070 mm |
| CP +3 / +6 | 12 | 0.030518 / 0.061035 mm |
| EWSS +3 | 12 | 128 N s/m² |
| SSR +3 | 10 | 16384 J/m² |

Per-message reference values, binary/decimal scales and all packing keys are
in the reports. Quantum is `2^E / 10^D`; constant +0 precipitation has zero
bits. Reported half-quantum is a nominal nearest-rounding scale, **not** an
error bound against unavailable unquantized model output or a science threshold.
Gradient accuracy and physics errors are not established by bit count alone.

## Principal gate: vertical coverage and terrain

Every captured upper-air index lists exactly **14 isobaric levels**:
1000, 925, 850, 700, 600, 500, 400, 300, 250, 200, 150, 100, 50, 10 hPa.
The ten upper-air parameters are d, gh, q, r, t, u, v, vo, w and z.
The captured 0.1° and 0.25° indexes expose the same parameter/level inventory
at corresponding steps; no additional atmospheric model-level product is
observed in this cycle despite the broader initialization wording.
All decoded representative upper-air messages are `isobaricInhPa`, with
`NV=0` and no vertical coefficient octets. The soil levels in the sfc file
are not atmospheric model levels. No genuine native hybrid-level product,
full native column, half-level A/B coefficients or native W interfaces are
observed. The complete inventory and directory evidence limit this conclusion
to this audited cycle/product, rather than proving no future product can exist.

`w` is pressure velocity, WMO (0,2,8), Pa/s, on pressure-level centers. It is
neither upward geometric m/s nor flex_extract-prepared native coordinate motion.
[Current #30](../vertical-transform.md) only admits independently validated
native interface omega conversion; center omega fails closed. Pressure-level
geopotential height does not manufacture those interfaces or A/B coefficients.

The 1000/925/850 hPa spacing leaves no terrain-following near-ground column.
High European terrain can lie above one or more lowest isobars; a finite
below-ground value is not evidence of a resolved surface layer. Even low
terrain has only sparse pressure-level sampling for PBL gradients. No native
137-level count is inferred from ECMWF/ERA5 ancestry, and no adequate complex
terrain PBL or #30 geometry is demonstrated. Adding a surface diagnostic wind
does not repair that column. This audit stops before inventing interpolation,
hydrostatic reconstruction or a new pressure-level production contract.

## Time, precipitation, flux and cloud boundaries

Indexes prove the full +0..+48 three-hour cadence. This gives 17 forcing
states for 192 quarter-hour output intervals; one weather interval spans 12
outputs. Smaller simulation/output steps cannot add unobserved meteorological
variability. PBL transients and rain pulses are unresolved between source times;
no quantitative wet-deposition or turbulence error estimate is claimed.

TP (228; 0/1/193) and CP (143; 0/1/195) are actual water-equivalent **metre
amounts**, with statistical processing 1, accumulation windows [0,+step] and
the forecast initialization as reset. +0 is a zero-length zero accumulation,
not a valid rate. SF is (0,1,198), metres water equivalent, [0,+3]. SD is a
snow-water-equivalent state, not snowfall or geometric snow depth. SF alone
does not supply a large-scale/convective snowfall split. The source indexes
contain no LSP parameter and no 3-D cloud liquid/ice fields; column TCLW/TCIW
cannot determine the vertical cloud profile or its bounds.

[Real value diagnostics](../../fixtures/meteorology/ifs-open-audit-v1/value-comparisons.json)
show TP +3→+6 decreases at 152,510 global points (minimum −0.061035 mm), CP
at 137,204 (−0.030518 mm). TP−CP at +3 is negative at 61,162 points (minimum
−0.030518 mm). These are diagnostics, not a proof of LSP identity or permission
to clip. Nonconsecutive +6→+24 and +24→+48 comparisons are explicitly labelled
longer windows, not misreported as three-hour forcing.

The [#216/#219 packing admission](../accumulation-contract.md#same-run-decreases-from-grib-packing-216)
is for its frozen GRIB1 class; it does not admit this GRIB2 product.
[#211](grib2-cp-units-211.md) covers one pinned flex_extract producer/reader
class and its `/1000`, not these raw amount messages. Shared ECMWF names do
not establish compatibility. [#202](precipitation-interval-mapping-202.md)
requires original point-member preparation; interval means cannot be passed
straight into #207/#119 as equivalent forcing. Its four-interval support for
an interior point spans T−6 to T+6 at three-hour cadence. Supporting endpoints
of a 0..48 simulation would need extra support or a separately validated
boundary decision; the captured +0..48 samples do not supply it.

EWSS/NSSS are run-accumulated momentum, SSR accumulated energy, not instantaneous
stress/radiation rates. Differencing and division by seconds would yield
interval means, while [#120](../temporal-interpolation.md#surface-flux-eligibility-120)
requires proven point semantics for the existing runtime sampler. No SSHF is
in the scoped indexes. No new flux, member or PBL metric is synthesized here.

## Operational estimates

[Feasibility arithmetic](../../fixtures/meteorology/ifs-open-audit-v1/feasibility.json)
uses an explicit example Europe box 25° W–45° E, 30–72° N, including endpoints:
701 × 421 = 295,121 grid points. 0.1° is approximately 11.12 km north–south
and 9.63/7.15/3.80 km east–west at 30/50/70° N. A 7 km zonal spacing at one
latitude does not establish a 5–8 km meteorological resolution. The documented
underlying forecast is about 9 km; no effective resolution was measured.

| This cycle, +0..+48 | Observed compressed bytes / calculated storage |
| --- | ---: |
| Complete 0.1° sfc + pl files, from indexes only | 7,916,224,337 bytes |
| Complete same-cycle 0.25° combined files, indexes only | 1,703,501,809 bytes |
| Selected diagnostic profile, 6 upper-air parameters × 14 + 9 surface quantities | 3,356,894,641 global download bytes |
| Eight quantity-compatible surface fields, 17 steps | 247,458,330 global download bytes |
| Regional profile, one f32 snapshot | 109,785,012 bytes (104.7 MiB) |
| Two regional snapshots / all 17 | 219,570,024 bytes / 1,866,345,204 bytes |
| Same illustrative profile, two full global f32 snapshots | 4,823,798,400 bytes |

The 93-equivalent-field profile is a storage illustration, not an admitted
consumer or a full P0 memory budget. Buffers for geometry, staging, particles,
scratch and output are additional; one full 0.1° field decoded as f64 alone is
51,868,800 bytes. Range selection avoids unrelated parameters, but cannot
crop the compressed global field geographically. Full P0 has no finite
source-complete download estimate because mandatory quantities are absent.
0.1° total bytes here are 4.65× 0.25°, rather than the grid-point ratio 6.25×;
compression/precision/field inventory matter.

At an ideal 100 Mbit/s, the 3.36 GB selected profile alone takes about 269 s;
this excludes latency, retries, indexes and decoding. These are arithmetic
estimates, not a benchmark or service guarantee. A production acquisition
design would use bounded concurrency, verify every range/checksum, archive
cycle/product identity, raw bytes, indexes and decoder identities, and fail
when required support is missing. The documented two-hour tier delay and
rolling archive are material. A +48 forecast lead is also not 48 hours from
wall-clock now once publication/cycle age is included: longer forecast support
and preprocessing padding would be required.

## Handoff and comparison with other providers

| Provider | Completeness / science / readiness under current contracts |
| --- | --- |
| IFS Open 0.1°, this audit | Pressure-only vertical coverage; missing cloud profile/SSHF/land classes, unproved precipitation/flux path; NO_GO for existing production consumers |
| [ERA5/ETEX #237](https://github.com/Grodahn/flexpart-gpu/issues/237) | Real 137-level/138-interface metadata exists for bounded U/V/T/q; surface join still needs proof. Preferred bounded first decoder; issue is open and does not prove P0 readiness |
| [ICON-EU #230](icon-eu-provider-230.md) | Richer native height fields but incompatible geometry/motion/precipitation/time semantics; NO_GO as first provider; #231–235 own its decisions |

Four finite prerequisite boundaries are recorded under #32 in the matrix:
IFS native geometry/motion source; this source's precipitation composition and
packing/member proof; missing sensible flux plus point-flux preparation; 3-D
cloud water and independent 13-class land-use input. These need separate
decisions/source evidence before decoder adoption. No existing issue's
scientific contract is expanded or closed here. #237 can share ecCodes,
canonical validation and provenance patterns, but this IFS product needs a
distinct versioned profile and adapter after those decisions. It must not
be treated as a product-name alias for ERA5 or for prepared FLEXPART files.

Licensed full IFS access is a separate conditional option: only a specified
agreement/catalogue and actual matched source inventory could establish
native geometry, fields, cadence and delivery. This audit obtained no licensed
data and makes no claim that open-data availability supplies that coverage or
that the approximately 5–8 km target is thereby met.

## Reproduction and checks

Offline commands (ecCodes 2.48.0 plus numpy required for Python; missing tools
fail, rather than skip):

```powershell
python -I scripts/provider/test_ifs_open_audit.py
cargo test --test ifs_open_provider_contract
python scripts/check_agent_navigation.py
python scripts/provider/ifs_open_audit.py fixtures/meteorology/ifs-open-audit-v1 --output target/ifs-modern.json --independent-values
python scripts/provider/ifs_open_comparisons.py fixtures/meteorology/ifs-open-audit-v1 --output target/ifs-values.json
```

The existing Linux oracle image can run the numeric legacy diagnostic with
the repository mounted at `/audit` and workdir `/audit`:

```text
python3 scripts/provider/ifs_open_audit.py fixtures/meteorology/ifs-open-audit-v1 --output target/ifs-legacy.json --independent-values --legacy-numeric-diagnostic
```

Use the recorded image ID, not merely a mutable tag. The independent path
reuses the bounded [#230 octet/libaec reader](../../scripts/provider/icon_eu_grib2_sections.py).
The tests check all hashes, complete inventory, raw metadata and decoder
evidence and regenerate the value diagnostics. Negative controls cover wrong
cycle, resolution, level, lead, units, packing/precision, statistical window,
missing values/fields/indexes, cross-run substitution and corrupt bytes.
Rust tests lock the matrix against `FIELD_SPECS` and the hash-pinned artifacts.
The standalone value-comparison command applies the same source hash, cycle,
product, grid and wind-frame checks before decoding values. Its report binds
the exact manifest and every consumed message hash, so replacing a file with
another cycle cannot produce a successful, apparently equivalent comparison.
This is research evidence, not a GPU or original-routine parity gate.

To inspect a **fresh explicitly chosen** coherent 00/12 cycle without overwriting
the immutable fixture, use a new directory (Windows `curl.exe` uses the system
certificate store):

```powershell
python scripts/provider/ifs_open_fetch.py --cycle YYYYMMDDHH --output target/ifs-fresh --messages
```

Inspect the fresh indexes first when the product contract changes. This command
has a fixed representative selection and byte ceilings, not an open-ended
operational downloader. It must not be interpreted as proof that a changed
product is compatible. Reproducing **this** decision uses the archived bytes,
not an expired cycle URL.
