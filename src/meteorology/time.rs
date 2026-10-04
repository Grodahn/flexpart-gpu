//! Canonical source timestamps, calendars and field-policy validation.
//!
//! Instantaneous sampling belongs to `temporal`; derived accumulated intervals
//! belong to `accumulation`. This module performs neither transformation.

use super::{ContractError, FieldId, TemporalPolicy};
use serde::{Deserialize, Serialize};

/// Declares how canonical epoch seconds identify meteorological validity times.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Calendar {
    Gregorian,
    ProlepticGregorian,
}

/// Distinguishes state, static data, interval rates and accumulated amounts.
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

/// Records the explicit origin of an accumulated meteorological amount.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accumulation {
    pub reset_epoch_seconds: i64,
}

/// Records source validity time and any explicit represented interval/reset.
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

pub(super) fn validate_time(id: FieldId, time: &FieldTime) -> Result<(), ContractError> {
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
                    if reset.reset_epoch_seconds != start {
                        return Err(ContractError::InvalidTemporalMetadata(id));
                    }
                }
                _ => unreachable!("precipitation policy kind checked above"),
            }
        }
        TemporalPolicy::SurfaceFluxRate => {
            if !matches!(
                time.kind,
                TemporalKind::Instantaneous | TemporalKind::IntervalMean
            ) {
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
