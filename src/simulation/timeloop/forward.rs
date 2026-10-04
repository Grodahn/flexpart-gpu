//! Forward run state, resource lifetime and public lifecycle facade.

use crate::config::ReleaseConfig;
use crate::coords::GridDomain;
use crate::gpu::{
    AdvectionDispatchKernel, AdvectionDualWindDispatchKernel, CompactionBuffers,
    CompactionPipelines, DecayDispatchKernel, DryDepositionDispatchKernel, DryDepositionIoBuffers,
    DualWindBuffers, GpuContext, HannaDispatchKernel, HannaParamsOutputBuffer,
    LangevinDispatchKernel, LangevinFusedDispatchKernel, ParticleBuffers, PblBuffers,
    PblDiagnosticsDispatchKernel, SurfaceFieldBuffer, WetDepositionDispatchKernel,
    WetDepositionIoBuffers, WindBuffers, WindSamplingPath,
};
use crate::io::GribPrefetchHandle;
use crate::particles::{ParticleStore, MAX_SPECIES};
use crate::physics::PhiloxCounter;
use crate::release::ReleaseManager;
use crate::simulation::timeloop::config::{validate_config, ForwardTimeLoopConfig};
use crate::simulation::timeloop::error::TimeLoopError;
use crate::simulation::timeloop::forcing::ForwardStepForcing;
use crate::simulation::timeloop::meteorology::MetTimeBracket;
use crate::simulation::timeloop::options::{is_compaction_enabled, is_validation_mode};
use crate::simulation::timeloop::reports::ForwardStepReport;

mod forcing;
mod meteorology;
mod operators;
mod output;
mod particles;
mod timestep;

/// Forward-mode integration driver orchestrating per-timestep GPU dispatch.
///
/// All GPU buffers and dispatch kernels are pre-allocated at construction time
/// (O-06) based on the known `particle_capacity` and grid dimensions. This
/// eliminates per-step `ensure_*` lazy-init checks.
pub struct ForwardTimeLoopDriver {
    config: ForwardTimeLoopConfig,
    current_time_seconds: i64,
    end_time_seconds: i64,
    step_index: usize,
    philox_counter: PhiloxCounter,
    release_manager: ReleaseManager,
    particle_store: ParticleStore,
    gpu_context: GpuContext,
    particle_buffers: ParticleBuffers,
    /// Single-wind buffers (legacy path, kept for backward compatibility).
    wind_buffers: Option<WindBuffers>,
    advection_dispatch_kernel: Option<AdvectionDispatchKernel>,
    /// Dual-time wind buffers: t0 and t1 uploaded once per met bracket,
    /// GPU interpolates inline using `alpha` (O-02 / Tier 1.2).
    /// Remains `Option` because the 3-D wind grid shape is only known at
    /// the first met bracket upload.
    dual_wind_buffers: Option<DualWindBuffers>,
    /// Pre-allocated dual-wind advection kernel; sampling path resolved from
    /// GPU capabilities at construction time.
    dual_wind_dispatch_kernel: AdvectionDualWindDispatchKernel,
    /// Met bracket tracking: lower bound [s since epoch].
    current_met_t0_seconds: Option<i64>,
    /// Met bracket tracking: upper bound [s since epoch].
    current_met_t1_seconds: Option<i64>,
    /// PBL diagnostics compute kernel (O-04 GPU PBL path).
    pbl_dispatch_kernel: PblDiagnosticsDispatchKernel,
    /// GPU buffer for packed surface meteorological fields (O-04 GPU PBL path).
    surface_field_buffer: SurfaceFieldBuffer,
    /// Double-buffered PBL state (ping-pong A/B, O-03 pipeline overlap).
    /// While the GPU reads from one slot, the CPU can upload the next
    /// step's PBL to the other slot without a data hazard.
    pbl_buffers: [PblBuffers; 2],
    /// Index into `pbl_buffers` for the current CPU write target (alternates 0 ↔ 1).
    pbl_write_index: usize,
    /// Whether a GPU submission is in-flight and has not yet been waited on.
    /// Set `true` after `queue.submit()`; cleared by `device.poll(Wait)` or
    /// by an async readback that internally polls.
    gpu_submission_pending: bool,
    /// `true` when running the full multi-dispatch validation path
    /// (`FLEXPART_GPU_VALIDATION=1`). Uses separated Hanna → Langevin
    /// dispatches so intermediate buffers can be inspected.
    /// Production path uses the fused Hanna+Langevin kernel instead.
    validation_mode: bool,
    /// Fused Hanna+Langevin dispatch kernel (production path).
    /// `None` in validation mode.
    langevin_fused_dispatch_kernel: Option<LangevinFusedDispatchKernel>,
    /// Intermediate Hanna output buffer (validation path only).
    hanna_params_output: Option<HannaParamsOutputBuffer>,
    /// Separated Hanna kernel (validation path only).
    hanna_dispatch_kernel: Option<HannaDispatchKernel>,
    /// Separated Langevin kernel (validation path only).
    langevin_dispatch_kernel: Option<LangevinDispatchKernel>,
    /// Dry deposition IO buffers (both production and validation paths).
    dry_deposition_io: DryDepositionIoBuffers,
    /// Dry deposition kernel (both production and validation paths).
    dry_deposition_dispatch_kernel: DryDepositionDispatchKernel,
    /// Wet deposition IO buffers (both production and validation paths).
    wet_deposition_io: WetDepositionIoBuffers,
    /// Wet deposition kernel (both production and validation paths).
    wet_deposition_dispatch_kernel: WetDepositionDispatchKernel,
    /// Radioactive decay kernel (both production and validation paths).
    decay_dispatch_kernel: DecayDispatchKernel,
    /// Reusable CPU-side scratch buffer for interleaved per-species dry
    /// deposition forcing (`slot * MAX_SPECIES + lane` layout).
    dry_forcing_scratch: Vec<f32>,
    /// Reusable CPU-side scratch buffer for interleaved per-species wet
    /// scavenging forcing.
    wet_scavenging_scratch: Vec<f32>,
    /// Reusable CPU-side scratch buffer for wet precipitating fraction forcing.
    wet_fraction_scratch: Vec<f32>,
    /// Last uploaded uniform dry deposition lanes, if all-species uniform.
    dry_uniform_cached: Option<[f32; MAX_SPECIES]>,
    /// Last uploaded uniform wet scavenging lanes, if all-species uniform.
    wet_scavenging_uniform_cached: Option<[f32; MAX_SPECIES]>,
    /// Last uploaded uniform wet precipitating fraction, if any.
    wet_fraction_uniform_cached: Option<f32>,
    /// Active GRIB prefetch handle, if a background read is in flight.
    grib_prefetch: Option<GribPrefetchHandle>,
    /// Whether active-particle compaction is enabled (O-07).
    /// Controlled by `FLEXPART_GPU_COMPACTION=1`.
    use_compaction: bool,
    /// Pre-allocated compaction pipelines (prefix-sum + gather/reorder).
    /// `None` when compaction is disabled.
    compaction_pipelines: Option<CompactionPipelines>,
    /// Pre-allocated compaction buffers (sized to `particle_capacity`).
    /// `None` when compaction is disabled.
    compaction_buffers: Option<CompactionBuffers>,
}

