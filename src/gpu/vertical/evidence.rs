//! Vertical device-result rows/reports and unchanged fail-closed paired evidence.
use super::error::GpuVerticalError;
use super::preparation::is_vertically_sampled;
use super::provenance::{
    model_oracle_case_from_row_case_id, pinned_model_oracle_evidence_for_case,
    pinned_w_oracle_evidence, vertical_inputs_sha256, vertical_sample_shader_sha256,
    vertical_w_bundle_shader_sha256, vertical_w_inputs_sha256, VerticalModelOracleCase,
    VERTICAL_GPU_ABSOLUTE_TOLERANCE, VERTICAL_GPU_CANDIDATE_DESCRIPTION,
    VERTICAL_GPU_IMPLEMENTATION_ID, VERTICAL_GPU_RELATIVE_TOLERANCE_MODEL,
    VERTICAL_GPU_RELATIVE_TOLERANCE_W, VERTICAL_GPU_REPORT_SCHEMA_ID,
    VERTICAL_GPU_REPORT_SCHEMA_VERSION,
};
use crate::gpu::{
    compare_finite_values, ComparisonEvidence, ComparisonPolicy, GpuAdapterEvidence,
    GpuCalculationEvidence, GpuCalculationPath, GpuCandidateEvidence, GpuContext, GpuEvidenceError,
    GpuEvidenceSchema, GpuExecutionEvidence, GpuExecutionStatus, NumericalVerdict,
    PinnedOracleEvidence,
};
use crate::meteorology::{FieldId, SchemaIdentity, VerticalReference, VerticalStaggering};
use serde::{Deserialize, Serialize};

fn is_lowercase_git_sha(revision: &str) -> bool {
    revision.len() == 40
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_candidate_revision(revision: &str) -> Result<(), GpuVerticalError> {
    if !is_lowercase_git_sha(revision) {
        return Err(GpuVerticalError::InvalidCandidateRevision {
            value: revision.to_string(),
        });
    }
    Ok(())
}

/// One machine-readable GPU-vs-oracle comparison row for #88.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerticalGpuRow {
    /// Stable issue-owned validation case.
    pub case_id: String,
    /// Canonical field identity that was sampled.
    pub field_id: FieldId,
    /// Vertical staggering of the sampled grid.
    pub staggering: VerticalStaggering,
    /// Requested vertical reference (AGL or ASL).
    pub reference: VerticalReference,
    /// Requested height in metres in the requested reference.
    pub requested_height_m: f32,
    /// Query height resolved to AGL actually dispatched.
    pub query_height_agl_m: f32,
    /// Local terrain ASL used for ASL resolution.
    pub terrain_asl_m: f32,
    /// Geometry identity (provenance + counts + ordering).
    pub geometry_identity: String,
    /// Physical bottom-to-top lower index actually bracketing the query.
    pub lower_physical_index: usize,
    /// Physical bottom-to-top upper index (`lower + 1`).
    pub upper_physical_index: usize,
    /// Weight applied to the lower value (`dz2`).
    pub weight_lower: f32,
    /// Weight applied to the upper value (`dz1`).
    pub weight_upper: f32,
    /// Element value computed on the GPU.
    pub gpu_value: f32,
    /// Pinned oracle value.
    pub oracle_value: f32,
    /// CPU #73 diagnostic value (migration reference only, never the candidate).
    pub cpu_value: f32,
    /// Predeclared comparison policy.
    pub comparison_policy: ComparisonPolicy,
    /// Absolute GPU-oracle difference.
    pub absolute_difference: f64,
    /// Whether the GPU value matches the oracle within tolerance.
    pub value_verdict: bool,
    /// Combined row verdict.
    pub row_verdict: bool,
    /// Repository-wide GPU execution and numerical evidence.
    pub gpu_evidence: GpuCalculationEvidence,
}

/// Machine-readable GPU-vs-oracle comparison report for #88.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerticalGpuReport {
    /// Report schema identity.
    pub schema: SchemaIdentity,
    /// Stable scenario identifier covering the pinned vertical cases.
    pub scenario_id: String,
    /// Candidate description recorded for traceability.
    pub candidate: String,
    /// Predeclared comparison policy for the dominant lane.
    pub comparison_policy: ComparisonPolicy,
    /// Per-query comparison rows with GPU evidence.
    pub rows: Vec<VerticalGpuRow>,
    /// Overall report verdict (`true` only when every row passes).
    pub status: bool,
}

