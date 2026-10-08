//! Backward receptor lifecycle, device steps and source attribution.

use crate::config::ReleaseConfig;
use crate::coords::GridDomain;
use crate::gpu::advection_resident::{ResidentAdvectionKernels, ResidentAdvectionStep};
use crate::gpu::{
    encode_decay_gpu_with_kernel, encode_dry_deposition_probability_gpu_with_kernel,
    encode_hanna_params_gpu_with_kernel, encode_pbl_diagnostics_gpu_with_kernel,
    encode_update_particles_turbulence_langevin_gpu_with_hanna_output_and_kernel,
    encode_wet_deposition_probability_gpu_with_kernel, AdvectionDualWindDispatchKernel,
    DecayDispatchKernel, DecayStepParams, DryDepositionDispatchKernel, DryDepositionIoBuffers,
    DryDepositionStepParams, DualWindBuffers, GpuContext, HannaDispatchKernel,
    HannaParamsOutputBuffer, LangevinDispatchKernel, ParticleBuffers, PblBuffers,
    PblDiagnosticsDispatchKernel, SurfaceFieldBuffer, WetDepositionDispatchKernel,
    WetDepositionIoBuffers, WetDepositionStepParams, WindSamplingPath,
};
use crate::io::{
    compute_pbl_parameters_from_met, interpolate_surface_fields_linear, PblMetInputGrids,
};
use crate::particles::{ParticleStore, MAX_SPECIES};
use crate::physics::{LangevinStep, PhiloxCounter};
use crate::release::ReleaseManager;
use crate::simulation::timeloop::config::{
    validate_backward_config, BackwardReceptorConfig, BackwardSourceCollection,
    BackwardSourceRegionConfig, BackwardTimeLoopConfig, TimeDirection,
};
use crate::simulation::timeloop::error::TimeLoopError;
use crate::simulation::timeloop::forcing::{
    cast_species_lanes, materialize_species_forcing_into, ForwardStepForcing,
};
use crate::simulation::timeloop::meteorology::{
    CanonicalMeteorologyBracket, CanonicalMeteorologySlot, MetTimeBracket,
    PreparedCanonicalMeteorology,
};
use crate::simulation::timeloop::options::is_gpu_pbl_enabled;
use crate::simulation::timeloop::reports::BackwardStepReport;
use crate::simulation::timeloop::time::{
    format_timestamp_seconds, interpolation_alpha, timestep_seconds_f32,
};
use std::collections::BTreeMap;

/// Backward-mode integration driver (`ldirect = -1`) for receptor attribution.
///
/// Always uses the multi-dispatch path (separated Hanna, Langevin, dry/wet
/// deposition) which is the same as the forward driver's validation mode.
/// This gives full per-stage inspectability for scientific analysis.
///
/// Optimizations applied:
/// - Dual-wind bracket tracking (O-02): `wind_t0`/`t1` uploaded once per met
///   bracket, GPU interpolates inline.
/// - GPU PBL diagnostics (O-04): surface fields uploaded, PBL computed on
///   GPU. Falls back to CPU when `FLEXPART_GPU_PBL_CPU=1`.
/// - Batched command encoder: all dispatches in a single `queue.submit()`.
/// - Zero-forcing skip: deposition dispatches skipped when forcing is zero.
///
/// No pipeline overlap or compaction — backward mode always synchronizes
/// every step for source attribution readback.
pub struct BackwardTimeLoopDriver {
    config: BackwardTimeLoopConfig,
    current_time_seconds: i64,
    end_time_seconds: i64,
    step_index: usize,
    philox_counter: PhiloxCounter,
    release_grid: GridDomain,
    release_manager: ReleaseManager,
    particle_store: ParticleStore,
    gpu_context: GpuContext,
    particle_buffers: ParticleBuffers,
    staged_particles: ParticleBuffers,
    resident_advection_kernels: ResidentAdvectionKernels,
    #[cfg(test)]
    latest_advection: Option<ResidentAdvectionStep>,
    /// Dual-time wind buffers (O-02): t0 and t1 uploaded once per met bracket.
    /// `None` until the first bracket upload (3-D grid shape unknown before).
    dual_wind_buffers: Option<DualWindBuffers>,
    dual_wind_dispatch_kernel: AdvectionDualWindDispatchKernel,
    current_met_t0_seconds: Option<i64>,
    current_met_t1_seconds: Option<i64>,
    /// GPU PBL diagnostics kernel (O-04).
    pbl_dispatch_kernel: PblDiagnosticsDispatchKernel,
    /// GPU buffer for packed surface meteorological fields (O-04).
    surface_field_buffer: SurfaceFieldBuffer,
    /// Single PBL buffer (no double-buffering needed — backward syncs every step).
    pbl_buffers: PblBuffers,
    hanna_params_output: HannaParamsOutputBuffer,
    hanna_dispatch_kernel: HannaDispatchKernel,
    langevin_dispatch_kernel: LangevinDispatchKernel,
    dry_deposition_io: DryDepositionIoBuffers,
    dry_deposition_dispatch_kernel: DryDepositionDispatchKernel,
    wet_deposition_io: WetDepositionIoBuffers,
    wet_deposition_dispatch_kernel: WetDepositionDispatchKernel,
    /// Radioactive decay kernel.
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
}

