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
| Canonical metadata | `cargo test --test meteorology_contract` | Host schema only; [contract](../meteorology-contract.md). |
| Horizontal | `cargo test --test horizontal_gpu test_gpu_interior_matches_oracle -- --exact` | Stable full `cargo test --test horizontal_gpu`; fresh horizontal report tied to revision and interpolation pin. |
| Vertical / W | `cargo test --test vertical_gpu` | Use `FLEXPART_GPU_REQUIRE_VERTICAL=1` for required execution; per-scenario reports checked by software CI. [Spatial contract](../interpolation-contract.md). |
| Instantaneous time | `cargo test --test temporal_gpu test_gpu_linear_interior_interpolation -- --exact` | `cargo test --test temporal_gpu test_gpu_vs_oracle_parity -- --exact`; temporal report and paired row evidence. |
| Accumulated intervals | `cargo test --test accumulation_contract` | `cargo test --test accumulation_gpu`; interval/cell evidence and pinned numpf contract. |

- Geometry host regression: `cargo test --test integration vertical_runtime`; [direct oracle driver](../../scripts/vertical/direct_oracle_driver.f90) and [production W driver](../../scripts/interpolation/w_production_oracle.sh) are owned by the linked geometry/spatial contracts.
- Broader gates: [software WGSL workflow](../../.github/workflows/software-wgpu.yml) for device fixtures; [technical gate](../../.github/workflows/validation-gate.yml) for pinned geometry/flex_extract oracles. Kernel evidence does not prove #76/#77 production adoption.

## Simulation

- Focused production check: `cargo test --test forward_timeloop test_forward_timeloop_synthetic_uniform_wind_is_deterministic -- --exact`; backward changes: `cargo test --test backward_timeloop`.
- Required evidence: actual driver GPU execution, forcing/release identity and owning case outputs. These tests are deterministic integration checks, not pinned scientific parity.
- Broader: `cargo test --test integration physics_validation`, then the issue-owned corpus/oracle gate.

## Transport-advection

- Focused analytical device check: `cargo test --test integration software_advection::test_sw_wgpu_advection_001_constant_wind_displacement -- --exact`; explicitly select the software adapter as in [CI](../../.github/workflows/software-wgpu.yml).
- Focused paired corpus: `python scripts/agent_validation.py --check comparison --case ADV-ANA-001`.
- Evidence: analytical displacement/device marker; paired runner inputs, raw oracle/candidate outputs and manifest. Corpus scientific verdict remains diagnostic.
- Broader: software workflow and owning production/oracle gate.

## PBL-turbulence

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

- Public facade/compact and pretty serialized-byte regression: `cargo test --test validation_case_contract`; [pre-decomposition snapshot](../../tests/fixtures/validation-case-serialization-v2.txt).
- Consumer check for supported cases: `python scripts/agent_validation.py --check comparison --case ADV-ANA-001`; other cases use their existing documented entry.
- Evidence: fail-closed missing/stale input/output and hash checks, actual subprocess statuses, manifest and comparison report. `PASS` describes execution; `DIAGNOSTIC_NO_PARITY_VERDICT` remains the current corpus verdict.
- Broader: `bash scripts/ci-gate.sh --particles 1000 --require-flex-extract-oracle` when changing its consumed contracts; full corpus/ETEX only when required by the owning issue. [Existing policy](../agent-validation.md) governs prerequisites/cache reuse.

## Navigation-only changes

- `python scripts/check_agent_navigation.py` audits inline/reference links, real heading anchors, command targets and named exact-test selectors without executing those commands.
- `python scripts/test_check_agent_navigation.py` covers broken links/references/anchors, missing command targets and renamed exact-test selectors.
- Review root-rule preservation, skill frontmatter and the complete diff; run `git diff --check`.
- No scientific computation changes: Cargo/oracle checks do not prove navigation quality. Checker/workflow changes require these executable checks and [navigation CI](../../.github/workflows/agent-navigation.yml).
