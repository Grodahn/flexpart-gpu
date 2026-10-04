# Gravitational Settling (Issue #35)

GPU-executed gravitational settling velocity for spherical aerosol carriers,
validated directly against pinned FLEXPART 11.1. No CPU scientific
implementation participates in acceptance.

## Normative source

Pinned FLEXPART 11.1 revision `c70586c2b7f5258850705325881c61f557ea9bd8`
(see `reference/flexpart-11.1.json`).

| Item | Pinned source | Role |
| --- | --- | --- |
| `get_settling` sphere branch | `src/settling_mod.f90:93-289` (`subroutine get_settling`, `ishape == 0` path) | Iterative Reynolds/settling loop, Clift-Gauvin drag, `settling = -sqrt(4*g*d*rho_p*Cun/(3*Cd*rho_air))`, up to 20 iterations, exit when `\|v-v_old\|/\|v\| < 0.01` |
| `viscosity` | `src/settling_mod.f90:291-303` (`real function viscosity`) | Sutherland dynamic viscosity: `eta0 = 1.827e-5`, `t_0 = 291.15`, `c = 120` |
| `part0` | `src/drydepo_mod.f90` (`subroutine part0`, single diameter bin) | Cunningham slip (`lam = 6.53e-8`, `alpha = 1.257 + 0.4*exp(-1.1/Kn)` with `eps = 1.2e-38` guard, `Cun = 1 + alpha*Kn`) and Stokes init (`myl = 1.81e-5`, `vsh = g*rho_p*d^2*Cun/(18*myl)`) |
| Species init | `src/readoptions_mod.f90` (`dquer` m-to-um conversion, `part0` call, `vsetaver = -vset`, `cunningham` weighting) | `PDIA`/`PDQUER` [m] to `dquer` [um]; single bin (`maxndia = 1`, `par_mod.f90:192`) gives `dmean = d`, `cunningham = cun`, `vsetaver = -vsh` |
| Constants | `src/par_mod.f90:61` | `ga = 9.81` |
| Shape guardrail | `PSHAPE` handling in `src/readoptions_mod.f90` + #126 scope | Only `PSHAPE == 0` spheres; all other shapes fail closed |

Scientific references: Naeslund and Thaning (1991) for the iterative
settling loop; Sutherland (1893) for viscosity; Clift and Gauvin (1971) for
the sphere drag coefficient; Bagheri and Bonadonna (2016) is explicitly
out of scope (non-spherical path rejected).

The driver `oracle/settling_oracle.f90` links to the compiled pristine
`drydepo_mod::part0` and `settling_mod::get_settling` routines. It sets a
constant two-level meteorology column and queries its lower level, so the
local `T`/`rho_air` inputs pass through the real routine unchanged. No
settling equations are duplicated in the oracle. Spatial interpolation
parity remains owned by #87-#90/#76. The driver uses valid `PDSIGMA=2`;
with pinned `maxndia=1`, the geometric bin diameter is mathematically the
species diameter. WGSL evaluates that single-bin identity directly; small
rounding differences from Fortran `exp/log` initialization are covered by
the declared comparison tolerance, rather than a bitwise parity claim.

## Required inputs

Carrier (per-species #10 properties, no shared defaults):

- `PDENSITY` [kg/m3] (`SpeciesConfig::particle_density_kg_m3`)
- `PDIA` [um] (`SpeciesConfig::mean_diameter_um`, converted from [m] per
  `readoptions_mod.f90:2341`)
- `PDSIGMA > 1`, finite, per the #10 carrier validation (one pinned mass bin)
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

`fixtures/settling/canonical-vectors-v1.json` (32 vectors, `SETTLE-001` to
`SETTLE-032`): the interior/regime cases plus all 16 corners of the declared
0.1-100 um, 500-3000 kg/m3, 200-320 K, 0.4-1.6 kg/m3 domain, including:

- Cunningham regime (0.1, 0.5 um) and near-zero velocities (absolute
  criterion applies)
- Same-diameter different-density pairs (per-species independence)
- `Re = 0.02` bracket (21.60 vs 21.62 um initial Reynolds numbers at standard air state)
- Cold/thin vs warm/dense air-property dependence

## Oracle

`fixtures/settling/oracle-v1.json` holds direct pinned-routine velocities plus
`pinned_revision`, normalized driver/build recipe hashes, executable hash,
raw output hash, canonical input hash, source-file/object hashes, Docker image
identity and compiler. `oracle-output-v1.txt` retains the raw Fortran output.
The generator checks the source revision and clean checkout before and after
execution, builds an unmodified Git archive in retained scratch, and retains
the executable and full build/run log under `target/settling-oracle/`.
Regenerate and independently repeat the check with:

```bash
python scripts/generate_settling_oracle.py --oracle-checkout ../flexpart
python scripts/generate_settling_oracle.py --oracle-checkout ../flexpart --check
python scripts/generate_settling_oracle.py --audit
```

## GPU implementation

- WGSL: `src/shaders/settling_velocity.wgsl` (one kernel per physical
  process; group 0 bindings 0/1/2 = queries/velocities/params).
- Host: `src/gpu/settling.rs` (`encode_settling_velocities` for production
  composition, `dispatch_*`/readback only for isolated validation).
- Precision: WGSL `f32` per `docs/GPU_CONTRACT.md`; host comparison in `f64`.
- Comparison policy: absolute `1e-9` m/s (near-zero) OR relative `0.01`
  (1% across the valid domain) via the #91 generic comparator.
- Evidence: `SettlingGpuReport` with per-row `GpuCalculationEvidence`
  (`wgsl_device`, adapter/backend identity, shader/input SHA, pinned oracle
  provenance, numerical verdict). Written by tests to
  `target/settling-gpu-evidence.json`, including honest numerical failure
  reports. Shader identity hashes rendered WGSL with the actual workgroup size.
  Required software CI sets `FLEXPART_GPU_REQUIRE_SETTLING=1`, deletes stale
  evidence, audits frozen inputs/raw outputs, executes all vectors on Lavapipe,
  and verifies the checked-out candidate revision and paired evidence.
- Sign convention: settling is strictly negative (downward, added to
  upward-positive vertical wind). Non-negative values fail row validation.
- Mass: the kernel binds no mass buffer; settling alone never removes mass
  (the mass-invariance test uploads particles and checks their GPU readback
  across a settling dispatch).

## Non-scope

Surface resistance/deposition velocity, ground-boundary handling, mass
removal, and deposition gridding (owned by #36/#37 and later issues). No
production timeloop integration is claimed here.
