//! Machine-readable evidence for calculative GPU execution and oracle comparison.
//!
//! This module owns only repository-wide provenance and comparison mechanics.
//! Scientific issues must supply their pinned oracle identity, input artifacts,
//! metric interpretation, and tolerance values.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::GpuContext;

/// Stable identifier for the repository-wide GPU execution evidence schema.
pub const GPU_EVIDENCE_SCHEMA_ID: &str = "flexpart-gpu.gpu-execution-evidence";

/// Current version of the repository-wide GPU execution evidence schema.
pub const GPU_EVIDENCE_SCHEMA_VERSION: u32 = 1;

/// Schema identity embedded in every GPU execution evidence document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuEvidenceSchema {
    /// Stable schema name used by evidence consumers.
    pub id: String,
    /// Positive schema version used for compatibility checks.
    pub version: u32,
}

impl Default for GpuEvidenceSchema {
    fn default() -> Self {
        Self {
            id: GPU_EVIDENCE_SCHEMA_ID.to_string(),
            version: GPU_EVIDENCE_SCHEMA_VERSION,
        }
    }
}

/// Whether the requested GPU calculation executed, failed, or was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuExecutionStatus {
    /// The configured calculation path completed.
    Passed,
    /// Initialization, resource preparation, dispatch, synchronization, or readback failed.
    Failed,
    /// The calculation did not run and must not be treated as validation evidence.
    Skipped,
}

/// Concrete calculation path represented by an evidence document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuCalculationPath {
    /// The calculation executed as WGSL through the selected `wgpu` device.
    WgslDevice,
    /// A separate CPU implementation was used instead of the declared GPU path.
    CpuReplacement,
    /// No calculation path executed.
    NotExecuted,
}

/// Adapter class needed to distinguish hardware GPU and software WGSL runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuAdapterClass {
    /// A discrete, integrated, virtual, or otherwise non-CPU adapter.
    HardwareGpu,
    /// A CPU-backed `wgpu` adapter that still executes the real WGSL path.
    SoftwareWgsl,
}

/// Stable, serializable provenance for the selected `wgpu` adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuAdapterEvidence {
    /// Adapter name reported by `wgpu`.
    pub name: String,
    /// Stable lowercase backend name.
    pub backend: String,
    /// Stable lowercase `wgpu::DeviceType` name.
    pub device_type: String,
    /// PCI or platform vendor identifier reported by `wgpu`.
    pub vendor_id: u32,
    /// PCI or platform device identifier reported by `wgpu`.
    pub device_id: u32,
    /// Driver name reported by `wgpu`.
    pub driver: String,
    /// Driver detail string reported by `wgpu`.
    pub driver_info: String,
    /// Hardware-GPU versus software-WGSL classification.
    pub adapter_class: GpuAdapterClass,
    /// Whether the caller explicitly requested a software fallback adapter.
    pub software_fallback_requested: bool,
}

impl GpuAdapterEvidence {
    /// Capture adapter provenance from the repository's central GPU context.
    #[must_use]
    pub fn from_context(context: &GpuContext) -> Self {
        let info = context.adapter_info();
        Self {
            name: info.name.clone(),
            backend: backend_name(info.backend).to_string(),
            device_type: device_type_name(info.device_type).to_string(),
            vendor_id: info.vendor,
            device_id: info.device,
            driver: info.driver.clone(),
            driver_info: info.driver_info.clone(),
            adapter_class: if context.is_software_adapter() {
                GpuAdapterClass::SoftwareWgsl
            } else {
                GpuAdapterClass::HardwareGpu
            },
            software_fallback_requested: context.fallback_requested(),
        }
    }

    /// Validate backend, device-type, and hardware/software classification.
    ///
    /// # Errors
    ///
    /// Returns [`GpuEvidenceError`] when required adapter identity is missing
    /// or its class contradicts the reported `wgpu` device type.
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        require_non_empty("adapter.name", &self.name)?;
        validate_adapter(self)
    }
}

/// Content-addressed identity of the GPU candidate calculation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuCandidateEvidence {
    /// Issue-owned calculation or case identifier.
    pub implementation_id: String,
    /// Candidate source revision.
    pub revision: String,
    /// SHA-256 of the WGSL source or generated shader bundle used by the run.
    pub shader_sha256: String,
    /// SHA-256 of the normalized candidate input artifact or manifest.
    pub input_sha256: String,
}

/// Identity of the authoritative pinned FLEXPART oracle for a scientific comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedOracleEvidence {
    /// Oracle implementation identifier, normally `FLEXPART-11.1` plus the owning routine.
    pub implementation_id: String,
    /// Pinned oracle source revision.
    pub revision: String,
    /// SHA-256 of the oracle executable used for the run.
    pub executable_sha256: String,
    /// SHA-256 of the raw or normalized oracle output artifact.
    pub output_sha256: String,
}

/// Repository-wide absolute/relative elementwise comparison policy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ComparisonPolicy {
    /// Absolute tolerance in the units owned by the scientific issue.
    pub absolute_tolerance: f64,
    /// Dimensionless relative tolerance owned by the scientific issue.
    pub relative_tolerance: f64,
}

