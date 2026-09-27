# FLEXPART 11.1 Interpolation Oracle Contract

Status: frozen reference for issues #72 (horizontal), #73 (vertical), #74 (temporal),
and #75 (accumulated-field interval/rate normalization).

For #73, model-level meter-coordinate sampling is frozen as direct FLEXPART oracle evidence. The `vertical-interface-wzlev` case freezes the #30 W/interface **handoff geometry/staggering and primitive interpolation behavior only**. Issue #80 now separately freezes pristine FLEXPART's complete `eta=no` W production path and demonstrates that direct interface sampling is not equivalent for the supported nonlinear case. The #73 W/interface implementation remains blocked until the #80 evidence is reviewed and merged.

This document freezes the *normative* interpolation behavior that the downstream
interpolation implementation issues must reproduce or explicitly diverge from.
It is the machine- and human-readable contract accompanying
`scripts/interpolation/direct_interpolation_oracle.f90` and the fixtures in
`fixtures/interpolation/`.

## 1. Normative reference

| Aspect | Frozen value |
| --- | --- |
| Software | FLEXPART 11.1 |
| Pinned revision | `c70586c2b7f5258850705325881c61f557ea9bd8` (tag `v11.1`) |
| Reference manifest | `reference/flexpart-11.1.json` |
| Build | `-cpp -mcmodel=large -UETA` (METRE mode, `eta=no`), gfortran, x86-64 |
| Oracle driver | `scripts/interpolation/direct_interpolation_oracle.f90` |
| Oracle harness | `scripts/interpolation/direct_oracle.sh` (container build + run) |
| Fixture pack | `fixtures/interpolation/` (+ per-case golden outputs) |

The oracle links the pristine `src/com_mod.o`, `src/par_mod.o`, `src/point_mod.o`,
`src/windfields_mod.o`, `src/interpol_mod.o` and calls the real pinned routines directly. The driver's output
is the only oracle evidence; no reimplementation is used as evidence.

Every machine-readable case carries a `semantics` object with explicit coordinate
conventions, horizontal/vertical staggering, storage/vertical ordering, units and time
semantics. The real ERA5/ETEX sampling case carries the same categories plus field-specific
units/staggering. These fields are part of the reproduced contract, not documentation-only
annotations.

The provenance sibling pins the final `contract-v1.json` SHA-256 and the exact source
hashes for the fixture generator, direct-oracle harness, real-column extractor, oracle
driver and FLEXPART reference manifest. It also records the actual gfortran version,
the pinned full-FLEXPART object-build profile/container, the driver compile/link flags,
and the complete sorted set of linked FLEXPART object files. CI requires those metadata
fields to reproduce exactly. The local oracle executable path/SHA is deliberately **not**
part of the frozen provenance: source/object/compiler metadata plus direct per-case output
hashes are the reproducible evidence, while an executable hash may legitimately vary with
local link/build details.

## 2. Horizontal grid conventions

FLEXPART reads grids with cell-center samples. The canonical convention frozen here is
cell-center anchored.

### Production coordinate path versus oracle exercise path

`point_mod::coordtrafo` is a real pinned FLEXPART routine, but it is **not** called
immediately before every meteorology interpolation. Its production role is release-point
initialization:

```
FLEXPART::read_options_and_initialise_flexpart
  -> point_mod::coordtrafo
```

That routine converts configured geographic release coordinates to FLEXPART grid
coordinates:

```
xt = (lon_deg - xlon0_deg) / dx_deg_x
yt = (lat_deg - ylat0_deg) / dy_deg_y
```

Runtime particle meteorology sampling already operates in FLEXPART grid coordinates.
The pinned source is **not one linear stack**. The relevant direct `CALL` edges for
the above-PBL meter-coordinate path are:

```
advance_mod::advance -> interpol_mod::init_interpol
  init_interpol -> find_ngrid
  init_interpol -> find_grid_indices
  init_interpol -> find_grid_distances
  init_interpol -> find_time_vars
  init_interpol -> find_z_level

advance_mod::advance -> advance_mod::adv_above_pbl
  adv_above_pbl -> interpol_mod::interpol_wind
    interpol_wind -> find_ngrid
    interpol_wind -> find_grid_indices
    interpol_wind -> find_grid_distances
    interpol_wind -> find_time_vars
    interpol_wind -> find_z_level_meters
    interpol_wind -> interpol_wind_meter
      interpol_wind_meter -> find_vert_vars
      interpol_wind_meter -> hor_interpol
      interpol_wind_meter -> vert_interpol
      interpol_wind_meter -> temporal_interpolation
```

