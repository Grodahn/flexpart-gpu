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
/// Number of land-use fractions in the pinned FLEXPART 11.1 dry-deposition contract.
pub const FLEXPART_LAND_USE_CLASS_COUNT: usize = 13;

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
    /// Longitude of the X=0 scalar/cell-center sample.
    ///
    /// Schema v1 fixes the horizontal origin at a cell center. An X-face is
    /// therefore located half a grid step west of the corresponding cell
    /// center, while a Y-face is half a grid step south of it.
    pub xlon0_deg: f64,
    /// Latitude of the Y=0 scalar/cell-center sample.
    pub ylat0_deg: f64,
    pub dx_deg: f64,
    pub dy_deg: f64,
    pub longitude_domain: LongitudeDomain,
}

impl HorizontalGrid {
    /// Whether schema-v1 X coordinates wrap periodically.
    ///
    /// Periodicity is canonical rather than provider metadata: an X grid is
    /// periodic iff its cell coverage nx * dx is exactly 360 degrees within
    /// the schema tolerance. All other grids are non-periodic and must fit
    /// wholly inside the declared longitude domain.
    #[must_use]
    pub fn is_periodic_x(&self) -> bool {
        grid_close(self.dx_deg * self.nx as f64, 360.0)
    }
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
    SnowDepth,
    WindU10m,
    WindV10m,
    Temperature2m,
    Dewpoint2m,
    LargeScalePrecipitation,
    ConvectivePrecipitation,
    TotalCloudCover,
    CloudTotalWater,
    SensibleHeatFlux,
    SurfaceSolarRadiation,
    SurfaceStressEastward,
    SurfaceStressNorthward,
    FrictionVelocity,
    ConvectiveVelocityScale,
    MixingHeight,
    TropopauseHeight,
    InverseObukhovLength,
    LandUseFractions,
}

impl FieldId {
    fn is_3d(self) -> bool {
        matches!(
            self,
            Self::WindU | Self::WindV | Self::VerticalVelocity | Self::Temperature
                | Self::SpecificHumidity | Self::Pressure | Self::AirDensity
                | Self::DensityGradient | Self::CloudTotalWater
        )
    }

    fn class_count(self) -> Option<usize> {
        match self {
            Self::LandUseFractions => Some(FLEXPART_LAND_USE_CLASS_COUNT),
            _ => None,
        }
    }

    fn spec(self) -> &'static FieldSpec {
        FIELD_SPECS
            .iter()
            .find(|spec| spec.id == self)
            .expect("every canonical FieldId must have exactly one FieldSpec")
    }

    fn is_static_ancillary(self) -> bool {
        self.spec().temporal_policy == TemporalPolicy::Static
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
        self.spec().unit
    }

    fn sign(self) -> SignConvention {
        self.spec().sign
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


/// Named physics requirement sets derived from the pinned FLEXPART 11.1 consumer trace.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RequirementSet {
    Advection,
    Convection,
    WetDeposition,
    DryDeposition,
    Settling,
}

/// Allowed time representation at the canonical boundary for one field.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemporalPolicy {
    Static,
    Instantaneous,
    PrecipitationAmount,
    SurfaceFluxRate,
}

/// Single source of truth for field-level canonical semantics that must stay in
/// lockstep with the human-reviewable field matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    pub id: FieldId,
    pub unit: Unit,
    pub sign: SignConvention,
    pub temporal_policy: TemporalPolicy,
    pub requirement_sets: &'static [RequirementSet],
}