impl VerticalGpuRow {
    // Metadata identifies the exact f32 query dispatched, so numeric tolerances
    // must not permit a different height/reference to masquerade as that query.
    #[allow(clippy::float_cmp)]
    fn validate_metadata(&self) -> Result<(), GpuEvidenceError> {
        if ![
            self.requested_height_m,
            self.query_height_agl_m,
            self.terrain_asl_m,
            self.cpu_value,
        ]
        .iter()
        .all(|value| value.is_finite())
            || !self.absolute_difference.is_finite()
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "non-finite vertical row metadata",
            ));
        }
        let resolved = match self.reference {
            VerticalReference::AboveGroundLevel => self.requested_height_m,
            VerticalReference::AboveMeanSeaLevel => self.requested_height_m - self.terrain_asl_m,
            VerticalReference::ModelNative => {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "vertical row reference must be explicit AGL or ASL",
                ))
            }
        };
        if resolved != self.query_height_agl_m {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row height resolution contradicts query",
            ));
        }
        let relative_limit = match self.staggering {
            VerticalStaggering::LevelCenter
                if is_vertically_sampled(self.field_id)
                    && self.field_id != FieldId::VerticalVelocity =>
            {
                VERTICAL_GPU_RELATIVE_TOLERANCE_MODEL
            }
            VerticalStaggering::LevelInterface if self.field_id == FieldId::VerticalVelocity => {
                VERTICAL_GPU_RELATIVE_TOLERANCE_W
            }
            _ => {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "vertical row field or staggering is unsupported by the pinned oracle",
                ))
            }
        };
        validate_vertical_policy(self.comparison_policy, relative_limit)?;
        Ok(())
    }

    /// Validate structural honesty and fail closed on contradiction.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError`] for contradictory verdicts, differences
    /// or embedded evidence that cannot prove a claimed pass.
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        self.validate_metadata()?;
        let comparison = compare_finite_values(
            &[f64::from(self.oracle_value)],
            &[f64::from(self.gpu_value)],
            self.comparison_policy,
        )?;
        if self.gpu_evidence.comparison != comparison {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU comparison contradicts vertical row values or policy",
            ));
        }
        if !is_lowercase_git_sha(&self.gpu_evidence.candidate.revision) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "candidate revision is not a lowercase Git commit SHA",
            ));
        }
        if self.gpu_evidence.candidate.implementation_id != VERTICAL_GPU_IMPLEMENTATION_ID {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU provenance contradicts the vertical calculation",
            ));
        }
        let expected_shader = if self.staggering == VerticalStaggering::LevelInterface {
            vertical_w_bundle_shader_sha256()
        } else {
            vertical_sample_shader_sha256()
        };
        if self.gpu_evidence.candidate.shader_sha256 != expected_shader {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU shader contradicts the vertical stage",
            ));
        }
        if self.staggering == VerticalStaggering::LevelInterface {
            if self.gpu_evidence.oracle.as_ref() != Some(&pinned_w_oracle_evidence()) {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "embedded GPU oracle contradicts the pinned W production oracle",
                ));
            }
        } else {
            let expected_case = model_oracle_case_from_row_case_id(&self.case_id)?;
            if self.gpu_evidence.oracle.as_ref()
                != Some(&pinned_model_oracle_evidence_for_case(expected_case))
            {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "embedded GPU oracle contradicts the pinned model oracle case",
                ));
            }
        }
        if self.gpu_evidence.case_id
            != format!("{}/height={}", self.case_id, self.query_height_agl_m)
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU case id contradicts the vertical row case",
            ));
        }
        if self.geometry_identity.trim().is_empty() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row geometry identity must not be empty",
            ));
        }
        let value_pass = comparison.verdict == NumericalVerdict::Passed;
        if self.value_verdict != value_pass {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical value verdict contradicts tolerance",
            ));
        }
        if self.row_verdict != self.value_verdict {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row verdict contradicts value verdict",
            ));
        }
        let expected_difference = (f64::from(self.gpu_value) - f64::from(self.oracle_value)).abs();
        if (self.absolute_difference - expected_difference).abs() > 1.0e-12 {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical absolute difference contradicts values",
            ));
        }
        if self.lower_physical_index.checked_add(1) != Some(self.upper_physical_index) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row indices must satisfy upper == lower + 1",
            ));
        }
        if !(0.0..=1.0).contains(&self.weight_lower) || !(0.0..=1.0).contains(&self.weight_upper) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row weights must lie in [0, 1]",
            ));
        }
        if ((self.weight_lower + self.weight_upper) - 1.0).abs() > 1.0e-5 {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row weights must sum to one",
            ));
        }
        self.gpu_evidence.validate()?;
        if self.row_verdict {
            self.gpu_evidence.require_paired_pass()?;
        }
        Ok(())
    }
}

