# Gravitational Settling (Issue #35)

GPU-executed gravitational settling velocity for spherical aerosol carriers,
validated directly against pinned FLEXPART 11.1. No CPU scientific
implementation participates in acceptance.

## Normative source

Pinned FLEXPART 11.1 revision `c70586c2b7f5258850705325881c61f557ea9bd8`
(see `reference/flexpart-11.1.json`).

| Item | Pinned source | Role |
| --- | --- | --- |
| `get_settling` sphere branch | `src/settling_mod.f90` (`subroutine get_settling`, `ishape == 0` path) | Iterative Reynolds/settling loop, Clift-Gauvin drag, `settling = -sqrt(4*g*d*rho_p*Cun/(3*Cd*rho_air))`, up to 20 iterations, exit when `\|v-v_old\|/\|v\| < 0.01` |
| `viscosity` | `src/settling_mod.f90` (`real function viscosity`) | Sutherland dynamic viscosity: `eta0 = 1.827e-5`, `t_0 = 291.15`, `c = 120` |
| `part0` | `src/drydepo_mod.f90` (`subroutine part0`, single diameter bin) | Cunningham slip (`lam = 6.53e-8`, `alpha = 1.257 + 0.4*exp(-1.1/Kn)` with `eps = 1.2e-38` guard, `Cun = 1 + alpha*Kn`) and Stokes init (`myl = 1.81e-5`, `vsh = g*rho_p*d^2*Cun/(18*myl)`) |
| Species init | `src/readoptions_mod.f90` (`dquer` m-to-um conversion, `part0` call, `vsetaver = -vset`, `cunningham` weighting) | `PDIA`/`PDQUER` [m] to `dquer` [um]; single bin (`maxndia = 1`, `par_mod.f90:192`) gives `dmean = d`, `cunningham = cun`, `vsetaver = -vsh` |
| Constants | `src/par_mod.f90:61` | `ga = 9.81` |
| Shape guardrail | `PSHAPE` handling in `src/readoptions_mod.f90` + #126 scope | Only `PSHAPE == 0` spheres; all other shapes fail closed |

Scientific references: Naeslund and Thaning (1991) for the iterative
settling loop; Sutherland (1893) for viscosity; Clift and Gauvin (1971) for
the sphere drag coefficient; Bagheri and Bonadonna (2016) is explicitly
out of scope (non-spherical path rejected).

The oracle harness `oracle/settling_oracle.f90` implements byte-identical
logic to the rows above. It bypasses only the `xt/yt/zt` grid interpolation
(`settling_mod.f90` temperature/density interpolation): #35 canonical vectors
supply local `T`/`rho_air` directly, which is the documented #35 scope.
Spatial interpolation remains owned by #87-#90/#76.

## Required inputs

Carrier (per-species #10 properties, no shared defaults):

- `PDENSITY` [kg/m3] (`SpeciesConfig::particle_density_kg_m3`)
- `PDIA` [um] (`SpeciesConfig::mean_diameter_um`, converted from [m] per
  `readoptions_mod.f90:2341`)
- `PSHAPE == 0` only (checked in `SpeciesConfig` validation and re-checked
  from the raw species map in `SettlingCarrier::from_species_config`)

Meteorology (local air state, same quantities interpolated by
`settling_mod::get_settling`):

- `temperature` [K]
- `air_density` [kg/m3]

## Declared valid domain

| Input | Range | Rationale |
| --- | --- | --- |
| `diameter_um` | `[0.1, 100]` | Covers Cunningham slip (`<1 um`), Stokes regime, `Re = 0.02` transition (near 20-30 um for typical densities), and inertial settling (`>=50 um`) |
| `particle_density_kg_m3` | `[500, 3000]` | Typical aerosol/mineral densities plus per-species variation |
| `temperature_k` | `[200, 320]` | Tropospheric range exercising Sutherland viscosity |
| `air_density_kg_m3` | `[0.4, 1.6]` | Sea level to high-altitude range exercising `vis_kin` and the settling denominator |

Out-of-domain, non-finite, non-positive, gas/passive-tracer, or
non-spherical inputs fail closed on the host and never reach device
execution.

## Canonical vectors

`fixtures/settling/canonical-vectors-v1.json` (14 vectors, `SETTLE-001` to
`SETTLE-014`): 0.1-100 um diameters, 800-2500 kg/m3 densities, 230-310 K,
0.6-1.3 kg/m3 air densities, including:

- Cunningham regime (0.1, 0.5 um) and near-zero velocities (absolute
  criterion applies)
- Same-diameter different-density pairs (per-species independence)
- `Re = 0.02` bracket (20 vs 30 um)
- Cold/thin vs warm/dense air-property dependence

## Oracle

`fixtures/settling/oracle-v1.json` holds Fortran-harness velocities plus
`pinned_revision`, `harness_sha256`, `executable_sha256`, `output_sha256`,
and `canonical_sha256`. Regenerate with:

```bash
python scripts/generate_settling_oracle.py
```

## GPU implementation

- WGSL: `src/shaders/settling_velocity.wgsl` (one kernel per physical
  process; group 0 = queries, group 1 = velocities, group 2 = params).
- Host: `src/gpu/settling.rs` (`encode_settling_velocities` for production
  composition, `dispatch_*`/readback only for isolated validation).
- Precision: WGSL `f32` per `docs/GPU_CONTRACT.md`; host comparison in `f64`.
- Comparison policy: absolute `1e-9` m/s (near-zero) OR relative `0.01`
  (1% across the valid domain) via the #91 generic comparator.
- Evidence: `SettlingGpuReport` with per-row `GpuCalculationEvidence`
  (`wgsl_device`, adapter/backend identity, shader/input SHA, pinned oracle
  provenance, numerical verdict). Written by tests to
  `target/settling-gpu-evidence.json`.
- Sign convention: settling is strictly negative (downward, added to
  upward-positive vertical wind). Non-negative values fail row validation.
- Mass: the kernel binds no mass buffer; settling alone never removes mass
  (proven by `test_settling_velocity_alone_does_not_remove_mass`).

## Non-scope

Surface resistance/deposition velocity, ground-boundary handling, mass
removal, and deposition gridding (owned by #36/#37 and later issues). No
production timeloop integration is claimed here.
