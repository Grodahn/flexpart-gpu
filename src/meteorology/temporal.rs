//! Canonical temporal interpolation of instantaneous meteorology fields for
//! issue #74.
//!
//! This module samples an ordered series of canonical #29 snapshots at a
//! requested validity time using the temporal semantics frozen by #71. It
//! reimplements the pinned FLEXPART 11.1 equations without rediscovering them:
//!
//! - `find_time_vars` (`interpol_mod.f90:190-198`):
//!   `dt1 = itime - memtime(1)`, `dt2 = memtime(2) - itime`,
//!   `dtt = 1 / (dt1 + dt2)`;
//! - `temporal_interpolation` (`interpol_mod.f90:531-537`):
//!   `output = (time1*dt2 + time2*dt1) * dtt`.
//!
//! The candidate matches the frozen oracle at an exact source timestamp and
//! interpolates linearly between the two bracketing snapshots inside any
//! interval. First and last endpoints are the inclusive snapshot values.
//!
//! Fail-closed policy, per #74: the raw Fortran primitives extrapolate without
//! a range guard, while the canonical API rejects requests before the first or
//! after the last snapshot timestamp instead of inventing values. Duplicate,
//! non-monotonic, missing or inconsistent timestamps, mixed calendars, and
//! non-instantaneous fields also fail closed. Timestamp units and calendars
//! come from the canonical `FieldTime` and are validated, never guessed.
//!
//! This ticket owns temporal interpolation of [`TemporalPolicy::Instantaneous`]
//! fields only. Accumulated (`PrecipitationAmount`), surface flux
//! (`SurfaceFluxRate`) and static fields are rejected; #75 owns accumulated/
//! interval/reset semantics. Horizontal (#72), vertical (#73), canonical 4D (#76)
//! and consumer migration (#77) are out of scope.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    Calendar, FieldId, HorizontalStaggering, SchemaIdentity, Snapshot, TemporalKind,
    TemporalPolicy, VerticalStaggering,
};

/// Schema identity of the machine-readable comparison report emitted for #74.
pub const REPORT_SCHEMA_ID: &str = "flexpart-gpu.temporal-interpolation-report";
/// Version of the comparison report schema emitted for #74.
pub const REPORT_SCHEMA_VERSION: u32 = 1;
/// Direct-call description of the candidate recorded in #74 comparison reports.
pub const CANDIDATE_DESCRIPTION: &str = "meteorology::temporal::sample_field";
/// Absolute portion of the default #74 comparison tolerance.
///
/// Applied as `ABS_TOL + REL_TOL * |oracle|`, matching the #72 candidate test.
pub const DEFAULT_ABS_TOL: f64 = 1.0e-6;
/// Relative portion of the default #74 comparison tolerance (`1e-5`, the
/// acceptance threshold for f32-representable synthetic time-linear values).
pub const DEFAULT_REL_TOL: f64 = 1.0e-5;

/// Requested sample time in canonical epoch seconds under one canonical
/// calendar. The calendar must equal the series calendar and is never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestedSampleTime {
    /// Canonical timestamp representation of the request.
    pub calendar: Calendar,
    /// Seconds since 1970-01-01T00:00:00Z under the declared calendar.
    pub epoch_seconds: i64,
}

impl RequestedSampleTime {
    /// Build a request for `epoch_seconds` under `calendar`.
    #[must_use]
    pub const fn new(calendar: Calendar, epoch_seconds: i64) -> Self {
        Self {
            calendar,
            epoch_seconds,
        }
    }
}

/// How one requested time relates to the ordered source snapshot series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalApplication {
    /// Requested time equals the first snapshot timestamp; the value is that
    /// snapshot unchanged.
    FirstEndpoint,
    /// Requested time equals the last snapshot timestamp; the value is that
    /// snapshot unchanged.
    LastEndpoint,
    /// Requested time equals an interior snapshot timestamp.
    ExactSourceTimestamp,
    /// Requested time lies strictly inside one bracketing snapshot pair and is
    /// linearly interpolated.
    LinearInterior,
}

/// Time-stamped source slice of one canonical snapshot used by the sampler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTimestamps {
    /// Lower bracketing snapshot timestamp in seconds.
    pub lower_epoch_seconds: i64,
    /// Upper bracketing snapshot timestamp in seconds.
    pub upper_epoch_seconds: i64,
}

