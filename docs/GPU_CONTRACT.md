# GPU Execution Contract

> **Status: NOT YET NORMATIVE — issue #91 is open.**
>
> This document is the designated repository-level GPU contract. Issue #91 owns its completion and must replace the unresolved sections below with verified decisions before #87, #88, #89, or #90 begins implementation. Until #91 is complete, dependent GPU-port work must not infer or invent missing policy.

## Authority

Once #91 is completed, this document is normative for implementation, modification, composition, and review of GPU calculation code in FLEXPART-GPU.

The pinned FLEXPART oracle owned by the relevant scientific issue is authoritative for scientific parity when it provides adequate coverage. Existing CPU implementations may be used as migration diagnostics, but a separate CPU reference implementation is not a general prerequisite and CPU/GPU agreement does not replace required FLEXPART-oracle evidence.

## Decisions owned by #91

#91 must finalize and document at least:

- GPU runtime/backend and supported execution environment;
- kernel implementation and dispatch model;
- host/device memory ownership and lifetime model;
- explicit transfer and synchronization rules;
- policy for keeping data device-resident across composed calculation stages;
- floating-point precision and numerical comparison policy;
- NaN, infinity, invalid-input and fail-closed behavior;
- GPU initialization, dispatch and execution error propagation;
- prohibition and detection of silent CPU fallback;
- proof that tests actually executed the intended GPU/device path;
- reusable pinned-FLEXPART-oracle-to-GPU comparison procedure;
- machine-readable evidence/provenance required from GPU validation.

## Invariants already fixed by #91

These requirements may not be weakened by downstream GPU-port issues:

1. Supported production GPU calculations must actually execute on the GPU/device.
2. Silent CPU fallback is prohibited for paths declared GPU-supported.
3. Host/device transfers must be explicit; downstream APIs must not hide unnecessary `GPU -> CPU -> GPU` round trips.
4. The architecture must permit meteorological data to remain device-resident across #87–#90 and the later #76 composition.
5. Scientific correctness is established against the relevant pinned FLEXPART oracle within the declared tolerance, not merely by agreement with a Rust CPU implementation.
6. GPU validation must record enough machine-readable evidence to distinguish numerical success from skipped or CPU-fallback execution.

## Unresolved until #91 implementation

The following are intentionally not guessed here:

- final backend/runtime decision and supported adapters;
- concrete buffer/layout abstractions;
- concrete dispatch API;
- exact precision policy and tolerances;
- exact synchronization strategy;
- exact GPU execution-evidence schema;
- exact CI GPU availability/skip/failure policy.

#87–#90 must not start implementation until these items are resolved and #91 is completed.
