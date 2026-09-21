# Canonical vertical transformation (#30)

This document defines the production vertical-coordinate boundary implemented for
RISK-03.3G-10b. It complements the canonical meteorology contract in
`docs/meteorology-contract.md`.

## Input and output boundary

The versioned `meteorology::Snapshot` is immutable normalized input. Vertical
transformation derives a separate `VerticalTransformResult`; it does not rewrite
the Snapshot and does not collapse the column-dependent geometry to the legacy
`WindFieldGrid::heights_m` one-dimensional mean profile.

The derived runtime state contains, per horizontal column:

- pressure on hybrid/W interfaces `[x,y,z+1]`;
- FLEXPART-compatible W/interface height AGL and ASL `[x,y,z+1]`;
- pressure on model levels `[x,y,z]`;
- model-level height AGL `[x,y,z]`;
- model-level height ASL `[x,y,z]`;
- local terrain/orography ASL `[x,y]`;
- normalized upward-positive geometric vertical velocity when #30 vertical-motion
  normalization is available.

The derived transform provenance also binds the runtime geometry to the exact
canonical Snapshot using a SHA-256 of its compact serialized representation.
When native vertical motion is normalized, its complete provider-independent
contract is bound independently by SHA-256 as well.

## Hybrid pressure and FLEXPART level mapping

Schema v1 preserves the provider/native half-level coefficients `A_half` [Pa]
and `B_half` [1]. At each horizontal cell and actual local surface pressure
`p_s`:

```
p_half(k) = A_half(k) + B_half(k) * p_s
A_full(k) = 0.5 * (A_half(k) + A_half(k+1))
B_full(k) = 0.5 * (B_half(k) + B_half(k+1))
p_full(k) = A_full(k) + B_full(k) * p_s
```

The last expression is numerically equivalent to the mean of the two adjacent
interface pressures. The implementation keeps the canonical Snapshot ordering
but derives physical traversal direction from `VerticalOrdering`; no fixed
137-level count or hard-coded pressure-index direction is allowed.

Because FLEXPART anchors the lowest W/interface coordinate at the physical
surface (`0 m AGL`), the surface-side hybrid interface must reconstruct to the
**actual local** `surface_pressure` for every column. A coefficient set that
matches only `reference_surface_pressure_pa` but yields a different pressure
at a local column is rejected fail-closed rather than pairing a non-surface
pressure with the 0 m AGL W surface.

FLEXPART 11.1 constructs ECMWF vertical coordinates differently in memory:
`windfields_mod` reverses the native half-level coefficient order into its
bottom-to-top arrays, inserts an artificial ground model level with
`akz(1)=0, bkz(1)=1`, derives the real model-level coefficients as adjacent
half-level means, and increments `nuvz`. Therefore FLEXPART
`verttransform_ecmwf_heights` starts at its artificial level 1 and loops from
`kz=2` through the real model levels. The canonical #29 Snapshot does not
serialize that artificial level; #30 starts from the local surface boundary and
visits the surface-most real model level first.

## FLEXPART 11.1 height reconstruction

Exact parity uses the surface thermodynamic state, not a lowest-model-level
fallback. For each column FLEXPART initializes:

```
Tv_surface = T2m * (1 + 0.378 * ew(Td2m, p_s) / p_s)
p_old = p_s
z_agl(surface) = 0
```

where `ew` is FLEXPART `qvsat_mod::ew`. For each real model level upward:

```
Tv = T * (1 + 0.608 * q)

if abs(Tv - Tv_old) > 0.2 K:
    dz = (R_air/g) * ln(p_old/p) *
         (Tv-Tv_old) / ln(Tv/Tv_old)
else:
    dz = (R_air/g) * ln(p_old/p) * Tv
```

The level AGL height is the cumulative `dz`. Canonical orography is metres
above mean sea level, so:

```
height_asl = terrain_asl + height_agl
height_agl = height_asl - terrain_asl
```

Below-sea-level terrain is valid. Impossible/non-finite pressure,
thermodynamic state, or geometry fails closed.

### W/interface geometric heights

FLEXPART uses a distinct geometric coordinate for W (`wzlev`). It is not
represented by the T/q/UV model-level heights alone. In physical bottom-to-top
order FLEXPART sets the surface W height to 0 m AGL, computes interior W
heights from adjacent UV/T/q level heights, and extrapolates the uppermost W
height. #30 reconstructs this coordinate explicitly and stores both AGL and ASL
forms in `VerticalTransformResult`.

The canonical top-to-bottom/bottom-to-top storage direction is preserved by
`VerticalOrdering`; no nearest-level or vertical interpolation occurs here.
#31 receives each interface pressure together with its W geometric height and
the retained interface-staggered vertical velocity.

## Oracle validation

