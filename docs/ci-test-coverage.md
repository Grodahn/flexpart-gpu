# Required CI test coverage audit (#175)

Contract: [issue #175](https://github.com/Grodahn/flexpart-gpu/issues/175).
Baseline: `56b7577a38a5ce62387315ed8f5dc01e00b96679` (latest main at branch creation,
including #173 and #174). Externally supplied provenance marker: **GPT6.1 Sol**.
No numerical implementation, oracle pin, fixture, tolerance or verdict changes.

The [before inventory](ci-test-coverage-before.json) lists **every old workflow
Cargo test invocation**, expanded over finite shell loops, with exact arguments,
workflow/step and environment. Defaults for absent compaction and validation flags
are both `0`, as defined in `src/simulation/timeloop/options.rs`. Both workflows
request `FLEXPART_GPU_SOFTWARE=1`, `WGPU_BACKEND=vulkan`,
`LIBGL_ALWAYS_SOFTWARE=1` and the installed Lavapipe ICD. Software workflow
candidate/head/merge revision overrides are preserved verbatim.

Run `python scripts/check_ci_test_coverage.py` for the before/after enumeration.
It maps every baseline driver test/configuration to a remaining invocation, every
other command to an unchanged command or the complete library suite, and protects
immutable baseline driver/canonical-library test identities, active evidence
checks, artifact upload inputs (including upload on failure), and all unchanged
scientific/device/provenance shell assertions and the complete
`scripts/ci-gate.sh` with hashes. This is a finite coverage/log auditor, not a test
runner. It does not execute Cargo, rebuild or compare oracles, or define a verdict.
The existing #174 runner remains available for local focused diagnostics.

## Forward/backward configuration matrix

`F` = complete `--test forward_timeloop` (8 tests), `B` = complete
`--test backward_timeloop` (2 tests), `G` = forward with
`--skip test_forward_timeloop_operator_call_order_is_preserved` (7 tests).
All commands retain `--nocapture`. New driver invocations explicitly set both
flags and `RUST_LOG=flexpart_gpu=info` so required adapter markers are captured.

| Compaction | Validation / turbulence | Before executed selections | After executed selection / retained transcript |
| --- | --- | --- | --- |
| 0 | 0 / fused Hanna+Langevin | F+B (#126), F (#139), dry capacity exact (#135), order exact + mixed capacity exact (#138) | F+B, `target/ci-gate/timeloop-production.log` |
| 0 | 1 / separated Hanna then Langevin | F (#126), F (#139) | F, `target/ci-gate/timeloop-validation.log` |
| 1 | 0 / fused Hanna+Langevin | F (#139), dry capacity exact (#135), order exact + mixed capacity exact (#138) | G, `target/ci-gate/hanna-forward-1-0.log` |
| 1 | 1 / separated Hanna then Langevin | F (#139) | G, `target/ci-gate/hanna-forward-1-1.log` |

The only assertion omitted in secondary configurations inspects identical source
text (`include_str!`) and reads neither flags nor a device. It still runs in both
complete targets. Every device-dependent forward test stays in **all four cells**,
including spatial sorting, deterministic transport, deferred output and retry.
Backward remains in its original default cell, with receptor release, decreasing
clock/alpha, signed transport, mass and source collection assertions unchanged.

| Required behavior / assertions | Owning test suffix (prefix `test_forward_timeloop_`) | After cells and evidence |
| --- | --- | --- |
| Forward release, deterministic transport, interpolation, inert mass | `synthetic_uniform_wind_is_deterministic` | All 4; `TIMELOOP-FORWARD-175` after final assertion |
| Spatial-sort slot permutation | `optional_spatial_sort_reorders_particle_slots` | All 4; `TIMELOOP-SORT-175` after final assertion |
| Operator and submission order, backward shared handoff, timestep order | `operator_call_order_is_preserved` | Both full compaction-0 runs; source-text assertion, flag independent |
| Transport before deposition, inclusive end/report before advance | `transport_precedes_deposition_and_reports_precede_advance` | All 4; `TIMELOOP-ORDER-126` |
| Dry-only active-prefix and full capacity | `dry_deposition_multiple_active_and_full_capacity` | All 4; capacity **8**, active **1/3/8**, two timesteps; `DRY-FORWARD-135` includes compaction |
| Mixed dry/wet/decay active-prefix and full capacity | `mixed_deposition_multiple_active_and_full_capacity` | All 4; capacity **8**, active **1/3/8**, two timesteps; `DRY-FORWARD-135`, `WET-FORWARD-138` include compaction |
| Deferred GPU gridding output versus cached host mass | `deferred_readback_preserves_gpu_output_and_cached_host_mass` | All 4; `TIMELOOP-DEFERRED-126` |
| Bad bracket/species/slot, release state and exactly one retry | `preparation_errors_preserve_release_and_retry_state` | All 4; `TIMELOOP-ERROR-126` |
| Backward release/source collection | `test_backward_timeloop_receptor_release_and_source_collection` (backward target) | Default 0/0; `TIMELOOP-BACKWARD-175` after final assertion |
| Backward invalid receptor configuration | `test_backward_config_requires_receptor` (backward target) | Default 0/0; successful named test and count |

Three new completion markers expose existing early returns on `NoAdapter`; missing
markers now fail the job. No test is deleted and no test assertion is altered.
The log auditor requires exactly the declared selected tests, positive successful
counts with no ignored tests, every driver marker, software-adapter selection and
each dry/wet capacity marker. Shell redirection truncates each transcript before
Cargo starts, so old log evidence cannot satisfy a new invocation.

## Remaining software workflow invocations

Each row below runs once before and once after, unless stated otherwise. Full
arguments and environment overrides are in the machine inventory. GPU evidence
is functional **software WGSL** execution, not hardware performance evidence.

| Cargo target/filter | Additional environment | Required existing evidence and checks retained |
| --- | --- | --- |
| `--test integration software_advection::test_sw_wgpu_advection_001_constant_wind_displacement -- --exact --nocapture` | job software/Vulkan | `SW-WGPU-ADVECTION-001`, exactly 1 passed, no FAILED, `software_adapter=true`; analytical displacement |
| `--test accumulation_gpu` | job | 12 large-scale + 12 convective precipitation cell reports; passed comparison, `wgsl_device`, `software_wgsl` |
| `--test temporal_gpu test_gpu_vs_oracle_parity -- --exact` | job | Temporal report schema, 3 rows, candidate `GITHUB_SHA`, pinned revision, paired GPU/comparison verdicts |
| `--test vertical_gpu -- --nocapture --test-threads=1` | `FLEXPART_GPU_REQUIRE_VERTICAL=1` | Model, real ERA5 column and two-stage W reports; row counts/values, source and shader hashes, pins, executable/output identities and existing tolerances |
| `--test horizontal_gpu` | candidate revision = `github.sha` | Fresh 7-row/3-case oracle report, candidate revision, pin, software adapter and paired verdict |
| `--test settling_gpu` | require settling; revision = `github.sha` | Frozen provenance audit + its Python tests; fresh 32-vector report, shader/input/oracle identities and paired verdict |
| `--test meteorology_resident` | job | 21-case fresh report; revision/shader/source/query hashes, active prefixes, fatal/empty/reset status, one submission, zero intermediate D2H |
| `--lib gpu::meteorology::resident::tests` | job | Resident corner cases, separately retained transcript |
| `--test meteorology_composition -- --test-threads=1` | job | Field-specific report, all source/handoff hashes, stage order/copies, pins/stage references, adapter, residency and unchanged comparisons |
| `--test pbl_options_contract` | job | `PBL-OPTIONS-147`, software adapter, 3 public paths with identical device results |
| `--lib gpu::pbl::tests` | job | PBL diagnostics, complete transcript |
| `--test canonical_timeloop_meteorology` | job | #173 schema, revision/source/shader/motion lineage; 36 U/V/center-W rows over 6 forward/backward selections; readiness, reuse/rebuild and fail-closed preflight |
| `--lib gpu::deposition::tests` | job | `DRY-PREFIX-135`, `DRY-BOUNDS-135`; shader prefix/initialization/bounds cases |
| `--lib gpu::wet_deposition::tests` | job | `WET-PREFIX-138` capacity 8 active 1/3/8; `WET-BOUNDS-138` |
| `--lib gpu::hanna::tests` | job | `HANNA-PREFIX-139` capacity 8 active 1/3/8 × substeps 0/4; `HANNA-BOUNDS-139` |
| `--lib gpu::langevin::tests` | job | Separated turbulence kernel diagnostics, complete transcript |
| `--test integration corpus_` | job | All deterministic corpus tests, positive successful count, skipped execution rejected; actual Cargo status propagated |

Preflight remains a separate `cargo run --bin gpu-preflight`: explicit H2D/WGSL
arithmetic/D2H, successful smoke values and versioned software adapter evidence.
All original scientific report assertions and uploads remain; complete Cargo logs are now
retained without `tee` mirroring their successful output. Redundant dry/wet
forward log files are replaced by the four authoritative driver transcripts.

## Technical workflow and library side effects

| Before | After | Proof / retained producer |
| --- | --- | --- |
| `cargo test --lib` | Once, unchanged complete selection; captured as `target/ci-gate/rust-lib.log` | Log audit requires positive counts and every pinned baseline canonical subset test |
| `cargo test --lib meteorology::field::tests` | Covered by complete lib | Host metadata assertions; no filesystem report producer |
| `cargo test --lib meteorology::snapshot::tests` | Covered by complete lib | Host snapshot/provenance assertions; no filesystem report producer |
| `cargo test --lib meteorology::vertical::` | Covered by complete lib | All relocated host geometry assertions; no filesystem report producer |
| `cargo test --lib meteorology::vertical_sampling::tests` | Covered by complete lib | Frozen goldens read via `include_str!`; no filesystem report producer |
| `--test validation_case_contract` | Unchanged | Serialized public case contract |
| `--test config_contract` | Unchanged | Configuration facade/parser transcript |
| `--test vertical_geometry_contract` | Unchanged | Geometry facade identity |
| `--test integration vertical_runtime` | Unchanged | Runtime/release geometry assertions |
| `--test integration vertical_sampling` | Unchanged | Writes the two required model/interface-W reports; existence checks remain |
| `--test meteorology_contract` | Unchanged | Canonical metadata/serialization assertions |

`python3 scripts/test_oracle_run_manifest.py` remains unchanged. The complete
`scripts/ci-gate.sh --particles 1000 --require-flex-extract-oracle` remains
unchanged, including its **five** Cargo test commands (interpolation contract,
production W oracle, vertical-sampling lib, vertical-sampling integration,
software advection). Its vertical lib/integration repeats are deliberately kept
because the script owns fresh unit/integration logs and report/provenance audits.
Its pinned clean FLEXPART/flex_extract checkouts, Docker/Fortran routines, real
column/eta-dot/interpolation/W oracle reproduction, candidate smoke, manifests,
hashes and `TECHNICAL_PASS` assertions remain authoritative. No oracle command
or scientific verdict changes. Technical success still proves no scientific parity.

## Counts, timing and output

Measured baseline GitHub runs on main, 2026-10-08:
[software 37758267482](https://github.com/Grodahn/flexpart-gpu/actions/runs/37758267482),
[technical 37758267486](https://github.com/Grodahn/flexpart-gpu/actions/runs/37758267486).
Full downloaded logs and job timestamps are retained locally under
`target/issue-175/before-*`; all reports/logs are also retained in GitHub artifacts.

| Static execution count | Before | After |
| --- | ---: | ---: |
| Software workflow Cargo test invocations | 29 | 21 |
| Technical workflow direct Cargo test invocations | 11 | 7 |
| Unchanged technical shell gate Cargo test invocations | 5 | 5 |
| Total, including technical shell gate | 45 | 33 |
| Forward invocations / full forward targets | 12 / 6 | 4 / 2 |
| Forward individual test executions | 54 | 30 |
| Unique driver assertion/configuration cells | 31 | 31 |

Representative observed baseline: software job **159 s**, technical job **423 s**;
full `gh run view --log` output **231,432** and **548,351 bytes** respectively.
The baseline software artifact retains **28 complete log files, 298,254 bytes**.
The four old forward-bearing steps occupy **28 s** in aggregate. Full-library
plus canonical-contract steps occupy **45 s** (including compilation).
Final PR run measurements are recorded in the PR description and retained under
`target/issue-175/after-*` after CI. Shared-runner, network and
compilation variation prevents attributing whole-job differences solely to test
selection; there is no promised speedup. These times are orchestration measurements.

## Verification and scope limits

`python scripts/test_check_ci_test_coverage.py` tests removed configuration cells,
capacity tests, scientific assertions and software requirements; missing adapter,
completion/capacity markers, zero tests, ignored tests and incomplete library logs.
It executes changed Bash step bodies with stub Cargo statuses **37**, **127** and
**0 with missing evidence**, proving exact nonzero propagation and fail-closed
successful subprocesses. Actual device/report/Fortran gates still run in CI.
Navigation, formatting, Clippy, Rust tests and final PR checks are recorded in the
PR. Full logs are retained on disk; successful audits emit one JSON line.

#112 was open at branch creation and contributed no production-advection tests to
this baseline. Check latest main before final verification and retain/add its
production tests and evidence if it merges. This issue changes neither #112's
branch nor its implementation. #174 runner internals, #176 caching and #177 job
restructuring are outside this diff. Required job/check names, triggers and
candidate/PR provenance remain unchanged.

Local final verification: all four WARP driver cells passed (10/8/7/7 tests,
including default backward). The complete `cargo test` run passed, including all **514 library
tests**; its extracted library transcript passes the same canonical-subset audit.
Formatting, Clippy, navigation, workflow YAML syntax, 17 coverage/shell negative
tests and 24 unchanged compact-runner regression tests passed. Authoritative
Lavapipe and pinned-oracle confirmation belongs to the final PR checks.


Review repairs freeze all ten baseline driver declarations and every canonical
library test name from main technical run 37758267486. Removing one baseline
dry-only driver test or one case within a library subset now fails the auditor.
New driver declarations are also required in the current selections. Required
checks must be active command lines, and retained paths must be real upload
inputs with `if: always()`; comments and command-line log destinations cannot
substitute for either. Inline driver adapter/provenance overrides are checked
against the original environment. These repairs change no test selection,
scientific validator, workflow command or measured invocation count.
