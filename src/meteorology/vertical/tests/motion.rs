//! Existing motion regressions, preserved from the pre-decomposition module.

use super::super::test_support::geometry_snapshot;
use super::*;
use crate::meteorology::VerticalOrdering;

#[test]
fn center_staggered_omega_fails_closed_without_flexpart_oracle() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::PressureVelocityOmega,
        unit: NativeVerticalMotionUnit::PascalPerSecond,
        sign: NativeVerticalMotionSign::PositivePressureIncreasing,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values: vec![-1.0, 0.0, -2.0, 0.0],
        provenance: NativeVerticalMotionProvenance {
            source_id: "unvalidated-center-omega".to_string(),
        },
    };

    let error = reconstruct_vertical_geometry_with_motion(&snapshot, &native)
        .expect_err("center-staggered omega must fail until independently validated");
    assert!(matches!(
        error,
        VerticalTransformError::InvalidNativeVerticalMotion {
            reason: "kind/unit/sign combination is not canonical for the declared representation"
        }
    ));
}

#[test]
fn eta_dot_production_path_remains_fail_closed_after_validation() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::EtaCoordinateVelocity,
        unit: NativeVerticalMotionUnit::PerSecond,
        sign: NativeVerticalMotionSign::PositiveEtaIncreasing,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values: vec![1.0e-5, 0.0, 3.0e-5, 0.0],
        provenance: NativeVerticalMotionProvenance {
            source_id: "validated-etadot-production-disabled".to_string(),
        },
    };

    let error = reconstruct_vertical_geometry_with_motion(&snapshot, &native)
        .expect_err("validated eta-dot preprocessing must remain disabled in production");
    assert!(matches!(
        error,
        VerticalTransformError::InvalidNativeVerticalMotion {
            reason: "eta-dot preprocessing is validation-only; production normalization remains disabled until explicitly enabled after #70"
        }
    ));
}

#[test]
fn geometric_vertical_motion_is_identity_with_explicit_upward_semantics() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::GeometricVelocity,
        unit: NativeVerticalMotionUnit::MeterPerSecond,
        sign: NativeVerticalMotionSign::PositiveUpward,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values: vec![1.0, -2.0, 3.0, -4.0],
        provenance: NativeVerticalMotionProvenance {
            source_id: "already-geometric".to_string(),
        },
    };

    let normalized =
        normalize_vertical_motion(&snapshot, &geometry, &native).expect("identity normalize");
    assert_eq!(normalized.values_ms, native.values);
}

#[test]
fn ambiguous_native_vertical_motion_semantics_fail_closed() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::PressureVelocityOmega,
        unit: NativeVerticalMotionUnit::MeterPerSecond,
        sign: NativeVerticalMotionSign::PositivePressureIncreasing,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values: vec![0.0; 4],
        provenance: NativeVerticalMotionProvenance {
            source_id: "wrong-unit".to_string(),
        },
    };

    let error = normalize_vertical_motion(&snapshot, &geometry, &native)
        .expect_err("omega declared as m/s must fail");
    assert!(matches!(
        error,
        VerticalTransformError::InvalidNativeVerticalMotion { .. }
    ));
}

#[test]
fn native_vertical_motion_wrong_shape_fails_closed() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::PressureVelocityOmega,
        unit: NativeVerticalMotionUnit::PascalPerSecond,
        sign: NativeVerticalMotionSign::PositivePressureIncreasing,
        vertical_staggering: VerticalStaggering::LevelInterface,
        values: vec![0.0; 4], // expected 2*1*(2+1) = 6
        provenance: NativeVerticalMotionProvenance {
            source_id: "wrong-shape".to_string(),
        },
    };

    let error = normalize_vertical_motion(&snapshot, &geometry, &native)
        .expect_err("wrong interface shape must fail");
    assert!(matches!(
        error,
        VerticalTransformError::ShapeMismatch {
            field: "native_vertical_motion",
            expected: 6,
            actual: 4
        }
    ));
}

#[test]
fn eta_dot_interface_staggering_fails_closed() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::EtaCoordinateVelocity,
        unit: NativeVerticalMotionUnit::PerSecond,
        sign: NativeVerticalMotionSign::PositiveEtaIncreasing,
        vertical_staggering: VerticalStaggering::LevelInterface,
        values: vec![0.0; 6],
        provenance: NativeVerticalMotionProvenance {
            source_id: "wrong-etadot-staggering".to_string(),
        },
    };

    let error = normalize_vertical_motion(&snapshot, &geometry, &native)
        .expect_err("raw eta-dot on interfaces is ambiguous and must fail");
    assert!(matches!(
        error,
        VerticalTransformError::InvalidNativeVerticalMotion { .. }
    ));
}

#[test]
fn non_finite_native_vertical_motion_fails_closed() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::GeometricVelocity,
        unit: NativeVerticalMotionUnit::MeterPerSecond,
        sign: NativeVerticalMotionSign::PositiveUpward,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values: vec![0.0, f32::NAN, 0.0, 0.0],
        provenance: NativeVerticalMotionProvenance {
            source_id: "nan-motion".to_string(),
        },
    };

    let error = normalize_vertical_motion(&snapshot, &geometry, &native)
        .expect_err("non-finite vertical motion must fail");
    assert!(matches!(
        error,
        VerticalTransformError::InvalidNativeVerticalMotionValue {
            index: 1,
            value
        } if value.is_nan()
    ));
}
