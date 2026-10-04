//! Existing model_levels regressions, preserved from the pre-decomposition module.

use super::super::test_support::geometry_snapshot;
use super::*;
use crate::meteorology::ContractError;
use approx::assert_relative_eq;

#[test]
fn flexpart_hypsometric_near_isothermal_branch_matches_analytic_solution() {
    let actual = flexpart_hypsometric_layer_thickness_m(100_000.0, 90_000.0, 300.0, 300.0);
    let expected = (R_AIR / GA) * (100_000.0_f32 / 90_000.0).ln() * 300.0;
    assert_relative_eq!(actual, expected, max_relative = 1.0e-6);
}

#[test]
fn height_reconstruction_is_independent_of_storage_direction() {
    let increasing =
        reconstruct_vertical_geometry(&geometry_snapshot(VerticalOrdering::Increasing))
            .expect("top-to-bottom storage must reconstruct");
    let decreasing =
        reconstruct_vertical_geometry(&geometry_snapshot(VerticalOrdering::Decreasing))
            .expect("bottom-to-top storage must reconstruct");

    for x in 0..2 {
        let inc_top = increasing.height_agl_m[volume_offset(x, 0, 0, 2, 1)];
        let inc_bottom = increasing.height_agl_m[volume_offset(x, 0, 1, 2, 1)];
        let dec_bottom = decreasing.height_agl_m[volume_offset(x, 0, 0, 2, 1)];
        let dec_top = decreasing.height_agl_m[volume_offset(x, 0, 1, 2, 1)];

        assert!(inc_top > inc_bottom);
        assert!(dec_top > dec_bottom);
        assert!(inc_bottom > 0.0);
        assert_relative_eq!(inc_bottom, dec_bottom, max_relative = 1.0e-6);
        assert_relative_eq!(inc_top, dec_top, max_relative = 1.0e-6);

        let terrain = increasing.terrain_asl_m[x];
        assert_relative_eq!(
            increasing.height_asl_m[volume_offset(x, 0, 0, 2, 1)],
            inc_top + terrain,
            epsilon = 1.0e-4
        );
        assert_relative_eq!(
            increasing.height_asl_m[volume_offset(x, 0, 1, 2, 1)],
            inc_bottom + terrain,
            epsilon = 1.0e-4
        );
    }
}

#[test]
fn exact_height_reconstruction_requires_flexpart_surface_thermodynamics() {
    let mut snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    snapshot
        .fields
        .retain(|field| field.id != FieldId::Dewpoint2m);
    let error = reconstruct_vertical_geometry(&snapshot)
        .expect_err("missing 2-m dewpoint must fail instead of using a fallback");
    assert_eq!(
        error,
        VerticalTransformError::Contract(ContractError::MissingRequiredField(FieldId::Dewpoint2m))
    );
}