The per-PR technical gate produces machine-readable #30 evidence below
`target/ci-gate/vertical-column/`. Validation intentionally has two independent
Fortran paths with different roles.

### Normative pinned-routine oracle

The normative #30 comparison executes the **compiled FLEXPART 11.1
`verttransform_mod::verttransform_ecmwf_heights` routine from the pinned full
model build**, not a locally rewritten or extracted copy of its equations.

The technical gate first builds the complete pristine FLEXPART checkout at the
commit pinned by `reference/flexpart-11.1.json`. The focused
`direct_oracle_driver.f90` is then compiled against the resulting
`verttransform_mod.mod` / `windfields_mod.mod` interfaces and linked with the
same FLEXPART object files produced by that build. Only `FLEXPART.o` is
excluded because it defines the normal model main program.

The driver initializes the vertical-coordinate state in the **real
`windfields_mod` module** from the canonical column fixture and calls
`verttransform_ecmwf_heights` directly. CI additionally verifies that the
resulting oracle binary contains the gfortran module symbol for that routine and
records SHA-256 hashes for the driver binary, `verttransform_mod.f90`,
`windfields_mod.f90`, `verttransform_mod.o`, and `windfields_mod.o`.
The sibling FLEXPART checkout remains clean and unmodified throughout.

This gives #30 a direct execution oracle for FLEXPART's model-level pressure,
hypsometric-height, `wzlev`, and `pinmconv` calculations while avoiding a
full meteorological file/model run for each column test. The small amount of
driver plumbing that maps canonical top-to-bottom half-level A/B metadata into
FLEXPART's bottom-to-top module state is separately guarded against the pinned
`windfields_mod` source contract. The driver also populates the real
`windfields_mod::akm/bkm` half-level arrays using that pinned mapping and emits
the resulting local interface pressure `akm + bkm * ps`; the comparison gate
checks every candidate interface pressure with the same pressure tolerances as
the model levels.

### Secondary source-conformance harness

`scripts/vertical/oracle_column.f90` remains as an independent, small scalar
replay of the relevant equations. It is useful for diagnostics and catches
unexpected disagreement between the Rust implementation, the directly linked
real FLEXPART routine, and our interpretation of the source. It is **not** the normative
oracle and its output is explicitly labelled
`FLEXPART_VERTICAL_CONFORMANCE_HARNESS_V1`.

### Controlled synthetic column

`fixtures/vertical/synthetic-column-v1.json` is a 1x1x3 hybrid column with
non-zero A and B coefficients. The Rust candidate is compared field-by-field
against the directly linked pinned FLEXPART routine. Interface omega is multiplied by the
`pinmconv` produced by that real FLEXPART routine, preserving the exact
FLEXPART pressure-to-height derivative in the comparison.

Outputs include:

- `comparison-report.json` — normative candidate vs pinned-routine result;
- `conformance-comparison-report.json` — secondary scalar-harness check;
- `routine-oracle-provenance.json` — pinned commit and source/routine hashes.

### Real ERA5/ETEX column

The gate also derives a 1x1x137 validation-only Snapshot from the checked-in
`era5-etex-native-v1.json` model-level fixture and
`era5-surface-19941023-24.npz`. It adds the exact FLEXPART surface boundary
(T2m, Td2m, surface geopotential converted to metres ASL) without modifying
the completed #29 fixture.

Outputs include:

- `real-column-snapshot.json`;
- `real-column-fixture-provenance.json`;
- `real-comparison-report.json` — normative candidate vs pinned routine;
- `real-conformance-comparison-report.json` — secondary harness comparison.

Both normative comparisons use predeclared tolerances:

- pressure: max(0.05 Pa absolute, 1e-6 relative);
- height: max(0.02 m absolute, 1e-5 relative);
- geometric vertical velocity: max(2e-5 m/s absolute, 1e-5 relative).

A missing artifact, dirty/wrong oracle checkout, oracle/provenance mismatch,
changed source contract, level-count mismatch, wrong execution-mode header, or
field outside tolerance fails the technical gate.

## Vertical motion normalization

Canonical `FieldId::VerticalVelocity` is always geometric vertical air velocity in
`m/s`, positive upward. Native/provider representations are never inserted into
that field before #30 normalization.

The provider-independent native contract records four independent facts:

- representation kind: geometric velocity, pressure velocity (omega), or eta-dot;
- unit: `m/s`, `Pa/s`, or `1/s`;
- sign convention;
- vertical staggering (model center or interface).

Unsupported or ambiguous combinations fail closed. Provider parameter ids and
variable names remain #32 concerns.

### Already-geometric velocity

`m/s`, positive upward is an identity conversion. Staggering is retained.

### Pressure velocity / omega

