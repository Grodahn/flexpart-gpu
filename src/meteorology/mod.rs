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
    pub hybrid_a_interface_pa: Option<Vec<f32>>,
    #[serde(default)]
    pub hybrid_b_interface: Option<Vec<f32>>,
    #[serde(default)]
    pub reference_surface_pressure_pa: Option<f32>,
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

    fn supports_horizontal_staggering(self, staggering: HorizontalStaggering) -> bool {
        match self {
            Self::WindU => matches!(
                staggering,
                HorizontalStaggering::CellCenter | HorizontalStaggering::XFace
            ),
            Self::WindV => matches!(
                staggering,
                HorizontalStaggering::CellCenter | HorizontalStaggering::YFace
            ),
            _ => staggering == HorizontalStaggering::CellCenter,
        }
    }

    fn supports_vertical_staggering(self, staggering: VerticalStaggering) -> bool {
        if !self.is_3d() {
            return staggering == VerticalStaggering::NotApplicable;
        }
        match self {
            Self::VerticalVelocity => matches!(
                staggering,
                VerticalStaggering::LevelCenter | VerticalStaggering::LevelInterface
            ),
            _ => staggering == VerticalStaggering::LevelCenter,
        }
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

/// Linearization of the multidimensional canonical field values.
///
/// Schema v1 supports only X-fastest storage. For shape [nx, ny, nz],
/// offset(x,y,z) = x + nx * (y + ny * z); for [nx, ny],
/// offset(x,y) = x + nx * y.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageOrder {
    XFastest,
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
    pub storage_order: StorageOrder,
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

    /// Fields genuinely represented by the checked-in real-data native-level
    /// fixture (`fixtures/meteorology/era5-etex-native-v1.json`).
    ///
    /// The native ERA5 snapshot carries provider-agnostic wind, temperature,
    /// humidity and surface pressure together with the native hybrid interface A/B
    /// metadata. It intentionally does not satisfy [`Self::advection`]:
    /// reconstructed 3-D pressure and canonical upward-positive vertical
    /// velocity are hybrid transforms owned by #30 and are not part of this
    /// fixture.
    #[must_use]
    pub fn real_data_native_levels() -> Self {
        Self {
            required_fields: [
                FieldId::WindU,
                FieldId::WindV,
                FieldId::Temperature,
                FieldId::SpecificHumidity,
                FieldId::SurfacePressure,
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
                || vertical.hybrid_a_interface_pa.is_some()
                || vertical.hybrid_b_interface.is_some()
                || vertical.reference_surface_pressure_pa.is_some()
                || vertical.surface_pressure_dependency.is_some()
            {
                return Err(ContractError::InvalidVerticalCoordinate);
            }
        }
        VerticalCoordinateKind::Pressure => {
            if vertical.reference != VerticalReference::ModelNative
                || vertical.level_values.iter().any(|value| *value <= 0.0)
                || vertical.hybrid_a_interface_pa.is_some()
                || vertical.hybrid_b_interface.is_some()
                || vertical.reference_surface_pressure_pa.is_some()
                || vertical.surface_pressure_dependency.is_some()
            {
                return Err(ContractError::InvalidVerticalCoordinate);
            }
        }
        VerticalCoordinateKind::HybridSigmaPressure => {
            let interfaces = vertical
                .interface_values
                .as_ref()
                .ok_or(ContractError::InvalidVerticalCoordinate)?;
            let a = vertical
                .hybrid_a_interface_pa
                .as_ref()
                .ok_or(ContractError::InvalidVerticalCoordinate)?;
            let b = vertical
                .hybrid_b_interface
                .as_ref()
                .ok_or(ContractError::InvalidVerticalCoordinate)?;
            let reference_surface_pressure_pa = vertical
                .reference_surface_pressure_pa
                .ok_or(ContractError::InvalidVerticalCoordinate)?;
            if vertical.reference != VerticalReference::ModelNative
                || a.len() != vertical.level_values.len() + 1
                || b.len() != vertical.level_values.len() + 1
                || interfaces.len() != vertical.level_values.len() + 1
                || a.iter().chain(b.iter()).any(|value| !value.is_finite())
                || !reference_surface_pressure_pa.is_finite()
                || reference_surface_pressure_pa <= 0.0
                || vertical.surface_pressure_dependency != Some(FieldId::SurfacePressure)
            {
                return Err(ContractError::InvalidVerticalCoordinate);
            }

            for ((interface_pressure, a_pa), b_fraction) in
                interfaces.iter().zip(a.iter()).zip(b.iter())
            {
                let reconstructed = *a_pa + *b_fraction * reference_surface_pressure_pa;
                if !vertical_close(*interface_pressure, reconstructed) {
                    return Err(ContractError::InvalidVerticalCoordinate);
                }
            }
            for (level_pressure, half_levels) in
                vertical.level_values.iter().zip(interfaces.windows(2))
            {
                let reconstructed = 0.5 * (half_levels[0] + half_levels[1]);
                if !vertical_close(*level_pressure, reconstructed) {
                    return Err(ContractError::InvalidVerticalCoordinate);
                }
            }
        }
    }
    Ok(())
}

fn vertical_close(actual: f32, expected: f32) -> bool {
    let tolerance = 0.05_f32.max(expected.abs() * 1.0e-6);
    (actual - expected).abs() <= tolerance
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
            Err(ContractError::InvalidTemporalMetadata(FieldId::LargeScalePrecipitation))
        );
    }
}
