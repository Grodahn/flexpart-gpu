//! Vertical GPU failures and the unchanged device error-scope boundary.
use crate::gpu::{GpuBufferError, GpuEvidenceError};
use crate::meteorology::{
    vertical::VerticalTransformError, vertical_sampling::VerticalSamplingError,
};
use thiserror::Error;

/// Errors for GPU vertical sampling.
#[derive(Debug, Error)]
pub enum GpuVerticalError {
    /// Canonical #73 sampling validation rejected the request.
    #[error(transparent)]
    Sampling(#[from] VerticalSamplingError),
    /// Canonical #30 runtime access failed.
    #[error(transparent)]
    Runtime(#[from] VerticalTransformError),
    /// Buffer transfer or readback failed.
    #[error(transparent)]
    Buffer(#[from] GpuBufferError),
    /// Evidence construction or validation failed.
    #[error(transparent)]
    Evidence(#[from] GpuEvidenceError),
    /// No queries were supplied; an empty comparison cannot prove parity.
    #[error("vertical GPU query sequence is empty")]
    EmptyQueries,
    /// A value does not fit in `u32`.
    #[error("value for {field} does not fit in u32: {value}")]
    ValueTooLarge {
        /// Name of the oversized value.
        field: &'static str,
        /// Offending value.
        value: usize,
    },
    /// Byte-size overflow while sizing a buffer.
    #[error("byte-size overflow while preparing {field}")]
    SizeOverflow {
        /// Name of the buffer being sized.
        field: &'static str,
    },
    /// Grid and query resources describe different counts.
    #[error("count mismatch for {field}: grids {grids}, queries {queries}, outputs {outputs}")]
    CountMismatch {
        /// Resource that mismatched.
        field: &'static str,
        /// Grid level count.
        grids: usize,
        /// Query count.
        queries: usize,
        /// Output count.
        outputs: usize,
    },
    /// Interface and level resources describe inconsistent level counts.
    #[error("level count mismatch for {field}: interfaces {interfaces}, levels {levels}, shared {shared}")]
    LevelCountMismatch {
        /// Resource that mismatched.
        field: &'static str,
        /// Interface count.
        interfaces: usize,
        /// Level count.
        levels: usize,
        /// Shared grid count.
        shared: usize,
    },
    /// GPU device scope reported an error.
    #[error("GPU {scope} error during vertical sampling: {message}")]
    Device {
        /// Scope that failed.
        scope: &'static str,
        /// Device error text.
        message: String,
    },
    /// A validated value is not finite as `f32`.
    #[error("validated vertical value for {field} is not finite as f32")]
    NonFiniteDeviceValue {
        /// Name of the offending lane.
        field: &'static str,
    },
    /// GPU output is non-finite where the oracle is finite.
    #[error("non-finite GPU vertical output at query {query_index}")]
    NonFiniteOutputValue {
        /// Index of the offending query.
        query_index: usize,
    },
    /// Candidate revision must be a 40-character lowercase Git SHA.
    #[error("candidate revision must be a 40-character lowercase Git SHA, got {value}")]
    InvalidCandidateRevision {
        /// Offending revision.
        value: String,
    },
    /// Failed to hash candidate inputs.
    #[error("failed to hash candidate inputs: {message}")]
    InputHash {
        /// Failure detail.
        message: String,
    },
    /// Request does not match the pinned vertical oracle contract.
    #[error("request does not match the pinned vertical oracle contract: {message}")]
    OracleContract {
        /// Failure detail.
        message: &'static str,
    },
}

pub(super) fn push_device_error_scopes(device: &wgpu::Device) {
    device.push_error_scope(wgpu::ErrorFilter::Internal);
    device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    device.push_error_scope(wgpu::ErrorFilter::Validation);
}

pub(super) fn pop_device_error_scopes(
    device: &wgpu::Device,
    scope: &'static str,
) -> Result<(), GpuVerticalError> {
    let validation = pollster::block_on(device.pop_error_scope());
    let out_of_memory = pollster::block_on(device.pop_error_scope());
    let internal = pollster::block_on(device.pop_error_scope());
    if let Some(error) = [validation, out_of_memory, internal]
        .into_iter()
        .flatten()
        .next()
    {
        return Err(GpuVerticalError::Device {
            scope,
            message: error.to_string(),
        });
    }
    Ok(())
}