/// Temporal weights in the exact shape reported by the #71 oracle (`DTS`).
///
/// `dt1` equals `itime - memtime(1)`, `dt2` equals `memtime(2) - itime` and
/// `dtt` equals `1 / (dt1 + dt2)`. The normalized blend weights follow the
/// FLEXPART temporal formula `(time1*dt2 + time2*dt1) * dtt`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TemporalWeights {
    /// Elapsed seconds from the lower snapshot timestamp (`dt1`).
    pub dt1_seconds: f64,
    /// Remaining seconds to the upper snapshot timestamp (`dt2`).
    pub dt2_seconds: f64,
    /// Inverse of the bracketing span (`dtt = 1 / (dt1 + dt2)`).
    pub inverse_span_per_second: f64,
}

impl TemporalWeights {
    /// Normalized blend weights `(w_lower, w_upper) = (dt2*dtt, dt1*dtt)`.
    #[must_use]
    pub fn normalized(self) -> (f64, f64) {
        (
            self.dt2_seconds * self.inverse_span_per_second,
            self.dt1_seconds * self.inverse_span_per_second,
        )
    }
}

/// One temporal sample of a whole canonical field.
#[derive(Debug, Clone, PartialEq)]
pub struct TemporalSample {
    /// Canonical field identity that was sampled.
    pub field_id: FieldId,
    /// Request that produced the sample.
    pub request: RequestedSampleTime,
    /// How the requested time relates to the source series.
    pub application: TemporalApplication,
    /// Active bracketing source timestamps.
    pub source_timestamps: SourceTimestamps,
    /// Temporal weights in `DTS`-aligned form.
    pub weights: TemporalWeights,
    /// Elementwise blended field values in canonical storage order.
    pub values: Vec<f32>,
}

/// Fail-closed errors for canonical temporal sampling (#74).
#[derive(Debug, Error, PartialEq)]
pub enum TemporalError {
    /// The field's canonical temporal policy is not instantaneous.
    #[error(
        "field {field_id:?} uses temporal policy {temporal_policy:?}; #74 samples instantaneous fields only"
    )]
    UnsupportedTemporalPolicy {
        field_id: FieldId,
        temporal_policy: TemporalPolicy,
    },
    /// Fewer than two snapshots cannot bracket any request.
    #[error("temporal sampling requires at least two snapshots, got {snapshots}")]
    InsufficientTemporalCoverage { snapshots: usize },
    /// A series snapshot lacks the requested field, so its timestamp and values
    /// are missing.
    #[error("snapshot {snapshot_index} does not contain field {field_id:?}")]
    MissingFieldSnapshot {
        snapshot_index: usize,
        field_id: FieldId,
    },
    /// A series snapshot holds the field as a non-instantaneous quantity.
    #[error("snapshot {snapshot_index} field {field_id:?} has kind {kind:?}, not instantaneous")]
    NonInstantaneousSnapshot {
        snapshot_index: usize,
        field_id: FieldId,
        kind: TemporalKind,
    },
    /// The same field uses different calendars across the series.
    #[error("inconsistent calendar across temporal series: {calendar_a:?} then {calendar_b:?}")]
    InconsistentCalendar {
        calendar_a: Calendar,
        calendar_b: Calendar,
    },
    /// The request calendar differs from the series calendar.
    #[error("requested calendar {requested:?} does not match series calendar {series:?}")]
    CalendarMismatch {
        requested: Calendar,
        series: Calendar,
    },
    /// Duplicate or descending snapshot timestamps are not a valid chronology.
    #[error(
        "snapshot timestamps are not strictly increasing at index {index}: {previous} then {current}"
    )]
    NonMonotonicTimestamps {
        index: usize,
        previous: i64,
        current: i64,
    },
    /// The field changes element count across the series.
    #[error(
        "field {field_id:?} shape is inconsistent at snapshot {snapshot_index}: expected {expected} values, got {actual}"
    )]
    InconsistentFieldShape {
        field_id: FieldId,
        snapshot_index: usize,
        expected: usize,
        actual: usize,
    },
    /// The field changes staggering across the series.
    #[error("field {field_id:?} staggering is inconsistent at snapshot {snapshot_index}")]
    InconsistentStaggering {
        field_id: FieldId,
        snapshot_index: usize,
    },
    /// The field carries a non-finite value.
    #[error("non-finite value in field {field_id:?} at snapshot {snapshot_index}, element {element_index}")]
    NonFiniteValue {
        field_id: FieldId,
        snapshot_index: usize,
        element_index: usize,
    },
    /// Request before the first snapshot timestamp (extrapolation fails closed).
    #[error(
        "requested time {requested} is before the first source timestamp {first}; extrapolation is unsupported"
    )]
    BeforeFirstCoverage { requested: i64, first: i64 },
    /// Request after the last snapshot timestamp (extrapolation fails closed).
    #[error(
        "requested time {requested} is after the last source timestamp {last}; extrapolation is unsupported"
    )]
    AfterLastCoverage { requested: i64, last: i64 },
    /// Element index out of range for the sampled field.
    #[error("element index {element_index} exceeds field length {length}")]
    OutOfBoundsElement { element_index: usize, length: usize },
    /// A comparison without oracle queries cannot provide validation evidence.
    #[error("temporal comparison requires at least one oracle query")]
    EmptyOracleQueries,
    /// The blended result is non-finite. Defensive guard: finite f32 inputs
    /// with `[0, 1]` weights cannot reach this, but the check protects the
    /// blend against future weight-scheme changes.
    #[error("non-finite temporal sample result")]
    NonFiniteResult,
}

