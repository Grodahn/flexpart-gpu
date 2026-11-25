//! Bounded ERA5 Complete GRIB2 input profile for the native ETEX fixture (#237).

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
    #[error("ERA5/ETEX v1 has no verified matching surface pressure; the ARCO derivative is not admitted")]
    SurfacePressureUnavailable,
    #[error("invalid canonical model-level payload: {0}")]
    Canonical(#[from] crate::meteorology::ContractError),
    #[error("cannot serialize diagnostic provenance: {0}")]
    Serialization(#[from] serde_json::Error),
}

use crate::meteorology::Snapshot;

#[cfg(feature = "eccodes")]
mod native;
#[cfg(feature = "eccodes")]
pub use native::{decode_model_levels, ModelLevelsDiagnostic};

/// Request a complete hybrid Snapshot from this profile.
///
/// # Errors
/// The checked-in input profile has no independently verified matching surface
/// pressure. Invalid model input fails with its decoding error first.
#[cfg(feature = "eccodes")]
pub fn load_snapshot(path: &Path, valid_time_epoch_seconds: i64) -> Result<Snapshot, DecodeError> {
    decode_model_levels(path, valid_time_epoch_seconds)?;
    Err(DecodeError::SurfacePressureUnavailable)
}
