//! Capacity-sized forcing validation/materialization and explicit GPU uploads.

use super::ForwardTimeLoopDriver;
use crate::simulation::timeloop::error::TimeLoopError;
use crate::simulation::timeloop::forcing::{
    cast_species_lanes, materialize_species_forcing_into, ForwardStepForcing,
};
use crate::simulation::timeloop::time::timestep_seconds_f32;
use std::time::{Duration, Instant};

/// Validated timestep magnitude, forcing skip decisions and upload durations.
pub(super) struct PreparedForcing {
    pub(super) step_dt_seconds: f32,
    pub(super) skip_dry_deposition: bool,
    pub(super) skip_wet_deposition: bool,
    pub(super) skip_decay: bool,
    pub(super) forcing_dur: Duration,
    pub(super) dep_upload_dur: Duration,
}

impl ForwardTimeLoopDriver {
    pub(super) fn prepare_forcing(
        &mut self,
        forcing: &ForwardStepForcing,
        profiling: bool,
    ) -> Result<PreparedForcing, TimeLoopError> {
        let step_dt_seconds = timestep_seconds_f32(self.config.timestep_seconds)?;

        forcing.check_species_shape()?;
        let skip_dry_deposition = forcing.skip_dry_deposition();
        let skip_wet_deposition = forcing.skip_wet_deposition();
        let skip_decay = forcing.skip_decay();

        let t = profiling.then(Instant::now);
        // Use buffer capacity (not dispatch count) for forcing materialization:
        // deposition I/O buffers were pre-allocated for the full capacity.
        // Species forcing uploads as interleaved `vec4` lanes
        // (`slot * MAX_SPECIES + lane`); plain `&[f32]` slices need a cast
        // through the species-lane helper first.
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
        let forcing_dur = t.map_or(Duration::ZERO, |t| t.elapsed());
        let dep_upload_dur = Duration::ZERO;

        Ok(PreparedForcing {
            step_dt_seconds,
            skip_dry_deposition,
            skip_wet_deposition,
            skip_decay,
            forcing_dur,
            dep_upload_dur,
        })
    }
}
