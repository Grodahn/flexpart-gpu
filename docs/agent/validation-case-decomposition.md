# Validation case decomposition (#125)

## Pre-change inventory

At main revision `740f56c`, `src/validation/case.rs` combines 5,858 lines
(253,080 UTF-8 bytes, about 247 KiB) of schema declarations, behavior and 123 tests:

| Responsibility | Existing surface | Target private module |
| --- | --- | --- |
| Root manifest, domain/integration constraints, units and physics consistency | Root types/error, `validate`, unit/surface/deposition checks | `manifest.rs` |
| Canonical document parsing and serialization | `load_from_file`, `parse`, `write_to_file`, schema/migration/round-trip tests | `document.rs` |
| Release geometry, inventory/species identity and chronology | Release types, timestamps, containment, mass/species checks | `release.rs` |
| Meteorology source/transformation and oracle profile identity | Wind/surface/source types, profile and real-weather checks | `meteorology.rs` |
| Output direction, timing and comparison grid | Direction/output types and output/grid checks | `output.rs` |
| Candidate/oracle RNG namespaces and preparation | RNG types, required-null deserializer, identity derivation and checks | `stochastic.rs` |
| Oracle COMMAND configuration | Override/formulation types, CTL/switch agreement checks | `oracle.rs` |
| External contract and evidence handoffs | Execution/candidate/metric references, representation limitations and required artifact classes | `handoff.rs` |

Comparison and verdict computation are not implemented in this file. It only
declares external metric/threshold references, representation limitations and
required comparison-report/run-manifest classes. Concrete artifact hashes and
scientific/input-equivalence verdicts remain with their existing owners.

## Target and preservation strategy

Use `src/validation/case/mod.rs` as the stable facade. Preserve its existing
public symbols with explicit re-exports; keep all eight implementation modules
private. Cross-module validation methods use `pub(super)` only when needed.
Keep the original validation call order, method bodies, error messages, schema
attributes and field order. Move tests with their owning responsibility; share
only fixture/schema helpers in a private test-only support module.

The source schema, fixture data, production caller paths and scientific formulas
are outside the diff. A public-facade integration test freezes compact/pretty
JSON byte hashes captured before decomposition and checks file-writing bytes.

## Agent context comparison

Representative task: inspect a rejected output timing or output-grid declaration.
Before: the primary module is approximately 247 KiB including unrelated RNG,
meteorology, release, parsing and handoff tests. After: start at the stable facade
and inspect `output.rs`, which owns the output model, checks and focused tests
(about 480 lines, about 18 KiB), alongside the 64-line facade (about 3 KiB). The
primary source surface is therefore about 21 KiB, approximately 91% smaller.
Inspect `manifest.rs` only for validation order or root field composition, and
`oracle.rs` only when the declared synchronization interval is the handoff in
question. Navigation/test entries identify those boundaries directly.

This is a source working-set estimate, not a runtime or scientific improvement.
No deviation from the Fortran reference or scientific contract is introduced.