impl ComparisonPolicy {
    /// Create a finite, non-negative tolerance policy.
    ///
    /// # Errors
    ///
    /// Returns [`GpuEvidenceError::InvalidTolerance`] for a negative or non-finite value.
    pub fn new(absolute_tolerance: f64, relative_tolerance: f64) -> Result<Self, GpuEvidenceError> {
        if !absolute_tolerance.is_finite() || absolute_tolerance < 0.0 {
            return Err(GpuEvidenceError::InvalidTolerance {
                name: "absolute_tolerance",
                value: absolute_tolerance,
            });
        }
        if !relative_tolerance.is_finite() || relative_tolerance < 0.0 {
            return Err(GpuEvidenceError::InvalidTolerance {
                name: "relative_tolerance",
                value: relative_tolerance,
            });
        }
        Ok(Self {
            absolute_tolerance,
            relative_tolerance,
        })
    }
}

/// Numerical verdict for the issue-owned oracle comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericalVerdict {
    /// All compared finite values satisfied the declared policy.
    Passed,
    /// The comparison failed closed.
    Failed,
    /// No scientific comparison was performed.
    NotEvaluated,
}

/// Machine-readable reason for a failed numerical comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonFailureKind {
    /// No values were supplied, so no numerical claim can be made.
    EmptyInput,
    /// Candidate and oracle arrays have different lengths.
    LengthMismatch,
    /// At least one compared input is NaN or infinite.
    NonFiniteValue,
    /// Finite inputs produced a non-finite error during comparison arithmetic.
    NonFiniteError,
    /// A finite value exceeded both declared tolerance bounds.
    ToleranceExceeded,
}

/// First fail-closed condition observed by the generic comparator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonFailure {
    /// Class of comparison failure.
    pub kind: ComparisonFailureKind,
    /// Element index when the failure is element-specific.
    pub index: Option<usize>,
    /// Finite oracle value when one can be represented in JSON.
    pub oracle_value: Option<f64>,
    /// Finite candidate value when one can be represented in JSON.
    pub candidate_value: Option<f64>,
}

/// Machine-readable summary of a generic finite elementwise comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparisonEvidence {
    /// Numerical pass/fail/not-evaluated verdict.
    pub verdict: NumericalVerdict,
    /// Declared absolute/relative tolerance policy, absent only when not evaluated.
    pub policy: Option<ComparisonPolicy>,
    /// Number of values supplied by the oracle.
    pub oracle_value_count: usize,
    /// Number of values supplied by the candidate.
    pub candidate_value_count: usize,
    /// Number of finite values compared before completion or failure.
    pub compared_value_count: usize,
    /// Largest finite absolute error observed.
    pub max_absolute_error: Option<f64>,
    /// Largest finite symmetric relative error observed.
    pub max_relative_error: Option<f64>,
    /// First fail-closed condition, if any.
    pub first_failure: Option<ComparisonFailure>,
}

impl ComparisonEvidence {
    /// Construct an explicit not-evaluated comparison record.
    #[must_use]
    pub const fn not_evaluated() -> Self {
        Self {
            verdict: NumericalVerdict::NotEvaluated,
            policy: None,
            oracle_value_count: 0,
            candidate_value_count: 0,
            compared_value_count: 0,
            max_absolute_error: None,
            max_relative_error: None,
            first_failure: None,
        }
    }
}

/// Execution details that prove which calculative path did or did not run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuExecutionEvidence {
    /// Execution outcome independent of the numerical verdict.
    pub status: GpuExecutionStatus,
    /// WGSL, prohibited CPU replacement, or no-execution path.
    pub calculation_path: GpuCalculationPath,
    /// Adapter provenance, required for successful WGSL execution.
    pub adapter: Option<GpuAdapterEvidence>,
    /// Explicit failure detail for a failed execution.
    pub failure: Option<String>,
    /// Explicit reason for skipped execution.
    pub skip_reason: Option<String>,
}

/// Complete repository-wide evidence record for one calculative GPU case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuCalculationEvidence {
    /// Versioned schema identity.
    pub schema: GpuEvidenceSchema,
    /// Stable case identifier owned by the implementing issue.
    pub case_id: String,
    /// Content-addressed candidate identity.
    pub candidate: GpuCandidateEvidence,
    /// Path and adapter execution evidence.
    pub execution: GpuExecutionEvidence,
    /// Pinned FLEXPART oracle identity, if scientific comparison was requested.
    pub oracle: Option<PinnedOracleEvidence>,
    /// Numerical comparison evidence.
    pub comparison: ComparisonEvidence,
}

impl GpuCalculationEvidence {
    /// Validate schema, provenance, execution, and comparison invariants.
    ///
    /// # Errors
    ///
    /// Returns [`GpuEvidenceError`] when the record could falsely represent a
    /// skipped, CPU-replaced, incomplete, or unpinned run as GPU validation.
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        if self.schema.id != GPU_EVIDENCE_SCHEMA_ID
            || self.schema.version != GPU_EVIDENCE_SCHEMA_VERSION
        {
            return Err(GpuEvidenceError::UnsupportedSchema {
                id: self.schema.id.clone(),
                version: self.schema.version,
            });
        }
        require_non_empty("case_id", &self.case_id)?;
        require_non_empty(
            "candidate.implementation_id",
            &self.candidate.implementation_id,
        )?;
        require_non_empty("candidate.revision", &self.candidate.revision)?;
        require_sha256("candidate.shader_sha256", &self.candidate.shader_sha256)?;
        require_sha256("candidate.input_sha256", &self.candidate.input_sha256)?;

