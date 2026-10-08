//! Production Petterssen orchestration over canonical #171 scalar U/V/W samplers.
//! No field interpolation, submission, wait or host query materialization occurs here.

use super::meteorology::resident::{
    QueryPass, ResidentQueryBatch, ResidentQueryKernels, ResidentSampleMetadata, ResidentStatus,
};
use super::meteorology::{
    CanonicalGpuField, MeteorologyCompositionError, MeteorologyCompositionKernels,
    MeteorologyTimeSelection,
};
use super::{GpuContext, ParticleBuffers};
use crate::meteorology::HorizontalGrid;
use crate::physics::VelocityToGridScale;
use wgpu::util::DeviceExt;

/// Consumer stages reuse the existing resident layout/pipeline helper.
pub(crate) struct ResidentAdvectionKernels {
    predictor: QueryPass,
    corrector: QueryPass,
    guard: QueryPass,
    commit: QueryPass,
}
impl ResidentAdvectionKernels {
    /// Build the consumer pipelines once per driver, surfacing scoped device errors.
    pub(crate) fn new(ctx: &GpuContext) -> Result<Self, MeteorologyCompositionError> {
        ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let kernels = Self {
            predictor: QueryPass::new(
                ctx,
                "Petterssen predictor query",
                include_str!("../shaders/advection_predictor_query.wgsl"),
                &[true, true, true, false],
            ),
            corrector: QueryPass::new(
                ctx,
                "Petterssen private corrector",
                include_str!("../shaders/advection_corrector.wgsl"),
                &[true, true, false, false],
            ),
            guard: QueryPass::new(
                ctx,
                "Petterssen downstream eligibility",
                include_str!("../shaders/advection_step_guard.wgsl"),
                &[false, true],
            ),
            commit: QueryPass::new(
                ctx,
                "Petterssen timestep commit",
                include_str!("../shaders/advection_step_commit.wgsl"),
                &[true, true, false],
            ),
        };
        let errors: Vec<_> = (0..3)
            .filter_map(|_| pollster::block_on(ctx.device.pop_error_scope()))
            .collect();
        if !errors.is_empty() {
            return Err(MeteorologyCompositionError::ResidentDevice(format!(
                "{errors:?}"
            )));
        }
        Ok(kernels)
    }
}