pub const FIELD_SPECS: &[FieldSpec] = &[
    FieldSpec { id: FieldId::WindU, unit: Unit::MeterPerSecond, sign: SignConvention::PositiveEastward, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::Advection] },
    FieldSpec { id: FieldId::WindV, unit: Unit::MeterPerSecond, sign: SignConvention::PositiveNorthward, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::Advection] },
    FieldSpec { id: FieldId::VerticalVelocity, unit: Unit::MeterPerSecond, sign: SignConvention::PositiveUpward, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::Advection] },
    FieldSpec { id: FieldId::Temperature, unit: Unit::Kelvin, sign: SignConvention::SignedScalar, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::Convection, RequirementSet::WetDeposition, RequirementSet::Settling] },
    FieldSpec { id: FieldId::SpecificHumidity, unit: Unit::KilogramPerKilogram, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::Convection, RequirementSet::WetDeposition] },
    FieldSpec { id: FieldId::Pressure, unit: Unit::Pascal, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::Convection] },
    FieldSpec { id: FieldId::AirDensity, unit: Unit::KilogramPerCubicMeter, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::WetDeposition, RequirementSet::Settling] },
    FieldSpec { id: FieldId::DensityGradient, unit: Unit::KilogramPerQuarticMeter, sign: SignConvention::SignedScalar, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[] },
    FieldSpec { id: FieldId::SurfacePressure, unit: Unit::Pascal, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::Convection, RequirementSet::DryDeposition] },
    FieldSpec { id: FieldId::Orography, unit: Unit::Meter, sign: SignConvention::SignedScalar, temporal_policy: TemporalPolicy::Static, requirement_sets: &[] },
    FieldSpec { id: FieldId::LandSeaMask, unit: Unit::Fraction, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Static, requirement_sets: &[] },
    FieldSpec { id: FieldId::SnowDepth, unit: Unit::Meter, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::DryDeposition] },
    FieldSpec { id: FieldId::WindU10m, unit: Unit::MeterPerSecond, sign: SignConvention::PositiveEastward, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[] },
    FieldSpec { id: FieldId::WindV10m, unit: Unit::MeterPerSecond, sign: SignConvention::PositiveNorthward, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[] },
    FieldSpec { id: FieldId::Temperature2m, unit: Unit::Kelvin, sign: SignConvention::SignedScalar, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::Convection, RequirementSet::DryDeposition] },
    FieldSpec { id: FieldId::Dewpoint2m, unit: Unit::Kelvin, sign: SignConvention::SignedScalar, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::Convection, RequirementSet::DryDeposition] },
    FieldSpec { id: FieldId::LargeScalePrecipitation, unit: Unit::KilogramPerSquareMeter, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::PrecipitationAmount, requirement_sets: &[RequirementSet::WetDeposition, RequirementSet::DryDeposition] },
    FieldSpec { id: FieldId::ConvectivePrecipitation, unit: Unit::KilogramPerSquareMeter, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::PrecipitationAmount, requirement_sets: &[RequirementSet::WetDeposition, RequirementSet::DryDeposition] },
    FieldSpec { id: FieldId::TotalCloudCover, unit: Unit::Fraction, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::WetDeposition] },
    FieldSpec { id: FieldId::CloudTotalWater, unit: Unit::KilogramPerKilogram, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::WetDeposition] },
    FieldSpec { id: FieldId::SensibleHeatFlux, unit: Unit::WattPerSquareMeter, sign: SignConvention::PositiveUpwardFlux, temporal_policy: TemporalPolicy::SurfaceFluxRate, requirement_sets: &[] },
    FieldSpec { id: FieldId::SurfaceSolarRadiation, unit: Unit::WattPerSquareMeter, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::SurfaceFluxRate, requirement_sets: &[RequirementSet::DryDeposition] },
    FieldSpec { id: FieldId::SurfaceStressEastward, unit: Unit::NewtonPerSquareMeter, sign: SignConvention::PositiveEastward, temporal_policy: TemporalPolicy::SurfaceFluxRate, requirement_sets: &[] },
    FieldSpec { id: FieldId::SurfaceStressNorthward, unit: Unit::NewtonPerSquareMeter, sign: SignConvention::PositiveNorthward, temporal_policy: TemporalPolicy::SurfaceFluxRate, requirement_sets: &[] },
    FieldSpec { id: FieldId::FrictionVelocity, unit: Unit::MeterPerSecond, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::DryDeposition] },
    FieldSpec { id: FieldId::ConvectiveVelocityScale, unit: Unit::MeterPerSecond, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[] },
    FieldSpec { id: FieldId::MixingHeight, unit: Unit::Meter, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[] },
    FieldSpec { id: FieldId::TropopauseHeight, unit: Unit::Meter, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[] },
    FieldSpec { id: FieldId::InverseObukhovLength, unit: Unit::PerMeter, sign: SignConvention::SignedScalar, temporal_policy: TemporalPolicy::Instantaneous, requirement_sets: &[RequirementSet::DryDeposition] },
    FieldSpec { id: FieldId::LandUseFractions, unit: Unit::Fraction, sign: SignConvention::NonNegative, temporal_policy: TemporalPolicy::Static, requirement_sets: &[RequirementSet::DryDeposition] },
];

