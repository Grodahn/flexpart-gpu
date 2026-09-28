# GPU Execution Contract

> **Status: normative repository-wide contract.**
>
> Issue #91 audited and established this contract. It applies to every
> calculative GPU implementation, modification, composition, integration, and
> review in FLEXPART-GPU. [`GPU_PIPELINE.md`](GPU_PIPELINE.md) is the maintained
> architecture map; this contract is authoritative if the documents conflict.

## Authority and scope

This contract governs meteorology, particle transport, advection, turbulence,
convection, deposition, decay, gridding, and later calculative GPU work. It is
not limited to issues #87–#90.

Scientific correctness is determined by the pinned FLEXPART oracle and proof
obligation owned by the relevant scientific issue. A Rust CPU implementation is
useful for migration diagnostics, but CPU/GPU agreement is not a general
substitute for authoritative oracle evidence.

Algorithm-specific field layouts, tolerances, input semantics, and scientific
operation ordering stay with their owning issues. If one of those semantics is
unknown, implementation must fail closed and record the dependency rather than
inventing it in shared GPU infrastructure.

## Issue #91 implementation audit

The audit covered `src/gpu/`, `src/simulation/timeloop.rs`, the canonical
meteorology/runtime-geometry boundaries, the preflight and CI gates, and the
composition needs of #87–#90, #76, and #77.

| Capability | Verified implementation | Classification and #91 action |
| --- | --- | --- |
| Runtime/device ownership | `gpu::GpuContext` owns one `wgpu::Device`, `wgpu::Queue`, and adapter identity | Satisfies the contract. Reused as the only runtime root; preflight now uses it too. |
| Adapter selection | `gpu::adapter::GpuAdapterOptions`; explicit software fallback environment/CLI controls | Satisfies with a small correction. Adapter provenance is now emitted in a stable machine schema. |
| Persistent resources | `ParticleBuffers`, `WindBuffers`, `DualWindBuffers`, `PblBuffers`, stage-specific IO buffers | Satisfies the shared lifetime requirement. No new generic buffer wrapper was justified. |
| H2D | buffer initialization, `queue.write_buffer`, texture upload, incremental particle-slot upload | Satisfies the contract. Transfers are explicit at resource construction/update APIs. |
| D2H | `download_buffer_bytes`/`download_buffer_typed` and typed download methods use MAP_READ staging buffers | Satisfies the contract at named host-consumer boundaries. Ordinary `encode_*` paths do not read back. |
| Encode composition | advection, Hanna, Langevin/fused Langevin, PBL, reflection, deposition, decay, compaction, and particle-step stages expose caller-encoder paths used by the production time loop | Satisfies the current production composition requirement. Future composable stages must follow the same pattern. |
| Standalone dispatch/readback helpers | convenience APIs submit/wait; interpolation, RNG, CBL, convection, and output helpers expose standalone completion or host results | Satisfies only as standalone/validation APIs. They must not be inserted between GPU-capable production stages; an owning integration ticket must add an encode/resource surface before such a stage becomes production-composed. |
| Production composition | forward/backward drivers keep particle/wind/PBL resources resident and batch dependent stages into caller-owned encoders | Satisfies the current architecture baseline. #76/#77 own canonical meteorology adoption, not #91. |
| Missing-adapter workflow behavior | several CPU-store convenience workflows returned `Ok(None)`/`Ok(false)` | Contract violation corrected by #91: missing GPU execution now returns an explicit error. |
| Smoke computation | preflight previously wrote a shader constant and returned text-only provenance | Shared gap corrected by #91: explicit H2D input, WGSL arithmetic, D2H readback, result verification, scoped GPU errors, and versioned JSON pass/fail evidence. |
| Generic oracle evidence | no single typed execution/comparison record existed | Shared gap corrected by `gpu::evidence`; it distinguishes execution and numerical verdicts and rejects false GPU claims. |
| Canonical #87–#90 layouts/kernels | not implemented when this audit was performed | Algorithm-specific. Deferred to the owning tickets; no speculative layout or kernel was added. |

The audit found no repository-wide need for a second runtime, command graph,
generic resource hierarchy, or allocation framework.

## Established GPU architecture

FLEXPART-GPU uses:

