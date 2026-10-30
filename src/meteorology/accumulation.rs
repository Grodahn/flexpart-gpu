//! Issue #75: accumulated-field interval/reset semantics.
//!
//! Provider-accumulated meteorological fields (principally precipitation) arrive
//! at the canonical boundary as `AccumulatedSinceReset` totals
//! ([`super::TemporalKind`]). Each declared snapshot carries the validity time,
//! the declared accumulation run start `reset_epoch_seconds`, and a
//! water-equivalent total in kg/m2. Schema v1 requires
//! `reset_epoch_seconds == interval_start_epoch_seconds` and
//! `valid_time_epoch_seconds == interval_end_epoch_seconds`, so the window a
//! source value covers is fully explicit before this module is called.
//!
//! This module defines and validates the canonical transformation from that
//! accumulated representation into per-interval **amounts** and **rates** consumed
//! by downstream sampling. It is the ownership boundary recorded by the #71
//! interpolation contract:
//!
//! > `reset_deaccumulation: not_performed_here; owned_by_issue_75`
//!
//! FLEXPART `interpol_rain` (`interpol_mod.f90:1209-1582`) samples `lsprec` /
//! `convprec` as already-normalized mm/h rates; deaccumulation is done upstream
//! of that boundary, here, not by emulating the interpolation quirk
//! `dtt = dt/3` (`interpol_mod.f90:1302-1316`). The derived mm/h rate is the
//! handoff unit to the #71 contract, while interval amounts and SI rates are the
//! physically distinctive outputs.
//!
//! ## Fail-closed semantics
//!
//! Given an ordered sequence of observations `(t_i, R_i, a_i)` with strictly
//! increasing valid times `t_i`, declared reset origins `R_i`, and non-negative
//! kg/m2 amounts `a_i`, resolution:
//!
//! - derives the leading interval `[R_0, t_0]` from the first source amount;
//! - within a declared run (`R_i == R_{i-1}`) treats `a_i - a_{i-1}` over
//!   `[t_{i-1}, t_i]` as the interval amount, requiring monotonic growth
//!   (a negative delta is **not** a reset);
//! - at a contiguous declared reset (`R_i == t_{i-1}`) treats `a_i` over
//!   `[t_{i-1}, t_i]` as the interval amount of the freshly declared run;
//! - fails closed on: empty sequences, non-increasing valid times, resets that
//!   move backwards, resets outside the source window, resets that fall inside a
//!   previously declared window, uncovered coverage gaps after a forward reset,
//!   negative deltas within a run, negative amounts, and non-finite amounts.
//!
//! Two representations are structurally distinguished: interval-integrated
//! **amount** (`kg/m2`, numerically equal to mm water depth) and **rate**
//! (`kg/(m2 s)` plus the `mm/h` handoff value). No global reset cadence is
//! assumed; reset metadata is always an explicit input.
//!
//! ## Same-run decreases from GRIB packing (#216, implemented by #219)
//!
//! A same-run decrease is admitted only through the `*_with_sources` entry
//! points, only when both observations carry a [`SourceQuantization`] record and
//! the frozen #216 rule ([`classify_packing_decrease`]) classifies the pair
//! `ADMIT_BOUNDED`. The admitted interval has amount and rates `+0.0`; the signed
//! difference and its bound are kept as a typed [`PackingDecreaseAdmission`],
//! never inside an amount or rate newtype. The next interval differences from the
//! unmodified lower value, so the summed interval amounts equal the final total
//! plus the projected decreases. Without a record on either observation the
//! decrease remains [`AccumulationError::NegativeDelta`]; with a record that does
//! not admit it, [`AccumulationError::PackingDecreaseRejected`] carries the #216
//! verdict and code. Interval metadata is unchanged by an admission and remains
//! cell-invariant.

use serde::Serialize;
use thiserror::Error;

use super::{FieldId, SourceQuantization, Unit};

mod packing_decrease;

pub use packing_decrease::{
    classify_packing_decrease, AdmittedPackingBound, PackingClassification,
    PackingDecreaseAdmission, PackingDecreaseCode, PackingDecreasePair, PackingDecreaseVerdict,
    PackingObservation, GRIB_PACKING_DECISION_SCHEMA,
};

/// Seconds in one hour used for the #71 `mm/h` rate handoff conversion.
const SECONDS_PER_HOUR: f64 = 3600.0;

/// Stable schema id of the machine-readable accumulation report.
pub const ACCUMULATION_CONTRACT_SCHEMA: &str = "flexpart-gpu.accumulation-contract";
/// Current accumulation contract schema version.
pub const ACCUMULATION_CONTRACT_VERSION: u32 = 1;

/// Source/amount unit this transformation is defined for.
pub const SOURCE_UNIT: &str = "kilogram_per_square_meter";
/// SI rate unit derived by the transformation.
pub const RATE_SI_UNIT: &str = "kilogram_per_square_meter_per_second";
/// Handoff rate unit consumed by the #71 sampling contract.
pub const RATE_HANDOFF_UNIT: &str = "millimeter_per_hour";

/// Relative tolerance for interval-amount/rate preservation in canonical tests
/// (acceptance criterion: preserve interval-integrated precipitation to
/// `1e-6` relative).
pub const RELATIVE_TOLERANCE: f64 = 1.0e-6;

/// Water-equivalent interval-integrated precipitation in kg/m2
/// (numerically equal to mm water depth).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmountKgPerSquareMeter(pub f64);

/// Precipitation rate in kg/(m2 s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateKgPerSquareMeterPerSecond(pub f64);

/// Precipitation rate in millimeters of water per hour; the #71 handoff unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateMillimeterPerHour(pub f64);

impl From<AmountKgPerSquareMeter> for f64 {
    fn from(value: AmountKgPerSquareMeter) -> Self {
        value.0
    }
}

impl From<RateKgPerSquareMeterPerSecond> for f64 {
    fn from(value: RateKgPerSquareMeterPerSecond) -> Self {
        value.0
    }
}

impl From<RateMillimeterPerHour> for f64 {
    fn from(value: RateMillimeterPerHour) -> Self {
        value.0
    }
}

/// One scalar observation of an accumulated field at one grid cell.
///
/// The amount is a total accumulated since `reset_epoch_seconds` and read at
/// `valid_time_epoch_seconds`, expressed in kg/m2. Schema v1 requires the
/// equivalent of `reset_epoch_seconds < valid_time_epoch_seconds`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccumulatedObservation {
    /// Moment at which `accumulated_amount_kg_per_square_meter` was observed.
    pub valid_time_epoch_seconds: i64,
    /// Declared start of the accumulation run that produced the amount.
    pub reset_epoch_seconds: i64,
    /// Water-equivalent total accumulated since the reset in kg/m2.
    pub accumulated_amount_kg_per_square_meter: f64,
}

