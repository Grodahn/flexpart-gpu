# Typed Hanna -> split Langevin handoff (#148)

Issue #148 owns this API-boundary refactor. The Langevin stage now exposes
`pub(crate) gpu::encode_update_particles_turbulence_langevin_gpu_with_hanna_output_and_kernel`
through the [canonical facade](../../src/gpu/mod.rs); its implementation stays
in the [Langevin owner](../../src/gpu/langevin.rs).

## Caller boundary and ownership

Before, [forward separated/validation operators](../../src/simulation/timeloop/forward/operators.rs)
supplied `&hanna_output.buffer` and `hanna_output.particle_count()` independently;
the [backward separated driver](../../src/simulation/timeloop/backward.rs) supplied
`&self.hanna_params_output.buffer` and `self.hanna_params_output.particle_count()`.
Both now borrow the existing `&HannaParamsOutputBuffer` as one producer-owned
argument. Context, particle buffers, step, key, base counter, prepared kernel
and caller-owned encoder are unchanged.

The typed stage function extracts the raw handle/count and immediately delegates
to the established public raw encoder. Its body creates no resources and does
not encode, submit, wait, copy or read back independently. The existing raw body,
Hanna output ownership/public fields/methods, and standalone APIs remain intact.
No WGSL or scientific calculation changes; there is no deviation from the
existing Fortran-derived equations or numerical policy.

## Preservation checks

- [Hanna prefix regression](../../src/gpu/hanna.rs): capacity=8, active=1/3/8,
  substeps=0/4. The typed prefix is compared byte-for-byte with a raw prefix on
  the same WGSL device, alongside the existing full-capacity raw control and
  original tolerances. Particle and Hanna inactive-tail sentinels remain intact.
  Counters remain `base + active * (1 if substeps <= 2 else 2)`.
- The same six retained `HANNA-PREFIX-139` records (adapter, normalized input
  hashes, inputs and outputs) match the pre-change audit-baseline run exactly.
- [Bounds regression](../../src/gpu/hanna.rs): typed and raw logical ranges
  0/1/2 reject active=3; physical storage 4/188 bytes rejects the required 192
  bytes, even with NaN dt. Logical coverage precedes storage, then timestep
  validation. Valid storage with NaN dt fails timestep validation; an empty
  prefix retains the early return and counter. The empty encoder finishes
  without a binding/submission. Raw malformed-buffer checks remain explicit.
- [Source-order regression](../../tests/forward_timeloop.rs) substitutes the
  canonical typed symbol in the same exact-once ordered operator list, and
  rejects raw handoff reconstruction at both enumerated callers.
- Full forward/backward targets and the existing software-WGSL compaction
  0/1 x validation 0/1 matrix preserve order, forcing/mass outputs, deferred
  readback, cached host state and preparation/retry behavior. Missing adapters
  cannot satisfy the required prefix/bounds or driver evidence markers.
- Existing compact `PBL-STABLE-004` runner retains pinned inputs and raw
  candidate/oracle output. Its input audit fails closed before comparison on
  existing declared meteorology differences; no parity verdict is inferred.

See [focused verification](test-map.md#pbl-turbulence) and the unchanged
[software-WGSL workflow](../../.github/workflows/software-wgpu.yml).
Local transcripts, preflight adapter evidence, baseline equality result and
working-set measurements are retained under `target/issue-148/`; paired raw
outputs/manifests remain under the existing corpus/agent-validation paths.
Local validation: Hanna 3/3, Langevin 4/4, all four forward matrix combinations
8/8 each, backward 2/2; full serialized Cargo suite 713 passed, 0 failed/ignored.
Formatting, Clippy (existing warnings), both navigation checks (26 regressions)
and audit-document link checks pass. Preflight records Microsoft Basic Render
Driver / DX12 / CPU as `software_wgsl`, with actual smoke output verified.

The compact PBL run exits 1: ten WGSL candidate seeds and the pristine pinned
oracle execute, then six input-audit checks reject the existing candidate/oracle
grid, vertical-coordinate and transformation differences with
`INPUT_EQUIVALENCE_NOT_DEMONSTRATED`. Scientific verdict is `NOT_EVALUATED`;
comparison/provenance stages do not run. Case/Fortran fixtures, scientific
reference profiles, input-equivalence implementation and audit code are unchanged
against parent `b089dfa`; this is the existing #52 fail-closed requirement, outside
#148. Raw outputs, oracle build identity and complete failure summary remain in
the original run directory. No gate, tolerance or scientific input was changed
to obtain a pass. The PR records final pushed-head CI outcomes.

## Representative agent working set

Use #131/#148's exact source-selection convention: `src/gpu/mod.rs`,
`src/gpu/hanna.rs`, `src/gpu/langevin.rs`, including colocated tests; count UTF-8
bytes with LF and source lines. Shared contracts, shaders, fixtures and separate
integration tests are excluded in both measurements. The baseline is
`b430aa5053539c985435e9296faa3759a888f7ab`.

| Measure | Audit baseline | After #148 |
| --- | --- | --- |
| Selected sources / stage owners | 3 / 2 | 3 / 2 |
| Source lines | 2,126 | 2,274 |
| UTF-8 bytes with LF | 84,308 | 90,121 |
| Existing top-level public stage types/functions | 13 | 13 |
| Hanna handoff arguments reconstructed by simulation | raw buffer + independent logical count (2) | producer-owned typed output (1) |
| New crate-visible stage entry / facade alias | 0 / 0 | 1 / 1 |
| Relevant imported encode functions per caller | 1 raw | 1 typed |

Reproduce by reading the selected paths with `git show <revision>:<path>` (or
normalizing current files to LF), summing `len(text.splitlines())` and
`len(text.encode("utf-8"))`; count stage declarations matching
`^pub (async )?(struct|enum|fn)` and the one new `pub(crate) fn` separately.
Files grow because the wrapper and preservation tests add evidence. The agentic
benefit is the explicit typed boundary: simulation consumes one producer-owned
argument through the facade and no longer reconstructs raw layout/count rules.
It is not a claim of smaller files, faster execution or scientific improvement.

Provenance marker: **GPT6.1 Sol**
