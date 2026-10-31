# Deposition meteorology consumer migration (#114)

Provenance marker: **Opus 5.5**.

Authority: FLEXPART 11.1 `c70586c2b7f5258850705325881c61f557ea9bd8`,
flex_extract 7.1.2 `e0005c99ac81d12faa45a8ff799debbd592b0dc0`. Tracker: #77.
The per-field matrix is the [machine-checked inventory](../fixtures/deposition/meteorology-consumer-inventory-v1.json),
locked by [its test](../tests/deposition_meteorology_inventory.rs).

## Result

**No current production deposition kernel samples meteorology, so there is
nothing to migrate.** The [dry kernel](../src/shaders/dry_deposition.wgsl) and
the [wet kernel](../src/shaders/wet_deposition.wgsl) read only:

| Kernel | Binding | Class | Owner of its derivation |
| --- | --- | --- | --- |
| dry | `particles` (`pos_z` m AGL against `2*href`, `mass[s]`) | particle state | existing particle ABI |
| dry | `deposition_velocity_m_s` (`vdep`, m/s per species) | external process coefficient | #36 (resistances), #37 (integration) |
| dry | `params` (`dt`, `href=15 m`) | run configuration | existing config |
| wet | `particles` (`mass[s]`) | particle state | existing particle ABI |
| wet | `scavenging_coefficient_s_inv` (`wetscav`, 1/s per species) | external process coefficient | #33 (rates), #34 (integration) |
| wet | `precipitating_fraction` (`grfraction`, 1) | external process coefficient | #33 |

The coefficients come from `ForwardStepForcing`. The host materializes them
and uploads them explicitly; corpus cases use constants. No interpolation or
sampling of meteorology happens on this path. So there is no competing legacy
deposition meteorology path to retire. The CPU modules
[`physics::deposition`](../src/physics/deposition.rs) and
[`physics::wet_scavenging`](../src/physics/wet_scavenging.rs) are
diagnostic only and are not reachable from either production driver.

Both kernels already run inside the canonical #76/#171 production step. Since
#173/#112, `run_timestep` rejects missing canonical meteorology before any
release or upload. Deposition then mutates only the staged particle copy, which
`ResidentAdvectionStep::encode_commit` publishes only when the shared resident
status is valid. Probabilities are reported only after `require_success`.

Deriving `vdep`, `wetscav` or `grfraction` from canonical meteorology would
mean implementing `getvdep`/`get_wetscav` on the GPU. That is new deposition
physics, explicitly outside #114.

## Inventory summary

FLEXPART field order and timing matter for the future owners:

- **Dry (`getvdep`)**: computed **on the grid at each source time**
  (`getfields_mod.f90:1052-1073`, `drydepo_mod.f90:867-1051`). Inputs: `ustar`,
  `tt2`, `ps`, `1/oli`, `ssr`, surface `rh` derived from `td2`, the source-memory
  `lsprec+convprec` member sum, snow depth, land-use fractions and a season
  derived from `bdate+wftime(n)` and latitude. The derived `vdep` is then
  interpolated bilinearly and linearly in time to the particle
  (`drydepo_mod.f90:827-865`). Raw meteorology is never sampled at the particle.
  The precipitation input is the source-time member, not the #119 particle-time
  rate. Every input row is `BLOCKED` on #36; `DRY-SEASON` also lacks a
  canonical representation.
- **Wet (`get_wetscav` via `interpol_rain`)**: per particle at `itime`
  (`wetdepo_mod.f90:204-433`, `interpol_mod.f90:1209-1582`). Inputs: LSP and CP
  through the `dt/3` path, total cloud cover, cloud water and temperature at
  level `kz` through ordinary `/dt` blends, and `icmv`-masked cloud
  bottom/top. LSP/CP are `BLOCKED` on #227 (particle scale) and #33 (consumer).
  Cloud fields, the level-index temperature and cloud bounds are `BLOCKED` on
  #33. `numpf=3`/`RRINT=1` are `BLOCKED` on #212 and rejected fail-closed.
- **`NOT_APPLICABLE`**: 10 m winds and model-level humidity/pressure are not
  consumed by pinned deposition. Species parameters are not meteorology
  (#33/#35/#36).

No row is `MIGRATED`. The lock test rejects a `MIGRATED` row, a meteorology
kernel binding, an unlinked `BLOCKED` row and unknown canonical field names.
Anyone who adds a real consumer must replace that guard with production evidence.

## #119 particle-scale assessment

`prepare_precipitation_time_samples` is a bounded validation API:

- Per query it allocates four full-grid #87 planes plus query, uniform and
  parameter buffers, and encodes 4 copies, 4 #87 dispatches and 1 #119 dispatch.
- Memory is `queries x 4 x cell_count x 4 B`. For 10^6 particles on a
  720x360 grid that is about 4.1 TB.
- `precipitation_temporal.wgsl` is `@workgroup_size(1)`, reads element `[0]`
  of four one-sample buffers and takes its time weights and `output_pair` from a
  per-query uniform. One dispatch cannot evaluate N lanes.
- #87 query buffers are built from host `(x, y)` pairs. #171 resident sampling
  rejects accumulated fields and cannot address #207 member planes.

A pair-shared, resident, lane-parallel path is feasible in principle: all
forward lanes share one `itime`, so one memory pair per step. But it needs a
new WGSL kernel and a resident #207 adapter. Under the #114 stop rule that is
split out as [#227](https://github.com/Grodahn/flexpart-gpu/issues/227),
with measurable memory/dispatch bounds, bitwise identity to the existing
batch API, and dual-adapter acceptance. The validated #119 batch API is
unchanged.

## Production evidence

`cargo test --lib deposition_production_tests -- --nocapture --test-threads=1`
runs the [production regression](../src/gpu/meteorology/deposition_production_tests.rs)
through the real forward and backward drivers with #76/#171 canonical
meteorology. Setup: capacity 130, active prefix 67, particles at 5 m AGL,
per-slot `vdep`, uniform `wetscav=0.1 s-1`, `grfraction=0.5`. It checks:

- missing canonical meteorology and short (capacity-mismatched) coefficients
  are rejected;
- valid steps update mass and probability with the unchanged kernel formulas
  (existing kernel-module bound, relative `1e-6`) and leave the inactive tail
  byte-identical;
- an independently failing canonical query (`TEST_QUERY_FAULT`) with valid peer
  lanes publishes no deposited mass (all particle bytes preserved) and no
  probabilities.

Run it once per adapter (set `FLEXPART_GPU_SOFTWARE=1` for software WGSL).
Required software CI (`gpu-transport-physics`, step *Prove canonical deposition
consumer boundary (#114)*) runs the inventory lock and this regression with
`FLEXPART_GPU_VALIDATION`/`FLEXPART_GPU_COMPACTION` set to `0/0`, `1/0` and `0/1`.
Each rejection is checked by error kind: the missing-owner error, the
dry-coefficient length error and a canonical meteorology step error.
Each run writes `target/ci-gate/deposition-meteorology/report-<adapter_class>.json`
with the revision, adapter, shader SHA-256, source snapshot hashes and sampled lanes.

This is production-path integration evidence for the unchanged deposition
boundary. It is not wet- or dry-deposition parity, and not evidence that any
meteorological field was migrated.

## Status

#114 stays open. Its claim is met vacuously for the current kernels. But #119
assigned the pair-shared particle-scale precipitation query to #114, and that
is now blocked on #227. Process physics remains with #33/#34 (wet) and
#35/#36/#37 (settling/dry).