impl BackwardTimeLoopDriver {
    #[cfg(test)]
    pub(crate) fn test_set_active_prefix(&mut self, count: usize) {
        for particle in self.particle_store.as_mut_slice().iter_mut().skip(count) {
            particle.time = -123;
            particle.vel_u = 42.0;
        }
        self.particle_buffers
            .upload_store(&self.gpu_context, &self.particle_store)
            .expect("test inactive-tail upload");
        self.particle_buffers.set_dispatch_count(count);
    }

    #[cfg(test)]
    pub(crate) async fn test_advection_state(
        &self,
    ) -> (
        Vec<crate::particles::Particle>,
        Vec<u32>,
        Vec<crate::gpu::meteorology::resident::ResidentSampleMetadata>,
        Vec<f32>,
        Vec<crate::gpu::meteorology::resident::ResidentQueryLane>,
    ) {
        let step = self
            .latest_advection
            .as_ref()
            .expect("production step encoded");
        let status = crate::gpu::download_buffer_typed::<u32>(
            &self.gpu_context,
            &step.status,
            6 * (self.particle_buffers.capacity() + 1) + 1,
            "test final Petterssen status",
        )
        .await
        .expect("status readback");
        (
            self.particle_buffers
                .download_particles(&self.gpu_context)
                .await
                .expect("final particles"),
            status,
            step.metadata.clone(),
            crate::gpu::download_buffer_typed::<f32>(&self.gpu_context, &step.values,6*self.particle_buffers.capacity(),"test final sampled values").await.unwrap(),
            crate::gpu::download_buffer_typed::<crate::gpu::meteorology::resident::ResidentQueryLane>(&self.gpu_context,&step.queries,2*self.particle_buffers.capacity(),"test final queries").await.unwrap(),
        )
    }

    /// Prepare validated canonical U/V/center-W for the current backward step and -dt.
    ///
    /// This is the explicit canonical preparation boundary for #112. It neither
    /// advances particles nor calls the legacy advection operator. Keep `slot`
    /// across steps and retain returned resource owners through their last consumer.
    /// # Errors
    /// Rejects completed simulations, invalid coverage, device mixing and upload errors.
    pub fn prepare_canonical_meteorology<'step, 'source>(
        &'step self,
        bracket: CanonicalMeteorologyBracket<'source>,
        slot: &mut CanonicalMeteorologySlot<'source>,
    ) -> Result<PreparedCanonicalMeteorology<'step, 'source>, TimeLoopError> {
        if !self.has_remaining_steps() {
            return Err(TimeLoopError::SimulationComplete);
        }
        Ok(slot.prepare(
            &self.gpu_context,
            bracket,
            self.current_time_seconds,
            -self.config.timestep_seconds,
        )?)
    }

