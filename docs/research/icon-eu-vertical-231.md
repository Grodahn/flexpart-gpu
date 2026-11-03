# ICON-EU vertical geometry decision (#231)

Issue: [#231](https://github.com/Grodahn/flexpart-gpu/issues/231) (MET-INGEST-32-ICON-VERT),
parent [#32](https://github.com/Grodahn/flexpart-gpu/issues/32) via
[#230](icon-eu-provider-230.md), tracking #12.

This note records a research decision. It is **not** a schema change, a #30
implementation, a provider decoder, a GPU kernel or a FLEXPART 11.1 parity
result.

Evidence level: real-source decoding plus decoded-value consistency checks,
and a diagnostic FLEXPART height replay tied to pinned routine output. No
production-path, FLEXPART-oracle or GPU validation was performed or claimed.
No pinned FLEXPART oracle consumes ICON input.

## Outcome

| # | Question | Verdict | Decision |
| --- | --- | --- | --- |
| 1 | Authority for model-level heights | `SUPPORTED_WITH_PROVEN_TRANSFORMATION` | Provider `HHL` is authoritative. FLEXPART's hypsometric reconstruction is `UNSUPPORTED` for ICON |
| 2 | Full-level height convention | `SUPPORTED_WITH_PROVEN_TRANSFORMATION` | `z_full(k) = (HHL(k) + HHL(k+1)) / 2`; AGL = ASL − `HSURF` |
| 3 | W/interface coordinate and shared-height preparation | `SUPPORTED_WITH_PROVEN_TRANSFORMATION` | Interface heights are `HHL(h)` (AGL = `HHL(h)` − `HSURF`). The surface interface is exactly 0 m AGL. The `wzlev` mean-of-levels formula is not applied |
| 4 | Interface pressure | `UNSUPPORTED` | Surface = `PS`. Interior and top interface pressure is not admitted; #242 owns that decision |
| 5 | #29 schema | `REQUIRES_SCHEMA_EXTENSION` | A new coordinate kind is needed ([#241](https://github.com/Grodahn/flexpart-gpu/issues/241)); #30 derivation is [#243](https://github.com/Grodahn/flexpart-gpu/issues/243) |

The machine-readable decision is
[`decision.json`](../../fixtures/meteorology/icon-eu-vertical-231-v1/decision.json)
(`flexpart-gpu.icon-eu-vertical-geometry-decision` v1). It binds by SHA-256
the decoded evidence, the sample request and the retrieval manifest.

ICON-EU remains `NO_GO` as the first #32 provider (#230). This decision only
removes the *vertical-geometry* semantic unknown. Admission still needs #241,
#243 and #242, plus #232–#235 for the other fields.

## Normative references

| Quantity | Reference |
| --- | --- |
| Full-level height | ICON `release-2024.10-public`, [`src/atm_dyn_iconam/mo_vertical_grid.f90:170-174`](https://gitlab.dkrz.de/icon/icon-model/-/blob/release-2024.10-public/src/atm_dyn_iconam/mo_vertical_grid.f90#L170-174): `z_mc(jk) = 0.5*(z_ifc(jk) + z_ifc(jk+1))` ("geometric height of full levels"). File SHA-256 `7ecebe82…a5fbf7c59` |
| Constants of the consistency check | Same release, `src/shared/mo_physical_constants.f90:96,108,121,148`: `grav = 9.80665`, `rd = 287.04`, `rv = 461.51`, `vtmpc1 = rv/rd − 1`. SHA-256 `42a144f3…5a057c2` |
| Layer bounds | GRIB2 code table 4.5 type 150. Every decoded model-level message `k` has first surface (150, k) and second surface (150, k+1) |
| Terrain / interface identity | Provider data: `HHL` (second surface 101, metres above mean sea level) and `HSURF` |
| FLEXPART heights and `wzlev` | FLEXPART 11.1 `c70586c2`, `verttransform_mod.f90:1975-2051` (`verttransform_ecmwf_heights`); `:296-353` (`verttransform_init`) |
| FLEXPART interface-pressure consumer | `conv_mod.f90:606,626-631` (`phconv`, `dpr`) |

The ICON source is the public release, not the DWD operational build of
ICON-EU. The decoded hydrostatic check below is the independent confirmation
that the operational export follows the same convention.

## Sample and decoding

| Item | Value |
| --- | --- |
| Product | ICON-EU deterministic `icon-eu_europe_regular-lat-lon`, centre 78 |
| Cycle / valid time | 2026-11-03T00:00Z, step 0. This is a new pinned sample, because the #230 cycle had expired from DWD's ~24 h rolling archive |
| Files | 301: `HSURF`; `HHL` 1–75; `PS`, `T_2M`, `TD_2M`; `P`, `T`, `QV` on all 74 full levels |
| Volume | 249.7 MB bz2 / 257.5 MB GRIB2, downloaded 2026-11-03 14:33–14:34 UTC |
| Vertical grid | `nlev=75`, `numberOfVGridUsed=4`, UUID `f4e27f3af03da229299dca84d11c2e40` in every `HHL`/`P`/`T`/`QV` message. This is the same UUID as #230 |
| Decoders | Primary: ecCodes 2.24.2 in `flexpart-fortran:latest`. Secondary: ecCodes 2.48.0. Independent: octet reader plus libaec for every value of every message, with maximum absolute difference 0.0. Derived numbers agree between the decoders to 5.2e-13 relative |

Representative GRIB2 SHA-256 prefixes (full list in the
[retrieval manifest](../../fixtures/meteorology/icon-eu-vertical-231-v1/retrieval-manifest.json)):

| File (`…_2026110300_`) | SHA-256 prefix |
| --- | --- |
| `HSURF` | `b0297e49efa9c9db` |
| `1_HHL` / `75_HHL` | `33fa0aa42b28c59d` / `cf5190696e2e5e34` |
| `000_PS` | `a05239cfa87c7085` |
| `000_74_P` | `10979c97aa8c1f03` |

The [decoded evidence](../../fixtures/meteorology/icon-eu-vertical-231-v1/decoded-evidence.json)
contains:

- the identity keys and value hash of every message;
- full-field statistics over all 904 689 columns;
- seven complete representative columns (75 `HHL`, 74 × `P`/`T`/`QV`, plus
  surface values).

The columns are selected deterministically:

- the FLEXPART shared-height initialization column;
- maximum and minimum terrain;
- the steepest index-space terrain gradient;
- the worst lowest-level reconstruction difference;
- the worst hydrostatic deviation;
- the point nearest 50° N 10° E.

The raw bytes remain authoritative. They are not committed (747 KB per `HHL`
level), so reproducing the evidence later requires an archive of the bytes
named in the manifest.

## Findings

### Structure (all columns)

- `HHL` strictly decreases with level index in every column. The minimum layer
  is 17.2 m; the lowest layer is 19.6–21.3 m. Levels 1–13 are horizontally
  constant, and the top is 22 770.33 m.
- `HHL(75) == HSURF` **exactly** in all 904 689 columns, including terrain
  from −414 m to 3 801.5 m.
- `P` strictly increases with level index, and `P(74) < PS` everywhere
  (`PS − P(74)` is 82–133 Pa).

### Q1 — authority for model-level heights

ICON's coordinate is height-based. `HHL` is the time-invariant geometry on
which `U/V/T/QV/P` are defined: heights are the coordinate and pressure is
diagnosed from them. FLEXPART's `verttransform_ecmwf_heights` instead
*integrates* heights from `akz + bkz·ps`, starting at a T2m/Td2m surface
boundary. ICON has no A/B coefficients, so no pinned FLEXPART invocation
consumes ICON pressure. Substituting provider `P` would be an assumed
ICON/ECMWF equivalence, not a FLEXPART oracle.

A double-precision replay of that routine with provider `P` shows that the two
height sets are not interchangeable:

| Level | Replay − HHL full level (m), p01 / p50 / p99 / max \|·\| |
| --- | --- |
| 74 (lowest) | −0.11 / 0.007 / 0.09 / 0.36 |
| 60 | −2.8 / −0.42 / 2.0 / 30.5 |
| 37 | −6.2 / −2.2 / 2.4 / 46.8 |
| 1 (top) | −10.5 / −6.4 / −1.6 / 49.4 |

- The systematic upper-level offset is mostly the different constants. FLEXPART
  uses `R/g = 287.05/9.81`, ICON `287.04/9.80665`; the ratio 0.99969 gives
  about −7 m at 22 km.
- The remaining column-to-column spread, up to 49 m, is not attributed further
  here. It reflects differences in the surface boundary (T2m/Td2m), the
  layer-mean temperature and the hydrostatic assumption. Choosing either
  height set silently would hide that spread.
- The replay is validated against the **pinned compiled** routine output frozen
  by #118 (`fixtures/interpolation/shared-height-v1/output.txt`). All 32
  level/interface heights agree within the #30 height tolerance
  `max(0.02 m, 1e-5 rel)`; the Rust lock checks this.

**Decision:** provider `HHL` is authoritative.

### Q2 — full-level height convention

The convention comes from the ICON source (`z_mc`, the arithmetic mean). It is
tested on the decoded data with the hydrostatic thickness between adjacent full
levels. The test uses ICON constants and
`Tv = T(1 + (Rv/Rd − 1)·QV)` with a log-mean layer temperature, and covers all
73 pairs in all columns (66 million samples):

| Convention for full level k | ratio p01 / p50 / p99 | median \|ratio − 1\| | closest of the three |
| --- | --- | --- | --- |
| `(HHL(k) + HHL(k+1))/2` | 0.9989 / 1.0000 / 1.0012 | 3.9e-5 | 83.3 % |
| `HHL(k)` (upper half level) | 0.734 / 0.984 / 1.000 | 1.6e-2 | 8.5 % |
| `HHL(k+1)` (lower half level) | 1.000 / 1.016 / 1.567 | 1.6e-2 | 8.2 % |

- The mean convention is hydrostatic to about 0.1 % at p01/p99. Its median
  deviation is 400 times smaller than either alternative.
- Its extreme samples (ratio 0.895 and 1.280) are rare outliers beyond p01/p99.
  The column with the largest deviation is retained: it lies in the Caucasus
  at 43.0° N 43.5° E (3 377 m). This audit does not attribute the outliers.
  Candidate causes are non-hydrostatic flow, the omitted condensate loading
  and the regridding.
- The regular-lat-lon export interpolates every field from the native grid. The
  mean relation survives that only if the remapping is linear and identical for
  all fields. The hydrostatic result supports this; it does not prove it.

**Decision:** `z_full(k) = (HHL(k) + HHL(k+1))/2`. Full level k is the layer
bounded by half levels k and k+1.

### Q3 — W/interface coordinate and shared-height preparation

| Quantity | Result |
| --- | --- |
| Surface interface `HHL(75) − HSURF` | exactly 0 m everywhere. This matches FLEXPART `wzlev(1) = 0` (`verttransform_mod.f90:2001-2002`) |
| FLEXPART-style interior interface `(z_full(k) + z_full(k+1))/2` − `HHL(k+1)` | −5.3 to 11.4 m (p50 2.5 m). It equals exactly ¼ of the `HHL` second difference (verified to 0.0 m) |
| FLEXPART-style top `wzlev(nwz)` − `HHL(1)` | −3.17 m (flat top) |
| `wzlev` from the FLEXPART replay − `HHL` interior | −48.8 to 40.7 m |

- FLEXPART's `wzlev` (`:2025-2029`) is a *construction*: the mean of adjacent
  level heights, with an extrapolated top. It approximates interfaces that the
  ECMWF path never knows geometrically.
- ICON supplies the interfaces exactly, and ICON `W` lives on them. The
  canonical #30 role "W/interface height" therefore takes `HHL`. Applying the
  `wzlev` formula would displace interfaces by metres for no reference benefit.
- The #184/#188/#190 preparation consumes, per column, the #30 AGL level and
  interface heights. It also uses a run-global AGL target grid taken from the
  first column with `ps > 100000 Pa` (`verttransform_mod.f90:309-319`). For
  ICON both inputs follow from the HHL-derived heights.
- In this sample the selected initialization column is index 0 (29.5° N,
  23.5° W). There, HHL and the replay differ by at most 6.1 m.
- W semantics, staggered U/V at half levels and the slope correction are owned
  by **#234**.

### Q4 — interface pressure

Interface pressure is required:

- #30 `VerticalTransformResult` carries it for every interface;
- FLEXPART convection consumes it (`conv_mod.f90:626-631`, `phconv` and `dpr`).

ICON-EU publishes neither interface pressure nor A/B coefficients. The surface
interface is `PS`. For the interior half levels, three candidates evaluated at
`HHL` were compared:

| Candidate | Difference from ln P linear in height (Pa), p01 / p50 / p99 / max \|·\| |
| --- | --- |
| Arithmetic mean of adjacent `P` (FLEXPART non-ECMWF branch, `conv_mod.f90:629`) | −68.8 / 1.9 / 11.1 / 80.6 |
| Geometric mean of adjacent `P` | −69.0 / −4.9 / 0.07 / 81.6 |

Extrapolating ln P linearly in height from levels 73/74 to `HHL(75)`
reproduces `PS` within −5.2 to +3.2 Pa. A hydrostatic extrapolation from level
74 gives −4.4 to +3.7 Pa. Both show consistency only; neither is an ICON
definition of half-level pressure.

None of the candidates has an ICON reference, and FLEXPART's non-ECMWF branch
belongs to the GFS path. **Decision:** `UNSUPPORTED` for the interior and top
interfaces, so consumers that need them fail closed. #242 owns the decision.

### Q5 — schema

- #29 v1 `VerticalCoordinateKind::GeometricHeight` holds one 1-D profile, but
  `HHL` varies per column below level 13.
- #30 `reconstruct_hybrid_pressure` rejects every non-hybrid coordinate.

**Decision:** `REQUIRES_SCHEMA_EXTENSION`. #241 defines a coordinate kind that
carries the 3-D half-level heights, the full-level pressure and the
vertical-grid identity. #243 derives the runtime geometry with exactly the
transformations above. No schema change is made here.

## Canonical runtime quantities

| Runtime quantity (#30) | ICON source | Classification |
| --- | --- | --- |
| Terrain ASL | `HSURF` (= `HHL(75)`) | `SUPPORTED_AS_IS` |
| Model-level height ASL / AGL | `(HHL(k)+HHL(k+1))/2` / − `HSURF` | `SUPPORTED_WITH_PROVEN_TRANSFORMATION` |
| Interface height ASL / AGL | `HHL(h)` / `HHL(h)` − `HSURF` | `SUPPORTED_AS_IS` / `SUPPORTED_WITH_PROVEN_TRANSFORMATION` |
| Model-level pressure | `P` | `SUPPORTED_AS_IS` |
| Surface interface pressure | `PS` | `SUPPORTED_AS_IS` |
| Interior/top interface pressure | — | `UNSUPPORTED` (#242) |

All 74/75 levels are retained.

## Follow-ups

| Issue | Kind | Claim |
| --- | --- | --- |
| [#241](https://github.com/Grodahn/flexpart-gpu/issues/241) | contract | #29 schema-v2 coordinate kind for column-varying half-level heights |
| [#243](https://github.com/Grodahn/flexpart-gpu/issues/243) | implementation | #30 runtime height geometry from that kind; interface pressure fails closed |
| [#242](https://github.com/Grodahn/flexpart-gpu/issues/242) | research/decision | ICON interior/top interface pressure |
| [#234](https://github.com/Grodahn/flexpart-gpu/issues/234) | research/decision (existing) | FLEXPART-compatible use of ICON `W` |

## Reproduction

Commands run from the repository root. Downloads go to a new empty directory
outside the repository. Use `python -I`, because the downloads are untrusted
data.

```bash
python -I scripts/provider/icon_eu_fetch.py --request fixtures/meteorology/icon-eu-vertical-231-v1/sample-request.json --output <new-dir> --expect fixtures/meteorology/icon-eu-vertical-231-v1/retrieval-manifest.json
```

```bash
docker run --rm -v <new-dir>:/data:ro -v "$PWD/scripts/provider:/tools:ro" -v "$PWD/fixtures/meteorology/icon-eu-vertical-231-v1:/request:ro" -v <out>:/out --entrypoint sh flexpart-fortran:latest -c 'cd /tmp && python3 -I /tools/icon_eu_vertical_231.py inspect --sample /data --request /request/sample-request.json --label container-eccodes-2.24.2 --output /out/primary.json'
```

```bash
python -I scripts/provider/icon_eu_vertical_231.py inspect --sample <new-dir> --request fixtures/meteorology/icon-eu-vertical-231-v1/sample-request.json --label eccodes-2.48.0 --output <out>/secondary.json
```

```bash
python -I scripts/provider/icon_eu_vertical_231.py summarize --primary <out>/primary.json --secondary <out>/secondary.json --output fixtures/meteorology/icon-eu-vertical-231-v1/decoded-evidence.json
```

- **Fetch:** after the DWD retention window the fetch fails closed.
  Reproduction then needs a byte-identical archive passed with `--base-url`.
  `--expect` rejects any byte difference.
- **Inspect:** each run fails closed when any of these holds:
  - the file set differs from the request;
  - a hash differs from the manifest;
  - ecCodes and the octet reader disagree on metadata;
  - libaec values differ;
  - grid, cycle, time or vertical-grid identity is not unique;
  - a layer-bound surface is wrong.
- **Summarize:** refuses the result when any of these holds:
  - message hashes differ;
  - the selected columns' raw values differ;
  - the primary run lacks the independent decode;
  - a derived number differs by more than 1e-12 relative.
- **Offline checks:**
  - `python -I scripts/provider/test_icon_eu_vertical_231.py` runs the
    summarizer refusals, request expansion and analytical geometry cases;
  - `cargo test --test icon_eu_vertical_geometry` pins the decision digest and
    the evidence/request/manifest hashes. It checks one verdict and a
    normative reference per question, and the FLEXPART and ICON pins. It
    re-derives every retained column offline: order, exact terrain identity,
    full-level heights, hydrostatic ratios, the `wzlev` identity and the
    FLEXPART replay. It also ties the replay to the pinned routine output.

Isolation: `scripts/provider/` is research tooling. It builds no canonical
snapshot and is not imported by production code.
