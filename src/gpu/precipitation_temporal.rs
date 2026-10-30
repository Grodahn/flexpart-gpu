//! Precipitation-specific time sampling of #207 point members (issue #119).
//!
//! Ports unchanged FLEXPART 11.1 `interpol_mod.f90:1302-1316,1549-1550`.
//! #87 owns all spatial arithmetic. The caller owns encoding, submission,
//! device error scopes and completion. Every consumer must check the shared
//! fatal status before publishing or using the batch; encoding is not success.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::meteorology::{accumulation::RATE_HANDOFF_UNIT, HorizontalStaggering};

use super::{
    create_horizontal_output_buffer, create_horizontal_query_buffers,
    create_horizontal_uniform_buffer, download_buffer_typed, encode_horizontal_samples,
    precipitation_members::{
        derive_member_sequence, PrecipitationMemberError, PrecipitationMemberKernels,
        PrecipitationMemberMetadata, PrecipitationPointMember, PreparedPrecipitationMembers,
        PRECIPITATION_MEMBER_PREPARATION,
    },
    GpuBufferError, GpuContext, GpuHorizontalError, HorizontalFieldBuffers,
    HorizontalInterpolationKernel, HorizontalQueryBuffers, HorizontalSampleOutput,
    HorizontalUniforms,
};

const SHADER: &str = include_str!("../shaders/precipitation_temporal.wgsl");

/// Fatal status bit added to #207's shared status for non-finite #87/#119 output.
pub const STATUS_NON_FINITE_SAMPLE: u32 = 4;

