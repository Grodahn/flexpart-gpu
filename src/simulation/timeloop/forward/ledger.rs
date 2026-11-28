//! Device-owned stable forward inventory and its explicit observation boundary.

use super::ForwardTimeLoopDriver;
use crate::gpu::advection_resident::ResidentAdvectionStep;
use crate::gpu::meteorology::resident::QueryPass;
use crate::gpu::{download_buffer_typed, GpuContext, ParticleBuffers};
use crate::particles::{Particle, MAX_SPECIES};
use crate::simulation::timeloop::error::TimeLoopError;
use crate::validation::mass_ledger::{
    LedgerBalance, LedgerObservation, LedgerPhase, LedgerStep, MassInventory,
};
use wgpu::util::DeviceExt;

type SpeciesMass = [f32; MAX_SPECIES];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LedgerStatus {
    code: u32,
    step: u32,
    lane: u32,
    committed: u32,
}

impl LedgerStatus {
    fn validate(self, expected_step: u32) -> Result<(), TimeLoopError> {
        if self.code != 0 {
            return Err(TimeLoopError::MassLedgerRejected {
                code: self.code,
                step: self.step,
                lane: self.lane,
            });
        }
        if self.step != expected_step || self.committed != 1 {
            return Err(TimeLoopError::MassLedgerUnavailable);
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BoundaryMass {
    before: SpeciesMass,
    released: SpeciesMass,
    sample: SpeciesMass,
    horizontal_exit: SpeciesMass,
    airborne: SpeciesMass,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DeviceInventory {
    status: LedgerStatus,
    injected: SpeciesMass,
    airborne: SpeciesMass,
    horizontal_exit: SpeciesMass,
    injection_correction: SpeciesMass,
    horizontal_exit_correction: SpeciesMass,
    wet: SpeciesMass,
    dry: SpeciesMass,
    decayed: SpeciesMass,
    age_exit: SpeciesMass,
    mass_exit: SpeciesMass,
    age_and_mass_exit: SpeciesMass,
    last: BoundaryMass,
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_LEDGER_FAULT: std::cell::Cell<Option<LedgerFault>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum LedgerFault {
    ClearStops,
    NegativeMass,
    CreditLiveExit,
}

pub(super) struct ForwardMassLedger {
    state: wgpu::Buffer,
    empty: wgpu::Buffer,
    audit: QueryPass,
    publish: QueryPass,
}

impl ForwardMassLedger {
    pub(super) fn new(ctx: &GpuContext) -> Result<Self, TimeLoopError> {
        ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let buffer = |label, size| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let result = Self {
            state: buffer(
                "forward mass ledger",
                std::mem::size_of::<DeviceInventory>() as u64,
            ),
            empty: buffer(
                "absent forward operator input",
                std::mem::size_of::<Particle>() as u64,
            ),
            audit: QueryPass::new(
                ctx,
                "forward mass audit",
                include_str!("../../../shaders/forward_mass_ledger.wgsl"),
                &[true, true, true, true, true, true, false],
            ),
            publish: QueryPass::new(
                ctx,
                "forward particle publication",
                include_str!("../../../shaders/forward_mass_commit.wgsl"),
                &[true, true, false],
            ),
        };
        let errors: Vec<_> = (0..3)
            .filter_map(|_| pollster::block_on(ctx.device.pop_error_scope()))
            .collect();
        if !errors.is_empty() {
            return Err(
                crate::gpu::meteorology::MeteorologyCompositionError::ResidentDevice(format!(
                    "{errors:?}"
                ))
                .into(),
            );
        }
        Ok(result)
    }

    /// Stage release payloads in a private particle snapshot. The original
    /// allocation remains the source for the independent pre-release observation.
    pub(super) fn encode_release(
        ctx: &GpuContext,
        original: &ParticleBuffers,
        released: &ParticleBuffers,
        start_slot: usize,
        batch: &[Particle],
        encoder: &mut wgpu::CommandEncoder,
    ) -> Option<wgpu::Buffer> {
        encoder.copy_buffer_to_buffer(
            &original.particle_buffer,
            0,
            &released.particle_buffer,
            0,
            original.particle_buffer.size(),
        );
        if batch.is_empty() {
            return None;
        }
        let payload = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("forward represented release masses and positions"),
                contents: bytemuck::cast_slice(batch),
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::STORAGE,
            });
        encoder.copy_buffer_to_buffer(
            &payload,
            0,
            &released.particle_buffer,
            (start_slot * std::mem::size_of::<Particle>()) as u64,
            payload.size(),
        );
        Some(payload)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_audit(
        &self,
        ctx: &GpuContext,
        original: &ParticleBuffers,
        released: &ParticleBuffers,
        staged: &ParticleBuffers,
        payload: Option<&wgpu::Buffer>,
        transport: Option<&ResidentAdvectionStep>,
        release_slots: std::ops::Range<usize>,
        species: usize,
        step: usize,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<wgpu::Buffer, TimeLoopError> {
        let word = |n| u32::try_from(n).map_err(|_| TimeLoopError::MassLedgerRange);
        let words = [
            word(original.capacity())?,
            word(release_slots.start)?,
            word(release_slots.len())?,
            word(species)?,
            word(step)?,
            u32::from(transport.is_some()),
            0,
            0,
        ];
        let params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("forward ledger boundary parameters"),
                contents: bytemuck::cast_slice(&words),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        #[cfg(test)]
        match TEST_LEDGER_FAULT.with(std::cell::Cell::take) {
            Some(LedgerFault::ClearStops) => {
                encoder.clear_buffer(
                    &transport
                        .expect("injected stop fault needs transport")
                        .stop_words,
                    0,
                    None,
                );
            }
            Some(fault) => {
                let (buffer, offset, bits) = match fault {
                    LedgerFault::CreditLiveExit => (
                        &transport
                            .expect("injected stop fault needs transport")
                            .stop_words,
                        0,
                        4_u32,
                    ),
                    LedgerFault::NegativeMass => {
                        (&staged.particle_buffer, 24, (-1.0_f32).to_bits())
                    }
                    LedgerFault::ClearStops => unreachable!("handled above"),
                };
                let fault = ctx
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("injected ledger transition fault"),
                        contents: bytemuck::bytes_of(&bits),
                        usage: wgpu::BufferUsages::COPY_SRC,
                    });
                encoder.copy_buffer_to_buffer(&fault, 0, buffer, offset, 4);
            }
            None => {}
        }
        self.audit.encode(
            ctx,
            &[
                &original.particle_buffer,
                &released.particle_buffer,
                payload.unwrap_or(&self.empty),
                &staged.particle_buffer,
                transport.map_or(&self.empty, |step| &step.stop_words),
                transport.map_or(&self.empty, |step| &step.status),
                &self.state,
                &params,
            ],
            64,
            encoder,
        );
        Ok(params)
    }

    pub(super) fn encode_publish(
        &self,
        ctx: &GpuContext,
        staged: &ParticleBuffers,
        original: &ParticleBuffers,
        params: &wgpu::Buffer,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), TimeLoopError> {
        let count =
            u32::try_from(original.capacity()).map_err(|_| TimeLoopError::MassLedgerRange)?;
        self.publish.encode(
            ctx,
            &[
                &self.state,
                &staged.particle_buffer,
                &original.particle_buffer,
                params,
            ],
            count,
            encoder,
        );
        Ok(())
    }

    async fn download(
        &self,
        ctx: &GpuContext,
        step: u32,
    ) -> Result<DeviceInventory, TimeLoopError> {
        let inventory =
            download_buffer_typed::<DeviceInventory>(ctx, &self.state, 1, "forward ledger")
                .await?
                .pop()
                .ok_or(TimeLoopError::MassLedgerUnavailable)?;
        inventory.status.validate(step)?;
        Ok(inventory)
    }

    pub(super) async fn require_success(
        &self,
        ctx: &GpuContext,
        step: u32,
    ) -> Result<(), TimeLoopError> {
        download_buffer_typed::<LedgerStatus>(ctx, &self.state, 1, "forward ledger status")
            .await?
            .pop()
            .ok_or(TimeLoopError::MassLedgerUnavailable)?
            .validate(step)
    }
}

impl ForwardTimeLoopDriver {
    /// Read the last committed production ledger boundary in the stable v1
    /// observation format. Time is relative to the configured start, in seconds.
    ///
    /// # Errors
    /// Returns an error for a rejected run, failed readback, or missing boundary.
    pub async fn download_mass_ledger_step(&self) -> Result<LedgerStep, TimeLoopError> {
        let step = self
            .latest_submitted_ledger_step
            .ok_or(TimeLoopError::MassLedgerUnavailable)?;
        let inventory = self.mass_ledger.download(&self.gpu_context, step).await?;
        let time_seconds = u64::from(step) * self.config.timestep_seconds.unsigned_abs();
        let terminal = self.is_terminal_ledger_step(step)?;
        let species = (0..self.ledger_species_count())
            .map(|slot| {
                let before = f64::from(inventory.last.before[slot]);
                let injection = f64::from(inventory.last.released[slot]);
                let sample = f64::from(inventory.last.sample[slot]);
                let exits = f64::from(inventory.last.horizontal_exit[slot]);
                let airborne = f64::from(inventory.last.airborne[slot]);
                let row = |phase, transfer_kg, airborne_kg| LedgerObservation {
                    phase,
                    transfer_kg,
                    airborne_kg,
                };
                let mut observations = vec![
                    row(LedgerPhase::Wet, 0.0, before),
                    row(LedgerPhase::Release, injection, sample),
                    row(LedgerPhase::Sample, 0.0, sample),
                ];
                if !terminal {
                    observations.extend([
                        row(LedgerPhase::HorizontalExit, exits, airborne),
                        row(LedgerPhase::Dry, 0.0, airborne),
                        row(LedgerPhase::MaximumAgeExit, 0.0, airborne),
                        row(LedgerPhase::MinimumMassExit, 0.0, airborne),
                        row(LedgerPhase::MinimumMassAndMaximumAgeExit, 0.0, airborne),
                    ]);
                }
                observations.push(row(LedgerPhase::Close, 0.0, airborne));
                observations
            })
            .collect();
        Ok(LedgerStep {
            time_seconds,
            species,
        })
    }

    /// Download the committed species and total inventories in kilograms.
    /// This is an explicit output boundary.
    ///
    /// # Errors
    /// Returns an error for a rejected run, failed readback, or missing boundary.
    pub async fn download_mass_ledger_balance(&self) -> Result<LedgerBalance, TimeLoopError> {
        let step = self
            .latest_submitted_ledger_step
            .ok_or(TimeLoopError::MassLedgerUnavailable)?;
        let inventory = self.mass_ledger.download(&self.gpu_context, step).await?;
        let species: Vec<_> = (0..self.ledger_species_count())
            .map(|slot| MassInventory {
                injected_kg: f64::from(inventory.injected[slot]),
                airborne_kg: f64::from(inventory.airborne[slot]),
                horizontal_exit_kg: f64::from(inventory.horizontal_exit[slot]),
                wet_kg: f64::from(inventory.wet[slot]),
                dry_kg: f64::from(inventory.dry[slot]),
                decayed_kg: f64::from(inventory.decayed[slot]),
                maximum_age_exit_kg: f64::from(inventory.age_exit[slot]),
                minimum_mass_exit_kg: f64::from(inventory.mass_exit[slot]),
                minimum_mass_and_maximum_age_exit_kg: f64::from(inventory.age_and_mass_exit[slot]),
            })
            .collect();
        let total = species
            .iter()
            .copied()
            .fold(MassInventory::default(), MassInventory::sum);
        Ok(LedgerBalance { species, total })
    }

    pub(super) fn ledger_species_count(&self) -> usize {
        self.config
            .transport_settling
            .as_ref()
            .map_or(0, |route| route.species.len())
            .min(MAX_SPECIES)
    }

    pub(super) fn is_terminal_ledger_step(&self, index: u32) -> Result<bool, TimeLoopError> {
        let duration = self
            .config
            .timestep_seconds
            .checked_mul(i64::from(index) + 1)
            .ok_or(TimeLoopError::MassLedgerRange)?;
        Ok(duration > self.ledger_duration_seconds)
    }

    pub(super) async fn require_forward_ledger_success(&self) -> Result<(), TimeLoopError> {
        if let Some(step) = self.latest_submitted_ledger_step {
            self.mass_ledger
                .require_success(&self.gpu_context, step)
                .await?;
        }
        Ok(())
    }
}