impl ForwardTimeLoopDriver {
    /// Create a new forward timeloop driver with empty particle store.
    ///
    /// All GPU buffers and dispatch kernels are pre-allocated here based on
    /// `particle_capacity` and `release_grid` dimensions (O-06). This avoids
    /// per-step lazy-init checks in the hot loop.
    pub async fn new(
        config: ForwardTimeLoopConfig,
        releases: &[ReleaseConfig],
        release_grid: GridDomain,
        particle_capacity: usize,
    ) -> Result<Self, TimeLoopError> {
        let (start_time_seconds, end_time_seconds) = validate_config(&config)?;
        let initial_philox_counter = config.initial_philox_counter;
        let met_grid_shape = (release_grid.nx, release_grid.ny);
        let release_manager = ReleaseManager::new(releases, release_grid)?;
        let particle_store = ParticleStore::with_capacity(particle_capacity);
        let gpu_context = GpuContext::new().await?;
        let particle_buffers = ParticleBuffers::from_store(&gpu_context, &particle_store);

        let validation_mode = is_validation_mode();

        let dual_wind_sampling_path = if gpu_context.supports_wind_texture_sampling() {
            WindSamplingPath::SampledTexture3d
        } else {
            WindSamplingPath::BufferStorage
        };
        let dual_wind_dispatch_kernel =
            AdvectionDualWindDispatchKernel::new(&gpu_context, dual_wind_sampling_path);

        let pbl_dispatch_kernel = PblDiagnosticsDispatchKernel::new(&gpu_context);
        let surface_field_buffer = SurfaceFieldBuffer::with_shape(&gpu_context, met_grid_shape);
        let pbl_placeholder_a = crate::pbl::PblState::new(met_grid_shape.0, met_grid_shape.1);
        let pbl_placeholder_b = crate::pbl::PblState::new(met_grid_shape.0, met_grid_shape.1);
        let pbl_buffers = [
            PblBuffers::from_state(&gpu_context, &pbl_placeholder_a)?,
            PblBuffers::from_state(&gpu_context, &pbl_placeholder_b)?,
        ];

        let langevin_fused_dispatch_kernel = if validation_mode {
            None
        } else {
            Some(LangevinFusedDispatchKernel::new(&gpu_context))
        };

        let (hanna_params_output, hanna_dispatch_kernel, langevin_dispatch_kernel) =
            if validation_mode {
                (
                    Some(HannaParamsOutputBuffer::new(
                        &gpu_context,
                        particle_capacity,
                    )?),
                    Some(HannaDispatchKernel::new(&gpu_context)),
                    Some(LangevinDispatchKernel::new(&gpu_context)),
                )
            } else {
                (None, None, None)
            };

        let zeros = vec![0.0_f32; particle_capacity];
        let zeros_species = vec![[0.0_f32; MAX_SPECIES]; particle_capacity];
        let dry_deposition_io =
            DryDepositionIoBuffers::from_species_velocities(&gpu_context, &zeros_species)?;
        let dry_deposition_dispatch_kernel = DryDepositionDispatchKernel::new(&gpu_context);
        let wet_deposition_io =
            WetDepositionIoBuffers::from_inputs(&gpu_context, &zeros_species, &zeros)?;
        let wet_deposition_dispatch_kernel = WetDepositionDispatchKernel::new(&gpu_context);

        let use_compaction = is_compaction_enabled();
        let (compaction_pipelines, compaction_buffers) = if use_compaction {
            let pipelines = CompactionPipelines::new(&gpu_context);
            let buffers = CompactionBuffers::new(&gpu_context, particle_capacity)?;
            (Some(pipelines), Some(buffers))
        } else {
            (None, None)
        };

        Ok(Self {
            config,
            current_time_seconds: start_time_seconds,
            end_time_seconds,
            step_index: 0,
            philox_counter: initial_philox_counter,
            release_manager,
            particle_store,
            decay_dispatch_kernel: DecayDispatchKernel::new(&gpu_context),
            gpu_context,
            particle_buffers,
            wind_buffers: None,
            advection_dispatch_kernel: None,
            dual_wind_buffers: None,
            dual_wind_dispatch_kernel,
            current_met_t0_seconds: None,
            current_met_t1_seconds: None,
            pbl_dispatch_kernel,
            surface_field_buffer,
            pbl_buffers,
            pbl_write_index: 0,
            gpu_submission_pending: false,
            validation_mode,
            langevin_fused_dispatch_kernel,
            hanna_params_output,
            hanna_dispatch_kernel,
            langevin_dispatch_kernel,
            dry_deposition_io,
            dry_deposition_dispatch_kernel,
            wet_deposition_io,
            wet_deposition_dispatch_kernel,
            dry_forcing_scratch: Vec::with_capacity(particle_capacity),
            wet_scavenging_scratch: Vec::with_capacity(particle_capacity),
            wet_fraction_scratch: Vec::with_capacity(particle_capacity),
            dry_uniform_cached: None,
            wet_scavenging_uniform_cached: None,
            wet_fraction_uniform_cached: None,
            grib_prefetch: None,
            use_compaction,
            compaction_pipelines,
            compaction_buffers,
        })
    }

