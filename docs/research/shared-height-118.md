# Shared metric-height research (#118)

The pinned `eta=no` ECMWF route first derives native column heights, remaps
fields to one common AGL height grid, then samples those transformed fields in
horizontal, vertical and temporal order. The direct experiment below establishes
this for regional non-polar, cell-centered U/V and boundary-column interface
omega/W. This is research evidence, not a production GPU implementation or a
claim of full FLEXPART parity. The #76/#171 differing-stencil rejection remains.

## Authority and exact production edges

All locations below refer exclusively to FLEXPART 11.1 revision
`c70586c2b7f5258850705325881c61f557ea9bd8`, selected by
[`reference/flexpart-11.1.json`](../../reference/flexpart-11.1.json).
The executable evidence hashes the actual inspected source files. Upstream
[`verttransform_mod.f90`](https://gitlab.phaidra.org/flexpart/flexpart/-/blob/c70586c2b7f5258850705325881c61f557ea9bd8/src/verttransform_mod.f90)
and [`interpol_mod.f90`](https://gitlab.phaidra.org/flexpart/flexpart/-/blob/c70586c2b7f5258850705325881c61f557ea9bd8/src/interpol_mod.f90)
are the source authority; comments describing doubled vertical resolution are
historical and do not override the active assignments.

| Pinned location | Actual behavior / direct edge |
| --- | --- |
| `getfields_mod.f90:246-287` | Reads each memory member with `readwind_ecmwf`, calculates parameters, calls `verttransform_ecmwf`, then `verttransform_nest`; sets `memtime` for the member. These are sequential lifecycle operations, not a call from interpolation to transformation. |
| `windfields_mod.f90:819-864` | Reverses hybrid half-level coefficients; appends an artificial surface center; assigns `nz=nuvz`, `aknew=akz`, `bknew=bkz`. Standard build does **not** double the height count. |
| `windfields_mod.f90:2612-2623` | Surface U/V are 10 m winds assigned to the artificial `z=0` center; surface T is T2m, surface q is copied from the lowest real model center. |
| `verttransform_mod.f90:94-137` | Local initialized `init=.true.` has Fortran's implicit SAVE lifetime; first call invokes `verttransform_init(n)` and clears it. Every member calls `verttransform_ecmwf_heights` and `verttransform_ecmwf_windfields`. The wrapper additionally handles polar/cloud fields, not exercised here. |
| `verttransform_mod.f90:296-353` | `verttransform_init`: first `ps>100000 Pa` point, scanning y then x; if none, fallback `(x=0,y=nymin1)`, **not** a maximum-pressure reduction. `height(1)=0`; other targets are this selected column's virtual-temperature hypsometric center heights. `nmixz` is the first target above `hmixmax`. |
| `verttransform_mod.f90:303-305,355-405` | Restart modes `ipin=1/4` read stored height levels and `nmixz`; other modes may write them. Reinitializing each bracket would violate the normal run-lifetime grid. Restart equivalence is source-traced only. |
| `verttransform_mod.f90:1975-2051` | Heights/pressure/density recomputed for every column from local ps, T2m/Td2m, center T/q and hybrid coefficients. Native `etauvheight` starts at zero AGL; `etawheight` has distinct interface construction and `pinmconv=dz/dp` uses the center-pressure stencil. |
| `verttransform_mod.f90:487-523,528-623` | Per-column UV and W bracket lookup; remap into `windfields_mod::uu/vv/ww`, T, PV, density, pressure and (METRE build) q/cloud fields at `height[]`. U/V center interpolation and W/interface interpolation use different native coordinates. |
| `verttransform_mod.f90:625-655` | W slope term uses horizontal differences of **native AGL center heights**, vertically weighted at the target, times remapped U/V and physical inverse-grid spacing. It adds no explicit `oro` term. Regional x/y boundaries skip it. |
| `verttransform_mod.f90:263-270,2169-2258` | Nests derive their own native geometry but remap to the same mother-grid `height[]`; no nest-specific target initialization. Nests are source-traced, not numerically accepted here. |
| `windfields_mod.f90:4084-4088,4186` | Native heights have `[x,y,level,memory]` storage; target `height` is one-dimensional. Transformed fields retain x/y, shared level and wind-memory dimensions. |
| `advance_mod.f90:338-372` | `adv_above_pbl -> interpol_wind`; public consumer samples already transformed memory fields. |
| `interpol_mod.f90:954-1029` | `interpol_wind -> find_ngrid -> find_grid_indices -> find_grid_distances -> find_time_vars -> find_z_level_meters -> interpol_wind_meter` in METRE build. No terrain/ASL argument or conversion. |
| `interpol_mod.f90:1651-1703` | Meter sampler finds vertical weights, loops over both times and both target levels: `hor_interpol` (4-D generic resolution), then `vert_interpol` per time, then `temporal_interpolation`. Finding weights earlier is not vertical sampling earlier. |
| `interpol_mod.f90:215-242,406-430,481-504,531-547` | Shared-height search; lower/upper clamps; linear weights; bilinear horizontal sum; linear time and vertical blends. `log_interpol=.false.` in pinned `par_mod`. |
| `initialise_mod.f90:384-405` | `kindz_to_z` converts ASL releases by subtracting bilinearly sampled terrain before storing AGL, then applies separate release bounds. This is not an ASL branch in `interpol_wind`. |

Thus the common grid is run-global across the mother grid, nests and memory
times, with provenance from the initially loaded mother member or restart file.
It is a common **AGL** coordinate, not a common ASL/geopotential surface.
Two columns at the same target AGL height can have different ASL heights.
Meteorological grid upper endpoints do not decide transport-domain physics (#180).

## Distinguishing direct experiment

[`input.txt`](../../fixtures/interpolation/shared-height-v1/input.txt) supplies
three real hybrid layers plus the artificial surface, four explicit bottom-to-top
half-level `(A Pa,B)` pairs, two different column profiles repeated in y, and two
times (0/3600 s). Local ps is 101500/90500 Pa initially and 100800/89500 Pa later;
terrain is 120/870 m ASL. T2m/Td2m and every center T/q and U/V are explicit.
The artificial surface obeys the reader's T/q assignment. Omega is explicit,
nonlinear, interface-staggered, Pa/s positive pressure-increasing; the top omega
is zero. This controlled input is not an ERA5 ingestion or eta-dot experiment.

The driver calls the **unmodified compiled** initializer, height routine,
wind-field transformation and public `interpol_wind` entrypoint. It passes the
native height outputs into the genuine `windfields_mod::etauvheight/etawheight`
arrays consumed by the transformation. It does not implement the transformation.
The three calls reproduce the applicable subset of the wrapper's actual path;
polar rotation, cloud processing, file reading and advection are not executed.
The linker cross-reference verifies initializer binding; #80's existing
disassembly verifier proves driver -> heights, driver -> windfields, driver ->
public sampler, and public sampler -> meter sampler.

The resulting target is `[0,1137.8990478515625,4574.7412109375,9521.84375] m AGL`.
It equals the first selected column at time zero, despite different neighboring
heights and changes at the second time. Nine queries include fractional x/y,
both target boundaries and strict interiors; three equal-height queries exercise
time endpoints and midpoint with different values. The y duplication makes this
a minimal distinguishing x-profile experiment while executing four-corner sampling.

The companion diagnostic computes (a) native vertical sampling per column then
horizontal blending, (b) native-level blending with bilinearly averaged invented
height geometry, and (c) sampling the **Fortran-produced** shared fields. Neither
(a) nor (b) is an oracle or an accepted interpretation of native geometry.
All three components distinguish both alternatives. For query 8 (`x≈0.7`,
`y=0.375`, `z=3371.8466796875 m AGL`, `t=3600 s`):

| Component | Direct pinned result (m/s) | Absolute difference from (a) | Absolute difference from (b) |
| --- | ---: | ---: | ---: |
| U | 19.94189453125 | 5.8075997527 | 6.1570823799 |
| V | 14.408058166503906 | 0.2808476997 | 0.1968243471 |
| W | 0.04986977577209473 | 0.0099831110 | 0.0139484387 |

Diagnostic (c) agrees with every direct result using #80's existing policy
`abs(a-b) <= 1e-6 m/s + 1e-5*max(abs(a),abs(b))`. No new tolerance is introduced.
Raw/decoded reproducibility is stricter: the accepted fixed-image output bytes
must reproduce exactly; source and research-tool identities must match.
Failure, missing evidence, non-finite values or absent linkage cannot pass.

## W, terrain and accepted scope

U/V originate at native centers (with the artificial ground populated from U10/V10).
The first/last **target** values copy the first/last native values even when native
top heights differ; strict interior targets interpolate, and U/V targets above a
column's native top copy its top values. This endpoint behavior is not generic
clamping at every local native top and must be reproduced explicitly.

W uses `wwh*pinmconv` on `etawheight`, then remaps to common `height[]`, then is
sampled. #30 owns normalized motion and native geometry; #80 owns this two-stage
interface semantics. A future GPU preparation stage must consume those values,
not multiply by `pinmconv` again. Top/bottom target W copy endpoint interfaces.
Center-staggered geometric W is **not** established as equivalent to this route.
Raw eta-dot remains #70's separate preprocessing contract.

The 2×2 regional grid is scientifically valid for the pinned boundary route:
all columns lie on boundaries, so the real slope branch skips correction even
with nonzero physical dxconst/dyconst. This extends #80 to differing native
columns but does not validate interior slope physics. Nonzero/differing `oro`
does not enter either height routine, field remap or meter sampler; pressure and
thermodynamics affect native AGL geometry. The output's ASL number is an explicitly
labelled equivalent `AGL + bilinear terrain` diagnostic. There is no claimed
direct ASL sampler execution. Release-conversion code is traced above; its
minimum/maximum clamps and terrain-changing particle transport remain separate.

Numerically accepted: cell-centered U/V, two instantaneous times, fractional
regional stencil, common AGL targets, interior and endpoint sampling; boundary
interface omega/W with #30/#80 normalization. Source-only observations: other
remapped fields, initialization fallback/restarts/nests/poles and interior W slope.
Do not enable those routes from this experiment.

## Architecture decision and lifecycle

Add a separate GPU-resident meteorology preparation stage upstream of #76/#171
for the accepted U/V route. Keep immutable #29 Snapshots and all per-column #30
geometry; derive new shared-grid resources. Do not average or replace native
geometry, and do not change the existing samplers' equations.

The target-grid owner lives for the run, records initial mother snapshot hash,
selected x/y, ps-selection rule, source geometry/transform identity, target bytes
and hash, reference AGL, artificial surface and target count. A resumed run must
restore explicitly verified target provenance or fail closed; reproducing the
pinned restart file format is not required by this decision. The initial target
can be selected from #30's matching native AGL column plus artificial zero; no
second hypsometric transform is justified. The future stage must prove this
handoff numerically and must select the column on device when selection is part
of its calculation. No silent per-bracket target regeneration is permitted.

Each prepared source owns transformed U/V f32 buffers indexed x-fastest by
`[target-level,y,x]`, per-source status, native snapshot/geometry identities,
target identity, time, field/unit/staggering and context token. The preparer
consumes explicit uploaded #30 geometry, native center U/V and required U10/V10
surface fields; missing surface data fails closed. It encodes remapping into the
caller-owned encoder through the existing `GpuContext`, without submit/wait or
intermediate readback. Validation statuses remain device-resident.

Preparation runs once per new met source against the run's target, retaining
both bracket members and resources until their final submitted consumers finish;
shared sources may be reused by adjacent brackets only with exact identities.
#76/#171 receive a typed transformed-field handoff and shared height resource;
they preserve existing horizontal -> vertical -> temporal sampling, time guards,
context, capacity/active-prefix, status and lifetime contracts. The consumer
integration ticket removes the specific nonuniform-native-height rejection only
for these proved transformed U/V resources. ASL callers must use an explicit,
validated terrain conversion; current terrain rejections stay until its own proof.

For C columns, N targets, F scalar fields and B retained sources, extra field
storage is `4*C*N*F*B` bytes plus `4*N` target bytes and statuses/metadata. Native
geometry remains allocated. With F=2 and B=2 this is `16*C*N` bytes, e.g. 16 MB
at C=10000,N=100 (decimal units), excluding native data. Preparation costs O(C*N)
with monotone bracket traversal (or a separately measured search policy), per
new source, rather than per particle. Binding/index limits must be checked;
unsupported size returns an error, never host sampling or hidden transfer.

W preparation/adoption is excluded from the U/V follow-ups: boundary-only W is
proved here, but a production grid with interior columns needs direct pinned
slope coverage and a verified combined U/V/normalized-interface handoff. Record
that narrow prerequisite separately, reusing #80. Other fields, face staggering,
clouds, nesting, poles, ETA mode and transport-top semantics remain fail-closed.

## Reproduction and retained evidence

Use the established `flexpart-fortran:latest` image built from
`docker/Dockerfile.fortran`. Clone the pinned reference with LF tracked bytes
(on Windows use `git -c core.autocrlf=false clone ...`). Then run:

```text
python scripts/interpolation/run_shared_height_oracle.py --checkout <pristine-flexpart> --output-dir target/shared-height-run-1
python scripts/interpolation/run_shared_height_oracle.py --checkout <pristine-flexpart> --output-dir target/shared-height-run-2
python scripts/interpolation/test_shared_height_oracle.py
```

The launcher records and executes the resolved immutable image ID, mounts the
oracle read-only and uses the existing #80 linker/symbol/disassembly capture. The shell checks actual commit
and clean status before and after, archives only tracked pinned source into a
fresh build directory (no stale objects), builds the complete model at
`FC=gfortran eta=no arch=x86-64 -j4`, then links the research driver at
`-O0 -fopenmp -mcmodel=large`. All routines remain unmodified. The normal model
main's build stamp stays in scratch. OMP single-thread controls follow the pin.

The versioned [raw output](../../fixtures/interpolation/shared-height-v1/output.txt)
and [decoded report](../../fixtures/interpolation/shared-height-v1/report.json)
retain native/shared values, queries, alternatives, raw-input decoding, source,
driver/harness/input/output/executable/object hashes, compiler/build profile,
resolved image and verified genuine call edges. Each fresh run retains full
compile/driver logs, executable, every linked object, symbol inventory,
cross-reference map and disassembly under the named target directory; volatile
build/executable hashes are recorded per run rather than required equal across
build paths. The frozen report is the inspected baseline, not a portable binary.
The evidence audit binds each executed f32 query height to its raw target bracket
and fraction and requires lower/upper boundaries plus strict interiors. CI
regenerates from the pristine pin and retains fresh artifacts on failure too.

No production source or shader changes belong to #118. The follow-up preparation,
consumer adoption and W slope proof are separate verification boundaries.

Published follow-ups: [#184 GPU U/V preparation](https://github.com/Grodahn/flexpart-gpu/issues/184),
[#185 GPU U/V consumer integration](https://github.com/Grodahn/flexpart-gpu/issues/185)
(depends on #184), and [#186 direct interior-W slope proof](https://github.com/Grodahn/flexpart-gpu/issues/186)
(prerequisite for general W preparation; reuses #80). #184 initially supports
the directly tested initial high-pressure column at (0,0). #185 ends at the
#76/#171 U/V query handoff: the existing forward/backward drivers require a full
U/V/W vector, so their adoption remains with the production-consumer owners
after a separately validated W route exists. #186 requires independent x-only,
y-only and zero-slope diagnostic controls around the genuine production call.
Their contracts retain the scope and fail-closed boundaries above.

## Interior W handoff established by #186

The [#186 direct interior experiment and architecture decision](interior-w-186.md)
supersede the interior-W evidence gap identified above for its bracketed regional
interface-omega route. The original 2x2 fixture still proves only boundary W;
its scientific values and acceptance scope remain unchanged. The new 3x3,
two-time fixture executes four calls to the same pinned routine and independently
distinguishes X/Y corrections and their sum. The correction multiplies native
center-height slopes by already remapped U/V, after normalized-interface W
remapping, with no explicit terrain-slope term.

W GPU preparation can reuse #30 normalized interface motion/geometry and #184
prepared U/V with identical source and target provenance. It must apply the
correction once before #76/#171 sampling. Separate W adoption owns the complete
vector/status/lifetime handoff, coordinated with #185; production consumers
remain with their owners. Center-W, unbracketed target heights, global/nested/
polar/unsupported initializer paths and #180 remain outside this numerical proof.
The linked #186 decision contains the full source ranges, numerical controls,
resource architecture, normalization lineage and reproducibility contract.

W follow-ups after successful #186 proof: [#188 preparation](https://github.com/Grodahn/flexpart-gpu/issues/188) and [#190 adoption](https://github.com/Grodahn/flexpart-gpu/issues/190), coordinated with #184/#185.
