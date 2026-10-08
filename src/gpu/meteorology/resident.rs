//! Device-resident query composition for #171; interpolation science stays in #87–#89.
//!
//! Sources own geometry/field uploads. Batches own timestep query/status resources;
//! prepared samples borrow both through their last consumer. All encode methods use
//! the caller's encoder. Only `observe_status` performs an explicit final D2H.

use super::{
    create_horizontal_output_buffer, create_horizontal_uniform_buffer,
    create_temporal_bracket_buffers, create_temporal_output_buffer, create_temporal_uniform_buffer,
    encode_horizontal_samples, encode_temporal_blend, CanonicalGpuField, FieldId, FieldTime,
    GpuContext, HorizontalGrid, HorizontalQueryBuffers, HorizontalResources, HorizontalStaggering,
    MeteorologyCompositionError, MeteorologyCompositionKernels, MeteorologyStage,
    MeteorologyStageRecord, MeteorologyTimeSelection, SignConvention, TemporalBracketMetadata,
    TemporalResources, VerticalStaggering, VerticalTransformProvenance,
};
use crate::gpu::{
    buffers::ParticleBuffers,
    vertical::{encode_vertical_batch, VerticalBatchBuffers},
};
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use wgpu::util::DeviceExt;

/// Producer-independent v1 lane: signed cells are authoritative; height is metres AGL.
/// Fractions remain separate, including when the cell cannot be represented in f32.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct ResidentQueryLane {
    pub(crate) cell_x: i32,
    pub(crate) cell_y: i32,
    pub(crate) fraction_x: f32,
    pub(crate) fraction_y: f32,
    pub(crate) height_agl_m: f32,
    pub(crate) active: u32,
    padding: [u32; 2],
}

/// Stable status ABI, stored as u32: inactive=0, pending=1, valid=2,
/// signed-cell=3, fraction=4, nonfinite=5, domain=6, geometry(#118)=7,
/// corner=8, sampled-result=9. Word zero of the resource is the shared fatal bit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidentStatus {
    /// Any invalid active lane makes the whole scientific batch fatal.
    pub fatal: bool,
    /// One reason per capacity lane, including explicit inactive lanes.
    pub lanes: Vec<u32>,
}

impl ResidentStatus {
    /// Reject deferred device failure at the final control-plane checkpoint.
    /// # Errors
    /// Fails for fatal or incompletely produced active status.
    pub fn require_success(&self) -> Result<(), MeteorologyCompositionError> {
        if self.fatal || self.lanes.iter().any(|s| !matches!(s, 0 | 2)) {
            return Err(MeteorologyCompositionError::Incompatible(
                "resident query batch failed",
            ));
        }
        Ok(())
    }
}

/// Timestep/intermediate-query owner, independent of the particle storage ABI.
pub struct ResidentQueryBatch<'ctx> {
    context: &'ctx GpuContext,
    grid: HorizontalGrid,
    capacity: u32,
    active_count: u32,
    lanes: wgpu::Buffer,
    status: wgpu::Buffer,
}

/// Narrow same-context producer write target; only the owning batch lends it.
/// Producers write every capacity lane and must use zero active state outside the prefix.
pub(crate) struct ResidentQueryWriteTarget<'a> {
    pub(crate) lanes: &'a wgpu::Buffer,
    pub(crate) capacity: u32,
    pub(crate) active_count: u32,
    pub(crate) context: &'a GpuContext,
}

fn incompatible(reason: &'static str) -> MeteorologyCompositionError {
    MeteorologyCompositionError::Incompatible(reason)
}

fn storage(
    ctx: &GpuContext,
    words: u64,
    label: &str,
) -> Result<wgpu::Buffer, MeteorologyCompositionError> {
    let bytes = words
        .checked_mul(4)
        .ok_or_else(|| incompatible("resident size overflow"))?;
    let limits = ctx.device.limits();
    if bytes == 0
        || bytes
            > limits
                .max_buffer_size
                .min(u64::from(limits.max_storage_buffer_binding_size))
    {
        return Err(incompatible("resident storage size exceeds device limits"));
    }
    Ok(ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }))
}