        match self.execution.status {
            GpuExecutionStatus::Passed => {
                if self.execution.calculation_path != GpuCalculationPath::WgslDevice
                    || self.execution.adapter.is_none()
                    || self.execution.failure.is_some()
                    || self.execution.skip_reason.is_some()
                {
                    return Err(GpuEvidenceError::InvalidExecutionState(
                        "passed execution requires a WGSL device path, adapter provenance, and no failure/skip reason",
                    ));
                }
                let adapter = self.execution.adapter.as_ref().ok_or(
                    GpuEvidenceError::InvalidExecutionState(
                        "passed execution requires adapter provenance",
                    ),
                )?;
                adapter.validate()?;
            }
            GpuExecutionStatus::Failed => {
                if self
                    .execution
                    .failure
                    .as_deref()
                    .is_none_or(|failure| failure.trim().is_empty())
                    || self.execution.skip_reason.is_some()
                {
                    return Err(GpuEvidenceError::InvalidExecutionState(
                        "failed execution requires a non-empty failure and no skip reason",
                    ));
                }
            }
            GpuExecutionStatus::Skipped => {
                if self.execution.calculation_path != GpuCalculationPath::NotExecuted
                    || self.execution.adapter.is_some()
                    || self.execution.failure.is_some()
                    || self
                        .execution
                        .skip_reason
                        .as_deref()
                        .is_none_or(|reason| reason.trim().is_empty())
                {
                    return Err(GpuEvidenceError::InvalidExecutionState(
                        "skipped execution requires no calculation/adapter/failure and a non-empty skip reason",
                    ));
                }
            }
        }

        if self.execution.calculation_path == GpuCalculationPath::CpuReplacement
            && self.execution.status != GpuExecutionStatus::Failed
        {
            return Err(GpuEvidenceError::InvalidExecutionState(
                "CPU replacement cannot satisfy a calculative GPU execution claim",
            ));
        }

        if let Some(oracle) = &self.oracle {
            require_non_empty("oracle.implementation_id", &oracle.implementation_id)?;
            require_non_empty("oracle.revision", &oracle.revision)?;
            require_sha256("oracle.executable_sha256", &oracle.executable_sha256)?;
            require_sha256("oracle.output_sha256", &oracle.output_sha256)?;
            if self.execution.status == GpuExecutionStatus::Passed
                && self.comparison.verdict == NumericalVerdict::NotEvaluated
            {
                return Err(GpuEvidenceError::InvalidExecutionState(
                    "successful execution with a pinned oracle requires a numerical verdict",
                ));
            }
        }

        self.validate_comparison()?;

        Ok(())
    }

    /// Require this record to prove a successful paired oracle validation.
    ///
    /// Structural [`Self::validate`] accepts honest failed and skipped records
    /// so they can be retained as evidence. Gates that require a passing
    /// scientific result must call this method instead.
    ///
    /// # Errors
    ///
    /// Returns [`GpuEvidenceError`] for malformed evidence or unless the WGSL
    /// execution and pinned-oracle comparison both passed.
    pub fn require_paired_pass(&self) -> Result<(), GpuEvidenceError> {
        self.validate()?;
        if self.execution.status != GpuExecutionStatus::Passed
            || self.execution.calculation_path != GpuCalculationPath::WgslDevice
            || self.oracle.is_none()
            || self.comparison.verdict != NumericalVerdict::Passed
        {
            return Err(GpuEvidenceError::NotPassing);
        }
        Ok(())
    }

    fn validate_comparison(&self) -> Result<(), GpuEvidenceError> {
        match self.comparison.verdict {
            NumericalVerdict::NotEvaluated => {
                if self.comparison.policy.is_some()
                    || self.comparison.oracle_value_count != 0
                    || self.comparison.candidate_value_count != 0
                    || self.comparison.compared_value_count != 0
                    || self.comparison.max_absolute_error.is_some()
                    || self.comparison.max_relative_error.is_some()
                    || self.comparison.first_failure.is_some()
                {
                    return Err(GpuEvidenceError::InvalidComparisonState(
                        "not-evaluated comparison must not contain policy, values, errors, or a failure",
                    ));
                }
            }
            NumericalVerdict::Passed => {
                self.require_paired_comparison()?;
                validate_comparison_policy(self.comparison.policy)?;
                if self.comparison.oracle_value_count != self.comparison.candidate_value_count
                    || self.comparison.oracle_value_count == 0
                    || self.comparison.compared_value_count != self.comparison.oracle_value_count
                    || self.comparison.first_failure.is_some()
                {
                    return Err(GpuEvidenceError::InvalidComparisonState(
                        "passed comparison requires equal fully compared arrays and no failure",
                    ));
                }
                validate_error_summary(&self.comparison)?;
            }
            NumericalVerdict::Failed => {
                self.require_paired_comparison()?;
                validate_comparison_policy(self.comparison.policy)?;
                let failure = self.comparison.first_failure.as_ref().ok_or(
                    GpuEvidenceError::InvalidComparisonState(
                        "failed comparison requires a first failure",
                    ),
                )?;
                if self.comparison.compared_value_count
                    > self
                        .comparison
                        .oracle_value_count
                        .min(self.comparison.candidate_value_count)
                {
                    return Err(GpuEvidenceError::InvalidComparisonState(
                        "compared value count exceeds the supplied array lengths",
                    ));
                }
                if failure.kind == ComparisonFailureKind::LengthMismatch
                    && self.comparison.oracle_value_count == self.comparison.candidate_value_count
                {
                    return Err(GpuEvidenceError::InvalidComparisonState(
                        "length-mismatch failure requires different array lengths",
                    ));
                }
                validate_optional_error("max_absolute_error", self.comparison.max_absolute_error)?;
                validate_optional_error("max_relative_error", self.comparison.max_relative_error)?;
                validate_comparison_failure(&self.comparison, failure)?;
            }
        }
        Ok(())
    }

    fn require_paired_comparison(&self) -> Result<(), GpuEvidenceError> {
        if self.execution.status != GpuExecutionStatus::Passed || self.oracle.is_none() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "evaluated comparison requires successful WGSL execution and pinned oracle provenance",
            ));
        }
        Ok(())
    }
}

