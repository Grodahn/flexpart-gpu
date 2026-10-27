# Canonical driver preparation (#173 / #197)

Both real driver `prepare_canonical_meteorology` methods adopt
`CanonicalMeteorologyBracket::from_shared_height_vector`. The bracket lends two
exact Snapshots, their opaque #30 runtime views and matching #184 prepared U/V
and #188 corrected W owners. The slot calls the existing
`CanonicalGpuField::from_shared_height_vector` constructor for all three fields.
No wind, target, normalization or correction is reconstructed at this boundary.
A canonical W field is optional; #30 normalized interface motion is authoritative.

Use `driver.meteorology_context()` for explicit source uploads and caller-owned
#184/#188 preparation encoders. Initialize the existing `SharedHeightTarget`
once at initial mother-member preparation, retain it for the run and prepare new
members against that owner. Submit preparation before the timestep in queue
order. Adoption does not submit, wait or read status. Each of the six production
queries checks the actual target and every U/V/W column status on device, so an
unsubmitted, incomplete or failed preparation cannot publish scientific state.

Each driver retains its first successfully adopted target independently of every
caller slot; even replacing the slot cannot reinitialize the target. The target
is bound only after complete host validation.

The caller retains `CanonicalMeteorologySlot` across steps. It owns an Arc of
three canonical fields and the complete prepared source dependencies. Unchanged
source identities and identical prepared owners reuse the Arc without uploads.
Replacement calls #190's exact source/geometry/time/normalization/context checks.
A different target owner rejects even for equal height bytes, and a prepared run
cannot revert to native meteorology. Failed adoption leaves the previous slot
intact. Adjacent source members can be cloned into the next pair against the same
run target. The timestep also retains the prepared W owners (and thus U/V,
native uploads, context and target) through deferred submission checkpoints.

Both current and signed advanced times resolve through #89 before production
work. Forward adds dt; backward subtracts it. Both times must be covered, even
when the legacy diagnostic bounds option permits clamping. Production preflight
checks the exact driver context, coordinate grid and retained shared-target
transport boundary before releases. See [resident advection](resident-advection.md).

`CanonicalMeteorologyBracket::new` retains the #173 native/normalized-center-W
representation for explicit diagnostic sampling only. Production timesteps reject
it. The existing `canonical_timeloop_meteorology` target still verifies that
compatibility owner's resident queries, source/motion transitions, retained-owner
execution and context/time rejection. Its diagnostic clock steps are explicitly
named legacy calls; they are not #197 production evidence.

## Evidence and supported envelope

[Shared advection evidence](shared-advection.md) exercises the actual forward and
backward production drivers, current and advanced samples, signed time, resource
reuse, failures and supported displacement. The #186 genuine linked binary
replays exact driver queries on an invariant-top control; the unchanged #190
query target covers the original nonuniform fractional-stencil cases.

The supported production route is regional mother-grid AGL, #184's accepted
initial (0,0) pressure selection, normalized interface omega and complete prepared
U/V/W. Native column/member heights may differ. [#198's ordinary transport action](transport-top.md)
uses the device run-target endpoint before advanced sampling and after correction.
Restart/fallback, global/nested/polar/ETA/ASL, requested settling (#203), wider
precision/adaptive-PBL routes and operational provider decoding remain unsupported.
No full FLEXPART production parity is claimed; scenario-8 trajectory precision
remains without an approved comparison policy (#204).