// Packed resident buffers must not narrow the existing per-plane #76 capability.
// Unsupported aggregate sizes remain an explicit resident preparation error.
pub(super) fn upload_source_columns(
    ctx: &GpuContext,
    columns: &[Vec<f32>],
    label: &str,
) -> Result<Option<wgpu::Buffer>, MeteorologyCompositionError> {
    let words = columns
        .iter()
        .try_fold(0_usize, |count, column| count.checked_add(column.len()))
        .ok_or_else(|| incompatible("resident source size overflow"))?;
    let bytes = (words as u64).checked_mul(4);
    let limits = ctx.device.limits();
    let limit = limits
        .max_buffer_size
        .min(u64::from(limits.max_storage_buffer_binding_size));
    if words == 0 || words > u32::MAX as usize || bytes.is_none_or(|bytes| bytes > limit) {
        return Ok(None);
    }
    let buffer = storage(ctx, words as u64, label)?;
    let values: Vec<_> = columns.iter().flatten().copied().collect();
    ctx.queue
        .write_buffer(&buffer, 0, bytemuck::cast_slice(&values));
    Ok(Some(buffer))
}

impl<'ctx> ResidentQueryBatch<'ctx> {
    /// Allocate a cell-center query/status batch with explicit capacity and active prefix.
    /// No positions or dynamic geometry are inspected on the host.
    /// # Errors
    /// Rejects invalid grid, counts or resource limits.
    pub fn new(
        ctx: &'ctx GpuContext,
        grid: &HorizontalGrid,
        capacity: u32,
        active_count: u32,
    ) -> Result<Self, MeteorologyCompositionError> {
        crate::meteorology::horizontal::validate_horizontal_grid(grid)?;
        if active_count > capacity
            || grid.nx > i32::MAX as usize
            || grid.ny > i32::MAX as usize
            || grid
                .nx
                .checked_mul(grid.ny)
                .is_none_or(|n| n > u32::MAX as usize)
        {
            return Err(incompatible("resident grid/count range"));
        }
        Ok(Self {
            context: ctx,
            grid: grid.clone(),
            capacity,
            active_count,
            lanes: storage(ctx, u64::from(capacity) * 8, "resident split queries")?,
            status: storage(
                ctx,
                u64::from(capacity) + 1,
                "resident shared and lane status",
            )?,
        })
    }

    /// Capacity is distinct from the host-known dispatch prefix.
    #[must_use]
    pub const fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Number of eligible prefix lanes; inactive flags inside the prefix remain non-errors.
    #[must_use]
    pub const fn active_count(&self) -> u32 {
        self.active_count
    }

    /// Change the control-plane prefix before preparing the next batch use.
    /// # Errors
    /// Rejects counts beyond capacity.
    pub fn set_active_count(&mut self, count: u32) -> Result<(), MeteorologyCompositionError> {
        if count > self.capacity {
            return Err(incompatible("resident active prefix exceeds capacity"));
        }
        self.active_count = count;
        Ok(())
    }

    pub(crate) fn write_target(&self) -> ResidentQueryWriteTarget<'_> {
        ResidentQueryWriteTarget {
            lanes: &self.lanes,
            capacity: self.capacity,
            active_count: self.active_count,
            context: self.context,
        }
    }

    /// Reset status in the caller encoder before a crate-local intermediate producer
    /// (such as #112) writes the same typed query ABI. Does not initialize positions.
    pub(crate) fn encode_reset(
        &self,
        ctx: &GpuContext,
        kernels: &ResidentQueryKernels<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<ResidentQueryWriteTarget<'_>, MeteorologyCompositionError> {
        self.validate(ctx, kernels)?;
        let params = uniform(ctx, &[self.capacity, self.active_count, 0, 0]);
        kernels
            .reset
            .encode(ctx, &[&self.status, &params], self.capacity, encoder);
        Ok(self.write_target())
    }

    /// Reset shared/lane status on-device, then copy split particle coordinates on-device.
    /// Does not submit, wait, map or read back. Particle height is already AGL in v1.
    /// # Errors
    /// Rejects context, buffer-size and prefix mismatches before encoding anything.
    pub fn encode_particles(
        &self,
        ctx: &GpuContext,
        particles: &ParticleBuffers,
        kernels: &ResidentQueryKernels<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), MeteorologyCompositionError> {
        self.validate(ctx, kernels)?;
        if !particles.belongs_to(ctx)
            || particles.capacity() < self.capacity as usize
            || particles.particle_count() != self.active_count as usize
            || particles.particle_buffer.size()
                < u64::from(self.capacity) * crate::particles::Particle::GPU_SIZE as u64
        {
            return Err(incompatible(
                "resident particle owner/capacity/prefix mismatch",
            ));
        }
        let target = self.encode_reset(ctx, kernels, encoder)?;
        let params = uniform(
            target.context,
            &[target.capacity, target.active_count, 0, 0],
        );
        kernels.producer.encode(
            ctx,
            &[&particles.particle_buffer, target.lanes, &params],
            target.capacity,
            encoder,
        );
        Ok(())
    }

    fn validate(
        &self,
        ctx: &GpuContext,
        kernels: &ResidentQueryKernels<'_>,
    ) -> Result<(), MeteorologyCompositionError> {
        if !std::ptr::eq(self.context, ctx) || !std::ptr::eq(kernels.context, ctx) {
            return Err(incompatible("resident batch/kernels context mismatch"));
        }
        Ok(())
    }

    /// Observe status only at the final explicit validation/output boundary.
    /// The caller must call `require_success` before accepting scientific output.
    /// # Errors
    /// Rejects context mismatch and readback failures.
    pub async fn observe_status(
        &self,
        ctx: &GpuContext,
    ) -> Result<ResidentStatus, MeteorologyCompositionError> {
        if !std::ptr::eq(self.context, ctx) {
            return Err(incompatible("status context mismatch"));
        }
        let words = crate::gpu::download_buffer_typed::<u32>(
            ctx,
            &self.status,
            self.capacity as usize + 1,
            "final resident status",
        )
        .await
        .map_err(|_| incompatible("resident status readback failed"))?;
        Ok(ResidentStatus {
            fatal: words[0] != 0,
            lanes: words[1..].to_vec(),
        })
    }
}

