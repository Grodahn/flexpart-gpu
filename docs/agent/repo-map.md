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

- Stable canonical host API: [meteorology facade](../../src/meteorology/mod.rs); provider ingestion [GRIB](../../src/io/grib2.rs), [NetCDF](../../src/io/netcdf.rs); [stable runtime-geometry facade](../../src/meteorology/vertical.rs) and [geometry owners](#vertical-geometry), [provider vertical transform](../../src/io/vertical_transform.rs).
- Private source contract owners (read only the matching responsibility): [schema identity/errors](../../src/meteorology/schema.rs), [field matrix/layout/requirements](../../src/meteorology/field.rs), [canonical horizontal grid](../../src/meteorology/grid.rs), [native vertical coordinates](../../src/meteorology/coordinate.rs), [source time metadata](../../src/meteorology/time.rs), [snapshot validation order/provenance](../../src/meteorology/snapshot.rs). [Pre-move inventory/context comparison](meteorology-decomposition.md).
- GPU host + shader pairs: [horizontal](../../src/gpu/horizontal.rs) / [kernel](../../src/shaders/horizontal_interpolation.wgsl); [vertical](../../src/gpu/vertical.rs) / [sample](../../src/shaders/vertical_sample.wgsl), [W remap](../../src/shaders/vertical_remap_w.wgsl); [instantaneous time](../../src/gpu/temporal.rs) / [kernel](../../src/shaders/temporal_interpolation.wgsl); [accumulated intervals](../../src/gpu/accumulation.rs) / [kernel](../../src/shaders/accumulated_interval.wgsl).
- Shared host validation/semantics: [horizontal](../../src/meteorology/horizontal.rs), [vertical sampling](../../src/meteorology/vertical_sampling.rs), [time bracket resolution](../../src/meteorology/temporal.rs), [interval metadata](../../src/meteorology/accumulation.rs). Read only the matching module alongside its GPU pair; CPU sampling remains diagnostic.
- Authority: [schema](../meteorology-contract.md), [geometry](../vertical-transform.md), [spatial sampling](../interpolation-contract.md), [time](../temporal-interpolation.md), [interval/reset](../accumulation-contract.md), [GPU](../GPU_CONTRACT.md).
- Oracle/fixtures: [interpolation](../../fixtures/interpolation/contract-v1.json), [provenance](../../fixtures/interpolation/contract-v1.provenance.json), [production W](../../fixtures/interpolation/w-production-oracle-v1.json), [temporal](../../fixtures/temporal/oracle-temporal-bilinear-scenario.json), [accumulation](../../fixtures/accumulation/contract-v1.json), [vertical columns](../../fixtures/vertical/).
- Tests: [schema](../../tests/meteorology_contract.rs), [horizontal](../../tests/horizontal_gpu.rs), [vertical](../../tests/vertical_gpu.rs), [time](../../tests/temporal_gpu.rs), [accumulation](../../tests/accumulation_gpu.rs); [verification](test-map.md#meteorology).
- Nonuniform-column research: [#118 decision/source trace](../research/shared-height-118.md), [direct oracle launcher](../../scripts/interpolation/run_shared_height_oracle.py), [frozen evidence](../../fixtures/interpolation/shared-height-v1/report.json). No production remapping is implemented by this research.
- Dependencies: providers -> canonical snapshot/geometry -> GPU sampling -> [canonical GPU composition](../../src/gpu/meteorology.rs) -> [simulation](#simulation). #76 owns canonical composition and #77 consumer migration; current [wind interface](../../src/wind/mod.rs) remains in the time loop. These kernel surfaces alone do not establish production adoption.

## GPU vertical

- Resident-query composition: [typed query/status and sampled batches](../../src/gpu/meteorology/resident.rs), [particle producer](../../src/shaders/particle_query.wgsl), [horizontal adapter](../../src/shaders/resident_query_adapter.wgsl), [ABI and ownership](../resident-meteorology.md). #171 keeps #87/#88/#89 authoritative; #112–#115 own production adoption. [Focused verification](test-map.md#resident-meteorology).

- Stable #88 GPU API: [vertical facade](../../src/gpu/vertical.rs). Existing `gpu` re-exports retain this boundary.
- Select only the relevant execution owner: [persistent buffers/uploads](../../src/gpu/vertical/resources.rs), [runtime columns/AGL preparation](../../src/gpu/vertical/preparation.rs), [pipeline/layout construction](../../src/gpu/vertical/pipeline.rs), [caller-owned encoding/bind groups](../../src/gpu/vertical/encode.rs), [standalone dispatch and explicit D2H](../../src/gpu/vertical/dispatch.rs), [scoped GPU errors](../../src/gpu/vertical/error.rs).
- WGSL: [model/center-W sample](../../src/shaders/vertical_sample.wgsl), [interface-W remap](../../src/shaders/vertical_remap_w.wgsl). Interface W encodes remap before sample with device-resident shared values.
- Evidence surfaces: [candidate/oracle identities and input/shader hashes](../../src/gpu/vertical/provenance.rs), [paired rows/reports and fail-closed validation](../../src/gpu/vertical/evidence.rs); generated per-scenario reports remain `target/ci-gate/vertical-gpu/*.json`.
- Authority: [GPU contract](../GPU_CONTRACT.md), [spatial contract](../interpolation-contract.md), [pre-move inventory/boundaries/context comparison](gpu-vertical-decomposition.md). #128 changes structure only; #88 owns scientific/device evidence.
- Fixtures: [model-level oracle](../../fixtures/interpolation/contract-v1.json), [per-case provenance](../../fixtures/interpolation/contract-v1.provenance.json), [W production oracle](../../fixtures/interpolation/w-production-oracle-v1.json).
- Tests: [focused vertical/device/oracle tests](../../tests/vertical_gpu.rs), [host-only checks](../../src/gpu/vertical/tests.rs); [verification](test-map.md#gpu-vertical).
- Named handoffs: [canonical runtime geometry](../../src/meteorology/vertical.rs) and [CPU diagnostics](../../src/meteorology/vertical_sampling.rs) for input semantics; [GPU composition consumer](../../src/gpu/meteorology.rs) only for caller-owned sequencing. #76/#77 adoption and #117 cleanup stay separate.

## Vertical geometry

- Stable canonical #30 boundary: [vertical facade](../../src/meteorology/vertical.rs); [opaque state, runtime views and validity](../../src/meteorology/vertical/runtime.rs). All callers retain this facade; no second geometry model exists.
- Select only the relevant owner: [hybrid pressure/local surface anchoring](../../src/meteorology/vertical/pressure.rs), [model-level thermodynamics/heights](../../src/meteorology/vertical/model_levels.rs), [W/interface heights and pinmconv](../../src/meteorology/vertical/interfaces.rs), [motion normalization](../../src/meteorology/vertical/motion.rs), [terrain and release AGL/ASL](../../src/meteorology/vertical/terrain.rs), [validation-only eta-dot](../../src/meteorology/vertical/eta_dot.rs).
- Named handoffs: [X-fastest addressing/traversal/field lookup](../../src/meteorology/vertical/layout.rs), [provenance](../../src/meteorology/vertical/provenance.rs), [errors](../../src/meteorology/vertical/error.rs). Read these only when crossing that boundary.
- Authority: [#30 geometry](../vertical-transform.md), [canonical schema](../meteorology-contract.md), [#80 W sampling semantics](../interpolation-contract.md); [pre-move inventory and context comparison](vertical-geometry-decomposition.md).
- Fixtures: [vertical columns](../../fixtures/vertical/), [frozen W evidence](../../fixtures/interpolation/w-production-oracle-v1.json), [pre-move facade identities](../../tests/fixtures/vertical-geometry-identity-v1.json).
- Tests: responsibility regressions in [vertical tests](../../src/meteorology/vertical/tests/), shared [synthetic builders](../../src/meteorology/vertical/test_support.rs) only when needed; [facade identity](../../tests/vertical_geometry_contract.rs), [runtime integration](../../tests/integration/vertical_runtime.rs); [focused verification](test-map.md#vertical-geometry).
- Dependencies: Snapshot -> canonical geometry -> [vertical sampling](../../src/meteorology/vertical_sampling.rs) / [GPU vertical execution](../../src/gpu/vertical.rs). #127 decomposes host geometry only; #128/#88 execution, provider decoding, sampling science and #117 cleanup remain separate.

## Configuration

- Stable caller facade: [configuration exports](../../src/config/mod.rs).
- Domain owners (types, file aliases/defaults, validation and colocated tests): [COMMAND run timing](../../src/config/command.rs), [RELEASES geometry/inventory](../../src/config/release.rs), [OUTGRID bounds/spacing](../../src/config/output.rs), [SPECIES units/sentinels/process activation](../../src/config/species.rs), [aggregate loading/validation order](../../src/config/simulation.rs).
- Shared handoffs: [file grammar, scalar/timestamp parsing and version policy](../../src/config/parsing.rs), [typed errors](../../src/config/error.rs); [temporary test files](../../src/config/test_support.rs) are test-only.
- Authority: [pre-move field/default/alias inventory and context comparison](config-decomposition.md), [species compatibility](../species-config.md). #129 preserves the existing schema and behavior; it adds no settings or scientific semantics.
- Tests: [public facade, frozen serialization/default/parser behavior](../../tests/config_contract.rs), [pre-move transcript](../../tests/fixtures/config-behavior-v1.json); [verification](test-map.md#configuration).
- Dependencies: file inputs -> config -> [release scheduling](../../src/release/mod.rs), [species mapping](../../src/physics/species.rs), [validation loading](../../src/validation/mod.rs). GPU/runtime and meteorology provider configuration belong to their existing owners; unknown COMMAND keys remain raw assignments.

## Simulation

- Canonical input preparation (#173): shared [typed bracket and source slot](../../src/simulation/timeloop/meteorology.rs), actual forward/backward `prepare_canonical_meteorology` entries in the preparation owners below, and [lifetime/#112 handoff](../canonical-timeloop-meteorology.md). This prepares existing #76/#171 owners consumed by the [canonical production advection operators](../resident-advection.md).
- Stable production entry: [simulation-driver facade](../../src/simulation/timeloop.rs); [forward state/lifecycle](../../src/simulation/timeloop/forward.rs), [timestep phases/advance](../../src/simulation/timeloop/forward/timestep.rs), [ordered GPU operators/submission](../../src/simulation/timeloop/forward/operators.rs), [backward attribution](../../src/simulation/timeloop/backward.rs).
- Preparation: [bracket handoff](../../src/simulation/timeloop/meteorology.rs), [prefetch/bracket/PBL inputs](../../src/simulation/timeloop/forward/meteorology.rs), [forcing shapes/cache](../../src/simulation/timeloop/forcing.rs), [forcing validation/uploads](../../src/simulation/timeloop/forward/forcing.rs).
- Host boundaries: [particle sync/sort](../../src/simulation/timeloop/forward/particles.rs), [explicit gridding](../../src/simulation/timeloop/forward/output.rs), [reports](../../src/simulation/timeloop/reports.rs), [errors](../../src/simulation/timeloop/error.rs); [configuration/validation](../../src/simulation/timeloop/config.rs), [time/bracket bounds](../../src/simulation/timeloop/time.rs), [runtime options](../../src/simulation/timeloop/options.rs).
- Inputs: [release scheduling](../../src/release/mod.rs), [stable config facade](../../src/config/mod.rs) / [configuration domains](#configuration), [particle state](../../src/particles/mod.rs).
- GPU: separated Hanna -> Langevin uses the crate-visible `gpu::encode_update_particles_turbulence_langevin_gpu_with_hanna_output_and_kernel` [facade](../../src/gpu/mod.rs), owned by [Langevin](../../src/gpu/langevin.rs); stage encoders in the rows below; [compaction](../../src/gpu/compaction.rs) / [kernel](../../src/shaders/compaction.wgsl).
- Authority: [GPU contract](../GPU_CONTRACT.md), [pipeline](../GPU_PIPELINE.md), [simulation flow](../science/simulation-flow.md), [pre-move inventory and preserved ordering](timeloop-decomposition.md).
- Fixtures: [canonical corpus cases](../../fixtures/corpus/cases/), [candidate physics identity](../../reference/candidate-physics/candidate-forward-timeloop-v1.json).
- Tests: [forward](../../tests/forward_timeloop.rs), [backward](../../tests/backward_timeloop.rs), [production physics](../../tests/integration/physics_validation.rs); [verification](test-map.md#simulation).
- Dependencies: meteorology/release -> transport/PBL/deposition/decay -> output. Preserve explicit host output boundaries; do not introduce intermediate readback to connect device stages.

## Transport-advection

- Production: [forward operator sequence](../../src/simulation/timeloop/forward/operators.rs), [coordinate/velocity units](../../src/coords/mod.rs).
- Canonical production: [resident Petterssen](../../src/gpu/advection_resident.rs); [migration inventory](../resident-advection.md).
- GPU diagnostics: [advection dispatch](../../src/gpu/advection.rs), [particle step/reflection](../../src/gpu/particle_step.rs); [buffer](../../src/shaders/advection.wgsl), [dual bracket](../../src/shaders/advection_dual_wind.wgsl), [texture](../../src/shaders/advection_texture.wgsl), [dual texture](../../src/shaders/advection_texture_dual_wind.wgsl), [particle step](../../src/shaders/particle_step.wgsl).
- Authority: [advection science](../science/advection.md), [GPU contract](../GPU_CONTRACT.md); [CPU diagnostic](../../src/physics/advection.rs) is not the production GPU proof.
- Fixtures: [ADV-ANA-001](../../fixtures/corpus/cases/ADV-ANA-001.json), [Fortran inputs](../../fixtures/corpus/fortran/ADV-ANA-001/).
- Tests: [software device displacement](../../tests/integration/software_advection.rs), [forward driver](../../tests/forward_timeloop.rs); [verification](test-map.md#transport-advection).
- Dependencies: meteorology/geometry and particle buffers -> advection -> PBL/turbulence; submission owned by simulation.

## PBL-turbulence

- Shared diagnostic options: canonical `pbl::PblComputationOptions` at the [PBL facade](../../src/pbl/mod.rs), backed by a [private options owner](../../src/pbl/options.rs). GPU encoding, [driver configuration](../../src/simulation/timeloop/config.rs) and [candidate profile conversion](../../src/validation/candidate_physics.rs) consume this facade; options inspection does not require CPU preparation. [Ownership and working-set comparison](pbl-options-ownership.md).
- Production: [forward operator sequence](../../src/simulation/timeloop/forward/operators.rs) and [backward separated path](../../src/simulation/timeloop/backward.rs) consume `&HannaParamsOutputBuffer` through the crate-visible `gpu::encode_update_particles_turbulence_langevin_gpu_with_hanna_output_and_kernel` [facade](../../src/gpu/mod.rs), owned by [split Langevin](../../src/gpu/langevin.rs) ([handoff and working-set audit](hanna-langevin-handoff.md)); [PBL state](../../src/pbl/mod.rs). CPU diagnostic/preparation routines remain separately owned by [IO PBL preparation](../../src/io/pbl_params.rs), which retains `io::pbl_params::PblComputationOptions`; [IO facade](../../src/io/mod.rs) retains `io::PblComputationOptions`. Both re-export the identical domain-owned type.
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

- Production: [forcing uploads](../../src/simulation/timeloop/forward/forcing.rs) and [composed step](../../src/simulation/timeloop/forward/operators.rs), [species inputs](../../src/physics/species.rs).
- GPU: [wet deposition](../../src/gpu/wet_deposition.rs) / [kernel](../../src/shaders/wet_deposition.wgsl).
- Authority: [deposition science](../science/deposition.md), [interval precipitation](../accumulation-contract.md), [GPU contract](../GPU_CONTRACT.md).
- Fixtures: [WET-008](../../fixtures/corpus/cases/WET-008.json), [Fortran inputs](../../fixtures/corpus/fortran/WET-008/), [wet species identity](../../reference/species-physics/species-040-wet-aerosol-v1.json).
- Tests: [GPU module](../../src/gpu/wet_deposition.rs), [mass evolution](../../tests/integration/deposition_decay.rs); [verification](test-map.md#wet-deposition).
- Dependencies: meteorology/rates + species -> scavenging -> mass accounting/output; [CPU scavenging diagnostics](../../src/physics/wet_scavenging.rs) and canonical consumer migration (#114) are separate boundaries.

## Dry-deposition-settling

- Production: [dry forcing uploads](../../src/simulation/timeloop/forward/forcing.rs) and [operator step](../../src/simulation/timeloop/forward/operators.rs); [species mapping](../../src/physics/species.rs).
- GPU: [dry probability/mass update](../../src/gpu/deposition.rs) / [kernel](../../src/shaders/dry_deposition.wgsl); [standalone settling velocity](../../src/gpu/settling.rs) / [kernel](../../src/shaders/settling_velocity.wgsl).
- Authority: [deposition science](../science/deposition.md), [settling science and domain](../science/settling.md), [limitations](../science/known-limitations.md), [GPU contract](../GPU_CONTRACT.md).
- Fixtures: [DRY-007](../../fixtures/corpus/cases/DRY-007.json), [Fortran inputs](../../fixtures/corpus/fortran/DRY-007/), [constant dry identity](../../reference/species-physics/species-040-dry-constant-v1.json); settling [canonical vectors](../../fixtures/settling/canonical-vectors-v1.json), [pinned oracle](../../fixtures/settling/oracle-v1.json), [raw output](../../fixtures/settling/oracle-output-v1.txt) and [direct driver](../../oracle/settling_oracle.f90).
- Tests: [GPU deposition module](../../src/gpu/deposition.rs), [mass evolution](../../tests/integration/deposition_decay.rs), [settling device/oracle](../../tests/settling_gpu.rs); [verification](test-map.md#dry-deposition-settling).
- Dependencies: surface/PBL + species -> dry deposition -> mass/output. [CPU resistance/bin utilities](../../src/physics/deposition.rs) accept settling velocity as an input. #35 owns the standalone GPU spherical settling calculation and its pinned oracle; timeloop composition belongs to #37. A supplied velocity or constant dry fixture does not prove settling physics.

## Decay-mass-ledger

- Production: [decay operator](../../src/simulation/timeloop/forward/operators.rs), [step reports](../../src/simulation/timeloop/reports.rs), [particle species masses](../../src/particles/mod.rs), [species decay constants](../../src/physics/species.rs). Accounting spans these surfaces; no separate mass-ledger module exists.
- GPU: [decay](../../src/gpu/decay.rs) / [kernel](../../src/shaders/decay.wgsl); deposition removal uses the two preceding GPU rows.
- Authority: [species configuration](../species-config.md), [GPU contract](../GPU_CONTRACT.md), issue-owned conservation tolerances.
- Fixtures: [inert species](../../reference/species-physics/species-024-inert-v1.json); analytical per-species decay inputs in [module tests](../../src/gpu/decay.rs).
- Tests: [mass conservation](../../tests/integration/mass_conservation.rs), [species/nuclides](../../tests/integration/species_nuclide.rs), [deposition accounting](../../tests/integration/scientific_invariants.rs); [verification](test-map.md#decay-mass-ledger).
- Dependencies: release inventory -> transport/deposition/decay -> remaining/removed mass -> output. Kernel decay agreement is distinct from production budget closure.

## Gridding-output

- Production boundary: [driver output boundary](../../src/simulation/timeloop/forward/output.rs), [NetCDF writer](../../src/io/netcdf_output.rs).
- GPU: [concentration gridding](../../src/gpu/gridding.rs) / [kernel](../../src/shaders/concentration_gridding.wgsl); explicit D2H for host output.
- Authority: [gridding science](../science/concentration-gridding.md), [case output-grid contract](../../schemas/validation-case-v2.schema.json), [GPU contract](../GPU_CONTRACT.md).
- Fixtures: [corpus output-grid manifests](../../fixtures/corpus/cases/), [ETEX mini](../../fixtures/etex/mini/README.md).
- Tests: [gridding module](../../src/gpu/gridding.rs), [scientific invariants](../../tests/integration/scientific_invariants.rs); [verification](test-map.md#gridding-output).
- Dependencies: resident particles/mass -> aggregation -> host file -> validation decoding. Meteorology and concentration grids are distinct.

## Validation-provenance

- Canonical tooling: [stable case facade](../../src/validation/case/mod.rs), [candidate physics identity](../../src/validation/candidate_physics.rs), [input-equivalence verdicts](../../src/validation/input_equivalence.rs), [corpus CLI](../../src/bin/corpus-run.rs).
- Case responsibilities (read only the matching owner): [root manifest/domain/units/physics and validation order](../../src/validation/case/manifest.rs), [document/schema parsing and serialization](../../src/validation/case/document.rs), [release/species/chronology](../../src/validation/case/release.rs), [meteorology source/profile](../../src/validation/case/meteorology.rs), [output timing/grid](../../src/validation/case/output.rs), [RNG identity/preparation](../../src/validation/case/stochastic.rs), [oracle COMMAND configuration](../../src/validation/case/oracle.rs), [external references/artifact handoffs](../../src/validation/case/handoff.rs). Each owner carries its focused tests; [shared test fixtures/schema helper](../../src/validation/case/test_support.rs) is test-only. [Decomposition inventory/context](validation-case-decomposition.md).
- GPU evidence: [typed schema/comparator](../../src/gpu/evidence.rs); no validation-specific shader/runtime.
- Authority: [compact workflow](../agent-validation.md), [corpus matrix](../corpus-matrix.md), [evaluation](../evaluation.md), [provenance](../run-provenance.md), [stochastic identity](../oracle-stochastic-identity.md), [CI gates](../ci-gates.md).
- Oracle/fixtures: [pinned revision](../../reference/flexpart-11.1.json), [corpus index](../../fixtures/corpus/corpus.json), [case schema](../../schemas/validation-case-v2.schema.json), [manifest schema](../../schemas/run-manifest-v1.schema.json).
- Shared provenance owner: [run identities, artifact verification and manifest invariants](../../scripts/provenance/run_provenance.py); workflow-specific writers below call this module. Inspect it when a task changes consumed-input checks or manifest identity, rather than duplicating those checks in a writer.
- Consumers: [focused runner](../../scripts/agent_validation.py), [corpus orchestration](../../scripts/run-corpus.sh), [input audit](../../scripts/corpus/audit_corpus_inputs.py), [comparison](../../scripts/corpus/compare_corpus.py), [provenance writer](../../scripts/corpus/write_corpus_manifest.py), [evaluation CLI](../../scripts/evaluate/evaluate_case.py).
- Tests: [serialized case bytes/public facade](../../tests/validation_case_contract.rs), [runner](../../scripts/test_agent_validation.py), [input audit](../../scripts/corpus/test_audit_corpus_inputs.py), [manifest](../../scripts/corpus/test_write_corpus_manifest_v1.py); [verification](test-map.md#validation-provenance).
- Dependencies: normalized cases/inputs -> candidate + pinned oracle -> decoded outputs -> comparison/provenance. Input equivalence, execution, and scientific verdicts remain separate contracts.

## Navigation maintenance

Use [dry runs](navigation-dry-runs.md) as examples of selecting a bounded working
set. Check all navigation surfaces with `python scripts/check_agent_navigation.py`.
[CI](../../.github/workflows/agent-navigation.yml) runs that checker and its
[negative regression tests](../../scripts/test_check_agent_navigation.py).
This checks inline/reference path links, heading anchors outside fenced examples,
command targets and named exact-test selectors. It inspects source declarations;
it does not execute tests or prove scientific semantics.
