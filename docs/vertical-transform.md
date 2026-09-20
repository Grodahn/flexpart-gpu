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

The normative #30 comparison executes the **actual FLEXPART 11.1
`verttransform_ecmwf_heights` routine source**, not a locally rewritten copy of
its equations.

At CI runtime, `scripts/vertical/extract_flexpart_vertical_routine.py`:

1. verifies the oracle checkout is clean and exactly at the commit pinned by
   `reference/flexpart-11.1.json`;
2. reads `src/verttransform_mod.f90` from that pristine checkout;
3. extracts the exact contiguous source slice from
   `subroutine verttransform_ecmwf_heights` through its matching
   `end subroutine`;
4. hashes both the original source and extracted routine and writes
   `routine-oracle-provenance.json`;
5. wraps that unchanged routine text only with the imports/state required to
   compile it as a focused column oracle.

The generated routine module is compiled with the pinned FLEXPART
`par_mod.f90` and `qvsat_mod.f90`. A small driver supplies the same
bottom-to-top `akz/bkz/aknew/bknew` state that FLEXPART constructs from the
canonical half-level A/B coefficients, including its artificial surface model
level. The FLEXPART checkout itself is never modified.

This isolates #30 from GRIB/ecCodes/NetCDF and the full model executable while
still executing the pinned FLEXPART implementation of pressure, hypsometric
height, `wzlev`, and `pinmconv`.

### Secondary source-conformance harness

`scripts/vertical/oracle_column.f90` remains as an independent, small scalar
replay of the relevant equations. It is useful for diagnostics and catches
unexpected disagreement between the Rust implementation, the extracted real
routine, and our interpretation of the source. It is **not** the normative
oracle and its output is explicitly labelled
`FLEXPART_VERTICAL_CONFORMANCE_HARNESS_V1`.

### Controlled synthetic column

`fixtures/vertical/synthetic-column-v1.json` is a 1x1x3 hybrid column with
non-zero A and B coefficients. The Rust candidate is compared field-by-field
against the extracted pinned routine. Interface omega is multiplied by the
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

A missing artifact, dirty/wrong oracle checkout, extraction/provenance mismatch,
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
use the `pinmconv` produced by the actual extracted FLEXPART
`verttransform_ecmwf_heights` routine and are compared interface by interface.

### Raw eta-dot

Raw ECMWF eta-coordinate velocity is `dη/dt [1/s]`. FLEXPART core does not use
that raw quantity as `wwh`; FLEXPART preprocessing normally multiplies eta-dot
by the hybrid-coordinate `dp/dη` factor to produce the FLEXPART-ready pressure
vertical velocity in `Pa/s`.

For a hybrid layer bounded by native half-level coefficients:

```
dA = A_lower - A_upper
dB = B_lower - B_upper
pref = reference_surface_pressure
scale = ps * (dA/ps + dB) / (dA/pref + dB)
```

The full-level eta-dot value is mapped to bounding interface pressure velocity
with the centered recurrence used by the preprocessing path. The model-top
pressure-velocity boundary is explicitly zero; the reconstructed interface
omega profile is then passed through the exact same omega→geometric-W
normalization described above.

Eta direction is explicit in the native contract (`positive_eta_increasing` or
`positive_eta_decreasing`) and is normalized before the recurrence. The
conversion provenance therefore distinguishes:

1. raw eta-dot input;
2. eta-dot → FLEXPART-ready pressure velocity;
3. pressure velocity → geometric `m/s`, positive upward.

The eta-dot preprocessing stage is validated independently from the FLEXPART
core oracle because the pristine FLEXPART 11.1 executable expects the
preprocessed pressure-velocity quantity, not raw eta-dot.

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
boundary for interpolation/sampling. Construction validates all derived array
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