/// One validated, time-ordered subset of a snapshot that the sampler reads.
struct SeriesEntry<'a> {
    timestamp_epoch_seconds: i64,
    calendar: Calendar,
    horizontal_staggering: HorizontalStaggering,
    vertical_staggering: VerticalStaggering,
    shape: &'a [usize],
    values: &'a [f32],
}

/// Extract and fail-closed validate the ordered instantaneous field series.
///
/// The resulting series is strictly increasing in time, uses one calendar at
/// the canonical field boundary, and has a consistent shape and staggering.
fn extract_series<'a>(
    field_id: FieldId,
    snapshots: &[&'a Snapshot],
) -> Result<Vec<SeriesEntry<'a>>, TemporalError> {
    let temporal_policy = field_id.spec().temporal_policy;
    if temporal_policy != TemporalPolicy::Instantaneous {
        return Err(TemporalError::UnsupportedTemporalPolicy {
            field_id,
            temporal_policy,
        });
    }
    if snapshots.len() < 2 {
        return Err(TemporalError::InsufficientTemporalCoverage {
            snapshots: snapshots.len(),
        });
    }

    let mut series = Vec::with_capacity(snapshots.len());
    let mut series_calendar: Option<Calendar> = None;
    for (snapshot_index, snapshot) in snapshots.iter().enumerate() {
        let field = snapshot
            .fields
            .iter()
            .find(|field| field.id == field_id)
            .ok_or(TemporalError::MissingFieldSnapshot {
                snapshot_index,
                field_id,
            })?;
        if field.time.kind != TemporalKind::Instantaneous {
            return Err(TemporalError::NonInstantaneousSnapshot {
                snapshot_index,
                field_id,
                kind: field.time.kind,
            });
        }
        match series_calendar {
            Some(calendar) if calendar != field.time.calendar => {
                return Err(TemporalError::InconsistentCalendar {
                    calendar_a: calendar,
                    calendar_b: field.time.calendar,
                });
            }
            None => series_calendar = Some(field.time.calendar),
            Some(_) => {}
        }
        for (element_index, value) in field.values.iter().enumerate() {
            if !value.is_finite() {
                return Err(TemporalError::NonFiniteValue {
                    field_id,
                    snapshot_index,
                    element_index,
                });
            }
        }
        series.push(SeriesEntry {
            timestamp_epoch_seconds: field.time.valid_time_epoch_seconds,
            calendar: field.time.calendar,
            horizontal_staggering: field.horizontal_staggering,
            vertical_staggering: field.vertical_staggering,
            shape: &field.shape,
            values: &field.values,
        });
    }

    for index in 1..series.len() {
        if series[index].timestamp_epoch_seconds <= series[index - 1].timestamp_epoch_seconds {
            return Err(TemporalError::NonMonotonicTimestamps {
                index,
                previous: series[index - 1].timestamp_epoch_seconds,
                current: series[index].timestamp_epoch_seconds,
            });
        }
    }

    let first = &series[0];
    for (snapshot_index, entry) in series.iter().enumerate().skip(1) {
        if entry.shape != first.shape || entry.values.len() != first.values.len() {
            return Err(TemporalError::InconsistentFieldShape {
                field_id,
                snapshot_index,
                expected: first.values.len(),
                actual: entry.values.len(),
            });
        }
        if entry.horizontal_staggering != first.horizontal_staggering
            || entry.vertical_staggering != first.vertical_staggering
        {
            return Err(TemporalError::InconsistentStaggering {
                field_id,
                snapshot_index,
            });
        }
    }
    Ok(series)
}

