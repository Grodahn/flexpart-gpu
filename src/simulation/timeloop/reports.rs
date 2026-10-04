//! Host-visible timestep and timing reports.

use crate::particles::MAX_SPECIES;
use crate::physics::PhiloxCounter;
use crate::simulation::timeloop::config::BackwardSourceCollection;
use std::collections::BTreeMap;

/// Result of one orchestrated forward timestep.
#[derive(Debug, Clone)]
pub struct ForwardStepReport {
    /// Zero-based step index in this run.
    pub step_index: usize,
    /// Timestamp at which this step was evaluated (`YYYYMMDDHHMMSS`).
    pub timestamp: String,
    /// Simulation time [s since epoch] for this step.
    pub simulation_time_seconds: i64,
    /// Temporal interpolation weight in `[0, 1]` (after clamp/strict handling).
    pub interpolation_alpha: f32,
    /// Number of particles released during this step.
    pub released_count: usize,
    /// Particle slots injected by release manager for this step.
    pub released_slots: Vec<usize>,
    /// Active particle count after GPU readback.
    pub active_particle_count: usize,
    /// Dry-deposition probability per slot per species (GPU output).
    pub dry_deposition_probability: Vec<[f32; MAX_SPECIES]>,
    /// Wet-deposition probability per slot per species (GPU output).
    pub wet_deposition_probability: Vec<[f32; MAX_SPECIES]>,
    /// Philox counter after the Langevin update.
    pub next_philox_counter: PhiloxCounter,
    /// Per-section timing breakdown (present only when `FLEXPART_GPU_PROFILE=1`).
    pub timing: Option<StepTimingReport>,
}

/// Per-section wall-clock timing for one forward timestep.
///
/// Populated only when `FLEXPART_GPU_PROFILE=1` is set. Each field records
/// the wall-clock duration of one pipeline stage in milliseconds. Used by
/// Phase 0 profiling to measure the actual CPU/GPU split before optimizing.
#[derive(Debug, Clone)]
pub struct StepTimingReport {
    /// `interpolate_wind_field_linear` (CPU).
    pub wind_interp_ms: f64,
    /// `interpolate_surface_fields_linear` (CPU).
    pub surf_interp_ms: f64,
    /// `compute_pbl_parameters_from_met` (CPU).
    pub pbl_ms: f64,
    /// `ensure_wind_buffers` — upload wind field to GPU.
    pub wind_upload_ms: f64,
    /// `ensure_pbl_buffers` — upload PBL state to GPU.
    pub pbl_upload_ms: f64,
    /// `materialize()` calls for dry/wet deposition forcing vectors (CPU alloc+fill).
    pub forcing_ms: f64,
    /// `ensure_dry_deposition_io_buffers` + `ensure_wet_deposition_io_buffers`.
    pub dep_upload_ms: f64,
    /// Blocking wait for the *previous* step's GPU submission to complete
    /// (O-03 pipeline overlap). Zero on the first step or when no work is
    /// pending.
    pub wait_prev_gpu_ms: f64,
    /// CPU-side command encoding: `create_command_encoder` through `queue.submit()`.
    pub gpu_encode_ms: f64,
    /// GPU execution: `queue.submit()` to `device.poll(Wait)` completion.
    pub gpu_exec_ms: f64,
    /// Compaction encode + submit + readback (O-07). Zero when compaction
    /// is disabled or all particles are active.
    pub compaction_ms: f64,
    /// Wall-clock for the entire `run_timestep` call.
    pub total_ms: f64,
}

impl StepTimingReport {
    pub(super) fn print_summary(&self, step_index: usize) {
        eprintln!(
            "[profile] step={step_index} \
             wind_interp={:.1}ms surf_interp={:.1}ms pbl={:.1}ms \
             wind_upload={:.1}ms pbl_upload={:.1}ms forcing={:.1}ms \
             dep_upload={:.1}ms wait_prev={:.1}ms \
             gpu_encode={:.1}ms gpu_exec={:.1}ms \
             compaction={:.1}ms \
             total={:.1}ms",
            self.wind_interp_ms,
            self.surf_interp_ms,
            self.pbl_ms,
            self.wind_upload_ms,
            self.pbl_upload_ms,
            self.forcing_ms,
            self.dep_upload_ms,
            self.wait_prev_gpu_ms,
            self.gpu_encode_ms,
            self.gpu_exec_ms,
            self.compaction_ms,
            self.total_ms,
        );
    }
}

/// Result of one orchestrated backward timestep.
#[derive(Debug, Clone, PartialEq)]
pub struct BackwardStepReport {
    /// Zero-based step index in this run.
    pub step_index: usize,
    /// Timestamp at which this step was evaluated (`YYYYMMDDHHMMSS`).
    pub timestamp: String,
    /// Simulation time [s since epoch] for this step.
    pub simulation_time_seconds: i64,
    /// Temporal interpolation weight in `[0, 1]` (after clamp/strict handling).
    pub interpolation_alpha: f32,
    /// Number of particles released from receptors during this step.
    pub released_count: usize,
    /// Particle slots injected for this step.
    pub released_slots: Vec<usize>,
    /// Active particle count after GPU readback.
    pub active_particle_count: usize,
    /// Per-source collection summary after this step.
    pub source_collections: BTreeMap<String, BackwardSourceCollection>,
    /// Philox counter after the Langevin update.
    pub next_philox_counter: PhiloxCounter,
}
