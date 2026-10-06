# Focused verification map

Select the matching domain in [the repository map](repo-map.md) before reading
tests. Commands run from the repository root; Python commands need only Python 3
unless the existing runner states additional prerequisites. Shell gates need
Bash/Linux or the documented Docker environment.

Focused checks below are falsification/diagnostic entry points. For Rust/WGSL,
stable final verification remains `cargo fmt --all -- --check`,
`cargo clippy`, `cargo test`, plus the owning issue's gates
([workflow policy](../agent-validation.md#agent-workflow-policy)).
Some legacy GPU tests return early without an adapter: a green test result alone
is not device evidence. Required execution needs actual WGSL output and adapter
provenance; authoritative oracle comparisons need the issue's pinned raw/decoded
outputs and hashes. Follow [GPU evidence rules](../GPU_CONTRACT.md).
This map does not create new tolerances or upgrade validation levels.

## GPU runtime

- Fast host falsification: `cargo test --lib gpu::evidence::tests`.
- Device evidence: `cargo run --bin gpu-preflight -- --software --json-output target/gpu-preflight.json`; require passed smoke and correct adapter class.
- Final infrastructure gate: [software WGSL workflow](../../.github/workflows/software-wgpu.yml). Smoke proves runtime transfers/execution, not physics.

## Meteorology

Choose the subdomain; do not run every row for one field edit.

| Surface | Fast focused check | Required device/oracle evidence |
| --- | --- | --- |
| Canonical metadata | `cargo test --lib meteorology::field::tests` and `cargo test --lib meteorology::snapshot::tests`, then `cargo test --test meteorology_contract` | Host schema and compact/pretty serialized-byte regression only; [contract](../meteorology-contract.md). |
| Horizontal | `cargo test --test horizontal_gpu test_gpu_interior_matches_oracle -- --exact` | Stable full `cargo test --test horizontal_gpu`; fresh horizontal report tied to revision and interpolation pin. |
| Vertical / W | `cargo test --test vertical_gpu` | Use `FLEXPART_GPU_REQUIRE_VERTICAL=1` for required execution; per-scenario reports checked by software CI. [Private owners and focused evidence](#gpu-vertical), [spatial contract](../interpolation-contract.md). |
| Instantaneous time | `cargo test --test temporal_gpu test_gpu_linear_interior_interpolation -- --exact` | `cargo test --test temporal_gpu test_gpu_vs_oracle_parity -- --exact`; temporal report and paired row evidence. |
| Accumulated intervals | `cargo test --test accumulation_contract` | `cargo test --test accumulation_gpu`; interval/cell evidence and pinned numpf contract. |

- Canonical metadata validation lives in the private owners linked by the [repository map](repo-map.md#meteorology); snapshot boundary tests exercise grid/coordinate/time checks through the supported facade. Sampling/device tests retain their public entry points.
- Composition regression: `cargo test --test meteorology_composition -- --test-threads=1`; [existing composition tests](../../tests/meteorology_composition.rs).
- Host geometry has its own [focused verification entry](#vertical-geometry); [direct oracle driver](../../scripts/vertical/direct_oracle_driver.f90) and [production W driver](../../scripts/interpolation/w_production_oracle.sh) are owned by the linked geometry/spatial contracts.
- Broader gates: [software WGSL workflow](../../.github/workflows/software-wgpu.yml) for device fixtures; [technical gate](../../.github/workflows/validation-gate.yml) for pinned geometry/flex_extract oracles. Kernel evidence does not prove #76/#77 production adoption.

## Vertical geometry

Start at the [stable facade](../../src/meteorology/vertical.rs) and select its
[responsibility owner](repo-map.md#vertical-geometry).

| Responsibility | Focused check | Test surface |
| --- | --- | --- |
| Hybrid pressure, local surface anchor and storage direction | `cargo test --lib meteorology::vertical::pressure::tests` | [pressure](../../src/meteorology/vertical/tests/pressure.rs) |
| Model-level integration and surface thermodynamics | `cargo test --lib meteorology::vertical::model_levels::tests` | [model levels](../../src/meteorology/vertical/tests/model_levels.rs) |
| W/interface construction and derived ASL validity | `cargo test --lib meteorology::vertical::interfaces::tests` | [interfaces](../../src/meteorology/vertical/tests/interfaces.rs) |
| Motion kind/unit/sign/staggering and rejection | `cargo test --lib meteorology::vertical::motion::tests` | [motion](../../src/meteorology/vertical/tests/motion.rs) |
| Validation-only calc_etadot | `cargo test --lib meteorology::vertical::eta_dot::tests` | [eta-dot](../../src/meteorology/vertical/tests/eta_dot.rs) |
| Terrain, release heights and AGL/ASL validity | `cargo test --lib meteorology::vertical::terrain::tests` | [terrain](../../src/meteorology/vertical/tests/terrain.rs) |
| Derived-state shape rejection | `cargo test --lib meteorology::vertical::runtime::tests` | [runtime](../../src/meteorology/vertical/tests/runtime.rs) |
| Snapshot/motion provenance binding | `cargo test --lib meteorology::vertical::provenance::tests` | [provenance](../../src/meteorology/vertical/tests/provenance.rs) |

- Complete relocated host suite: `cargo test --lib meteorology::vertical::` (all 24 original regressions).
- Public facade serialization/runtime identity: `cargo test --test vertical_geometry_contract`; hashes captured before #127 cover model levels and W/interfaces, both storage directions and single/multiple columns with nonzero and below-sea-level terrain.
- Runtime/release handoff: `cargo test --test integration vertical_runtime`; model-level and #80 interface-W oracle regressions: `cargo test --test integration vertical_sampling`; frozen #80 verdict/provenance: `cargo test --test w_production_oracle`.
- GPU consumer check: `cargo test --test vertical_gpu` with `FLEXPART_GPU_REQUIRE_VERTICAL=1` and actual device evidence. This does not change or complete #128/#88.
- Final: normal Rust checks and navigation audit; [technical CI](../../.github/workflows/validation-gate.yml) runs `scripts/ci-gate.sh --particles 1000 --require-flex-extract-oracle`, including fresh pinned #30 synthetic/real columns, calc_etadot and #80 evidence reproduction; [software CI](../../.github/workflows/software-wgpu.yml) requires actual WGSL execution. Existing fixtures/tolerances/verdicts remain authoritative, including #80 `not_equivalent`; these checks do not create a new parity claim.

## Configuration

- First preservation checks: `cargo test --lib config::`, then `cargo test --test config_contract`.
- Select the owner for narrower inspection: `cargo test --lib config::command::tests`, `cargo test --lib config::release::tests`, `cargo test --lib config::output::tests`, `cargo test --lib config::species::tests`, or `cargo test --lib config::simulation::tests`.
- Public boundary evidence: the pre-decomposition transcript checks compact/pretty serialized bytes, defaults, aliases/precedence, timestamps, sentinels/conversions, process predicates, rejected inputs/error text, validation order and Serde round trips. File-loading tests check required-path failures and sorted species ownership. These are configuration preservation checks, not scientific parity evidence.
- Representative OUTGRID task: `cargo test --lib config::output::tests::test_output_grid_inferred_spacing_and_zero_dimensions_are_preserved -- --exact`; [source context comparison](config-decomposition.md#final-layout-and-representative-context).
- Broader: formatting, Clippy, full Cargo tests, [navigation checks](#navigation-only-changes), and existing [software WGSL](../../.github/workflows/software-wgpu.yml) / [technical gate](../../.github/workflows/validation-gate.yml). The technical gate also runs the public config transcript. No new numerical/oracle contract is introduced by decomposition.

## GPU vertical

- First/focused check: set `FLEXPART_GPU_REQUIRE_VERTICAL=1` and run `cargo test --test vertical_gpu -- --nocapture --test-threads=1`. Set `FLEXPART_GPU_SOFTWARE=1` for required software WGSL execution; missing adapters fail closed.
- Model-level oracle: `cargo test --test vertical_gpu gpu_model_matches_71_vertical_model_levels -- --exact`; real column: `cargo test --test vertical_gpu gpu_model_matches_71_real_era5_column -- --exact`.
- W production oracle: `cargo test --test vertical_gpu gpu_w_two_stage_matches_80_pristine_oracle -- --exact`; caller-owned composition: `cargo test --test vertical_gpu gpu_encode_composes_two_stages_without_intermediate_submit -- --exact`.
- Actual shader arithmetic: `cargo test --test vertical_gpu gpu_wgsl_arithmetic_determines_output -- --exact`; model execution evidence: `cargo test --test vertical_gpu gpu_model_evidence_proves_device_execution -- --exact`.
- Evidence: [identity/hash owner](../../src/gpu/vertical/provenance.rs), [paired report validator](../../src/gpu/vertical/evidence.rs), the three per-scenario reports under `target/ci-gate/vertical-gpu/`, and [software CI report checks](../../.github/workflows/software-wgpu.yml). Preserve oracle pins, five bindings, shader bytes, f32 policy and report meaning.
- Shared-resource consumer: `cargo test --test meteorology_composition -- --test-threads=1`; it must retain caller-owned submission and zero intermediate readbacks. Host-only checks: `cargo test --lib gpu::vertical::tests`.
- Broader: final Rust checks, [software WGSL](../../.github/workflows/software-wgpu.yml), [technical oracle gate](../../.github/workflows/validation-gate.yml), [navigation checks](../../scripts/check_agent_navigation.py). [Inventory/context](gpu-vertical-decomposition.md). These preserve #88 evidence and do not add a scientific claim.

## Simulation

- Focused production check: `cargo test --test forward_timeloop test_forward_timeloop_synthetic_uniform_wind_is_deterministic -- --exact`; backward changes: `cargo test --test backward_timeloop`.
- Operator structure: `cargo test --test forward_timeloop test_forward_timeloop_operator_call_order_is_preserved -- --exact`.
- Required driver device/order regression: `cargo test --test forward_timeloop test_forward_timeloop_transport_precedes_deposition_and_reports_precede_advance -- --exact --nocapture`.
- Deferred host/output boundary: `cargo test --test forward_timeloop test_forward_timeloop_deferred_readback_preserves_gpu_output_and_cached_host_mass -- --exact --nocapture`.
- Fail-closed preparation/retry: `cargo test --test forward_timeloop test_forward_timeloop_preparation_errors_preserve_release_and_retry_state -- --exact --nocapture`.
- Forward GPU cases serialize device creation within the test binary to avoid the observed concurrent WARP crash.
- Run the full forward/backward targets; repeat forward with `FLEXPART_GPU_VALIDATION=1` for separated operators. The new device regressions fail on missing adapters. [Software CI](../../.github/workflows/software-wgpu.yml) runs both forward variants and backward.
- Required evidence: actual driver GPU execution, forcing/release identity and owning case outputs. These tests are deterministic integration checks, not pinned scientific parity.
- Broader: `cargo test --test integration physics_validation`, then the issue-owned corpus/oracle gate.

## Transport-advection

- Focused analytical device check: `cargo test --test integration software_advection::test_sw_wgpu_advection_001_constant_wind_displacement -- --exact`; explicitly select the software adapter as in [CI](../../.github/workflows/software-wgpu.yml).
- Focused paired corpus: `python scripts/agent_validation.py --check comparison --case ADV-ANA-001`.
- Evidence: analytical displacement/device marker; paired runner inputs, raw oracle/candidate outputs and manifest. Corpus scientific verdict remains diagnostic.
- Broader: software workflow and owning production/oracle gate.

## PBL-turbulence

- Shared options identity/default/profile contract: `cargo test --test pbl_options_contract`; [public-path regressions](../../tests/pbl_options_contract.rs) freeze all eight default bits, exercise CPU/driver consumers and require real WGSL results through all three aliases. Set `FLEXPART_GPU_SOFTWARE=1` for required software execution; absence is a failure.
- CPU preparation remains separate: `cargo test --lib io::pbl_params::tests`. Candidate mapping: `cargo test --lib validation::candidate_physics::tests`. GPU diagnostics: `cargo test --lib gpu::pbl::tests -- --test-threads=1`; pair with the required-adapter options regression and [software workflow](../../.github/workflows/software-wgpu.yml). Forward/backward preservation uses the [simulation entry](#simulation).
- Typed separated handoff/prefix: `cargo test --lib gpu::hanna::tests::test_hanna_langevin_capacity_output_serves_active_prefix -- --exact --nocapture --test-threads=1`; [stage tests](../../src/gpu/hanna.rs) compare typed/raw WGSL output at capacity=8, active=1/3/8, substeps=0/4 and retain inactive-tail sentinels/counters.
- Typed/raw fail-closed bounds: `cargo test --lib gpu::hanna::tests::test_hanna_langevin_undersized_prefix_fails_before_submission -- --exact --nocapture --test-threads=1`; logical then physical undersizing precede timestep validation and binding. Run both Hanna and [Langevin module tests](../../src/gpu/langevin.rs) and the existing software-WGSL compaction x validation matrix.
- Fast equation diagnostics: `cargo test --lib physics::hanna::tests` (CPU analytical level).
- Production integration: `cargo test --test integration physics_validation`; focused paired stable case: `python scripts/agent_validation.py --check comparison --case PBL-STABLE-004`.
- Evidence: issue-owned stable/neutral/unstable Hanna and ensemble metrics, seeds, device provenance and pinned oracle when required; one trajectory or CPU/GPU agreement is insufficient.
- Broader: relevant three PBL corpus cases and [corpus gate policy](../corpus-matrix.md), not an inferred parity verdict.

## Convection

- Focused diagnostic: `cargo test --test convection_chain`.
- Evidence level: analytical matrix/mass checks and CPU/GPU mixing diagnostic. No full pinned convection parity gate is wired by this test.
- Final: normal Rust checks; the owning integration/oracle issue must supply production evidence before such a claim.

## Wet-deposition

- Fast analytical mass evolution: `cargo test --test integration deposition_decay::deposition_mass_evolution_cpu_matches_analytical_exponential_decay -- --exact`.
- Device diagnostic: `cargo test --lib gpu::wet_deposition::tests`; paired production case: `python scripts/agent_validation.py --check comparison --case WET-008`.
- Evidence: species/scavenging/precipitation identities, GPU execution, retained paired budgets and inert calibration dependency. [Corpus thresholds](../../fixtures/corpus/thresholds.json) are diagnostic.
- Broader: owning deposition production gate; canonical rate/consumer changes additionally use the meteorology row.

## Dry-deposition-settling

- Fast resistance diagnostics: `cargo test --lib physics::deposition::tests`.
- Device diagnostic: `cargo test --lib gpu::deposition::tests`; paired constant-deposition case: `python scripts/agent_validation.py --check comparison --case DRY-007`.
- Settling gate: set `FLEXPART_GPU_REQUIRE_SETTLING=1` and run `cargo test --test settling_gpu`; frozen provenance: `python scripts/generate_settling_oracle.py --audit`; direct reference regeneration: `python scripts/generate_settling_oracle.py --oracle-checkout <pinned-clean-checkout> --check`.
- Evidence: deposition needs mapped species/velocity, actual GPU mass removal and paired budgets. #35 settling needs all 32 canonical rows, pinned raw-output/input/driver identities, downward velocities, unchanged device particle masses and actual WGSL adapter evidence; constant dry velocity does not prove settling physics.
- Broader: owning deposition gate and required software-WGSL CI. The standalone #35 gate does not prove #37 timeloop composition, ground handling or deposition resistance.

## Decay-mass-ledger

- Fast conservation diagnostic: `cargo test --lib physics::species::tests::decay_mass_step_conserves_mass -- --exact`.
- Device diagnostic: `cargo test --lib gpu::decay::tests`; integration: `cargo test --test integration mass_conservation` and `cargo test --test integration scientific_invariants`.
- Evidence: per-species decay and released/remaining/removed mass under issue-owned tolerances; adapter execution and full driver budgets for production claims.
- Broader: owning species/decay or production budget gate; no independent full-decay pinned parity gate is implied here.

## Gridding-output

- Focused analytical device check: `cargo test --lib gpu::gridding::tests::gpu_concentration_gridding_simple_particle_pattern_matches_expected_cells -- --exact`.
- Integration: `cargo test --test integration scientific_invariants::concentration_gridding_gpu_matches_cpu_reference_with_outheights -- --exact`.
- Evidence: actual GPU accumulation, cell volume/output-height identity, mass and averaging semantics; CPU comparison is diagnostic.
- Broader: owning output/corpus case and [technical gate](../ci-gates.md); NetCDF changes need the feature-enabled writer checks defined by their issue.

## Validation-provenance

Choose the artifact contract being changed.

- Focused wrapper: `python scripts/test_agent_validation.py`.
- Input equivalence: `python scripts/corpus/test_audit_corpus_inputs.py`.
- Corpus manifest: `python scripts/corpus/test_write_corpus_manifest_v1.py`; shared provenance: `python scripts/provenance/test_run_provenance.py`.
- Canonical case validation: `cargo test --lib validation::case::` (all eight private responsibility modules).
- Focused case checks: choose the owner below; use [shared fixtures/schema helper](../../src/validation/case/test_support.rs) only when needed to interpret a test.

| Case responsibility | Focused check |
| --- | --- |
| Root domain/integration, units, physics consistency | `cargo test --lib validation::case::manifest::tests` |
| Document/schema parsing and serialization | `cargo test --lib validation::case::document::tests` |
| Release geometry, inventory/species, chronology | `cargo test --lib validation::case::release::tests` |
| Meteorology identity and profile | `cargo test --lib validation::case::meteorology::tests` |
| Output timing/direction and grid | `cargo test --lib validation::case::output::tests` |
| Candidate/oracle RNG identity | `cargo test --lib validation::case::stochastic::tests` |
| Oracle COMMAND configuration | `cargo test --lib validation::case::oracle::tests` |
| External reference/artifact handoff | `cargo test --lib validation::case::handoff::tests` |

- Boundary regressions: `cargo test --lib validation::case::output::tests::sampling_interval_exceeding_averaging_window_rejected -- --exact` and `cargo test --lib validation::case::stochastic::tests::direct_serde_deserialization_requires_nullable_oracle_state_fields -- --exact`. These belong to the output and RNG owners even when parsing/serialization is part of the assertion.
- Public facade/compact and pretty serialized-byte regression: `cargo test --test validation_case_contract`; [pre-decomposition snapshot](../../tests/fixtures/validation-case-serialization-v2.txt).
- Consumer check for supported cases: `python scripts/agent_validation.py --check comparison --case ADV-ANA-001`; other cases use their existing documented entry.
- Evidence: fail-closed missing/stale input/output and hash checks, actual subprocess statuses, manifest and comparison report. `PASS` describes execution; `DIAGNOSTIC_NO_PARITY_VERDICT` remains the current corpus verdict.
- Broader: `bash scripts/ci-gate.sh --particles 1000 --require-flex-extract-oracle` when changing its consumed contracts; full corpus/ETEX only when required by the owning issue. [Existing policy](../agent-validation.md) governs prerequisites/cache reuse.

## Navigation-only changes

- `python scripts/check_agent_navigation.py` audits inline/reference links, real heading anchors, command targets and named exact-test selectors without executing those commands.
- `python scripts/test_check_agent_navigation.py` covers broken links/references/anchors, missing command targets and renamed exact-test selectors.
- Review root-rule preservation, skill frontmatter and the complete diff; run `git diff --check`.
- No scientific computation changes: Cargo/oracle checks do not prove navigation quality. Checker/workflow changes require these executable checks and [navigation CI](../../.github/workflows/agent-navigation.yml).
