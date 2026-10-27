# Mean-Wind Advection

## Scheme: Petterssen Predictor–Corrector

FLEXPART uses a two-step scheme attributed to Petterssen (1940), which
provides second-order accuracy without storing intermediate derivatives.

### Algorithm

Given a particle at position **x₀** with timestep `dt`:

1. **Interpolate** wind at current position and time:
   `(u₀, v₀, w₀) = wind(x₀, t)`

2. **Predictor** (Euler forward):
   `x_pred = x₀ + dt · (u₀, v₀, w₀)`

3. **Upper transport action** on the predictor: for the supported regional AGL
   route, apply original `pushpartdown` before querying the advanced wind.

4. **Interpolate** at that adjusted predictor and the signed advanced time:
   `(u1, v1, w1) = wind(x_pred_adjusted, t + dt)`.

5. **Correct vertical transport** using the adjusted predictor:
   `z1 = z_pred_adjusted + ((w1 - w0) * 0.5) * dt`, then apply the upper action again.
   Existing horizontal and lower boundary handling remain separately unchanged.

Without an intervening boundary action this is the Petterssen trapezoidal method.
The boundary-adjusted result cannot generally be reconstructed from the original
position plus the averaged vertical wind.

### Velocity-to-Grid Scaling

Wind is in m/s; particle coordinates are in fractional grid cells. The
conversion uses per-particle scale factors:

```
Δcell_x = u [m/s] · x_grid_per_meter · dt
Δcell_y = v [m/s] · y_grid_per_meter · dt
Δz      = w [m/s] · dt                        (vertical stays in metres)
```

where:
- `x_grid_per_meter = 1 / (R_earth · cos(lat) · Δλ · π/180)`
- `y_grid_per_meter = 1 / (R_earth · Δφ · π/180)`

### Wind Interpolation

Trilinear interpolation in (x, y, z):

- **Horizontal**: bilinear on the four surrounding grid points.
- **Vertical**: linear between the two enclosing model levels, after mapping
  `pos_z` [m] to a fractional level index via the `level_heights_m` array.
- **Temporal**: linear between the two bracketing meteorological time steps.

Only the **mean** wind is interpolated here. Turbulent fluctuations are handled
separately by the Langevin equation.

## Pinned upper transport boundary (#180)

The direct [#180 transport proof and finite adoption contract](../research/transport-top-180.md)
execute pristine FLEXPART 11.1 `advance` and `pushpartdown`, including the compiled
Petterssen correction. In the accepted regional `eta=no` route, `height(nz)` is
the endpoint of the run-lifetime shared AGL target initialized on the initial
mother member. It is unchanged by the particle's native column or source time.
Production #198 reuses #184's device target through the complete #197/#190 vector.

The original inclusive test is `z >= height(nz)`. Backward transport with the
settling flag sets sticky `nstop`; other cases assign the f32 expression
`height(nz) - 100*(nxmax/3.e5)`. Boundary action occurs before advanced-time
sampling and after correction, which is applied to the adjusted predictor.
The preceding schematic trapezoidal formula is equivalent only when boundary
actions do not intervene. Production #198 implements the ordinary placement and
ordering. Requested settling remains rejected; the original scientific stop is not a
fatal meteorology condition. The [#203 contract](../research/settling-stop-203.md)
defines the run-owned flag, step-private sticky stop and termination handoff for a
later implementation. See [evidence scope](../transport-top.md).

The proof retains 24 boundary and 72 public-advance cases, exact raw/decoded
observations and two distinct fresh-build provenance inventories. It introduces
no GPU trajectory tolerance. A f64 probe just below the top remains below in
Fortran even when narrowing to f32 would round it to the top; GPU adoption must
state its representable-state scope. Restart, ETA, global/nested/polar routes
and adaptive PBL substeps are not numerically accepted by this experiment.

## Source files

| File | Role |
|------|------|
| `src/gpu/advection_resident.rs` | Canonical forward/backward production orchestration using #171 and #87/#88/#89 |
| `src/shaders/advection_predictor_query.wgsl` | Private resident predictor queries; scientific particles are read-only |
| `src/shaders/advection_corrector.wgsl` | Corrected private timestep state |
| `src/shaders/advection_step_guard.wgsl`, `advection_step_commit.wgsl` | Whole-step eligibility and guarded publication |
| `src/shaders/advection*_dual_wind.wgsl`, `advection.wgsl` | Explicit standalone/legacy diagnostics only |
| `src/physics/advection.rs`, `src/gpu/advection.rs` | Retained CPU/GPU diagnostics |

See [production inventory and evidence](../resident-advection.md). The pinned
`advance_mod.f90:664-779` correction samples the advanced signed time. The
canonical path uses the existing #89 selection independently for both stages;
legacy diagnostics retain their old one-alpha behavior. Existing repository
horizontal clamping and vertical/turbulence coupling are preserved for supported
geometry; they are not a claim of all FLEXPART boundary/stochastic behavior.
Different native column/member tops are admitted through complete preparation;
replacing the run target remains rejected. Scenario-8 trajectory precision has no
approved comparison policy (#204); no trajectory tolerance is introduced.
Both production drivers consume #190's complete prepared shared-height U/V/W
vector; their source preparation, identity and evidence contract is in
[shared advection](../shared-advection.md).

## References

- Petterssen, S. (1940). *Weather Analysis and Forecasting*. McGraw-Hill.
- Stohl, A. et al. (2005). *Atmos. Chem. Phys.*, 5, 2461–2474 (Section 2.2).