/// Errors raised while constructing or validating GPU evidence.
#[derive(Debug, Error, PartialEq)]
pub enum GpuEvidenceError {
    /// A comparison tolerance is negative or non-finite.
    #[error("{name} must be finite and non-negative, got {value}")]
    InvalidTolerance {
        /// Name of the invalid tolerance.
        name: &'static str,
        /// Invalid tolerance value.
        value: f64,
    },
    /// A required string field is empty.
    #[error("required evidence field `{0}` is empty")]
    EmptyField(&'static str),
    /// A content hash is not a lowercase hexadecimal SHA-256 value.
    #[error("evidence field `{field}` is not a lowercase SHA-256 value: {value}")]
    InvalidSha256 {
        /// Name of the invalid field.
        field: &'static str,
        /// Invalid value.
        value: String,
    },
    /// The schema identifier or version is unsupported.
    #[error("unsupported GPU evidence schema {id} version {version}")]
    UnsupportedSchema {
        /// Schema identifier.
        id: String,
        /// Schema version.
        version: u32,
    },
    /// Execution fields form a contradictory or non-fail-closed state.
    #[error("invalid GPU execution evidence: {0}")]
    InvalidExecutionState(&'static str),
    /// Numerical comparison fields form a contradictory or incomplete state.
    #[error("invalid GPU comparison evidence: {0}")]
    InvalidComparisonState(&'static str),
    /// The record is structurally valid but does not prove a paired pass.
    #[error(
        "GPU evidence does not contain a successful WGSL execution and pinned-oracle comparison"
    )]
    NotPassing,
}

/// Compare finite oracle and candidate values with the declared issue-owned tolerances.
///
/// A value passes when either its absolute error is within the absolute
/// tolerance or its symmetric relative error is within the relative tolerance.
/// Empty inputs, NaN, infinity, and length mismatches fail closed and are
/// represented without placing non-finite JSON numbers in the evidence document.
///
/// # Errors
///
/// Returns [`GpuEvidenceError::InvalidTolerance`] when the supplied policy was
/// constructed without [`ComparisonPolicy::new`] and contains an invalid value.
pub fn compare_finite_values(
    oracle_values: &[f64],
    candidate_values: &[f64],
    policy: ComparisonPolicy,
) -> Result<ComparisonEvidence, GpuEvidenceError> {
    let policy = ComparisonPolicy::new(policy.absolute_tolerance, policy.relative_tolerance)?;
    if let Some(failure) = comparison_shape_failure(oracle_values, candidate_values, policy) {
        return Ok(failure);
    }

    let mut max_absolute_error = 0.0_f64;
    let mut max_relative_error = 0.0_f64;
    for (index, (&oracle, &candidate)) in oracle_values
        .iter()
        .zip(candidate_values.iter())
        .enumerate()
    {
        if !oracle.is_finite() || !candidate.is_finite() {
            return Ok(failed_comparison(
                oracle_values.len(),
                candidate_values.len(),
                policy,
                ComparisonFailure {
                    kind: ComparisonFailureKind::NonFiniteValue,
                    index: Some(index),
                    oracle_value: oracle.is_finite().then_some(oracle),
                    candidate_value: candidate.is_finite().then_some(candidate),
                },
                index,
                (index > 0).then_some(max_absolute_error),
                (index > 0).then_some(max_relative_error),
            ));
        }

        let absolute_error = (candidate - oracle).abs();
        let relative_scale = candidate.abs().max(oracle.abs());
        let relative_error = if relative_scale == 0.0 {
            0.0
        } else {
            absolute_error / relative_scale
        };
        if !absolute_error.is_finite() || !relative_error.is_finite() {
            return Ok(failed_comparison(
                oracle_values.len(),
                candidate_values.len(),
                policy,
                ComparisonFailure {
                    kind: ComparisonFailureKind::NonFiniteError,
                    index: Some(index),
                    oracle_value: Some(oracle),
                    candidate_value: Some(candidate),
                },
                index,
                (index > 0).then_some(max_absolute_error),
                (index > 0).then_some(max_relative_error),
            ));
        }
        max_absolute_error = max_absolute_error.max(absolute_error);
        max_relative_error = max_relative_error.max(relative_error);

        if absolute_error > policy.absolute_tolerance && relative_error > policy.relative_tolerance
        {
            return Ok(failed_comparison(
                oracle_values.len(),
                candidate_values.len(),
                policy,
                ComparisonFailure {
                    kind: ComparisonFailureKind::ToleranceExceeded,
                    index: Some(index),
                    oracle_value: Some(oracle),
                    candidate_value: Some(candidate),
                },
                index + 1,
                Some(max_absolute_error),
                Some(max_relative_error),
            ));
        }
    }

    Ok(ComparisonEvidence {
        verdict: NumericalVerdict::Passed,
        policy: Some(policy),
        oracle_value_count: oracle_values.len(),
        candidate_value_count: candidate_values.len(),
        compared_value_count: oracle_values.len(),
        max_absolute_error: Some(max_absolute_error),
        max_relative_error: Some(max_relative_error),
        first_failure: None,
    })
}

fn comparison_shape_failure(
    oracle_values: &[f64],
    candidate_values: &[f64],
    policy: ComparisonPolicy,
) -> Option<ComparisonEvidence> {
    let kind = if oracle_values.len() != candidate_values.len() {
        ComparisonFailureKind::LengthMismatch
    } else if oracle_values.is_empty() {
        ComparisonFailureKind::EmptyInput
    } else {
        return None;
    };
    Some(failed_comparison(
        oracle_values.len(),
        candidate_values.len(),
        policy,
        ComparisonFailure {
            kind,
            index: None,
            oracle_value: None,
            candidate_value: None,
        },
        0,
        None,
        None,
    ))
}

fn failed_comparison(
    oracle_value_count: usize,
    candidate_value_count: usize,
    policy: ComparisonPolicy,
    first_failure: ComparisonFailure,
    compared_value_count: usize,
    max_absolute_error: Option<f64>,
    max_relative_error: Option<f64>,
) -> ComparisonEvidence {
    ComparisonEvidence {
        verdict: NumericalVerdict::Failed,
        policy: Some(policy),
        oracle_value_count,
        candidate_value_count,
        compared_value_count,
        max_absolute_error,
        max_relative_error,
        first_failure: Some(first_failure),
    }
}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), GpuEvidenceError> {
    if value.trim().is_empty() {
        return Err(GpuEvidenceError::EmptyField(field));
    }
    Ok(())
}