/// Sample one canonical instantaneous field at a requested validity time.
///
/// The series must be ordered from oldest to newest. Requests strictly between
/// two snapshots are linearly interpolated with the #71-frozen FLEXPART
/// formula; requests exactly at a snapshot timestamp return that snapshot
/// unchanged. Requests outside the ordered window, or any inconsistent or
/// non-instantaneous chronology, fail closed (see [`TemporalError`]).
///
/// # Errors
/// Returns [`TemporalError`] for unsupported temporal policies, insufficient
/// or non-monotonic coverage, missing or non-instantaneous snapshots,
/// inconsistent calendars/shapes, non-finite inputs, and requests outside the
/// supported coverage window.
///
/// # Panics
/// Only on internal invariants established during series validation: at least
/// two snapshots, and a bracketing pair for any request within the validated
/// series coverage. A caller cannot trigger these panics with public inputs.
pub fn sample_field(
    field_id: FieldId,
    snapshots: &[&Snapshot],
    request: RequestedSampleTime,
) -> Result<TemporalSample, TemporalError> {
    let series = extract_series(field_id, snapshots)?;
    if request.calendar != series[0].calendar {
        return Err(TemporalError::CalendarMismatch {
            requested: request.calendar,
            series: series[0].calendar,
        });
    }
    if request.epoch_seconds < series[0].timestamp_epoch_seconds {
        return Err(TemporalError::BeforeFirstCoverage {
            requested: request.epoch_seconds,
            first: series[0].timestamp_epoch_seconds,
        });
    }
    let last = series
        .last()
        .expect("extract_series guarantees at least two snapshots")
        .timestamp_epoch_seconds;
    if request.epoch_seconds > last {
        return Err(TemporalError::AfterLastCoverage {
            requested: request.epoch_seconds,
            last,
        });
    }

    let exact_index = series
        .iter()
        .position(|entry| entry.timestamp_epoch_seconds == request.epoch_seconds);
    let application = match exact_index {
        Some(0) => TemporalApplication::FirstEndpoint,
        Some(index) if index + 1 == series.len() => TemporalApplication::LastEndpoint,
        Some(_) => TemporalApplication::ExactSourceTimestamp,
        None => TemporalApplication::LinearInterior,
    };

    // The leftmost bracketing pair that contains the request. For an exact
    // interior timestamp this is (previous, exact) because the request equals
    // the upper member there; first and last endpoints reduce to the
    // all-inclusive edge pairs.
    let lower_index = (0..series.len() - 1)
        .find(|&index| {
            series[index].timestamp_epoch_seconds <= request.epoch_seconds
                && request.epoch_seconds <= series[index + 1].timestamp_epoch_seconds
        })
        .expect("request within coverage must have a bracketing pair");
    let upper_index = lower_index + 1;
    let lower = &series[lower_index];
    let upper = &series[upper_index];

    // Calculate in i128 first so every ordered pair of public i64 timestamps
    // remains overflow-safe before conversion to the interpolation precision.
    let dt1_i128 = i128::from(request.epoch_seconds) - i128::from(lower.timestamp_epoch_seconds);
    let dt2_i128 = i128::from(upper.timestamp_epoch_seconds) - i128::from(request.epoch_seconds);
    #[allow(clippy::cast_precision_loss)]
    let dt1 = dt1_i128 as f64;
    #[allow(clippy::cast_precision_loss)]
    let dt2 = dt2_i128 as f64;
    let span = dt1 + dt2;
    debug_assert!(span > 0.0 && dt1 >= 0.0 && dt2 >= 0.0);
    let inverse_span = 1.0 / span;
    let weight_lower = dt2 * inverse_span;
    let weight_upper = dt1 * inverse_span;

    let mut values = Vec::with_capacity(lower.values.len());
    for (lower_value, upper_value) in lower.values.iter().zip(upper.values) {
        let blended =
            weight_lower * f64::from(*lower_value) + weight_upper * f64::from(*upper_value);
        if !blended.is_finite() {
            return Err(TemporalError::NonFiniteResult);
        }
        // f32 rounding is intentional: canonical fields store f32 samples.
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let value = blended as f32;
        if !value.is_finite() {
            return Err(TemporalError::NonFiniteResult);
        }
        values.push(value);
    }

    Ok(TemporalSample {
        field_id,
        request,
        application,
        source_timestamps: SourceTimestamps {
            lower_epoch_seconds: lower.timestamp_epoch_seconds,
            upper_epoch_seconds: upper.timestamp_epoch_seconds,
        },
        weights: TemporalWeights {
            dt1_seconds: dt1,
            dt2_seconds: dt2,
            inverse_span_per_second: inverse_span,
        },
        values,
    })
}