/// Fail-closed boundary of precipitation temporal preparation and execution.
#[derive(Debug, Error)]
pub enum PrecipitationTemporalError {
    /// Existing upstream validation failure.
    #[error(transparent)]
    Member(#[from] PrecipitationMemberError),
    /// Existing #87 grid/query failure.
    #[error(transparent)]
    Horizontal(#[from] GpuHorizontalError),
    /// Explicit validation readback failure.
    #[error(transparent)]
    Buffer(#[from] GpuBufferError),
    /// Identity, window or resource mismatch.
    #[error("invalid precipitation time-sampling contract: {0}")]
    Invalid(&'static str),
    /// Unsupported direction from the caller.
    #[error("backward precipitation time sampling is unsupported")]
    BackwardTime,
    /// Batch-wide producer or sampling failure; no lane can be published.
    #[error("precipitation GPU fatal status: {0}")]
    DeviceStatus(u32),
}

/// A particle's unshifted sampling time and canonical regional grid position.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PrecipitationTimeQuery {
    /// X in canonical grid-index coordinates, validated by #87.
    pub xt: f64,
    /// Y in canonical grid-index coordinates, validated by #87.
    pub yt: f64,
    /// Integer seconds in the member calendar/time reference.
    pub itime_epoch_seconds: i64,
}

/// Exact original memory selection; both entries refer to the same #207 owner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrecipitationMemoryPair {
    /// Genuine member at the left memory timestamp.
    pub left: PrecipitationPointMember,
    /// Next genuine member, or the same final member at `T_last` only.
    pub right: PrecipitationPointMember,
}

/// Resolve the complete validated #207 window using the frozen #202 policy.
///
/// # Errors
/// Rejects unsupported preparation, modified metadata, missing four-interval
/// support, non-increasing/duplicate members, wrong units and outside coverage.
pub fn select_precipitation_memory_pair(
    metadata: &PrecipitationMemberMetadata,
    itime: i64,
) -> Result<PrecipitationMemoryPair, PrecipitationTemporalError> {
    metadata.preparation.validate()?;
    for interval in &metadata.intervals {
        if interval
            .end_epoch_seconds
            .checked_sub(interval.start_epoch_seconds)
            .is_none()
        {
            return Err(PrecipitationTemporalError::Invalid(
                "interval seconds overflow",
            ));
        }
    }
    if metadata.representation != PRECIPITATION_MEMBER_PREPARATION
        || metadata.unit != RATE_HANDOFF_UNIT
    {
        return Err(PrecipitationTemporalError::Invalid(
            "expected #207 point rates in mm/h",
        ));
    }
    let sequence = derive_member_sequence(&metadata.intervals)?;
    if sequence.members != metadata.members
        || sequence.delta_seconds != metadata.delta_seconds
        || sequence.sequence_start_epoch_seconds != metadata.sequence_start_epoch_seconds
    {
        return Err(PrecipitationTemporalError::Invalid(
            "incomplete or modified member window",
        ));
    }
    let first = &metadata.members[0];
    let last = &metadata.members[metadata.members.len() - 1];
    if itime < first.valid_time_epoch_seconds || itime > last.valid_time_epoch_seconds {
        return Err(PrecipitationMemberError::OutsideCoverage {
            valid_time_epoch_seconds: itime,
        }
        .into());
    }
    let slot = metadata
        .members
        .partition_point(|m| m.valid_time_epoch_seconds <= itime)
        - 1;
    let left_time = metadata.members[slot].valid_time_epoch_seconds;
    let right_time =
        metadata.members[(slot + 1).min(metadata.members.len() - 1)].valid_time_epoch_seconds;
    Ok(PrecipitationMemoryPair {
        left: metadata.member_at(left_time)?.clone(),
        right: metadata.member_at(right_time)?.clone(),
    })
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TimeParameters {
    dt1_seconds: i32,
    dt2_seconds: i32,
    dt_seconds: i32,
    output_pair: u32,
}

impl TimeParameters {
    fn new(
        pair: &PrecipitationMemoryPair,
        itime: i64,
        index: usize,
    ) -> Result<Self, PrecipitationTemporalError> {
        let delta = |end: i64, start: i64| {
            end.checked_sub(start)
                .and_then(|d| i32::try_from(d).ok())
                .ok_or(PrecipitationTemporalError::Invalid(
                    "time difference exceeds original integer seconds",
                ))
        };
        Ok(Self {
            dt1_seconds: delta(itime, pair.left.valid_time_epoch_seconds)?,
            dt2_seconds: delta(pair.right.valid_time_epoch_seconds, itime)?,
            dt_seconds: delta(
                pair.right.valid_time_epoch_seconds,
                pair.left.valid_time_epoch_seconds,
            )?,
            output_pair: u32::try_from(index)
                .map_err(|_| PrecipitationTemporalError::Invalid("too many queries"))?,
        })
    }
}

/// Persistent kernels sharing the existing runtime; constructed outside encoding.
pub struct PrecipitationTemporalKernels {
    context: Arc<()>,
    members: PrecipitationMemberKernels,
    horizontal: HorizontalInterpolationKernel,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl PrecipitationTemporalKernels {
    /// Create #207/#87 and precipitation-specific temporal pipelines once.
    ///
    /// # Errors
    /// Forwards existing stage initialization failures. The caller owns wgpu
    /// error scopes, as for #76 composition.
    pub fn new(ctx: &GpuContext) -> Result<Self, PrecipitationTemporalError> {
        let entries: Vec<_> = (0..7)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 6 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: binding < 4,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("precipitation_time_layout"),
                entries: &entries,
            });
        let pipeline_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("precipitation_time_pipeline_layout"),
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            });
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("precipitation_time_shader"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
        let pipeline = ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("precipitation_time_pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        Ok(Self {
            context: Arc::clone(&ctx.identity),
            members: PrecipitationMemberKernels::new(ctx)?,
            horizontal: HorizontalInterpolationKernel::new(ctx)?,
            layout,
            pipeline,
        })
    }
}

struct SpatialPlane {
    field: HorizontalFieldBuffers,
    output: HorizontalSampleOutput,
}