/// Validated interval/reset metadata shared by CPU reference derivation and
/// GPU input preparation.
///
/// This control-plane record intentionally contains no derived amount or rate.
/// Production GPU callers use it to preserve the #75 fail-closed chronology
/// without executing the production numerical transformation on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccumulatedIntervalMetadata {
    /// Start of the represented interval in epoch seconds.
    pub interval_start_epoch_seconds: i64,
    /// End of the represented interval in epoch seconds.
    pub interval_end_epoch_seconds: i64,
    /// Strictly positive interval duration in seconds.
    pub duration_seconds: i64,
    /// Whether the interval amount is the current fresh-run accumulation.
    pub reset_applied: bool,
    /// Whether this row declares a new run relative to the previous row.
    pub detected_reset: bool,
}

/// One derived interval product for the coverage between two observation
/// instants (or, for the first observation, between the declared reset and the
/// first valid time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntervalProduct {
    /// Start of the derived interval (explicit, never inferred from a cadence).
    pub interval_start_epoch_seconds: i64,
    /// End of the derived interval (always an observation valid time or the
    /// declared reset for the leading interval).
    pub interval_end_epoch_seconds: i64,
    /// Interval-integrated precipitation amount in kg/m2.
    pub amount_kg_per_square_meter: AmountKgPerSquareMeter,
    /// Quotient `amount / duration` in kg/(m2 s).
    pub rate_kg_per_square_meter_per_second: RateKgPerSquareMeterPerSecond,
    /// Handoff rate in mm/h equal to the SI rate times 3600; 1 kg/m2 == 1 mm.
    pub rate_millimeter_per_hour: RateMillimeterPerHour,
    /// True when the interval amount was taken from the current observation's
    /// accumulated value rather than a within-run difference.
    pub reset_applied: bool,
}

/// Result of resolving an ordered accumulated-observation sequence into
/// contiguous interval products.
#[derive(Debug, Clone, PartialEq)]
pub struct IntervalSequence {
    /// Intervals in observation order; every interval has a strictly positive
    /// duration and a non-negative amount.
    pub intervals: Vec<IntervalProduct>,
    /// Number of newly declared reset boundaries detected from the source
    /// metadata (`R_i > R_{i-1}` for `i > 0`).
    pub detected_reset_count: usize,
    /// #216 admissions; their intervals carry `+0.0` amount and rates.
    pub quantization_admissions: Vec<PackingDecreaseAdmission>,
}

impl IntervalSequence {
    /// Sum of all derived interval amounts in kg/m2.
    ///
    /// For a single-run sequence this telescopes to the final source amount plus
    /// [`Self::projected_decrease_kg_per_square_meter`].
    #[must_use]
    pub fn total_amount_kg_per_square_meter(&self) -> f64 {
        self.intervals
            .iter()
            .map(|product| product.amount_kg_per_square_meter.0)
            .sum()
    }

    /// Sum of the admitted decreases projected to `+0.0`, in kg/m2 (`>= 0`).
    #[must_use]
    pub fn projected_decrease_kg_per_square_meter(&self) -> f64 {
        self.quantization_admissions
            .iter()
            .map(|admission| -admission.signed_difference_kg_per_square_meter)
            .sum()
    }
}

/// Validated interval metadata plus the #216 admissions of one cell series.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedIntervalSequence {
    /// Cell-invariant #75 metadata, identical to [`validate_interval_sequence`].
    pub intervals: Vec<AccumulatedIntervalMetadata>,
    /// Admitted same-run decreases of this series, in interval order.
    pub quantization_admissions: Vec<PackingDecreaseAdmission>,
}

/// One cell's accumulated observations with the identity the #216 rule needs.
///
/// `sources[i]` is the optional source record of `observations[i]`'s field. No
/// record may be synthesized; pass `None` when the provider supplied none.
#[derive(Debug, Clone, Copy)]
pub struct AccumulatedSeries<'a> {
    /// Canonical field of every observation.
    pub field_id: FieldId,
    /// Canonical unit of every accumulated value.
    pub canonical_unit: Unit,
    /// Shared horizontal grid identity.
    pub grid_id: &'a str,
    /// Horizontal cell index, recorded in admissions.
    pub cell_index: usize,
    /// Observations in source order.
    pub observations: &'a [AccumulatedObservation],
    /// One optional source record per observation.
    pub sources: &'a [Option<&'a SourceQuantization>],
}

/// How a same-run decrease is treated.
#[derive(Clone, Copy)]
enum DecreasePolicy<'a> {
    /// Every decrease is `NegativeDelta` (the original #75 rule).
    Strict,
    /// Decreases are classified with the #216 rule.
    Provenance(&'a AccumulatedSeries<'a>),
}

/// Why an accumulated sequence cannot be resolved (all fail-closed cases).
#[derive(Debug, Clone, PartialEq, Error)]
pub enum AccumulationError {
    /// The sequence contains no observations.
    #[error("accumulation sequence is empty")]
    EmptySequence,
    /// Valid times must be strictly increasing; equal timestamps are ambiguous.
    #[error(
        "observation {index}: valid time must strictly increase (previous {previous}, current {current})"
    )]
    NonIncreasingTimestamps {
        index: usize,
        previous: i64,
        current: i64,
    },
    /// Reset origins may not move backwards over the sequence.
    #[error(
        "observation {index}: reset origin moved backwards (previous {previous}, current {current})"
    )]
    BackwardsReset {
        index: usize,
        previous: i64,
        current: i64,
    },
    /// A reset origin must lie strictly before the observation valid time.
    #[error("observation {index}: reset origin {reset} is not before valid time {valid_time}")]
    ResetNotBeforeValidTime {
        index: usize,
        reset: i64,
        valid_time: i64,
    },
    /// A forward reset that lands inside a previously declared window
    /// contradicts that observation's stated coverage.
    #[error(
        "observation {index}: reset origin {reset} falls inside the previously declared window (previous valid time {previous_valid_time})"
    )]
    MidWindowReset {
        index: usize,
        reset: i64,
        previous_valid_time: i64,
    },
    /// A forward reset leaves a coverage gap; no rule may fabricate the missing
    /// interval.
    #[error(
        "observation {index}: reset origin {reset} leaves uncovered coverage after previous valid time {previous_valid_time}"
    )]
    UncoveredGap {
        index: usize,
        previous_valid_time: i64,
        reset: i64,
    },
    /// Within a declared run the accumulated amount decreased; a negative delta
    /// is never interpreted as a reset.
    #[error(
        "observation {index}: same-run accumulated amount decreased from {previous} to {current} without a declared reset"
    )]
    NegativeDelta {
        index: usize,
        previous: f64,
        current: f64,
    },
    /// A same-run decrease with source records that the #216 rule does not admit.
    #[error(
        "observation {index}: same-run accumulated amount decreased from {previous} to {current}; #216 classification {verdict:?}/{code:?} does not admit it"
    )]
    PackingDecreaseRejected {
        index: usize,
        previous: f64,
        current: f64,
        verdict: PackingDecreaseVerdict,
        code: PackingDecreaseCode,
    },
    /// The series supplies a different number of source records than observations.
    #[error("{sources} source records for {observations} observations")]
    SourceCountMismatch { observations: usize, sources: usize },
    /// Source amounts are non-negative by the canonical sign contract.
    #[error("observation {index}: source amount {amount} is negative")]
    NegativeAmount { index: usize, amount: f64 },
    /// Non-finite source amounts are rejected like every canonical value.
    #[error("observation {index}: source amount is not finite")]
    NonFiniteAmount { index: usize },
    /// The interval endpoints cannot be represented as a positive `i64`
    /// duration and therefore cannot be transferred unambiguously.
    #[error(
        "observation {index}: interval duration from {start} to {end} is outside the supported positive i64 range"
    )]
    IntervalDurationOutOfRange { index: usize, start: i64, end: i64 },
}