fn validate_comparison_policy(policy: Option<ComparisonPolicy>) -> Result<(), GpuEvidenceError> {
    let policy = policy.ok_or(GpuEvidenceError::InvalidComparisonState(
        "evaluated comparison requires a tolerance policy",
    ))?;
    ComparisonPolicy::new(policy.absolute_tolerance, policy.relative_tolerance).map(|_| ())
}

fn validate_error_summary(comparison: &ComparisonEvidence) -> Result<(), GpuEvidenceError> {
    let absolute =
        comparison
            .max_absolute_error
            .ok_or(GpuEvidenceError::InvalidComparisonState(
                "passed comparison requires a maximum absolute error",
            ))?;
    let relative =
        comparison
            .max_relative_error
            .ok_or(GpuEvidenceError::InvalidComparisonState(
                "passed comparison requires a maximum relative error",
            ))?;
    validate_optional_error("max_absolute_error", Some(absolute))?;
    validate_optional_error("max_relative_error", Some(relative))
}

fn validate_optional_error(
    field: &'static str,
    value: Option<f64>,
) -> Result<(), GpuEvidenceError> {
    if value.is_some_and(|error| !error.is_finite() || error < 0.0) {
        return Err(GpuEvidenceError::InvalidComparisonState(field));
    }
    Ok(())
}

