//! Existing pressure regressions, preserved from the pre-decomposition module.

use super::super::test_support::hybrid_snapshot;
use super::*;

#[test]
fn hybrid_pressure_uses_local_surface_pressure_and_declared_ordering() {
    let increasing = reconstruct_hybrid_pressure(&hybrid_snapshot(VerticalOrdering::Increasing))
        .expect("increasing pressure coordinate must reconstruct");
    assert_eq!(
        increasing.interface_pressure_pa,
        vec![50_000.0, 45_000.0, 75_000.0, 67_500.0, 100_000.0, 90_000.0]
    );
    assert_eq!(
        increasing.level_pressure_pa,
        vec![62_500.0, 56_250.0, 87_500.0, 78_750.0]
    );

    let decreasing = reconstruct_hybrid_pressure(&hybrid_snapshot(VerticalOrdering::Decreasing))
        .expect("decreasing pressure coordinate must reconstruct");
    assert_eq!(
        decreasing.interface_pressure_pa,
        vec![100_000.0, 90_000.0, 75_000.0, 67_500.0, 50_000.0, 45_000.0]
    );
}

#[test]
fn hybrid_surface_interface_must_match_actual_local_surface_pressure() {
    let mut snapshot = hybrid_snapshot(VerticalOrdering::Increasing);
    let a = snapshot
        .vertical_coordinate
        .hybrid_a_interface_pa
        .as_mut()
        .expect("hybrid A");
    let b = snapshot
        .vertical_coordinate
        .hybrid_b_interface
        .as_mut()
        .expect("hybrid B");

    // This still matches the schema's 100 kPa reference interface:
    // 10 kPa + 0.9 * 100 kPa = 100 kPa. At the second column's actual
    // 90 kPa surface pressure it becomes 91 kPa and must fail closed
    // rather than pairing 91 kPa with the 0 m AGL W surface.
    a[2] = 10_000.0;
    b[2] = 0.9;

    let error = reconstruct_hybrid_pressure(&snapshot)
        .expect_err("surface hybrid interface must track each column's local ps");
    assert!(matches!(
        error,
        VerticalTransformError::SurfaceInterfacePressureMismatch {
            x: 1,
            y: 0,
            interface_pressure_pa,
            surface_pressure_pa,
        } if (interface_pressure_pa - 91_000.0).abs() < 0.1
            && (surface_pressure_pa - 90_000.0).abs() < 0.1
    ));
}
