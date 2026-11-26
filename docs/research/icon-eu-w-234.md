# ICON-EU vertical-motion decision (#234)

**Verdict `UNSUPPORTED`.** The continuum transformation is determined. The
regular-grid wind reconstruction and the shared-height boundary policy lack
the independent evidence required to admit the result to the pinned FLEXPART
11.1 transport path. The [versioned decision](../../fixtures/meteorology/icon-eu-w-234-v1/decision.json)
records the blocked handoff and binds the [research evidence](../../fixtures/meteorology/icon-eu-w-234-v1/evidence.json).
Scope is the deterministic ICON-EU regular-lat-lon export audited by
[#230](icon-eu-provider-230.md) and the geometry decided in [#231](icon-eu-vertical-231.md).

## Source quantities and sign

WMO GRIB2 parameter (0,2,9) identifies geometric vertical velocity. For this
export, `W = Dz/Dt` in m/s, positive toward greater geometric height ASL. The
index of the half level increases **downward** from 1 (model top) to 75
(surface). Matching vertical-grid UUIDs bind W and the time-invariant HHL.
U/V are earth-relative east/north winds, horizontally collocated on the export
grid and vertically at the 74 full levels. Full level (k) lies midway in
geometric height between HHL(k) and HHL(k+1). Source evidence comes from
[#230](icon-eu-provider-230.md), [#231](icon-eu-vertical-231.md), the
[WMO GRIB2 parameter entry](https://ci.codes.wmo.int/grib2/codeflag/4.2/0-2-9) and the
[ICON dynamical core paper](https://doi.org/10.1002/qj.2378).

Public ICON release `release-2024.10-public` directly distinguishes the
prognostic geometric `w` from the contravariant correction. The velocity
advection initializes the contravariant velocity from `w` and subtracts
`w_concorr_c`. In the solver, the lower-boundary geometric W equals that
correction, while the contravariant mass flux there is zero. At the top the
solver has separate rigid-lid and vertically nested branches. The public
release establishes the model conventions. The operational DWD export provides
the independent product-specific observations below.
[Velocity advection, lines 468–510](https://gitlab.dkrz.de/icon/icon-model/-/blob/release-2024.10-public/src/atm_dyn_iconam/mo_velocity_advection.f90#L468-510) ·
[Nonhydrostatic solver, lines 2246–2279](https://gitlab.dkrz.de/icon/icon-model/-/blob/release-2024.10-public/src/atm_dyn_iconam/mo_solve_nonhydro.f90#L2246-2279)

## Transformation

Let `Z(x,y,s)` be geometric height ASL of the static ICON surface labeled by
an upward-oriented continuous coordinate `s`. Let `H(x,y) = Z(x,y,s_ground)`
be terrain height and `A = Z − H` be height AGL. Use a local east/north tangent basis for horizontal
derivatives. The chain rule on a particle gives

```text
w = dZ/dt = U ∂x Z|s + V ∂y Z|s + (∂s Z) ds/dt
W_cross = (∂s Z) ds/dt = w - U ∂x Z|s - V ∂y Z|s
dA/dt = w - U ∂x H - V ∂y H
       = W_cross + U ∂x (Z-H)|s + V ∂y (Z-H)|s
```

The vertical velocities are upward-positive in m/s. The slopes are dimensionless.
`W_cross` is the vertical metric rate through the native surface. A coordinate
index rate additionally divides by the corresponding vertical Jacobian, with
the opposite sign for an index that increases downward. A moving coordinate
would add a `−∂t Z` term to `W_cross`. HHL is static.

On the exported grid a candidate interior stencil at one fixed half-level
index uses opposite neighbors. For longitude/latitude increments in radians,
the denominators are `2 R cos(φ_j) Δλ` eastward and
`2 R Δφ` northward, with the GRIB sphere radius of 6,371,229 m. This
is a shallow-atmosphere export-grid diagnostic. The region is nonperiodic.
The experiment excludes the outer row and column on each side. A production
decision needs an explicit regional-edge treatment and an error budget for
the product remapping and the metric approximation.

For half levels 2–74, let `z_u = z_full(h−1)`, `z_l = z_full(h)` and
`z_i = HHL(h)`. A piecewise linear wind profile in local geometric height gives
`U_i = U_l + (U_u - U_l) * (z_i - z_l) / (z_u - z_l)`, and the same for V.
A positive bracket is required. Equal weights require equal distances to the
two full levels. ICON uses related height weights on its native grid and
computes the terrain correction from edge winds and native metrics, with
three-point extrapolation of edge winds at the lower boundary. The
regular-grid diagnostic cannot recover that discrete operator from exported
cell-center U/V. Native interpolation, products with slopes, and export
remapping generally do not commute.
[ICON vertical grid, lines 910–947](https://gitlab.dkrz.de/icon/icon-model/-/blob/release-2024.10-public/src/atm_dyn_iconam/mo_vertical_grid.f90#L910-947) ·
[ICON velocity advection, lines 229–315](https://gitlab.dkrz.de/icon/icon-model/-/blob/release-2024.10-public/src/atm_dyn_iconam/mo_velocity_advection.f90#L229-315)

## Boundaries and FLEXPART handoff

- At the stationary impermeable terrain, `Z = H`, so both `W_cross` and
  `dA/dt` are zero. This is a boundary condition on the **transformed**
  quantity. Raw geometric W can equal the terrain-following motion
  `U ∂x H + V ∂y H`. Neither 10 m wind nor a lowest-full-level copy
  establishes the actual interface U/V. Using the latter below is a sign check.
- At the observed flat ASL model top, `W_cross = W` without an extrapolation
  of U/V for the HHL slope term. The AGL rate still needs the terrain slope
  term and a justified top U/V or a separate transport endpoint policy.
  A flat ASL surface is sloped in AGL over relief. The pinned export has
  nonzero W at the top, so imposing a rigid lid would change the source.
  The sample alone does not establish the operational nesting configuration.
- [#30](../vertical-transform.md) describes the pinned ECMWF native motion
  normalization with `pinmconv = dz/dp` (negative m/Pa). The ECMWF reader
  zeros surface W for its full interface profile (`windfields_mod.f90:2470–2473`).
  Normalization and interface remapping are in `verttransform_mod.f90:544–545,615–621`.
  The ICON transform is
  kinematic and uses HHL-derived ASL/AGL interface heights from #231 rather
  than reconstructing ECMWF `wzlev`. The canonical upward geometric
  velocity identity in [#29/#30](../meteorology-contract.md) cannot supply the
  required coordinate semantics by itself.
- In the pinned `eta=no` path, [#186](interior-w-186.md) shows that FLEXPART
  remaps normalized native-interface W and then **adds** the slopes of
  native **AGL center** heights using remapped U/V and the current column's
  center bracket. Regional edges and target endpoints skip this addition.
  The sign matches the second chain-rule step in the continuum. The discrete
  stencil and its sampling order are tied to the ECMWF reference.
  [#188](../shared-height-w-preparation.md) requires the exact normalized
  ECMWF omega identity, and [#190](../shared-height-vector.md) adopts its
  prepared W. Feeding either raw W or an already AGL-corrected W into that
  sequence can produce an extra slope term. Feeding W_cross needs a separately
  validated ICON remap and endpoint rule before it could be admitted.
  The transport top is a separate boundary from the source model lid.
  [FLEXPART source pin](../../reference/flexpart-11.1.json) ·
  [Pinned transport top decision](../transport-top.md)

## Verification and accuracy

The independent analytical reference advances a particle exactly along a
straight trajectory using rational arithmetic. It differences explicit
particle-to-plane and particle-to-ground heights at −1 and +1 s.
It calls neither the wind interpolation nor the candidate slope subtraction.
The candidate is evaluated against those rates and separately specified
expected values. The nonuniform bracket returns the exact half-level wind
(5, 1) m/s. All five cases agree exactly.

| Case | W_cross (m/s) | dA/dt (m/s) |
| --- | ---: | ---: |
| Flat | 3 | 3 |
| Terrain-tangent flow | 0 | 0 |
| Tilted interior with different terrain slope | −1 | −9 |
| Flat ASL lid over sloped terrain, geometric W = 0 | 0 | −11 |
| Northward downslope | 2 | 2 |

The real evidence uses the 2026-10-31 00 UTC analysis from #230 for wind and
the 2026-11-03 00 UTC archive from #231 for geometry. The reproduction tool
hashes five raw #231 HHL messages and independently redecodes them with ecCodes
2.48.0. The full decoded array hashes match the two audits at every selected
level. Thus the reused static arrays are **bit-identical** to #230, despite
different GRIB2 message digests. No dynamic fields from different cycles are
paired. The wind values used here are the frozen extrema and surface
statistics derived in #230, tied to raw-message and decoded-array digests.
The #230 raw W/U/V arrays were unavailable for a new pointwise comparison.

| Check | Real result |
| --- | --- |
| Surface sign check inherited from #230 | W75 vs. U74/V74 terrain motion correlates at 0.91514. Through-origin regression slope is 0.79473. Mean absolute W75 on flat cells is 0.00001547 m/s. Full-domain maximum absolute W75 is 0.73124 m/s. |
| Model top | HHL gradients are exactly zero. W1 ranges from −0.05399 to +0.07564 m/s, also the W_cross range for the flat lid. |
| Midlevel 37 | Maximum absolute ASL slopes are 0.00466 east and 0.00549 north. W37 ranges from −0.97626 to +3.03631 m/s. The frozen audit lacks adjacent full-level wind 36 for a two-sided slope correction there. |
| Interior half level 74 | Maximum absolute ASL slopes are 0.15506 east and 0.15176 north across 900,625 centered cells. The lower wind weight spans approximately 0.463 to 0.699. |
| Interior half level 74, extrema enclosure | For height-linear U/V, the convex weights bound each wind component by the larger adjacent absolute field extremum. The triangle inequality then bounds the absolute slope correction by 5.064 m/s, enclosing W_cross in [−5.910, +5.391] m/s. The corresponding native AGL slope term has a 0.003604 m/s bound. These are enclosures, with no pointwise pairing, cancellation or expected error inferred from the extrema. |

For level 74, casting heights and performing the stencil numerator
subtractions in f32 while retaining f64 spacings changes the east/north slopes
by at most 1.29 × 10⁻⁸ and 4.40 × 10⁻⁹. With the pinned
wind extrema, the induced error in the slope correction is bounded by
2.63 × 10⁻⁷ m/s. This covers only those f32 operations. It excludes
wind rounding, coordinate metrics, the differencing and interpolation error,
source packing and native-to-export remapping. The high surface correlation
supports the sign and physical interpretation. Its regression coefficient
does not justify scaling W or setting an acceptance tolerance. There is no
measured pointwise transformation error for the operational export.

## Reproduction and follow-up

Run from the repository root. The archive path contains the cycle directory
and the uncompressed messages identified in the [#231 retrieval manifest](../../fixtures/meteorology/icon-eu-vertical-231-v1/retrieval-manifest.json).
The tool requires NumPy and ecCodes for that replay and records both versions.
It verifies the decision and upstream artifact digests before decoding.
The offline standard-library test also checks the frozen output digest.

```text
python -I scripts/provider/icon_eu_w_234.py --geometry-archive <231-archive>
python -I scripts/provider/test_icon_eu_w_234.py
```

A subsequent research issue needs to pin a same-cycle sample with pointwise W,
interior and boundary wind reconstruction inputs, and an independent native
or publisher-derived contravariant reference (including its mapping to the
regular export). It needs tolerances for slope, staggering, remapping and
regional edges plus independent checks of both boundary levels. A separate
schema/runtime and shared-height handoff decision can then specify the motion
kind and prove the ICON route relative to AGL, including its upper-endpoint
semantics. Those dependencies build on geometry [#241](https://github.com/Grodahn/flexpart-gpu/issues/241)
and [#243](https://github.com/Grodahn/flexpart-gpu/issues/243). The later
production issue owns GPU execution and validation of the chosen route.

Data source: Deutscher Wetterdienst (DWD), CC BY 4.0, as attributed in #230/#231.
Research provenance: GPT-6 Sol.