/// Verdict attached to one report row.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "state", content = "reason")]
pub enum Verdict {
    /// The interval amount/rate were derived successfully.
    Passed,
    /// Resolution failed closed; the payload is the canonical error text.
    Failed(String),
}

/// One machine-readable evidence row: source observation, declared metadata,
/// derived interval amount/rate, optional oracle comparison and verdict.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IntervalEvidence {
    /// Index of the source observation this row describes.
    pub source_index: usize,
    /// Source observation valid time.
    pub source_valid_time_epoch_seconds: i64,
    /// Source-declared reset origin (run start).
    pub source_reset_epoch_seconds: i64,
    /// Source-declared accumulated amount in kg/m2.
    pub source_accumulated_amount_kg_per_square_meter: f64,
    /// Declared interval window `[reset, valid]` as recorded by the source.
    pub declared_interval_start_epoch_seconds: i64,
    pub declared_interval_end_epoch_seconds: i64,
    /// True when the source metadata declares a new run relative to the
    /// previous observation (`reset > previous reset`).
    pub detected_reset: bool,
    /// True when the derived amount was taken from the observed accumulated
    /// value instead of a within-run difference.
    pub reset_applied: bool,
    /// Derived interval start (None when resolution failed).
    pub derived_interval_start_epoch_seconds: Option<i64>,
    /// Derived interval end (None when resolution failed).
    pub derived_interval_end_epoch_seconds: Option<i64>,
    /// Derived interval-integrated amount in kg/m2 (None when resolution failed).
    pub derived_amount_kg_per_square_meter: Option<f64>,
    /// Derived SI rate in kg/(m2 s) (None when resolution failed).
    pub derived_rate_kg_per_square_meter_per_second: Option<f64>,
    /// Derived handoff rate in mm/h (None when resolution failed).
    pub derived_rate_millimeter_per_hour: Option<f64>,
    /// Optional oracle/reference rate in mm/h supplied by the caller.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oracle_reference_rate_millimeter_per_hour: Option<f64>,
    /// The derived mm/h rate that is compared against the reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_rate_millimeter_per_hour: Option<f64>,
    /// Whether `candidate` matches `oracle` within `RELATIVE_TOLERANCE`
    /// (None when either side is absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches_oracle_within_relative_tolerance: Option<bool>,
    /// #216 admission of this row; its derived values are `+0.0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantization_admission: Option<PackingDecreaseAdmission>,
    /// Passed, or Failed with the canonical reason.
    pub verdict: Verdict,
}

/// Machine-readable accumulation report recording source observations, interval
/// and reset metadata, derived amount/rate, optional oracle comparison and a
/// verdict per row.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AccumulationReport {
    pub schema: String,
    pub schema_version: u32,
    /// Field identity supplied by the caller (e.g. canonical `FieldId`).
    pub field_identity: String,
    pub source_accumulated_unit: String,
    pub derived_amount_unit: String,
    pub derived_rate_si_unit: String,
    pub derived_rate_handoff_unit: String,
    /// Overall report verdict, including sequence-level and oracle-coverage checks.
    pub verdict: Verdict,
    /// How many interval rows failed derivation or a requested oracle comparison.
    pub failed_rows: usize,
    /// One row per source observation, in observation order.
    pub intervals: Vec<IntervalEvidence>,
}

/// Resolve an ordered accumulated-observation sequence into interval products.
///
/// Fail-closed semantics are documented in the module documentation; every
/// returned interval has a strictly positive duration and a non-negative amount
/// derived without any assumed global reset cadence.
///
/// # Errors
///
/// Returns [`AccumulationError`] on the first violating observation.
pub fn resolve_interval_sequence(
    observations: &[AccumulatedObservation],
) -> Result<IntervalSequence, AccumulationError> {
    resolve_with_policy(observations, DecreasePolicy::Strict)
}

/// [`resolve_interval_sequence`] with the #216 admission of same-run decreases.
///
/// # Errors
///
/// Returns [`AccumulationError`] on the first violating observation, including
/// [`AccumulationError::PackingDecreaseRejected`] for a decrease that carries
/// source records but is not `ADMIT_BOUNDED`.
pub fn resolve_interval_sequence_with_sources(
    series: &AccumulatedSeries<'_>,
) -> Result<IntervalSequence, AccumulationError> {
    check_source_count(series)?;
    resolve_with_policy(series.observations, DecreasePolicy::Provenance(series))
}

fn resolve_with_policy(
    observations: &[AccumulatedObservation],
    policy: DecreasePolicy<'_>,
) -> Result<IntervalSequence, AccumulationError> {
    if observations.is_empty() {
        return Err(AccumulationError::EmptySequence);
    }
    let derived = derive_all(observations, policy);
    let mut intervals = Vec::with_capacity(derived.len());
    let mut detected_reset_count = 0;
    let mut quantization_admissions = Vec::new();
    for result in derived {
        let derived = result?;
        if derived.detected_reset {
            detected_reset_count += 1;
        }
        intervals.push(derived.product);
        quantization_admissions.extend(derived.admission);
    }
    Ok(IntervalSequence {
        intervals,
        detected_reset_count,
        quantization_admissions,
    })
}

fn check_source_count(series: &AccumulatedSeries<'_>) -> Result<(), AccumulationError> {
    if series.sources.len() == series.observations.len() {
        Ok(())
    } else {
        Err(AccumulationError::SourceCountMismatch {
            observations: series.observations.len(),
            sources: series.sources.len(),
        })
    }
}

/// Validate accumulated observations and derive interval/reset metadata only.
///
/// This is the production host boundary for the GPU transformation. It checks
/// every #75 chronology, reset, finiteness, sign, monotonicity, and duration
/// rule, but does not calculate an interval amount or rate.
///
/// # Errors
///
/// Returns [`AccumulationError`] on the first violating observation.
pub fn validate_interval_sequence(
    observations: &[AccumulatedObservation],
) -> Result<Vec<AccumulatedIntervalMetadata>, AccumulationError> {
    if observations.is_empty() {
        return Err(AccumulationError::EmptySequence);
    }
    (0..observations.len())
        .map(|index| {
            validate_interval_metadata(observations, index, DecreasePolicy::Strict)
                .map(|(metadata, _)| metadata)
        })
        .collect()
}

