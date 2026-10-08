//! Ordered forward GPU operators and the unchanged single-submit boundary.

use super::forcing::PreparedForcing;
use super::meteorology::PreparedMeteorology;
use super::ForwardTimeLoopDriver;
use crate::gpu::advection_resident::ResidentAdvectionStep;
use crate::gpu::{
    encode_compaction_with_reorder, encode_decay_gpu_with_kernel,
    encode_dry_deposition_probability_gpu_with_kernel, encode_hanna_params_gpu_with_kernel,
    encode_langevin_fused_gpu, encode_pbl_diagnostics_gpu_with_kernel,
    encode_update_particles_turbulence_langevin_gpu_with_hanna_output_and_kernel,
    encode_wet_deposition_probability_gpu_with_kernel, DecayStepParams, DryDepositionStepParams,
    WetDepositionStepParams,
};
use crate::physics::{LangevinStep, PhiloxCounter};
use crate::simulation::timeloop::error::TimeLoopError;
use crate::simulation::timeloop::forcing::ForwardStepForcing;
use crate::simulation::timeloop::meteorology::{
    CanonicalMeteorologyResources, CanonicalMeteorologyStepTimes,
};
use std::time::{Duration, Instant};

impl ForwardTimeLoopDriver {
    /// Submit the dependent GPU operators in their preserved order with one encoder.
    ///
    /// Prepared inputs refer to the current PBL write slot. This boundary does
    /// not poll or read back; the coordinator commits pending state and counters
    /// only after successful submission.
    pub(super) fn submit_operators(
        &self,
        met: &PreparedMeteorology,
        canonical: Option<&CanonicalMeteorologyResources<'_>>,
        times: Option<CanonicalMeteorologyStepTimes>,
        prepared: &PreparedForcing,
        forcing: &ForwardStepForcing,
        profiling: bool,
    ) -> Result<(PhiloxCounter, Duration, Option<ResidentAdvectionStep>), TimeLoopError> {
        let PreparedMeteorology { use_gpu_pbl, .. } = *met;
        let PreparedForcing {
            step_dt_seconds,
            skip_dry_deposition,
            skip_wet_deposition,
            skip_decay,
            ..
        } = *prepared;
        let dry_params = DryDepositionStepParams {
            dt_seconds: step_dt_seconds,
            reference_height_m: self.config.dry_reference_height_m,
        };
        let wet_params = WetDepositionStepParams {
            dt_seconds: step_dt_seconds,
        };
        // Radioactive decay commutes with deposition survival factors; it runs
        // last so deposition probability outputs reflect pre-decay masses.
        let decay_params = DecayStepParams {
            dt_seconds: step_dt_seconds,
            decay_constants_s_inv: forcing.decay_lanes(),
        };
        let t = profiling.then(Instant::now);
        let (next_philox_counter, advection) = {
            let sampling_kernels = canonical
                .map(|_| {
                    crate::gpu::meteorology::MeteorologyCompositionKernels::new(&self.gpu_context)
                })
                .transpose()?;
            let resident_kernels = canonical
                .map(|_| {
                    crate::gpu::meteorology::resident::ResidentQueryKernels::new(&self.gpu_context)
                })
                .transpose()?;
            let mut encoder =
                self.gpu_context
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("forward_timeloop_step_encoder"),
                    });

            // ── PBL diagnostics (both paths) ────────────────────────
            if use_gpu_pbl {
                encode_pbl_diagnostics_gpu_with_kernel(
                    &self.gpu_context,
                    &self.surface_field_buffer,
                    &self.pbl_buffers[self.pbl_write_index],
                    &self.config.pbl_options,
                    &self.pbl_dispatch_kernel,
                    &mut encoder,
                )?;
            }

            // ── Advection (both paths) ───────────────────────────────
            let advection = if let Some(canonical) = canonical {
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
                    step_dt_seconds,
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
                    met.interpolation_alpha,
                    step_dt_seconds,
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

            // ── Hanna + Langevin turbulence ──────────────────────────
            let pbl_buffers = &self.pbl_buffers[self.pbl_write_index];
            let langevin_step = LangevinStep {
                dt_seconds: step_dt_seconds,
                rho_grad_over_rho: forcing.rho_grad_over_rho,
                n_substeps: self.config.langevin_vertical_substeps,
                min_height_m: self.config.langevin_min_height_m,
            };

            let next_philox_counter = if !self.validation_mode {
                // ── Production: fused Hanna+Langevin (single dispatch) ──
                encode_langevin_fused_gpu(
                    &self.gpu_context,
                    physics_particles,
                    pbl_buffers,
                    langevin_step,
                    self.config.philox_key,
                    self.philox_counter,
                    self.langevin_fused_dispatch_kernel
                        .as_ref()
                        .expect("fused kernel allocated in production mode"),
                    &mut encoder,
                )?
            } else {
                // ── Validation: separated Hanna → Langevin dispatches ──
                let hanna_output = self
                    .hanna_params_output
                    .as_ref()
                    .expect("hanna output allocated in validation mode");
                encode_hanna_params_gpu_with_kernel(
                    &self.gpu_context,
                    physics_particles,
                    pbl_buffers,
                    hanna_output,
                    self.hanna_dispatch_kernel
                        .as_ref()
                        .expect("hanna kernel allocated in validation mode"),
                    &mut encoder,
                )?;
                encode_update_particles_turbulence_langevin_gpu_with_hanna_output_and_kernel(
                    &self.gpu_context,
                    physics_particles,
                    hanna_output,
                    langevin_step,
                    self.config.philox_key,
                    self.philox_counter,
                    self.langevin_dispatch_kernel
                        .as_ref()
                        .expect("langevin kernel allocated in validation mode"),
                    &mut encoder,
                )?
            };

            // ── Deposition (both paths) ──────────────────────────────
            if !skip_dry_deposition {
                encode_dry_deposition_probability_gpu_with_kernel(
                    &self.gpu_context,
                    physics_particles,
                    &self.dry_deposition_io,
                    dry_params,
                    &self.dry_deposition_dispatch_kernel,
                    &mut encoder,
                )?;
            }
            if !skip_wet_deposition {
                encode_wet_deposition_probability_gpu_with_kernel(
                    &self.gpu_context,
                    physics_particles,
                    &self.wet_deposition_io,
                    wet_params,
                    &self.wet_deposition_dispatch_kernel,
                    &mut encoder,
                )?;
            }
            if !skip_decay {
                encode_decay_gpu_with_kernel(
                    &self.gpu_context,
                    physics_particles,
                    decay_params,
                    &self.decay_dispatch_kernel,
                    &mut encoder,
                )?;
            }

            // O-07: Encode compaction + gather/reorder after all physics
            // passes. Compaction reads particle flags set by deposition and
            // reorders the buffer so active particles are contiguous.
            if self.use_compaction {
                encode_compaction_with_reorder(
                    &self.gpu_context,
                    physics_particles,
                    self.compaction_buffers
                        .as_ref()
                        .expect("compaction buffers allocated when compaction is enabled"),
                    self.compaction_pipelines
                        .as_ref()
                        .expect("compaction pipelines allocated when compaction is enabled"),
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
            (next_philox_counter, advection)
        };
        let gpu_encode_dur = t.map_or(Duration::ZERO, |t| t.elapsed());

        Ok((next_philox_counter, gpu_encode_dur, advection))
    }
}
