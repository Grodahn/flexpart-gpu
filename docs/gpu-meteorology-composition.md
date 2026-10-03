# Canonical GPU meteorology composition (#76)

`gpu::meteorology` composes the existing #87–#90 resources and encode APIs under
[GPU_CONTRACT.md](GPU_CONTRACT.md). It adds no shader, interpolation equation,
provider decoder, physics consumer migration or tolerance policy.

## Public handoff and ownership

1. Initialize `MeteorologyCompositionKernels::new(&ctx)` once, outside encoding.
2. `CanonicalGpuField::upload(&ctx, field_id, snapshots, runtimes)` validates
   complete canonical snapshots and explicitly uploads persistent resources.
   Supply one runtime slot per snapshot: model fields require their exact #30
   runtime; surface/class fields require `None`.
3. `source.prepare_sample(&ctx, request)` validates field-specific coverage and
   allocates query/intermediate/output owners. Requests use canonical grid
   coordinates; geographic mapping remains the existing #87 control plane.
4. `plan.encode(&ctx, &kernels, &mut encoder)` records the applicable stages and
   returns `EncodedMeteorologySample`. It never submits, polls, waits, maps or
   reads back. Bind `handoff.values` directly to a later GPU consumer. The caller
   owns scoped device errors, submission and completion; discard the encoder
   after any encoding error.
5. Keep the context, source, runtime geometry, kernels and plan alive through the
   last submitted consumer. Readback belongs to its explicit host boundary.

Source and kernel owners borrow the exact `GpuContext`; model sources also borrow
immutable #30 runtime geometry. This prevents mixing independent runtime instances
even when wgpu's per-instance device IDs compare equal. Plans borrow their source
and privately own their intermediates. Only a successfully encoded plan exposes a
handoff. Encoding is not a completed-execution verdict.

Outputs are STORAGE/COPY_SRC `f32`: one scalar, or canonical class-index lanes.
Metadata preserves field identity, output unit/sign, grid and source staggering,
query/time/height reference, resolved AGL heights, original timestamps and snapshot
hashes, instantaneous bracket/application/weights, interval/reset identity, and
complete #30 geometry/motion provenance. Interface W retains its source interface
staggering although the remapped intermediate uses `[ground, model levels]`.
Source timestamps and snapshot hashes are aligned one-to-one: instantaneous
handoffs retain the selected bracket; accumulated handoffs retain the complete
validated observation sequence needed to identify delta/reset history.

## Field-specific paths

| Source class | Encoded path | Explicit selection |
| --- | --- | --- |
| Instantaneous surface scalar supported by #89 | #87 for each bracket member -> #89 | `Instantaneous(RequestedSampleTime)` |
| Instantaneous model-center field | #87 for physical levels of each bracket -> #88 -> #89 | Instantaneous; required AGL/ASL height |
| Center-staggered normalized W | Model path, with values from the exact #30 motion owner | Instantaneous; required AGL/ASL height |
| Interface W supported by #80/#88 | #88 remap -> #88 particle-height sample per bracket -> #89 | Instantaneous; single column only |
| Accumulated precipitation | #90 interval/reset transform -> selected product plane -> #87 | `AccumulatedInterval`, exact endpoints, distinct amount/SI-rate/mm-h-rate quantity |
| Canonical interval-total precipitation | #87 only; no inferred rate conversion | `IntervalTotal`, exact endpoints |
| Canonical interval-mean surface flux | #87 only; no instantaneous blend | `IntervalMean`, exact endpoints |
| Static scalar/class field | #87 for each class lane | `Static`; no vertical/time stage |

Model composition follows pinned FLEXPART `interpol_mod.f90:1651–1703`:
horizontal sampling precedes vertical sampling in each source member; temporal
sampling consumes the spatial results. Physical bottom-to-top order is extracted
through #88's runtime APIs. W remapping stays upstream of particle sampling.
Accumulation remains a separate preprocessing branch, never a fourth dimension.

