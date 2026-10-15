# Shared-height W preparation (#188)

`gpu::shared_height_w` prepares normalized interface W on #184's existing AGL
target, then applies the pinned regional interior correction from #186.
This is preparation-only evidence. Query adoption (#190), U/V query adoption
(#185), production consumers (#112-#115) and transport boundaries (#180) remain
separate. Existing #76/#171 nonuniform and ASL rejection stays unchanged.

## Inputs, identity and resource lifetime

`SharedHeightWSource::upload` accepts an immutable canonical Snapshot, its opaque
#30 runtime with `omega_interface_flexpart11_pinmconv_v1` normalized interface
motion, and `Arc<PreparedSharedHeightUv>` from that exact Snapshot, geometry,
time, target and context. It uploads only [interface AGL height, normalized W].
The existing #184 native-center buffer supplies center heights including zero;
its U10/V10 handling, shared target, remapped U/V and statuses are reused.
No geometry reconstruction, omega conversion, target selection or sampling is
performed by the W stage. Raw omega and prepared/corrected W are not upload
argument types; geometric identity/center-W normalization is explicitly rejected.

`SharedHeightWPreparer::prepare` encodes two compute passes in the caller's
encoder: interface remap, then same-native-center-index neighbor slope addition.
The pinned source ranges are `verttransform_mod.f90:489-545,615-655` and
`windfields_mod.f90:630-631`. Endpoints copy normalized native interface endpoints;
regional horizontal boundaries skip the correction. The X latitude factor and
independent physical inverse dx/dy are calculated in WGSL. No terrain term is
added. Normalization and correction each occur once.

W is immutable f32, m/s positive upward, x-fastest `[target-level,y,x]`. The W
owner retains its interface upload, exact prepared U/V owner, native center
upload, run target and context token. Retain it through the final submitted
consumer. Metadata records normalized-motion lineage, interface-input hash,
horizontal hash, U/V metadata hash, correction identity, source/geometry/time,
target and device identity. Adjacent brackets can reuse W with identical
identities and the same prepared U/V owner. A re-upload against that owner is
accepted; a different U/V owner or target is rejected even for identical bytes.

Consumers must check the target status, every required U/V column status, and
every required W column status. W status 0 is pending, 1 is remapped only, 2 is
fully corrected, 3 is invalid/dependency failure, and 4 is nonfinite arithmetic.
Only 2 is accepted. The fixture consumer rejects the entire source when any
column/dependency fails. There is no internal submit, poll, wait or readback.
The pipeline uses at most eight storage buffers per stage, with checked sizes,
index/dispatch limits and uniform capacity.

## Numerical verification

The production test compares all 72 full-correction values (two times, nine
columns, four levels) to genuine #186 SHARED W under the unchanged finite rule
`abs(a-b) <= 1e-6 + 1e-5*max(abs(a),abs(b))`, in m/s.
It retains all 288 full/X/Y/zero device values. At both interior target levels
of both source times, each X-only-minus-zero and Y-only-minus-zero is compared
to its direct Fortran difference under that same policy. Both directions must
be independently nonzero, same-sign and additive. Raw diagnostic X/Y final W
can have cancellation; their proof obligation applies to the differences from
zero. Zero-remap comparisons and #186's omitted/double-normalization diagnostics
separately establish normalization exactly once. Omitted/doubled correction
must fail the frozen full result. Endpoints and horizontal boundaries are
exactly unchanged across controls; endpoint copies equal the uploaded normalized
f32 values exactly, with the Fortran comparison using the velocity policy.

The X latitude factor is the pinned cosine. Some DirectX native cosine
implementations exceed the final velocity budget near cancellation. The WGSL
therefore evaluates the even cosine Maclaurin polynomial through degree 14 on
the accepted `[-pi/2,pi/2]` interval. Its exact-arithmetic truncation error is
less than `7e-11`; actual f32 device results remain subject to the same oracle
policy. This is a numerical evaluation choice, not another atmospheric equation
or changed tolerance. Invalid/nonpositive/nonfinite physical factors fail closed.

Diagnostic controls return `DiagnosticSharedHeightW`, which cannot become a
production `PreparedSharedHeightW`. Zero factors never enter accepted production
geometry. The production method always uses both physical factors.

## Reproduction

```text
python scripts/interpolation/test_interior_w_oracle.py
python scripts/interpolation/run_shared_height_oracle.py --interior-w --checkout <pristine-flexpart> --output-dir target/interior-w-first
python scripts/interpolation/run_shared_height_oracle.py --interior-w --checkout <pristine-flexpart> --output-dir target/interior-w-second
python scripts/interpolation/compare_interior_w_builds.py --first target/interior-w-first --second target/interior-w-second --output target/interior-w-reproducibility.json
```

Set `FLEXPART_GPU_INTERIOR_W_ORACLE=target/interior-w-first/report.json` to compare
to the freshly rebuilt oracle. Run `cargo test --test shared_height_w_gpu --
--nocapture --test-threads=1`, followed by
`python scripts/interpolation/check_shared_height_w_gpu.py target/ci-gate/interior-w-gpu/comparison.json --hardware`.
Use `FLEXPART_GPU_SOFTWARE=1` and `--software` for a real software-WGSL adapter.
Missing devices fail the test; they cannot produce passing evidence.
`FLEXPART_GPU_INTERIOR_W_EVIDENCE` chooses a separate evidence directory.

Evidence includes exact serialized source/native/U/V/grid preimages, native
interface lanes, raw device W and U/V, target bytes, all direct references,
individual slope contributions, differences, hashes, linked-oracle provenance
and `GpuCalculationEvidence`. The auditor recomputes comparisons and input hashes;
mutation tests reject absent/corrupt rows, directions, provenance and execution.
Full logs remain with the owning run. The software CI adds this test/validator to
#177's meteorology domain and immutable aggregate evidence; the technical gate
also uses its two freshly reproduced direct oracle builds.

Unsupported routes remain center-W, raw omega/eta-dot, ASL target conversion,
unbracketed strict interior targets, initializer fallback/restarts/nests,
global/polar grids, invalid spacing, malformed/nonfinite geometry or values,
mismatched source/time/context/target/provenance, failed dependencies and device
limits. The accepted initializer remains #184's first-load regional mother
(0,0) with ps>100000 Pa. There is no production consumer migration or full
FLEXPART parity claim.