Omega is `dp/dt` in `Pa/s`, positive toward increasing pressure. FLEXPART 11.1
describes its W input as Pa/s and converts it to geometric vertical velocity by
multiplying by `pinmconv = dz/dp`. Since pressure decreases with height,
`dz/dp < 0`; therefore negative omega (rising air) becomes positive geometric
`w`.

Only the native interface/W representation is enabled for pressure velocity.
Center-staggered omega fails closed because FLEXPART 11.1's validated W path is
interface-staggered and #30 has no independent pinned oracle for a center-grid
omega conversion. #30 does not invent a center-to-interface interpolation;
that would belong to #31 if a future validated input contract requires it.

For the native interface/W representation, #30 reproduces FLEXPART's
`pinmconv` discretization on the physical bottom-to-top column, including the
artificial surface model level:

- one-sided `dz/dp` at the lower boundary;
- centered `dz/dp` in the interior;
- one-sided `dz/dp` at the upper boundary.

The normalized values retain their vertical staggering. #30 does not interpolate
them onto another vertical grid; #31 owns interpolation/sampling.

The synthetic column oracle supplies explicit interface omega values to the
Rust candidate and the pinned-routine driver. The oracle-side geometric values
use the `pinmconv` produced by the actual directly linked FLEXPART
`verttransform_ecmwf_heights` routine and are compared interface by interface.

### Raw eta-dot

Raw ECMWF eta-coordinate velocity (`dη/dt`) is represented by the native
contract so provider adapters can identify it explicitly, but **#30 does not
currently normalize it in production**.

The FLEXPART-facing reference is clear about the boundary: FLEXPART consumes a
pressure vertical velocity in `Pa/s`; ECMWF parameter 77 (`dη/dt`) is
preprocessed by `flex_extract` / `calc_etadot`, including multiplication by
`dp/dη`, before it becomes FLEXPART-ready input. What is not yet pinned in
this repository is an independent executable/golden reference for the complete
full-level-to-half-level preprocessing and its indexing/sign semantics.

Therefore `NativeVerticalMotionKind::EtaCoordinateVelocity` fails closed with
`InvalidNativeVerticalMotion` even when its unit/sign/staggering metadata are
otherwise valid. No candidate-side recurrence is accepted as scientific
evidence for itself.

Eta-dot support may only be enabled after a pinned independent preprocessing
reference (for example the matching `flex_extract calc_etadot` implementation
or golden outputs produced by it) is added and compared against the candidate.
Follow-up #70 owns that work.
Until then, the supported FLEXPART parity boundary is preprocessed pressure
velocity / omega in `Pa/s`.

## Integration boundaries for #28 and #31

### Release heights consumed by #28

#30 exposes `resolve_release_height` and `resolve_release_height_at_column`.
They require an explicit `VerticalReference::AboveGroundLevel` or
`VerticalReference::AboveMeanSeaLevel`; `ModelNative` is rejected.

The returned `ResolvedReleaseHeight` preserves:

- the original numeric input and reference;
- local terrain ASL;
- resolved AGL height;
- resolved ASL height.

AGL must be non-negative. ASL below local terrain fails closed. Terrain below
mean sea level remains valid. #28 should perform release sampling in the reference declared by canonical
`ReleaseSpec`. For a release exactly on a canonical horizontal grid cell,
`*_at_column` is a direct integration/test convenience. For an arbitrary
lon/lat release between grid points, #31 must first provide terrain using its
canonical horizontal sampling semantics; #28 then passes that sampled terrain
to `resolve_release_height` / `resolve_release_height_range`. #28 must not
choose a nearest column implicitly, reimplement terrain offsets, or infer
AGL/ASL from legacy configuration.

#26 still owns the canonical `ReleaseSpec` field that declares AGL vs ASL;
#27 owns particle creation/injection. #30 intentionally does not modify
`ReleaseConfig` or guess a reference while those tickets remain open.

### Runtime geometry consumed by #31

`VerticalTransformResult::runtime_view()` is the provider-independent runtime
boundary for interpolation/sampling. `VerticalTransformResult` and
`NormalizedVerticalMotion` are opaque, serialize-only derived types: external
callers cannot construct, deserialize, or mutate them to recombine geometry and
motion from different Snapshots. Construction validates all derived array
shapes, including normalized vertical-motion staggering, before exposing data.

The borrowed `VerticalRuntimeView` provides:

- runtime dimensions;
- local terrain ASL by horizontal cell;
- pressure, AGL height and ASL height at model-level indices;
- interface pressure plus FLEXPART `wzlev`-compatible AGL/ASL height;
- the normalized vertical-motion field with its retained staggering.

The view performs no clamping or interpolation. Out-of-bounds access fails.
#31 owns horizontal/vertical/temporal interpolation and must consume this
runtime geometry instead of deriving a second height coordinate or using
legacy `WindFieldGrid::heights_m`.