struct SampleResources {
    planes: [SpatialPlane; 4],
    queries: HorizontalQueryBuffers,
    uniforms: HorizontalUniforms,
    parameters: wgpu::Buffer,
}

/// Typed output provenance, preserving the whole upstream window and #216 admissions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrecipitationTemporalMetadata {
    /// Immutable #207 point-member source identity and four-interval supports.
    pub members: PrecipitationMemberMetadata,
    /// Unshifted query times and #87 coordinates in output order.
    pub queries: Vec<PrecipitationTimeQuery>,
    /// Selected original memory pair per query, including the final duplicate.
    pub memory_pairs: Vec<PrecipitationMemoryPair>,
    /// Output quantity, always `millimeter_per_hour`.
    pub unit: String,
}

/// Owns intermediates and borrows the genuine #207 producer through completion.
pub struct PreparedPrecipitationTimeSamples<'a, 'source, 'ctx> {
    producer: &'a mut PreparedPrecipitationMembers<'source, 'ctx>,
    resources: Vec<SampleResources>,
    output: wgpu::Buffer,
    metadata: PrecipitationTemporalMetadata,
    encoded: bool,
}

/// Device-resident final LSP/CP handoff; all lanes depend on the fatal status.
pub struct EncodedPrecipitationTimeSamples<'a> {
    /// Interleaved `[LSP, CP]` per query, `f32` in mm/h.
    pub rates_mm_per_h: &'a wgpu::Buffer,
    /// Shared #207/#87/#119 fatal status. Any nonzero value invalidates the batch.
    pub status: &'a wgpu::Buffer,
    /// Exact source/member/query identities retained by the owner.
    pub metadata: &'a PrecipitationTemporalMetadata,
}

/// Prepare one finite forward-time batch from a genuine #207 resource owner.
///
/// No arbitrary buffer, raw #90 rate, amount or CPU member can enter this API.
/// `direction` must be +1 (the existing FLEXPART forward convention).
///
/// # Errors
/// Rejects context, direction, query, complete-window and resource errors before
/// encoding. #207 already validated source/grid/calendar/#216 preparation.
pub fn prepare_precipitation_time_samples<'a, 'source, 'ctx>(
    ctx: &GpuContext,
    producer: &'a mut PreparedPrecipitationMembers<'source, 'ctx>,
    queries: &[PrecipitationTimeQuery],
    direction: i32,
) -> Result<PreparedPrecipitationTimeSamples<'a, 'source, 'ctx>, PrecipitationTemporalError> {
    if direction != 1 {
        return Err(PrecipitationTemporalError::BackwardTime);
    }
    if !producer.belongs_to(ctx) {
        return Err(PrecipitationTemporalError::Invalid("different GPU context"));
    }
    if queries.is_empty() {
        return Err(PrecipitationTemporalError::Invalid("empty queries"));
    }
    let members = producer.metadata();
    if members.horizontal_grid.is_periodic_x() {
        return Err(PrecipitationTemporalError::Invalid(
            "only regional geometry is established",
        ));
    }
    let pairs = queries
        .iter()
        .map(|q| select_precipitation_memory_pair(members, q.itime_epoch_seconds))
        .collect::<Result<Vec<_>, _>>()?;
    let scratch = vec![0.0; members.cell_count];
    let mut resources = Vec::with_capacity(queries.len());
    for (index, (query, pair)) in queries.iter().zip(&pairs).enumerate() {
        let plane = || -> Result<SpatialPlane, PrecipitationTemporalError> {
            Ok(SpatialPlane {
                field: HorizontalFieldBuffers::from_grid_and_values(
                    ctx,
                    &members.horizontal_grid,
                    &scratch,
                    HorizontalStaggering::CellCenter,
                )?,
                output: create_horizontal_output_buffer(ctx, 1)?,
            })
        };
        resources.push(SampleResources {
            planes: [plane()?, plane()?, plane()?, plane()?],
            queries: create_horizontal_query_buffers(
                ctx,
                &members.horizontal_grid,
                &scratch,
                HorizontalStaggering::CellCenter,
                &[(query.xt, query.yt)],
            )?
            .0,
            uniforms: create_horizontal_uniform_buffer(ctx, &members.horizontal_grid, 1)?,
            parameters: ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("precipitation_time_parameters"),
                    contents: bytemuck::bytes_of(&TimeParameters::new(
                        pair,
                        query.itime_epoch_seconds,
                        index,
                    )?),
                    usage: wgpu::BufferUsages::UNIFORM,
                }),
        });
    }
    let size = queries
        .len()
        .checked_mul(8)
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n <= u64::from(ctx.device.limits().max_storage_buffer_binding_size))
        .ok_or(PrecipitationTemporalError::Invalid(
            "output buffer exceeds device limits",
        ))?;
    // Unsubmitted work must fail validation instead of looking like dry weather.
    let output = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("precipitation_time_rates_mmh"),
            contents: bytemuck::cast_slice(&vec![
                f32::NAN;
                usize::try_from(size / 4).map_err(|_| {
                    PrecipitationTemporalError::Invalid("output size")
                })?
            ]),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
    let metadata = PrecipitationTemporalMetadata {
        members: members.clone(),
        queries: queries.to_vec(),
        memory_pairs: pairs,
        unit: RATE_HANDOFF_UNIT.to_owned(),
    };
    Ok(PreparedPrecipitationTimeSamples {
        producer,
        resources,
        output,
        metadata,
        encoded: false,
    })
}