struct QueryPass {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl QueryPass {
    // This helper only builds the issue-owned query/status/fixture layouts.
    fn new(ctx: &GpuContext, label: &str, shader: &str, read_only: &[bool]) -> Self {
        let mut entries: Vec<_> = read_only
            .iter()
            .enumerate()
            .map(|(i, read_only)| wgpu::BindGroupLayoutEntry {
                binding: u32::try_from(i).expect("query bindings fit u32"),
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: *read_only,
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: u32::try_from(read_only.len()).expect("query bindings fit u32"),
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let layout = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &entries,
            });
        let shader = ctx.load_shader(label, shader);
        let pipeline = ctx.create_compute_pipeline(label, &shader, "main", &[&layout]);
        Self { layout, pipeline }
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        buffers: &[&wgpu::Buffer],
        count: u32,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(i, buffer)| wgpu::BindGroupEntry {
                binding: u32::try_from(i).expect("query bindings fit u32"),
                resource: buffer.as_entire_binding(),
            })
            .collect();
        let bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("resident query stage"),
            layout: &self.layout,
            entries: &entries,
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("resident query stage"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bg, &[]);
        crate::gpu::dispatch_1d(&mut pass, count, 64);
    }
}

fn uniform(ctx: &GpuContext, words: &[u32]) -> wgpu::Buffer {
    ctx.device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("resident control-plane parameters"),
            contents: bytemuck::cast_slice(words),
            usage: wgpu::BufferUsages::UNIFORM,
        })
}

/// Context-lifetime producer/adapter/status pipelines; no second interpolation sampler.
pub struct ResidentQueryKernels<'ctx> {
    context: &'ctx GpuContext,
    reset: QueryPass,
    producer: QueryPass,
    adapter: QueryPass,
    check: QueryPass,
    consumer: QueryPass,
}

