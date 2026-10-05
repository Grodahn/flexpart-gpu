//! Frozen vertical candidate/oracle identities and normalized device-input hashes.
use super::error::GpuVerticalError;
use super::pipeline::{REMAP_SHADER_SOURCE, SAMPLE_SHADER_SOURCE};
use super::resources::{validate_finite_lane, validate_grid_columns, validate_interface_columns};
use crate::gpu::{ComparisonPolicy, GpuEvidenceError, PinnedOracleEvidence};
use crate::meteorology::{
    vertical::VerticalRuntimeView, vertical_sampling::VerticalSamplingError, VerticalStaggering,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Candidate implementation identity for machine-readable evidence.
///
/// Both the ordinary sample stage and the W two-stage bundle (remap plus
/// sample) report this single identity; the executed shader bundle recorded
/// alongside it distinguishes the stages.
pub const VERTICAL_GPU_IMPLEMENTATION_ID: &str =
    "meteorology::vertical-gpu::encode_vertical_sample";

/// Human-readable candidate description recorded in issue-specific reports.
pub const VERTICAL_GPU_CANDIDATE_DESCRIPTION: &str =
    "meteorology::vertical-gpu::encode_vertical_sample+remap_w (WGSL device)";

/// Pinned FLEXPART 11.1 revision owning the #71/#80 vertical oracles.
pub const VERTICAL_ORACLE_REVISION: &str = "c70586c2b7f5258850705325881c61f557ea9bd8";

/// Pinned oracle linked-object-set identity from #71 provenance.
///
/// This digest identifies the linked `.o` set of the #71 direct-interpolation
/// oracle driver (`linked_object_set_sha256` in
/// `fixtures/interpolation/contract-v1.provenance.json`) and is the
/// executable identity for model-level evidence.
pub const VERTICAL_ORACLE_EXECUTABLE_SHA256: &str =
    "388d1f824306df30fc74fbedc6464e86602b6a93ba782e9e1dad20ffacc3327c";

/// Pinned per-case oracle output digest for the #71 `vertical-model-levels` case.
///
/// From the `cases` map in
/// `fixtures/interpolation/contract-v1.provenance.json`; this digest
/// identifies the oracle output artifact, not the fixture file.
pub const VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_MODEL_LEVELS: &str =
    "95f72332cb4b5e8e2878debaba12bb41f4a45234514687a86d7ec9e7abcc2520";

/// Pinned per-case oracle output digest for the #71
/// `real-era5-etex-temperature-column` case.
///
/// From the `cases` map in
/// `fixtures/interpolation/contract-v1.provenance.json`; this digest
/// identifies the oracle output artifact, not the fixture file.
pub const VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_REAL_COLUMN: &str =
    "5679760f10c75679ecd597979b5caadfd3bf30debbbf9b0fa1fc6af1aa9e3771";

/// Pinned digest of the #80 W production oracle executable.
///
/// This is the `binary_sha256` recorded in the `build` section of
/// `fixtures/interpolation/w-production-oracle-v1.json` provenance: the
/// linked oracle binary actually executed for the production-path oracle.
/// W evidence records this digest as the oracle executable identity.
pub const VERTICAL_W_ORACLE_BINARY_SHA256: &str =
    "9e2ff66ab93a815cd90011943042e23ecf3f44a8ce000de9db819d02f9ef4c95";

/// Pinned #71 model-level oracle case identified in machine-readable evidence.
///
/// Each case carries its own oracle-output digest from the `cases` map in
/// `fixtures/interpolation/contract-v1.provenance.json`. A single shared
/// digest cannot identify both cases, so evidence rows must name exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalModelOracleCase {
    /// Synthetic `vertical-model-levels` oracle case.
    ModelLevels,
    /// Real-data `real-era5-etex-temperature-column` oracle case.
    RealEra5Column,
}

impl VerticalModelOracleCase {
    /// Stable oracle case identifier matching the #71 contract fixture.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ModelLevels => "vertical-model-levels",
            Self::RealEra5Column => "real-era5-etex-temperature-column",
        }
    }

    /// Pinned oracle-output digest identifying this case's oracle artifact.
    #[must_use]
    pub const fn output_sha256(self) -> &'static str {
        match self {
            Self::ModelLevels => VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_MODEL_LEVELS,
            Self::RealEra5Column => VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_REAL_COLUMN,
        }
    }
}

/// Pinned raw oracle output digest for the #80 W production oracle.
pub const VERTICAL_W_ORACLE_OUTPUT_SHA256: &str =
    "730c08209e4ffd7f7c941926846e53bc9cd30b2b255d69eaf3b21e40337c2671";

/// Pinned oracle implementation for model-level vertical sampling.
pub const VERTICAL_MODEL_ORACLE_IMPLEMENTATION_ID: &str = "FLEXPART-11.1 vert_interpol";

/// Pinned oracle implementation for the #80 W production path.
pub const VERTICAL_W_ORACLE_IMPLEMENTATION_ID: &str =
    "FLEXPART-11.1 verttransform_ecmwf_windfields+interpol_wind_meter";