impl PreparedPrecipitationTimeSamples<'_, '_, '_> {
    /// Encode #90 -> #207 -> #87 -> #119 without submit, poll or host transfer.
    ///
    /// # Errors
    /// Rejects another context/kernel and forwards stage errors. Discard the
    /// encoder after an error. Retain this owner and kernels through completion.
    pub fn encode<'s>(
        &'s mut self,
        ctx: &GpuContext,
        kernels: &PrecipitationTemporalKernels,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<EncodedPrecipitationTimeSamples<'s>, PrecipitationTemporalError> {
        if self.encoded {
            return Err(PrecipitationTemporalError::Invalid(
                "sample batch already encoded",
            ));
        }
        if !self.producer.belongs_to(ctx) || !Arc::ptr_eq(&kernels.context, &ctx.identity) {
            return Err(PrecipitationTemporalError::Invalid(
                "different GPU context or kernels",
            ));
        }
        let upstream = self.producer.encode(ctx, &kernels.members, encoder)?;
        for (resources, pair) in self.resources.iter().zip(&self.metadata.memory_pairs) {
            let inputs = [
                upstream.large_scale_mm_per_h,
                upstream.large_scale_mm_per_h,
                upstream.convective_mm_per_h,
                upstream.convective_mm_per_h,
            ];
            for (index, (plane, input)) in resources.planes.iter().zip(inputs).enumerate() {
                let member = if index % 2 == 0 {
                    &pair.left
                } else {
                    &pair.right
                };
                let (offset, bytes) = upstream.metadata.plane_byte_range(member);
                encoder.copy_buffer_to_buffer(input, offset, &plane.field.buffer, 0, bytes);
                encode_horizontal_samples(
                    ctx,
                    &plane.field,
                    &resources.queries,
                    &plane.output,
                    &resources.uniforms,
                    &kernels.horizontal,
                    encoder,
                )?;
            }
            encode_time(
                ctx,
                kernels,
                resources,
                &self.output,
                upstream.status,
                encoder,
            );
        }
        self.encoded = true;
        Ok(EncodedPrecipitationTimeSamples {
            rates_mm_per_h: &self.output,
            status: upstream.status,
            metadata: &self.metadata,
        })
    }

    /// Explicit final validation boundary after caller submission/completion.
    ///
    /// # Errors
    /// Rejects unencoded plans, other contexts, any fatal status and non-finite
    /// result. No valid prefix is returned when any producer or sample failed.
    pub async fn download(&self, ctx: &GpuContext) -> Result<Vec<f32>, PrecipitationTemporalError> {
        if !self.encoded || !self.producer.belongs_to(ctx) {
            return Err(PrecipitationTemporalError::Invalid(
                "unencoded plan or different context",
            ));
        }
        let status = self.producer.validation_status(ctx).await?;
        if status != 0 {
            return Err(PrecipitationTemporalError::DeviceStatus(status));
        }
        let values: Vec<f32> = download_buffer_typed(
            ctx,
            &self.output,
            self.resources.len() * 2,
            "precipitation_time_validation",
        )
        .await?;
        if values.iter().any(|v| !v.is_finite()) {
            return Err(PrecipitationTemporalError::DeviceStatus(
                STATUS_NON_FINITE_SAMPLE,
            ));
        }
        Ok(values)
    }
}