impl<'ctx> ResidentQueryKernels<'ctx> {
    /// Build issue-owned pipelines once, outside production encoding.
    /// # Errors
    /// Surfaces scoped shader/pipeline errors.
    pub fn new(ctx: &'ctx GpuContext) -> Result<Self, MeteorologyCompositionError> {
        ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = Self {
            context: ctx,
            reset: QueryPass::new(
                ctx,
                "resident reset",
                include_str!("../../shaders/resident_status_reset.wgsl"),
                &[false],
            ),
            producer: QueryPass::new(
                ctx,
                "particle query producer",
                include_str!("../../shaders/particle_query.wgsl"),
                &[true, false],
            ),
            adapter: QueryPass::new(
                ctx,
                "resident #87 adapter",
                include_str!("../../shaders/resident_query_adapter.wgsl"),
                &[true, false, true, true, false, false, false],
            ),
            check: QueryPass::new(
                ctx,
                "resident result status",
                include_str!("../../shaders/resident_sample_status.wgsl"),
                &[true, false],
            ),
            consumer: QueryPass::new(
                ctx,
                "resident fixture consumer",
                include_str!("../../shaders/resident_fixture_consumer.wgsl"),
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
        Ok(result)
    }
}

/// Unambiguous scalar batch metadata: `values[lane * lane_stride + component]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidentSampleMetadata {
    /// Allocated query lanes.
    pub capacity: u32,
    /// Eligible active prefix (per-lane flags can deactivate lanes within it).
    pub active_count: u32,
    /// v1 supports exactly one scalar component per field.
    pub component_count: u32,
    /// Distance in f32 elements between lanes; exactly one in v1.
    pub lane_stride: u32,
    /// Canonical field identity.
    pub field_id: FieldId,
    /// Canonical unit, sign and staggering are retained explicitly.
    pub unit: String,
    /// Canonical direction/sign.
    pub sign: SignConvention,
    /// Canonical horizontal grid identity.
    pub horizontal_grid: HorizontalGrid,
    /// Source cell-center staggering.
    pub horizontal_staggering: HorizontalStaggering,
    /// Source model-center or surface staggering.
    pub vertical_staggering: VerticalStaggering,
    /// Shared control-plane selection for every lane.
    pub time: MeteorologyTimeSelection,
    /// Resolved #89 bracket, including original indices and weights.
    pub temporal_bracket: Option<TemporalBracketMetadata>,
    /// Selected source identities, in execution order.
    pub source_snapshot_sha256: Vec<String>,
    /// Selected source field times, in execution order.
    pub source_times: Vec<FieldTime>,
    /// #30 geometry identities, in source order.
    pub geometry_identity: Vec<String>,
    /// Full #30 provenance for downstream verification.
    pub geometry_provenance: Vec<VerticalTransformProvenance>,
}

struct ResidentSpatial {
    horizontal: Vec<HorizontalResources>,
    vertical: VerticalBatchBuffers,
    params: wgpu::Buffer,
}

impl super::SourceMember<'_> {
    fn resident_buffers(
        &self,
    ) -> Result<(&wgpu::Buffer, &wgpu::Buffer), MeteorologyCompositionError> {
        let unsupported = || {
            MeteorologyCompositionError::Unsupported(
                "resident source aggregate exceeds device storage or index limits",
            )
        };
        let values = self.resident_values.as_ref().ok_or_else(unsupported)?;
        let heights = match &self.runtime {
            Some(runtime) => runtime.resident_heights.as_ref().ok_or_else(unsupported)?,
            None => values,
        };
        Ok((values, heights))
    }
}

/// Prepared source/query borrow: intermediate values stay resident through the last consumer.
pub struct PreparedResidentSample<'a, 'ctx, 'query> {
    source: &'a CanonicalGpuField<'ctx>,
    batch: &'a mut ResidentQueryBatch<'query>,
    indices: Vec<usize>,
    spatial: Vec<ResidentSpatial>,
    temporal: Option<TemporalResources>,
    metadata: ResidentSampleMetadata,
    stages: Vec<MeteorologyStageRecord>,
}

impl ResidentSpatial {
    #[allow(clippy::too_many_arguments)]
    fn encode(
        &self,
        ctx: &GpuContext,
        batch: &ResidentQueryBatch<'_>,
        member: &super::SourceMember<'_>,
        index: usize,
        kernels: &MeteorologyCompositionKernels<'_>,
        resident: &ResidentQueryKernels<'_>,
        stages: &mut Vec<MeteorologyStageRecord>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), MeteorologyCompositionError> {
        let bytes = u64::from(batch.capacity) * 4;
        let (values, heights) = member.resident_buffers()?;
        resident.adapter.encode(
            ctx,
            &[
                &batch.lanes,
                &self.horizontal[0].queries.buffer,
                heights,
                values,
                &self.vertical.heights,
                &self.vertical.queries,
                &batch.status,
                &self.params,
            ],
            batch.capacity,
            encoder,
        );
        for (level, (h, plane)) in self.horizontal.iter().zip(&member.planes).enumerate() {
            if level != 0 {
                encoder.copy_buffer_to_buffer(
                    &self.horizontal[0].queries.buffer,
                    0,
                    &h.queries.buffer,
                    0,
                    u64::from(batch.capacity) * 32,
                );
            }
            encode_horizontal_samples(
                ctx,
                plane,
                &h.queries,
                &h.output,
                &h.uniforms,
                &kernels.horizontal,
                encoder,
            )?;
            stages.push(MeteorologyStageRecord {
                stage: MeteorologyStage::Horizontal,
                source_index: Some(index),
                plane_index: Some(level),
            });
            if member.runtime.is_some() {
                encoder.copy_buffer_to_buffer(
                    &h.output.buffer,
                    0,
                    &self.vertical.values,
                    level as u64 * bytes,
                    bytes,
                );
            }
        }
        if member.runtime.is_some() {
            encode_vertical_batch(ctx, &self.vertical, &kernels.vertical, encoder);
            stages.push(MeteorologyStageRecord {
                stage: MeteorologyStage::Vertical,
                source_index: Some(index),
                plane_index: None,
            });
        }
        Ok(())
    }