- Rust for host-side validation and orchestration;
- `wgpu` as the runtime and portability layer;
- WGSL compute shaders for calculative device kernels;
- `GpuContext` as the central owner of device, queue, and adapter information;
- the existing typed buffer/resource wrappers as the default device-resident
  data foundation;
- caller-owned command encoders for composition.

Do not introduce an independent runtime, device/queue owner, shader runtime,
submission framework, or generic buffer architecture unless a repository-wide
requirement is demonstrated and this document and `GPU_PIPELINE.md` are updated.

## Hardware and backend policy

A supported runtime is a `wgpu` compute adapter/backend that satisfies the
features and limits explicitly required by the calculation stage. There is no
repository-wide vendor requirement. A stage that requires an optional feature
must either select a scientifically equivalent device path already declared by
that stage or return an explicit unsupported error.

Execution provenance uses these classifications:

- `hardware_gpu`: any non-CPU `wgpu` adapter, including discrete, integrated,
  or virtual GPU devices;
- `software_wgsl`: a CPU-backed `wgpu` adapter such as Lavapipe or WARP that
  executes the real WGSL/device path;
- `not_executed`: the calculation-path value used when no adapter calculation
  ran and no GPU validation occurred; no adapter class is then present.

A software WGSL adapter is allowed for explicitly designated functional and
infrastructure tests. It is not a separate CPU reference implementation, but it
must never be reported as hardware-GPU execution or performance evidence.

## Execution, submission, and composition

Composable stages follow this sequence:

1. validate host-visible dimensions, ranges, and required finite inputs;
2. prepare or update persistent GPU resources explicitly;
3. encode work into a caller-provided `wgpu::CommandEncoder` without submitting,
   polling, mapping, or reading back;
4. let the composition owner encode later dependent stages;
5. submit once at the owning boundary;
6. synchronize or read back only for a genuine host consumer.

Reusable `dispatch_*` helpers may create an encoder, submit it, and wait when
standalone completion is part of their documented API. A `dispatch_*` helper is
not a composition surface merely because it executes a shader.

Synchronization must correspond to a real data dependency, host-visible
result, validation/output boundary, or resource-lifetime requirement. It must
not be added for implementation convenience between device-capable stages.

## End-to-end device residency

Once data enters a calculative GPU pipeline, intermediate state consumed only by
later GPU stages remains device-resident. A GPU-capable producer must not force
`GPU -> CPU -> GPU` merely to hand data to a GPU-capable consumer.

The intended migration boundary is:

`canonical meteorology + #30 runtime geometry -> explicit GPU upload -> GPU-resident #87/#88/#89 operations plus #90-derived accumulated-field resources -> #76 composition -> #77 production consumers -> downstream GPU physics -> explicit host output/validation boundary`

This is a residency and ownership map, not a universal scientific call stack.
#76 owns field-specific composition. #90 is a separate accumulated-field
interval/reset preprocessing branch, not an ordinary fourth interpolation
stage. The pinned scientific contracts/oracles own operation order.

## Buffer, transfer, and lifetime rules

### Host-to-device

H2D occurs only through an API that visibly creates or updates a GPU resource,
such as buffer initialization, `queue.write_buffer`, texture upload, or a named
resource upload method. Long-lived data is uploaded when its source changes, not
once per particle query.

### Device-to-host

D2H uses a named download/readback API and a staging/mapping step. It is allowed
for:

- final or scheduled output;
- explicit oracle/scientific validation;
- diagnostics requested by a host consumer;
- a documented genuinely host-owned downstream boundary.

An ordinary calculation or `encode_*` API must not hide D2H.

### Lifetimes

- `GpuContext`: process or simulation-run lifetime;
- particle buffers: simulation lifetime, replaced only for explicit resize or
  compaction ownership changes;
- canonical meteorology resources: source bracket/resource lifetime;
- derived runtime geometry: lifetime of its canonical geometry source;
- accumulated amount/rate resources: represented interval lifetime;
- uniforms/query parameters: timestep or dispatch lifetime;
- readback staging buffers: explicit output/validation lifetime only.

Owning structs must live until their last encoded/submitted consumer completes.
WGSL binding layouts and scientific field layouts remain issue-owned.

## Error and fallback policy

Initialization, validation, resource preparation, dispatch preparation,
synchronization, mapping, and readback failures must surface as an error or an
unambiguously failing process/gate. They must not become placeholder output,
unchanged data reported as success, or a successful skip.

