//! Forward timestep phases, pending-work transitions, report and inclusive time advancement.

use super::forcing::PreparedForcing;
use super::meteorology::PreparedMeteorology;
use super::ForwardTimeLoopDriver;
use crate::release::ReleaseStepReport;
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
    ///
    /// Production releases are private until the device accepts the complete
    /// boundary. Reports without observation can describe queued work.
    /// Finalization and output require the sticky device verdict.
    pub async fn run_timestep(
        &mut self,
        met: &MetTimeBracket<'_>,
        forcing: &ForwardStepForcing,
    ) -> Result<ForwardStepReport, TimeLoopError> {
        self.run_timestep_impl(met, forcing, false).await
    }

    /// Explicit legacy diagnostic only; never production or canonical-migration evidence.
    /// Retained for the existing independently owned synthetic/oracle diagnostics (#117).
    pub async fn run_legacy_diagnostic_timestep(
        &mut self,
        met: &MetTimeBracket<'_>,
        forcing: &ForwardStepForcing,
    ) -> Result<ForwardStepReport, TimeLoopError> {
        self.run_timestep_impl(met, forcing, true).await
    }

    async fn run_timestep_impl(
        &mut self,
        met: &MetTimeBracket<'_>,
        forcing: &ForwardStepForcing,
        legacy_diagnostic: bool,
    ) -> Result<ForwardStepReport, TimeLoopError> {
        if self.production_ledger_rejected {
            return self
                .require_forward_ledger_success()
                .await
                .and(Err(TimeLoopError::MassLedgerUnavailable));
        }
        if (legacy_diagnostic && self.latest_submitted_ledger_step.is_some())
            || (!legacy_diagnostic
                && self.step_index != 0
                && self.latest_submitted_ledger_step.is_none())
        {
            return Err(TimeLoopError::MassLedgerUnsupported(
                "cannot switch between diagnostic and production runs",
            ));
        }
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
        // A per-step override cannot replace the run-owned carrier route or enable a velocity producer.
        if forcing.settling_enabled {
            return Err(crate::gpu::meteorology::MeteorologyCompositionError::Unsupported(
                "transport settling requires validated settling and sticky particle-stop state (#203; physics #35/#37)",
            ).into());
        }
        if !legacy_diagnostic {
            let route = self.transport_settling.as_ref().ok_or(
                crate::gpu::meteorology::MeteorologyCompositionError::Missing(
                    "run-owned transport settling/carrier route",
                ),
            )?;
            route.stop_route(
                &self.gpu_context,
                crate::simulation::timeloop::time::timestep_seconds_f32(
                    self.config.timestep_seconds,
                )?,
            )?;
        }
        if let Some(canonical) = met.canonical {
            canonical.validate_production(&self.gpu_context)?;
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
        let step_number =
            u32::try_from(self.step_index).map_err(|_| TimeLoopError::MassLedgerRange)?;
        let terminal = !legacy_diagnostic && self.is_terminal_ledger_step(step_number)?;
        if !legacy_diagnostic {
            if self.ledger_duration_seconds % self.config.timestep_seconds != 0 {
                return Err(TimeLoopError::MassLedgerUnsupported(
                    "terminal time must align with the v1 synchronization interval",
                ));
            }
            let species = &self
                .config
                .transport_settling
                .as_ref()
                .expect("canonical species ownership checked")
                .species;
            if !forcing
                .wet_scavenging_coefficient_s_inv
                .iter()
                .all(super::super::forcing::ParticleForcingField::is_zero)
                || !forcing.wet_precipitating_fraction.is_zero()
                || species
                    .iter()
                    .any(|s| s.below_cloud_wet_active() || s.in_cloud_wet_active())
            {
                return Err(TimeLoopError::MassLedgerUnsupported(
                    "#34 pre-release canonical wet-deposition mass credits are unavailable",
                ));
            }
            if !forcing.skip_dry_deposition()
                || species
                    .iter()
                    .any(crate::config::SpeciesConfig::dry_deposition_active)
            {
                return Err(TimeLoopError::MassLedgerUnsupported(
                    "#37 transport-substep dry-deposition mass credits are unavailable",
                ));
            }
            if forcing
                .decay_constant_s_inv
                .iter()
                .any(|lambda| *lambda != 0.0)
                || species
                    .iter()
                    .any(crate::config::SpeciesConfig::decay_active)
            {
                return Err(TimeLoopError::MassLedgerUnsupported(
                    "#39 coupled dry/decay and deposited-stock decay are unavailable",
                ));
            }
            if self.config.spatial_sort.is_some() {
                return Err(TimeLoopError::MassLedgerUnsupported(
                    "host spatial sorting cannot mutate a production ledger transaction",
                ));
            }
        }
        let times = met
            .canonical
            .filter(|_| !terminal)
            .map(|canonical| {
                canonical
                    .bracket()
                    .resolve_step_times(self.current_time_seconds, self.config.timestep_seconds)
            })
            .transpose()?;
        if legacy_diagnostic {
            self.apply_spatial_sort_if_enabled()?;
        }

        let profiling = is_profiling_enabled();
        let total_start = profiling.then(Instant::now);

        // ── Phase 1: CPU prep (overlaps with previous GPU submission) ──

        let timestamp = format_timestamp_seconds(self.current_time_seconds)?;
        let batch = if legacy_diagnostic {
            None
        } else {
            self.release_manager.prepare_batch(
                &timestamp,
                self.particle_buffers.capacity() - self.next_production_slot,
            )?
        };
        let release = batch
            .as_ref()
            .map_or(&[][..], |(_, batch)| batch.as_slice());
        let release_report = if legacy_diagnostic {
            self.release_manager.inject_and_upload_for_time(
                &timestamp,
                &mut self.particle_store,
                &self.particle_buffers,
                &self.gpu_context,
            )?
        } else {
            ReleaseStepReport {
                released_count: release.len(),
                released_slots: (self.next_production_slot
                    ..self.next_production_slot + release.len())
                    .collect(),
            }
        };

        // O-07: After release, widen the dispatch window to cover both the
        // compacted active prefix from the previous step and newly released
        // particles (which the release manager placed at the first free
        // slots immediately after the active prefix).
        if self.use_compaction && legacy_diagnostic {
            self.particle_buffers
                .set_dispatch_count(self.particle_store.active_count());
        }

        if legacy_diagnostic {
            self.upload_dual_wind_if_bracket_changed(met)?;
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

        let dispatch = if self.use_compaction && !legacy_diagnostic {
            self.next_production_slot + release.len()
        } else {
            self.particle_buffers.particle_count()
        };
        self.released_particles.set_dispatch_count(dispatch);
        self.staged_particles.set_dispatch_count(dispatch);

        // ── Phase 3: Encode + submit (non-blocking) ───────────────────

        let (next_philox_counter, gpu_encode_dur, advection) = self.submit_operators(
            &prepared_met,
            met.canonical,
            times,
            &prepared_forcing,
            forcing,
            profiling,
            release,
        )?;

        #[cfg(test)]
        {
            if advection.is_some() {
                self.latest_advection = advection;
            }
        }
        #[cfg(not(test))]
        drop(advection);
        if !legacy_diagnostic {
            self.latest_submitted_ledger_step = Some(step_number);
            self.next_production_slot += release.len();
            if let Some((schedule, _)) = batch {
                self.release_manager = schedule;
            }
        }
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

        if profiling
            || self.use_compaction
            || self.config.sync_particle_store_each_step
            || self.config.collect_deposition_probabilities_each_step
        {
            if let Err(error) = self.require_forward_ledger_success().await {
                self.production_ledger_rejected = true;
                return Err(error);
            }
        }
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
        if self.use_compaction && !terminal {
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
            if !legacy_diagnostic {
                self.next_production_slot = active_count;
            }
        }
        if self.use_compaction && terminal {
            self.particle_buffers.set_dispatch_count(dispatch);
            self.particle_store.reset_after_compaction(dispatch);
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
