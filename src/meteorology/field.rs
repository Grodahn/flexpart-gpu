//! Canonical field matrix, payload layout and physics requirement sets.
//!
//! This is the sole owner of field units, signs, time policies, supported
//! staggering and physical value domains; it performs no sampling.

use super::{ContractError, FieldTime, VerticalStaggering};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Number of land-use fractions in the pinned FLEXPART 11.1 dry-deposition contract.
pub const FLEXPART_LAND_USE_CLASS_COUNT: usize = 13;
/// Identifies a provider-independent physical field in the canonical matrix.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    pub(super) fn is_3d(self) -> bool {
        matches!(
            self,
            Self::WindU
                | Self::WindV
                | Self::VerticalVelocity
                | Self::Temperature
                | Self::SpecificHumidity
                | Self::Pressure
                | Self::AirDensity
                | Self::DensityGradient
                | Self::CloudTotalWater
        )
    }

    pub(super) fn class_count(self) -> Option<usize> {
        match self {
            Self::LandUseFractions => Some(FLEXPART_LAND_USE_CLASS_COUNT),
            _ => None,
        }
    }

    pub(super) fn spec(self) -> &'static FieldSpec {
        FIELD_SPECS
            .iter()
            .find(|spec| spec.id == self)
            .expect("every canonical FieldId must have exactly one FieldSpec")
    }

    pub(super) fn supports_horizontal_staggering(self, staggering: HorizontalStaggering) -> bool {
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

    pub(super) fn supports_vertical_staggering(self, staggering: VerticalStaggering) -> bool {
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

    pub(super) fn unit(self) -> Unit {
        self.spec().unit
    }

    pub(super) fn sign(self) -> SignConvention {
        self.spec().sign
    }
}

/// Declares the physical unit of normalized canonical field values.
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

/// Declares the direction or admissible sign of a canonical physical quantity.
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
    PblTurbulence,
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

/// Authoritative field semantics and named consumer membership for schema v1.
pub const FIELD_SPECS: &[FieldSpec] = &[
    FieldSpec {
        id: FieldId::WindU,
        unit: Unit::MeterPerSecond,
        sign: SignConvention::PositiveEastward,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::Advection, RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::WindV,
        unit: Unit::MeterPerSecond,
        sign: SignConvention::PositiveNorthward,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::Advection, RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::VerticalVelocity,
        unit: Unit::MeterPerSecond,
        sign: SignConvention::PositiveUpward,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::Advection],
    },
    FieldSpec {
        id: FieldId::Temperature,
        unit: Unit::Kelvin,
        sign: SignConvention::SignedScalar,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[
            RequirementSet::PblTurbulence,
            RequirementSet::Convection,
            RequirementSet::WetDeposition,
            RequirementSet::Settling,
        ],
    },
    FieldSpec {
        id: FieldId::SpecificHumidity,
        unit: Unit::KilogramPerKilogram,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[
            RequirementSet::PblTurbulence,
            RequirementSet::Convection,
            RequirementSet::WetDeposition,
        ],
    },
    FieldSpec {
        id: FieldId::Pressure,
        unit: Unit::Pascal,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::Convection],
    },
    FieldSpec {
        id: FieldId::AirDensity,
        unit: Unit::KilogramPerCubicMeter,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[
            RequirementSet::PblTurbulence,
            RequirementSet::WetDeposition,
            RequirementSet::Settling,
        ],
    },
    FieldSpec {
        id: FieldId::DensityGradient,
        unit: Unit::KilogramPerQuarticMeter,
        sign: SignConvention::SignedScalar,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::SurfacePressure,
        unit: Unit::Pascal,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[
            RequirementSet::PblTurbulence,
            RequirementSet::Convection,
            RequirementSet::DryDeposition,
        ],
    },
    FieldSpec {
        id: FieldId::Orography,
        unit: Unit::Meter,
        sign: SignConvention::SignedScalar,
        temporal_policy: TemporalPolicy::Static,
        requirement_sets: &[],
    },
    FieldSpec {
        id: FieldId::LandSeaMask,
        unit: Unit::Fraction,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Static,
        requirement_sets: &[],
    },
    FieldSpec {
        id: FieldId::SnowDepth,
        unit: Unit::Meter,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::DryDeposition],
    },
    FieldSpec {
        id: FieldId::WindU10m,
        unit: Unit::MeterPerSecond,
        sign: SignConvention::PositiveEastward,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::WindV10m,
        unit: Unit::MeterPerSecond,
        sign: SignConvention::PositiveNorthward,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::Temperature2m,
        unit: Unit::Kelvin,
        sign: SignConvention::SignedScalar,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[
            RequirementSet::PblTurbulence,
            RequirementSet::Convection,
            RequirementSet::DryDeposition,
        ],
    },
    FieldSpec {
        id: FieldId::Dewpoint2m,
        unit: Unit::Kelvin,
        sign: SignConvention::SignedScalar,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[
            RequirementSet::PblTurbulence,
            RequirementSet::Convection,
            RequirementSet::DryDeposition,
        ],
    },
    FieldSpec {
        id: FieldId::LargeScalePrecipitation,
        unit: Unit::KilogramPerSquareMeter,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::PrecipitationAmount,
        requirement_sets: &[RequirementSet::WetDeposition, RequirementSet::DryDeposition],
    },
    FieldSpec {
        id: FieldId::ConvectivePrecipitation,
        unit: Unit::KilogramPerSquareMeter,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::PrecipitationAmount,
        requirement_sets: &[RequirementSet::WetDeposition, RequirementSet::DryDeposition],
    },
    FieldSpec {
        id: FieldId::TotalCloudCover,
        unit: Unit::Fraction,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::WetDeposition],
    },
    FieldSpec {
        id: FieldId::CloudTotalWater,
        unit: Unit::KilogramPerKilogram,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::WetDeposition],
    },
    FieldSpec {
        id: FieldId::SensibleHeatFlux,
        unit: Unit::WattPerSquareMeter,
        sign: SignConvention::PositiveUpwardFlux,
        temporal_policy: TemporalPolicy::SurfaceFluxRate,
        requirement_sets: &[RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::SurfaceSolarRadiation,
        unit: Unit::WattPerSquareMeter,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::SurfaceFluxRate,
        requirement_sets: &[RequirementSet::DryDeposition],
    },
    FieldSpec {
        id: FieldId::SurfaceStressEastward,
        unit: Unit::NewtonPerSquareMeter,
        sign: SignConvention::PositiveEastward,
        temporal_policy: TemporalPolicy::SurfaceFluxRate,
        requirement_sets: &[RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::SurfaceStressNorthward,
        unit: Unit::NewtonPerSquareMeter,
        sign: SignConvention::PositiveNorthward,
        temporal_policy: TemporalPolicy::SurfaceFluxRate,
        requirement_sets: &[RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::FrictionVelocity,
        unit: Unit::MeterPerSecond,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::PblTurbulence, RequirementSet::DryDeposition],
    },
    FieldSpec {
        id: FieldId::ConvectiveVelocityScale,
        unit: Unit::MeterPerSecond,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::MixingHeight,
        unit: Unit::Meter,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::TropopauseHeight,
        unit: Unit::Meter,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::PblTurbulence],
    },
    FieldSpec {
        id: FieldId::InverseObukhovLength,
        unit: Unit::PerMeter,
        sign: SignConvention::SignedScalar,
        temporal_policy: TemporalPolicy::Instantaneous,
        requirement_sets: &[RequirementSet::PblTurbulence, RequirementSet::DryDeposition],
    },
    FieldSpec {
        id: FieldId::LandUseFractions,
        unit: Unit::Fraction,
        sign: SignConvention::NonNegative,
        temporal_policy: TemporalPolicy::Static,
        requirement_sets: &[RequirementSet::DryDeposition],
    },
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

/// Identifies the physical or land-use-class axes of a canonical field.
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

/// Locates field samples relative to scalar cell centers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HorizontalStaggering {
    CellCenter,
    XFace,
    YFace,
}

