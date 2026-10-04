//! Canonical schema identity and shared fail-closed boundary errors.

use super::FieldId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable canonical meteorology schema id.
pub const SCHEMA_ID: &str = "flexpart-gpu.canonical-meteorology";
/// Current canonical meteorology schema version.
pub const SCHEMA_VERSION: u32 = 1;
/// Identifies the canonical meteorology schema accepted by consumers.
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
/// Preserves the shared fail-closed errors of canonical snapshot validation.
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