fn requirements_for(set: RequirementSet) -> Requirements {
    Requirements {
        required_fields: FIELD_SPECS
            .iter()
            .filter(|spec| spec.requirement_sets.contains(&set))
            .map(|spec| spec.id)
            .collect(),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
    Z,
    Class,
}

/// Linearization of the multidimensional canonical field values.
///
/// Schema v1 supports only X-fastest storage. For shape [nx, ny, nz],
/// offset(x,y,z) = x + nx * (y + ny * z); for [nx, ny],
/// offset(x,y) = x + nx * y. Class-axis fields use the same rule:
/// offset(x,y,class) = x + nx * (y + ny * class).
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
    /// Time-invariant ancillary data. The timestamp remains a provenance stamp
    /// only and must not cause temporal interpolation.
    Static,
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
            required_fields: FIELD_SPECS.iter().map(|spec| spec.id).collect(),
        }
    }

    /// Wind components consumed by the production advection path.
    #[must_use]
    pub fn advection() -> Self {
        requirements_for(RequirementSet::Advection)
    }

    /// Meteorological state used by the pinned FLEXPART 11.1 Emanuel-convection path.
    #[must_use]
    pub fn convection() -> Self {
        requirements_for(RequirementSet::Convection)
    }

    /// Canonical inputs needed to derive and sample the pinned wet-deposition forcing.
    #[must_use]
    pub fn wet_deposition() -> Self {
        requirements_for(RequirementSet::WetDeposition)
    }

    /// Physics-ready surface forcing used by the pinned dry-deposition path.
    #[must_use]
    pub fn dry_deposition() -> Self {
        requirements_for(RequirementSet::DryDeposition)
    }

    /// Meteorology consumed by FLEXPART gravitational settling.
    #[must_use]
    pub fn settling() -> Self {
        requirements_for(RequirementSet::Settling)
    }

    /// Fields genuinely represented by the checked-in real-data native-level
    /// fixture (`fixtures/meteorology/era5-etex-native-v1.json`).
    ///
    /// This fixture-specific set is intentionally not a physics requirement set.
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
    #[error("dynamic field {0:?} does not share the snapshot validity time/calendar")]
    InconsistentSnapshotTime(FieldId),
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

    // Schema v1 anchors xlon0/ylat0 at scalar cell centers. Validate the
    // complete Y cell coverage rather than only the origin so #31 never has to
    // guess how an apparently valid grid behaves beyond a pole.
    let south_edge = grid.ylat0_deg - 0.5 * grid.dy_deg;
    let north_edge =
        grid.ylat0_deg + (grid.ny.saturating_sub(1) as f64 + 0.5) * grid.dy_deg;
    if south_edge < -90.0 - GRID_TOLERANCE_DEG || north_edge > 90.0 + GRID_TOLERANCE_DEG {
        return Err(ContractError::InvalidHorizontalGrid);
    }

    let (lon_min, lon_max, origin_valid) = match grid.longitude_domain {
        LongitudeDomain::Minus180To180 => (
            -180.0,
            180.0,
            (-180.0..=180.0).contains(&grid.xlon0_deg),
        ),
        LongitudeDomain::ZeroTo360 => (
            0.0,
            360.0,
            (0.0..360.0).contains(&grid.xlon0_deg),
        ),
    };
    if !origin_valid {
        return Err(ContractError::InvalidHorizontalGrid);
    }

    let x_coverage = grid.dx_deg * grid.nx as f64;
    if x_coverage > 360.0 + GRID_TOLERANCE_DEG {
        return Err(ContractError::InvalidHorizontalGrid);
    }

    // Exactly-global cell coverage is the one supported periodic topology.
    // Regional grids are explicitly non-periodic and may not cross the
    // longitude-domain seam.
    if !grid.is_periodic_x() {
        let west_edge = grid.xlon0_deg - 0.5 * grid.dx_deg;
        let east_edge =
            grid.xlon0_deg + (grid.nx.saturating_sub(1) as f64 + 0.5) * grid.dx_deg;
        if west_edge < lon_min - GRID_TOLERANCE_DEG
            || east_edge > lon_max + GRID_TOLERANCE_DEG
        {
            return Err(ContractError::InvalidHorizontalGrid);
        }
    }
    Ok(())
}

const GRID_TOLERANCE_DEG: f64 = 1.0e-9;