Here `hor_interpol` is the generic interface; the 4-D wind-field calls resolve to
`hor_interpol_4d`. Importantly, `init_interpol` and
`adv_above_pbl -> interpol_wind` are sibling branches from `advance`; the latter
recomputes grid/time interpolation state. The machine-readable contract therefore
stores direct call edges rather than implying a synthetic sequence.

The `horizontal-geographic-interior` direct oracle intentionally composes
`coordtrafo -> find_grid_indices -> find_grid_distances -> hor_interpol_4d` so the
geographic-to-grid mapping is exercised by pristine code before the interpolation
primitive. That composition is therefore recorded as `oracle_exercise_path`, **not**
as a pristine production call chain. The machine-readable fixture separately records
`production_direct_call_edges` plus the generic-interface resolution.

FLEXPART computes the zonal step from the stored first/last longitudes
(`gridcheck_ecmwf`, `windfields_mod.f90:572-718`):

```
dx = (xaux2 - xaux1) / (nxfield - 1)
```

For exactly-global grids `nx = nxfield + 1`: the `nxfield+1`-th column is the wrapped
duplicate of column 0, and `nxmax = nxfield + 1`. Non-global grids have `nx = nxfield`,
`nxmax = nxfield`.

The pinned default `nxshift = 359` (`FLEXPART.f90:259`) rotates the ECMWF grid origin to
`-1°`. This is a **grid-loading** concern that does not affect interpolation shape. The
oracle fixture convention is `nxshift = 0` equivalent (`xlon0_deg` = first stored cell
center) so that `X_can = (lon_deg - xlon0_deg)/dx_deg_x` is the canonical mapping.
`nxshift` normalization is deferred to ingestion (#32).

### Sampling

`find_grid_indices` (`interpol_mod.f90:128-166`, mother grid path `147-153`):

```
ix   = int(xt)
jy   = int(yt)
ixp  = ix + 1    ; if (ixp >= nxmax) ixp = ixp - nxmax   ["wraparound", :161-164]
jyp  = jy + 1    ; if (jyp >= nymax) jyp = jyp - 1       ["temporary fix", :156-159]
```

- The east wrap sets `ixp` to `ix + 1 - nxmax`, which maps the wrapped duplicate column
  back to column 0 for global grids (and is applied with a warning for regional grids).
- The north clamp keeps `jyp` inside `[0, nymax-1]`; there is **no** south clamp and no
  west wrap. A particle with `xt < 0` or `yt < 0` is out of contract and must be
  rejected or flagged by the caller.

`find_grid_distances` (`interpol_mod.f90:168-188`):

```
ddx  = xt - real(ix)
ddy  = yt - real(jy)
rddx = 1 - ddx ; rddy = 1 - ddy
p1 = rddx*rddy      ! (ix  , jy )
p2 = ddx *rddy      ! (ixp , jy )
p3 = rddx*ddy       ! (ix  , jyp)
p4 = ddx *ddy       ! (ixp , jyp)
```

`hor_interpol_4d` / `hor_interpol_2d` (`interpol_mod.f90:481-504`):

```
output = p1*f(ix ,jy ) + p2*f(ixp,jy ) + p3*f(ix ,jyp) + p4*f(ixp,jyp)
```

### Verified golden (interior)

Grid `nx=4, ny=3, dx=dy=1, xlon0=ylat0=0`, `f(x,y) = 100 + 10*y + 100*x`,
query `(xt,yt) = (1.25, 0.5)`, `kz=1`:

```
ix, jy, ixp, jyp = 1, 0, 2, 1
p1..p4           = 0.375, 0.125, 0.375, 0.125
value            = 230.0
```

The same non-periodic fixture also samples the valid north-edge query
`(xt,yt)=(2.9,2.0)`. It remains inside the canonical domain
`0 <= xt <= nx-1`, `0 <= yt <= ny-1`; `jyp` is clamped at the exact last
row while the east-neighbor index stays inside the regional grid. #71 therefore
does **not** freeze regional east-wrap behavior outside the supported domain.

For the periodic fixture, `0 <= xt < nx` covers the seam up to (but excluding)
the duplicate endpoint at `xt=nx`; Y remains `0 <= yt <= ny-1`. Its north-edge
case is now exactly `yt=2.0`, not an overshoot. Any horizontal query outside
these domains is deliberately not part of the #71 candidate contract and must
fail closed in #72 rather than inheriting raw primitive wrap/clamp behavior.

### Geographic-to-grid oracle case

The `horizontal-geographic-interior` case uses a non-zero geographic origin
and non-unit spacing: `xlon0=-2°`, `ylat0=48°`, `dx=dy=0.25°`.
Its inputs are **Lon/Lat**, not precomputed grid coordinates:

```
lon=-1.6875°, lat=48.125° -> point_mod::coordtrafo -> xt=1.25, yt=0.5
lon=-1.3750°, lat=48.375° -> point_mod::coordtrafo -> xt=2.50, yt=1.5
```

Those transformed coordinates are then sampled by the pinned horizontal
interpolation routines. The first query intentionally lands on the same
`xt/yt` as the index-space interior fixture, providing an end-to-end check
that geographic and direct-grid paths converge before interpolation.

### Verified golden (global seam via duplicate column)

Global index fixture with `periodic=1`, canonical `nx=4`, `dx=90°` (`nx*dx=360°`) and FLEXPART `nxmax=5`, query
`(xt,yt) = (3.2, 1.5)`, `kz=1`. This samples across the periodic seam via
FLEXPART's duplicated ghost column. For this valid canonical-domain query
`ixp=4 < nxmax`, so the explicit `ixp >= nxmax` correction branch is **not**
executed. #71 therefore freezes the global duplicate-column seam behavior, not
that explicit correction branch. Exercising the branch requires a query on/beyond
the duplicate endpoint and is outside the supported canonical coordinate domain:

```
ix, jy, ixp, jyp = 3, 1, 4, 2
value            = 355.0        (ixp=4 is the duplicate ghost column copied from column 0)
```

## 3. Vertical conventions (METRE mode)

METRE mode (`eta=no`, `-UETA`) uses the single shared metric height array `height`
(`windfields_mod.f90:210-211`; `find_z_level_meters`, `interpol_mod.f90:215-242`):

```
indz  = nz-1 ; indzp = nz
zt <= height(1)                      -> indz=1, indzp=2, lbounds(1)=T
zt >= height(nz)                     -> indz=nz-1, indzp=nz, lbounds(2)=T
first i with height(i) > zt          -> indz=i-1, indzp=i
```

`find_vert_vars` (`interpol_mod.f90:315-404`) delegates to the **linear** path
`find_vert_vars_lin` (`interpol_mod.f90:406-430`) because `log_interpol=.false.` in the
pinned build:

```
bounds(1)=T  -> dz1=0, dz2=1
bounds(2)=T  -> dz1=1, dz2=0
otherwise    -> dz1=(zpos - vl(zl))/(vl(zl+1)-vl(zl))
                dz2=(vl(zl+1) - zpos)/(vl(zl+1)-vl(zl))
```

`vert_interpol` (`interpol_mod.f90:539-547`) is linear:

```
output = input1*dz2 + input2*dz1
```

`(input1, dz1)` belong to the lower (closer-to-ground) level.

### Verified golden

`height = [10, 100, 1000]`, values `[1, 2, 3]`:

| zt | indz/indzp | dz1, dz2 | value |
| --- | --- | --- | --- |
| 5 | 1/2 | 0, 1 | 1.0 |
| 50 | 1/2 | 0.44444448, 0.55555558 | 1.44444442 |
| 100 | 2/3 | 0, 1 | 2.0 |
| 500 | 2/3 | 0.44444448, 0.55555558 | 2.44444466 |
| 1000 | 2/3 | 1, 0 | 3.0 |
| 2000 | 2/3 | 1, 0 | 3.0 |

## 4. Temporal conventions

`find_time_vars` (`interpol_mod.f90:190-198`) derives weights from the two
in-memory wind-field times:

```
dt1 = itime - memtime(1)
dt2 = memtime(2) - itime
dtt = 1 / (dt1 + dt2)
```

`temporal_interpolation` (`interpol_mod.f90:531-537`) then applies them:

```
output = (time1*dt2 + time2*dt1) * dtt
```

There is **no range guard in either primitive**. For a non-zero memory span,
`dt1 + dt2 = memtime(2) - memtime(1)` remains constant, so calling these
routines with `itime` outside the two memory times performs linear
**extrapolation** rather than becoming mathematically undefined.

### Production caller path and range ownership

The pinned FLEXPART lifecycle separates wind-memory management from particle
sampling. These are direct call edges, not a single stack:

```
timemanager_mod::timemanager -> getfields_mod::getfields
timemanager_mod::timemanager -> advance_mod::advance

advance_mod::advance -> interpol_mod::init_interpol
init_interpol -> find_time_vars

advance_mod::advance -> advance_mod::adv_above_pbl
adv_above_pbl -> interpol_mod::interpol_wind
interpol_wind -> find_time_vars
interpol_wind -> interpol_wind_meter
interpol_wind_meter -> temporal_interpolation

advance_mod::advance -> advance_mod::petterssen_corr
petterssen_corr -> interpol_mod::interpol_wind_short
interpol_wind_short -> find_time_vars
interpol_wind_short -> interpol_wind_meter
```

Thus `getfields` and `advance` are sibling calls from `timemanager`.
`temporal_interpolation` is reached inside the concrete interpolation consumers;
it is not directly downstream of `init_interpol`.

The Petterssen correction is a concrete caller-side example of this separation:
`advance_mod::advance` checks that the predicted step end remains within the
loaded wind-field time before calling `petterssen_corr`. That guard is **not**
part of `find_time_vars` or `temporal_interpolation`, and must not be inferred
as primitive behavior.

Therefore the #71 oracle contract is:

- inside the selected two-member window, downstream #74 values must match the
  direct FLEXPART routines;
- the direct FLEXPART temporal primitives themselves extrapolate when invoked
  outside that window;
- if #74 chooses to reject requests outside its canonical snapshot window, that
  is an explicit fail-closed candidate/API policy and a documented divergence
  from the raw primitive outside its normal caller-managed domain, not FLEXPART
  oracle behavior.

### Verified golden

`memtime = [0, 3600]`, `time1=10, time2=20`:

| itime | dt1, dt2 | output |
| --- | --- | --- |
| 0 | 0, 3600 | 10.0 |
| 1800 | 1800, 1800 | 15.0 |
| 3600 | 3600, 0 | 20.0 |

## 5. 2-D layer / accumulated-field sampling (`interpol_rain`)

`interpol_rain` (`interpol_mod.f90:1209-1582`) samples seven surface/layer quantities
with the same horizontal bilinear seed `(ix,jy,ixp,jyp,p1..p4)` and the same two
in-memory members:

- `lsprec` (large-scale precipitation) and `convprec` (convective) use the
  `mp/ip/dtp1/dtp2` pair selection;
- `tcc`, `ctwc`, `tt`, and the cloud-bottom/top `ip`-masked averages use `dt1/dt2/dt`.

### Production ingest and sampling crosswalk

The precipitation fields used by `interpol_rain` enter the two FLEXPART wind-memory
slots through these direct calls:

```
timemanager_mod::timemanager -> getfields_mod::getfields
getfields_mod::getfields -> windfields_mod::readwind_ecmwf   # ECMWF
getfields_mod::getfields -> windfields_mod::readwind_gfs     # GFS
```

Those readers populate the data fields `windfields_mod::lsprec` and
`windfields_mod::convprec`; field names are not represented as call-graph nodes.

The production wet-deposition consumer uses these direct calls:

```
timemanager_mod::timemanager -> wetdepo_mod::wetdepo
wetdepo_mod::wetdepo -> wetdepo_mod::get_wetscav
get_wetscav -> interpol_mod::find_ngrid
get_wetscav -> interpol_mod::find_grid_indices
get_wetscav -> interpol_mod::find_grid_distances
get_wetscav -> interpol_mod::find_z_level_meters   # eta=no build
get_wetscav -> interpol_mod::interpol_rain
```

`get_wetscav` also nudges east/north border positions slightly into the mother grid
before calling the grid-index routines. That caller-side boundary handling is distinct
from the raw primitive behavior frozen by the synthetic horizontal cases.

Input units (pinned build): precipitation fields are stored as **mm/h rates**; ECMWF
data is read without in-core accumulation deconvolution (accumulation -> interval/rate
normalization is upstream, e.g. preprocessing such as flex_extract, and candidate
normalization is owned by #75). GFS precipitation is converted from mm/s to mm/h by
multiplication by 3600 in `readwind_gfs` (`windfields_mod.f90`). FLEXPART performs
**no** accumulation reset/deaccumulation at the `interpol_rain` sampling boundary.

### Frozen quirk: unconditional `dtt = dt/3` (`interpol_mod.f90:1302-1316, 1549-1550`)

```
dt  = memtime(2) - memtime(1)
dtt = dt/3
numpf == 1  -> mp=[1,2], ip=[1,1], dtp1=dt1, dtp2=dt2   (default build, par_mod.f90:190)
yint1 = (y1(1)*dtp2 + y1(2)*dtp1) / dtt        ! lsp
yint2 = (y2(1)*dtp2 + y2(2)*dtp1) / dtt        ! cp
```

For `numpf=1` the denominator is `dt/3` while `dtp1+dtp2 = dt`, so the interpolated
precipitation is **3 × the linear blend**. This is the actual pinned sampling behavior
and is frozen here. It sits **downstream** of #75's accumulation-to-interval/rate
normalization boundary: #75 must preserve the normalized quantity, units, interval and
provenance needed by the composed sampler, but must not implement reset/deaccumulation
by emulating this interpolation quirk.

`tcc`, `ctwc`, `tt`, and cloud masking use the plain denominator `dt`:

```
yint3 = (y3(1)*dt2 + y3(2)*dt1) / dt           ! tcc            (:1552)
yint4 = (y4(1)*dt2 + y4(2)*dt1) / dt           ! ctwc (if lcw)  (:1553)
ytint = (ytt(1)*dt2 + ytt(2)*dt1) / dt         ! tt             (:1554)
intiy1/intiy2   = int(blend)                   ! cloud bot/top  (:1559-1579)
```

Cloud cloud-bottom/top fields are `icmv`-masked (`par_mod.f90:306`,
`icmv = -9999`). A grid point whose value equals `icmv` contributes weight 0 and the
masked sum is renormalized by `ipsum` (`interpol_mod.f90:1378-1444`); if `ipsum == 0`
the interpolated cloud value is `icmv` and both `intiy1/intiy2` collapse to `icmv`
(`:1568-1573`).

### Verified golden

`nx=4, ny=3, dx=dy=1`, `memtime=[0,3600]`, query `(xt,yt,itime,kz) = (1.25,0.5,1800,1)`
(see `rain-layer-fields` fixture for the full field listing):

```
lsprec t1: linear-in-x field 0..11 ; t2: 100..111
convprec t1 = 0.5 ; t2 = 1.5
tcc t1 = 0.2 ; t2 = 0.6 ; tt t1 = 280 ; t2 = 290
ctwc t1 = 0.001 ; t2 = 0.004  (lcw = .true.)

dt1 = 1800, dt2 = 1800, dt = 3600, dtt (post) = 1200
yint1 (lsp)  = 159.75      ( = 3/2 * (3.25 + 103.25) )
yint2 (cp)   = 3.0
yint3 (tcc)  = 0.4
ytint (tt)   = 285.0
yint4 (ctwc) = 0.0025
intiy1/intiy2 = -9999, -9999   (all four corners icmv -> CLOUD -9999 -9999)
```

## 6. Real #29/#30 ERA5/ETEX oracle-sampled column

The contract now contains both the real source descriptor
`era5-etex-real-column-v1` **and an actual direct-oracle sampling case**,
`real-era5-etex-temperature-column`.

The source is the checked-in #29 canonical fixture
`fixtures/meteorology/era5-etex-native-v1.json`, selecting canonical
`x=0, y=0` at `-2° / 48°` and `1994-10-23T15:00:00Z`. Its 137 real ERA5
temperature values are paired with the 137 model-level AGL heights produced for
that exact column by #30's pinned
`verttransform_mod::verttransform_ecmwf_heights` direct-routine oracle. The
native top-to-bottom/increasing-pressure profile and the oracle heights are
reversed together into the bottom-to-top metric ordering consumed by
`find_z_level_meters`.

#71 then feeds those **real values and real #30 geometry** through the pinned
FLEXPART 11.1 interpolation binary, calling
`find_z_level_meters -> find_vert_vars -> vert_interpol`. Five samples freeze
both level-array boundaries, one exact interior level and two strict interior
interpolations:

| AGL query | Direct FLEXPART temperature |
| ---: | ---: |
| 10.004482 m | 286.817047 K |
| 1132.992432 m | 277.940491 K |
| 13156.470703 m | 221.304260 K |
| 37143.621094 m | 238.244354 K |
| 76287.062500 m | 186.494110 K |

The #30 real geometry oracle output is frozen by SHA-256
`7fe3f5fa17d0067464258c93efbe1bbf9cf2403b4d442191b08b6deb29e026a4`.
The resulting #71 interpolation-oracle output is frozen in provenance as
`5679760f10c75679ecd597979b5caadfd3bf30debbbf9b0fa1fc6af1aa9e3771`.

CI step 2b regenerates and validates the real #30 column against pristine
FLEXPART. Step 2d derives the real #71 input from that output, runs the pinned
interpolation routines, and requires the complete case, oracle-output hash and
source hash to reproduce the committed contract. This keeps #29 provider
normalization and #30 vertical transformation outside #71 while proving that
#71 samples their real handoff rather than merely pointing at a descriptor.

## 7. Interface-staggered #30 handoff fixture — not end-to-end W production parity

#73 consumes the derived runtime geometry owned by #30: model-level heights for
center-staggered fields and FLEXPART-`wzlev`-compatible W/interface heights for
interface-staggered vertical motion.

The `vertical-interface-wzlev` fixture freezes that **handoff contract**:

1. #30's direct oracle driver calls pristine
   `verttransform_mod::verttransform_ecmwf_heights`.
2. Its `wzlev` output supplies the four W/interface AGL heights
   `[0, 1954.7922363, 4363.8686523, 7076.8583984] m` in bottom-to-top order.
3. The same direct oracle's `omega * pinmconv` result supplies the
   interface-staggered geometric vertical-velocity values
   `[0.1353315860, 0.1002457738, 0.0602269098, 0.0] m/s`.
4. #71 feeds that real #30 geometry/value profile through the pristine
   `find_z_level_meters`, `find_vert_vars` and `vert_interpol` primitives at
   interior and boundary sample heights.

The #30 source output is frozen by SHA-256
`5015ea3a9a9e42b1a2b88c60c2867b74a632bffd1b9cfefdc186b005c752b197`.
CI step 2d refuses to generate the interface fixture if the step-2b direct-oracle
output differs from that evidence. The fixture declares
`vertical_staggering=level_interface`, and the Rust validation test requires every
query in that case to carry the interface coordinate id.

### Important production-path boundary

This fixture must **not** be interpreted as proof that direct interpolation on
`wzlev` is numerically identical to pristine FLEXPART's final `eta=no` W sampling.

The pinned meter-coordinate production path first remaps interface/native W onto
FLEXPART's shared `height[]` grid inside
`verttransform_mod::verttransform_ecmwf_windfields`. Particle sampling later runs
through the public `interpol_mod::interpol_wind` entry point, which dispatches to
`interpol_wind_meter` and samples `windfields_mod::ww` on that shared height grid.
That is a two-stage vertical path:

```
W/interface geometry + pressure velocity
  -> verttransform_ecmwf_windfields
  -> ww on FLEXPART height[]
  -> interpol_wind
  -> interpol_wind_meter
  -> final particle w
```

#71 currently freezes the #30 handoff and the underlying linear interpolation
primitive, but does not directly execute this full two-stage path. Because two
successive interpolations are not generally equivalent to one direct interpolation
for an arbitrary non-linear profile, no end-to-end W parity claim is made here.

Issue #80 freezes that production path in
`fixtures/interpolation/w-production-oracle-v1.json`. Its deliberately nonlinear
interface-omega profile is executed through the compiled pristine routines and sampled
at the lower and upper boundaries plus three strict-interior heights. The retained
linked-executable evidence verifies these call edges:

```
driver -> verttransform_ecmwf_heights
driver -> verttransform_ecmwf_windfields
driver -> interpol_wind
interpol_wind -> interpol_wind_meter
```

The result is **not equivalent**. The largest observed absolute difference between
the pristine two-stage result and direct interpolation on the #30 interface geometry
is `0.057038949297001734 m/s`, compared with the declared combined absolute/relative
tolerance (`1e-6 m/s`, `1e-5`). The upper production-grid boundary also differs:
pristine FLEXPART returns `0 m/s`, while direct interface interpolation returns
`0.015056730163961408 m/s`.

Therefore #73 must reproduce the frozen two-stage result, or document an intentional
canonical divergence with separate candidate and oracle expectations. This issue does
not implement that change and does not remove `InterfaceVerticalMotionBlocked`.

The checked-in canonical ERA5/ETEX fixture contains the #29/#30 thermodynamic column
but no vertical-motion field. The #80 report records
`not_available_in_checked_in_canonical_fixture` and retains the synthetic direct-oracle
proof without adding provider decoding or eta-dot preprocessing.

The current fixture remains normative for its narrower claim: #30 W/interface
geometry/staggering, units/order, and the behavior of the pinned primitive
`find_z_level_meters -> find_vert_vars -> vert_interpol` on that handoff.

## 8. Out-of-contract semantics (fail closed)

The following are deliberately **not** frozen by #71; issue authors must treat them as
unresolved and must not invent behavior:

- `numpf = 3` temporally-equidistant precipitation scheme
  (`interpol_mod.f90:1317-1340`): dead in the pinned `numpf=1` build. Freezing it
  requires a separate `numpf=3` oracle build.
- End-to-end meter-coordinate W production sampling remains outside #71. The separate
  #80 report freezes it and concludes that direct #30-interface sampling is not
  equivalent. The `vertical-interface-wzlev` case remains a #30 handoff/primitive
  fixture only.
- Native ETA-mode `interpol_wind_eta` execution (`interpol_mod.f90:1590-1650`):
  not part of the canonical #30 runtime-geometry boundary.
- Logarithmic vertical interpolation (`log_interpol=.true.`): not active in the pinned
  build; `find_vert_vars` (`interpol_mod.f90:364-392`) is documented but unreachable.
- Nesting (`ngrid > 0`), polar-overshoot pole handling (`find_ngrid_sp/dp`), and
  `nxshift` non-zero grid rotation.

Any candidate implementation that needs these must open a follow-up oracle issue rather
than extending this contract.

## 9. Artifacts

- `scripts/interpolation/direct_interpolation_oracle.f90` — modes `horizontal`,
  `vertical`, `temporal`, `rain`; calls the pinned routines; versioned output header
  `FLEXPART_INTERPOLATION_ROUTINE_ORACLE_V1`.
- `scripts/interpolation/direct_oracle.sh` — container build + run harness; records
  the actual compiler version and complete sorted linked-object set alongside the binary.
- `fixtures/interpolation/*.json` — canonical sampling cases (schema
  `flexpart-gpu.interpolation-contract.v1`) with embedded golden values and provenance,
  including the `vertical-interface-wzlev` **handoff/primitive** fixture sourced
  from #30's direct FLEXPART `wzlev`/`pinmconv` evidence (not end-to-end W production
  parity) and `real-era5-etex-temperature-column`,
  which samples a real #29 temperature profile on #30 direct-oracle geometry.
- `fixtures/meteorology/era5-etex-native-v1.json` plus its provenance and
  surface archive — real #29 source referenced by `era5-etex-real-column-v1`; #30 CI
  supplies the pinned direct-routine vertical-oracle evidence for that selection.
- `reference/flexpart-11.1.json` — pinned reference manifest.
