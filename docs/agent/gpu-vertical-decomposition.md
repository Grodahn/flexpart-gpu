# GPU vertical decomposition (#128)

## Pre-move inventory and target

Recorded before code movement at base `f4fe2d2`. The sole implementation
contract is issue #128; the numerical and execution authorities remain #88
and [the GPU contract](../GPU_CONTRACT.md).

The original [GPU vertical facade](../../src/gpu/vertical.rs) mixed:

| Responsibility | Original surface | Target private owner |
| --- | --- | --- |
| Persistent source/grid/query/output buffers and upload validation | Resource structs, constructors, geometry hashes and size/finite/shape checks | `vertical/resources.rs` |
| Canonical runtime column preparation | Model/center-W/interface-W extraction, ordering, provenance checks and AGL resolution | `vertical/preparation.rs` |
| Reusable pipelines | Sample/remap shader selection, layouts, workgroup policy and creation-time scopes/wait | `vertical/pipeline.rs` |
| Caller-owned command encoding | Sample/remap uniforms and bind groups, count/geometry checks, compute passes and ordered two-stage encoding | `vertical/encode.rs` |
| Standalone completion and explicit D2H | Isolated dispatch, submit/wait/scopes, validation helpers and download | `vertical/dispatch.rs` |
| Calculation/oracle identity | Pinned constants, comparison policies, source/input hashes and geometry identity | `vertical/provenance.rs` |
| Device-result evidence | Serialized rows/reports, fail-closed validation and paired evidence builders | `vertical/evidence.rs` |
| GPU failures | Public error variants and shared device error-scope push/pop | `vertical/error.rs` |
| Existing host-only unit checks | Shader hashes and constructor preconditions | `vertical/tests.rs` (test-only) |

Keep `src/gpu/vertical.rs` as the small stable facade: preserve its scientific
and composition documentation and explicitly re-export every existing public
item. All implementation modules stay private; cross-owner helpers/fields use
`pub(super)` only where required. Existing `gpu` re-exports and consumers keep
their signatures. Binding construction stays inside encoding to retain the
dispatch-local uniform and bind-group lifetime. No new generic runtime or
resource abstraction is needed.

## Preserved execution boundaries

- Pipeline constructors retain their existing error scopes and `poll(Wait)`.
- `encode_vertical_sample_with_kernel` and `encode_vertical_remap_w_with_kernel`
  retain validation, uniform allocation, binding creation, and compute passes
  in their original order. They never submit, poll, map, or download.
- `encode_vertical_w_two_stage_with_kernels` encodes remap then sample in one
  caller-owned encoder. Shared values remain device-resident.
- Standalone dispatch retains its own scopes, encoder, one submit and wait.
  The standalone W helper retains its two separate dispatch/submit/wait steps.
- `download_vertical_samples` remains the named staging/mapping D2H boundary.
- Resource ownership, source/dispatch lifetimes, five group-0 bindings, 16-byte
  uniforms, workgroup width 64, shader bytes, f32 evaluation, labels, pinned
  oracle identities, comparison policies and evidence fields stay unchanged.

This is structural evidence preservation, not a new scientific parity claim.
Meteorology geometry, production consumer migration and #117 cleanup remain
outside this change. A discovered scientific/contract defect requires a
focused follow-up rather than a repair here.

## Final layout and representative context

The facade is 120 lines, with 54 explicit public re-exports. Eight private
implementation owners and one test-only module implement the inventory above.
The largest owner is [evidence](../../src/gpu/vertical/evidence.rs), 663 lines;
the caller-encoder owner is [encode](../../src/gpu/vertical/encode.rs), 239 lines.
The shader files and all external callers remain unchanged.

Representative task: inspect W remap-before-sample encoding and rejection of
a shared grid with mismatched heights. Read the facade and encoding owner,
both shaders and [vertical tests](../../tests/vertical_gpu.rs). Resource hash
construction and pipeline layouts are named adjacent handoffs only when needed.
The contracts, navigation instructions and frozen oracle fixtures are unchanged
context in both measurements and excluded from the source totals.

Measured as complete file lines and UTF-8 bytes with line endings normalized
to LF, against base `f4fe2d2`:

| Source working set | Before | After |
| --- | ---: | ---: |
| GPU host source | `vertical.rs`: 2,655 lines / 100,377 bytes | facade + encode: 359 lines / 14,525 bytes |
| Same sample/remap shaders and full vertical integration tests | 2,410 lines / 92,388 bytes | 2,410 lines / 92,388 bytes |
| Complete representative source set | 5,065 lines / 192,765 bytes, 4 files | 2,769 lines / 106,913 bytes, 5 files |

This reduces required host-source bytes by 85.5% and the conservative complete
source set by 44.5%. It measures context selection, not model tokens, runtime
performance, or total repository size. Loading every private owner would undo
the benefit; the [GPU vertical row](repo-map.md#gpu-vertical) selects the owner.

## Verification evidence

Before movement, the required software-device suite passed all 24 vertical
tests. After movement the same suite passed, including the two #71 model-level
oracle cases, the #80 W production oracle, ordering/shared-height rejection,
shader-arithmetic sensitivity, and fail-closed evidence regressions.

A bounded extraction audit compared all 95 non-test original items after
normalizing comments/whitespace, restricted visibility and relocation paths;
the executable bodies, type layouts, constants and 54 facade exports matched.
The three complete per-scenario reports (6 synthetic model rows, 5 real-column
rows, 5 W rows) were byte-identical before/after on the same WARP adapter and
base revision. They retain actual `wgsl_device` execution, `software_wgsl`
classification, pinned oracle hashes and passing comparisons. This same-adapter
identity check does not impose new cross-backend bitwise numerical policy.

Full local transcripts and the mechanical audit remain under
`target/issue-128/`; generated scenario evidence remains under
`target/ci-gate/vertical-gpu/`. Final Rust and CI outcomes are recorded in the PR.

The composition source-inspection regression now locates the sample/remap
bodies in `vertical/encode.rs`, stopping at the following two-stage wrapper.
It keeps the same forbidden host-completion checks over the same two bodies;
no execution or scientific validation criterion changes.