impl VerticalGpuReport {
    /// Validate structural honesty and fail closed on contradiction.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError`] when the schema, verdicts, tolerances or
    /// embedded evidence are missing or contradictory.
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        validate_vertical_policy(
            self.comparison_policy,
            VERTICAL_GPU_RELATIVE_TOLERANCE_MODEL,
        )?;
        if self.scenario_id.trim().is_empty() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical report scenario must not be empty",
            ));
        }
        if self.schema.id != VERTICAL_GPU_REPORT_SCHEMA_ID
            || self.schema.version != VERTICAL_GPU_REPORT_SCHEMA_VERSION
        {
            return Err(GpuEvidenceError::UnsupportedSchema {
                id: self.schema.id.clone(),
                version: self.schema.version,
            });
        }
        if self.candidate != VERTICAL_GPU_CANDIDATE_DESCRIPTION {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical GPU report does not identify the pinned candidate",
            ));
        }
        if self.rows.is_empty() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical GPU report must contain at least one row",
            ));
        }
        let candidate_revision = self.rows[0].gpu_evidence.candidate.revision.clone();
        for row in &self.rows {
            if row.comparison_policy.absolute_tolerance > self.comparison_policy.absolute_tolerance
                || row.comparison_policy.relative_tolerance
                    > self.comparison_policy.relative_tolerance
            {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "vertical row policy exceeds its report policy",
                ));
            }
            if row.gpu_evidence.candidate.revision != candidate_revision {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "vertical GPU row candidate revision contradicts its report",
                ));
            }
            row.validate()?;
        }
        let all_pass = self.rows.iter().all(|row| row.row_verdict);
        if self.status != all_pass {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical GPU report status contradicts row verdicts",
            ));
        }
        Ok(())
    }

    /// Require this report to prove a successful paired oracle validation.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError`] unless every row validates and proves a
    /// WGSL paired pass.
    pub fn require_paired_pass(&self) -> Result<(), GpuEvidenceError> {
        self.validate()?;
        if !self.status {
            return Err(GpuEvidenceError::NotPassing);
        }
        for row in &self.rows {
            row.gpu_evidence.require_paired_pass()?;
            if !row.row_verdict {
                return Err(GpuEvidenceError::NotPassing);
            }
        }
        Ok(())
    }
}

fn validate_vertical_policy(
    policy: ComparisonPolicy,
    relative_limit: f64,
) -> Result<(), GpuEvidenceError> {
    ComparisonPolicy::new(policy.absolute_tolerance, policy.relative_tolerance)?;
    if policy.absolute_tolerance > VERTICAL_GPU_ABSOLUTE_TOLERANCE
        || policy.relative_tolerance > relative_limit
    {
        return Err(GpuEvidenceError::InvalidComparisonState(
            "vertical policy exceeds the issue-owned oracle tolerances",
        ));
    }
    Ok(())
}

/// Borrowed W source lanes bound into two-stage evidence input hashes.
///
/// These are the exact host-to-device remap inputs
/// ([`super::resources::VerticalWInterfaceInputs`]); the device-computed shared values are
/// never an input and are therefore not hashed.
#[derive(Debug, Clone, Copy)]
pub struct VerticalWSourceLanes<'a> {
    /// Interface heights AGL in physical bottom-to-top order (`nz + 1`).
    pub interface_heights_agl_m: &'a [f32],
    /// Interface values in physical bottom-to-top order (`nz + 1`).
    pub interface_values_ms: &'a [f32],
    /// Level heights AGL in physical bottom-to-top order (`nz`).
    pub level_heights_agl_m: &'a [f32],
}

#[allow(clippy::too_many_arguments)]
fn build_paired_gpu_evidence(
    ctx: &GpuContext,
    case_id: &str,
    query_height_agl_m: f32,
    shader_sha256: String,
    input_sha256: String,
    oracle: PinnedOracleEvidence,
    comparison: ComparisonEvidence,
    candidate_revision: &str,
) -> Result<GpuCalculationEvidence, GpuEvidenceError> {
    let gpu_evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: format!("{case_id}/height={query_height_agl_m}"),
        candidate: GpuCandidateEvidence {
            implementation_id: VERTICAL_GPU_IMPLEMENTATION_ID.to_string(),
            revision: candidate_revision.to_string(),
            shader_sha256,
            input_sha256,
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(GpuAdapterEvidence::from_context(ctx)),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(oracle),
        comparison,
    };
    gpu_evidence.validate()?;
    Ok(gpu_evidence)
}