/// Predeclared pass/fail tolerance applied as `abs + rel * |oracle|`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tolerance {
    /// Absolute tolerance portion.
    pub absolute: f64,
    /// Relative tolerance portion.
    pub relative: f64,
}

impl Tolerance {
    /// Predeclare an absolute/relative comparison tolerance.
    #[must_use]
    pub const fn new(absolute: f64, relative: f64) -> Self {
        Self { absolute, relative }
    }

    /// Whether `candidate` agrees with `oracle` within the tolerance rule.
    #[must_use]
    pub fn allows(self, candidate: f64, oracle: f64) -> bool {
        (candidate - oracle).abs() <= self.absolute + self.relative * oracle.abs()
    }

    /// Absolute difference between candidate and oracle.
    #[must_use]
    pub fn absolute_difference(self, candidate: f64, oracle: f64) -> f64 {
        (candidate - oracle).abs()
    }
}

/// Pass/fail verdict of one comparison facet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The facet agrees with the oracle within tolerance.
    Pass,
    /// The facet disagrees with the oracle or sampling failed closed.
    Fail,
}

impl Verdict {
    /// Whether the verdict is a pass.
    #[must_use]
    pub const fn is_pass(self) -> bool {
        matches!(self, Self::Pass)
    }

    /// Build a verdict from a pass/fail boolean.
    #[must_use]
    pub const fn from_bool(passed: bool) -> Self {
        if passed {
            Self::Pass
        } else {
            Self::Fail
        }
    }
}

/// Oracle `DTS` triple `[dt1, dt2, dtt]` for the bracketing pair, as reported
/// by the #71 temporal-bilinear fixture.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OracleDts {
    /// Oracle `dt1` seconds.
    pub dt1_seconds: f64,
    /// Oracle `dt2` seconds.
    pub dt2_seconds: f64,
    /// Oracle `dtt`.
    pub inverse_span_per_second: f64,
}

/// One candidate-vs-oracle sample comparison requested by a scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OracleQuery {
    /// Requested validity time in epoch seconds.
    pub requested_time_epoch_seconds: i64,
    /// Which field element to compare.
    pub element_index: usize,
    /// Independent oracle/expected element value.
    pub oracle_value: f32,
    /// Optional #71 `DTS` oracle for the expected bracketing weights.
    #[serde(default)]
    pub oracle_dts: Option<OracleDts>,
    /// Optional expected application classification.
    #[serde(default)]
    pub expected_application: Option<TemporalApplication>,
}

impl OracleQuery {
    /// Build a value-only query; weight and application expectations stay unset.
    #[must_use]
    pub const fn new(
        requested_time_epoch_seconds: i64,
        element_index: usize,
        oracle_value: f32,
    ) -> Self {
        Self {
            requested_time_epoch_seconds,
            element_index,
            oracle_value,
            oracle_dts: None,
            expected_application: None,
        }
    }
}

/// One machine-readable comparison row required by #74.
///
/// Records the field identity, the active source timestamps, the requested
/// timestamp, the interpolation weights (`DTS` aligned), the candidate and
/// oracle values, the tolerance and the verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonRow {
    /// Canonical field identity that was sampled.
    pub field_id: FieldId,
    /// Compared element index.
    pub element_index: usize,
    /// Requested validity time in epoch seconds.
    pub requested_time_epoch_seconds: i64,
    /// Active bracketing source timestamps.
    pub source_timestamps: SourceTimestamps,
    /// Temporal application classification of the request.
    pub application: TemporalApplication,
    /// Candidate temporal weights in `DTS`-aligned form.
    pub weights: TemporalWeights,
    /// Candidate element value.
    pub candidate_value: f32,
    /// Independent oracle/expected element value.
    pub oracle_value: f32,
    /// Predeclared comparison tolerance.
    pub tolerance: Tolerance,
    /// Absolute candidate-oracle difference.
    pub absolute_difference: f64,
    /// Whether the value comparison passes.
    pub value_verdict: Verdict,
    /// Whether the oracle `DTS` weights match (passes trivially when absent).
    pub weights_verdict: Verdict,
    /// Whether the expected application classification matches.
    pub application_verdict: Verdict,
    /// Combined verdict of this row.
    pub row_verdict: Verdict,
}

