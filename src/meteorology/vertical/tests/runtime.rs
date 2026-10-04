//! Existing runtime regressions, preserved from the pre-decomposition module.

use super::super::reconstruct_vertical_geometry;
use super::super::test_support::geometry_snapshot;
use super::*;
use crate::meteorology::VerticalOrdering;

#[test]
fn runtime_view_rejects_internal_corrupt_derived_shape() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let mut geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    geometry.height_agl_m.pop();

    let error = geometry
        .runtime_view()
        .expect_err("corrupt internal runtime shape must fail");
    assert!(matches!(
        error,
        VerticalTransformError::RuntimeShapeMismatch {
            field: "height_agl_m",
            ..
        }
    ));
}
