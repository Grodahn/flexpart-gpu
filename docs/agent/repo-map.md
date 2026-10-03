# Repository navigation

Authoritative task-to-path map for issue #123. Start at [root instructions](../../AGENTS.md),
select one domain below, then open its [verification entry](test-map.md).
Paths describe this checkout, not completion of open migration issues.
Scientific authority stays with the linked contracts and the task's owning issue.

Read only the selected row's implementation, applicable contract, and focused test
first. Inspect dependencies only when the task crosses their named handoff.
Use a scoped `rg -n <symbol> <listed-file>` if a symbol is missing; widen to the
listed dependency before searching a directory. Do not inventory the repository
for a task already covered here. Update this map in the same change when a mapped
path moves. Markdown links are the machine-readable path surface, audited by
[the navigation checker](../../scripts/check_agent_navigation.py).

## GPU runtime

- Production: [GpuContext](../../src/gpu/mod.rs), [adapter selection](../../src/gpu/adapter.rs).
- Resources/dispatch: [buffers and downloads](../../src/gpu/buffers.rs), [workgroups](../../src/gpu/workgroup.rs); [smoke shader](../../src/shaders/gpu_contract_smoke.wgsl).
- Authority: [GPU contract](../GPU_CONTRACT.md), [pipeline map](../GPU_PIPELINE.md).
- Oracle/fixtures: [pin](../../reference/flexpart-11.1.json); smoke uses analytical integer inputs in [preflight](../../src/gpu/preflight.rs).
- Tests: [preflight tests](../../src/gpu/preflight.rs), [evidence tests](../../src/gpu/evidence.rs); [verification](test-map.md#gpu-runtime).
- Dependencies: all GPU domains use this owner; composition/submission belongs to [simulation](#simulation).

## Meteorology

- Canonical host boundary: [Snapshot/metadata](../../src/meteorology/mod.rs); provider ingestion [GRIB](../../src/io/grib2.rs), [NetCDF](../../src/io/netcdf.rs); [runtime geometry](../../src/meteorology/vertical.rs), [vertical transform](../../src/io/vertical_transform.rs).
- GPU host + shader pairs: [horizontal](../../src/gpu/horizontal.rs) / [kernel](../../src/shaders/horizontal_interpolation.wgsl); [vertical](../../src/gpu/vertical.rs) / [sample](../../src/shaders/vertical_sample.wgsl), [W remap](../../src/shaders/vertical_remap_w.wgsl); [instantaneous time](../../src/gpu/temporal.rs) / [kernel](../../src/shaders/temporal_interpolation.wgsl); [accumulated intervals](../../src/gpu/accumulation.rs) / [kernel](../../src/shaders/accumulated_interval.wgsl).
- Authority: [schema](../meteorology-contract.md), [geometry](../vertical-transform.md), [spatial sampling](../interpolation-contract.md), [time](../temporal-interpolation.md), [interval/reset](../accumulation-contract.md), [GPU](../GPU_CONTRACT.md).
- Oracle/fixtures: [interpolation](../../fixtures/interpolation/contract-v1.json), [provenance](../../fixtures/interpolation/contract-v1.provenance.json), [production W](../../fixtures/interpolation/w-production-oracle-v1.json), [temporal](../../fixtures/temporal/oracle-temporal-bilinear-scenario.json), [accumulation](../../fixtures/accumulation/contract-v1.json), [vertical columns](../../fixtures/vertical/).
- Tests: [schema](../../tests/meteorology_contract.rs), [horizontal](../../tests/horizontal_gpu.rs), [vertical](../../tests/vertical_gpu.rs), [time](../../tests/temporal_gpu.rs), [accumulation](../../tests/accumulation_gpu.rs); [verification](test-map.md#meteorology).
- Dependencies: providers -> canonical snapshot/geometry -> GPU sampling -> [simulation](#simulation). #76 owns canonical composition and #77 consumer migration; current [wind interface](../../src/wind/mod.rs) remains in the time loop. These kernel surfaces alone do not establish production adoption.

## Simulation

- Production owner: [forward/backward drivers and forcing](../../src/simulation/timeloop.rs); [release scheduling](../../src/release/mod.rs), [configuration](../../src/config/mod.rs), [particle state](../../src/particles/mod.rs).
- GPU: stage encoders in the rows below; [compaction](../../src/gpu/compaction.rs) / [kernel](../../src/shaders/compaction.wgsl).
- Authority: [GPU contract](../GPU_CONTRACT.md), [pipeline](../GPU_PIPELINE.md), [simulation flow](../science/simulation-flow.md).
- Fixtures: [canonical corpus cases](../../fixtures/corpus/cases/), [candidate physics identity](../../reference/candidate-physics/candidate-forward-timeloop-v1.json).
- Tests: [forward](../../tests/forward_timeloop.rs), [backward](../../tests/backward_timeloop.rs), [production physics](../../tests/integration/physics_validation.rs); [verification](test-map.md#simulation).
- Dependencies: meteorology/release -> transport/PBL/deposition/decay -> output. Preserve explicit host output boundaries; do not introduce intermediate readback to connect device stages.

## Transport-advection

- Production: [time loop](../../src/simulation/timeloop.rs), [coordinate/velocity units](../../src/coords/mod.rs).
- GPU: [advection dispatch](../../src/gpu/advection.rs), [particle step/reflection](../../src/gpu/particle_step.rs); [buffer](../../src/shaders/advection.wgsl), [dual bracket](../../src/shaders/advection_dual_wind.wgsl), [texture](../../src/shaders/advection_texture.wgsl), [dual texture](../../src/shaders/advection_texture_dual_wind.wgsl), [particle step](../../src/shaders/particle_step.wgsl).
- Authority: [advection science](../science/advection.md), [GPU contract](../GPU_CONTRACT.md); [CPU diagnostic](../../src/physics/advection.rs) is not the production GPU proof.
- Fixtures: [ADV-ANA-001](../../fixtures/corpus/cases/ADV-ANA-001.json), [Fortran inputs](../../fixtures/corpus/fortran/ADV-ANA-001/).
- Tests: [software device displacement](../../tests/integration/software_advection.rs), [forward driver](../../tests/forward_timeloop.rs); [verification](test-map.md#transport-advection).
- Dependencies: meteorology/geometry and particle buffers -> advection -> PBL/turbulence; submission owned by simulation.

## PBL-turbulence

- Production: [time loop](../../src/simulation/timeloop.rs); host inputs [PBL preparation](../../src/io/pbl_params.rs), [PBL state](../../src/pbl/mod.rs).
- GPU: [PBL diagnostics](../../src/gpu/pbl.rs) / [kernel](../../src/shaders/pbl_diagnostics.wgsl); [Hanna](../../src/gpu/hanna.rs) / [kernel](../../src/shaders/hanna_params.wgsl); [fused Langevin](../../src/gpu/langevin_fused.rs) / [kernel](../../src/shaders/langevin_fused.wgsl); [split Langevin](../../src/gpu/langevin.rs) / [kernel](../../src/shaders/langevin.wgsl); [reflection](../../src/gpu/pbl_reflection.rs) / [kernel](../../src/shaders/pbl_reflection.wgsl).
- Authority: [diffusion science](../science/turbulent-diffusion.md), [known limitations](../science/known-limitations.md), [GPU contract](../GPU_CONTRACT.md).
- Fixtures: [stable](../../fixtures/corpus/cases/PBL-STABLE-004.json), [neutral](../../fixtures/corpus/cases/PBL-NEUTRAL-005.json), [unstable](../../fixtures/corpus/cases/PBL-UNSTABLE-006.json).
- Tests: [production physics](../../tests/integration/physics_validation.rs), [dispersion](../../tests/integration/horizontal_dispersion.rs); [verification](test-map.md#pbl-turbulence).
- Dependencies: surface meteorology -> PBL -> Hanna/Langevin -> reflected particles. [Hanna CPU diagnostics](../../src/physics/hanna.rs), [CBL standalone](../../src/gpu/cbl.rs) / [kernel](../../src/shaders/cbl.wgsl), [RNG](../../src/gpu/rng.rs) / [kernel](../../src/shaders/philox_rng.wgsl) are adjacent only for their owned tests; CPU override is not GPU evidence.

## Convection

- Current boundary: [simplified host chain](../../src/physics/convection.rs) -> [standalone GPU mixing](../../src/gpu/convection.rs) / [kernel](../../src/shaders/convective_mixing.wgsl). No canonical production time-loop convection composition exists in this checkout.
- Authority: [known limitations](../science/known-limitations.md), [GPU standalone/composition contract](../GPU_CONTRACT.md); full scientific scope belongs to the owning convection issue.
- Fixtures/tests: analytical matrices constructed in [convection chain tests](../../tests/convection_chain.rs); [verification](test-map.md#convection). No pinned full-convection parity fixture is declared here.
- Dependencies: meteorology/PBL -> convection chain -> particle mixing; #115 owns canonical consumer migration. Do not infer production integration from the standalone helper.

## Wet-deposition

- Production: [forcing and composed step](../../src/simulation/timeloop.rs), [species inputs](../../src/physics/species.rs).
- GPU: [wet deposition](../../src/gpu/wet_deposition.rs) / [kernel](../../src/shaders/wet_deposition.wgsl).
- Authority: [deposition science](../science/deposition.md), [interval precipitation](../accumulation-contract.md), [GPU contract](../GPU_CONTRACT.md).
- Fixtures: [WET-008](../../fixtures/corpus/cases/WET-008.json), [Fortran inputs](../../fixtures/corpus/fortran/WET-008/), [wet species identity](../../reference/species-physics/species-040-wet-aerosol-v1.json).
- Tests: [GPU module](../../src/gpu/wet_deposition.rs), [mass evolution](../../tests/integration/deposition_decay.rs); [verification](test-map.md#wet-deposition).
- Dependencies: meteorology/rates + species -> scavenging -> mass accounting/output; [CPU scavenging diagnostics](../../src/physics/wet_scavenging.rs) and canonical consumer migration (#114) are separate boundaries.

## Dry-deposition-settling

- Production: [dry forcing and step](../../src/simulation/timeloop.rs); [species mapping](../../src/physics/species.rs).
- GPU: [dry probability/mass update](../../src/gpu/deposition.rs) / [kernel](../../src/shaders/dry_deposition.wgsl).
- Authority: [deposition science](../science/deposition.md), [limitations](../science/known-limitations.md), [GPU contract](../GPU_CONTRACT.md).
- Fixtures: [DRY-007](../../fixtures/corpus/cases/DRY-007.json), [Fortran inputs](../../fixtures/corpus/fortran/DRY-007/), [constant dry identity](../../reference/species-physics/species-040-dry-constant-v1.json).
- Tests: [GPU module](../../src/gpu/deposition.rs), [mass evolution](../../tests/integration/deposition_decay.rs); [verification](test-map.md#dry-deposition-settling).
- Dependencies: surface/PBL + species -> dry deposition -> mass/output. [CPU resistance/bin utilities](../../src/physics/deposition.rs) accept settling velocity as an input; this main checkout has no standalone GPU settling-velocity calculation. #35 owns that calculation and its oracle; do not treat a supplied velocity or constant dry fixture as its proof.

## Decay-mass-ledger

- Production: [decay scheduling and step reports](../../src/simulation/timeloop.rs), [particle species masses](../../src/particles/mod.rs), [species decay constants](../../src/physics/species.rs). Accounting spans these surfaces; no separate mass-ledger module exists.
- GPU: [decay](../../src/gpu/decay.rs) / [kernel](../../src/shaders/decay.wgsl); deposition removal uses the two preceding GPU rows.
- Authority: [species configuration](../species-config.md), [GPU contract](../GPU_CONTRACT.md), issue-owned conservation tolerances.
- Fixtures: [inert species](../../reference/species-physics/species-024-inert-v1.json); analytical per-species decay inputs in [module tests](../../src/gpu/decay.rs).
- Tests: [mass conservation](../../tests/integration/mass_conservation.rs), [species/nuclides](../../tests/integration/species_nuclide.rs), [deposition accounting](../../tests/integration/scientific_invariants.rs); [verification](test-map.md#decay-mass-ledger).
- Dependencies: release inventory -> transport/deposition/decay -> remaining/removed mass -> output. Kernel decay agreement is distinct from production budget closure.

## Gridding-output

- Production boundary: [time-loop output](../../src/simulation/timeloop.rs), [NetCDF writer](../../src/io/netcdf_output.rs).
- GPU: [concentration gridding](../../src/gpu/gridding.rs) / [kernel](../../src/shaders/concentration_gridding.wgsl); explicit D2H for host output.
- Authority: [gridding science](../science/concentration-gridding.md), [case output-grid contract](../../schemas/validation-case-v2.schema.json), [GPU contract](../GPU_CONTRACT.md).
- Fixtures: [corpus output-grid manifests](../../fixtures/corpus/cases/), [ETEX mini](../../fixtures/etex/mini/README.md).
- Tests: [gridding module](../../src/gpu/gridding.rs), [scientific invariants](../../tests/integration/scientific_invariants.rs); [verification](test-map.md#gridding-output).
- Dependencies: resident particles/mass -> aggregation -> host file -> validation decoding. Meteorology and concentration grids are distinct.

## Validation-provenance

- Canonical tooling: [case contract](../../src/validation/case.rs), [candidate physics identity](../../src/validation/candidate_physics.rs), [input-equivalence verdicts](../../src/validation/input_equivalence.rs), [corpus CLI](../../src/bin/corpus-run.rs).
- GPU evidence: [typed schema/comparator](../../src/gpu/evidence.rs); no validation-specific shader/runtime.
- Authority: [compact workflow](../agent-validation.md), [corpus matrix](../corpus-matrix.md), [evaluation](../evaluation.md), [provenance](../run-provenance.md), [stochastic identity](../oracle-stochastic-identity.md), [CI gates](../ci-gates.md).
- Oracle/fixtures: [pinned revision](../../reference/flexpart-11.1.json), [corpus index](../../fixtures/corpus/corpus.json), [case schema](../../schemas/validation-case-v2.schema.json), [manifest schema](../../schemas/run-manifest-v1.schema.json).
- Consumers: [focused runner](../../scripts/agent_validation.py), [corpus orchestration](../../scripts/run-corpus.sh), [input audit](../../scripts/corpus/audit_corpus_inputs.py), [comparison](../../scripts/corpus/compare_corpus.py), [provenance writer](../../scripts/corpus/write_corpus_manifest.py), [evaluation CLI](../../scripts/evaluate/evaluate_case.py).
- Tests: [runner](../../scripts/test_agent_validation.py), [input audit](../../scripts/corpus/test_audit_corpus_inputs.py), [manifest](../../scripts/corpus/test_write_corpus_manifest_v1.py); [verification](test-map.md#validation-provenance).
- Dependencies: normalized cases/inputs -> candidate + pinned oracle -> decoded outputs -> comparison/provenance. Input equivalence, execution, and scientific verdicts remain separate contracts.

## Navigation maintenance

Use [dry runs](navigation-dry-runs.md) as examples of selecting a bounded working
set. Check all navigation surfaces with `python scripts/check_agent_navigation.py`.
[CI](../../.github/workflows/agent-navigation.yml) runs that checker and its
[negative regression tests](../../scripts/test_check_agent_navigation.py).
This checks path/anchor and command-target drift, not scientific semantics.