    fn new(
        ctx: &GpuContext,
        batch: &ResidentQueryBatch<'_>,
        member: &super::SourceMember<'_>,
    ) -> Result<Self, MeteorologyCompositionError> {
        member.resident_buffers()?;
        let count = batch.capacity as usize;
        let levels =
            u32::try_from(member.planes.len()).map_err(|_| incompatible("resident level count"))?;
        if levels == 0
            || (member.runtime.is_some() && levels < 2)
            || (member.runtime.is_none() && levels != 1)
        {
            return Err(incompatible("resident unsupported scalar plane layout"));
        }
        let horizontal = member
            .planes
            .iter()
            .map(|plane| {
                Ok(HorizontalResources {
                    queries: HorizontalQueryBuffers::resident(ctx, plane, count)?,
                    output: create_horizontal_output_buffer(ctx, count)?,
                    uniforms: create_horizontal_uniform_buffer(ctx, &batch.grid, count)?,
                })
            })
            .collect::<Result<Vec<_>, MeteorologyCompositionError>>()?;
        Ok(Self {
            horizontal,
            vertical: VerticalBatchBuffers::new(ctx, levels.max(2), batch.capacity)?,
            params: uniform(
                ctx,
                &[
                    batch.capacity,
                    batch.active_count,
                    u32::try_from(batch.grid.nx).map_err(|_| incompatible("nx"))?,
                    u32::try_from(batch.grid.ny).map_err(|_| incompatible("ny"))?,
                    u32::from(member.planes[0].is_periodic_x()),
                    levels,
                    u32::from(member.runtime.is_some()),
                    0,
                ],
            ),
        })
    }
}

impl<'ctx> CanonicalGpuField<'ctx> {
    /// Prepare batch resources using only immutable source/control metadata.
    /// Dynamic query validation and per-lane column selection run on-device.
    /// # Errors
    /// Rejects owner/grid/time mismatches and unsupported class, accumulated or interface fields.
    pub fn prepare_resident<'a, 'query>(
        &'a self,
        ctx: &GpuContext,
        batch: &'a mut ResidentQueryBatch<'query>,
        time: MeteorologyTimeSelection,
    ) -> Result<PreparedResidentSample<'a, 'ctx, 'query>, MeteorologyCompositionError> {
        if !std::ptr::eq(self.context, ctx)
            || !std::ptr::eq(batch.context, ctx)
            || batch.grid != self.members[0].snapshot.horizontal_grid
        {
            return Err(incompatible(
                "resident source/query grid or context mismatch",
            ));
        }
        let field = &self.members[0].field;
        if !matches!(
            time,
            MeteorologyTimeSelection::Instantaneous(_) | MeteorologyTimeSelection::Static
        ) || field.axis_order.contains(&crate::meteorology::Axis::Class)
            || !matches!(
                field.vertical_staggering,
                VerticalStaggering::NotApplicable | VerticalStaggering::LevelCenter
            )
        {
            return Err(MeteorologyCompositionError::Unsupported("resident v1 supports scalar surface/model-center fields with static/instantaneous time"));
        }
        let (indices, bracket, _, _) = self.select_sources(time)?;
        let count = batch.capacity as usize;
        let mut spatial = Vec::new();
        for index in &indices {
            spatial.push(ResidentSpatial::new(ctx, batch, &self.members[*index])?);
        }
        let temporal = if let Some(bracket) = &bracket {
            let mut bracket = bracket.clone();
            bracket.element_count = count;
            Some(TemporalResources {
                buffers: create_temporal_bracket_buffers(
                    ctx,
                    &vec![0.0; count],
                    &vec![0.0; count],
                    bracket.source_timestamps.lower_epoch_seconds,
                    bracket.source_timestamps.upper_epoch_seconds,
                )?,
                uniforms: create_temporal_uniform_buffer(ctx, &bracket)?,
                output: create_temporal_output_buffer(ctx, count)?,
                bracket,
            })
        } else {
            None
        };
        let selected: Vec<_> = indices.iter().map(|i| &self.members[*i]).collect();
        let metadata = ResidentSampleMetadata {
            capacity: batch.capacity,
            active_count: batch.active_count,
            component_count: 1,
            lane_stride: 1,
            field_id: self.field_id,
            unit: serde_json::to_value(field.unit)?
                .as_str()
                .ok_or_else(|| incompatible("unit encoding"))?
                .to_owned(),
            sign: field.sign,
            horizontal_grid: batch.grid.clone(),
            horizontal_staggering: field.horizontal_staggering,
            vertical_staggering: field.vertical_staggering,
            time,
            temporal_bracket: bracket.as_ref().map(|b| TemporalBracketMetadata {
                lower_index: b.lower_index,
                upper_index: b.upper_index,
                source_timestamps: b.source_timestamps,
                application: b.application,
                weights: b.weights,
            }),
            source_snapshot_sha256: selected.iter().map(|s| s.snapshot_sha256.clone()).collect(),
            source_times: selected.iter().map(|s| s.field.time.clone()).collect(),
            geometry_identity: selected
                .iter()
                .filter_map(|s| s.runtime.as_ref().map(|r| r.identity.clone()))
                .collect(),
            geometry_provenance: selected
                .iter()
                .filter_map(|s| s.runtime.as_ref().map(|r| r.provenance.clone()))
                .collect(),
        };
        Ok(PreparedResidentSample {
            source: self,
            batch,
            indices,
            spatial,
            temporal,
            metadata,
            stages: Vec::new(),
        })
    }
}