#[allow(clippy::too_many_arguments)]
fn finish_vertical_gpu_row(
    ctx: &GpuContext,
    case_id: &str,
    field_id: FieldId,
    staggering: VerticalStaggering,
    reference: VerticalReference,
    requested_height_m: f32,
    query_height_agl_m: f32,
    terrain_asl_m: f32,
    geometry_identity: &str,
    lower_physical_index: usize,
    upper_physical_index: usize,
    weight_lower: f32,
    weight_upper: f32,
    gpu_value: f32,
    oracle_value: f32,
    cpu_value: f32,
    comparison_policy: ComparisonPolicy,
    shader_sha256: String,
    input_sha256: String,
    oracle: PinnedOracleEvidence,
    candidate_revision: &str,
) -> Result<VerticalGpuRow, GpuVerticalError> {
    if !gpu_value.is_finite() || !oracle_value.is_finite() || !cpu_value.is_finite() {
        return Err(GpuVerticalError::NonFiniteOutputValue { query_index: 0 });
    }
    if geometry_identity.trim().is_empty() {
        return Err(GpuVerticalError::OracleContract {
            message: "vertical row geometry identity must not be empty",
        });
    }
    validate_candidate_revision(candidate_revision)?;
    let comparison = compare_finite_values(
        &[f64::from(oracle_value)],
        &[f64::from(gpu_value)],
        comparison_policy,
    )?;
    let value_verdict = comparison.verdict == NumericalVerdict::Passed;
    let gpu_evidence = build_paired_gpu_evidence(
        ctx,
        case_id,
        query_height_agl_m,
        shader_sha256,
        input_sha256,
        oracle,
        comparison,
        candidate_revision,
    )?;
    let row = VerticalGpuRow {
        case_id: case_id.to_string(),
        field_id,
        staggering,
        reference,
        requested_height_m,
        query_height_agl_m,
        terrain_asl_m,
        geometry_identity: geometry_identity.to_string(),
        lower_physical_index,
        upper_physical_index,
        weight_lower,
        weight_upper,
        gpu_value,
        oracle_value,
        cpu_value,
        comparison_policy,
        absolute_difference: (f64::from(gpu_value) - f64::from(oracle_value)).abs(),
        value_verdict,
        row_verdict: value_verdict,
        gpu_evidence,
    };
    row.validate()?;
    Ok(row)
}

fn validate_evidence_query(
    grid_count: usize,
    queries: &[f32],
    query: f32,
    lower: usize,
    upper: usize,
) -> Result<(), GpuVerticalError> {
    if !queries.contains(&query) {
        return Err(GpuVerticalError::OracleContract {
            message: "row query is absent from the hashed device batch",
        });
    }
    if lower.checked_add(1) != Some(upper) || upper >= grid_count {
        return Err(GpuVerticalError::OracleContract {
            message: "row indices are outside the hashed geometry",
        });
    }
    Ok(())
}

