# GPU Execution Contract

> **Status: NOT YET NORMATIVE — issue #91 is open.**
>
> This document is the designated repository-level GPU contract. Issue #91 owns its completion and verification before dependent GPU-port work begins. Until #91 is complete, downstream GPU work must not infer or invent missing policy.

## Authority

Once #91 is completed, this document is normative for implementation, modification, composition, and review of GPU calculation code in FLEXPART-GPU.

Issue #91 audits the existing GPU implementation against this contract, closes verified infrastructure gaps, and makes the established architecture explicit. It must not introduce a parallel GPU architecture where the existing implementation already provides a suitable shared mechanism.

The pinned FLEXPART oracle owned by the relevant scientific issue is authoritative for scientific parity when it provides adequate coverage. Existing CPU implementations may be used as migration diagnostics, but a separate CPU reference implementation is not a general prerequisite and CPU/GPU agreement does not replace required FLEXPART-oracle evidence.

## Established GPU architecture

FLEXPART-GPU uses:

- Rust for host-side orchestration;
- `wgpu` as the GPU runtime and portability layer;
- WGSL compute shaders for GPU calculation kernels;
- the existing `GpuContext` as the central owner of `wgpu::Device`, `wgpu::Queue`, and adapter information;
- the existing shared GPU buffer infrastructure as the default basis for GPU-resident data.

All supported GPU calculation paths must integrate with this architecture.

Downstream issues must not introduce an independent GPU runtime, device/queue ownership model, shader runtime, or parallel generic buffer-management architecture unless an existing mechanism is demonstrated to be insufficient and the architectural change is explicitly documented here.

## Execution and dispatch model

The established composable execution pattern separates command encoding from submission:

1. resource preparation and validation;
2. `encode_*` functions encode GPU work into a caller-provided `wgpu::CommandEncoder` without submitting or waiting;
3. reusable `dispatch_*` convenience functions may create an encoder, call the corresponding encode path, submit the command buffer, and synchronize when their API contract requires completion;
4. composed production paths should prefer encode-level composition so consecutive GPU stages can share a command flow without intermediate host synchronization or readback.

New composable GPU calculation stages should follow this separation unless a documented technical constraint requires otherwise.

Synchronization must not be introduced merely as an implementation convenience. It must correspond to an actual dependency, host-visible result requirement, validation boundary, or resource-lifetime requirement.

## End-to-end device-resident calculation path

GPU-resident data is the default between composed GPU calculation stages, not only inside the meteorology port.

The intended production direction is:

`canonical meteorology -> GPU upload -> #87 -> #88 -> #89/#90 -> #76 4D composition -> #77 production physics/particle GPU pipeline -> explicit host/output boundary`

Once data has entered a GPU calculation pipeline, intermediate results consumed only by later GPU stages must remain device-resident.

Unnecessary `GPU -> CPU -> GPU` round trips are prohibited across the complete composed production path. Host readback is permitted only when the host genuinely consumes the result, for explicit diagnostics/validation, final output, or another documented boundary.

Issues #87–#90 must produce and consume GPU resources that can be composed directly by #76. Issue #76 must compose those stages through GPU-resource/encode-level interfaces rather than requiring host materialization between stages. Issue #77 must be able to connect the composed meteorology path to production GPU physics/particle consumers without an architectural requirement for intermediate CPU readback.

This is an interface and composition requirement, not permission for #91 to implement #76 or #77 functionality.

The same principle applies to later GPU calculation stages: new APIs must not make host materialization a mandatory boundary when both producer and consumer execute on the GPU.

## Buffer, transfer, and resource ownership

The existing shared GPU buffer infrastructure is the starting point for new GPU-resident meteorological and downstream calculation resources.

The established storage-buffer baseline uses explicit GPU storage resources with transfer capability. Host-to-device updates are explicit queue writes or explicit resource creation from host data. Device-to-host readback is explicit and uses a dedicated staging resource and mapping step; calculation APIs must not hide readback as part of ordinary GPU-stage composition.

Existing long-lived resource types such as `WindBuffers`, `ParticleBuffers`, and related GPU buffer wrappers demonstrate the intended ownership model: GPU resources are created outside individual kernel invocations and can be reused across dispatches.

Issue #91 must audit whether the existing buffer abstractions and composition model can support #87–#90, #76 integration, and handoff into #77 without unnecessary copies, readbacks, or synchronization. New generic abstractions may be introduced only for requirements not adequately represented by the existing infrastructure.