/// [`validate_interval_sequence`] with the #216 admission of same-run decreases.
///
/// This is the production host boundary of #90 for fields that may carry
/// source-quantization records. The returned metadata is identical to the strict
/// path for every admitted sequence; admissions are returned separately.
///
/// # Errors
///
/// Returns [`AccumulationError`] on the first violating observation.
pub fn validate_interval_sequence_with_sources(
    series: &AccumulatedSeries<'_>,
) -> Result<ValidatedIntervalSequence, AccumulationError> {
    check_source_count(series)?;
    if series.observations.is_empty() {
        return Err(AccumulationError::EmptySequence);
    }
    let mut intervals = Vec::with_capacity(series.observations.len());
    let mut quantization_admissions = Vec::new();
    for index in 0..series.observations.len() {
        let (metadata, admission) = validate_interval_metadata(
            series.observations,
            index,
            DecreasePolicy::Provenance(series),
        )?;
        intervals.push(metadata);
        quantization_admissions.extend(admission);
    }
    Ok(ValidatedIntervalSequence {
        intervals,
        quantization_admissions,
    })
}

/// Build a machine-readable report over `observations` without failing: rows
/// carry the derived interval amount/rate or the fail-closed verdict.
///
/// `oracle_reference_rate_mm_per_hour`, when provided, must supply exactly one
/// finite reference value per source observation; each row then records
/// candidate vs reference and the `1e-6`-relative verdict. Missing or extra
/// references fail the overall report rather than weakening the comparison.
/// `field_identity` names the resolved field.
///
/// Rows after a failed observation carry a failed verdict without derived
/// values: their derivation would otherwise consume rejected source input.
#[must_use]
pub fn build_accumulation_report(
    field_identity: &str,
    observations: &[AccumulatedObservation],
    oracle_reference_rate_mm_per_hour: Option<&[f64]>,
) -> AccumulationReport {
    report_with_policy(
        field_identity,
        observations,
        oracle_reference_rate_mm_per_hour,
        DecreasePolicy::Strict,
    )
}

/// [`build_accumulation_report`] with the #216 admission of same-run decreases.
///
/// An admitted row passes with `+0.0` derived values and records its typed
/// admission. A source-count mismatch fails the report closed.
#[must_use]
pub fn build_accumulation_report_with_sources(
    field_identity: &str,
    series: &AccumulatedSeries<'_>,
    oracle_reference_rate_mm_per_hour: Option<&[f64]>,
) -> AccumulationReport {
    if let Err(error) = check_source_count(series) {
        let mut report = report_with_policy(
            field_identity,
            series.observations,
            oracle_reference_rate_mm_per_hour,
            DecreasePolicy::Strict,
        );
        report.verdict = Verdict::Failed(error.to_string());
        return report;
    }
    report_with_policy(
        field_identity,
        series.observations,
        oracle_reference_rate_mm_per_hour,
        DecreasePolicy::Provenance(series),
    )
}

fn report_with_policy(
    field_identity: &str,
    observations: &[AccumulatedObservation],
    oracle_reference_rate_mm_per_hour: Option<&[f64]>,
    policy: DecreasePolicy<'_>,
) -> AccumulationReport {
    let derived = derive_all(observations, policy);
    let first_failure = derived.iter().position(Result::is_err);
    let oracle_comparison_requested = oracle_reference_rate_mm_per_hour.is_some();
    let intervals = observations
        .iter()
        .enumerate()
        .map(|(index, observation)| {
            let outcome = match &derived[index] {
                Ok(product) => match first_failure {
                    Some(first) if index > first => RowOutcome::Blocked { failed: first },
                    _ => RowOutcome::Derived(product),
                },
                Err(error) => RowOutcome::Rejected(error),
            };
            interval_evidence(
                index,
                observation,
                outcome,
                oracle_reference_rate_mm_per_hour.and_then(|refs| refs.get(index)),
                oracle_comparison_requested,
            )
        })
        .collect::<Vec<IntervalEvidence>>();

    let failed_rows = intervals
        .iter()
        .filter(|row| matches!(row.verdict, Verdict::Failed(_)))
        .count();
    let verdict = if observations.is_empty() {
        Verdict::Failed(AccumulationError::EmptySequence.to_string())
    } else if let Some(references) = oracle_reference_rate_mm_per_hour {
        if references.len() != observations.len() {
            Verdict::Failed(format!(
                "oracle reference count {} does not match observation count {}",
                references.len(),
                observations.len()
            ))
        } else if failed_rows > 0 {
            Verdict::Failed(format!("{failed_rows} interval report row(s) failed"))
        } else {
            Verdict::Passed
        }
    } else if failed_rows > 0 {
        Verdict::Failed(format!("{failed_rows} interval report row(s) failed"))
    } else {
        Verdict::Passed
    };
    AccumulationReport {
        schema: ACCUMULATION_CONTRACT_SCHEMA.to_string(),
        schema_version: ACCUMULATION_CONTRACT_VERSION,
        field_identity: field_identity.to_string(),
        source_accumulated_unit: SOURCE_UNIT.to_string(),
        derived_amount_unit: SOURCE_UNIT.to_string(),
        derived_rate_si_unit: RATE_SI_UNIT.to_string(),
        derived_rate_handoff_unit: RATE_HANDOFF_UNIT.to_string(),
        verdict,
        failed_rows,
        intervals,
    }
}

