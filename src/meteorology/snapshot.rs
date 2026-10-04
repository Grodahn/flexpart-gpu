//! Canonical snapshot assembly, validation order and schema provenance.
//!
//! This owner composes the source metadata validators without changing their
//! precedence. Providers normalize inputs before this boundary; GPU sampling
//! consumes validated snapshots through the canonical facade.

use super::coordinate::validate_vertical;
use super::field::validate_domain;
use super::grid::validate_grid;
use super::time::validate_time;
use super::{
    Axis, Calendar, ContractError, Field, FieldId, HorizontalGrid, HorizontalStaggering,
    Requirements, SchemaIdentity, TemporalKind, VerticalCoordinate, VerticalCoordinateKind,
    VerticalStaggering, SCHEMA_ID, SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Groups canonical source fields sharing one grid and dynamic validity time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub schema: SchemaIdentity,
    pub horizontal_grid: HorizontalGrid,
    pub vertical_coordinate: VerticalCoordinate,
    pub fields: Vec<Field>,
}
/// Reports the canonical schema identity of a validated input boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Provenance {
    pub schema_id: String,
    pub schema_version: u32,
}

impl Snapshot {
    /// Validate schema, dimensions, units, signs, time semantics and required fields.
    ///
    /// # Errors
    /// Returns ContractError for any ambiguous, missing, unsupported or inconsistent input.
    pub fn validate(&self, requirements: &Requirements) -> Result<(), ContractError> {
        if self.schema.id != SCHEMA_ID || self.schema.version != SCHEMA_VERSION {
            return Err(ContractError::UnsupportedSchema);
        }
        validate_grid(&self.horizontal_grid)?;
        validate_vertical(&self.vertical_coordinate)?;

        let mut seen = BTreeSet::new();
        let mut dynamic_time_anchor: Option<(Calendar, i64)> = None;
        for field in &self.fields {
            if !seen.insert(field.id) {
                return Err(ContractError::DuplicateField(field.id));
            }
            self.validate_field(field)?;

            if field.time.kind != TemporalKind::Static {
                let field_time = (field.time.calendar, field.time.valid_time_epoch_seconds);
                match dynamic_time_anchor {
                    Some(anchor) if anchor != field_time => {
                        return Err(ContractError::InconsistentSnapshotTime(field.id));
                    }
                    None => dynamic_time_anchor = Some(field_time),
                    _ => {}
                }
            }
        }
        if self.vertical_coordinate.kind == VerticalCoordinateKind::HybridSigmaPressure
            && !seen.contains(&FieldId::SurfacePressure)
        {
            return Err(ContractError::MissingRequiredField(
                FieldId::SurfacePressure,
            ));
        }

        for id in &requirements.required_fields {
            if !seen.contains(id) {
                return Err(ContractError::MissingRequiredField(*id));
            }
        }
        Ok(())
    }

    /// Identifies the schema declared by this snapshot for evidence consumers.
    #[must_use]
    pub fn provenance(&self) -> Provenance {
        Provenance {
            schema_id: self.schema.id.clone(),
            schema_version: self.schema.version,
        }
    }