Buffer ownership and lifetime must make it possible to reuse uploaded meteorological fields across calculation stages and, where scientifically valid, across repeated particle calculations.

Exact meteorological field layouts remain owned by the corresponding scientific implementation issues where they depend on algorithm-specific requirements. They must nevertheless conform to this execution contract.

## GPU versus software execution

A production path declared GPU-supported must execute its calculation kernels through the configured `wgpu` device. Silent substitution with a separate CPU implementation is prohibited.

A software `wgpu` adapter may execute the real WGSL shader path for functional testing where explicitly allowed. Such execution must be distinguishable from hardware-GPU execution and must never be reported as hardware-GPU performance evidence.

Validation evidence must record enough adapter information to determine which execution path was actually used.

## Error handling and fail-closed behavior

GPU initialization, resource creation, shader/pipeline creation, encoding, dispatch, synchronization, and readback failures must propagate explicitly.

A failed GPU path must not silently:

- invoke a CPU implementation;
- skip the calculation;
- return placeholder data;
- downgrade scientific validation requirements.

Where required semantics are unresolved, implementation must fail closed rather than invent behavior.

## Numerical policy

Scientific correctness is determined against the pinned FLEXPART oracle relevant to the calculation being implemented.

Agreement with a Rust CPU implementation is useful for migration diagnostics but is not sufficient scientific validation.

Precision choices, NaN/Inf handling, invalid-input behavior, and comparison tolerances must follow the relevant FLEXPART semantics and the scientific contract of the implementing issue.

Issue #91 must define repository-wide numerical rules where they are infrastructure-level concerns. Algorithm-specific tolerances must not be guessed by #91.

## Execution evidence

GPU validation must produce machine-readable evidence sufficient to establish that:

- the intended WGSL calculation path was dispatched;
- execution was not silently replaced by a CPU implementation;
- the selected adapter/backend is identifiable;
- hardware-GPU execution can be distinguished from software-adapter execution;
- the FLEXPART oracle and comparison configuration are identifiable;
- the numerical result satisfies the declared tolerance.

A passing test that skipped GPU execution is not GPU validation.

## Relationship to downstream issues

Issue #91 owns the common GPU execution infrastructure and this repo-wide contract.

Issues #87–#90 own their respective scientific algorithms and GPU kernels. They must use the architecture defined here rather than independently deciding GPU runtime/backend architecture, device/queue ownership, generic buffer-management strategy, fallback policy, execution-evidence semantics, or generic GPU/host transfer policy.

Issue #76 owns composition of the completed meteorological stages into the 4D meteorological GPU pipeline. Its implementation must preserve device residency and compose GPU-stage interfaces without mandatory intermediate host readback.

Issue #77 owns migration/integration of production physics consumers. It must be able to consume the composed GPU meteorology path directly on-device where the producer and consumer are GPU stages. It may optimize or refactor the integrated pipeline, but must preserve this contract unless an explicit architectural change updates this document.

Later GPU work is governed by the same rules; this contract is not limited to #87–#90.

## Completion criteria for issue #91

Issue #91 is complete when:

1. the existing GPU implementation has been audited against this contract;
2. existing mechanisms have been reused wherever suitable;
3. verified shared-infrastructure gaps required for the downstream GPU pipeline have been closed;
4. no competing GPU architecture has been introduced unnecessarily;
5. device-resident composition of #87–#90 through #76 and handoff into #77 is architecturally possible without mandatory intermediate host readback;
6. silent CPU fallback is prevented or detectable;
7. GPU execution can be demonstrated through machine-readable evidence;
8. repository-wide numerical, error-propagation, and validation rules owned by #91 are finalized;
9. remaining algorithm-specific decisions are explicitly delegated to their owning issues rather than guessed here.

After completion of #91, this document is normative for subsequent GPU implementation and review.

## Remaining decisions owned by #91

The following must still be resolved from verified repository/oracle requirements during #91 rather than guessed in advance:

- exact repository-wide floating-point precision policy where not already scientifically constrained;
- repository-wide NaN/infinity/invalid-input policy where not owned by a scientific issue;
- exact GPU execution-evidence schema;
- exact CI hardware-GPU/software-adapter availability, skip, and failure policy;
- any concrete shared resource/composition abstraction proven necessary for the downstream GPU pipeline and not already covered by existing GPU infrastructure.

Dependent GPU-port implementation must not invent these remaining foundation items before #91 is completed.