Existing typed stage APIs allocate private copy destinations that are completely
written by encoded producers before consumption. Horizontal level outputs populate
the vertical lane; spatial outputs populate the temporal bracket; #90 products
populate the selected horizontal plane. All transfers are device-to-device copies
in the caller's encoder. Query/resource preparation is explicit and separate from
`encode`. There is no intermediate host materialization or CPU sampled value.

Accumulated input/output resources have observation-sequence lifetime and can be
reused across queries. Encoding currently recomputes their deterministic #90
transformation before the selected consumer. No performance claim or submission
scheduler is introduced.

## Fail-closed boundaries and follow-ups

`MeteorologyCompositionError` wraps canonical and stage errors and identifies
missing, incompatible and unsupported combinations. Private resources, full
canonical validation and exact context/source identity guard the boundary.
There is no CPU fallback. Existing #87 horizontal domains, #89 endpoint/coverage
policy and #88 vertical boundary behavior are preserved; no new clamp is added.

Amounts, rates, static fields and interval means cannot enter the instantaneous
branch. Missing/ambiguous interval coverage and malformed resets fail closed.
The following paths require their own contracts and remain explicit rejections:

- [#118](https://github.com/Grodahn/flexpart-gpu/issues/118): fractional model
  stencils with differing active #30 height profiles, or differing terrain for
  ASL. No averaged profile or arbitrary column is selected. Integer/local-column
  and equal-profile queries are supported.
- [#119](https://github.com/Grodahn/flexpart-gpu/issues/119): precipitation
  rate-grid time sampling, including `interpol_rain`'s frozen `dt/3` behavior.
  Explicit interval products are not reported as final wet-deposition forcing.
- [#120](https://github.com/Grodahn/flexpart-gpu/issues/120): instantaneous
  surface-flux eligibility. Canonical schema permits it, but #74/#89's temporal
  policy rejects it. Interval means are supported separately.
- Multi-column interface W remains rejected by #80/#88 because the supported
  oracle omits slope correction; this existing limitation is unchanged.

#112–#115 and #77 own adoption; no production consumer is migrated here.

## Verification and evidence

```text
cargo test --test meteorology_composition -- --test-threads=1
```

This target requires actual adapter execution and fails if no adapter exists.
The device test covers surface endpoints/interior, model AGL/ASL and inherited
bounds, center/interface W, a later bracket in a three-member series,
leading/delta/reset accumulated products, interval totals,
interval means and static scalar/class fields. All producers and minimal downstream
device-copy consumers share one caller submission. Only final consumer outputs are
read back. A source audit rejects host-completion calls throughout composition and
its called stage encode bodies. Negative tests cover metadata, dimensions, coverage,
geometry, representation and independent-context/resource incompatibility.

`target/ci-gate/meteorology-composition/report.json` records candidate revision,
composition/test source hashes, existing shader hashes, adapter classification,
actual stage order/source indices, device-copy counts, handoff identity, expected
and final values, existing stage comparison policies and pass/fail results.
Per-case normalized inputs, exact serialized source snapshots, and hashed handoff
metadata are retained beside it. Tests assert complete stage/source/plane records
and exact timestamp/hash alignment. CI independently checks expected records,
source and handoff hashes, shader identity, copy counts, units and numerical
comparison under the stage-owned policies. Skipped execution
cannot pass. Numerical failures retain per-case evidence and fail the target.
The aggregate report is invalidated before device setup, so failed reruns cannot
leave a previous passing aggregate verdict.

Stage science remains owned by the existing tests and artifacts:

| Owner | Reused check | Evidence |
| --- | --- | --- |
| #87 | `cargo test --test horizontal_gpu` | `target/horizontal-gpu-evidence.json` |
| #88 | `cargo test --test vertical_gpu` | `target/ci-gate/vertical-gpu/*.json` |
| #89 | `cargo test --test temporal_gpu` | `target/temporal-gpu-evidence.json` |
| #90 | `cargo test --test accumulation_gpu` | `target/ci-gate/accumulation-gpu/*.json` |

The software-WGSL workflow runs this target after the existing stage gates,
checks current/complete evidence and uploads the reports. This is composition
integration evidence, not a new per-stage oracle framework or full FLEXPART parity
claim. Corpus/ETEX consumer validation belongs to subsequent migration contracts.