/// Machine-readable candidate-vs-oracle comparison report for #74.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonReport {
    /// Report schema identity.
    pub schema: SchemaIdentity,
    /// Scenario identifier that produced the report.
    pub scenario_id: String,
    /// Candidate description recorded for traceability.
    pub candidate: String,
    /// Canonical field identity under test.
    pub field_id: FieldId,
    /// Validated series calendar.
    pub calendar: Calendar,
    /// Full ordered series of source timestamps.
    pub source_timestamps_epoch_seconds: Vec<i64>,
    /// Predeclared comparison tolerance.
    pub tolerance: Tolerance,
    /// Per-query comparison rows.
    pub rows: Vec<ComparisonRow>,
    /// Overall report verdict.
    pub status: Verdict,
}

/// Build a machine-readable comparison report for one scenario.
///
/// Samples every query through the canonical candidate and compares candidate
/// values, `DTS` weights and application classifications against the expected
/// oracle entries. Any sampling failure or out-of-range element fails closed.
///
/// # Errors
/// Returns [`TemporalError`] if any query cannot be sampled or references an
/// invalid element index.
pub fn build_comparison_report(
    scenario_id: &str,
    field_id: FieldId,
    snapshots: &[&Snapshot],
    tolerance: Tolerance,
    queries: &[OracleQuery],
) -> Result<ComparisonReport, TemporalError> {
    if queries.is_empty() {
        return Err(TemporalError::EmptyOracleQueries);
    }
    let series = extract_series(field_id, snapshots)?;
    let calendar = series[0].calendar;
    let length = series[0].values.len();
    let source_timestamps_epoch_seconds: Vec<i64> = series
        .iter()
        .map(|entry| entry.timestamp_epoch_seconds)
        .collect();

    let mut rows = Vec::with_capacity(queries.len());
    for query in queries {
        if query.element_index >= length {
            return Err(TemporalError::OutOfBoundsElement {
                element_index: query.element_index,
                length,
            });
        }
        let request = RequestedSampleTime {
            calendar,
            epoch_seconds: query.requested_time_epoch_seconds,
        };
        let sample = sample_field(field_id, snapshots, request)?;
        let candidate_value = sample.values[query.element_index];
        let value_verdict = Verdict::from_bool(
            tolerance.allows(f64::from(candidate_value), f64::from(query.oracle_value)),
        );
        let weights_verdict = match query.oracle_dts {
            Some(oracle_dts) => Verdict::from_bool(
                tolerance.allows(sample.weights.dt1_seconds, oracle_dts.dt1_seconds)
                    && tolerance.allows(sample.weights.dt2_seconds, oracle_dts.dt2_seconds)
                    && tolerance.allows(
                        sample.weights.inverse_span_per_second,
                        oracle_dts.inverse_span_per_second,
                    ),
            ),
            None => Verdict::Pass,
        };
        let application_verdict = match query.expected_application {
            Some(expected) => Verdict::from_bool(sample.application == expected),
            None => Verdict::Pass,
        };
        let row_verdict = Verdict::from_bool(
            value_verdict.is_pass() && weights_verdict.is_pass() && application_verdict.is_pass(),
        );
        rows.push(ComparisonRow {
            field_id,
            element_index: query.element_index,
            requested_time_epoch_seconds: query.requested_time_epoch_seconds,
            source_timestamps: sample.source_timestamps,
            application: sample.application,
            weights: sample.weights,
            candidate_value,
            oracle_value: query.oracle_value,
            tolerance,
            absolute_difference: tolerance
                .absolute_difference(f64::from(candidate_value), f64::from(query.oracle_value)),
            value_verdict,
            weights_verdict,
            application_verdict,
            row_verdict,
        });
    }

    let status = Verdict::from_bool(rows.iter().all(|row| row.row_verdict.is_pass()));
    Ok(ComparisonReport {
        schema: SchemaIdentity {
            id: REPORT_SCHEMA_ID.to_string(),
            version: REPORT_SCHEMA_VERSION,
        },
        scenario_id: scenario_id.to_string(),
        candidate: CANDIDATE_DESCRIPTION.to_string(),
        field_id,
        calendar,
        source_timestamps_epoch_seconds,
        tolerance,
        rows,
        status,
    })
}
