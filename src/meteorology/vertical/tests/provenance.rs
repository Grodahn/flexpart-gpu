//! Existing provenance regressions, preserved from the pre-decomposition module.

use super::super::motion::normalize_vertical_motion;
use super::super::test_support::geometry_snapshot;
use super::super::{
    reconstruct_vertical_geometry, NativeVerticalMotion, NativeVerticalMotionKind,
    NativeVerticalMotionProvenance, NativeVerticalMotionSign, NativeVerticalMotionUnit,
};
use super::*;
use crate::meteorology::{FieldId, VerticalStaggering};

#[test]
fn vertical_motion_provenance_records_source_and_output_staggering() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::PressureVelocityOmega,
        unit: NativeVerticalMotionUnit::PascalPerSecond,
        sign: NativeVerticalMotionSign::PositivePressureIncreasing,
        vertical_staggering: VerticalStaggering::LevelInterface,
        values: vec![0.0, 0.0, -1.0, 0.0, -2.0, 0.0],
        provenance: NativeVerticalMotionProvenance {
            source_id: "provenance-test".to_string(),
        },
    };

    let normalized = normalize_vertical_motion(&snapshot, &geometry, &native).expect("normalize");
    assert_eq!(
        normalized.provenance.source_vertical_staggering,
        VerticalStaggering::LevelInterface
    );
    assert_eq!(
        normalized.provenance.output_vertical_staggering,
        VerticalStaggering::LevelInterface
    );
    assert_eq!(
        normalized.provenance.algorithm_id,
        "omega_interface_flexpart11_pinmconv_v1"
    );
    assert_eq!(normalized.provenance.source_native_motion_sha256.len(), 64);
    assert!(normalized
        .provenance
        .source_native_motion_sha256
        .chars()
        .all(|character| character.is_ascii_hexdigit()));

    assert_eq!(geometry.provenance.source_snapshot_sha256.len(), 64);
    assert!(geometry
        .provenance
        .source_snapshot_sha256
        .chars()
        .all(|character| character.is_ascii_hexdigit()));
}

#[test]
fn provenance_hashes_change_when_transform_inputs_change() {
    let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");

    let mut changed_snapshot = snapshot.clone();
    changed_snapshot
        .fields
        .iter_mut()
        .find(|field| field.id == FieldId::Temperature2m)
        .expect("temperature2m")
        .values[0] += 0.25;
    let changed_geometry =
        reconstruct_vertical_geometry(&changed_snapshot).expect("changed geometry");
    assert_ne!(
        geometry.provenance.source_snapshot_sha256,
        changed_geometry.provenance.source_snapshot_sha256
    );

    let native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::PressureVelocityOmega,
        unit: NativeVerticalMotionUnit::PascalPerSecond,
        sign: NativeVerticalMotionSign::PositivePressureIncreasing,
        vertical_staggering: VerticalStaggering::LevelInterface,
        values: vec![0.0, 0.0, -1.0, 0.0, -2.0, 0.0],
        provenance: NativeVerticalMotionProvenance {
            source_id: "hash-binding".to_string(),
        },
    };
    let first = normalize_vertical_motion(&snapshot, &geometry, &native).expect("normalize");
    let mut changed_native = native.clone();
    changed_native.values[2] = -1.25;
    let second =
        normalize_vertical_motion(&snapshot, &geometry, &changed_native).expect("normalize");
    assert_ne!(
        first.provenance.source_native_motion_sha256,
        second.provenance.source_native_motion_sha256
    );
}
