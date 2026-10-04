//! Canonical X-fastest addressing, physical traversal and immutable field lookup.

use super::super::{FieldId, Snapshot, VerticalOrdering};
use super::VerticalTransformError;

/// Borrow a required canonical field without introducing provider decoding.
pub(super) fn field_values(
    snapshot: &Snapshot,
    id: FieldId,
) -> Result<&[f32], VerticalTransformError> {
    snapshot
        .fields
        .iter()
        .find(|field| field.id == id)
        .map(|field| field.values.as_slice())
        .ok_or(VerticalTransformError::MissingField(id))
}

/// Preserve canonical storage order while traversing real levels from the surface.
#[inline]
pub(super) const fn model_level_index_from_surface(
    ordering: VerticalOrdering,
    nz: usize,
    step: usize,
) -> usize {
    match ordering {
        VerticalOrdering::Increasing => nz - 1 - step,
        VerticalOrdering::Decreasing => step,
    }
}

/// Address local surface data in canonical X-fastest storage.
#[inline]
pub(super) const fn surface_offset(x: usize, y: usize, nx: usize) -> usize {
    x + nx * y
}

/// Address a model-level field without changing its canonical ordering.
#[inline]
pub(super) const fn volume_offset(x: usize, y: usize, z: usize, nx: usize, ny: usize) -> usize {
    x + nx * (y + ny * z)
}

/// Address the distinct interface axis in canonical X-fastest storage.
#[inline]
pub(super) const fn interface_offset(x: usize, y: usize, z: usize, nx: usize, ny: usize) -> usize {
    x + nx * (y + ny * z)
}
