# Resident meteorology queries (#171)

This is the reusable query/resource handoff for the canonical #76 facade.
It establishes composition with existing #87/#88/#89 science, not production
consumer migration or a new scientific parity claim.

`CanonicalGpuField::prepare_resident` allocates timestep resources from static
source metadata. It never inspects positions. `encode_from_particles` records
status reset, the particle producer, adaptation, canonical sampling and result
validation into the caller's encoder. `encode_fixture_consumer` records the
minimal device-status-aware consumer. The caller submits once and observes
status only at its final checkpoint; `ResidentStatus::require_success` rejects
fatal or incomplete execution before output is accepted.

## Query and status ABI

The independent query lane is 32 bytes, in this order:

| Offset (bytes) | Quantity/type |
| --- | --- |
| 0, 4 | Authoritative signed `i32` cell x/y |
| 8, 12 | Separate `f32` fractions in `[0, 1)` |
| 16 | `f32` height in metres AGL |
| 20 | `u32` active flag; zero means inactive |
| 24, 28 | Reserved zero words |

Capacity and eligible active-prefix count are separate control metadata.
Inactive flags inside the prefix and all lanes beyond it are non-errors.
The 96-byte Particle representation occurs only in the particle producer.
Crate-local intermediate producers receive `ResidentQueryWriteTarget` from
`encode_reset`, write the same ABI, and then use the crate-local sample encoder.
This is the write target for later #112 work; it implements no predictor physics.

Status is `u32[capacity + 1]`: word zero is shared fatal state, followed by one
lane reason. Codes are 0 inactive, 1 pending, 2 valid, 3 negative signed cell,
4 invalid fraction, 5 non-finite query, 6 outside supported domain, 7 differing
geometry within the active stencil (#118), 8 non-finite source corner, and
9 incomplete/non-finite sampled result. Reset and validation are separate device
dispatches. Fatal status prevents *all* fixture writes, including valid lanes in
other workgroups. Invalid lanes still receive bounded #87 indices, a finite
height and a valid fallback height column before downstream sampling.

## Source resources and scientific ownership

Canonical sources own x-fastest, physical bottom-to-top field planes and the
source-lifetime device arrays needed to validate corners and choose #30 columns.
Height resources are `[column * levels + level]`. Each lane compares only its
non-zero-weight stencil against its lower corner using #76's exact compatible
height rule. Independent lanes can select different compatible profiles.

Packed resident field/height resources must fit the device's storage-binding and
index limits. When an aggregate exceeds those limits, the existing per-plane
#76 upload and host-query facade retain their capability. Resident preparation
returns an explicit unsupported error; it never falls back to host queries.

The adapter writes #87's existing 32-byte query geometry. Only #87 performs the
bilinear field weighted sum. #88 uses heights `[lane * levels + level]` and
horizontally sampled values `[level * capacity + lane]`; two explicit strides
extend its existing shader without changing the boundary/search/weight equations.
#89 blends the lane-aligned vector with one host-resolved bracket for the batch.

The sampled handoff has one scalar component and lane stride one, separately
records capacity/active count, and carries field/unit/sign/staggering, grid,
source times/hashes, bracket and #30 geometry provenance. Class/multicomponent,
accumulated/interval and interface-W resident paths reject explicitly in v1.

## Ownership and evidence

Sources and geometry live for their source/bracket. Query/status resources live
for their timestep/intermediate use. A prepared sample exclusively borrows its
query batch, preventing another use from resetting shared status while its
sample handoff remains borrowed. Keep prepared resources through submission and
the last GPU consumer. Particle resources carry a unique context token; backend
handle equality alone does not prove ownership across independent instances.

The [resident device test](../tests/meteorology_resident.rs) retains
`target/ci-gate/meteorology-resident/report.json`, with bit-preserved inputs,
source/grid/geometry identities, status, stages, shader/input hashes and adapter
provenance. Final checkpoint readbacks supply evidence only. Existing horizontal,
vertical and temporal pinned-oracle gates retain their tolerances and authority.
The [software workflow](../.github/workflows/software-wgpu.yml) requires both.

#112 can adopt this substrate for resident advection queries. Production consumer
migrations remain #112–#115; within-stencil differing profiles remain #118;
precipitation time and surface-flux eligibility remain #119/#120. ASL conversion,
interface-W extension and #117 cleanup are outside this change.

[#173 driver preparation](canonical-timeloop-meteorology.md) supplies validated
source pairs and U/V/normalized center-W owners at both actual driver boundaries.
It resolves current/predicted times and retains bracket resources for #112;
production advection still uses the existing legacy operator until that migration.
