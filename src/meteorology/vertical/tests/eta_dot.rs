//! Existing eta_dot regressions, preserved from the pre-decomposition module.

use super::super::interfaces::pressure_velocity_interfaces_to_geometric;
use super::super::test_support::{eta_dot_motion, eta_dot_snapshot, geometry_snapshot};
use super::super::{
    reconstruct_vertical_geometry, NativeVerticalMotionSign, NativeVerticalMotionUnit,
};
use super::*;
use approx::assert_relative_eq;
use serde::Deserialize;

#[derive(Deserialize)]
struct FrozenColumns {
    hybrid_a_interface_pa: Vec<f32>,
    hybrid_b_interface: Vec<f32>,
    columns: Vec<FrozenColumn>,
}

#[derive(Deserialize)]
struct FrozenColumn {
    id: String,
    surface_pressure_pa: f32,
    eta_dot_per_s: Vec<f32>,
    oracle_below_level_pa_s: Vec<f64>,
}

#[test]
fn eta_dot_full_l137_matches_frozen_original_for_both_array_orderings() {
    let fixture: FrozenColumns = serde_json::from_str(include_str!(
        "../../../../fixtures/vertical/calc-etadot-l137-v1.json"
    ))
    .expect("frozen calc_etadot columns");
    assert_eq!(fixture.hybrid_a_interface_pa.len(), 138);
    assert_eq!(fixture.hybrid_b_interface.len(), 138);
    assert_eq!(fixture.columns.len(), 3);
    const ABS_TOL_PA_S: f64 = 1.0e-7;
    const REL_TOL: f64 = 3.0e-5;

    for column in fixture.columns {
        assert_eq!(column.eta_dot_per_s.len(), 137);
        assert_eq!(column.oracle_below_level_pa_s.len(), 137);
        for ordering in [VerticalOrdering::Increasing, VerticalOrdering::Decreasing] {
            let mut snapshot = eta_dot_snapshot(ordering);
            let mut a = fixture.hybrid_a_interface_pa.clone();
            let mut b = fixture.hybrid_b_interface.clone();
            let mut motion = column.eta_dot_per_s.clone();
            if ordering == VerticalOrdering::Decreasing {
                a.reverse();
                b.reverse();
                motion.reverse();
            }
            let ps = column.surface_pressure_pa;
            let interfaces: Vec<_> = a.iter().zip(&b).map(|(a, b)| a + b * ps).collect();
            let vertical = &mut snapshot.vertical_coordinate;
            vertical.level_values = interfaces
                .windows(2)
                .map(|pair| (pair[0] + pair[1]) / 2.0)
                .collect();
            vertical.interface_values = Some(interfaces);
            vertical.hybrid_a_interface_pa = Some(a);
            vertical.hybrid_b_interface = Some(b);
            vertical.reference_surface_pressure_pa = Some(ps);
            snapshot.fields[0].values = vec![ps];

            let mut output = eta_dot_to_pressure_velocity(&snapshot, &eta_dot_motion(motion))
                .expect("valid complete hybrid column")
                .values_interface_pa_s;
            if ordering == VerticalOrdering::Decreasing {
                output.reverse();
            }
            assert_eq!(output.len(), 138);
            assert_eq!(output[0].to_bits(), 0.0_f32.to_bits());
            for (k, (&actual, &expected)) in output[1..]
                .iter()
                .zip(&column.oracle_below_level_pa_s)
                .enumerate()
            {
                let error = (f64::from(actual) - expected).abs();
                assert!(
                    error <= ABS_TOL_PA_S.max(REL_TOL * expected.abs()),
                    "{} {ordering:?} interface below level {}: {actual} vs {expected}",
                    column.id,
                    k + 1
                );
            }
        }
    }
}

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
    let mut geometric = eta_dot_motion(vec![1.0, 2.0, 3.0]);
    geometric.kind = NativeVerticalMotionKind::GeometricVelocity;
    geometric.unit = NativeVerticalMotionUnit::MeterPerSecond;
    geometric.sign = NativeVerticalMotionSign::PositiveUpward;
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &geometric),
        Err(VerticalTransformError::InvalidNativeVerticalMotion { .. })
    ));
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

    let mut wrong_order = snapshot;
    wrong_order.vertical_coordinate.ordering = VerticalOrdering::Decreasing;
    assert!(matches!(
        eta_dot_to_pressure_velocity(&wrong_order, &eta_dot_motion(vec![1.0, 2.0, 3.0])),
        Err(VerticalTransformError::Contract(_))
    ));
}

#[test]
fn eta_dot_rejects_invalid_layer_thickness_and_surface_anchor() {
    let mut snapshot = eta_dot_snapshot(VerticalOrdering::Increasing);
    let motion = eta_dot_motion(vec![1.0e-5, -2.0e-5, 3.0e-5]);
    // The serialized reference geometry remains valid at 100 kPa. With 5 kPa
    // locally the last layer inverts and must not get a signed or zero factor.
    snapshot.fields[0].values = vec![5_000.0];
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &motion),
        Err(VerticalTransformError::InvalidEtaDotTransform { k: 3, .. })
    ));

    snapshot.fields[0].values = vec![100_000.0];
    // The modified coefficients agree only at the reference surface pressure.
    snapshot
        .vertical_coordinate
        .hybrid_a_interface_pa
        .as_mut()
        .expect("A")[3] = 10_000.0;
    snapshot
        .vertical_coordinate
        .hybrid_b_interface
        .as_mut()
        .expect("B")[3] = 0.9;
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &motion),
        Err(VerticalTransformError::InvalidNativeVerticalMotion { .. })
    ));
}

#[test]
fn eta_dot_rejects_negative_local_top_pressure() {
    let mut snapshot = eta_dot_snapshot(VerticalOrdering::Increasing);
    let vertical = &mut snapshot.vertical_coordinate;
    // At the declared 100 kPa reference the top is 1 kPa. Raising local
    // pressure to 120 kPa makes the top negative without inverting any layer.
    vertical.hybrid_a_interface_pa.as_mut().expect("A")[0] = 11_000.0;
    vertical.hybrid_b_interface.as_mut().expect("B")[0] = -0.1;
    vertical.interface_values.as_mut().expect("interfaces")[0] = 1_000.0;
    vertical.level_values[0] = 31_500.0;
    snapshot.fields[0].values[0] = 120_000.0;
    let motion = eta_dot_motion(vec![1.0e-5, -2.0e-5, 3.0e-5]);
    assert!(matches!(
        eta_dot_to_pressure_velocity(&snapshot, &motion),
        Err(VerticalTransformError::InvalidPressureColumn {
            x: 0,
            y: 0,
            index: 0
        })
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
