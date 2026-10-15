# Direct transport upper-boundary proof (#180)

This closes the direct transport experiment left outstanding by the latest
#180 research comment. It reuses #118's initial/native profiles and the established
Fortran image, build and linker infrastructure. It does not repeat #118/#186's
remapping proofs or change any production Rust/WGSL behavior.

## Authority and traced ownership

All source locations refer to the clean revision selected by
[`reference/flexpart-11.1.json`](../../reference/flexpart-11.1.json),
`c70586c2b7f5258850705325881c61f557ea9bd8`. The experiment hashes every tracked
archived source other than the build-stamped, unlinked normal `FLEXPART.f90` main.
The inspected `advance_mod.f90` SHA-256 is
`7c11a52ad5e0a4b4de36282d72fd4cbebe47e110a30c3609fda3a3ac266d2d68`.

| Pinned source | Actual edge or ownership |
| --- | --- |
| `windfields_mod.f90:708-716,4084-4088,4186` | Regional ECMWF `nxmax=nxfield`; the global cyclic extension differs. Native `etauvheight/etawheight` have column/member dimensions. `height(nzmax)` is a separate one-dimensional f32 array. |
| `verttransform_mod.f90:94-137,296-353` | Initialized local `init` has SAVE lifetime. First mother-member transformation initializes the target once. `verttransform_init` scans y then x for the first `ps>100000 Pa`; normal target starts at zero AGL and follows that column's thermodynamic center heights. Restart reads saved levels; it does not select a new column. |
| `getfields_mod.f90:246-287`; `verttransform_mod.f90:487-655` | Each source member/column gets native geometry and transformed fields on the retained target. Transport samples those fields. See the existing [#118 proof](shared-height-118.md) and [#186 proof](interior-w-186.md). |
| `advance_mod.f90:139-151,182-242,338-444` | `advance` resets `nstop`; initializes interpolation/PBL state; above PBL, `adv_above_pbl` samples current wind, computes real settling if enabled, sets positive `idt=abs(lsynctime-itimec+itime)`, and updates z with `dt*ldirect`. Particle z is f64, sampled velocities and many increments are f32. |
| `advance_mod.f90:287-333` | Horizontal update precedes the first `pushpartdown`. Correction requires a full synchronization step, advanced signed time within memory coverage, and the same selected grid. A boundary-set `nstop` is **not** an immediate return before this correction. |
| `advance_mod.f90:664-779` | `petterssen_corr` saves old wind, samples `itime+idt*ldirect` at the already moved/adjusted predictor, recomputes settling, and adds half the wind difference times the signed step to that predictor. Lower reflection and horizontal correction precede another `pushpartdown`. This is not generally equivalent to recomputing from the original z after predictor placement. |
| `advance_mod.f90:616-655` | In METRE mode, PBL substeps also call `pushpartdown` after vertical mean transport, before testing PBL exit/lower reflection. Adaptive PBL physics is source-traced here, not numerically validated by this above-PBL experiment. |
| `advance_mod.f90:872-900` | In METRE mode the condition is inclusive: `part%z >= real(height(nz),dp)`. With `ldirect=-1 && lsettling`, set `nstop` and retain z. Otherwise `set_z(height(nz)-100.*eps)`, where `eps=nxmax/3.e5` is f32. The flag test does not depend on actual nonzero settling velocity. ETA instead uses `zeta/uvheight`; it is excluded. |
| `timemanager_mod.f90`, call to `advance` and following termination loop | `nstop` is a particle fate flag: skip subsequent processing and terminate flagged live particles after the parallel loop. It is not a failed meteorology batch. |

The transport ceiling is exactly the endpoint of the **run-owned shared AGL
target** on this route. It is not a local native highest center, a maximum over
native columns, an interpolated column top, terrain ASL, or a newly defined
independent physical ceiling. Different native tops do not cause a different
transport ceiling. The source-only restart, fallback-column, nest, polar/global
and ETA paths are not promoted to accepted production routes by this result.

## Genuine executable experiment

The [driver](../../scripts/transport/top_boundary_oracle.f90) calls the actual
initializer/native-height routine, private `pushpartdown`, and public `advance`.
It uses the unchanged [#118 input](../../fixtures/interpolation/shared-height-v1/input.txt).
One independent synthetic run copies the initial selected thermodynamic profile
to both columns/members as an equal-height control; the other uses all four
original profiles. Within either run, the initializer is called only for member 1.
The paired source members are assigned times `0, direction*3600 s`.

To isolate transport, explicitly controlled **already shared-grid** winds are
provided directly to the real sampler. This is a routine-level synthetic oracle,
not an ingestion/remap/complete meteorology-to-output simulation. U moves the
particle from x=0.25 to approximately x=0.75 over 600 s; y=0 is a legal regional
boundary. The x=0/x=1 native column weights therefore change. W is constant in
height; scenario 8 varies it independently with x and source time. The exact
forcing construction, f32 rounding and all species/state initialization are
retained in the hashed driver. No analytical implementation replaces transport.

The two native geometries, both directions, and start epochs `0` and
`direction*1200 s` exercise current and advanced source selections. The nine
public-advance scenarios are: stationary below; stationary at; stationary above;
predictor crossing followed by inward correction; corrector-only crossing;
predictor crossing with unchanged wind; crossings at both stages; independent
x/time variation; and real spherical-aerosol settling. All start heights except
the private predicate probes are f32-representable. At/above starting states
exercise the routine's defined boundary handling, not release-file acceptance.
The private routine probes incoming z at `top-0.0001`, `top`, `top+0.0001` metres
in f64 for both directions, with and without the settling flag.

Real settling uses the original `get_settling`, sphere `ishape=0`, density
2500 kg/m3, pinned `dquer=10` (converted by the routine from micrometres),
Cunningham factor 1, initial iterative estimate -0.01 m/s, shared T=280 K and
air density 1 kg/m3. This proves transport interaction for that controlled
case; it does not establish general settling parity or change species units.
Turbulence, dry deposition, nests and restart are disabled.

There are **24 private-boundary cases and 72 public-advance cases**, plus eight
geometry records. The full raw rows and decoded observations are retained in
[the fixture](../../fixtures/transport/top-boundary-v1/report.json) and
[raw output](../../fixtures/transport/top-boundary-v1/output.txt).

| Direct observation | Executed result |
| --- | --- |
| Shared target endpoint | `9521.84375 m AGL` in both synthetic runs, both members and both columns. |
| Unequal native endpoints, members 1/2 and columns 0/1 | `9521.84375`, `8526.822265625`, `9618.2646484375`, `8612.1044921875 m`. All corresponding transport rows exactly match the equal-height control. |
| Ordinary at/above boundary, either direction | `9521.8427734375 m`, `nstop=false`. The f32 subtraction is rounded before assignment to f64 particle z. |
| Private probe just below | `9521.84365000000071 m` unchanged, even though conversion of this f64 value to f32 would round to the top. |
| Backward + settling flag at/above | `nstop=true`; incoming z retained by `pushpartdown`. Forward + settling uses ordinary placement. |
| Predictor crosses, correction points inward, start epoch 0 | Final `9515.8427734375 m`. Original starting z was `9520.84375 m`; a final-only trapezoidal clamp would miss the first placement. |
| Corrector-only crossing and repeated crossings | Final ordinary placement `9521.8427734375 m` for both signs/epochs. |
| Genuine backward settling, both epochs | Settling `-0.00775631330907344818 m/s`, returned z `9526.4877724647522 m`, sticky `nstop=true`. Correction still runs; the stop is not erased. |
| Genuine forward settling | Settling `-0.00775631284341216087 m/s`, returned z `9517.18019642494619 m`, `nstop=false`. |

The raw output SHA-256 is
`b10df540cc86ebb1afaccba6f9178deabbf4af1135c76adfc582868e2c957be2`.
Source/executable/driver/object identities and exact scalar observations belong
to the machine-readable reports; build-path-dependent binaries need not hash
identically across fresh builds. Epoch-dependent final low bits are retained,
not hidden behind a new tolerance.

## Reproduction and audit

Use the existing `flexpart-fortran:latest` image from
[`docker/Dockerfile.fortran`](../../docker/Dockerfile.fortran) and an independent
clean checkout of the pinned reference, with LF tracked bytes:

```text
python scripts/transport/run_top_boundary_oracle.py --checkout <pristine-flexpart> --output-dir target/top-first
python scripts/transport/run_top_boundary_oracle.py --checkout <pristine-flexpart> --output-dir target/top-second
python scripts/transport/compare_top_boundary_builds.py --first target/top-first --second target/top-second --output target/top-pair.json
python scripts/transport/test_top_boundary_oracle.py
```

The launcher resolves and executes an immutable image ID, mounts the pin
read-only, archives tracked sources into a new scratch tree, and uses the
manifest's full `FC=gfortran eta=no arch=x86-64 -j4` build and single-thread OMP
environment. It reuses #80's original link/cache helper. The driver is compiled
at `-O0 -fopenmp -mcmodel=large`; all reference routines remain at the pinned
`-O3` profile. No upstream source is edited. Upstream's timestamp edit to the
scratch normal main is excluded from linking, as in #118/#186.

Private access uses `objcopy --globalize-symbol` on the scratch `advance_mod.o`.
The original object is retained; .text bytes must remain identical, and repeating
that one operation on the original must reproduce the **entire** linked object.
ELF symbol-table reordering makes inverse localization an unsuitable byte test.
Both ELF inventories/disassemblies, original/exported object hashes, every linked
object and the executable remain in each run's build directory. GNU `-O3` inlines
the private correction and boundary actions into public `advance`; the audit
therefore verifies the actual public sampler/advanced short sampler/settling
call instructions rather than claiming nonexistent private call instructions.
It also verifies driver calls and the original-object linker binding.

Each run checks pristine revision/status before and after execution; retains
input/raw output, compile/run logs, compiler, image, manifest/profile, all
unmodified archived source hashes, tool/driver hashes, executable hashes and
decoded records. The paired auditor checks all retained artifacts and requires
distinct scratch-build origins. Two relocated copies of one build do not pass.
The frozen [paired proof](../../fixtures/transport/top-boundary-v1/reproducibility.json)
retains both inventories. Fixed-image raw output and decoded observations must
reproduce **exactly**. This is regression reproducibility, not a newly invented
scientific error tolerance. `--record` only records inspected evidence; it never
reports regression PASS. Missing baseline/evidence, changed sources, nonfinite,
duplicate or missing rows, failed linkage, absent actual settling or missing
direction/epoch coverage fail closed.

The [dedicated CI gate](../../.github/workflows/transport-top-oracle.yml) rebuilds
and checks two fresh executions and retains artifacts even on failure.

## Finite GPU follow-up contract

One follow-up owns **shared-target transport boundary adoption and its action**
at the existing #173/#112 handoff. It must reuse
`SharedHeightTarget` / `SharedHeightTargetIdentity` from #184 and the #185 query
handoff, with the complete matching prepared U/V/W route from #188/#190 before
enabling differing native geometry. It must not build a second target initializer,
wind sampler, geometry reduction or CPU fallback.

A proposed typed `SharedHeightTransportBoundary` lends the existing target
height/status buffers and exact context/target identity, target count, regional
mother-grid `nxmax` provenance and run direction/settling-enabled flag. Its owner
retains the target through all submitted consumers; its lifetime is the run,
while prepared field members retain source/bracket lifetimes. Both times and
all current/predictor/corrector positions use the same endpoint. Host audit
expectation bytes cannot replace device target/status consumption. Validate
AGL, artificial ground, accepted initializer/selection, level count, target
hash/identity, native geometry, full field/vector/time identity and context.
For the accepted regional ECMWF route, bind `nxmax` to validated mother-grid
`nxfield`; do not silently reuse an unrelated legacy wind dimension or constant.

The GPU must apply the inclusive top action after predictor movement **before**
advanced-time sampling, then apply the correction to that adjusted predictor
and apply the action again. Preserve signed time, actual `lsettling` flag and
sticky per-particle stop state; a scientific stop must exclude the lane from
later physics without converting a normal stop into a fatal meteorology batch.
Preserve #112's private staging, fatal-status atomicity and caller encoder.
Do not change unrelated horizontal/lower-boundary or PBL/turbulence semantics.

Finite acceptance is the direct-oracle boundary matrix and crossing/retreat
stage order on real WGSL, through **both** production drivers. Require exact
stop predicates and exact f32 placement for the demonstrated target/nxmax;
require the current and predictor query heights/statuses to demonstrate ordering,
with shader/input/target/source hashes and adapter evidence. Reuse existing
sampler/coordinate comparison policies only for their owned quantities. This
research defines **no general new trajectory tolerance**. In particular, the
f64-only `top-0.0001` predicate probe cannot be represented faithfully by an
f32-only state. The adoption claim must explicitly be restricted to representable
GPU states, and no rounding-to-top result may be called equivalent for that
probe. A wider precision claim requires its own decision/proof.

Missing/failed target or preparation status, mismatched target/vector/context,
unsupported initialization/restart/nest/global/ETA/ASL route, or absent validated
W preparation/adoption must continue to fail closed. Adaptive PBL substep and
general settling integration are outside the numerical acceptance demonstrated
here; preserve their rejections/owners. If this handoff would require a new
physics semantic, runtime consumer, source or tolerance, stop and record the
dependency. These research fixtures establish neither GPU boundary parity nor
full FLEXPART/production/operational parity.