fn validate_comparison_failure(
    comparison: &ComparisonEvidence,
    failure: &ComparisonFailure,
) -> Result<(), GpuEvidenceError> {
    validate_optional_finite_value(failure.oracle_value)?;
    validate_optional_finite_value(failure.candidate_value)?;
    match failure.kind {
        ComparisonFailureKind::EmptyInput => {
            if comparison.oracle_value_count != 0
                || comparison.candidate_value_count != 0
                || comparison.compared_value_count != 0
                || failure.index.is_some()
                || failure.oracle_value.is_some()
                || failure.candidate_value.is_some()
                || comparison.max_absolute_error.is_some()
                || comparison.max_relative_error.is_some()
            {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "empty-input failure must not contain counts, an index, or values",
                ));
            }
        }
        ComparisonFailureKind::LengthMismatch => {
            if failure.index.is_some()
                || failure.oracle_value.is_some()
                || failure.candidate_value.is_some()
                || comparison.compared_value_count != 0
                || comparison.max_absolute_error.is_some()
                || comparison.max_relative_error.is_some()
            {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "length mismatch must not contain an element index or values",
                ));
            }
        }
        ComparisonFailureKind::NonFiniteValue => {
            validate_element_failure(comparison, failure)?;
            if failure.oracle_value.is_some() && failure.candidate_value.is_some() {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "non-finite failure must omit at least one unrepresentable value",
                ));
            }
            validate_preceding_error_summary(comparison, failure)?;
        }
        ComparisonFailureKind::NonFiniteError => {
            validate_element_failure(comparison, failure)?;
            let (Some(oracle), Some(candidate)) = (failure.oracle_value, failure.candidate_value)
            else {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "non-finite error failure requires both finite input values",
                ));
            };
            let absolute_error = (candidate - oracle).abs();
            let relative_scale = candidate.abs().max(oracle.abs());
            let relative_error = if relative_scale == 0.0 {
                0.0
            } else {
                absolute_error / relative_scale
            };
            if absolute_error.is_finite() && relative_error.is_finite() {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "non-finite error failure requires overflowing comparison arithmetic",
                ));
            }
            validate_preceding_error_summary(comparison, failure)?;
        }
        ComparisonFailureKind::ToleranceExceeded => {
            validate_element_failure(comparison, failure)?;
            if failure.oracle_value.is_none() || failure.candidate_value.is_none() {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "tolerance failure requires both finite compared values",
                ));
            }
            if comparison.compared_value_count != failure.index.unwrap_or_default() + 1 {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "tolerance failure count must include the failing value",
                ));
            }
            validate_tolerance_failure(comparison, failure)?;
        }
    }
    Ok(())
}

fn validate_preceding_error_summary(
    comparison: &ComparisonEvidence,
    failure: &ComparisonFailure,
) -> Result<(), GpuEvidenceError> {
    let index = failure.index.unwrap_or_default();
    if comparison.compared_value_count != index {
        return Err(GpuEvidenceError::InvalidComparisonState(
            "non-finite failure count must stop before the failing value",
        ));
    }
    let has_previous_values = index > 0;
    if comparison.max_absolute_error.is_some() != has_previous_values
        || comparison.max_relative_error.is_some() != has_previous_values
    {
        return Err(GpuEvidenceError::InvalidComparisonState(
            "non-finite failure error maxima must summarize exactly the preceding values",
        ));
    }
    Ok(())
}

fn validate_tolerance_failure(
    comparison: &ComparisonEvidence,
    failure: &ComparisonFailure,
) -> Result<(), GpuEvidenceError> {
    let oracle = failure
        .oracle_value
        .ok_or(GpuEvidenceError::InvalidComparisonState(
            "tolerance failure requires an oracle value",
        ))?;
    let candidate = failure
        .candidate_value
        .ok_or(GpuEvidenceError::InvalidComparisonState(
            "tolerance failure requires a candidate value",
        ))?;
    let policy = comparison
        .policy
        .ok_or(GpuEvidenceError::InvalidComparisonState(
            "tolerance failure requires a comparison policy",
        ))?;
    let absolute_error = (candidate - oracle).abs();
    let relative_scale = candidate.abs().max(oracle.abs());
    let relative_error = if relative_scale == 0.0 {
        0.0
    } else {
        absolute_error / relative_scale
    };
    if absolute_error <= policy.absolute_tolerance || relative_error <= policy.relative_tolerance {
        return Err(GpuEvidenceError::InvalidComparisonState(
            "tolerance failure values do not exceed both declared tolerances",
        ));
    }
    if comparison
        .max_absolute_error
        .is_none_or(|maximum| maximum < absolute_error)
        || comparison
            .max_relative_error
            .is_none_or(|maximum| maximum < relative_error)
    {
        return Err(GpuEvidenceError::InvalidComparisonState(
            "tolerance failure requires error maxima covering the failing value",
        ));
    }
    Ok(())
}

fn validate_element_failure(
    comparison: &ComparisonEvidence,
    failure: &ComparisonFailure,
) -> Result<(), GpuEvidenceError> {
    if comparison.oracle_value_count != comparison.candidate_value_count {
        return Err(GpuEvidenceError::InvalidComparisonState(
            "element failure requires equal array lengths",
        ));
    }
    if failure
        .index
        .is_none_or(|index| index >= comparison.oracle_value_count)
    {
        return Err(GpuEvidenceError::InvalidComparisonState(
            "element failure requires an in-range index",
        ));
    }
    Ok(())
}

fn validate_optional_finite_value(value: Option<f64>) -> Result<(), GpuEvidenceError> {
    if value.is_some_and(|value| !value.is_finite()) {
        return Err(GpuEvidenceError::InvalidComparisonState(
            "comparison failure values must be finite when present",
        ));
    }
    Ok(())
}