    /// Returns `true` while at least one timestep remains.
    #[must_use]
    pub fn has_remaining_steps(&self) -> bool {
        self.current_time_seconds <= self.end_time_seconds
    }

    /// Current simulation time [s since epoch].
    #[must_use]
    pub fn current_time_seconds(&self) -> i64 {
        self.current_time_seconds
    }

    /// Inclusive configured end time [s since epoch].
    #[must_use]
    pub fn end_time_seconds(&self) -> i64 {
        self.end_time_seconds
    }

    /// Read-only access to host-side particle storage.
    #[must_use]
    pub fn particle_store(&self) -> &ParticleStore {
        &self.particle_store
    }

    /// Consume the driver and return the host-side particle storage.
    #[must_use]
    pub fn into_particle_store(self) -> ParticleStore {
        self.particle_store
    }

    /// Run until `end_timestamp` using fixed forcing and one met bracket.
    ///
    /// Calls [`finalize`](Self::finalize) after the last step to drain any
    /// pending GPU submission.
    pub async fn run_to_end(
        &mut self,
        met: &MetTimeBracket<'_>,
        forcing: &ForwardStepForcing,
    ) -> Result<Vec<ForwardStepReport>, TimeLoopError> {
        let mut reports = Vec::new();
        while self.has_remaining_steps() {
            reports.push(self.run_timestep(met, forcing).await?);
        }
        self.finalize().await?;
        Ok(reports)
    }

    /// Drain the last pending GPU submission (if any).
    ///
    /// Must be called after the final [`run_timestep`](Self::run_timestep)
    /// when the caller drives the loop manually. [`run_to_end`](Self::run_to_end)
    /// calls this automatically. Safe to call multiple times.
    pub async fn finalize(&mut self) -> Result<(), TimeLoopError> {
        if self.gpu_submission_pending {
            self.gpu_context.device.poll(wgpu::Maintain::Wait);
            self.gpu_submission_pending = false;
        }
        Ok(())
    }
}
