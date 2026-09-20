//! Canonical provider-independent meteorology contract for issue #29.
//!
//! Provider/file adapters normalize into these types. Physics must not depend on
//! GRIB ids, NetCDF variable names, provider naming, or implicit unit/sign rules.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable canonical meteorology schema id.
pub const SCHEMA_ID: &str = "flexpart-gpu.canonical-meteorology";
/// Current canonical meteorology schema version.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchemaIdentity {
    pub id: String,
    pub version: u32,
}

impl Default for SchemaIdentity {
    fn default() -> Self {
        Self {
            id: SCHEMA_ID.to_string(),
            version: SCHEMA_VERSION,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HorizontalGrid {
    pub nx: usize,
    pub ny: usize,
    pub xlon0_deg: f64,
    pub ylat0_deg: f64,
    pub dx_deg: f64,
    pub dy_deg: f64,
    pub longitude_domain: LongitudeDomain,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LongitudeDomain {
    Minus180To180,
    ZeroTo360,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerticalCoordinate {
    pub kind: VerticalCoordinateKind,
    pub reference: VerticalReference,
    pub ordering: VerticalOrdering,
    pub level_values: Vec<f32>,
    #[serde(default)]
    pub interface_values: Option<Vec<f32>>,
    #[serde(default)]
    pub hybrid_a_pa: Option<Vec<f32>>,
    #[serde(default)]
    pub hybrid_b: Option<Vec<f32>>,
    #[serde(default)]
    pub surface_pressure_dependency: Option<FieldId>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerticalCoordinateKind {
    GeometricHeight,
    Pressure,
    HybridSigmaPressure,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerticalReference {
    AboveMeanSeaLevel,
    AboveGroundLevel,
    ModelNative,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerticalOrdering {
    Increasing,
    Decreasing,
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum FieldId {
    WindU,
    WindV,
    VerticalVelocity,
    Temperature,
    SpecificHumidity,
    Pressure,
    AirDensity,
    DensityGradient,
    SurfacePressure,
    Orography,
    LandSeaMask,
    WindU10m,
    WindV10m,
    Temperature2m,
    Dewpoint2m,
    LargeScalePrecipitation,
    ConvectivePrecipitation,
    TotalCloudCover,
    CloudLiquidWater,
    CloudIceWater,
    SensibleHeatFlux,
    SurfaceSolarRadiation,
    SurfaceStressEastward,
    SurfaceStressNorthward,
    FrictionVelocity,
    ConvectiveVelocityScale,
    MixingHeight,
    TropopauseHeight,
    InverseObukhovLength,
    RoughnessLength,
    LandUseClass,
}

impl FieldId {
    #[must_use]
    pub fn p0_fields() -> &'static [Self] {
        &[
            Self::WindU, Self::WindV, Self::VerticalVelocity, Self::Temperature,
            Self::SpecificHumidity, Self::Pressure, Self::AirDensity, Self::DensityGradient,
            Self::SurfacePressure, Self::Orography, Self::LandSeaMask, Self::WindU10m,
            Self::WindV10m, Self::Temperature2m, Self::Dewpoint2m,
            Self::LargeScalePrecipitation, Self::ConvectivePrecipitation,
            Self::TotalCloudCover, Self::CloudLiquidWater, Self::CloudIceWater,
            Self::SensibleHeatFlux, Self::SurfaceSolarRadiation,
            Self::SurfaceStressEastward, Self::SurfaceStressNorthward,
            Self::FrictionVelocity, Self::ConvectiveVelocityScale, Self::MixingHeight,
            Self::TropopauseHeight, Self::InverseObukhovLength, Self::RoughnessLength,
            Self::LandUseClass,
        ]
    }

    fn is_3d(self) -> bool {
        matches!(
            self,
            Self::WindU | Self::WindV | Self::VerticalVelocity | Self::Temperature
                | Self::SpecificHumidity | Self::Pressure | Self::AirDensity
                | Self::DensityGradient | Self::CloudLiquidWater | Self::CloudIceWater
        )
    }

    fn unit(self) -> Unit {
        match self {
            Self::WindU | Self::WindV | Self::VerticalVelocity | Self::WindU10m
            | Self::WindV10m | Self::FrictionVelocity | Self::ConvectiveVelocityScale => {
                Unit::MeterPerSecond
            }
            Self::Temperature | Self::Temperature2m | Self::Dewpoint2m => Unit::Kelvin,
            Self::SpecificHumidity | Self::CloudLiquidWater | Self::CloudIceWater => {
                Unit::KilogramPerKilogram
            }
            Self::Pressure | Self::SurfacePressure => Unit::Pascal,
            Self::AirDensity => Unit::KilogramPerCubicMeter,
            Self::DensityGradient => Unit::KilogramPerQuarticMeter,
            Self::Orography | Self::MixingHeight | Self::TropopauseHeight
            | Self::RoughnessLength => Unit::Meter,
            Self::LandSeaMask | Self::TotalCloudCover => Unit::Fraction,
            Self::LargeScalePrecipitation | Self::ConvectivePrecipitation => {
                Unit::KilogramPerSquareMeter
            }
            Self::SensibleHeatFlux | Self::SurfaceSolarRadiation => Unit::WattPerSquareMeter,
            Self::SurfaceStressEastward | Self::SurfaceStressNorthward => {
                Unit::NewtonPerSquareMeter
            }
            Self::InverseObukhovLength => Unit::PerMeter,
            Self::LandUseClass => Unit::ClassIndex,
        }
    }

    fn sign(self) -> SignConvention {
        match self {
            Self::WindU | Self::SurfaceStressEastward => SignConvention::PositiveEastward,
            Self::WindV | Self::SurfaceStressNorthward => SignConvention::PositiveNorthward,
            Self::VerticalVelocity => SignConvention::PositiveUpward,
            Self::SensibleHeatFlux => SignConvention::PositiveUpwardFlux,
            Self::Temperature | Self::Temperature2m | Self::Dewpoint2m
            | Self::DensityGradient | Self::InverseObukhovLength => SignConvention::SignedScalar,
            _ => SignConvention::NonNegative,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    MeterPerSecond,
    Kelvin,
    KilogramPerKilogram,
    Pascal,
    KilogramPerCubicMeter,
    KilogramPerQuarticMeter,
    Meter,
    Fraction,
    KilogramPerSquareMeter,
    WattPerSquareMeter,
    NewtonPerSquareMeter,
    PerMeter,
    ClassIndex,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignConvention {
    PositiveEastward,
    PositiveNorthward,
    PositiveUpward,
    PositiveUpwardFlux,
    NonNegative,
    SignedScalar,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
    Z,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HorizontalStaggering {
    CellCenter,
    XFace,
    YFace,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerticalStaggering {
    NotApplicable,
    LevelCenter,
    LevelInterface,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Calendar {
    Gregorian,
    ProlepticGregorian,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemporalKind {
    Instantaneous,
    IntervalMean,
    IntervalTotal,
    AccumulatedSinceReset,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accumulation {
    pub reset_epoch_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldTime {
    pub calendar: Calendar,
    pub kind: TemporalKind,
    pub valid_time_epoch_seconds: i64,
    #[serde(default)]
    pub interval_start_epoch_seconds: Option<i64>,
    #[serde(default)]
    pub interval_end_epoch_seconds: Option<i64>,
    #[serde(default)]
    pub accumulation: Option<Accumulation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Field {
    pub id: FieldId,
    pub shape: Vec<usize>,
    pub axis_order: Vec<Axis>,
    pub unit: Unit,
    pub sign: SignConvention,
    pub horizontal_staggering: HorizontalStaggering,
    pub vertical_staggering: VerticalStaggering,
    pub time: FieldTime,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub schema: SchemaIdentity,
    pub horizontal_grid: HorizontalGrid,
    pub vertical_coordinate: VerticalCoordinate,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Requirements {
    pub required_fields: BTreeSet<FieldId>,
}

impl Requirements {
    #[must_use]
    pub fn p0_complete() -> Self {
        Self {
            required_fields: FieldId::p0_fields().iter().copied().collect(),
        }
    }

    #[must_use]
    pub fn advection() -> Self {
        Self {
            required_fields: [
                FieldId::WindU,
                FieldId::WindV,
                FieldId::VerticalVelocity,
                FieldId::Temperature,
                FieldId::SpecificHumidity,
                FieldId::Pressure,
            ]
            .into_iter()
            .collect(),
        }
    }
}

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
        for field in &self.fields {
            if !seen.insert(field.id) {
                return Err(ContractError::DuplicateField(field.id));
            }
            self.validate_field(field)?;
        }
        for id in &requirements.required_fields {
            if !seen.contains(id) {
                return Err(ContractError::MissingRequiredField(*id));
            }
        }
        Ok(())
    }

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

        let grid = &self.horizontal_grid;
        let (shape, axes) = if field.id.is_3d() {
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
            (vec![grid.nx, grid.ny, nz], vec![Axis::X, Axis::Y, Axis::Z])
        } else {
            if field.vertical_staggering != VerticalStaggering::NotApplicable {
                return Err(ContractError::InvalidStaggering(field.id));
            }
            (vec![grid.nx, grid.ny], vec![Axis::X, Axis::Y])
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

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("unsupported canonical meteorology schema")]
    UnsupportedSchema,
    #[error("invalid horizontal grid")]
    InvalidHorizontalGrid,
    #[error("invalid vertical coordinate")]
    InvalidVerticalCoordinate,
    #[error("duplicate field {0:?}")]
    DuplicateField(FieldId),
    #[error("missing required field {0:?}")]
    MissingRequiredField(FieldId),
    #[error("unit mismatch for {0:?}")]
    UnitMismatch(FieldId),
    #[error("sign mismatch for {0:?}")]
    SignMismatch(FieldId),
    #[error("invalid staggering for {0:?}")]
    InvalidStaggering(FieldId),
    #[error("shape or axis mismatch for {0:?}")]
    ShapeMismatch(FieldId),
    #[error("value count mismatch for {0:?}")]
    ValueCountMismatch(FieldId),
    #[error("non-finite value in {0:?}")]
    NonFiniteValue(FieldId),
    #[error("invalid temporal metadata for {0:?}")]
    InvalidTemporalMetadata(FieldId),
    #[error("invalid value domain for {0:?}")]
    InvalidValueDomain(FieldId),
}

fn validate_grid(grid: &HorizontalGrid) -> Result<(), ContractError> {
    if grid.nx == 0
        || grid.ny == 0
        || !grid.dx_deg.is_finite()
        || !grid.dy_deg.is_finite()
        || grid.dx_deg <= 0.0
        || grid.dy_deg <= 0.0
        || !grid.xlon0_deg.is_finite()
        || !grid.ylat0_deg.is_finite()
        || !(-90.0..=90.0).contains(&grid.ylat0_deg)
    {
        return Err(ContractError::InvalidHorizontalGrid);
    }
    let lon_valid = match grid.longitude_domain {
        LongitudeDomain::Minus180To180 => (-180.0..=180.0).contains(&grid.xlon0_deg),
        LongitudeDomain::ZeroTo360 => (0.0..360.0).contains(&grid.xlon0_deg),
    };
    if !lon_valid {
        return Err(ContractError::InvalidHorizontalGrid);
    }
    Ok(())
}

fn validate_vertical(vertical: &VerticalCoordinate) -> Result<(), ContractError> {
    if vertical.level_values.is_empty()
        || vertical.level_values.iter().any(|value| !value.is_finite())
        || !monotonic(&vertical.level_values, vertical.ordering)
    {
        return Err(ContractError::InvalidVerticalCoordinate);
    }
    if let Some(interfaces) = &vertical.interface_values {
        if interfaces.len() != vertical.level_values.len() + 1
            || interfaces.iter().any(|value| !value.is_finite())
            || !monotonic(interfaces, vertical.ordering)
        {
            return Err(ContractError::InvalidVerticalCoordinate);
        }
    }

    match vertical.kind {
        VerticalCoordinateKind::GeometricHeight => {
            if vertical.reference == VerticalReference::ModelNative
                || vertical.hybrid_a_pa.is_some()
                || vertical.hybrid_b.is_some()
                || vertical.surface_pressure_dependency.is_some()
            {
                return Err(ContractError::InvalidVerticalCoordinate);
            }
        }
        VerticalCoordinateKind::Pressure => {
            if vertical.reference != VerticalReference::ModelNative
                || vertical.level_values.iter().any(|value| *value <= 0.0)
                || vertical.hybrid_a_pa.is_some()
                || vertical.hybrid_b.is_some()
                || vertical.surface_pressure_dependency.is_some()
            {
                return Err(ContractError::InvalidVerticalCoordinate);
            }
        }
        VerticalCoordinateKind::HybridSigmaPressure => {
            let a = vertical
                .hybrid_a_pa
                .as_ref()
                .ok_or(ContractError::InvalidVerticalCoordinate)?;
            let b = vertical
                .hybrid_b
                .as_ref()
                .ok_or(ContractError::InvalidVerticalCoordinate)?;
            if vertical.reference != VerticalReference::ModelNative
                || a.len() != vertical.level_values.len()
                || b.len() != vertical.level_values.len()
                || a.iter().chain(b.iter()).any(|value| !value.is_finite())
                || vertical.surface_pressure_dependency != Some(FieldId::SurfacePressure)
            {
                return Err(ContractError::InvalidVerticalCoordinate);
            }
        }
    }
    Ok(())
}

fn monotonic(values: &[f32], ordering: VerticalOrdering) -> bool {
    values.windows(2).all(|pair| match ordering {
        VerticalOrdering::Increasing => pair[1] > pair[0],
        VerticalOrdering::Decreasing => pair[1] < pair[0],
    })
}

fn validate_time(id: FieldId, time: &FieldTime) -> Result<(), ContractError> {
    match time.kind {
        TemporalKind::Instantaneous => {
            if time.interval_start_epoch_seconds.is_some()
                || time.interval_end_epoch_seconds.is_some()
                || time.accumulation.is_some()
            {
                return Err(ContractError::InvalidTemporalMetadata(id));
            }
        }
        TemporalKind::IntervalMean | TemporalKind::IntervalTotal => {
            validate_interval(id, time)?;
            if time.accumulation.is_some() {
                return Err(ContractError::InvalidTemporalMetadata(id));
            }
        }
        TemporalKind::AccumulatedSinceReset => {
            validate_interval(id, time)?;
            let reset = time
                .accumulation
                .as_ref()
                .ok_or(ContractError::InvalidTemporalMetadata(id))?;
            let start = time
                .interval_start_epoch_seconds
                .ok_or(ContractError::InvalidTemporalMetadata(id))?;
            if reset.reset_epoch_seconds > start {
                return Err(ContractError::InvalidTemporalMetadata(id));
            }
        }
    }

    if matches!(id, FieldId::LargeScalePrecipitation | FieldId::ConvectivePrecipitation)
        && !matches!(
            time.kind,
            TemporalKind::IntervalTotal | TemporalKind::AccumulatedSinceReset
        )
    {
        return Err(ContractError::InvalidTemporalMetadata(id));
    }
    Ok(())
}

fn validate_interval(id: FieldId, time: &FieldTime) -> Result<(), ContractError> {
    let start = time
        .interval_start_epoch_seconds
        .ok_or(ContractError::InvalidTemporalMetadata(id))?;
    let end = time
        .interval_end_epoch_seconds
        .ok_or(ContractError::InvalidTemporalMetadata(id))?;
    if end <= start || end != time.valid_time_epoch_seconds {
        return Err(ContractError::InvalidTemporalMetadata(id));
    }
    Ok(())
}

fn validate_domain(field: &Field) -> Result<(), ContractError> {
    if field.sign == SignConvention::NonNegative && field.values.iter().any(|value| *value < 0.0) {
        return Err(ContractError::InvalidValueDomain(field.id));
    }
    if matches!(field.id, FieldId::LandSeaMask | FieldId::TotalCloudCover)
        && field.values.iter().any(|value| !(0.0..=1.0).contains(value))
    {
        return Err(ContractError::InvalidValueDomain(field.id));
    }
    if matches!(
        field.id,
        FieldId::Temperature | FieldId::Temperature2m | FieldId::Dewpoint2m
            | FieldId::Pressure | FieldId::SurfacePressure
    ) && field.values.iter().any(|value| *value <= 0.0)
    {
        return Err(ContractError::InvalidValueDomain(field.id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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

    fn field(id: FieldId, values: Vec<f32>) -> Field {
        Field {
            id,
            shape: vec![2, 1, 2],
            axis_order: vec![Axis::X, Axis::Y, Axis::Z],
            unit: id.unit(),
            sign: id.sign(),
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
                hybrid_a_pa: None,
                hybrid_b: None,
                surface_pressure_dependency: None,
            },
            fields: vec![
                field(FieldId::WindU, vec![1.0; 4]),
                field(FieldId::WindV, vec![0.5; 4]),
                field(FieldId::VerticalVelocity, vec![0.0; 4]),
                field(FieldId::Temperature, vec![280.0; 4]),
                field(FieldId::SpecificHumidity, vec![0.004; 4]),
                field(FieldId::Pressure, vec![90_000.0, 80_000.0, 90_000.0, 80_000.0]),
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
        value.fields.retain(|field| field.id != FieldId::SpecificHumidity);
        assert_eq!(
            value.validate(&Requirements::advection()),
            Err(ContractError::MissingRequiredField(FieldId::SpecificHumidity))
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
            Err(ContractError::InvalidTemporalMetadata(FieldId::LargeScalePrecipitation))
        );
    }
}
