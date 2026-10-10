# Interior-column W slope research (#186)

The direct pinned experiment establishes the regional `eta=no` interior W
correction on nonuniform native columns. It closes the interior evidence gap
left by #118's 2x2 regional boundary fixture. The validation level is direct
pinned-routine synthetic research. It establishes neither GPU parity nor full
FLEXPART production parity. Existing #76/#171 rejection remains active.

## Source authority and sequence

Every location refers to pristine FLEXPART 11.1 revision
`c70586c2b7f5258850705325881c61f557ea9bd8`, selected by
[`reference/flexpart-11.1.json`](../../reference/flexpart-11.1.json).
The [#118 call-chain record](shared-height-118.md#authority-and-exact-production-edges)
still owns the wider initialization, loading and sampling trace.

| Pinned source | Behavior |
| --- | --- |
| `verttransform_mod.f90:1975-2051` | `verttransform_ecmwf_heights` produces native `etauvheight` center AGL geometry, distinct `etawheight` interface AGL geometry, pressure/density and `pinmconv`. |
| `verttransform_mod.f90:2031-2042` | `pinmconv` is the native center-pressure `dz/dp` stencil: one-sided at the two endpoints, centered inside, including artificial ground. It is negative, in m/Pa. |
| `verttransform_mod.f90:489-523` | Separate `idx` center and `idxw` interface brackets associate each strict interior target `height(iz)` with its local column. Neighbor columns do not choose their own target-height brackets for the slope. |
| `verttransform_mod.f90:526-548` | Target endpoint U/V copy native center endpoints; W copies normalized interface endpoints. `cosf(jy)=1/cos((jy*dy+ylat0)*pi180)`. |
| `verttransform_mod.f90:574-612` | U/V are remapped from native centers to `uu/vv(ix,jy,iz,n)` on the common AGL target. An interior target above native center top copies top U/V. |
| `verttransform_mod.f90:615-621` | W is remapped using `idxw`, interface heights and `wwh*pinmconv` at each native interface, **before** the slope is added. |
| `verttransform_mod.f90:627-639` | South/north boundaries skip correction. Regional west/east boundaries skip it too. Global x endpoints wrap their neighbor; this global branch is source-only here. |
| `verttransform_mod.f90:640-655` | Reuses current-column center bracket `idx`, weights neighboring native center heights at those same native indices, and adds the X/Y slope multiplied by **already remapped** `uu/vv`. |
| `windfields_mod.f90:630-631` | `dxconst=180/(dx*r_earth*pi)`, `dyconst=180/(dy*r_earth*pi)`, with degree grid increments. The X term additionally uses `cosf(jy)`. |
| `interpol_mod.f90:954-1029,1651-1703` | Public `interpol_wind` calls `interpol_wind_meter`; sampling is horizontal, vertical, then temporal on the corrected shared fields. |

For an interior target H at current column (x,y), let native center upper index
be K, Z be `etauvheight`, and `d1=H-Z(x,y,K-1)`,
`d2=Z(x,y,K)-H`, `d=d1+d2`. The exact source interpretation is:

```text
SX1 = 0.5 * (Z(x+1,y,K-1) - Z(x-1,y,K-1))
SX2 = 0.5 * (Z(x+1,y,K)   - Z(x-1,y,K))
SY1 = 0.5 * (Z(x,y+1,K-1) - Z(x,y-1,K-1))
SY2 = 0.5 * (Z(x,y+1,K)   - Z(x,y-1,K))
SX = (SX1*d2 + SX2*d1)/d
SY = (SY1*d2 + SY2*d1)/d
W_shared = W_interface_remapped
         + SX*U_shared*dxconst/cos(latitude(y))
         + SY*V_shared*dyconst
```

The differences are metres per grid index; physical inverse spacing makes
the slopes dimensionless, hence the added terms are m/s. The center bracket
is distinct from the W/interface remapping bracket. Native U/V at K, U10/V10,
or separately sampled neighbor U/V must not replace the remapped multipliers.
The report's source-equation replay is labelled diagnostic; the authoritative
observations are differences of four actual unmodified-routine outputs.

Terrain `oro` is metres ASL and does not enter these native AGL differences.
There is no separate explicit orography-slope correction in this code. Different
terrain does not convert the common AGL grid into common ASL surfaces.
The sampler accepts AGL; the recorded ASL number is a caller diagnostic
`AGL + bilinear terrain`, not a directly executed ASL sampler branch.

The slope loop only visits target indices `2..nz-1`; target endpoints retain
their copied interface W and get no slope term, including at the interior
horizontal point. Above-native-center-top behavior in the slope's retained
`idx` branch is source-observed but not accepted numerically here. The accepted
scope requires bracketed strict interior targets in both native center and
interface geometry. Global/nested/polar routes, restart/fallback target
selection, ETA mode, center-W equivalence and transport-top behavior (#180)
remain outside this proof.

## Direct experiment and numerical verdict

The complete [input](../../fixtures/interpolation/interior-w-v1/input.txt) is a
3x3 regional grid at origin (-2,48) degrees, dx=0.25 degrees, dy=0.4 degrees.
The sole horizontal interior point is (1,1), at latitude 48.4 degrees.
It contains three real hybrid layers and the artificial ground center; explicit
bottom-to-top half-level A/B; local pressure; T2m/Td2m; center T/q; U10/V10
assigned to the artificial center; nonlinear native center U/V; native interface
omega in Pa/s; and terrain from 120 to 1120 m ASL. Both horizontal directions
have different thermodynamic profiles and local pressures. Two distinct source
states occur at 0 and 3600 s.

The run-lifetime common target selected by the genuine initializer is
`[0,1137.8990478515625,4574.05419921875,9520.14453125] m AGL`.
The first (0,0) pressure exceeds 100000 Pa, matching #184's supported initializer.
Native profiles change at the second time; the target remains identical.

The [driver](../../scripts/interpolation/interior_w_oracle.f90) calls genuine
`verttransform_init`, `verttransform_ecmwf_heights`, and
`verttransform_ecmwf_windfields` linked from the pristine full model build.
For each source member it executes four calls using identical inputs:
full physical spacing, X-only (`dyconst=0`), Y-only (`dxconst=0`), and zero
(`dxconst=dyconst=0`). The raw output records each control factor. These zeroed
factors are diagnostic experiments, never supported production geometry.
The routine inputs are `intent(in)`; only these two driver-state factors change.
All four U/V arrays and target heights agree exactly. Every boundary W value
and each target endpoint are exactly unchanged across controls.

The driver preserves each actual transformed source pair and calls genuine
`interpol_wind` for each variant. Fifteen queries at fractional (0.7,0.6)
cover lower/upper target boundaries and all three intervening height brackets
at t=0,1800,3600 s. The interior column carries bilinear weight 0.42; it cannot
be omitted without changing the result. The frozen
[raw output](../../fixtures/interpolation/interior-w-v1/output.txt) and
[report](../../fixtures/interpolation/interior-w-v1/report.json) retain every
native/shared/query value, all controls, X/Y/combined differences, and
normalization diagnostics.

| Source time (s) | Interior target index | X-only minus zero (m/s) | Y-only minus zero (m/s) | Full minus zero (m/s) |
| --- | --- | --- | --- | --- |
| 0 | 2 | -0.040551286190748215 | -0.01949474774301052 | -0.060046035796403885 |
| 0 | 3 | -0.10254267603158951 | -0.1550040990114212 | -0.2575467675924301 |
| 3600 | 2 | -0.04898042604327202 | -0.026718389242887497 | -0.07569881528615952 |
| 3600 | 3 | -0.13401979207992554 | -0.18516555428504944 | -0.3191853538155556 |

Each direction independently exceeds #80's predeclared combined velocity
comparison policy: `abs(a-b) <= 1e-6 + 1e-5*max(abs(a),abs(b))`, in m/s.
All selected interior contributions have the same sign; cancellation cannot
provide the verdict. The largest additive residual at an interior source column
is `7.450580596923828e-9 m/s`. Each of the nine strict-interior fractional queries
independently distinguishes both directions, including both source times.
The unchanged endpoints independently confirm the correction-skipping behavior.

## Normalization and existing ownership

The pinned routine multiplies **each native** omega value by its corresponding
`pinmconv` in lines 544-545 and 620-621, then interpolates the normalized values
on W/interface heights. It does not multiply a shared-grid W result by one
target-dependent conversion factor. Negative pressure-increasing omega becomes
positive upward W through the negative native derivative.

#30 already owns this conversion (`omega_interface_flexpart11_pinmconv_v1`),
native interface and center geometry, ordering, and same-Snapshot provenance.
#80 already owns the normalized-interface -> shared-height -> sampling sequence.
The zero-slope control agrees with that existing normalized-interface diagnostic
at every target and distinguishes omitted and double normalization at both
interior target levels of both source times. The four controls and the
wrong-native-U/V diagnostics independently establish the additional slope term
and use of remapped multipliers. There is no new conversion contract.

The #30 arrays have three real centers and four interfaces here. The preparation
view appends the existing artificial zero-height center for U/V and the slope;
U10/V10 supply its velocities. W retains all four interface samples. Neither
center-staggered W nor center omega is proved equivalent. Raw eta-dot and provider
decoding stay with their existing owners. No additional native geometry or
normalization prerequisite was found for this accepted interface-omega route.

## GPU architecture handoff

Future W preparation consumes the existing #30 normalized **interface** motion
in m/s, its `wzlev` AGL coordinates, native center AGL coordinates plus artificial
zero, dimensions/order and Snapshot/geometry/native-motion/normalization hashes.
It also consumes #184's remapped U/V from the **same source, target and context**,
and the canonical regional horizontal geometry (degree dx/dy, latitude origin,
dimensions and non-global boundary identity). It must validate these identities
before encoding. It must not reconstruct pressure, hypsometric heights or omega
conversion, or consume U/V from a different time/bracket.

The run-lifetime target resource and initialization provenance remain #184's
owner. Source native geometry is retained, not averaged or overwritten. W has
the same f32 x-fastest `[target-level,y,x]` layout as U/V, with source interface
staggering and normalized-motion lineage retained in metadata; output samples
are on the common metric grid. Per-source metadata additionally identifies the
slope algorithm, horizontal geometry hash and completed correction stage.
An already corrected W field must not be fed back into preparation.

Sequence for each newly loaded source:

1. Upload matching canonical/#30 inputs through explicit existing resource APIs.
2. Prepare U/V on the persistent shared target through #184.
3. Remap already normalized interface motion using the independent W bracket;
   copy target endpoints as in #80.
4. At strict target interiors and regional interior columns, use the current
   column's native center bracket and same-index X/Y neighbors; add the two
   verified terms using prepared U/V. Skip regional horizontal boundaries and
   both target endpoints exactly as the pin does.
5. Hand immutable corrected W plus matching U/V/shared-target resources and
   preparation status to the existing #76/#171 query composition. Query sampling
   stays horizontal -> vertical -> temporal; it performs no further slope or
   normalization operation.

Use the existing `GpuContext`, GPU interpolation components, status conventions,
typed persistent resources and caller-owned encoder. Keep native geometry,
prepared U/V/W and intermediate/status resources resident on device. Preparation
runs once per new source; retain both bracket members until their last submitted
consumers finish. Adjacent brackets may reuse a source only with identical
source/geometry/normalization/target/horizontal/context identities. Run target
ownership outlives these source resources. With B retained sources, C columns
and N targets, W adds `4*B*C*N` bytes plus status/metadata, without replacing
native data. Validate binding sizes, index ranges and allocation limits.

Missing/failed preparation, missing adapter/device execution, nonfinite or
nonmonotonic geometry, unmatched identities/times/levels, incomplete normalized
interface input, center-W/omega, unbracketed strict target heights, invalid
latitude/spacing, unsupported staggering/global/nested/polar/restart routes,
or resource/index limits must fail closed. Diagnostic zero-spacing controls
are available only in validation. No host fallback, intermediate D2H, second
meteorology sampler/runtime, or independent conversion is justified.

#184 owns U/V preparation; #185 owns U/V query adoption. W preparation and W
adoption have distinct validation boundaries. W adoption must preserve the
complete U/V/W vector identity and existing per-lane/fatal status semantics.
It ends at canonical query integration; production-consumer adoption remains
with #112-#115. None of this resolves #180 or authorizes removing current
nonuniform-geometry rejection before separately validated GPU adoption.

## Reproduction and fail-closed evidence

```text
python scripts/interpolation/run_shared_height_oracle.py --interior-w --checkout <pristine-flexpart> --output-dir target/interior-w-fresh-1
python scripts/interpolation/run_shared_height_oracle.py --interior-w --checkout <pristine-flexpart> --output-dir target/interior-w-fresh-2
python scripts/interpolation/test_interior_w_oracle.py
```

Each invocation archives tracked source from the exact clean pin into a fresh
container-local build, compiles the full pristine model, and uses #80's existing
linker/symbol/disassembly capture. It executes the resolved immutable Docker
image ID, with the manifest's single-thread OMP environment and build profile;
driver flags are `-O0 -fopenmp -mcmodel=large`. No upstream routine is altered.
The complete scratch build is copied back before audit. Source/executable/object
hashes, compiler/image/profile, driver/cache/helper hashes, input/output hashes,
symbols, call sites, linker map and full logs remain in each target directory.
Retained before/after records must both contain only the exact pinned commit,
backed by actual clean-checkout checks. Executable edges prove the driver calls
the real height, wind-field and public sampler routines, and that the public
sampler calls the real meter sampler; the initializer's object binding is also
verified.

Frozen scientific rows and raw output must reproduce exactly across two fresh
builds. Volatile binary, object and log hashes are retained per build rather
than silently required equal across different scratch paths. The paired
reproducibility record retains those run identities. The auditor refuses missing
frozen evidence, incomplete/nonfinite/duplicate records, incorrect control
factors, changed U/V/target/input/query values, either missing direction,
opposite-sign cancellation, nonadditivity, normalization ambiguity, source
changes, missing linkage or malformed image identity. Technical CI reproduces
the oracle and uploads complete fresh artifacts, including on failure.


Published implementation follow-ups: [#188 GPU W preparation](https://github.com/Grodahn/flexpart-gpu/issues/188) and [#190 GPU W canonical adoption](https://github.com/Grodahn/flexpart-gpu/issues/190). They were created after the genuine four-control scientific proof and two fresh-build hash audit passed. #188 depends on #184; #190 depends on #188, #184 and #185. Their scope excludes production-consumer migration.
