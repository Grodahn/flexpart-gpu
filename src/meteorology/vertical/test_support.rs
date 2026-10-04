//! Shared synthetic inputs for the unchanged vertical regressions.

use super::super::{
    Axis, Calendar, Field, FieldId, FieldTime, HorizontalGrid, HorizontalStaggering,
    LongitudeDomain, SchemaIdentity, SignConvention, Snapshot, StorageOrder, TemporalKind, Unit,
    VerticalCoordinate, VerticalCoordinateKind, VerticalOrdering, VerticalReference,
    VerticalStaggering,
};
use super::*;

fn field_time(kind: TemporalKind) -> FieldTime {
    FieldTime {
        calendar: Calendar::Gregorian,
        kind,
        valid_time_epoch_seconds: 1_700_000_000,
        interval_start_epoch_seconds: None,
        interval_end_epoch_seconds: None,
        accumulation: None,
    }
}

/// Supply unequal local surface pressures and terrain for ordering regressions.
pub(super) fn hybrid_snapshot(ordering: VerticalOrdering) -> Snapshot {
    let (interfaces, levels) = match ordering {
        VerticalOrdering::Increasing => (
            vec![50_000.0, 75_000.0, 100_000.0],
            vec![62_500.0, 87_500.0],
        ),
        VerticalOrdering::Decreasing => (
            vec![100_000.0, 75_000.0, 50_000.0],
            vec![87_500.0, 62_500.0],
        ),
    };
    let (a, b) = match ordering {
        VerticalOrdering::Increasing => (vec![0.0, 0.0, 0.0], vec![0.5, 0.75, 1.0]),
        VerticalOrdering::Decreasing => (vec![0.0, 0.0, 0.0], vec![1.0, 0.75, 0.5]),
    };

    Snapshot {
        schema: SchemaIdentity::default(),
        horizontal_grid: HorizontalGrid {
            nx: 2,
            ny: 1,
            xlon0_deg: 0.0,
            ylat0_deg: 0.0,
            dx_deg: 1.0,
            dy_deg: 1.0,
            longitude_domain: LongitudeDomain::Minus180To180,
        },
        vertical_coordinate: VerticalCoordinate {
            kind: VerticalCoordinateKind::HybridSigmaPressure,
            reference: VerticalReference::ModelNative,
            ordering,
            level_values: levels,
            interface_values: Some(interfaces),
            hybrid_a_interface_pa: Some(a),
            hybrid_b_interface: Some(b),
            reference_surface_pressure_pa: Some(100_000.0),
            surface_pressure_dependency: Some(FieldId::SurfacePressure),
        },
        fields: vec![
            Field {
                id: FieldId::SurfacePressure,
                shape: vec![2, 1],
                axis_order: vec![Axis::X, Axis::Y],
                storage_order: StorageOrder::XFastest,
                unit: Unit::Pascal,
                sign: SignConvention::NonNegative,
                horizontal_staggering: HorizontalStaggering::CellCenter,
                vertical_staggering: VerticalStaggering::NotApplicable,
                time: field_time(TemporalKind::Instantaneous),
                values: vec![100_000.0, 90_000.0],
            },
            Field {
                id: FieldId::Orography,
                shape: vec![2, 1],
                axis_order: vec![Axis::X, Axis::Y],
                storage_order: StorageOrder::XFastest,
                unit: Unit::Meter,
                sign: SignConvention::SignedScalar,
                horizontal_staggering: HorizontalStaggering::CellCenter,
                vertical_staggering: VerticalStaggering::NotApplicable,
                time: field_time(TemporalKind::Static),
                values: vec![100.0, -20.0],
            },
        ],
    }
}