    /// Create a new backward timeloop driver with empty particle store.
    ///
    /// All GPU buffers and dispatch kernels are pre-allocated here based on
    /// `particle_capacity` and `release_grid` dimensions, following the
    /// forward driver pattern (O-06).
    pub async fn new(
        config: BackwardTimeLoopConfig,
        release_grid: GridDomain,
        particle_capacity: usize,
    ) -> Result<Self, TimeLoopError> {
        let (start_time_seconds, end_time_seconds) = validate_backward_config(&config)?;
        let initial_philox_counter = config.initial_philox_counter;
        let met_grid_shape = (release_grid.nx, release_grid.ny);
        let release_manager = ReleaseManager::new(
            &build_receptor_release_configs(&config.receptors, &config.start_timestamp),
            release_grid.clone(),
        )?;
        let particle_store = ParticleStore::with_capacity(particle_capacity);
        let gpu_context = GpuContext::new().await?;
        let particle_buffers = ParticleBuffers::from_store(&gpu_context, &particle_store);
        let staged_particles = ParticleBuffers::from_store(&gpu_context, &particle_store);
        let resident_advection_kernels = ResidentAdvectionKernels::new(&gpu_context)?;

        let dual_wind_sampling_path = if gpu_context.supports_wind_texture_sampling() {
            WindSamplingPath::SampledTexture3d
        } else {
            WindSamplingPath::BufferStorage
        };
        let dual_wind_dispatch_kernel =
            AdvectionDualWindDispatchKernel::new(&gpu_context, dual_wind_sampling_path);

        let pbl_dispatch_kernel = PblDiagnosticsDispatchKernel::new(&gpu_context);
        let surface_field_buffer = SurfaceFieldBuffer::with_shape(&gpu_context, met_grid_shape);
        let pbl_placeholder = crate::pbl::PblState::new(met_grid_shape.0, met_grid_shape.1);
        let pbl_buffers = PblBuffers::from_state(&gpu_context, &pbl_placeholder)?;

        let hanna_params_output = HannaParamsOutputBuffer::new(&gpu_context, particle_capacity)?;
        let hanna_dispatch_kernel = HannaDispatchKernel::new(&gpu_context);
        let langevin_dispatch_kernel = LangevinDispatchKernel::new(&gpu_context);

        let zeros = vec![0.0_f32; particle_capacity];
        let zeros_species = vec![[0.0_f32; MAX_SPECIES]; particle_capacity];
        let dry_deposition_io =
            DryDepositionIoBuffers::from_species_velocities(&gpu_context, &zeros_species)?;
        let dry_deposition_dispatch_kernel = DryDepositionDispatchKernel::new(&gpu_context);
        let wet_deposition_io =
            WetDepositionIoBuffers::from_inputs(&gpu_context, &zeros_species, &zeros)?;
        let wet_deposition_dispatch_kernel = WetDepositionDispatchKernel::new(&gpu_context);
        let decay_dispatch_kernel = DecayDispatchKernel::new(&gpu_context);

        Ok(Self {
            config,
            current_time_seconds: start_time_seconds,
            end_time_seconds,
            step_index: 0,
            philox_counter: initial_philox_counter,
            release_grid,
            release_manager,
            particle_store,
            gpu_context,
            particle_buffers,
            staged_particles,
            resident_advection_kernels,
            #[cfg(test)]
            latest_advection: None,
            dual_wind_buffers: None,
            dual_wind_dispatch_kernel,
            current_met_t0_seconds: None,
            current_met_t1_seconds: None,
            pbl_dispatch_kernel,
            surface_field_buffer,
            pbl_buffers,
            hanna_params_output,
            hanna_dispatch_kernel,
            langevin_dispatch_kernel,
            dry_deposition_io,
            dry_deposition_dispatch_kernel,
            wet_deposition_io,
            wet_deposition_dispatch_kernel,
            decay_dispatch_kernel,
            dry_forcing_scratch: Vec::with_capacity(particle_capacity),
            wet_scavenging_scratch: Vec::with_capacity(particle_capacity),
            wet_fraction_scratch: Vec::with_capacity(particle_capacity),
            dry_uniform_cached: None,
            wet_scavenging_uniform_cached: None,
            wet_fraction_uniform_cached: None,
        })
    }

    /// Returns `true` while at least one timestep remains.
    #[must_use]
    pub fn has_remaining_steps(&self) -> bool {
        self.current_time_seconds >= self.end_time_seconds
    }

