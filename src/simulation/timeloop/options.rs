//! Existing cached environment switches and profiling conversion.

use std::sync::OnceLock;
use std::time::Duration;

pub(super) fn is_profiling_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("FLEXPART_GPU_PROFILE")
            .map(|v| v == "1")
            .unwrap_or(false)
    })
}

/// Returns `true` when PBL diagnostics should be computed on GPU (default).
///
/// Set `FLEXPART_GPU_PBL_CPU=1` to force the CPU fallback path.
pub(super) fn is_gpu_pbl_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !std::env::var("FLEXPART_GPU_PBL_CPU")
            .map(|v| v == "1")
            .unwrap_or(false)
    })
}

/// Returns `true` when the full multi-dispatch validation path is requested.
///
/// Set `FLEXPART_GPU_VALIDATION=1` to use separated Hanna → Langevin
/// dispatches (5 dispatches per step), suitable for debugging and
/// scientific validation. The default production path uses the fused
/// Hanna+Langevin kernel (4 dispatches: advection + fused H+L + dry dep + wet dep).
pub(super) fn is_validation_mode() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("FLEXPART_GPU_VALIDATION")
            .map(|v| v == "1")
            .unwrap_or(false)
    })
}

/// Returns `true` when active-particle compaction is enabled (O-07).
///
/// Set `FLEXPART_GPU_COMPACTION=1` to run a prefix-sum compaction + gather
/// after each physics step, packing active particles into contiguous leading
/// buffer slots. Subsequent dispatches then use `active_count` instead of
/// `particle_capacity` for workgroup sizing, avoiding wasted GPU threads
/// when deposition or domain exit deactivates particles.
///
/// Default: OFF (the initial benchmark has all particles active, so
/// compaction would add overhead with zero benefit).
pub(super) fn is_compaction_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("FLEXPART_GPU_COMPACTION")
            .map(|v| v == "1")
            .unwrap_or(false)
    })
}

pub(super) fn dur_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