`wgpu` validation, out-of-memory, and internal errors observed through error
scopes are failures. Buffer mapping errors and closed completion channels are
errors. Missing required adapters are errors in production and required GPU
gates.

A production path declared GPU-supported must execute its calculation through
the configured `wgpu` device. Silent substitution by a separate CPU
implementation is prohibited. An explicit diagnostic/reference CPU mode may
exist, but it is not GPU execution and must be recorded as `cpu_replacement`
with a failing GPU execution claim.

The current `FLEXPART_GPU_PBL_CPU=1` switch is an explicit migration/diagnostic
override, not a GPU-supported production result and not valid GPU evidence. #77
owns the final canonical consumer migration.

## Numerical policy

### Precision

- WGSL calculative kernels use `f32` unless an owning scientific contract proves
  that another representation is required and the selected `wgpu` targets
  support it.
- Host orchestration may use `f64` for coordinates, time conversion, oracle
  decoding, comparison, and error accumulation. Host precision does not change
  the precision claim of the shader result.
- Integer or fixed-point atomics are allowed when their scale, range, overflow,
  and decoding contract is explicit.

### Evaluation order

GPU and CPU/oracle evaluation order, contraction, interpolation implementation,
and transcendental rounding may differ. Bitwise equality is required only when
the owning issue proves it is normative (for example, a specified integer/RNG
contract). Otherwise the issue must declare finite absolute and relative
tolerances derived from its scientific proof obligation.

#91 defines no scientific tolerance values.

### Generic finite comparison

`gpu::compare_finite_values` implements the common elementwise rule. For oracle
value `o`, candidate value `c`, absolute tolerance `A`, and relative tolerance
`R`:

`absolute_error = |c - o|`

`relative_error = absolute_error / max(|c|, |o|)` (zero when both values are zero)

The value passes when `absolute_error <= A OR relative_error <= R`. The complete
comparison passes only when at least one value is supplied, lengths match, and
every value passes.

### Invalid and non-finite values

- host inputs required to be finite by a calculation contract are validated
  before dispatch;
- a NaN or infinity in either generic comparison input fails closed;
- empty comparison inputs fail closed because they cannot prove a numerical claim;
- a length mismatch fails closed;
- non-finite values may be scientifically meaningful only when the owning issue
  explicitly defines their encoding and comparison outside the generic finite
  comparator;
- signed zero compares equal under the generic rule.

## Determinism and reproducibility

Deterministic kernels must reproduce under the same normalized inputs, shader,
adapter/backend class, workgroup policy, and configuration to the tolerance
owned by their issue. Stochastic kernels must record all seeds/counters and use
the issue-owned ensemble/statistical proof; one trajectory is not a parity
claim.

Cross-backend bitwise identity is not assumed for floating-point kernels. Every
evidence record must retain enough provenance to identify the exact candidate,
inputs, shader, oracle, adapter, backend, and comparison policy.

## Machine-readable execution evidence

`gpu::GpuCalculationEvidence` is the repository-wide schema
`flexpart-gpu.gpu-execution-evidence`, version `1`. Its required structure is:

| Field | Required meaning |
| --- | --- |
| `schema.id`, `schema.version` | exact schema identity/version |
| `case_id` | stable issue-owned validation case |
| `candidate.implementation_id` | candidate calculation identity |
| `candidate.revision` | candidate source revision |
| `candidate.shader_sha256` | lowercase SHA-256 of executed WGSL/shader bundle |
| `candidate.input_sha256` | lowercase SHA-256 of normalized input/manifest |
| `execution.status` | `passed`, `failed`, or `skipped` |
| `execution.calculation_path` | `wgsl_device`, `cpu_replacement`, or `not_executed` |
| `execution.adapter` | name, backend, device type/ids, driver, adapter class, fallback request |
| `execution.failure` / `skip_reason` | explicit failure or skip cause |
| `oracle` | pinned implementation/revision plus executable and output SHA-256 |
| `comparison` | verdict, tolerance policy, lengths, error maxima, first failure |

Validation invariants are executable in `GpuCalculationEvidence::validate`:

- a passing execution requires `wgsl_device` and adapter provenance;
- CPU replacement cannot pass a GPU execution claim;
- a skipped run has no adapter/calculation and can never be a pass;
- a successful execution paired with an oracle requires a numerical verdict;
- required artifacts use content hashes;
- missing, malformed, non-finite, or length-inconsistent evidence fails closed.