    /// Current simulation time [s since epoch].
    #[must_use]
    pub fn current_time_seconds(&self) -> i64 {
        self.current_time_seconds
    }

    /// Read-only access to host-side particle storage.
    #[must_use]
    pub fn particle_store(&self) -> &ParticleStore {
        &self.particle_store
    }

    /// Consume the driver and return host-side particle storage.
    #[must_use]
    pub fn into_particle_store(self) -> ParticleStore {
        self.particle_store
    }

    /// Run one orchestrated backward timestep.
    ///
    /// All GPU dispatches are encoded into a single command encoder and
    /// submitted in one `queue.submit()` call. The driver always waits for
    /// GPU completion and downloads particle state, because source
    /// attribution needs host-side particle data.
    pub async fn run_timestep(
        &mut self,
        met: &MetTimeBracket<'_>,
        forcing: &ForwardStepForcing,
    ) -> Result<BackwardStepReport, TimeLoopError> {
        self.run_timestep_impl(met, forcing, false).await
    }

    /// Explicit legacy diagnostic only; never production or canonical-migration evidence.
    /// Retained for the existing independently owned synthetic/oracle diagnostics (#117).
    pub async fn run_legacy_diagnostic_timestep(
        &mut self,
        met: &MetTimeBracket<'_>,
        forcing: &ForwardStepForcing,
    ) -> Result<BackwardStepReport, TimeLoopError> {
        self.run_timestep_impl(met, forcing, true).await
    }

