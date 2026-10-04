//! Existing terrain regressions, preserved from the pre-decomposition module.

use super::*;

#[test]
fn agl_asl_conversion_is_column_local_and_handles_below_sea_level_terrain() {
    let terrain = vec![100.0, -20.0];
    let agl = vec![0.0, 0.0, 500.0, 500.0];
    let asl = height_agl_to_asl(2, 1, 2, &agl, &terrain).expect("AGL to ASL");
    assert_eq!(asl, vec![100.0, -20.0, 600.0, 480.0]);
    let roundtrip = height_asl_to_agl(2, 1, 2, &asl, &terrain).expect("ASL to AGL");
    assert_eq!(roundtrip, agl);
}

#[test]
fn release_and_height_reference_arithmetic_overflow_fails_closed() {
    assert!(matches!(
        resolve_release_height(f32::MAX, VerticalReference::AboveGroundLevel, f32::MAX,),
        Err(VerticalTransformError::InvalidReleaseHeight { .. })
    ));

    assert!(matches!(
        height_agl_to_asl(1, 1, 1, &[f32::MAX], &[f32::MAX]),
        Err(VerticalTransformError::InvalidAbsoluteHeight { .. })
    ));

    assert!(matches!(
        height_asl_to_agl(1, 1, 1, &[f32::MAX], &[-f32::MAX]),
        Err(VerticalTransformError::InvalidAbsoluteHeight { .. })
    ));
}

#[test]
fn asl_below_local_terrain_fails_closed() {
    let error = height_asl_to_agl(1, 1, 1, &[99.0], &[100.0])
        .expect_err("below-terrain ASL height must fail");
    assert!(matches!(
        error,
        VerticalTransformError::HeightBelowTerrain { .. }
    ));
}
