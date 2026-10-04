//! Timestamp conversion, timestep magnitude and bracket bounds.

use crate::io::{TemporalInterpolationError, TimeBoundsBehavior};
use crate::simulation::timeloop::error::TimeLoopError;

pub(super) fn timestep_seconds_f32(value: i64) -> Result<f32, TimeLoopError> {
    let dt = value as f32;
    if !dt.is_finite() || dt <= 0.0 {
        return Err(TimeLoopError::InvalidTimestep { value });
    }
    Ok(dt)
}

pub(super) fn interpolation_alpha(
    time_t0_seconds: i64,
    time_t1_seconds: i64,
    target_time_seconds: i64,
    bounds_behavior: TimeBoundsBehavior,
) -> Result<f32, TimeLoopError> {
    if time_t1_seconds <= time_t0_seconds {
        return Err(TemporalInterpolationError::InvalidTimeBracket {
            time_t0_seconds,
            time_t1_seconds,
        }
        .into());
    }
    let effective_target = if (time_t0_seconds..=time_t1_seconds).contains(&target_time_seconds) {
        target_time_seconds
    } else {
        match bounds_behavior {
            TimeBoundsBehavior::Strict => {
                return Err(TemporalInterpolationError::TargetOutsideBracket {
                    target_time_seconds,
                    time_t0_seconds,
                    time_t1_seconds,
                }
                .into());
            }
            TimeBoundsBehavior::Clamp => {
                target_time_seconds.clamp(time_t0_seconds, time_t1_seconds)
            }
        }
    };

    let elapsed_seconds = (effective_target - time_t0_seconds) as f64;
    let window_seconds = (time_t1_seconds - time_t0_seconds) as f64;
    Ok((elapsed_seconds / window_seconds) as f32)
}

pub(super) fn parse_timestamp_seconds(value: &str) -> Result<i64, TimeLoopError> {
    if value.len() != 14 || !value.chars().all(|c| c.is_ascii_digit()) {
        return Err(TimeLoopError::InvalidTimestamp {
            value: value.to_string(),
        });
    }

    let year = value[0..4]
        .parse::<i32>()
        .map_err(|_| TimeLoopError::InvalidTimestamp {
            value: value.to_string(),
        })?;
    let month = value[4..6]
        .parse::<u32>()
        .map_err(|_| TimeLoopError::InvalidTimestamp {
            value: value.to_string(),
        })?;
    let day = value[6..8]
        .parse::<u32>()
        .map_err(|_| TimeLoopError::InvalidTimestamp {
            value: value.to_string(),
        })?;
    let hour = value[8..10]
        .parse::<u32>()
        .map_err(|_| TimeLoopError::InvalidTimestamp {
            value: value.to_string(),
        })?;
    let minute = value[10..12]
        .parse::<u32>()
        .map_err(|_| TimeLoopError::InvalidTimestamp {
            value: value.to_string(),
        })?;
    let second = value[12..14]
        .parse::<u32>()
        .map_err(|_| TimeLoopError::InvalidTimestamp {
            value: value.to_string(),
        })?;

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(TimeLoopError::InvalidTimestamp {
            value: value.to_string(),
        });
    }

    let days = days_from_civil(year, month, day);
    Ok(days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))
}

pub(super) fn format_timestamp_seconds(seconds: i64) -> Result<String, TimeLoopError> {
    let days = seconds.div_euclid(86_400);
    let sod = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(0..=9999).contains(&year) {
        return Err(TimeLoopError::TimestampOutOfRange { seconds });
    }

    let hour = sod / 3_600;
    let minute = (sod % 3_600) / 60;
    let second = sod % 60;
    Ok(format!(
        "{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}"
    ))
}

/// Gregorian civil date to days since Unix epoch (1970-01-01).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let mut y = i64::from(year);
    let m = i64::from(month);
    let d = i64::from(day);

    if m <= 2 {
        y -= 1;
    }
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`], returns `(year, month, day)`.
fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    if m <= 2 {
        y += 1;
    }
    (y as i32, m as u32, d as u32)
}