    fn validate_field(&self, field: &Field) -> Result<(), ContractError> {
        if field.unit != field.id.unit() {
            return Err(ContractError::UnitMismatch(field.id));
        }
        if field.sign != field.id.sign() {
            return Err(ContractError::SignMismatch(field.id));
        }
        if !field
            .id
            .supports_horizontal_staggering(field.horizontal_staggering)
            || !field
                .id
                .supports_vertical_staggering(field.vertical_staggering)
        {
            return Err(ContractError::InvalidStaggering(field.id));
        }

        let grid = &self.horizontal_grid;
        let (nx, ny) = match field.horizontal_staggering {
            HorizontalStaggering::CellCenter => (grid.nx, grid.ny),
            HorizontalStaggering::XFace => (
                grid.nx
                    .checked_add(1)
                    .ok_or(ContractError::ShapeMismatch(field.id))?,
                grid.ny,
            ),
            HorizontalStaggering::YFace => (
                grid.nx,
                grid.ny
                    .checked_add(1)
                    .ok_or(ContractError::ShapeMismatch(field.id))?,
            ),
        };
        let (shape, axes) = if let Some(class_count) = field.id.class_count() {
            if field.vertical_staggering != VerticalStaggering::NotApplicable {
                return Err(ContractError::InvalidStaggering(field.id));
            }
            (
                vec![nx, ny, class_count],
                vec![Axis::X, Axis::Y, Axis::Class],
            )
        } else if field.id.is_3d() {
            let nz = match field.vertical_staggering {
                VerticalStaggering::LevelCenter => self.vertical_coordinate.level_values.len(),
                VerticalStaggering::LevelInterface => self
                    .vertical_coordinate
                    .interface_values
                    .as_ref()
                    .ok_or(ContractError::InvalidStaggering(field.id))?
                    .len(),
                VerticalStaggering::NotApplicable => {
                    return Err(ContractError::InvalidStaggering(field.id));
                }
            };
            (vec![nx, ny, nz], vec![Axis::X, Axis::Y, Axis::Z])
        } else {
            if field.vertical_staggering != VerticalStaggering::NotApplicable {
                return Err(ContractError::InvalidStaggering(field.id));
            }
            (vec![nx, ny], vec![Axis::X, Axis::Y])
        };

        if field.shape != shape || field.axis_order != axes {
            return Err(ContractError::ShapeMismatch(field.id));
        }
        let count = field
            .shape
            .iter()
            .try_fold(1_usize, |acc, value| acc.checked_mul(*value))
            .ok_or(ContractError::ShapeMismatch(field.id))?;
        if field.values.len() != count {
            return Err(ContractError::ValueCountMismatch(field.id));
        }
        if field.values.iter().any(|value| !value.is_finite()) {
            return Err(ContractError::NonFiniteValue(field.id));
        }
        validate_time(field.id, &field.time)?;
        validate_domain(field)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        Accumulation, FieldTime, LongitudeDomain, SignConvention, StorageOrder, Unit,
        VerticalOrdering, VerticalReference, FLEXPART_LAND_USE_CLASS_COUNT,
    };
    use super::*;
    fn instant() -> FieldTime {
        FieldTime {
            calendar: Calendar::Gregorian,
            kind: TemporalKind::Instantaneous,
            valid_time_epoch_seconds: 1_000,
            interval_start_epoch_seconds: None,
            interval_end_epoch_seconds: None,
            accumulation: None,
        }
    }

    fn static_time() -> FieldTime {
        FieldTime {
            calendar: Calendar::Gregorian,
            kind: TemporalKind::Static,
            valid_time_epoch_seconds: 0,
            interval_start_epoch_seconds: None,
            interval_end_epoch_seconds: None,
            accumulation: None,
        }
    }

    fn field(id: FieldId, values: Vec<f32>) -> Field {
        Field {
            id,
            shape: vec![2, 1, 2],
            axis_order: vec![Axis::X, Axis::Y, Axis::Z],
            unit: id.unit(),
            sign: id.sign(),
            storage_order: StorageOrder::XFastest,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::LevelCenter,
            time: instant(),
            values,
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            schema: SchemaIdentity::default(),
            horizontal_grid: HorizontalGrid {
                nx: 2,
                ny: 1,
                xlon0_deg: 6.0,
                ylat0_deg: 50.0,
                dx_deg: 0.25,
                dy_deg: 0.25,
                longitude_domain: LongitudeDomain::Minus180To180,
            },
            vertical_coordinate: VerticalCoordinate {
                kind: VerticalCoordinateKind::Pressure,
                reference: VerticalReference::ModelNative,
                ordering: VerticalOrdering::Decreasing,
                level_values: vec![90_000.0, 80_000.0],
                interface_values: None,
                hybrid_a_interface_pa: None,
                hybrid_b_interface: None,
                reference_surface_pressure_pa: None,
                surface_pressure_dependency: None,
            },
            fields: vec![
                field(FieldId::WindU, vec![1.0; 4]),
                field(FieldId::WindV, vec![0.5; 4]),
                field(FieldId::VerticalVelocity, vec![0.0; 4]),
                field(FieldId::Temperature, vec![280.0; 4]),
                field(FieldId::SpecificHumidity, vec![0.004; 4]),
                field(
                    FieldId::Pressure,
                    vec![90_000.0, 80_000.0, 90_000.0, 80_000.0],
                ),
            ],
        }
    }
    #[test]
    fn synthetic_advection_snapshot_validates() {
        snapshot().validate(&Requirements::advection()).unwrap();
    }