/// Build one model-level GPU-vs-oracle row with #91 execution evidence.
///
/// `gpu_value` must come from actual device execution and `oracle_value` from
/// the pinned #71 oracle case named by `model_case`; `cpu_value` is diagnostic
/// only. Level indices/weights are host-derived diagnostics from the identical
/// #73 primitive; they do not replace device execution proof. Vertical motion
/// is rejected here (no pinned #71 model oracle exists for motion):
/// center-staggered motion is covered by CPU-parity tests without paired
/// oracle evidence, and interface motion requires
/// [`build_vertical_w_gpu_row`].
///
/// # Errors
/// Returns [`GpuVerticalError`] for motion fields, case-id mismatches,
/// invalid tolerances, non-finite values, invalid candidate provenance or
/// evidence construction failures.
#[allow(clippy::too_many_arguments)]
pub fn build_vertical_model_gpu_row(
    ctx: &GpuContext,
    case_id: &str,
    model_case: VerticalModelOracleCase,
    field_id: FieldId,
    reference: VerticalReference,
    requested_height_m: f32,
    query_height_agl_m: f32,
    terrain_asl_m: f32,
    geometry_identity: &str,
    lower_physical_index: usize,
    upper_physical_index: usize,
    weight_lower: f32,
    weight_upper: f32,
    gpu_value: f32,
    oracle_value: f32,
    cpu_value: f32,
    comparison_policy: ComparisonPolicy,
    grid_heights_agl_m: &[f32],
    grid_values: &[f32],
    all_queries_agl_m: &[f32],
    candidate_revision: &str,
) -> Result<VerticalGpuRow, GpuVerticalError> {
    if !case_id.contains(model_case.as_str()) {
        return Err(GpuVerticalError::OracleContract {
            message: "model row case id must name its pinned #71 oracle case",
        });
    }
    if field_id == FieldId::VerticalVelocity {
        return Err(GpuVerticalError::OracleContract {
            message: "model rows require a non-motion field with a pinned #71 oracle",
        });
    }
    let input_sha = vertical_inputs_sha256(
        grid_heights_agl_m,
        grid_values,
        all_queries_agl_m,
        VerticalStaggering::LevelCenter,
        geometry_identity,
    )?;
    validate_evidence_query(
        grid_heights_agl_m.len(),
        all_queries_agl_m,
        query_height_agl_m,
        lower_physical_index,
        upper_physical_index,
    )?;
    finish_vertical_gpu_row(
        ctx,
        case_id,
        field_id,
        VerticalStaggering::LevelCenter,
        reference,
        requested_height_m,
        query_height_agl_m,
        terrain_asl_m,
        geometry_identity,
        lower_physical_index,
        upper_physical_index,
        weight_lower,
        weight_upper,
        gpu_value,
        oracle_value,
        cpu_value,
        comparison_policy,
        vertical_sample_shader_sha256(),
        input_sha,
        pinned_model_oracle_evidence_for_case(model_case),
        candidate_revision,
    )
}

/// Build one two-stage W GPU-vs-oracle row with #91 execution evidence.
///
/// `gpu_value` must come from actual two-stage device execution (remap plus
/// sample) and `oracle_value` from the pinned #80 production-path oracle;
/// `cpu_value` is diagnostic only. The input hash binds the shared grid
/// heights, every dispatched W source lane, and the queries, so the hashed
/// inputs provably correspond to the dispatched work.
///
/// # Errors
/// Returns [`GpuVerticalError`] for non-W fields, invalid tolerances,
/// non-finite values, invalid candidate provenance or evidence construction
/// failures.
#[allow(clippy::too_many_arguments)]
pub fn build_vertical_w_gpu_row(
    ctx: &GpuContext,
    case_id: &str,
    field_id: FieldId,
    reference: VerticalReference,
    requested_height_m: f32,
    query_height_agl_m: f32,
    terrain_asl_m: f32,
    geometry_identity: &str,
    lower_physical_index: usize,
    upper_physical_index: usize,
    weight_lower: f32,
    weight_upper: f32,
    gpu_value: f32,
    oracle_value: f32,
    cpu_value: f32,
    comparison_policy: ComparisonPolicy,
    shared_heights_agl_m: &[f32],
    w_source: VerticalWSourceLanes<'_>,
    all_queries_agl_m: &[f32],
    candidate_revision: &str,
) -> Result<VerticalGpuRow, GpuVerticalError> {
    if field_id != FieldId::VerticalVelocity {
        return Err(GpuVerticalError::OracleContract {
            message: "W rows require the vertical-velocity field",
        });
    }
    let input_sha = vertical_w_inputs_sha256(
        shared_heights_agl_m,
        w_source.interface_heights_agl_m,
        w_source.interface_values_ms,
        w_source.level_heights_agl_m,
        all_queries_agl_m,
        geometry_identity,
    )?;
    validate_evidence_query(
        shared_heights_agl_m.len(),
        all_queries_agl_m,
        query_height_agl_m,
        lower_physical_index,
        upper_physical_index,
    )?;
    finish_vertical_gpu_row(
        ctx,
        case_id,
        field_id,
        VerticalStaggering::LevelInterface,
        reference,
        requested_height_m,
        query_height_agl_m,
        terrain_asl_m,
        geometry_identity,
        lower_physical_index,
        upper_physical_index,
        weight_lower,
        weight_upper,
        gpu_value,
        oracle_value,
        cpu_value,
        comparison_policy,
        vertical_w_bundle_shader_sha256(),
        input_sha,
        pinned_w_oracle_evidence(),
        candidate_revision,
    )
}