/// Absolute tolerance for GPU comparison (matches #73/#80 candidate rules).
pub const VERTICAL_GPU_ABSOLUTE_TOLERANCE: f64 = 1.0e-6;

/// Relative tolerance for model-level GPU comparison (matches #71/#73 `1e-4`).
pub const VERTICAL_GPU_RELATIVE_TOLERANCE_MODEL: f64 = 1.0e-4;

/// Relative tolerance for W/interface GPU comparison (matches #80 `1e-5`).
pub const VERTICAL_GPU_RELATIVE_TOLERANCE_W: f64 = 1.0e-5;

/// Schema identity for issue-specific vertical GPU evidence.
pub const VERTICAL_GPU_REPORT_SCHEMA_ID: &str = "flexpart-gpu.vertical-gpu-evidence";

/// Version of the issue-specific vertical GPU evidence schema.
pub const VERTICAL_GPU_REPORT_SCHEMA_VERSION: u32 = 1;

/// Lowercase hexadecimal SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// SHA-256 of the executed sampling WGSL source.
#[must_use]
pub fn vertical_sample_shader_sha256() -> String {
    sha256_hex(SAMPLE_SHADER_SOURCE.as_bytes())
}

/// SHA-256 of the executed W remap WGSL source.
#[must_use]
pub fn vertical_remap_shader_sha256() -> String {
    sha256_hex(REMAP_SHADER_SOURCE.as_bytes())
}

/// SHA-256 of the executed W two-stage bundle (sample + remap sources).
#[must_use]
pub fn vertical_w_bundle_shader_sha256() -> String {
    let mut combined =
        Vec::with_capacity(SAMPLE_SHADER_SOURCE.len() + REMAP_SHADER_SOURCE.len() + 1);
    combined.extend_from_slice(SAMPLE_SHADER_SOURCE.as_bytes());
    combined.push(b'\n');
    combined.extend_from_slice(REMAP_SHADER_SOURCE.as_bytes());
    sha256_hex(&combined)
}

/// SHA-256 of the normalized JSON encoding of validated model-level GPU inputs.
///
/// Binds grid heights/values, query heights, staggering, and geometry identity
/// so the hashed inputs provably correspond to the dispatched work. For the
/// two-stage W path use [`vertical_w_inputs_sha256`], which additionally binds
/// the dispatched W source lanes.
///
/// # Errors
/// Returns [`GpuVerticalError`] for malformed device input batches or failed JSON encoding.
pub fn vertical_inputs_sha256(
    grid_heights_agl_m: &[f32],
    grid_values: &[f32],
    query_heights_agl_m: &[f32],
    staggering: VerticalStaggering,
    geometry_identity: &str,
) -> Result<String, GpuVerticalError> {
    #[derive(Serialize)]
    struct NormalizedVerticalInput<'a> {
        grid_heights_agl_m: &'a [f32],
        grid_values: &'a [f32],
        query_heights_agl_m: &'a [f32],
        staggering: String,
        geometry_identity: &'a str,
    }
    validate_grid_columns(grid_heights_agl_m, grid_values)?;
    if query_heights_agl_m.is_empty() {
        return Err(GpuVerticalError::EmptyQueries);
    }
    validate_finite_lane(query_heights_agl_m, "query_heights_agl_m")?;
    let normalized = NormalizedVerticalInput {
        grid_heights_agl_m,
        grid_values,
        query_heights_agl_m,
        staggering: format!("{staggering:?}"),
        geometry_identity,
    };
    let json = serde_json::to_vec(&normalized).map_err(|err| GpuVerticalError::InputHash {
        message: err.to_string(),
    })?;
    Ok(sha256_hex(&json))
}

/// SHA-256 of the normalized JSON encoding of validated two-stage W GPU inputs.
///
/// Binds every host-to-device input of the W production path: the shared grid
/// heights, the W source lanes (interface heights/values plus level heights),
/// the query heights, and the geometry identity. The shared *values* buffer
/// is device-computed by the remap kernel and therefore cannot be part of the
/// dispatched-input binding; it is never hashed as an input.
///
/// # Errors
/// Returns [`GpuVerticalError`] for malformed device input batches or failed JSON encoding.
pub fn vertical_w_inputs_sha256(
    shared_heights_agl_m: &[f32],
    interface_heights_agl_m: &[f32],
    interface_values_ms: &[f32],
    level_heights_agl_m: &[f32],
    query_heights_agl_m: &[f32],
    geometry_identity: &str,
) -> Result<String, GpuVerticalError> {
    #[derive(Serialize)]
    struct NormalizedVerticalWInput<'a> {
        shared_heights_agl_m: &'a [f32],
        interface_heights_agl_m: &'a [f32],
        interface_values_ms: &'a [f32],
        level_heights_agl_m: &'a [f32],
        query_heights_agl_m: &'a [f32],
        staggering: String,
        geometry_identity: &'a str,
    }
    let nz = validate_interface_columns(
        interface_heights_agl_m,
        interface_values_ms,
        level_heights_agl_m,
    )?;
    if shared_heights_agl_m.len() != nz + 1
        || shared_heights_agl_m[0] != 0.0
        || shared_heights_agl_m[1..] != *level_heights_agl_m
    {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "shared grid heights differ from the W source model heights",
        }
        .into());
    }
    if query_heights_agl_m.is_empty() {
        return Err(GpuVerticalError::EmptyQueries);
    }
    validate_finite_lane(query_heights_agl_m, "query_heights_agl_m")?;
    let normalized = NormalizedVerticalWInput {
        shared_heights_agl_m,
        interface_heights_agl_m,
        interface_values_ms,
        level_heights_agl_m,
        query_heights_agl_m,
        staggering: format!("{:?}", VerticalStaggering::LevelInterface),
        geometry_identity,
    };
    let json = serde_json::to_vec(&normalized).map_err(|err| GpuVerticalError::InputHash {
        message: err.to_string(),
    })?;
    Ok(sha256_hex(&json))
}