    #[test]
    fn serialization_roundtrip_preserves_contract() {
        let original = snapshot();
        let json = serde_json::to_string_pretty(&original).unwrap();
        let decoded: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
        assert_eq!(decoded.provenance().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn missing_field_fails_closed() {
        let mut value = snapshot();
        value
            .fields
            .retain(|field| field.id != FieldId::VerticalVelocity);
        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::MissingRequiredField(
                FieldId::VerticalVelocity
            ))
        );
    }

    #[test]
    fn land_use_fractions_require_13_normalized_classes() {
        let mut value = snapshot();
        let mut fractions = vec![0.0; 2 * FLEXPART_LAND_USE_CLASS_COUNT];
        for x in 0..2 {
            fractions[x + 2 * 6] = 1.0;
        }
        value.fields.push(Field {
            id: FieldId::LandUseFractions,
            shape: vec![2, 1, FLEXPART_LAND_USE_CLASS_COUNT],
            axis_order: vec![Axis::X, Axis::Y, Axis::Class],
            unit: Unit::Fraction,
            sign: SignConvention::NonNegative,
            storage_order: StorageOrder::XFastest,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::NotApplicable,
            time: static_time(),
            values: fractions,
        });
        value
            .validate(&Requirements {
                required_fields: [FieldId::LandUseFractions].into_iter().collect(),
            })
            .expect("normalized 13-class land-use fractions must validate");

        let land_use = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::LandUseFractions)
            .expect("land-use fractions");
        land_use.values[2 * 7] = 0.1;
        assert_eq!(
            value.validate(&Requirements {
                required_fields: [FieldId::LandUseFractions].into_iter().collect(),
            }),
            Err(ContractError::InvalidValueDomain(FieldId::LandUseFractions))
        );
    }

    #[test]
    fn flux_accumulation_must_be_normalized_before_canonical_boundary() {
        let mut value = snapshot();
        value.fields.push(Field {
            id: FieldId::SurfaceSolarRadiation,
            shape: vec![2, 1],
            axis_order: vec![Axis::X, Axis::Y],
            unit: Unit::WattPerSquareMeter,
            sign: SignConvention::NonNegative,
            storage_order: StorageOrder::XFastest,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::NotApplicable,
            time: FieldTime {
                calendar: Calendar::Gregorian,
                kind: TemporalKind::IntervalTotal,
                valid_time_epoch_seconds: 1_000,
                interval_start_epoch_seconds: Some(0),
                interval_end_epoch_seconds: Some(1_000),
                accumulation: None,
            },
            values: vec![100.0, 100.0],
        });
        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidTemporalMetadata(
                FieldId::SurfaceSolarRadiation
            ))
        );
    }

    #[test]
    fn non_precip_state_rejects_interval_total_semantics() {
        let mut value = snapshot();
        let temperature = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::Temperature)
            .expect("temperature field");
        temperature.time = FieldTime {
            calendar: Calendar::Gregorian,
            kind: TemporalKind::IntervalTotal,
            valid_time_epoch_seconds: 1_000,
            interval_start_epoch_seconds: Some(0),
            interval_end_epoch_seconds: Some(1_000),
            accumulation: None,
        };

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidTemporalMetadata(FieldId::Temperature))
        );
    }

    #[test]
    fn unit_mismatch_fails_closed() {
        let mut value = snapshot();
        let wind_u = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::WindU)
            .expect("wind_u field");
        wind_u.unit = Unit::Kelvin;

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::UnitMismatch(FieldId::WindU))
        );
    }

    #[test]
    fn shape_mismatch_fails_closed() {
        let mut value = snapshot();
        let temperature = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::Temperature)
            .expect("temperature field");
        temperature.shape = vec![1, 1, 2];
        temperature.values = vec![280.0; 2];

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::ShapeMismatch(FieldId::Temperature))
        );
    }

    #[test]
    fn non_periodic_grid_crossing_longitude_seam_fails_closed() {
        let mut value = snapshot();
        value.horizontal_grid.xlon0_deg = 179.9;
        value.horizontal_grid.dx_deg = 0.25;
        assert!(!value.horizontal_grid.is_periodic_x());
        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidHorizontalGrid)
        );
    }

    #[test]
    fn grid_extending_beyond_pole_fails_closed() {
        let mut value = snapshot();
        value.horizontal_grid.ylat0_deg = 89.9;
        value.horizontal_grid.dy_deg = 0.25;
        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidHorizontalGrid)
        );
    }

    #[test]
    fn exactly_global_x_coverage_is_periodic() {
        let mut value = snapshot();
        value.horizontal_grid.nx = 4;
        value.horizontal_grid.xlon0_deg = -180.0;
        value.horizontal_grid.dx_deg = 90.0;
        for field in &mut value.fields {
            field.shape[0] = 4;
            field.values = vec![field.values[0]; 4 * value.horizontal_grid.ny * field.shape[2]];
        }
        assert!(value.horizontal_grid.is_periodic_x());
        value
            .validate(&Requirements::advection())
            .expect("exactly-global x coverage must use periodic schema-v1 topology");
    }

    #[test]
    fn dynamic_fields_must_share_valid_time_and_calendar() {
        let mut value = snapshot();
        let temperature = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::Temperature)
            .expect("temperature field");
        temperature.time.valid_time_epoch_seconds += 1;

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InconsistentSnapshotTime(
                FieldId::Temperature
            ))
        );

        let mut value = snapshot();
        let temperature = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::Temperature)
            .expect("temperature field");
        temperature.time.calendar = Calendar::ProlepticGregorian;
        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InconsistentSnapshotTime(
                FieldId::Temperature
            ))
        );
    }

    #[test]
    fn static_ancillary_requires_static_time_semantics() {
        let mut value = snapshot();
        value.fields.push(Field {
            id: FieldId::Orography,
            shape: vec![2, 1],
            axis_order: vec![Axis::X, Axis::Y],
            unit: Unit::Meter,
            sign: SignConvention::SignedScalar,
            storage_order: StorageOrder::XFastest,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::NotApplicable,
            time: FieldTime {
                calendar: Calendar::Gregorian,
                kind: TemporalKind::Static,
                valid_time_epoch_seconds: 123,
                interval_start_epoch_seconds: None,
                interval_end_epoch_seconds: None,
                accumulation: None,
            },
            values: vec![100.0, 120.0],
        });
        value
            .validate(&Requirements::advection())
            .expect("static ancillary timestamp is provenance-only and must not join dynamic time alignment");

        let orography = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::Orography)
            .expect("orography");
        orography.time.kind = TemporalKind::Instantaneous;
        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidTemporalMetadata(FieldId::Orography))
        );
    }

    #[test]
    fn dynamic_field_rejects_static_time_semantics() {
        let mut value = snapshot();
        let temperature = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::Temperature)
            .expect("temperature field");
        temperature.time.kind = TemporalKind::Static;
        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidTemporalMetadata(FieldId::Temperature))
        );
    }

    #[test]
    fn unknown_calendar_fails_deserialization() {
        let mut encoded = serde_json::to_value(snapshot()).expect("serialize snapshot");
        encoded["fields"][0]["time"]["calendar"] = serde_json::Value::String("julian".into());
        let decoded = serde_json::from_value::<Snapshot>(encoded);
        assert!(
            decoded.is_err(),
            "unsupported calendars must fail deserialization"
        );
    }

    #[test]
    fn inconsistent_hybrid_metadata_fails_closed() {
        let mut value = snapshot();
        value.vertical_coordinate = VerticalCoordinate {
            kind: VerticalCoordinateKind::HybridSigmaPressure,
            reference: VerticalReference::ModelNative,
            ordering: VerticalOrdering::Increasing,
            level_values: vec![25_000.0, 75_000.0],
            interface_values: Some(vec![0.0, 50_000.0, 100_000.0]),
            hybrid_a_interface_pa: Some(vec![0.0, 0.0]),
            hybrid_b_interface: Some(vec![0.0, 0.5, 1.0]),
            reference_surface_pressure_pa: Some(100_000.0),
            surface_pressure_dependency: Some(FieldId::SurfacePressure),
        };

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidVerticalCoordinate)
        );
    }

    #[test]
    fn negative_hybrid_interface_pressure_fails_closed() {
        let mut value = snapshot();
        value.vertical_coordinate = VerticalCoordinate {
            kind: VerticalCoordinateKind::HybridSigmaPressure,
            reference: VerticalReference::ModelNative,
            ordering: VerticalOrdering::Increasing,
            level_values: vec![24_950.0, 75_000.0],
            interface_values: Some(vec![-100.0, 50_000.0, 100_000.0]),
            hybrid_a_interface_pa: Some(vec![-100.0, 0.0, 0.0]),
            hybrid_b_interface: Some(vec![0.0, 0.5, 1.0]),
            reference_surface_pressure_pa: Some(100_000.0),
            surface_pressure_dependency: Some(FieldId::SurfacePressure),
        };

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidVerticalCoordinate)
        );
    }

    #[test]
    fn missing_hybrid_surface_pressure_dependency_fails_closed() {
        let mut value = snapshot();
        value.vertical_coordinate = VerticalCoordinate {
            kind: VerticalCoordinateKind::HybridSigmaPressure,
            reference: VerticalReference::ModelNative,
            ordering: VerticalOrdering::Increasing,
            level_values: vec![25_000.0, 75_000.0],
            interface_values: Some(vec![0.0, 50_000.0, 100_000.0]),
            hybrid_a_interface_pa: Some(vec![0.0, 0.0, 0.0]),
            hybrid_b_interface: Some(vec![0.0, 0.5, 1.0]),
            reference_surface_pressure_pa: Some(100_000.0),
            surface_pressure_dependency: None,
        };

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidVerticalCoordinate)
        );
    }

    #[test]
    fn hybrid_coordinate_requires_actual_surface_pressure_field() {
        let mut value = snapshot();
        value.vertical_coordinate = VerticalCoordinate {
            kind: VerticalCoordinateKind::HybridSigmaPressure,
            reference: VerticalReference::ModelNative,
            ordering: VerticalOrdering::Increasing,
            level_values: vec![25_000.0, 75_000.0],
            interface_values: Some(vec![0.0, 50_000.0, 100_000.0]),
            hybrid_a_interface_pa: Some(vec![0.0, 0.0, 0.0]),
            hybrid_b_interface: Some(vec![0.0, 0.5, 1.0]),
            reference_surface_pressure_pa: Some(100_000.0),
            surface_pressure_dependency: Some(FieldId::SurfacePressure),
        };

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::MissingRequiredField(
                FieldId::SurfacePressure
            ))
        );
    }

    #[test]
    fn wind_u_x_face_staggering_is_supported() {
        let mut value = snapshot();
        let wind_u = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::WindU)
            .expect("wind_u field");
        wind_u.horizontal_staggering = HorizontalStaggering::XFace;
        wind_u.shape = vec![3, 1, 2];
        wind_u.values = vec![1.0; 6];

        value
            .validate(&Requirements::advection())
            .expect("wind_u x-face staggering is explicitly supported");
    }

    #[test]
    fn wind_u_y_face_staggering_fails_closed() {
        let mut value = snapshot();
        let wind_u = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::WindU)
            .expect("wind_u field");
        wind_u.horizontal_staggering = HorizontalStaggering::YFace;
        wind_u.shape = vec![2, 2, 2];
        wind_u.values = vec![1.0; 8];

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidStaggering(FieldId::WindU))
        );
    }

    #[test]
    fn scalar_face_staggering_fails_closed() {
        let mut value = snapshot();
        let temperature = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::Temperature)
            .expect("temperature field");
        temperature.horizontal_staggering = HorizontalStaggering::XFace;
        temperature.shape = vec![3, 1, 2];
        temperature.values = vec![280.0; 6];

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidStaggering(FieldId::Temperature))
        );
    }

    #[test]
    fn vertical_velocity_interface_staggering_is_supported() {
        let mut value = snapshot();
        value.vertical_coordinate.interface_values = Some(vec![95_000.0, 85_000.0, 75_000.0]);
        let vertical_velocity = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::VerticalVelocity)
            .expect("vertical velocity field");
        vertical_velocity.vertical_staggering = VerticalStaggering::LevelInterface;
        vertical_velocity.shape = vec![2, 1, 3];
        vertical_velocity.values = vec![0.0; 6];

        value
            .validate(&Requirements::advection())
            .expect("vertical velocity interface staggering is explicitly supported");
    }

    #[test]
    fn scalar_interface_staggering_fails_closed() {
        let mut value = snapshot();
        value.vertical_coordinate.interface_values = Some(vec![95_000.0, 85_000.0, 75_000.0]);
        let temperature = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::Temperature)
            .expect("temperature field");
        temperature.vertical_staggering = VerticalStaggering::LevelInterface;
        temperature.shape = vec![2, 1, 3];
        temperature.values = vec![280.0; 6];

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidStaggering(FieldId::Temperature))
        );
    }

    #[test]
    fn accumulation_reset_after_interval_start_fails_closed() {
        let mut value = snapshot();
        value.fields.push(Field {
            id: FieldId::LargeScalePrecipitation,
            shape: vec![2, 1],
            axis_order: vec![Axis::X, Axis::Y],
            unit: Unit::KilogramPerSquareMeter,
            sign: SignConvention::NonNegative,
            storage_order: StorageOrder::XFastest,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::NotApplicable,
            time: FieldTime {
                calendar: Calendar::Gregorian,
                kind: TemporalKind::AccumulatedSinceReset,
                valid_time_epoch_seconds: 1_000,
                interval_start_epoch_seconds: Some(0),
                interval_end_epoch_seconds: Some(1_000),
                accumulation: Some(Accumulation {
                    reset_epoch_seconds: 1,
                }),
            },
            values: vec![0.1, 0.2],
        });

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidTemporalMetadata(
                FieldId::LargeScalePrecipitation
            ))
        );
    }

    #[test]
    fn accumulation_reset_before_interval_start_fails_closed() {
        let mut value = snapshot();
        value.fields.push(Field {
            id: FieldId::LargeScalePrecipitation,
            shape: vec![2, 1],
            axis_order: vec![Axis::X, Axis::Y],
            unit: Unit::KilogramPerSquareMeter,
            sign: SignConvention::NonNegative,
            storage_order: StorageOrder::XFastest,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::NotApplicable,
            time: FieldTime {
                calendar: Calendar::Gregorian,
                kind: TemporalKind::AccumulatedSinceReset,
                valid_time_epoch_seconds: 1_000,
                interval_start_epoch_seconds: Some(10),
                interval_end_epoch_seconds: Some(1_000),
                accumulation: Some(Accumulation {
                    reset_epoch_seconds: 0,
                }),
            },
            values: vec![0.1, 0.2],
        });

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidTemporalMetadata(
                FieldId::LargeScalePrecipitation
            ))
        );
    }

    #[test]
    fn specific_humidity_above_one_fails_closed() {
        let mut value = snapshot();
        let humidity = value
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::SpecificHumidity)
            .expect("specific humidity field");
        humidity.values[0] = 1.01;

        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidValueDomain(FieldId::SpecificHumidity))
        );
    }

    #[test]
    fn ambiguous_accumulation_fails_closed() {
        let mut value = snapshot();
        value.fields.push(Field {
            id: FieldId::LargeScalePrecipitation,
            shape: vec![2, 1],
            axis_order: vec![Axis::X, Axis::Y],
            unit: Unit::KilogramPerSquareMeter,
            sign: SignConvention::NonNegative,
            storage_order: StorageOrder::XFastest,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::NotApplicable,
            time: FieldTime {
                calendar: Calendar::Gregorian,
                kind: TemporalKind::AccumulatedSinceReset,
                valid_time_epoch_seconds: 1_000,
                interval_start_epoch_seconds: Some(0),
                interval_end_epoch_seconds: Some(1_000),
                accumulation: None,
            },
            values: vec![0.1, 0.2],
        });
        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::InvalidTemporalMetadata(
                FieldId::LargeScalePrecipitation
            ))
        );
    }
}