fn grid_close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() <= GRID_TOLERANCE_DEG
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
    let policy = id.spec().temporal_policy;

    match policy {
        TemporalPolicy::Static => {
            if time.kind != TemporalKind::Static
                || time.interval_start_epoch_seconds.is_some()
                || time.interval_end_epoch_seconds.is_some()
                || time.accumulation.is_some()
            {
                return Err(ContractError::InvalidTemporalMetadata(id));
            }
        }
        TemporalPolicy::Instantaneous => {
            if time.kind != TemporalKind::Instantaneous
                || time.interval_start_epoch_seconds.is_some()
                || time.interval_end_epoch_seconds.is_some()
                || time.accumulation.is_some()
            {
                return Err(ContractError::InvalidTemporalMetadata(id));
            }
        }
        TemporalPolicy::PrecipitationAmount => {
            if !matches!(
                time.kind,
                TemporalKind::IntervalTotal | TemporalKind::AccumulatedSinceReset
            ) {
                return Err(ContractError::InvalidTemporalMetadata(id));
            }
            validate_interval(id, time)?;
            match time.kind {
                TemporalKind::IntervalTotal => {
                    if time.accumulation.is_some() {
                        return Err(ContractError::InvalidTemporalMetadata(id));
                    }
                }
                TemporalKind::AccumulatedSinceReset => {
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
                _ => unreachable!("precipitation policy kind checked above"),
            }
        }
        TemporalPolicy::SurfaceFluxRate => {
            if !matches!(time.kind, TemporalKind::Instantaneous | TemporalKind::IntervalMean) {
                return Err(ContractError::InvalidTemporalMetadata(id));
            }
            match time.kind {
                TemporalKind::Instantaneous => {
                    if time.interval_start_epoch_seconds.is_some()
                        || time.interval_end_epoch_seconds.is_some()
                        || time.accumulation.is_some()
                    {
                        return Err(ContractError::InvalidTemporalMetadata(id));
                    }
                }
                TemporalKind::IntervalMean => {
                    validate_interval(id, time)?;
                    if time.accumulation.is_some() {
                        return Err(ContractError::InvalidTemporalMetadata(id));
                    }
                }
                _ => unreachable!("surface-flux policy kind checked above"),
            }
        }
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
    if matches!(
        field.id,
        FieldId::LandSeaMask | FieldId::TotalCloudCover | FieldId::LandUseFractions
    ) && field.values.iter().any(|value| !(0.0..=1.0).contains(value))
    {
        return Err(ContractError::InvalidValueDomain(field.id));
    }
    if field.id == FieldId::LandUseFractions {
        let nx = field.shape[0];
        let ny = field.shape[1];
        for y in 0..ny {
            for x in 0..nx {
                let sum: f32 = (0..FLEXPART_LAND_USE_CLASS_COUNT)
                    .map(|class| field.values[x + nx * (y + ny * class)])
                    .sum();
                if (sum - 1.0).abs() > 1.0e-5 {
                    return Err(ContractError::InvalidValueDomain(field.id));
                }
            }
        }
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

    fn doc_token<T: serde::Serialize>(value: T) -> String {
        match serde_json::to_value(value).expect("serialize contract enum") {
            serde_json::Value::String(value) => value,
            _ => panic!("contract enum must serialize as a string"),
        }
    }

    fn field_spec_doc_row(spec: &FieldSpec) -> String {
        let requirement_sets = if spec.requirement_sets.is_empty() {
            "—".to_string()
        } else {
            spec.requirement_sets
                .iter()
                .map(|set| format!("`{}`", doc_token(*set)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "| `{}` | `{}` | `{}` | `{}` | {} |",
            doc_token(spec.id),
            doc_token(spec.unit),
            doc_token(spec.sign),
            doc_token(spec.temporal_policy),
            requirement_sets
        )
    }

    #[test]
    fn field_specs_are_unique_and_define_all_p0_requirements() {
        let ids: BTreeSet<_> = FIELD_SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids.len(), FIELD_SPECS.len(), "duplicate FieldSpec id");
        assert_eq!(Requirements::p0_complete().required_fields, ids);
    }

    #[test]
    fn advection_requirement_is_wind_only() {
        assert_eq!(
            Requirements::advection().required_fields,
            [
                FieldId::WindU,
                FieldId::WindV,
                FieldId::VerticalVelocity,
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn documentation_field_spec_matrix_matches_contract() {
        let docs = include_str!("../../docs/meteorology-contract.md");
        let begin = docs
            .find("<!-- BEGIN GENERATED FIELD SPEC MATRIX -->")
            .expect("generated field-spec matrix begin marker");
        let end = docs
            .find("<!-- END GENERATED FIELD SPEC MATRIX -->")
            .expect("generated field-spec matrix end marker");
        assert!(end > begin, "generated field-spec matrix markers out of order");
        let block = &docs[begin..end];

        for spec in FIELD_SPECS {
            let row = field_spec_doc_row(spec);
            assert!(
                block.contains(&row),
                "docs field-spec matrix is stale or missing row: {row}"
            );
        }
        let data_rows = block.lines().filter(|line| line.starts_with("| `")).count();
        assert_eq!(
            data_rows,
            FIELD_SPECS.len(),
            "docs field-spec matrix has stale or extra data rows"
        );
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
            time: instant(),
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
            Err(ContractError::InconsistentSnapshotTime(FieldId::Temperature))
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
            Err(ContractError::InconsistentSnapshotTime(FieldId::Temperature))
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
        assert!(decoded.is_err(), "unsupported calendars must fail deserialization");
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
