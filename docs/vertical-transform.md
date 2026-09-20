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

- pressure on hybrid interfaces `[x,y,z+1]`;
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

## Oracle validation

The per-PR technical gate produces machine-readable #30 evidence below
`target/ci-gate/vertical-column/`.

### Controlled synthetic column

`fixtures/vertical/synthetic-column-v1.json` is a 1x1x3 hybrid column with
non-zero A and B coefficients. The Rust candidate report is compared level by
level with a Fortran harness. The harness links `par_mod` and
`qvsat_mod` directly from the pristine FLEXPART 11.1 checkout pinned by
`reference/flexpart-11.1.json`; the comparator also verifies that the pinned
`verttransform_mod.f90` and `windfields_mod.f90` still contain the exact
height and hybrid-level source contracts represented by the harness.

Output: `comparison-report.json`.

### Real ERA5/ETEX column

The gate also derives a 1x1x137 validation-only Snapshot from the checked-in
`era5-etex-native-v1.json` model-level fixture and
`era5-surface-19941023-24.npz`. It adds the exact FLEXPART surface boundary
(T2m, Td2m, surface geopotential converted to metres ASL) without modifying
the completed #29 fixture.

Outputs:

- `real-column-snapshot.json`;
- `real-column-fixture-provenance.json`;
- `real-comparison-report.json`.

Both comparisons use predeclared tolerances:

- pressure: max(0.05 Pa absolute, 1e-6 relative);
- height: max(0.02 m absolute, 1e-5 relative).

A missing artifact, dirty/wrong oracle checkout, changed source contract, level
count mismatch, or field outside tolerance fails the technical gate.
