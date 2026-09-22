# FLEXPART 11.1 Interpolation Oracle Contract

Status: frozen reference for issues #72 (horizontal), #73 (vertical), #74 (temporal),
and #75 (accumulated-field interval/rate normalization).

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
driver and FLEXPART reference manifest. CI requires those metadata fields to reproduce
exactly.

## 2. Horizontal grid conventions

FLEXPART reads grids with cell-center samples. The canonical convention frozen here is
cell-center anchored. Geographic positions are converted by the **real pinned**
`point_mod::coordtrafo` production routine before interpolation:

```
xt = (lon_deg - xlon0_deg) / dx_deg_x
yt = (lat_deg - ylat0_deg) / dy_deg_y
```

The direct oracle configures `point_mod::{xlon0,ylat0,dx,dy}`, passes the
geographic query through `coordtrafo`, and then feeds the returned `xt/yt`
through `find_grid_indices -> find_grid_distances -> hor_interpol_4d`.
Consequently this Lon/Lat-to-grid conversion is direct FLEXPART oracle evidence,
not a formula implemented only by the fixture generator or documentation.

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

### Verified golden (periodic wrap)

Same grid with `periodic=1` (canonical `nx=4`, FLEXPART `nxmax=5`), query
`(xt,yt) = (3.2, 1.5)`, `kz=1`:

```
ix, jy, ixp, jyp = 3, 1, 4, 2
value            = 355.0        (ixp=4 wraps to column 0)
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

The pinned FLEXPART call path separates interpolation math from wind-memory
coverage policy:

```
getfields_mod::getfields
    -> maintains the wind fields / memtime entries held in memory

advance_mod::advance
    -> interpol_mod::init_interpol(itime, ...)
        -> find_grid_indices / find_grid_distances
        -> find_time_vars(itime)
    -> interpol_* consumers
        -> temporal_interpolation(...)
```

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

Input units (pinned build): precipitation fields are stored as **mm/h rates**; ECMWF
data is read without conversion (accumulation -> rate conversion is owned by the
pre-processing pipeline such as flex_extract), GFS data is multiplied by 3600 at read
(`windfields_mod.f90`). FLEXPART performs **no** accumulation deconvolution.

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
FLEXPART. Step 2c derives the real #71 input from that output, runs the pinned
interpolation routines, and requires the complete case, oracle-output hash and
source hash to reproduce the committed contract. This keeps #29 provider
normalization and #30 vertical transformation outside #71 while proving that
#71 samples their real handoff rather than merely pointing at a descriptor.

## 7. Interface-staggered vertical sampling on #30 W geometry

#73 does not reconstruct FLEXPART's native ETA coordinate. Its normative input is the
derived runtime geometry owned by #30: model-level heights for center-staggered fields
and FLEXPART-`wzlev`-compatible W/interface heights for interface-staggered vertical
motion.

The `vertical-interface-wzlev` fixture therefore freezes that exact handoff rather
than pretending that a coordinate label changes the METRE interpolation routine:

1. #30's direct oracle driver calls the pristine pinned
   `verttransform_mod::verttransform_ecmwf_heights` routine.
2. Its `wzlev` output supplies the four W/interface AGL heights
   `[0, 1954.7922363, 4363.8686523, 7076.8583984] m` in bottom-to-top order.
3. The same direct oracle's `omega * pinmconv` result supplies the
   interface-staggered geometric vertical velocity values
   `[0.1353315860, 0.1002457738, 0.0602269098, 0.0] m/s`.
4. #71 feeds that actual W geometry/value profile through the pristine pinned
   `find_z_level_meters`, `find_vert_vars` and `vert_interpol` routines at
   interior and boundary sample heights.

The #30 source output is frozen by SHA-256
`5015ea3a9a9e42b1a2b88c60c2867b74a632bffd1b9cfefdc186b005c752b197`.
CI step 2c refuses to generate the interface fixture if the step-2b direct-oracle
output differs from that evidence. The fixture also declares
`vertical_staggering=level_interface`, and the Rust validation test requires every
query in that case to carry the interface coordinate id.

This proves both vertical sampling classes required by #71 at the #30/#73 boundary:
`level_center` over model-level geometry and `level_interface` over real
FLEXPART-derived W geometry.

A full ETA-mode `interpol_wind_eta` execution remains outside this contract because
#73 consumes #30 runtime geometry rather than native provider ETA coordinates. It is
not needed to claim interface-staggered parity at the canonical sampling boundary.

## 8. Out-of-contract semantics (fail closed)

The following are deliberately **not** frozen by #71; issue authors must treat them as
unresolved and must not invent behavior:

- `numpf = 3` temporally-equidistant precipitation scheme
  (`interpol_mod.f90:1317-1340`): dead in the pinned `numpf=1` build. Freezing it
  requires a separate `numpf=3` oracle build.
- Native ETA-mode `interpol_wind_eta` execution (`interpol_mod.f90:1590-1650`):
  not part of the canonical #30 runtime-geometry boundary. W/interface sampling itself
  is frozen above using direct FLEXPART `wzlev` geometry.
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
- `scripts/interpolation/direct_oracle.sh` — container build + run harness.
- `fixtures/interpolation/*.json` — canonical sampling cases (schema
  `flexpart-gpu.interpolation-contract.v1`) with embedded golden values and provenance,
  including `vertical-interface-wzlev` sourced from #30's direct FLEXPART
  `wzlev`/`pinmconv` evidence and `real-era5-etex-temperature-column`,
  which samples a real #29 temperature profile on #30 direct-oracle geometry.
- `fixtures/meteorology/era5-etex-native-v1.json` plus its provenance and
  surface archive — real #29 source referenced by `era5-etex-real-column-v1`; #30 CI
  supplies the pinned direct-routine vertical-oracle evidence for that selection.
- `reference/flexpart-11.1.json` — pinned reference manifest.