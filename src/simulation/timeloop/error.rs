//! Driver errors preserve the existing propagation boundary.

use crate::gpu::{
    GpuAdvectionError, GpuBufferError, GpuCompactionError, GpuConcentrationGriddingError,
    GpuDecayError, GpuDryDepositionError, GpuError, GpuHannaError, GpuLangevinError,
    GpuLangevinFusedError, GpuPblDiagnosticsError, GpuPblReflectionError, GpuWetDepositionError,
};
use crate::io::{PblParameterError, TemporalInterpolationError};
use crate::particles::ParticleSortError;
use crate::release::ReleaseError;
use thiserror::Error;

/// Errors produced by forward time-loop orchestration.
#[derive(Debug, Error)]
pub enum TimeLoopError {
    /// Canonical meteorology preflight/preparation failed before physics submission.
    #[error("canonical meteorology preparation failed: {0}")]
    CanonicalMeteorology(#[from] crate::gpu::meteorology::MeteorologyCompositionError),
    #[error("invalid timestamp `{value}`: expected 14 digits YYYYMMDDHHMMSS")]
    InvalidTimestamp { value: String },
    #[error("timestamp out of representable range: seconds={seconds}")]
    TimestampOutOfRange { seconds: i64 },
    #[error("invalid config: start timestamp `{start}` must be <= end timestamp `{end}`")]
    InvalidTimeRange { start: String, end: String },
    #[error("invalid backward config: start timestamp `{start}` must be >= end timestamp `{end}`")]
    InvalidBackwardTimeRange { start: String, end: String },
    #[error("invalid config timestep_seconds: {value} (must be > 0)")]
    InvalidTimestep { value: i64 },
    #[error("invalid spatial sort interval_steps: {value} (must be > 0)")]
    InvalidSpatialSortInterval { value: usize },
    #[error(
        "invalid config dry_reference_height_m: {value} (must be finite and strictly positive)"
    )]
    InvalidDryReferenceHeight { value: f32 },
    #[error("invalid config langevin_vertical_substeps: {value} (must be in 1..=4)")]
    InvalidLangevinSubsteps { value: u32 },
    #[error(
        "invalid config langevin_min_height_m: {value} (must be finite and strictly positive)"
    )]
    InvalidLangevinMinHeight { value: f32 },
    #[error("forward time-loop has reached end time; no remaining steps")]
    SimulationComplete,
    #[error("forcing length mismatch for `{field}`: expected {expected}, got {actual}")]
    ForcingLengthMismatch {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("backward config requires at least one receptor release")]
    MissingReceptors,
    #[error(
        "invalid receptor `{name}`: {field}={value} ({reason}); expected physically valid bounds"
    )]
    InvalidReceptor {
        name: String,
        field: &'static str,
        value: f64,
        reason: &'static str,
    },
    #[error("invalid source region `{name}` bounds for `{field}`: min {min} > max {max}")]
    InvalidSourceBounds {
        name: String,
        field: &'static str,
        min: f64,
        max: f64,
    },
    #[error("temporal interpolation failed: {0}")]
    Temporal(#[from] TemporalInterpolationError),
    #[error("PBL parameter computation failed: {0}")]
    Pbl(#[from] PblParameterError),
    #[error("particle release failed: {0}")]
    Release(#[from] ReleaseError),
    #[error("particle spatial sorting failed: {0}")]
    ParticleSort(#[from] ParticleSortError),
    #[error("GPU initialization failed: {0}")]
    Gpu(#[from] GpuError),
    #[error("GPU buffer operation failed: {0}")]
    GpuBuffer(#[from] GpuBufferError),
    #[error("GPU advection dispatch failed: {0}")]
    GpuAdvection(#[from] GpuAdvectionError),
    #[error("GPU Hanna dispatch failed: {0}")]
    GpuHanna(#[from] GpuHannaError),
    #[error("GPU Langevin dispatch failed: {0}")]
    GpuLangevin(#[from] GpuLangevinError),
    #[error("GPU PBL diagnostics dispatch failed: {0}")]
    GpuPblDiagnostics(#[from] GpuPblDiagnosticsError),
    #[error("GPU PBL reflection dispatch failed: {0}")]
    GpuPblReflection(#[from] GpuPblReflectionError),
    #[error("GPU dry deposition dispatch failed: {0}")]
    GpuDryDeposition(#[from] GpuDryDepositionError),
    #[error("GPU decay dispatch failed: {0}")]
    GpuDecay(#[from] GpuDecayError),
    #[error("GPU wet deposition dispatch failed: {0}")]
    GpuWetDeposition(#[from] GpuWetDepositionError),
    #[error("GPU concentration gridding failed: {0}")]
    GpuConcentrationGridding(#[from] GpuConcentrationGriddingError),
    #[error("GPU compaction failed: {0}")]
    GpuCompaction(#[from] GpuCompactionError),
    #[error("GPU fused Hanna+Langevin dispatch failed: {0}")]
    GpuLangevinFused(#[from] GpuLangevinFusedError),
}
