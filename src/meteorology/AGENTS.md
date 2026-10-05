# Meteorology task context

Select the [meteorology row](../../docs/agent/repo-map.md#meteorology), then only
the schema, geometry, spatial, temporal or accumulated-field contract being changed.
Provider normalization lives at the linked IO boundary; sampling semantics live
with their owning contracts. Inspect the paired GPU host/kernel and fixture before
changing a production calculation. #76 composition and #77 adoption are separate
handoffs. Use [focused checks](../../docs/agent/test-map.md#meteorology).

The stable canonical API is [the facade](mod.rs). Private source owners are
[schema identity/errors](schema.rs), [field matrix/layout/requirements](field.rs),
[horizontal grid](grid.rs), [native vertical coordinates](coordinate.rs),
[source time metadata](time.rs), and [snapshot validation/provenance](snapshot.rs).
Read only the matching owner; metadata validation does not own sampling or
derived runtime geometry. [Inventory/context](../../docs/agent/meteorology-decomposition.md).