/// Typed sampled batch, carrying mandatory shared/lane device status alongside values.
pub struct EncodedResidentSample<'a> {
    /// Scalar lane-aligned f32 values, valid only when shared status permits consumption.
    pub values: &'a wgpu::Buffer,
    /// Scientific status binding: shared fatal word followed by one lane reason per capacity.
    pub status: &'a wgpu::Buffer,
    /// Exact field/grid/time/layout provenance.
    pub metadata: &'a ResidentSampleMetadata,
    /// Actual canonical sampler encode sequence.
    pub stages: &'a [MeteorologyStageRecord],
    context: &'a GpuContext,
}

impl PreparedResidentSample<'_, '_, '_> {
    /// Encode the complete resident particle producer and sampling chain in one encoder.
    /// This public entry guarantees reset/production precede every adapter and sampler.
    /// # Errors
    /// Rejects static owner/count/grid/time mismatches; discard the encoder on error.
    pub fn encode_from_particles<'a>(
        &'a mut self,
        ctx: &GpuContext,
        particles: &ParticleBuffers,
        kernels: &MeteorologyCompositionKernels<'_>,
        resident: &ResidentQueryKernels<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<EncodedResidentSample<'a>, MeteorologyCompositionError> {
        self.batch
            .encode_particles(ctx, particles, resident, encoder)?;
        self.encode(ctx, kernels, resident, encoder)
    }

    /// Encode adaptation, canonical #87/#88/#89 sampling and final finite-result status.
    /// Producer/reset must precede this in the same caller encoder. No internal submission/D2H.
    /// # Errors
    /// Rejects contexts before work; on any error discard the encoder.
    pub(crate) fn encode<'a>(
        &'a mut self,
        ctx: &GpuContext,
        kernels: &MeteorologyCompositionKernels<'_>,
        resident: &ResidentQueryKernels<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<EncodedResidentSample<'a>, MeteorologyCompositionError> {
        self.batch.validate(ctx, resident)?;
        if !std::ptr::eq(kernels.context, ctx) {
            return Err(incompatible("canonical kernels context mismatch"));
        }
        self.stages.clear();
        let bytes = u64::from(self.batch.capacity) * 4;
        for (spatial, index) in self.spatial.iter().zip(&self.indices) {
            spatial.encode(
                ctx,
                self.batch,
                &self.source.members[*index],
                *index,
                kernels,
                resident,
                &mut self.stages,
                encoder,
            )?;
        }
        if let Some(t) = &self.temporal {
            encoder.copy_buffer_to_buffer(
                self.spatial_output(0),
                0,
                &t.buffers.lower_buffer,
                0,
                bytes,
            );
            encoder.copy_buffer_to_buffer(
                self.spatial_output(1),
                0,
                &t.buffers.upper_buffer,
                0,
                bytes,
            );
            encode_temporal_blend(
                ctx,
                &t.buffers,
                &t.output,
                &t.uniforms,
                &t.bracket,
                &kernels.temporal,
                encoder,
            )?;
            self.stages.push(MeteorologyStageRecord {
                stage: MeteorologyStage::Temporal,
                source_index: None,
                plane_index: None,
            });
        }
        let values = self
            .temporal
            .as_ref()
            .map_or_else(|| self.spatial_output(0), |t| &t.output.buffer);
        let params = uniform(ctx, &[self.batch.capacity, self.batch.active_count, 0, 0]);
        resident.check.encode(
            ctx,
            &[values, &self.batch.status, &params],
            self.batch.capacity,
            encoder,
        );
        Ok(EncodedResidentSample {
            values,
            status: &self.batch.status,
            metadata: &self.metadata,
            stages: &self.stages,
            context: self.source.context,
        })
    }

    fn spatial_output(&self, index: usize) -> &wgpu::Buffer {
        let spatial = &self.spatial[index];
        if self.source.members[self.indices[index]].runtime.is_some() {
            &spatial.vertical.output
        } else {
            &spatial.horizontal[0].output.buffer
        }
    }
}

