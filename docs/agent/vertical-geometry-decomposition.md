# Vertical geometry decomposition (#127)

## Pre-move responsibility inventory and target boundaries

Recorded before moving code from `src/meteorology/vertical.rs`, at base
`375426db534e666e5a95b3b203acdb167a4b7f7d`. Issue #127 is the sole
implementation contract. The [#30 geometry contract](../vertical-transform.md)
and existing #80 evidence retain their authority; this decomposition changes
neither science nor verdicts.

| Existing responsibility | Target private owner under `src/meteorology/vertical/` |
| --- | --- |
| Opaque derived state, borrowed runtime view, model/interface point identity, shape and index rejection | `runtime.rs` |
| Hybrid A/B pressure reconstruction, local surface anchoring, pressure ordering/tolerance checks | `pressure.rs` |
| Surface/model virtual temperature, Goff-Gratch, hypsometric integration and model-level geometry assembly | `model_levels.rs` |
| FLEXPART W heights, artificial surface level, physical traversal, `pinmconv` interface omega conversion | `interfaces.rs` |
| Native/normalized motion types, unit/sign/staggering checks, same-Snapshot composition and normalization | `motion.rs` |
| Validation-only pinned calc_etadot recurrence and its reference pressure | `eta_dot.rs` |
| Column-local AGL/ASL conversion, explicit release-height/reference/range resolution and shape checks | `terrain.rs` |
| Serialized input hashing and transform provenance | `provenance.rs` |
| Shared fail-closed error vocabulary | `error.rs` |
| X-fastest offsets and model-level traversal shared by geometry owners; canonical field lookup | `layout.rs` |
| Existing unit regressions and synthetic builders | `tests/`, grouped by responsibility with shared [test_support.rs](../../src/meteorology/vertical/test_support.rs) |

`src/meteorology/vertical.rs` remains the stable canonical facade, with explicit
re-exports of the existing public types/functions. Implementation modules stay
private. Cross-owner helpers and opaque fields are visible only within this
vertical subtree (`pub(super)`); no crate-wide geometry construction or second
representation is introduced. Local validators stay with their owner rather
than forming an unrelated catch-all validation layer.

The runtime state and normalized motion keep their field order, serialize-only
derives and external opacity. Geometry assembly, arithmetic order, storage
direction, terrain anchoring, error variants/messages, motion staggering,
provenance strings/hashes and validation order are preserved. No sampling,
provider, GPU implementation, or #117 CPU cleanup changes belong here.

## Verification and context comparison

Before movement, capture the existing 24 vertical unit checks, runtime and
sampling integration checks, #80 frozen-evidence checks, and candidate column
reports. After movement, rerun these checks and compare candidate report bytes
and integration report bytes. Frozen fixtures remain untouched. Final Rust,
navigation and pinned-oracle/device CI gates remain required.

Representative task: inspect terrain/release AGL/ASL rejection. Counts use
UTF-8 source bytes with LF line endings, independent of checkout CRLF settings.
Contracts and root instructions are required in both cases and excluded from
the source-only comparison.

| Source working set | Lines | UTF-8 bytes (LF) |
| --- | ---: | ---: |
| Before: complete `vertical.rs`, including unrelated motion/eta-dot tests | 2,586 | 100,463 |
| After: [stable facade](../../src/meteorology/vertical.rs) | 42 | 1,583 |
| After: [terrain/release owner](../../src/meteorology/vertical/terrain.rs) | 260 | 8,515 |
| After: [terrain tests](../../src/meteorology/vertical/tests/terrain.rs) | 41 | 1,379 |
| After: representative total | 343 | 11,477 |

The representative source working set is 88.6% smaller in bytes and 86.7%
smaller in lines. Optional [addressing](../../src/meteorology/vertical/layout.rs)
and [error definitions](../../src/meteorology/vertical/error.rs) are named
handoffs; inspecting both adds 6,077 UTF-8 bytes (LF) and still avoids loading
model-level, W, motion, eta-dot and provenance implementations. A column-bound
release task additionally opens the [runtime view](../../src/meteorology/vertical/runtime.rs).
These counts measure source selection, not model tokens or runtime performance.

An implementation-time token audit compared all 77 original function bodies,
including methods, synthetic helpers and the 24 original tests, against their
new owners. Only whitespace/comments and optional Rustfmt trailing commas were
excluded; all expression/control-flow/error tokens matched. Derived field
ordering, serde derives and externally opaque construction remain unchanged.
The [public facade regression](../../tests/vertical_geometry_contract.rs) freezes
compact/pretty geometry serialization and runtime point-access hashes from the
pre-move revision for both storage directions, single/multiple columns, unequal
surface pressures and nonzero/below-sea-level terrain. Compile-fail doctests
also preserve external mutation/deserialization rejection for derived geometry
and normalized motion.

The existing #80 `not_equivalent` oracle conclusion is retained intentionally.
This task discovers no new scientific defect and makes no change to that
owning contract. No deviation from the Fortran reference is introduced by #127,
so a scientific-changelog entry is unnecessary. Exact test/CI results and
environment limitations are retained in the PR body; the [verification map](test-map.md#vertical-geometry)
defines the stable commands.