    async fn run_timestep_impl(
        &mut self,
        met: &MetTimeBracket<'_>,
        forcing: &ForwardStepForcing,
        legacy_diagnostic: bool,
    ) -> Result<BackwardStepReport, TimeLoopError> {
        if !self.has_remaining_steps() {
            return Err(TimeLoopError::SimulationComplete);
        }

        if !legacy_diagnostic && met.canonical.is_none() {
            return Err(crate::gpu::meteorology::MeteorologyCompositionError::Missing(
                "production advection requires #173 canonical meteorology; legacy-only provider inputs need #32").into());
        }
        if legacy_diagnostic && met.canonical.is_some() {
            return Err(
                crate::gpu::meteorology::MeteorologyCompositionError::Incompatible(
                    "canonical fields cannot enter the legacy diagnostic path",
                )
                .into(),
            );
        }
        if let Some(canonical) = met.canonical {
            let grid = canonical.bracket().horizontal_grid();
            let release = &self.release_grid;
            if grid.nx != release.nx
                || grid.ny != release.ny
                || grid.xlon0_deg != release.xlon0
                || grid.ylat0_deg != release.ylat0
                || grid.dx_deg != release.dx
                || grid.dy_deg != release.dy
            {
                return Err(
                    crate::gpu::meteorology::MeteorologyCompositionError::Incompatible(
                        "canonical meteorology and particle coordinate grids differ",
                    )
                    .into(),
                );
            }
        }
        let times = met
            .canonical
            .map(|canonical| {
                canonical
                    .bracket()
                    .resolve_step_times(self.current_time_seconds, -self.config.timestep_seconds)
            })
            .transpose()?;
        let timestamp = format_timestamp_seconds(self.current_time_seconds)?;
        let release_report = self.release_manager.inject_and_upload_for_time(
            &timestamp,
            &mut self.particle_store,
            &self.particle_buffers,
            &self.gpu_context,
        )?;

        let interpolation_alpha = interpolation_alpha(
            met.time_t0_seconds,
            met.time_t1_seconds,
            self.current_time_seconds,
            self.config.time_bounds_behavior,
        )?;

        // O-02: upload wind_t0/t1 once per met bracket.
        if legacy_diagnostic {
            self.upload_dual_wind_if_bracket_changed(met)?;
        }

        let interpolated_surface = interpolate_surface_fields_linear(
            met.surface_t0,
            met.surface_t1,
            met.time_t0_seconds,
            met.time_t1_seconds,
            self.current_time_seconds,
            self.config.time_bounds_behavior,
        )?;

        // O-04: GPU PBL by default; CPU fallback with FLEXPART_GPU_PBL_CPU=1.
        let use_gpu_pbl = is_gpu_pbl_enabled();
        if use_gpu_pbl {
            self.surface_field_buffer
                .upload(&self.gpu_context, &interpolated_surface)?;
        } else {
            let computed_pbl = compute_pbl_parameters_from_met(
                PblMetInputGrids {
                    surface: &interpolated_surface,
                    profile: None,
                },
                self.config.pbl_options,
            )?;
            self.pbl_buffers
                .upload_state(&self.gpu_context, &computed_pbl.pbl_state)?;
        }

        let dt_seconds = timestep_seconds_f32(self.config.timestep_seconds)?;

        forcing.check_species_shape()?;
        let skip_dry_deposition = forcing.skip_dry_deposition();
        let skip_wet_deposition = forcing.skip_wet_deposition();
        let skip_decay = forcing.skip_decay();

        let slot_count = self.particle_buffers.capacity();
        if !skip_dry_deposition {
            let dry_velocity = materialize_species_forcing_into(
                &forcing.dry_deposition_velocity_m_s,
                slot_count,
                "dry_deposition_velocity_m_s",
                &mut self.dry_forcing_scratch,
                &mut self.dry_uniform_cached,
            )?;
            if let Some(dry_velocity) = dry_velocity {
                let lanes = cast_species_lanes(dry_velocity, slot_count)?;
                self.dry_deposition_io
                    .upload_deposition_velocity(&self.gpu_context, lanes)?;
            }
        }
        if !skip_wet_deposition {
            let wet_scavenging = materialize_species_forcing_into(
                &forcing.wet_scavenging_coefficient_s_inv,
                slot_count,
                "wet_scavenging_coefficient_s_inv",
                &mut self.wet_scavenging_scratch,
                &mut self.wet_scavenging_uniform_cached,
            )?;
            let wet_fraction = forcing.wet_precipitating_fraction.materialize_into(
                slot_count,
                "wet_precipitating_fraction",
                &mut self.wet_fraction_scratch,
                &mut self.wet_fraction_uniform_cached,
            )?;
            if let Some(wet_scavenging) = wet_scavenging {
                let lanes = cast_species_lanes(wet_scavenging, slot_count)?;
                self.wet_deposition_io
                    .upload_scavenging_coefficient(&self.gpu_context, lanes)?;
            }
            if let Some(wet_fraction) = wet_fraction {
                self.wet_deposition_io
                    .upload_precipitating_fraction(&self.gpu_context, wet_fraction)?;
            }
        }

        self.staged_particles
            .set_dispatch_count(self.particle_buffers.particle_count());
        // ── Encode all GPU dispatches in a single command encoder ──────
        let (next_philox_counter, advection) = {
            let sampling_kernels = met
                .canonical
                .map(|_| {
                    crate::gpu::meteorology::MeteorologyCompositionKernels::new(&self.gpu_context)
                })
                .transpose()?;
            let resident_kernels = met
                .canonical
                .map(|_| {
                    crate::gpu::meteorology::resident::ResidentQueryKernels::new(&self.gpu_context)
                })
                .transpose()?;
            let mut encoder =
                self.gpu_context
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("backward_timeloop_step_encoder"),
                    });

            if use_gpu_pbl {
                encode_pbl_diagnostics_gpu_with_kernel(
                    &self.gpu_context,
                    &self.surface_field_buffer,
                    &self.pbl_buffers,
                    &self.config.pbl_options,
                    &self.pbl_dispatch_kernel,
                    &mut encoder,
                )?;
            }