/// Device-owned timestep values/status, retained through the deferred host checkpoint.
/// The six independent status regions cannot clear each other's fatal word.
#[derive(Clone)]
pub(crate) struct ResidentAdvectionStep {
    pub(crate) status: wgpu::Buffer,
    pub(crate) values: wgpu::Buffer,
    #[cfg(test)]
    pub(crate) queries: wgpu::Buffer,
    params: wgpu::Buffer,
    capacity: u32,
    pub(crate) metadata: Vec<ResidentSampleMetadata>,
}
impl ResidentAdvectionStep {
    /// Allocate and validate explicit timestep resources before recording any scientific stages.
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        ctx: &GpuContext,
        original: &ParticleBuffers,
        staged: &ParticleBuffers,
        fields: &[CanonicalGpuField<'_>; 3],
        grid: &HorizontalGrid,
        signed_dt: f32,
        scale: VelocityToGridScale,
    ) -> Result<Self, MeteorologyCompositionError> {
        let invalid = |reason| MeteorologyCompositionError::Incompatible(reason);
        if !original.belongs_to(ctx)
            || !staged.belongs_to(ctx)
            || original.capacity() != staged.capacity()
            || original.particle_count() != staged.particle_count()
        {
            return Err(invalid(
                "advection particle owners/capacity/prefix mismatch",
            ));
        }
        if !signed_dt.is_finite()
            || !scale.x_grid_per_meter.is_finite()
            || !scale.y_grid_per_meter.is_finite()
            || scale.z_grid_per_meter.to_bits() != 1.0_f32.to_bits()
        {
            return Err(invalid(
                "canonical advection requires finite scales and AGL metres with z scale one",
            ));
        }
        let capacity = u32::try_from(original.capacity())
            .map_err(|_| invalid("advection capacity overflow"))?;
        let active = u32::try_from(original.particle_count())
            .map_err(|_| invalid("advection prefix overflow"))?;
        let top = fields[0].resident_advection_top_m()?;
        let allocation = |words: u64, label| -> Result<wgpu::Buffer, MeteorologyCompositionError> {
            let bytes = words
                .checked_mul(4)
                .ok_or_else(|| invalid("advection size overflow"))?;
            if bytes == 0
                || bytes
                    > ctx.device.limits().max_buffer_size.min(u64::from(
                        ctx.device.limits().max_storage_buffer_binding_size,
                    ))
            {
                return Err(invalid("advection resources exceed device limits"));
            }
            Ok(ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }))
        };
        let words = [
            capacity,
            active,
            u32::try_from(grid.nx).map_err(|_| invalid("grid x overflow"))?,
            u32::try_from(grid.ny).map_err(|_| invalid("grid y overflow"))?,
            signed_dt.to_bits(),
            scale.x_grid_per_meter.to_bits(),
            scale.y_grid_per_meter.to_bits(),
            top.to_bits(),
        ];
        let result = Self {
            #[cfg(test)]
            queries: allocation(16 * u64::from(capacity), "Petterssen final query evidence")?,
            status: allocation(
                6 * (u64::from(capacity) + 1) + 1,
                "Petterssen six-stage status",
            )?,
            values: allocation(6 * u64::from(capacity), "Petterssen six sampled components")?,
            params: ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Petterssen control parameters"),
                    contents: bytemuck::cast_slice(&words),
                    usage: wgpu::BufferUsages::UNIFORM,
                }),
            capacity,
            metadata: Vec::with_capacity(6),
        };
        Ok(result)
    }

    /// Encode both canonical Petterssen stages into private state without submitting.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode(
        ctx: &GpuContext,
        original: &ParticleBuffers,
        staged: &ParticleBuffers,
        fields: &[CanonicalGpuField<'_>; 3],
        grid: &HorizontalGrid,
        times: [MeteorologyTimeSelection; 2],
        signed_dt: f32,
        scale: VelocityToGridScale,
        kernels: &ResidentAdvectionKernels,
        sampling: &MeteorologyCompositionKernels<'_>,
        resident: &ResidentQueryKernels<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<Self, MeteorologyCompositionError> {
        let mut result = Self::prepare(ctx, original, staged, fields, grid, signed_dt, scale)?;
        let capacity = result.capacity;
        let active = u32::try_from(original.particle_count())
            .map_err(|_| MeteorologyCompositionError::Incompatible("advection prefix overflow"))?;
        // Transaction state is an explicit D2D snapshot. Scientific particles remain unchanged until commit.
        encoder.copy_buffer_to_buffer(
            &original.particle_buffer,
            0,
            &staged.particle_buffer,
            0,
            original.particle_buffer.size(),
        );
        encoder.clear_buffer(&result.status, 0, None);
        for (time_stage, time) in times.into_iter().enumerate() {
            for (component, field) in fields.iter().enumerate() {
                let index = time_stage * 3 + component;
                let mut batch = ResidentQueryBatch::new(ctx, grid, capacity, active)?;
                if time_stage == 0 {
                    batch.encode_particles(ctx, original, resident, encoder)?;
                } else {
                    let target = batch.encode_reset(ctx, resident, encoder)?;
                    kernels.predictor.encode(
                        ctx,
                        &[
                            &original.particle_buffer,
                            &result.values,
                            &result.status,
                            target.lanes,
                            &result.params,
                        ],
                        capacity,
                        encoder,
                    );
                }
                #[cfg(test)]
                if component == 0 {
                    encoder.copy_buffer_to_buffer(
                        batch.write_target().lanes,
                        0,
                        &result.queries,
                        time_stage as u64 * u64::from(capacity) * 32,
                        u64::from(capacity) * 32,
                    );
                }
                let mut plan = field.prepare_resident(ctx, &mut batch, time)?;
                let sample = plan.encode(ctx, sampling, resident, encoder)?;
                result.metadata.push(sample.metadata.clone());
                encoder.copy_buffer_to_buffer(
                    sample.values,
                    0,
                    &result.values,
                    index as u64 * u64::from(capacity) * 4,
                    u64::from(capacity) * 4,
                );
                encoder.copy_buffer_to_buffer(
                    sample.status,
                    0,
                    &result.status,
                    index as u64 * (u64::from(capacity) + 1) * 4,
                    (u64::from(capacity) + 1) * 4,
                );
            }
        }
        kernels.corrector.encode(
            ctx,
            &[
                &original.particle_buffer,
                &result.values,
                &result.status,
                &staged.particle_buffer,
                &result.params,
            ],
            capacity,
            encoder,
        );
        // A separate dispatch observes failures from every workgroup before downstream physics.
        kernels.guard.encode(
            ctx,
            &[&staged.particle_buffer, &result.status, &result.params],
            capacity,
            encoder,
        );
        Ok(result)
    }

    /// Publish private downstream results only after whole-step device eligibility.
    pub(crate) fn encode_commit(
        &self,
        ctx: &GpuContext,
        staged: &ParticleBuffers,
        original: &ParticleBuffers,
        kernels: &ResidentAdvectionKernels,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        kernels.commit.encode(
            ctx,
            &[
                &staged.particle_buffer,
                &self.status,
                &original.particle_buffer,
                &self.params,
            ],
            self.capacity,
            encoder,
        );
    }

    /// Final control-plane observation only; no query or value leaves the device between stages.
    pub(crate) async fn require_success(
        &self,
        ctx: &GpuContext,
    ) -> Result<(), MeteorologyCompositionError> {
        let words = super::download_buffer_typed::<u32>(
            ctx,
            &self.status,
            6 * (self.capacity as usize + 1) + 1,
            "Petterssen final status",
        )
        .await
        .map_err(|_| {
            MeteorologyCompositionError::Incompatible("Petterssen status readback failed")
        })?;
        for region in words[..words.len() - 1].chunks_exact(self.capacity as usize + 1) {
            ResidentStatus {
                fatal: region[0] != 0,
                lanes: region[1..].to_vec(),
            }
            .require_success()?;
        }
        if words.last() != Some(&0) {
            return Err(MeteorologyCompositionError::Incompatible(
                "Petterssen corrected arithmetic failed",
            ));
        }
        Ok(())
    }
}