/// Add thermodynamics to the same canonical columns for geometry regressions.
pub(super) fn geometry_snapshot(ordering: VerticalOrdering) -> Snapshot {
    let mut snapshot = hybrid_snapshot(ordering);
    let (temperature, specific_humidity) = match ordering {
        // X-fastest storage: both X cells for the upper level, then both
        // X cells for the surface-most level.
        VerticalOrdering::Increasing => (
            vec![280.0, 281.0, 290.0, 291.0],
            vec![0.005, 0.006, 0.010, 0.011],
        ),
        VerticalOrdering::Decreasing => (
            vec![290.0, 291.0, 280.0, 281.0],
            vec![0.010, 0.011, 0.005, 0.006],
        ),
    };

    snapshot.fields.extend([
        Field {
            id: FieldId::Temperature,
            shape: vec![2, 1, 2],
            axis_order: vec![Axis::X, Axis::Y, Axis::Z],
            storage_order: StorageOrder::XFastest,
            unit: Unit::Kelvin,
            sign: SignConvention::SignedScalar,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::LevelCenter,
            time: field_time(TemporalKind::Instantaneous),
            values: temperature,
        },
        Field {
            id: FieldId::SpecificHumidity,
            shape: vec![2, 1, 2],
            axis_order: vec![Axis::X, Axis::Y, Axis::Z],
            storage_order: StorageOrder::XFastest,
            unit: Unit::KilogramPerKilogram,
            sign: SignConvention::NonNegative,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::LevelCenter,
            time: field_time(TemporalKind::Instantaneous),
            values: specific_humidity,
        },
        Field {
            id: FieldId::Temperature2m,
            shape: vec![2, 1],
            axis_order: vec![Axis::X, Axis::Y],
            storage_order: StorageOrder::XFastest,
            unit: Unit::Kelvin,
            sign: SignConvention::SignedScalar,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::NotApplicable,
            time: field_time(TemporalKind::Instantaneous),
            values: vec![292.0, 294.0],
        },
        Field {
            id: FieldId::Dewpoint2m,
            shape: vec![2, 1],
            axis_order: vec![Axis::X, Axis::Y],
            storage_order: StorageOrder::XFastest,
            unit: Unit::Kelvin,
            sign: SignConvention::SignedScalar,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::NotApplicable,
            time: field_time(TemporalKind::Instantaneous),
            values: vec![285.0, 286.0],
        },
    ]);
    snapshot
}

/// Supply nonzero A/B coefficients to exercise the validation-only recurrence.
pub(super) fn eta_dot_snapshot(ordering: VerticalOrdering) -> Snapshot {
    let (a, b, interface_values, level_values) = match ordering {
        VerticalOrdering::Increasing => (
            vec![1_000.0, 2_000.0, 3_000.0, 0.0],
            vec![0.5, 0.6, 0.7, 1.0],
            vec![51_000.0, 62_000.0, 73_000.0, 100_000.0],
            vec![56_500.0, 67_500.0, 86_500.0],
        ),
        VerticalOrdering::Decreasing => (
            vec![0.0, 3_000.0, 2_000.0, 1_000.0],
            vec![1.0, 0.7, 0.6, 0.5],
            vec![100_000.0, 73_000.0, 62_000.0, 51_000.0],
            vec![86_500.0, 67_500.0, 56_500.0],
        ),
    };
    Snapshot {
        schema: SchemaIdentity::default(),
        horizontal_grid: HorizontalGrid {
            nx: 1,
            ny: 1,
            xlon0_deg: 0.0,
            ylat0_deg: 0.0,
            dx_deg: 1.0,
            dy_deg: 1.0,
            longitude_domain: LongitudeDomain::Minus180To180,
        },
        vertical_coordinate: VerticalCoordinate {
            kind: VerticalCoordinateKind::HybridSigmaPressure,
            reference: VerticalReference::ModelNative,
            ordering,
            level_values,
            interface_values: Some(interface_values),
            hybrid_a_interface_pa: Some(a),
            hybrid_b_interface: Some(b),
            reference_surface_pressure_pa: Some(100_000.0),
            surface_pressure_dependency: Some(FieldId::SurfacePressure),
        },
        fields: vec![Field {
            id: FieldId::SurfacePressure,
            shape: vec![1, 1],
            axis_order: vec![Axis::X, Axis::Y],
            storage_order: StorageOrder::XFastest,
            unit: Unit::Pascal,
            sign: SignConvention::NonNegative,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::NotApplicable,
            time: field_time(TemporalKind::Instantaneous),
            values: vec![100_000.0],
        }],
    }
}

/// Retain explicit full-level eta-dot semantics in preprocessing regressions.
pub(super) fn eta_dot_motion(values: Vec<f32>) -> NativeVerticalMotion {
    NativeVerticalMotion {
        kind: NativeVerticalMotionKind::EtaCoordinateVelocity,
        unit: NativeVerticalMotionUnit::PerSecond,
        sign: NativeVerticalMotionSign::PositiveEtaIncreasing,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values,
        provenance: NativeVerticalMotionProvenance {
            source_id: "eta-dot-synthetic-column".to_string(),
        },
    }
}