`validate` checks structural honesty and therefore accepts a well-formed failed
or skipped record. A validation gate must call `require_paired_pass`, which
additionally requires successful WGSL execution, pinned oracle provenance, and
a passing numerical verdict.

Execution status and numerical verdict are intentionally separate. A shader can
execute correctly while failing its scientific comparison; neither fact may
hide the other.

## Preflight smoke computation

`gpu-preflight` and `gpu::run_preflight` verify shared infrastructure only. The
smoke path:

1. initializes the central `GpuContext`;
2. uploads three host integers into a uniform buffer;
3. executes `gpu_contract_smoke.wgsl` to compute
   `multiplicand * multiplier + addend`;
4. copies the result to a MAP_READ staging buffer;
5. reads it back and verifies the expected value;
6. records backend/adapter provenance and hardware/software classification;
7. surfaces initialization, scoped device, mapping, and mismatch failures.

`gpu-preflight --json-output <path>` writes
`flexpart-gpu.gpu-preflight` version `1` for both pass and failure. `--no-smoke`
records overall `status = skipped`, `smoke_test.status = skipped`, and an
explicit `skip_reason`; that record is capability information, not smoke
validation. The smoke is infrastructure evidence and never scientific parity
evidence.

The preflight record contains `schema`, overall `status`, normalized
`requested_backend`, an optional initialized `report`, and mutually exclusive
`failure`/`skip_reason` fields. The report contains the same schema/backend,
adapter provenance, relevant device limits, feature support, and the smoke
inputs, expected value, actual value, and status. `GpuPreflightRecord::validate`
enforces these pass/fail/skip invariants before the CLI writes the record.

## Pinned-oracle verification pattern

Every calculative scientific GPU ticket uses:

`pinned FLEXPART oracle -> normalized hashed inputs -> WGSL candidate on recorded adapter -> issue-declared comparison policy -> machine-readable verdict`

The owning issue must name the pinned revision, routine/binary/artifact,
invocation, normalized inputs, raw and decoded outputs, metrics, tolerances, and
required validation level. It then emits a valid `GpuCalculationEvidence`
record. Missing oracle output, missing hashes, missing adapter execution, skipped
execution, decoder failure, or incomplete metrics cannot produce a pass.

## CI policy

- Pure Rust schema/comparison tests run everywhere and require no adapter.
- The required software-WGSL job provisions Lavapipe, explicitly requests the
  fallback adapter, runs the real WGSL smoke/cases, and fails if the adapter is
  missing, execution is skipped, JSON evidence is absent, or any verdict fails.
- A developer unit test may return early when no adapter exists only if it makes
  no GPU-validation claim. Such a test is not evidence.
- Hardware-GPU validation/performance requires an explicitly provisioned runner
  and an evidence record classified `hardware_gpu`. No available hardware runner
  is `not run`, never a pass and never software performance evidence.
- An owning scientific issue may require hardware execution in addition to the
  software gate. That requirement and its runner/artifacts must be explicit.
- Required reports and raw logs are uploaded even on failure where CI permits,
  but artifact upload does not convert failure into success.

## Review and stop rules

A calculative GPU change must be rejected or returned for architectural review
if it:

1. creates a second runtime/device/queue ownership model;
2. downloads an intermediate only to upload it into another GPU stage;
3. hides D2H in an ordinary calculation API;
4. forces submit/wait between stages that can share an encoder;
5. silently replaces or skips a required GPU calculation;
6. reports a software adapter as hardware performance evidence;
7. embeds algorithm-specific tolerances in shared infrastructure;
8. claims parity without the owning pinned oracle and complete evidence.

If implementation reveals a new physics-relevant semantic, unnamed production
consumer, external data source/oracle, or ownership of another issue's contract,
stop expanding the current issue and record the dependency.

## Downstream ownership

- #87: horizontal meteorology GPU semantics;
- #88: vertical sampling GPU semantics;
- #89: instantaneous temporal GPU semantics;
- #90: accumulated-field interval/reset GPU transformation;
- #76: field-specific canonical 4-D GPU composition;
- #77: production-consumer migration and canonical handoff.

All of them must preserve this contract, and later calculative GPU tickets are
subject to the same rules.