fn validate_adapter(adapter: &GpuAdapterEvidence) -> Result<(), GpuEvidenceError> {
    if !matches!(
        adapter.backend.as_str(),
        "vulkan" | "metal" | "dx12" | "gl" | "browser_webgpu"
    ) {
        return Err(GpuEvidenceError::InvalidExecutionState(
            "passed execution requires a recognized non-empty backend",
        ));
    }
    if !matches!(
        adapter.device_type.as_str(),
        "other" | "integrated_gpu" | "discrete_gpu" | "virtual_gpu" | "cpu"
    ) {
        return Err(GpuEvidenceError::InvalidExecutionState(
            "passed execution requires a recognized device type",
        ));
    }
    let class_matches_type = match adapter.adapter_class {
        GpuAdapterClass::HardwareGpu => adapter.device_type != "cpu",
        GpuAdapterClass::SoftwareWgsl => adapter.device_type == "cpu",
    };
    if !class_matches_type {
        return Err(GpuEvidenceError::InvalidExecutionState(
            "adapter class contradicts the reported device type",
        ));
    }
    Ok(())
}

fn require_sha256(field: &'static str, value: &str) -> Result<(), GpuEvidenceError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(GpuEvidenceError::InvalidSha256 {
            field,
            value: value.to_string(),
        });
    }
    Ok(())
}

fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Empty => "empty",
        wgpu::Backend::Vulkan => "vulkan",
        wgpu::Backend::Metal => "metal",
        wgpu::Backend::Dx12 => "dx12",
        wgpu::Backend::Gl => "gl",
        wgpu::Backend::BrowserWebGpu => "browser_webgpu",
    }
}