/// Carries normalized physical values with explicit layout and time semantics.
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

/// Selects fields that must exist for a declared physics consumer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Requirements {
    pub required_fields: BTreeSet<FieldId>,
}

impl Requirements {
    /// Requires every physical field defined by the canonical P0 matrix.
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

    /// Meteorological state and derived diagnostics consumed by the pinned
    /// FLEXPART 11.1 PBL/turbulence path.
    #[must_use]
    pub fn pbl_turbulence() -> Self {
        requirements_for(RequirementSet::PblTurbulence)
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

pub(super) fn validate_domain(field: &Field) -> Result<(), ContractError> {
    if field.sign == SignConvention::NonNegative && field.values.iter().any(|value| *value < 0.0) {
        return Err(ContractError::InvalidValueDomain(field.id));
    }
    if matches!(
        field.id,
        FieldId::LandSeaMask | FieldId::TotalCloudCover | FieldId::LandUseFractions
    ) && field
        .values
        .iter()
        .any(|value| !(0.0..=1.0).contains(value))
    {
        return Err(ContractError::InvalidValueDomain(field.id));
    }
    if field.id == FieldId::SpecificHumidity
        && field
            .values
            .iter()
            .any(|value| !(0.0..=1.0).contains(value))
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
        FieldId::Temperature
            | FieldId::Temperature2m
            | FieldId::Dewpoint2m
            | FieldId::Pressure
            | FieldId::SurfacePressure
    ) && field.values.iter().any(|value| *value <= 0.0)
    {
        return Err(ContractError::InvalidValueDomain(field.id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            [FieldId::WindU, FieldId::WindV, FieldId::VerticalVelocity,]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn pbl_turbulence_requirement_is_machine_readable() {
        assert_eq!(
            Requirements::pbl_turbulence().required_fields,
            [
                FieldId::WindU,
                FieldId::WindV,
                FieldId::Temperature,
                FieldId::SpecificHumidity,
                FieldId::AirDensity,
                FieldId::DensityGradient,
                FieldId::SurfacePressure,
                FieldId::WindU10m,
                FieldId::WindV10m,
                FieldId::Temperature2m,
                FieldId::Dewpoint2m,
                FieldId::SensibleHeatFlux,
                FieldId::SurfaceStressEastward,
                FieldId::SurfaceStressNorthward,
                FieldId::FrictionVelocity,
                FieldId::ConvectiveVelocityScale,
                FieldId::MixingHeight,
                FieldId::TropopauseHeight,
                FieldId::InverseObukhovLength,
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
        assert!(
            end > begin,
            "generated field-spec matrix markers out of order"
        );
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
    fn documentation_detailed_matrix_covers_every_canonical_field() {
        let docs = include_str!("../../docs/meteorology-contract.md");
        let begin = docs
            .find("<!-- BEGIN DETAILED FIELD TRACE MATRIX -->")
            .expect("detailed field-trace matrix begin marker");
        let end = docs
            .find("<!-- END DETAILED FIELD TRACE MATRIX -->")
            .expect("detailed field-trace matrix end marker");
        assert!(
            end > begin,
            "detailed field-trace matrix markers out of order"
        );
        let block = &docs[begin..end];

        for spec in FIELD_SPECS {
            let token = format!("\x60{}\x60", doc_token(spec.id));
            assert!(
                block.contains(&token),
                "detailed oracle/consumer matrix does not cover canonical field {token}"
            );
        }
    }
}
