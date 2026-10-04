//! Forward timestep phases, pending-work transitions, report and inclusive time advancement.

use super::forcing::PreparedForcing;
use super::meteorology::PreparedMeteorology;
use super::ForwardTimeLoopDriver;
use crate::simulation::timeloop::error::TimeLoopError;
use crate::simulation::timeloop::forcing::ForwardStepForcing;
use crate::simulation::timeloop::meteorology::MetTimeBracket;
use crate::simulation::timeloop::options::{dur_ms, is_profiling_enabled};
use crate::simulation::timeloop::reports::{ForwardStepReport, StepTimingReport};
use crate::simulation::timeloop::time::format_timestamp_seconds;
use std::time::{Duration, Instant};

impl ForwardTimeLoopDriver {
    /// Run one orchestrated timestep with CPU/GPU overlap (O-03).
    ///
    /// The method is split into four phases:
    ///
    /// 1. **CPU prep** — surface interpolation, PBL computation, buffer
    ///    staging. Runs concurrently with the *previous* step's in-flight
    ///    GPU work.
    /// 2. **Wait previous GPU** — block until the previous submission
    ///    completes (no-op on the first step).
    /// 3. **Encode + submit** — build the command encoder and call
    ///    `queue.submit()`. Returns immediately; GPU starts asynchronously.
    /// 4. **Optional wait + readback** — if per-step synchronization or
    ///    deposition probability collection is enabled, block until *this*
    ///    step's GPU work finishes and download results.
    ///
    /// PBL buffers use a ping-pong double buffer so that CPU uploads for
    /// step N+1 never race with GPU reads from step N.
    pub async fn run_timestep(
        &mut self,
        met: &MetTimeBracket<'_>,
        forcing: &ForwardStepForcing,
    ) -> Result<ForwardStepReport, TimeLoopError> {
        if !self.has_remaining_steps() {
            return Err(TimeLoopError::SimulationComplete);
        }
        self.apply_spatial_sort_if_enabled()?;

        let profiling = is_profiling_enabled();
        let total_start = profiling.then(Instant::now);

        // ── Phase 1: CPU prep (overlaps with previous GPU submission) ──

        let timestamp = format_timestamp_seconds(self.current_time_seconds)?;
        let release_report = self.release_manager.inject_and_upload_for_time(
            &timestamp,
            &mut self.particle_store,
            &self.particle_buffers,
            &self.gpu_context,
        )?;

        // O-07: After release, widen the dispatch window to cover both the
        // compacted active prefix from the previous step and newly released
        // particles (which the release manager placed at the first free
        // slots immediately after the active prefix).
        if self.use_compaction {
            self.particle_buffers
                .set_dispatch_count(self.particle_store.active_count());
        }

        let prepared_met = self.prepare_meteorology(met, profiling)?;
        let PreparedMeteorology {
            interpolation_alpha,
            wind_upload_dur,
            wind_interp_dur,
            surf_interp_dur,
            pbl_dur,
            pbl_upload_dur,
            ..
        } = prepared_met;

        let prepared_forcing = self.prepare_forcing(forcing, profiling)?;
        let PreparedForcing {
            skip_dry_deposition,
            skip_wet_deposition,
            forcing_dur,
            dep_upload_dur,
            ..
        } = prepared_forcing;

        // ── Phase 2: Wait for previous GPU submission ──────────────────

        let wait_prev_dur = if self.gpu_submission_pending {
            let t = profiling.then(Instant::now);
            self.gpu_context.device.poll(wgpu::Maintain::Wait);
            self.gpu_submission_pending = false;
            t.map_or(Duration::ZERO, |t| t.elapsed())
        } else {
            Duration::ZERO
        };

        // ── Phase 3: Encode + submit (non-blocking) ───────────────────

        let (next_philox_counter, gpu_encode_dur) =
            self.submit_operators(&prepared_met, &prepared_forcing, forcing, profiling)?;

        self.gpu_submission_pending = true;
        self.pbl_write_index = 1 - self.pbl_write_index;

        // ── Phase 4: Optional wait + readback ──────────────────────────
        // When profiling, we always poll to measure `gpu_exec`. When
        // collecting deposition probabilities or syncing the particle
        // store, the async download methods poll internally.

        let gpu_exec_dur = if profiling {
            let t = Instant::now();
            self.gpu_context.device.poll(wgpu::Maintain::Wait);
            self.gpu_submission_pending = false;
            t.elapsed()
        } else {
            Duration::ZERO
        };

        self.philox_counter = next_philox_counter;

        let dry_probability =
            if self.config.collect_deposition_probabilities_each_step && !skip_dry_deposition {
                let result = self
                    .dry_deposition_io
                    .download_probabilities(&self.gpu_context)
                    .await?;
                self.gpu_submission_pending = false;
                result
            } else {
                Vec::new()
            };
        let wet_probability =
            if self.config.collect_deposition_probabilities_each_step && !skip_wet_deposition {
                let result = self
                    .wet_deposition_io
                    .download_probabilities(&self.gpu_context)
                    .await?;
                self.gpu_submission_pending = false;
                result
            } else {
                Vec::new()
            };

        let t_compact = profiling.then(Instant::now);
        if self.use_compaction {
            if self.gpu_submission_pending {
                self.gpu_context.device.poll(wgpu::Maintain::Wait);
                self.gpu_submission_pending = false;
            }
            let active_count = self
                .compaction_buffers
                .as_ref()
                .expect("compaction buffers allocated when compaction is enabled")
                .download_active_count(&self.gpu_context)
                .await? as usize;
            self.particle_buffers.set_dispatch_count(active_count);
            self.particle_store.reset_after_compaction(active_count);
        }
        if self.config.sync_particle_store_each_step {
            if self.gpu_submission_pending {
                self.gpu_context.device.poll(wgpu::Maintain::Wait);
                self.gpu_submission_pending = false;
            }
            self.sync_store_from_gpu().await?;
        }
        let compaction_dur = t_compact.map_or(Duration::ZERO, |t| t.elapsed());

        let timing = if profiling {
            let total_dur = total_start.map_or(Duration::ZERO, |t| t.elapsed());
            let report = StepTimingReport {
                wind_interp_ms: dur_ms(wind_interp_dur),
                surf_interp_ms: dur_ms(surf_interp_dur),
                pbl_ms: dur_ms(pbl_dur),
                wind_upload_ms: dur_ms(wind_upload_dur),
                pbl_upload_ms: dur_ms(pbl_upload_dur),
                forcing_ms: dur_ms(forcing_dur),
                dep_upload_ms: dur_ms(dep_upload_dur),
                wait_prev_gpu_ms: dur_ms(wait_prev_dur),
                gpu_encode_ms: dur_ms(gpu_encode_dur),
                gpu_exec_ms: dur_ms(gpu_exec_dur),
                compaction_ms: dur_ms(compaction_dur),
                total_ms: dur_ms(total_dur),
            };
            report.print_summary(self.step_index);
            Some(report)
        } else {
            None
        };

        let report = ForwardStepReport {
            step_index: self.step_index,
            timestamp,
            simulation_time_seconds: self.current_time_seconds,
            interpolation_alpha,
            released_count: release_report.released_count,
            released_slots: release_report.released_slots,
            // In performance mode without per-step host sync, this value reflects
            // the host-side cached count and can lag GPU-side deactivations.
            active_particle_count: self.particle_store.active_count(),
            dry_deposition_probability: dry_probability,
            wet_deposition_probability: wet_probability,
            next_philox_counter: self.philox_counter,
            timing,
        };

        self.step_index += 1;
        self.current_time_seconds = self
            .current_time_seconds
            .saturating_add(self.config.timestep_seconds);

        Ok(report)
    }
}
