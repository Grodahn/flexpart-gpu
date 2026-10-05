//! Canonical native vertical-coordinate metadata and schema checks.
//!
//! Derived runtime geometry and actual-local-pressure reconstruction belong
//! to `vertical`; this module only checks the serialized coordinate contract.

use super::{ContractError, FieldId};
use serde::{Deserialize, Serialize};

/// Records native levels and optional hybrid interfaces before runtime derivation.
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

/// Identifies the physical quantity represented by native vertical levels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerticalCoordinateKind {
    GeometricHeight,
    Pressure,
    HybridSigmaPressure,
}

/// Identifies the datum of native height or model-coordinate metadata.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerticalReference {
    AboveMeanSeaLevel,
    AboveGroundLevel,
    ModelNative,
}

/// Declares the strict order of serialized native levels and interfaces.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerticalOrdering {
    Increasing,
    Decreasing,
}

/// Locates a canonical field on level centers, interfaces or the surface.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerticalStaggering {
    NotApplicable,
    LevelCenter,
    LevelInterface,
}

pub(super) fn validate_vertical(vertical: &VerticalCoordinate) -> Result<(), ContractError> {
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
                || vertical.level_values.iter().any(|value| *value <= 0.0)
                || interfaces.iter().any(|value| *value < 0.0)
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
