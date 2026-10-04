//! Shared fail-closed error vocabulary for the canonical vertical boundary.

use super::super::{ContractError, FieldId, VerticalReference};
use thiserror::Error;

/// Errors returned by canonical vertical transformations.
#[derive(Debug, Error, PartialEq)]
pub enum VerticalTransformError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("vertical transform requires hybrid sigma-pressure coordinates")]
    UnsupportedVerticalCoordinate,
    #[error("missing canonical field {0:?}")]
    MissingField(FieldId),
    #[error("invalid local surface pressure at (x={x}, y={y}): {pressure_pa} Pa")]
    InvalidSurfacePressure {
        x: usize,
        y: usize,
        pressure_pa: f32,
    },
    #[error("hybrid surface interface at (x={x}, y={y}) is {interface_pressure_pa} Pa but local surface pressure is {surface_pressure_pa} Pa")]
    SurfaceInterfacePressureMismatch {
        x: usize,
        y: usize,
        interface_pressure_pa: f32,
        surface_pressure_pa: f32,
    },
    #[error("reconstructed pressure is invalid/non-monotonic at (x={x}, y={y}, index={index})")]
    InvalidPressureColumn { x: usize, y: usize, index: usize },
    #[error("invalid surface thermodynamic state at (x={x}, y={y}): T2m={temperature_k} K, Td2m={dewpoint_k} K, ps={pressure_pa} Pa")]
    InvalidSurfaceThermodynamics {
        x: usize,
        y: usize,
        temperature_k: f32,
        dewpoint_k: f32,
        pressure_pa: f32,
    },
    #[error("invalid model-level thermodynamic state at (x={x}, y={y}, z={z}): T={temperature_k} K, q={specific_humidity}")]
    InvalidThermodynamics {
        x: usize,
        y: usize,
        z: usize,
        temperature_k: f32,
        specific_humidity: f32,
    },
    #[error("invalid reconstructed height at (x={x}, y={y}, z={z}): {height_agl_m} m AGL")]
    InvalidHeightColumn {
        x: usize,
        y: usize,
        z: usize,
        height_agl_m: f32,
    },
    #[error("shape mismatch for {field}: expected {expected}, got {actual}")]
    ShapeMismatch {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("runtime vertical shape mismatch for {field}: expected {expected}, got {actual}")]
    RuntimeShapeMismatch {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error(
        "vertical runtime index out of bounds: x={x}, y={y}, z={z:?}, shape=({nx},{ny},{nz:?})"
    )]
    RuntimeIndexOutOfBounds {
        x: usize,
        y: usize,
        z: Option<usize>,
        nx: usize,
        ny: usize,
        nz: Option<usize>,
    },
    #[error("release height must use explicit AGL or ASL reference, got {reference:?}")]
    UnsupportedReleaseHeightReference { reference: VerticalReference },
    #[error(
        "invalid release height {height_m} m for {reference:?} over terrain {terrain_asl_m} m ASL"
    )]
    InvalidReleaseHeight {
        height_m: f32,
        reference: VerticalReference,
        terrain_asl_m: f32,
    },
    #[error("invalid release height range {lower_m}..{upper_m} m for {reference:?}")]
    InvalidReleaseHeightRange {
        lower_m: f32,
        upper_m: f32,
        reference: VerticalReference,
    },
    #[error("unsupported or ambiguous native vertical-motion semantics: {reason}")]
    InvalidNativeVerticalMotion { reason: &'static str },
    #[error("invalid native vertical-motion value at index {index}: {value}")]
    InvalidNativeVerticalMotionValue { index: usize, value: f32 },
    #[error(
        "eta-dot preprocessing produced non-finite pressure velocity at (x={x}, y={y}, level={k})"
    )]
    InvalidEtaDotTransform { x: usize, y: usize, k: usize },
    #[error("failed to serialize {input} for provenance hashing")]
    ProvenanceSerialization { input: &'static str },
    #[error("vertical-motion conversion requires at least two model levels")]
    InsufficientVerticalLevels,
    #[error("invalid dz/dp conversion at (x={x}, y={y}, z={z})")]
    InvalidPressureToHeightDerivative { x: usize, y: usize, z: usize },
    #[error("invalid ASL height at (x={x}, y={y}, z={z}): ASL={height_asl_m} m, terrain={terrain_asl_m} m")]
    InvalidAbsoluteHeight {
        x: usize,
        y: usize,
        z: usize,
        height_asl_m: f32,
        terrain_asl_m: f32,
    },
    #[error("height at (x={x}, y={y}, z={z}) lies below terrain: ASL={height_asl_m} m, terrain={terrain_asl_m} m")]
    HeightBelowTerrain {
        x: usize,
        y: usize,
        z: usize,
        height_asl_m: f32,
        terrain_asl_m: f32,
    },
}