/// Geometry identity string for evidence (provenance + level count + ordering).
#[must_use]
pub fn vertical_geometry_identity(runtime: VerticalRuntimeView<'_>) -> String {
    let provenance = runtime.provenance();
    let (nx, ny, nz) = runtime.dimensions();
    format!(
        "snapshot={}:ordering={:?}:levels={}:nx={}:ny={}:pressure={}:height={}:wheight={}",
        provenance.source_snapshot_sha256,
        provenance.source_vertical_ordering,
        nz,
        nx,
        ny,
        provenance.pressure_algorithm_id,
        provenance.height_algorithm_id,
        provenance.w_height_algorithm_id
    )
}

/// Build the repository-wide model-level comparison policy.
///
/// Absolute `1e-6` OR relative `1e-4` follows the #91 finite comparator.
/// This is at least as strict as the additive #71/#73 fixture rule.
///
/// # Errors
/// Returns [`GpuEvidenceError`] for an invalid tolerance policy.
pub fn vertical_model_comparison_policy() -> Result<ComparisonPolicy, GpuEvidenceError> {
    ComparisonPolicy::new(
        VERTICAL_GPU_ABSOLUTE_TOLERANCE,
        VERTICAL_GPU_RELATIVE_TOLERANCE_MODEL,
    )
}

/// Build the repository-wide W/interface comparison policy.
///
/// Absolute `1e-6` OR relative `1e-5` follows the #91 finite comparator.
/// This is at least as strict as the additive #80 fixture rule.
///
/// # Errors
/// Returns [`GpuEvidenceError`] for an invalid tolerance policy.
pub fn vertical_w_comparison_policy() -> Result<ComparisonPolicy, GpuEvidenceError> {
    ComparisonPolicy::new(
        VERTICAL_GPU_ABSOLUTE_TOLERANCE,
        VERTICAL_GPU_RELATIVE_TOLERANCE_W,
    )
}

/// Pinned oracle evidence for one #71 model-level oracle case.
///
/// The executable digest is the linked-object-set identity from #71
/// provenance; the output digest is the per-case oracle-output digest from
/// the `cases` map in `fixtures/interpolation/contract-v1.provenance.json`.
pub(super) fn pinned_model_oracle_evidence_for_case(
    case: VerticalModelOracleCase,
) -> PinnedOracleEvidence {
    PinnedOracleEvidence {
        implementation_id: VERTICAL_MODEL_ORACLE_IMPLEMENTATION_ID.to_string(),
        revision: VERTICAL_ORACLE_REVISION.to_string(),
        executable_sha256: VERTICAL_ORACLE_EXECUTABLE_SHA256.to_string(),
        output_sha256: case.output_sha256().to_string(),
    }
}

/// Resolve the pinned #71 model-level oracle case named by a row case id.
///
/// The row case id must contain exactly one known #71 oracle case identifier
/// (`vertical-model-levels` or `real-era5-etex-temperature-column`); otherwise
/// the row cannot be bound to a pinned output artifact and validation fails.
///
/// # Errors
/// Returns [`GpuEvidenceError::InvalidComparisonState`] when the case id
/// names zero or more than one known oracle case.
pub(super) fn model_oracle_case_from_row_case_id(
    case_id: &str,
) -> Result<VerticalModelOracleCase, GpuEvidenceError> {
    let mut found: Option<VerticalModelOracleCase> = None;
    for case in [
        VerticalModelOracleCase::ModelLevels,
        VerticalModelOracleCase::RealEra5Column,
    ] {
        if case_id.contains(case.as_str()) {
            if found.is_some() {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "vertical row case id names more than one pinned model oracle case",
                ));
            }
            found = Some(case);
        }
    }
    found.ok_or(GpuEvidenceError::InvalidComparisonState(
        "vertical row case id names no pinned model oracle case",
    ))
}

pub(super) fn pinned_w_oracle_evidence() -> PinnedOracleEvidence {
    PinnedOracleEvidence {
        implementation_id: VERTICAL_W_ORACLE_IMPLEMENTATION_ID.to_string(),
        revision: VERTICAL_ORACLE_REVISION.to_string(),
        executable_sha256: VERTICAL_W_ORACLE_BINARY_SHA256.to_string(),
        output_sha256: VERTICAL_W_ORACLE_OUTPUT_SHA256.to_string(),
    }
}
