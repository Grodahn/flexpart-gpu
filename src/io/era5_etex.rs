//! Bounded ERA5 Complete GRIB2 input profiles for the native ETEX fixture.

use std::path::Path;
use thiserror::Error;

/// Failures at the native ERA5/ETEX provider boundary.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("unsupported ERA5/ETEX v1 valid time {0} (UTC epoch seconds)")]
    UnsupportedTime(i64),
    #[error("cannot read model-level source: {0}")]
    Io(#[from] std::io::Error),
    #[error("model-level source does not match the pinned byte count and SHA-256 for the requested time")]
    SourceIdentity,
    #[error("invalid GRIB2 frame at byte {0}")]
    Frame(usize),
    #[error("ecCodes could not decode the source: {0}")]
    Codec(#[from] eccodes::errors::CodesError),
    #[error("GRIB2 message {message}: unexpected {key}, expected {expected}, got {actual}")]
    Key {
        message: usize,
        key: &'static str,
        expected: String,
        actual: String,
    },
    #[error("unexpected or duplicate model record for {time:04} UTC, parameter {parameter}, level {level}")]
    Record {
        time: i64,
        parameter: i64,
        level: i64,
    },
    #[error("incomplete model record coverage")]
    Coverage,
    #[error(
        "no verified matching surface pressure was supplied; the ARCO derivative is not admitted"
    )]
    SurfacePressureUnavailable,
    #[error("cannot read surface-pressure source: {0}")]
    SurfaceIo(#[source] std::io::Error),
    #[error("surface-pressure source does not match the pinned byte count and SHA-256 for the requested time")]
    SurfaceSourceIdentity,
    #[error("surface-pressure request does not match the pinned ERA5 model-source lineage")]
    SurfaceLineage,
    #[error("unexpected or duplicate surface-pressure record for {0:04} UTC")]
    SurfaceRecord(i64),
    #[error("incomplete surface-pressure record coverage")]
    SurfaceCoverage,
    #[error("GRIB2 message {message}: invalid log surface pressure at source cell {cell}")]
    SurfacePressureValue { message: usize, cell: usize },
    #[error("invalid canonical meteorology payload: {0}")]
    Canonical(#[from] crate::meteorology::ContractError),
    #[error("cannot process decoder provenance: {0}")]
    Serialization(#[from] serde_json::Error),
}

use crate::meteorology::Snapshot;

#[cfg(feature = "eccodes")]
mod native;
#[cfg(feature = "eccodes")]
pub use native::{
    decode_model_levels, load_snapshot_with_surface, ModelLevelsDiagnostic, VerifiedSnapshot,
};

/// Request a hybrid Snapshot without supplying surface pressure.
///
/// # Errors
/// Valid model inputs return a missing-surface error. Invalid inputs fail with
/// their decoding error first. Supply both files to `load_snapshot_with_surface`.
#[cfg(feature = "eccodes")]
pub fn load_snapshot(path: &Path, valid_time_epoch_seconds: i64) -> Result<Snapshot, DecodeError> {
    decode_model_levels(path, valid_time_epoch_seconds)?;
    Err(DecodeError::SurfacePressureUnavailable)
}
