//! Existing eta_dot regressions, preserved from the pre-decomposition module.

use super::super::interfaces::pressure_velocity_interfaces_to_geometric;
use super::super::test_support::{eta_dot_motion, eta_dot_snapshot, geometry_snapshot};
use super::super::{
    reconstruct_vertical_geometry, NativeVerticalMotionSign, NativeVerticalMotionUnit,
};
use super::*;
use approx::assert_relative_eq;

#[test]
fn eta_dot_transform_matches_pinned_calc_etadot_formula_hand_check() {
    // calc_etadot.f90 (v7.1.2) lines 545-557 with META=1/MDPDETA=1:
    //   ETAR(K) = 2*ETAR(K)*PS*(DAK/PS+DBK)/(DAK/P00+DBK); P00=101325.0
    //   ETAR(K) = ETAR(K) - ETAR(K-1) for K > 1
    // A = [1000, 2000, 3000, 0], B = [0.5, 0.6, 0.7, 1.0], PS = 100000.
    let p00 = FLEX_EXTRACT_CALC_ETADOT_REFERENCE_PRESSURE_PA;
    let ps = 100_000.0_f32;
    let (detadot_1, detadot_2, detadot_3) = (-1.0e-5_f32, -2.0e-5_f32, 3.0e-5_f32);

    let scaled_k =
        |deta: f32, dak: f32, dbk: f32| 2.0 * deta * ps * (dak / ps + dbk) / (dak / p00 + dbk);
    let expected_1 = scaled_k(detadot_1, 1_000.0, 0.1);
    let expected_2 = scaled_k(detadot_2, 1_000.0, 0.1) - expected_1;
    let expected_3 = scaled_k(detadot_3, -3_000.0, 0.3) - expected_2;

    for ordering in [VerticalOrdering::Increasing, VerticalOrdering::Decreasing] {
        let snapshot = eta_dot_snapshot(ordering);
        let values = match ordering {
            VerticalOrdering::Increasing => vec![detadot_1, detadot_2, detadot_3],
            VerticalOrdering::Decreasing => vec![detadot_3, detadot_2, detadot_1],
        };
        let result = eta_dot_to_pressure_velocity(&snapshot, &eta_dot_motion(values))
            .expect("validated eta-dot preprocessing");
        assert_eq!(result.values_interface_pa_s.len(), 4);

        let expected = match ordering {
            // Canonical interface order for Increasing: index 0 = top.
            VerticalOrdering::Increasing => vec![0.0, expected_1, expected_2, expected_3],
            // Canonical interface order for Decreasing: index 0 = surface.
            VerticalOrdering::Decreasing => vec![expected_3, expected_2, expected_1, 0.0],
        };
        for (actual, want) in result.values_interface_pa_s.iter().zip(&expected) {
            assert_relative_eq!(actual, want, max_relative = 1.0e-5);
        }
        assert_eq!(
            result.algorithm_id,
            "flex_extract_7_1_2_calc_etadot_meta_mdpdeta_v1"
        );
    }
}

#[test]
fn eta_dot_output_composes_with_flexpart_geometry_consumers() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let motion = eta_dot_motion(vec![1.0e-5, -2.0e-5, 3.0e-5, -4.0e-5]);
    let result = eta_dot_to_pressure_velocity(&snapshot, &motion).expect("eta-dot transform");
    assert_eq!(result.values_interface_pa_s.len(), 6);

    // The produced interface Pa/s field is exactly the input shape that the
    // #30 omega-interface consumer requires; it must not be rejected.
    let geometric = pressure_velocity_interfaces_to_geometric(
        &snapshot,
        &geometry,
        &result.values_interface_pa_s,
    )
    .expect("validated pressure velocity composes with the existing pinmconv path");
    assert_eq!(geometric.len(), 6);
    assert!(geometric.iter().all(|value| value.is_finite()));
}

#[test]
fn eta_dot_rejects_non_canonical_semantics_fail_closed() {
    let snapshot = eta_dot_snapshot(VerticalOrdering::Increasing);
    let mut wrong_unit = eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5]);
    wrong_unit.unit = NativeVerticalMotionUnit::PascalPerSecond;
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &wrong_unit),
        Err(VerticalTransformError::InvalidNativeVerticalMotion { .. })
    ));

    let mut wrong_staggering = eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5]);
    wrong_staggering.vertical_staggering = VerticalStaggering::LevelInterface;
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &wrong_staggering),
        Err(VerticalTransformError::InvalidNativeVerticalMotion { .. })
    ));

    let mut unvalidated_sign = eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5]);
    unvalidated_sign.sign = NativeVerticalMotionSign::PositiveEtaDecreasing;
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &unvalidated_sign),
        Err(VerticalTransformError::InvalidNativeVerticalMotion { .. })
    ));

    let wrong_shape = eta_dot_motion(vec![-1.0e-5, -2.0e-5]);
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &wrong_shape),
        Err(VerticalTransformError::ShapeMismatch { .. })
    ));
}

#[test]
fn eta_dot_rejects_non_finite_inputs_and_bad_coefficients_fail_closed() {
    let mut snapshot = eta_dot_snapshot(VerticalOrdering::Increasing);
    snapshot.vertical_coordinate.hybrid_a_interface_pa = Some(vec![1_000.0, 2_000.0, 3_000.0]);
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5])),
        Err(VerticalTransformError::Contract(_))
    ));

    let snapshot = eta_dot_snapshot(VerticalOrdering::Increasing);
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &eta_dot_motion(vec![f32::NAN, -2.0e-5, 3.0e-5])),
        Err(VerticalTransformError::InvalidNativeVerticalMotionValue { index: 0, .. })
    ));

    let mut bad_pressure = eta_dot_snapshot(VerticalOrdering::Increasing);
    bad_pressure.fields[0].values = vec![f32::NEG_INFINITY];
    assert!(matches!(
        eta_dot_to_pressure_velocity(
            &bad_pressure,
            &eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5])
        ),
        Err(VerticalTransformError::Contract(_))
    ));
}

#[test]
fn eta_dot_rejects_non_hybrid_coordinates_fail_closed() {
    let mut snapshot = eta_dot_snapshot(VerticalOrdering::Increasing);
    snapshot.vertical_coordinate.kind = VerticalCoordinateKind::Pressure;
    snapshot.vertical_coordinate.hybrid_a_interface_pa = None;
    snapshot.vertical_coordinate.hybrid_b_interface = None;
    snapshot.vertical_coordinate.reference_surface_pressure_pa = None;
    snapshot.vertical_coordinate.surface_pressure_dependency = None;
    snapshot.vertical_coordinate.interface_values = None;
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5])),
        Err(VerticalTransformError::UnsupportedVerticalCoordinate)
    ));
}
