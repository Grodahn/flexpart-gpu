# Meteorology facade decomposition (#130)

## Pre-refactor inventory

Recorded before moving code at revision `186be537b81271433230e8a5fea815809310e0bb`.
The root owns 1,718 lines / 67,626 checkout bytes (CRLF), including 33 tests.
It declares five public domain modules but implements no sampling or provider decoding.

| Non-facade responsibility | Original lines / symbols | Final private owner |
| --- | --- | --- |
| Schema identity/default, schema constants and cross-domain validation errors | 17-20, 24-37, 626-656; `SchemaIdentity`, `SCHEMA_ID`, `SCHEMA_VERSION`, `ContractError` | `schema.rs` |
| Horizontal coordinate metadata, canonical periodicity, full-cell grid validation and degree tolerance | 39-74, 658-723; `HorizontalGrid`, `LongitudeDomain`, `is_periodic_x`, `validate_grid`, `grid_close` | `grid.rs` |
| Native vertical coordinate metadata, interface/hybrid consistency and ordering checks | 76-116, 725-824; coordinate types, `validate_vertical`, `vertical_close`, `monotonic` | `coordinate.rs` |
| Field identifiers, dimensionality/class cardinality, supported staggering, units/signs, field matrix, physics requirement sets | 117-318, 415-484; `FieldId` helpers, `FIELD_SPECS`, `requirements_for`, `Requirements` | `field.rs` |
| Field layout/storage/staggering types, payload and physical value-domain checks | 319-347, 393-406, 916-955; `Axis`, `StorageOrder`, `HorizontalStaggering`, `Field`, `validate_domain` | `field.rs` |
| Vertical staggering metadata | 348-354; `VerticalStaggering` | `coordinate.rs` |
| Calendar, temporal kind, explicit accumulation reset, field timestamps and field-policy/window validation | 356-391, 826-914; time types, `validate_time`, `validate_interval` | `time.rs` |
| Snapshot composition, validation order, duplicate/required fields, dynamic time alignment, hybrid surface-pressure dependency, field shapes/counts/finite values and provenance | 407-413, 485-625; `Snapshot`, `Provenance`, `validate`, `validate_field`, `provenance` | `snapshot.rs` |
| Five field matrix / requirement tests and their document-row helpers | 1032-1156 | `field.rs` tests |
| Twenty-eight snapshot boundary/serialization regressions and four synthetic fixture helpers | 961-1030, 1158-1717 | `snapshot.rs` tests |

## Ownership and preservation

The canonical metadata modules are private. The facade explicitly exports every
previous canonical public type/constant; these are supported consumer paths.
No obsolete internal-path aliases are added. Cross-owner helpers use only
`pub(super)`; remaining helpers and snapshot field validation stay private.

The existing public owners are unchanged: `horizontal.rs` samples horizontal
fields; `vertical.rs` derives runtime geometry and normalizes native motion;
`vertical_sampling.rs` samples that geometry; `temporal.rs` resolves/samples
instantaneous brackets; `accumulation.rs` derives interval amounts/rates.
GPU composition remains in `src/gpu/meteorology.rs`, with existing stage hosts
and shaders. Provider decoding remains at the IO boundary.

Canonical grid validation and horizontal sampling validation have distinct
existing error contracts and checks (including sampling's explicit non-finite
coverage guard). Both are preserved verbatim. Moving metadata into `grid.rs`
does not unify them or define new sampling semantics. Likewise, `coordinate.rs`
validates serialized native metadata; `vertical.rs` continues to own actual-local-
surface-pressure reconstruction and derived geometry. `time.rs` validates source
metadata only; it performs no interpolation or deaccumulation. These are already
distinct boundaries, so no architecture decision or follow-up is required.

Original validation order, error text, serde attributes/field order, field matrix,
floating-point evaluation order and tolerances are retained. No Fortran routine
is ported and no scientific deviation is introduced.

## Final facade and representative context

The facade is 34 lines / 1,223 UTF-8 bytes. Its six private owners are
`schema.rs` (identity/errors), `field.rs` (matrix/layout/requirements/domain),
`grid.rs` (horizontal source metadata), `coordinate.rs` (native vertical metadata),
`time.rs` (source time metadata), and `snapshot.rs` (composition/validation/tests).
The five public sampling/geometry/interval modules retain their existing paths.

Representative task: inspect rejection of malformed canonical source time
metadata. Counts use UTF-8 with LF so checkout newline policy cannot distort
comparison. The original task baseline root is 1,718 lines / 65,913 bytes;
current main after its independent formatting PR is 1,950 lines / 67,991 bytes.

| Source working set | Before (original / updated main) | After |
| --- | --- | --- |
| Primary API + source time metadata/validator | root: 65,913 / 67,991 bytes | facade + time owner: 175 lines / 6,620 bytes |
| Include the complete existing snapshot boundary-test owner | same root: 65,913 / 67,991 bytes | facade + time + snapshot: 1,002 lines / 37,004 bytes |

The primary source surface is about 90% smaller; including the entire boundary
regression owner is about 44% smaller against the original baseline (46% against
updated main). Read `snapshot.rs` for validation precedence or the relevant
boundary assertion. Read `field.rs` only when crossing into field policy matrix
membership; inspect sampling or derived intervals only at their named handoff.
This estimates source context, not scientific or runtime improvement.

## Preservation checks

All 33 original root tests move intact: five matrix/requirements tests to the
field owner and 28 boundary/serialization regressions to the snapshot owner.
The technical workflow now selects those modules rather than a stale root
filter. Canonical contract tests additionally freeze compact and pretty JSON
hashes for both the synthetic and real native-level snapshots, captured before
movement on the original implementation.

Focused checks: metadata owners and canonical contract, horizontal/vertical/time/
accumulation device targets, composition and existing geometry regressions.
The pre/post local device run produced 118 identical JSON artifacts across
horizontal/time, vertical, accumulated intervals and composition inputs/handoffs/
reports. This compares serialized evidence as well as numerical results without
changing tolerances or upgrading the evidence to scientific production parity.
Full local logs and the pre-refactor evidence snapshot are retained under
`target/issue-130/` in the initiating checkout. Final Rust/navigation and pinned
technical/software CI outcomes are reported in the PR for its exact head.