fn encode_time(
    ctx: &GpuContext,
    kernels: &PrecipitationTemporalKernels,
    resources: &SampleResources,
    output: &wgpu::Buffer,
    status: &wgpu::Buffer,
    encoder: &mut wgpu::CommandEncoder,
) {
    let buffers = [
        &resources.planes[0].output.buffer,
        &resources.planes[1].output.buffer,
        &resources.planes[2].output.buffer,
        &resources.planes[3].output.buffer,
        output,
        status,
        &resources.parameters,
    ];
    let entries: Vec<_> = buffers
        .iter()
        .zip(0..)
        .map(|(buffer, binding)| wgpu::BindGroupEntry {
            binding,
            resource: buffer.as_entire_binding(),
        })
        .collect();
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("precipitation_time_bindings"),
        layout: &kernels.layout,
        entries: &entries,
    });
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("precipitation_time_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&kernels.pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.dispatch_workgroups(1, 1, 1);
}

/// SHA-256 of the precipitation-specific temporal shader for #91 evidence.
#[must_use]
pub fn precipitation_temporal_shader_sha256() -> String {
    format!("{:x}", Sha256::digest(SHADER.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meteorology::HorizontalGrid;

    #[test]
    #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
    fn test_precipitation_temporal_original_arithmetic_and_fatal_status() {
        let ctx =
            pollster::block_on(GpuContext::new()).expect("required arithmetic WGSL execution");
        let kernels = PrecipitationTemporalKernels::new(&ctx).expect("kernels");
        let snapshot: serde_json::Value = serde_json::from_str(include_str!(
            "../../fixtures/vertical/synthetic-column-v1.json"
        ))
        .expect("snapshot");
        let mut grid: HorizontalGrid =
            serde_json::from_value(snapshot["horizontal_grid"].clone()).expect("grid");
        grid.nx = 4;
        grid.ny = 3;
        grid.dx_deg = 1.0;
        grid.dy_deg = 1.0;
        let lsp0: Vec<f32> = (0..12).map(|v| v as f32).collect();
        let lsp1: Vec<f32> = (100..112).map(|v| v as f32).collect();
        let status = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("arithmetic_status"),
                contents: bytemuck::bytes_of(&0_u32),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            });
        let output = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("arithmetic_rates"),
            size: 80,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let statuses = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("arithmetic_status_history"),
            size: 40,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut retained = Vec::new();
        for (index, t) in [0, 900, 1800, 2700, 3600, 0, 0, 0, 0, 0]
            .into_iter()
            .enumerate()
        {
            encoder.clear_buffer(&status, 0, None);
            let values = if index == 5 {
                vec![0.7; 12]
            } else {
                lsp0.clone()
            };
            let plane = |values: &[f32]| SpatialPlane {
                field: HorizontalFieldBuffers::from_grid_and_values(
                    &ctx,
                    &grid,
                    values,
                    HorizontalStaggering::CellCenter,
                )
                .expect("field"),
                output: create_horizontal_output_buffer(&ctx, 1).expect("output"),
            };
            let resources = SampleResources {
                planes: [
                    plane(&values),
                    plane(&lsp1),
                    plane(&[0.5; 12]),
                    plane(&[1.5; 12]),
                ],
                queries: create_horizontal_query_buffers(
                    &ctx,
                    &grid,
                    &values,
                    HorizontalStaggering::CellCenter,
                    &[(1.25, 0.5)],
                )
                .expect("query")
                .0,
                uniforms: create_horizontal_uniform_buffer(&ctx, &grid, 1).expect("uniforms"),
                parameters: ctx
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("arithmetic_parameters"),
                        contents: bytemuck::bytes_of(&TimeParameters {
                            dt1_seconds: t,
                            dt2_seconds: if index == 5 { 0 } else { 3600 - t },
                            dt_seconds: if index == 5 { 0 } else { 3600 },
                            output_pair: u32::try_from(index).expect("index"),
                        }),
                        usage: wgpu::BufferUsages::UNIFORM,
                    }),
            };
            // The canonical #87 query identity includes field bytes. Build each
            // query with its own uploaded plane for this isolated control only.
            for (p, values) in resources.planes.iter().zip([
                values.as_slice(),
                lsp1.as_slice(),
                &[0.5; 12],
                &[1.5; 12],
            ]) {
                let query = create_horizontal_query_buffers(
                    &ctx,
                    &grid,
                    values,
                    HorizontalStaggering::CellCenter,
                    &[(1.25, 0.5)],
                )
                .expect("query")
                .0;
                encode_horizontal_samples(
                    &ctx,
                    &p.field,
                    &query,
                    &p.output,
                    &resources.uniforms,
                    &kernels.horizontal,
                    &mut encoder,
                )
                .expect("#87");
            }
            if index == 6 {
                encoder.clear_buffer(&status, 0, None);
                let fatal = ctx
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("producer_failure"),
                        contents: bytemuck::bytes_of(&2_u32),
                        usage: wgpu::BufferUsages::COPY_SRC,
                    });
                encoder.copy_buffer_to_buffer(&fatal, 0, &status, 0, 4);
            }
            if index >= 7 {
                let value = [f32::NAN, f32::INFINITY, f32::MAX][index - 7];
                let sample = ctx
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("invalid_device_spatial_sample"),
                        contents: bytemuck::bytes_of(&value),
                        usage: wgpu::BufferUsages::COPY_SRC,
                    });
                encoder.copy_buffer_to_buffer(&sample, 0, &resources.planes[0].output.buffer, 0, 4);
            }
            encode_time(&ctx, &kernels, &resources, &output, &status, &mut encoder);
            encoder.copy_buffer_to_buffer(
                &status,
                0,
                &statuses,
                u64::try_from(index).expect("index") * 4,
                4,
            );
            retained.push(resources);
        }
        ctx.queue.submit(Some(encoder.finish()));
        let values: Vec<f32> = pollster::block_on(download_buffer_typed(
            &ctx,
            &output,
            20,
            "arithmetic_control",
        ))
        .expect("readback");
        assert_eq!(
            &values[..10],
            &[9.75, 1.5, 84.75, 2.25, 159.75, 3.0, 234.75, 3.75, 309.75, 4.5]
        );
        assert_eq!(
            values[10].to_bits(),
            (0.7_f32 / (1.0_f32 / 3.0_f32)).to_bits()
        );
        assert!(
            values[12..].iter().all(|v| v.is_nan()),
            "fatal producer must not publish zero rain"
        );
        let status: Vec<u32> =
            pollster::block_on(download_buffer_typed(&ctx, &statuses, 10, "fatal_status"))
                .expect("status");
        assert_eq!(status, [0, 0, 0, 0, 0, 0, 2, 4, 4, 4]);
    }
}