/// How one report row relates to the resolution outcome.
#[derive(Clone, Copy)]
enum RowOutcome<'a> {
    /// The row's own source values derive an interval product.
    Derived(&'a DerivedInterval),
    /// The row's own source values violate the contract.
    Rejected(&'a AccumulationError),
    /// An earlier row failed closed, so this row derives nothing rather than
    /// consuming rejected source input.
    Blocked { failed: usize },
}

fn interval_evidence(
    index: usize,
    observation: &AccumulatedObservation,
    outcome: RowOutcome<'_>,
    oracle_reference: Option<&f64>,
    oracle_comparison_requested: bool,
) -> IntervalEvidence {
    let declared_interval_start_epoch_seconds = observation.reset_epoch_seconds;
    let declared_interval_end_epoch_seconds = observation.valid_time_epoch_seconds;

    let mut quantization_admission = None;
    let (detected_reset, reset_applied, derived, mut verdict) = match outcome {
        RowOutcome::Derived(derived) => {
            quantization_admission.clone_from(&derived.admission);
            (
                derived.detected_reset,
                derived.product.reset_applied,
                Some(derived.product),
                Verdict::Passed,
            )
        }
        RowOutcome::Rejected(error) => (false, false, None, Verdict::Failed(error.to_string())),
        RowOutcome::Blocked { failed } => (
            false,
            false,
            None,
            Verdict::Failed(format!(
                "observation {index}: unresolved because observation {failed} failed closed"
            )),
        ),
    };

    let derived_interval_start_epoch_seconds =
        derived.map(|product| product.interval_start_epoch_seconds);
    let derived_interval_end_epoch_seconds =
        derived.map(|product| product.interval_end_epoch_seconds);
    let derived_amount_kg_per_square_meter =
        derived.map(|product| product.amount_kg_per_square_meter.0);
    let derived_rate_kg_per_square_meter_per_second =
        derived.map(|product| product.rate_kg_per_square_meter_per_second.0);
    let derived_rate_millimeter_per_hour =
        derived.map(|product| product.rate_millimeter_per_hour.0);

    let candidate_rate_millimeter_per_hour = derived_rate_millimeter_per_hour;
    let matches_oracle_within_relative_tolerance =
        match (candidate_rate_millimeter_per_hour, oracle_reference) {
            (Some(candidate), Some(reference)) => Some(matches_relative(candidate, *reference)),
            _ => None,
        };
    if derived.is_some() && oracle_comparison_requested {
        verdict = match (oracle_reference, matches_oracle_within_relative_tolerance) {
            (None, _) => Verdict::Failed(format!(
                "observation {index}: oracle reference rate is missing"
            )),
            (Some(reference), _) if !reference.is_finite() => Verdict::Failed(format!(
                "observation {index}: oracle reference rate is not finite"
            )),
            (Some(reference), Some(false)) => Verdict::Failed(format!(
                "observation {index}: candidate rate does not match oracle reference {reference} within relative tolerance {RELATIVE_TOLERANCE}"
            )),
            (Some(_), Some(true)) => Verdict::Passed,
            (Some(_), None) => Verdict::Failed(format!(
                "observation {index}: candidate rate is unavailable for oracle comparison"
            )),
        };
    }

    IntervalEvidence {
        source_index: index,
        source_valid_time_epoch_seconds: observation.valid_time_epoch_seconds,
        source_reset_epoch_seconds: observation.reset_epoch_seconds,
        source_accumulated_amount_kg_per_square_meter: observation
            .accumulated_amount_kg_per_square_meter,
        declared_interval_start_epoch_seconds,
        declared_interval_end_epoch_seconds,
        detected_reset,
        reset_applied,
        derived_interval_start_epoch_seconds,
        derived_interval_end_epoch_seconds,
        derived_amount_kg_per_square_meter,
        derived_rate_kg_per_square_meter_per_second,
        derived_rate_millimeter_per_hour,
        oracle_reference_rate_millimeter_per_hour: oracle_reference.copied(),
        candidate_rate_millimeter_per_hour,
        matches_oracle_within_relative_tolerance,
        quantization_admission,
        verdict,
    }
}

/// One derived row: product, declared-reset flag and optional #216 admission.
#[derive(Debug, Clone, PartialEq)]
struct DerivedInterval {
    product: IntervalProduct,
    detected_reset: bool,
    admission: Option<PackingDecreaseAdmission>,
}

fn derive_all(
    observations: &[AccumulatedObservation],
    policy: DecreasePolicy<'_>,
) -> Vec<Result<DerivedInterval, AccumulationError>> {
    (0..observations.len())
        .map(|index| derive_interval(observations, index, policy))
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn derive_interval(
    observations: &[AccumulatedObservation],
    index: usize,
    policy: DecreasePolicy<'_>,
) -> Result<DerivedInterval, AccumulationError> {
    let (metadata, admission) = validate_interval_metadata(observations, index, policy)?;
    let observation = observations[index];
    let interval_amount = if admission.is_some() {
        // #216 projection: the admitted decrease is +0.0, never a signed amount.
        0.0
    } else if metadata.reset_applied {
        observation.accumulated_amount_kg_per_square_meter
    } else {
        observation.accumulated_amount_kg_per_square_meter
            - observations[index - 1].accumulated_amount_kg_per_square_meter
    };
    let duration = metadata.duration_seconds as f64;
    let rate_si = interval_amount / duration;
    let rate_millimeter_per_hour = rate_si * SECONDS_PER_HOUR;
    Ok(DerivedInterval {
        product: IntervalProduct {
            interval_start_epoch_seconds: metadata.interval_start_epoch_seconds,
            interval_end_epoch_seconds: metadata.interval_end_epoch_seconds,
            amount_kg_per_square_meter: AmountKgPerSquareMeter(interval_amount),
            rate_kg_per_square_meter_per_second: RateKgPerSquareMeterPerSecond(rate_si),
            rate_millimeter_per_hour: RateMillimeterPerHour(rate_millimeter_per_hour),
            reset_applied: metadata.reset_applied,
        },
        detected_reset: metadata.detected_reset,
        admission,
    })
}

/// The canonical `f32` a value was uploaded as, when it is exactly one.
#[allow(clippy::cast_possible_truncation, clippy::float_cmp)] // exact identity check
fn exact_f32(value: f64) -> Option<f32> {
    let narrowed = value as f32;
    (f64::from(narrowed) == value).then_some(narrowed)
}

/// Classify a same-run decrease of `observations[index]` (#216).
fn admit_decrease(
    observations: &[AccumulatedObservation],
    index: usize,
    policy: DecreasePolicy<'_>,
) -> Result<PackingDecreaseAdmission, AccumulationError> {
    let (previous, current) = (observations[index - 1], observations[index]);
    let negative_delta = AccumulationError::NegativeDelta {
        index,
        previous: previous.accumulated_amount_kg_per_square_meter,
        current: current.accumulated_amount_kg_per_square_meter,
    };
    let DecreasePolicy::Provenance(series) = policy else {
        return Err(negative_delta);
    };
    let (previous_source, current_source) = (series.sources[index - 1], series.sources[index]);
    if previous_source.is_none() && current_source.is_none() {
        return Err(negative_delta);
    }
    let rejected = |verdict, code| AccumulationError::PackingDecreaseRejected {
        index,
        previous: previous.accumulated_amount_kg_per_square_meter,
        current: current.accumulated_amount_kg_per_square_meter,
        verdict,
        code,
    };
    // #216 classifies the exact uploaded f32 values; any other value did not take
    // the declared f32-nearest conversion.
    let (Some(c_prev), Some(c_cur)) = (
        exact_f32(previous.accumulated_amount_kg_per_square_meter),
        exact_f32(current.accumulated_amount_kg_per_square_meter),
    ) else {
        return Err(rejected(
            PackingDecreaseVerdict::Unresolved,
            PackingDecreaseCode::UnsupportedConversion,
        ));
    };
    let observation = |value, observation: AccumulatedObservation, source| {
        PackingObservation::canonical(
            value,
            observation.valid_time_epoch_seconds,
            observation.reset_epoch_seconds,
            series.grid_id,
            source,
        )
    };
    let classification = classify_packing_decrease(&PackingDecreasePair {
        field_id: series.field_id,
        canonical_unit: series.canonical_unit,
        previous: observation(c_prev, previous, previous_source),
        current: observation(c_cur, current, current_source),
    });
    let (Some(bound), Some(previous_source), Some(current_source)) =
        (classification.admitted, previous_source, current_source)
    else {
        return Err(rejected(classification.verdict, classification.code));
    };
    Ok(PackingDecreaseAdmission {
        decision: GRIB_PACKING_DECISION_SCHEMA.to_string(),
        verdict: classification.verdict,
        code: classification.code,
        field_id: series.field_id,
        interval_index: index,
        cell_index: series.cell_index,
        interval_start_epoch_seconds: previous.valid_time_epoch_seconds,
        interval_end_epoch_seconds: current.valid_time_epoch_seconds,
        previous_canonical_kg_per_square_meter: c_prev,
        current_canonical_kg_per_square_meter: c_cur,
        signed_difference_kg_per_square_meter: bound.signed_difference_kg_per_square_meter,
        signed_difference_exact: bound.signed_difference_exact,
        bound_kg_per_square_meter: bound.bound_kg_per_square_meter,
        bound_exact: bound.bound_exact,
        previous_source: previous_source.clone(),
        current_source: current_source.clone(),
    })
}

fn validate_interval_metadata(
    observations: &[AccumulatedObservation],
    index: usize,
    policy: DecreasePolicy<'_>,
) -> Result<
    (
        AccumulatedIntervalMetadata,
        Option<PackingDecreaseAdmission>,
    ),
    AccumulationError,
> {
    let observation = observations[index];
    let amount = observation.accumulated_amount_kg_per_square_meter;
    if !amount.is_finite() {
        return Err(AccumulationError::NonFiniteAmount { index });
    }
    if amount < 0.0 {
        return Err(AccumulationError::NegativeAmount { index, amount });
    }
    if observation.reset_epoch_seconds >= observation.valid_time_epoch_seconds {
        return Err(AccumulationError::ResetNotBeforeValidTime {
            index,
            reset: observation.reset_epoch_seconds,
            valid_time: observation.valid_time_epoch_seconds,
        });
    }

    let mut admission = None;
    let (interval_start, reset_applied, detected_reset) = if index == 0 {
        (observation.reset_epoch_seconds, true, false)
    } else {
        let previous = observations[index - 1];
        if observation.valid_time_epoch_seconds <= previous.valid_time_epoch_seconds {
            return Err(AccumulationError::NonIncreasingTimestamps {
                index,
                previous: previous.valid_time_epoch_seconds,
                current: observation.valid_time_epoch_seconds,
            });
        }
        if observation.reset_epoch_seconds < previous.reset_epoch_seconds {
            return Err(AccumulationError::BackwardsReset {
                index,
                previous: previous.reset_epoch_seconds,
                current: observation.reset_epoch_seconds,
            });
        }
        if observation.reset_epoch_seconds == previous.reset_epoch_seconds {
            if amount < previous.accumulated_amount_kg_per_square_meter {
                admission = Some(admit_decrease(observations, index, policy)?);
            }
            (previous.valid_time_epoch_seconds, false, false)
        } else {
            let reset = observation.reset_epoch_seconds;
            if reset < previous.valid_time_epoch_seconds {
                return Err(AccumulationError::MidWindowReset {
                    index,
                    reset,
                    previous_valid_time: previous.valid_time_epoch_seconds,
                });
            }
            if reset > previous.valid_time_epoch_seconds {
                return Err(AccumulationError::UncoveredGap {
                    index,
                    previous_valid_time: previous.valid_time_epoch_seconds,
                    reset,
                });
            }
            (reset, true, true)
        }
    };
    let interval_end = observation.valid_time_epoch_seconds;
    let duration_seconds = interval_end.checked_sub(interval_start).ok_or(
        AccumulationError::IntervalDurationOutOfRange {
            index,
            start: interval_start,
            end: interval_end,
        },
    )?;
    if duration_seconds <= 0 {
        return Err(AccumulationError::IntervalDurationOutOfRange {
            index,
            start: interval_start,
            end: interval_end,
        });
    }
    Ok((
        AccumulatedIntervalMetadata {
            interval_start_epoch_seconds: interval_start,
            interval_end_epoch_seconds: interval_end,
            duration_seconds,
            reset_applied,
            detected_reset,
        },
        admission,
    ))
}

/// `1e-6`-relative closeness used for interval-preservation verdicts in the
/// machine-readable report.
#[must_use]
pub fn matches_relative(candidate: f64, reference: f64) -> bool {
    let diff = (candidate - reference).abs();
    diff <= RELATIVE_TOLERANCE * reference.abs()
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::{
        build_accumulation_report, matches_relative, resolve_interval_sequence,
        AccumulatedObservation, AccumulationError, IntervalSequence, Verdict,
    };

    fn obs(t: i64, r: i64, amount: f64) -> AccumulatedObservation {
        AccumulatedObservation {
            valid_time_epoch_seconds: t,
            reset_epoch_seconds: r,
            accumulated_amount_kg_per_square_meter: amount,
        }
    }

    fn single_run() -> Vec<AccumulatedObservation> {
        vec![
            obs(1_800, 0, 1.0),
            obs(3_600, 0, 3.0),
            obs(5_400, 0, 6.0),
            obs(7_200, 0, 6.0),
        ]
    }

    #[test]
    fn monotonic_run_without_reset_derives_interval_amounts_and_rates() {
        let resolved = resolve_interval_sequence(&single_run())
            .unwrap_or_else(|error| panic!("single-run sequence must resolve: {error}"));

        let products = [
            (0, 1_800, 1.0, 2.0, true, false),
            (1_800, 3_600, 2.0, 4.0, false, false),
            (3_600, 5_400, 3.0, 6.0, false, false),
            (5_400, 7_200, 0.0, 0.0, false, false),
        ];
        let mut intervals = resolved.intervals.iter();
        for (index, (start, end, amount, mmh, reset_applied, detected)) in
            products.iter().enumerate()
        {
            let product = intervals
                .next()
                .unwrap_or_else(|| panic!("interval {index} must exist"));
            assert_eq!(product.interval_start_epoch_seconds, *start);
            assert_eq!(product.interval_end_epoch_seconds, *end);
            assert_relative_eq!(
                product.amount_kg_per_square_meter.0,
                *amount,
                epsilon = 1.0e-12
            );
            assert_relative_eq!(product.rate_millimeter_per_hour.0, *mmh, epsilon = 1.0e-12);
            assert_relative_eq!(
                product.rate_kg_per_square_meter_per_second.0,
                amount / 1_800.0,
                epsilon = 1.0e-15
            );
            assert_eq!(product.reset_applied, *reset_applied);
            assert!(!*detected);
        }
        assert_eq!(resolved.detected_reset_count, 0);
    }

    #[test]
    fn resolved_sequence_total_amount_telescopes_to_final_source_value() {
        let resolved: IntervalSequence =
            resolve_interval_sequence(&single_run()).expect("single-run sequence resolves");
        assert_relative_eq!(
            resolved.total_amount_kg_per_square_meter(),
            6.0,
            epsilon = 1.0e-12
        );
        assert_eq!(resolved.intervals.len(), 4);
    }

    #[test]
    fn rate_si_to_millimeter_per_hour_handoff_is_exact() {
        // 1 kg/m2 accumulated over 3600 s is exactly 1 mm/h.
        let resolved = resolve_interval_sequence(&[obs(3_600, 0, 1.0)])
            .expect("single leading interval resolves");
        let product = &resolved.intervals[0];
        assert_relative_eq!(product.rate_millimeter_per_hour.0, 1.0, epsilon = 1.0e-12);
        assert_relative_eq!(
            product.rate_kg_per_square_meter_per_second.0,
            1.0 / 3_600.0,
            epsilon = 1.0e-15
        );
    }

    #[test]
    fn leading_interval_uses_declared_reset_start() {
        // reset at 600 s declares the leading window [600, 1800]; 2.0 kg/m2 over
        // 1200 s is 6 mm/h.
        let resolved = resolve_interval_sequence(&[obs(1_800, 600, 2.0), obs(3_600, 600, 6.0)])
            .expect("sequence resolves");
        let leading = &resolved.intervals[0];
        assert_eq!(leading.interval_start_epoch_seconds, 600);
        assert_eq!(leading.interval_end_epoch_seconds, 1_800);
        assert_relative_eq!(leading.amount_kg_per_square_meter.0, 2.0, epsilon = 1.0e-12);
        assert_relative_eq!(leading.rate_millimeter_per_hour.0, 6.0, epsilon = 1.0e-12);
        assert!(leading.reset_applied);
    }

    #[test]
    fn contiguous_declared_reset_applies_fresh_run_amount() {
        // obs 1 declares a new run starting exactly at the previous valid time.
        let resolved = resolve_interval_sequence(&[
            obs(1_800, 0, 1.0),
            obs(3_600, 1_800, 4.0),
            obs(5_400, 1_800, 7.0),
        ])
        .expect("contiguous reset sequence resolves");

        let intervals = [
            (0, 1_800, 1.0, 2.0, true),
            (1_800, 3_600, 4.0, 8.0, true),
            (3_600, 5_400, 3.0, 6.0, false),
        ];
        for (product, expected) in resolved.intervals.iter().zip(intervals.iter()) {
            let (start, end, amount, mmh, reset_applied) = *expected;
            assert_eq!(product.interval_start_epoch_seconds, start);
            assert_eq!(product.interval_end_epoch_seconds, end);
            assert_relative_eq!(
                product.amount_kg_per_square_meter.0,
                amount,
                epsilon = 1.0e-12
            );
            assert_relative_eq!(product.rate_millimeter_per_hour.0, mmh, epsilon = 1.0e-12);
            assert_eq!(product.reset_applied, reset_applied);
        }
        assert_eq!(resolved.detected_reset_count, 1);
        assert_relative_eq!(
            resolved.total_amount_kg_per_square_meter(),
            8.0,
            epsilon = 1.0e-12
        );
    }

    #[test]
    fn same_run_decrease_fails_closed_and_is_not_treated_as_reset() {
        let error = resolve_interval_sequence(&[obs(1_800, 0, 1.0), obs(3_600, 0, 0.5)])
            .expect_err("decreasing same-run amount must fail closed");
        assert_eq!(
            error,
            AccumulationError::NegativeDelta {
                index: 1,
                previous: 1.0,
                current: 0.5,
            }
        );
    }

    #[test]
    fn backwards_reset_fails_closed() {
        let error = resolve_interval_sequence(&[obs(1_800, 0, 1.0), obs(3_600, -100, 2.0)])
            .expect_err("backwards reset must fail closed");
        assert_eq!(
            error,
            AccumulationError::BackwardsReset {
                index: 1,
                previous: 0,
                current: -100,
            }
        );
    }

    #[test]
    fn mid_window_reset_fails_closed() {
        // Reset at 1200 s falls inside the previously declared [0, 1800] window.
        let error = resolve_interval_sequence(&[obs(1_800, 0, 1.0), obs(3_600, 1_200, 2.0)])
            .expect_err("mid-window reset must fail closed");
        assert_eq!(
            error,
            AccumulationError::MidWindowReset {
                index: 1,
                reset: 1_200,
                previous_valid_time: 1_800,
            }
        );
    }

    #[test]
    fn uncovered_gap_after_forward_reset_fails_closed() {
        // The 3600 s reset leaves [1800, 3600] with no source coverage; the
        // resolver must not fabricate the missing interval.
        let error = resolve_interval_sequence(&[obs(1_800, 0, 1.0), obs(5_400, 3_600, 2.0)])
            .expect_err("uncovered gap must fail closed");
        assert_eq!(
            error,
            AccumulationError::UncoveredGap {
                index: 1,
                previous_valid_time: 1_800,
                reset: 3_600,
            }
        );
    }

    #[test]
    fn reset_not_before_valid_time_fails_closed() {
        let error = resolve_interval_sequence(&[obs(1_800, 0, 1.0), obs(3_600, 3_600, 5.0)])
            .expect_err("zero-length reset window must fail closed");
        assert_eq!(
            error,
            AccumulationError::ResetNotBeforeValidTime {
                index: 1,
                reset: 3_600,
                valid_time: 3_600,
            }
        );
    }

    #[test]
    fn equal_valid_times_fail_closed() {
        let error = resolve_interval_sequence(&[obs(1_800, 0, 1.0), obs(1_800, 0, 2.0)])
            .expect_err("equal valid times must fail closed");
        assert_eq!(
            error,
            AccumulationError::NonIncreasingTimestamps {
                index: 1,
                previous: 1_800,
                current: 1_800,
            }
        );
    }

    #[test]
    fn empty_sequence_fails_closed() {
        let error = resolve_interval_sequence(&[]).expect_err("empty sequence must fail");
        assert_eq!(error, AccumulationError::EmptySequence);
    }

    #[test]
    fn negative_source_amount_fails_closed() {
        let error = resolve_interval_sequence(&[obs(1_800, 0, -1.0)])
            .expect_err("negative amount must fail closed");
        assert_eq!(
            error,
            AccumulationError::NegativeAmount {
                index: 0,
                amount: -1.0
            }
        );
    }

    #[test]
    fn non_finite_source_amount_fails_closed() {
        let error = resolve_interval_sequence(&[obs(1_800, 0, f64::NAN)])
            .expect_err("non-finite amount must fail closed");
        assert_eq!(error, AccumulationError::NonFiniteAmount { index: 0 });
    }

    #[test]
    fn report_records_source_metadata_derived_values_and_reset_flags() {
        let report = build_accumulation_report(
            "large_scale_precipitation",
            &[
                obs(1_800, 0, 1.0),
                obs(3_600, 1_800, 4.0),
                obs(5_400, 1_800, 7.0),
            ],
            None,
        );
        assert_eq!(report.field_identity, "large_scale_precipitation");
        assert_eq!(report.verdict, Verdict::Passed);
        assert_eq!(report.failed_rows, 0);
        assert_eq!(report.source_accumulated_unit, "kilogram_per_square_meter");
        assert_eq!(report.derived_rate_handoff_unit, "millimeter_per_hour");

        let row = &report.intervals[0];
        assert_eq!(row.source_index, 0);
        assert_eq!(row.source_valid_time_epoch_seconds, 1_800);
        assert_eq!(row.source_reset_epoch_seconds, 0);
        assert_relative_eq!(
            row.source_accumulated_amount_kg_per_square_meter,
            1.0,
            epsilon = 1.0e-12
        );
        assert!(!row.detected_reset);
        assert!(row.reset_applied);
        assert_eq!(row.derived_interval_start_epoch_seconds, Some(0));
        assert_eq!(row.derived_interval_end_epoch_seconds, Some(1_800));
        assert_eq!(row.derived_amount_kg_per_square_meter, Some(1.0));
        assert_eq!(row.verdict, Verdict::Passed);

        let reset_row = &report.intervals[1];
        assert!(reset_row.detected_reset);
        assert!(reset_row.reset_applied);
        assert_eq!(reset_row.derived_interval_start_epoch_seconds, Some(1_800));
        assert_eq!(reset_row.derived_amount_kg_per_square_meter, Some(4.0));
        assert_eq!(reset_row.verdict, Verdict::Passed);

        let json = serde_json::to_string(&report).expect("report serializes");
        assert!(json.contains("\"state\":\"passed\""));
        assert!(json.contains("\"field_identity\":\"large_scale_precipitation\""));
    }

    #[test]
    fn report_records_failed_rows_with_reason() {
        let report = build_accumulation_report(
            "large_scale_precipitation",
            &[obs(1_800, 0, 1.0), obs(3_600, 0, 0.5)],
            None,
        );
        assert!(matches!(report.verdict, Verdict::Failed(_)));
        assert_eq!(report.failed_rows, 1);
        let row = &report.intervals[1];
        assert_eq!(row.verdict, Verdict::Failed("observation 1: same-run accumulated amount decreased from 1 to 0.5 without a declared reset".to_string()));
        assert_eq!(row.derived_amount_kg_per_square_meter, None);
        assert_eq!(row.derived_rate_millimeter_per_hour, None);
        assert!(!row.reset_applied);

        let json = serde_json::to_string(&report).expect("report serializes");
        assert!(json.contains("\"state\":\"failed\""));
        assert!(json.contains("same-run accumulated amount decreased"));
    }

    #[test]
    fn report_blocks_tail_after_failed_observation() {
        // Observation 2 is locally consistent with observation 1's raw values
        // (5.0 >= 0.5 in the same run), but observation 1 was rejected, so the
        // tail row must not derive a rate from rejected source input.
        let report = build_accumulation_report(
            "large_scale_precipitation",
            &[obs(1_800, 0, 1.0), obs(3_600, 0, 0.5), obs(5_400, 0, 5.0)],
            None,
        );
        assert!(matches!(report.verdict, Verdict::Failed(_)));
        assert_eq!(report.failed_rows, 2);
        assert_eq!(report.intervals[0].verdict, Verdict::Passed);
        assert!(matches!(report.intervals[1].verdict, Verdict::Failed(_)));

        let tail = &report.intervals[2];
        assert_eq!(
            tail.verdict,
            Verdict::Failed(
                "observation 2: unresolved because observation 1 failed closed".to_string()
            )
        );
        assert_eq!(tail.derived_interval_start_epoch_seconds, None);
        assert_eq!(tail.derived_amount_kg_per_square_meter, None);
        assert_eq!(tail.derived_rate_millimeter_per_hour, None);
        assert!(!tail.detected_reset);
        assert!(!tail.reset_applied);
        // Source provenance is still recorded even though nothing is derived.
        assert_eq!(tail.source_valid_time_epoch_seconds, 5_400);
        assert_relative_eq!(
            tail.source_accumulated_amount_kg_per_square_meter,
            5.0,
            epsilon = 1.0e-12
        );
    }

    #[test]
    fn report_compares_candidate_rate_against_oracle_reference() {
        let report = build_accumulation_report(
            "large_scale_precipitation",
            &[
                obs(1_800, 0, 1.0),
                obs(3_600, 1_800, 4.0),
                obs(5_400, 1_800, 7.0),
            ],
            Some(&[2.0, 8.0, 6.0]),
        );
        assert_eq!(report.verdict, Verdict::Passed);
        for row in &report.intervals {
            assert_eq!(row.verdict, Verdict::Passed);
            assert_eq!(row.matches_oracle_within_relative_tolerance, Some(true));
            assert_eq!(
                row.oracle_reference_rate_millimeter_per_hour,
                row.candidate_rate_millimeter_per_hour
            );
        }

        let mismatched = build_accumulation_report(
            "large_scale_precipitation",
            &[
                obs(1_800, 0, 1.0),
                obs(3_600, 1_800, 4.0),
                obs(5_400, 1_800, 7.0),
            ],
            Some(&[2.0, 8.0, 7.0]),
        );
        assert_eq!(
            mismatched.intervals[2].matches_oracle_within_relative_tolerance,
            Some(false)
        );
        assert!(matches!(
            mismatched.intervals[2].verdict,
            Verdict::Failed(_)
        ));
        assert_eq!(mismatched.failed_rows, 1);
        assert!(matches!(mismatched.verdict, Verdict::Failed(_)));
        let json = serde_json::to_string(&mismatched).expect("mismatch report serializes");
        assert!(json.contains("does not match oracle reference"));
    }

    #[test]
    fn empty_report_fails_closed_at_report_level() {
        let report = build_accumulation_report("large_scale_precipitation", &[], None);

        assert_eq!(
            report.verdict,
            Verdict::Failed("accumulation sequence is empty".to_string())
        );
        assert_eq!(report.failed_rows, 0);
        assert!(report.intervals.is_empty());
        let json = serde_json::to_string(&report).expect("empty failure report serializes");
        assert!(json.contains("accumulation sequence is empty"));
    }

    #[test]
    fn requested_oracle_comparison_requires_complete_finite_references() {
        let observations = [obs(1_800, 0, 1.0), obs(3_600, 0, 3.0)];

        let missing =
            build_accumulation_report("large_scale_precipitation", &observations, Some(&[2.0]));
        assert!(matches!(missing.verdict, Verdict::Failed(_)));
        assert_eq!(missing.failed_rows, 1);
        assert!(matches!(missing.intervals[1].verdict, Verdict::Failed(_)));

        let extra = build_accumulation_report(
            "large_scale_precipitation",
            &observations,
            Some(&[2.0, 4.0, 6.0]),
        );
        assert_eq!(
            extra.verdict,
            Verdict::Failed(
                "oracle reference count 3 does not match observation count 2".to_string()
            )
        );
        assert_eq!(extra.failed_rows, 0);

        let non_finite = build_accumulation_report(
            "large_scale_precipitation",
            &observations,
            Some(&[2.0, f64::NAN]),
        );
        assert!(matches!(non_finite.verdict, Verdict::Failed(_)));
        assert_eq!(non_finite.failed_rows, 1);
        assert!(matches!(
            non_finite.intervals[1].verdict,
            Verdict::Failed(_)
        ));
    }

    #[test]
    fn relative_closeness_uses_issue_75_tolerance() {
        assert!(matches_relative(1.000_000_000_4, 1.0));
        assert!(!matches_relative(1.001, 1.0));
        // The tolerance is explicitly relative: a large reference keeps a
        // proportional band, while zero references require an exact match.
        assert!(matches_relative(100_000.05, 100_000.0));
        assert!(!matches_relative(100_000.5, 100_000.0));
        assert!(matches_relative(0.0, 0.0));
        assert!(!matches_relative(1.0e-12, 0.0));
    }
}
