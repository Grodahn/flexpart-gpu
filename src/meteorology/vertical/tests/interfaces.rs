//! Existing interfaces regressions, preserved from the pre-decomposition module.

use super::*;

#[test]
fn derived_asl_geometry_overflow_fails_closed() {
    assert!(matches!(
        reconstruct_flexpart_w_heights(
            VerticalOrdering::Increasing,
            1,
            1,
            1,
            &[f32::MAX],
            &[f32::MAX],
        ),
        Err(VerticalTransformError::InvalidAbsoluteHeight { x: 0, y: 0, .. })
    ));
}