            // Advection: negative dt for backward time direction.
            let advection = if let Some(canonical) = met.canonical {
                let step = ResidentAdvectionStep::encode(
                    &self.gpu_context,
                    &self.particle_buffers,
                    &self.staged_particles,
                    canonical.fields(),
                    canonical.bracket().horizontal_grid(),
                    [
                        times.expect("canonical times resolved").current,
                        times.expect("canonical times resolved").predicted,
                    ],
                    TimeDirection::Backward.advection_dt_seconds(dt_seconds),
                    self.config.velocity_to_grid_scale,
                    &self.resident_advection_kernels,
                    sampling_kernels
                        .as_ref()
                        .expect("canonical sampling kernels"),
                    resident_kernels.as_ref().expect("canonical query kernels"),
                    &mut encoder,
                )?;

                Some(step)
            } else {
                crate::gpu::encode_advection_dual_wind_gpu_with_kernel(
                    &self.gpu_context,
                    &self.particle_buffers,
                    self.dual_wind_buffers
                        .as_ref()
                        .expect("explicit diagnostic upload"),
                    interpolation_alpha,
                    TimeDirection::Backward.advection_dt_seconds(dt_seconds),
                    self.config.velocity_to_grid_scale,
                    &self.dual_wind_dispatch_kernel,
                    &mut encoder,
                )?;
                None
            };
            let physics_particles = if advection.is_some() {
                &self.staged_particles
            } else {
                &self.particle_buffers
            };

            // B-01 MVP: turbulence and deposition use positive dt magnitude.
            let langevin_step = LangevinStep {
                dt_seconds,
                rho_grad_over_rho: forcing.rho_grad_over_rho,
                n_substeps: 4,
                min_height_m: 0.01,
            };

            encode_hanna_params_gpu_with_kernel(
                &self.gpu_context,
                physics_particles,
                &self.pbl_buffers,
                &self.hanna_params_output,
                &self.hanna_dispatch_kernel,
                &mut encoder,
            )?;

            let next_pc =
                encode_update_particles_turbulence_langevin_gpu_with_hanna_output_and_kernel(
                    &self.gpu_context,
                    physics_particles,
                    &self.hanna_params_output,
                    langevin_step,
                    self.config.philox_key,
                    self.philox_counter,
                    &self.langevin_dispatch_kernel,
                    &mut encoder,
                )?;

            if !skip_dry_deposition {
                encode_dry_deposition_probability_gpu_with_kernel(
                    &self.gpu_context,
                    physics_particles,
                    &self.dry_deposition_io,
                    DryDepositionStepParams {
                        dt_seconds,
                        reference_height_m: self.config.dry_reference_height_m,
                    },
                    &self.dry_deposition_dispatch_kernel,
                    &mut encoder,
                )?;
            }

            if !skip_wet_deposition {
                encode_wet_deposition_probability_gpu_with_kernel(
                    &self.gpu_context,
                    physics_particles,
                    &self.wet_deposition_io,
                    WetDepositionStepParams { dt_seconds },
                    &self.wet_deposition_dispatch_kernel,
                    &mut encoder,
                )?;
            }

            // B-01 MVP: decay uses positive dt magnitude like deposition.
            if !skip_decay {
                encode_decay_gpu_with_kernel(
                    &self.gpu_context,
                    physics_particles,
                    DecayStepParams {
                        dt_seconds,
                        decay_constants_s_inv: forcing.decay_lanes(),
                    },
                    &self.decay_dispatch_kernel,
                    &mut encoder,
                )?;
            }