/// Minimal scientific fixture state, intentionally separate from production consumers.
pub struct ResidentFixtureState<'ctx> {
    context: &'ctx GpuContext,
    capacity: u32,
    /// Bind/download only at the final validation boundary.
    pub values: wgpu::Buffer,
}

impl<'ctx> ResidentFixtureState<'ctx> {
    /// Explicit fixture H2D initialization, permitting byte-exact no-mutation proof.
    /// # Errors
    /// Rejects empty or oversized state.
    pub fn upload(
        ctx: &'ctx GpuContext,
        values: &[f32],
    ) -> Result<Self, MeteorologyCompositionError> {
        let capacity = u32::try_from(values.len()).map_err(|_| incompatible("fixture size"))?;
        let buffer = storage(ctx, u64::from(capacity), "resident fixture state")?;
        ctx.queue
            .write_buffer(&buffer, 0, bytemuck::cast_slice(values));
        Ok(Self {
            context: ctx,
            capacity,
            values: buffer,
        })
    }
}

impl EncodedResidentSample<'_> {
    /// Gate all fixture mutation on-device on the shared fatal bit, before any lane writes.
    /// # Errors
    /// Rejects context/capacity mismatches before recording the consumer.
    pub fn encode_fixture_consumer(
        &self,
        ctx: &GpuContext,
        kernels: &ResidentQueryKernels<'_>,
        state: &ResidentFixtureState<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), MeteorologyCompositionError> {
        if !std::ptr::eq(self.context, ctx)
            || !std::ptr::eq(kernels.context, ctx)
            || !std::ptr::eq(state.context, ctx)
            || state.capacity != self.metadata.capacity
        {
            return Err(incompatible("fixture context/capacity mismatch"));
        }
        let params = uniform(
            ctx,
            &[self.metadata.capacity, self.metadata.active_count, 0, 0],
        );
        kernels.consumer.encode(
            ctx,
            &[self.values, self.status, &state.values, &params],
            self.metadata.capacity,
            encoder,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meteorology::{temporal::RequestedSampleTime, Calendar, Snapshot};
    use crate::particles::{Particle, ParticleInit};

    #[test]
    fn test_resident_aggregate_limit_preserves_per_plane_canonical_sampling() {
        // A 64-byte binding fits each 48-byte field plane, but not the 144-byte
        // packed source. A small real device limit avoids a huge allocation fixture.
        let options = crate::gpu::GpuAdapterOptions::from_env();
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&options.to_request_adapter_options()))
                .expect("resident source-limit gate requires WGSL");
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: 64,
            ..wgpu::Limits::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("resident aggregate-limit fixture"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .unwrap();
        let ctx = GpuContext {
            identity: std::sync::Arc::new(()),
            device,
            queue,
            adapter_info: adapter.get_info(),
            fallback_requested: options.force_software_fallback,
        };
        assert_eq!(ctx.device.limits().max_storage_buffer_binding_size, 64);
        let mut s0: Snapshot = serde_json::from_str(include_str!(
            "../../../fixtures/vertical/synthetic-column-v1.json"
        ))
        .unwrap();
        s0.horizontal_grid.nx = 4;
        s0.horizontal_grid.ny = 3;
        for field in &mut s0.fields {
            field.shape[0] = 4;
            field.shape[1] = 3;
            field.values = field
                .values
                .iter()
                .flat_map(|value| vec![*value; 12])
                .collect();
        }
        let mut s1 = s0.clone();
        for field in &mut s1.fields {
            field.time.valid_time_epoch_seconds += 3600;
        }
        let r0 = crate::meteorology::vertical::reconstruct_vertical_geometry(&s0).unwrap();
        let r1 = crate::meteorology::vertical::reconstruct_vertical_geometry(&s1).unwrap();
        let v0 = r0.runtime_view().unwrap();
        let v1 = r1.runtime_view().unwrap();
        let source = CanonicalGpuField::upload(
            &ctx,
            FieldId::Temperature,
            &[&s0, &s1],
            &[Some(v0), Some(v1)],
        )
        .unwrap();
        assert!(source.members.iter().all(|member| {
            member.resident_values.is_none()
                && member.runtime.as_ref().unwrap().resident_heights.is_none()
        }));
        let time = MeteorologyTimeSelection::Instantaneous(RequestedSampleTime::new(
            Calendar::Gregorian,
            s0.fields[0].time.valid_time_epoch_seconds,
        ));
        let mut batch = ResidentQueryBatch::new(&ctx, &s0.horizontal_grid, 1, 1).unwrap();
        assert!(matches!(
            source.prepare_resident(&ctx, &mut batch, time),
            Err(MeteorologyCompositionError::Unsupported(
                "resident source aggregate exceeds device storage or index limits"
            ))
        ));

        let kernels = MeteorologyCompositionKernels::new(&ctx).unwrap();
        let mut plan = source
            .prepare_sample(
                &ctx,
                super::super::MeteorologySampleRequest {
                    xt: 0.0,
                    yt: 0.0,
                    height: Some(super::super::MeteorologyHeight {
                        meters: 0.0,
                        reference: crate::meteorology::VerticalReference::AboveGroundLevel,
                    }),
                    time,
                },
            )
            .unwrap();
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let sample = plan.encode(&ctx, &kernels, &mut encoder).unwrap();
        ctx.queue.submit(Some(encoder.finish()));
        let result = pollster::block_on(crate::gpu::download_buffer_typed::<f32>(
            &ctx,
            sample.values,
            1,
            "final per-plane compatibility result",
        ))
        .unwrap();
        let (_, values) = crate::gpu::vertical::physical_model_column_from_runtime(
            v0,
            FieldId::Temperature,
            &source.members[0].field.values,
            0,
            0,
        )
        .unwrap();
        assert_eq!(result, [values[0]]);
    }

    #[test]
    fn test_resident_nonfinite_source_corner_fails_on_device_without_mutation() {
        assert_eq!(std::mem::size_of::<ResidentQueryLane>(), 32);
        let ctx =
            pollster::block_on(GpuContext::new()).expect("resident corner gate requires WGSL");
        let kernels = MeteorologyCompositionKernels::new(&ctx).unwrap();
        let resident = ResidentQueryKernels::new(&ctx).unwrap();
        let s0: Snapshot = serde_json::from_str(include_str!(
            "../../../fixtures/vertical/synthetic-column-v1.json"
        ))
        .unwrap();
        let mut s1 = s0.clone();
        for f in &mut s1.fields {
            f.time.valid_time_epoch_seconds += 3600;
        }
        let source =
            CanonicalGpuField::upload(&ctx, FieldId::SurfacePressure, &[&s0, &s1], &[None, None])
                .unwrap();
        // Fault injection changes only device validation data after static source validation.
        ctx.queue.write_buffer(
            source.members[0].resident_values.as_ref().unwrap(),
            0,
            &f32::NAN.to_le_bytes(),
        );
        let particle = Particle::new(&ParticleInit {
            cell_x: 0,
            cell_y: 0,
            pos_x: 0.0,
            pos_y: 0.0,
            pos_z: 0.0,
            mass: [1.0, 0.0, 0.0, 0.0],
            release_point: 0,
            class: 0,
            time: 0,
        });
        let particles = ParticleBuffers::from_particles(&ctx, &[particle]);
        let mut batch = ResidentQueryBatch::new(&ctx, &s0.horizontal_grid, 1, 1).unwrap();
        let state = ResidentFixtureState::upload(&ctx, &[-0.0]).unwrap();
        let time = MeteorologyTimeSelection::Instantaneous(RequestedSampleTime::new(
            Calendar::Gregorian,
            1700000000,
        ));
        let mut plan = source.prepare_resident(&ctx, &mut batch, time).unwrap();
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let sample = plan
            .encode_from_particles(&ctx, &particles, &kernels, &resident, &mut encoder)
            .unwrap();
        sample
            .encode_fixture_consumer(&ctx, &resident, &state, &mut encoder)
            .unwrap();
        ctx.queue.submit(Some(encoder.finish()));
        ctx.device.poll(wgpu::Maintain::Wait);
        drop(plan);
        let status = pollster::block_on(batch.observe_status(&ctx)).unwrap();
        assert!(status.fatal);
        assert_eq!(status.lanes, [8]);
        assert!(status.require_success().is_err());
        let bits = pollster::block_on(crate::gpu::download_buffer_typed::<u32>(
            &ctx,
            &state.values,
            1,
            "final corner fixture",
        ))
        .unwrap();
        assert_eq!(bits, [(-0.0_f32).to_bits()]);
    }
}