fn device_type_name(device_type: wgpu::DeviceType) -> &'static str {
    match device_type {
        wgpu::DeviceType::Other => "other",
        wgpu::DeviceType::IntegratedGpu => "integrated_gpu",
        wgpu::DeviceType::DiscreteGpu => "discrete_gpu",
        wgpu::DeviceType::VirtualGpu => "virtual_gpu",
        wgpu::DeviceType::Cpu => "cpu",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA256_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA256_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn valid_adapter() -> GpuAdapterEvidence {
        GpuAdapterEvidence {
            name: "test-adapter".to_string(),
            backend: "vulkan".to_string(),
            device_type: "integrated_gpu".to_string(),
            vendor_id: 1,
            device_id: 2,
            driver: "test-driver".to_string(),
            driver_info: "test-driver-info".to_string(),
            adapter_class: GpuAdapterClass::HardwareGpu,
            software_fallback_requested: false,
        }
    }

    fn valid_paired_evidence() -> GpuCalculationEvidence {
        let policy = ComparisonPolicy::new(1.0e-6, 1.0e-3).expect("valid policy");
        GpuCalculationEvidence {
            schema: GpuEvidenceSchema::default(),
            case_id: "case-1".to_string(),
            candidate: GpuCandidateEvidence {
                implementation_id: "candidate".to_string(),
                revision: "candidate-revision".to_string(),
                shader_sha256: SHA256_A.to_string(),
                input_sha256: SHA256_B.to_string(),
            },
            execution: GpuExecutionEvidence {
                status: GpuExecutionStatus::Passed,
                calculation_path: GpuCalculationPath::WgslDevice,
                adapter: Some(valid_adapter()),
                failure: None,
                skip_reason: None,
            },
            oracle: Some(PinnedOracleEvidence {
                implementation_id: "FLEXPART-11.1".to_string(),
                revision: "pinned-revision".to_string(),
                executable_sha256: SHA256_A.to_string(),
                output_sha256: SHA256_B.to_string(),
            }),
            comparison: compare_finite_values(&[1.0], &[1.0], policy)
                .expect("valid comparison policy"),
        }
    }

    #[test]
    fn test_gpu_evidence_mixed_absolute_relative_tolerance_passes() {
        let policy = ComparisonPolicy::new(1.0e-6, 1.0e-3).expect("valid policy");
        let evidence = compare_finite_values(&[0.0, 1_000.0], &[5.0e-7, 1_000.5], policy)
            .expect("valid comparison policy");

        assert_eq!(evidence.verdict, NumericalVerdict::Passed);
        assert_eq!(evidence.compared_value_count, 2);
        assert!(evidence.first_failure.is_none());
    }

    #[test]
    fn test_gpu_evidence_non_finite_value_fails_closed() {
        let policy = ComparisonPolicy::new(0.0, 0.0).expect("valid policy");
        let evidence =
            compare_finite_values(&[1.0], &[f64::NAN], policy).expect("valid comparison policy");

        assert_eq!(evidence.verdict, NumericalVerdict::Failed);
        assert_eq!(
            evidence.first_failure.as_ref().map(|failure| failure.kind),
            Some(ComparisonFailureKind::NonFiniteValue)
        );
        serde_json::to_string(&evidence).expect("non-finite failure remains valid JSON");
    }

    #[test]
    fn test_gpu_evidence_non_finite_error_fails_closed() {
        let policy = ComparisonPolicy::new(f64::MAX, 1.0).expect("valid policy");
        let evidence = compare_finite_values(&[f64::MAX], &[-f64::MAX], policy)
            .expect("valid comparison policy");

        assert_eq!(evidence.verdict, NumericalVerdict::Failed);
        assert_eq!(
            evidence.first_failure.as_ref().map(|failure| failure.kind),
            Some(ComparisonFailureKind::NonFiniteError)
        );
        serde_json::to_string(&evidence).expect("overflow failure remains valid JSON");
    }

    #[test]
    fn test_gpu_evidence_length_mismatch_fails_closed() {
        let policy = ComparisonPolicy::new(0.0, 0.0).expect("valid policy");
        let evidence =
            compare_finite_values(&[1.0], &[1.0, 2.0], policy).expect("valid comparison policy");

        assert_eq!(evidence.verdict, NumericalVerdict::Failed);
        assert_eq!(
            evidence.first_failure.as_ref().map(|failure| failure.kind),
            Some(ComparisonFailureKind::LengthMismatch)
        );
    }

    #[test]
    fn test_gpu_evidence_empty_comparison_fails_closed() {
        let policy = ComparisonPolicy::new(0.0, 0.0).expect("valid policy");
        let evidence = compare_finite_values(&[], &[], policy).expect("valid comparison policy");

        assert_eq!(evidence.verdict, NumericalVerdict::Failed);
        assert_eq!(
            evidence.first_failure.as_ref().map(|failure| failure.kind),
            Some(ComparisonFailureKind::EmptyInput)
        );
    }

    #[test]
    fn test_gpu_evidence_tolerance_failure_is_not_a_paired_pass() {
        let policy = ComparisonPolicy::new(0.01, 0.01).expect("valid policy");
        let mut evidence = valid_paired_evidence();
        evidence.comparison =
            compare_finite_values(&[1.0], &[2.0], policy).expect("valid comparison policy");

        assert_eq!(evidence.comparison.verdict, NumericalVerdict::Failed);
        evidence
            .validate()
            .expect("honest numerical failure is structurally valid");
        assert_eq!(
            evidence.require_paired_pass(),
            Err(GpuEvidenceError::NotPassing)
        );
    }

    #[test]
    fn test_gpu_evidence_invalid_tolerance_is_rejected() {
        let invalid_policy = ComparisonPolicy {
            absolute_tolerance: f64::NAN,
            relative_tolerance: 0.0,
        };

        assert!(matches!(
            compare_finite_values(&[1.0], &[1.0], invalid_policy),
            Err(GpuEvidenceError::InvalidTolerance {
                name: "absolute_tolerance",
                ..
            })
        ));
    }

    #[test]
    fn test_gpu_evidence_complete_paired_record_validates() {
        let evidence = valid_paired_evidence();

        evidence.validate().expect("complete evidence is valid");
        evidence
            .require_paired_pass()
            .expect("complete paired evidence proves a pass");
        let json = serde_json::to_value(evidence).expect("evidence serializes");
        assert_eq!(json["schema"]["id"], GPU_EVIDENCE_SCHEMA_ID);
        assert_eq!(json["execution"]["calculation_path"], "wgsl_device");
        assert_eq!(json["comparison"]["verdict"], "passed");
    }

    #[test]
    fn test_gpu_evidence_incomplete_pass_fails_closed() {
        let mut evidence = valid_paired_evidence();
        evidence.comparison.compared_value_count = 0;

        assert!(matches!(
            evidence.validate(),
            Err(GpuEvidenceError::InvalidComparisonState(_))
        ));
    }

    #[test]
    fn test_gpu_evidence_cpu_replacement_cannot_pass() {
        let evidence = GpuCalculationEvidence {
            schema: GpuEvidenceSchema::default(),
            case_id: "case-1".to_string(),
            candidate: GpuCandidateEvidence {
                implementation_id: "candidate".to_string(),
                revision: "revision".to_string(),
                shader_sha256: SHA256_A.to_string(),
                input_sha256: SHA256_B.to_string(),
            },
            execution: GpuExecutionEvidence {
                status: GpuExecutionStatus::Passed,
                calculation_path: GpuCalculationPath::CpuReplacement,
                adapter: None,
                failure: None,
                skip_reason: None,
            },
            oracle: None,
            comparison: ComparisonEvidence::not_evaluated(),
        };

        assert!(evidence.validate().is_err());
    }

    #[test]
    fn test_gpu_evidence_skipped_run_cannot_prove_pass() {
        let evidence = GpuCalculationEvidence {
            schema: GpuEvidenceSchema::default(),
            case_id: "case-1".to_string(),
            candidate: GpuCandidateEvidence {
                implementation_id: "candidate".to_string(),
                revision: "revision".to_string(),
                shader_sha256: SHA256_A.to_string(),
                input_sha256: SHA256_B.to_string(),
            },
            execution: GpuExecutionEvidence {
                status: GpuExecutionStatus::Skipped,
                calculation_path: GpuCalculationPath::NotExecuted,
                adapter: None,
                failure: None,
                skip_reason: Some("adapter unavailable".to_string()),
            },
            oracle: Some(PinnedOracleEvidence {
                implementation_id: "FLEXPART-11.1".to_string(),
                revision: "pinned-revision".to_string(),
                executable_sha256: SHA256_A.to_string(),
                output_sha256: SHA256_B.to_string(),
            }),
            comparison: ComparisonEvidence::not_evaluated(),
        };

        evidence
            .validate()
            .expect("a skip is recordable but never passing");
        assert_eq!(
            evidence.require_paired_pass(),
            Err(GpuEvidenceError::NotPassing)
        );
        assert_ne!(evidence.execution.status, GpuExecutionStatus::Passed);
        assert_ne!(evidence.comparison.verdict, NumericalVerdict::Passed);
    }
}