            if let Some(step) = &advection {
                step.encode_commit(
                    &self.gpu_context,
                    &self.staged_particles,
                    &self.particle_buffers,
                    &self.resident_advection_kernels,
                    &mut encoder,
                );
            }
            self.gpu_context.queue.submit(Some(encoder.finish()));
            (next_pc, advection)
        };

        #[cfg(test)]
        {
            self.latest_advection = advection.clone();
        }
        self.gpu_context.device.poll(wgpu::Maintain::Wait);
        if let Some(step) = &advection {
            step.require_success(&self.gpu_context).await?;
        }
        self.philox_counter = next_philox_counter;

        self.sync_store_from_gpu().await?;
        let source_collections = collect_source_collections(
            &self.particle_store,
            &self.release_grid,
            &self.config.source_regions,
        );
        let report = BackwardStepReport {
            step_index: self.step_index,
            timestamp,
            simulation_time_seconds: self.current_time_seconds,
            interpolation_alpha,
            released_count: release_report.released_count,
            released_slots: release_report.released_slots,
            active_particle_count: self.particle_store.active_count(),
            source_collections,
            next_philox_counter: self.philox_counter,
        };

        self.step_index += 1;
        self.current_time_seconds = self
            .current_time_seconds
            .saturating_sub(self.config.timestep_seconds);

        Ok(report)
    }

    /// Run until `end_timestamp` using fixed forcing and one met bracket.
    /// Explicit legacy diagnostic loop; never canonical production validation.
    pub async fn run_legacy_diagnostic_to_end(
        &mut self,
        met: &MetTimeBracket<'_>,
        forcing: &ForwardStepForcing,
    ) -> Result<Vec<BackwardStepReport>, TimeLoopError> {
        let mut reports = Vec::new();
        while self.has_remaining_steps() {
            reports.push(self.run_legacy_diagnostic_timestep(met, forcing).await?);
        }
        Ok(reports)
    }

    /// Integrate the canonical production path through the inclusive final step.
    pub async fn run_to_end(
        &mut self,
        met: &MetTimeBracket<'_>,
        forcing: &ForwardStepForcing,
    ) -> Result<Vec<BackwardStepReport>, TimeLoopError> {
        let mut reports = Vec::new();
        while self.has_remaining_steps() {
            reports.push(self.run_timestep(met, forcing).await?);
        }
        Ok(reports)
    }

    /// Upload wind_t0 and wind_t1 to dual-time GPU buffers if the met bracket
    /// changed. Creates buffers on first call. Skips when the bracket is
    /// unchanged (only `alpha` varies within a bracket).
    fn upload_dual_wind_if_bracket_changed(
        &mut self,
        met: &MetTimeBracket<'_>,
    ) -> Result<(), TimeLoopError> {
        let bracket_changed = self.current_met_t0_seconds != Some(met.time_t0_seconds)
            || self.current_met_t1_seconds != Some(met.time_t1_seconds);
        if !bracket_changed {
            return Ok(());
        }

        if let Some(buffers) = &self.dual_wind_buffers {
            buffers.upload_t0(&self.gpu_context, met.wind_t0)?;
            buffers.upload_t1(&self.gpu_context, met.wind_t1)?;
        } else {
            self.dual_wind_buffers = Some(DualWindBuffers::from_fields(
                &self.gpu_context,
                met.wind_t0,
                met.wind_t1,
            )?);
        }

        self.current_met_t0_seconds = Some(met.time_t0_seconds);
        self.current_met_t1_seconds = Some(met.time_t1_seconds);
        Ok(())
    }

    async fn sync_store_from_gpu(&mut self) -> Result<(), TimeLoopError> {
        let updated = self
            .particle_buffers
            .download_particles(&self.gpu_context)
            .await?;
        self.particle_store.as_mut_slice().copy_from_slice(&updated);
        self.particle_store.recount_active();
        Ok(())
    }
}

pub(super) fn collect_source_collections(
    store: &ParticleStore,
    grid: &GridDomain,
    source_regions: &[BackwardSourceRegionConfig],
) -> BTreeMap<String, BackwardSourceCollection> {
    let mut collections: BTreeMap<String, BackwardSourceCollection> = source_regions
        .iter()
        .map(|source| (source.name.clone(), BackwardSourceCollection::default()))
        .collect();
    if source_regions.is_empty() {
        return collections;
    }

    for particle in store.as_slice() {
        if !particle.is_active() {
            continue;
        }
        for source in source_regions {
            if source.contains_particle(particle, grid) {
                if let Some(collection) = collections.get_mut(&source.name) {
                    collection.hit_count += 1;
                    collection.total_mass_kg += particle.mass[0];
                }
            }
        }
    }

    collections
}

fn build_receptor_release_configs(
    receptors: &[BackwardReceptorConfig],
    release_timestamp: &str,
) -> Vec<ReleaseConfig> {
    receptors
        .iter()
        .map(|receptor| ReleaseConfig {
            name: receptor.name.clone(),
            start_time: release_timestamp.to_string(),
            end_time: release_timestamp.to_string(),
            lon: receptor.lon,
            lat: receptor.lat,
            z_min: receptor.z_m,
            z_max: receptor.z_m,
            mass_kg: receptor.mass_kg,
            particle_count: receptor.particle_count,
            species_masses_kg: None,
            raw: BTreeMap::new(),
        })
        .collect()
}
