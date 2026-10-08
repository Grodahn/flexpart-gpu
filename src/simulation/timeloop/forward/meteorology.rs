//! Prefetch, bracket residency and PBL input preparation before the previous-step wait.

use super::ForwardTimeLoopDriver;
use crate::gpu::{AdvectionDispatchKernel, DualWindBuffers, WindBuffers, WindSamplingPath};
use crate::io::{
    compute_pbl_parameters_from_met, interpolate_surface_fields_linear, Era5GribGridMetadata,
    Era5MvpSnapshot, Grib2ReaderError, GribPrefetchHandle, PblMetInputGrids,
};
use crate::simulation::timeloop::error::TimeLoopError;
use crate::simulation::timeloop::meteorology::{
    CanonicalMeteorologyBracket, CanonicalMeteorologySlot, MetTimeBracket,
    PreparedCanonicalMeteorology,
};
use crate::simulation::timeloop::options::is_gpu_pbl_enabled;
use crate::simulation::timeloop::time::interpolation_alpha;
use crate::wind::WindField3D;
use std::path::Path;
use std::time::{Duration, Instant};

/// Prepared bracket/PBL inputs and their existing profiling durations.
pub(super) struct PreparedMeteorology {
    pub(super) interpolation_alpha: f32,
    pub(super) use_gpu_pbl: bool,
    pub(super) wind_upload_dur: Duration,
    pub(super) wind_interp_dur: Duration,
    pub(super) surf_interp_dur: Duration,
    pub(super) pbl_dur: Duration,
    pub(super) pbl_upload_dur: Duration,
}

impl ForwardTimeLoopDriver {
    /// Prepare validated canonical U/V/center-W for the current forward step and +dt.
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
            self.config.timestep_seconds,
        )?)
    }

    /// Start prefetching the next meteorological file in a background thread.
    ///
    /// Call this when the next met file path is known (e.g. based on simulation
    /// time approaching a bracket boundary). The prefetch runs on a dedicated OS
    /// thread in parallel with GPU computation. Any previously active prefetch
    /// is replaced (the orphaned thread will run to completion but its result is
    /// discarded).
    pub fn prefetch_met(
        &mut self,
        path: impl AsRef<Path> + Send + 'static,
        expected_grid: Option<Era5GribGridMetadata>,
    ) {
        self.grib_prefetch = Some(GribPrefetchHandle::start(path, expected_grid));
    }

    /// If a prefetch is active and complete, consume and return the result.
    ///
    /// Returns `None` if no prefetch is active or if the background thread has
    /// not finished yet. On success the internal handle is consumed; calling
    /// this again without a new [`prefetch_met`](Self::prefetch_met) returns
    /// `None`.
    pub fn try_consume_prefetch(&mut self) -> Option<Result<Era5MvpSnapshot, Grib2ReaderError>> {
        if self.grib_prefetch.as_ref().is_some_and(|h| h.is_ready()) {
            Some(
                self.grib_prefetch
                    .take()
                    .expect("checked Some above")
                    .await_result(),
            )
        } else {
            None
        }
    }

    /// Legacy single-wind upload path, kept for backward compatibility testing.
    #[allow(dead_code)]
    pub(super) fn ensure_wind_buffers(&mut self, wind: &WindField3D) -> Result<(), TimeLoopError> {
        if let Some(buffers) = &self.wind_buffers {
            buffers.upload_field(&self.gpu_context, wind)?;
            return Ok(());
        }
        self.wind_buffers = Some(WindBuffers::from_field(&self.gpu_context, wind)?);
        Ok(())
    }

    /// Legacy single-wind kernel setup, kept for backward compatibility testing.
    #[allow(dead_code)]
    pub(super) fn ensure_advection_dispatch_kernel(&mut self, sampling_path: WindSamplingPath) {
        let recreate = match self.advection_dispatch_kernel.as_ref() {
            Some(kernel) => kernel.sampling_path() != sampling_path,
            None => true,
        };
        if recreate {
            self.advection_dispatch_kernel = Some(AdvectionDispatchKernel::new(
                &self.gpu_context,
                sampling_path,
            ));
        }
    }

    /// Upload wind_t0 and wind_t1 to dual-time GPU buffers if the met bracket
    /// changed. Creates the buffers on first call (the 3-D wind grid shape is
    /// only known at first met bracket). Skips entirely when the bracket is
    /// unchanged, since only `alpha` varies within a bracket.
    pub(super) fn upload_dual_wind_if_bracket_changed(
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

    /// Stage the current bracket and PBL inputs before the previous-step wait.
    ///
    /// This preserves the existing GPU path and explicit diagnostic override;
    /// it neither submits work nor waits or reads device state.
    pub(super) fn prepare_meteorology(
        &mut self,
        met: &MetTimeBracket<'_>,
        profiling: bool,
    ) -> Result<PreparedMeteorology, TimeLoopError> {
        let interpolation_alpha = interpolation_alpha(
            met.time_t0_seconds,
            met.time_t1_seconds,
            self.current_time_seconds,
            self.config.time_bounds_behavior,
        )?;

        // O-02: upload wind_t0 and wind_t1 once per met bracket.
        let t = profiling.then(Instant::now);
        self.upload_dual_wind_if_bracket_changed(met)?;
        let wind_upload_dur = t.map_or(Duration::ZERO, |t| t.elapsed());
        let wind_interp_dur = Duration::ZERO;

        let t = profiling.then(Instant::now);
        let interpolated_surface = interpolate_surface_fields_linear(
            met.surface_t0,
            met.surface_t1,
            met.time_t0_seconds,
            met.time_t1_seconds,
            self.current_time_seconds,
            self.config.time_bounds_behavior,
        )?;
        let surf_interp_dur = t.map_or(Duration::ZERO, |t| t.elapsed());

        let use_gpu_pbl = is_gpu_pbl_enabled();
        let (pbl_dur, pbl_upload_dur) = if use_gpu_pbl {
            let t = profiling.then(Instant::now);
            self.surface_field_buffer
                .upload(&self.gpu_context, &interpolated_surface)?;
            let dur = t.map_or(Duration::ZERO, |t| t.elapsed());
            (dur, Duration::ZERO)
        } else {
            let t = profiling.then(Instant::now);
            let computed_pbl = compute_pbl_parameters_from_met(
                PblMetInputGrids {
                    surface: &interpolated_surface,
                    profile: None,
                },
                self.config.pbl_options,
            )?;
            let pbl_dur = t.map_or(Duration::ZERO, |t| t.elapsed());

            let t = profiling.then(Instant::now);
            self.pbl_buffers[self.pbl_write_index]
                .upload_state(&self.gpu_context, &computed_pbl.pbl_state)?;
            let pbl_upload_dur = t.map_or(Duration::ZERO, |t| t.elapsed());
            (pbl_dur, pbl_upload_dur)
        };

        Ok(PreparedMeteorology {
            interpolation_alpha,
            use_gpu_pbl,
            wind_upload_dur,
            wind_interp_dur,
            surf_interp_dur,
            pbl_dur,
            pbl_upload_dur,
        })
    }
}
