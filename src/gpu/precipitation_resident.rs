//! Pair-shared particle-scale precipitation rates on #171 resident lanes (#227).
//!
//! One forward step shares `itime` across all lanes, so the unchanged #119
//! memory pair is selected once on the host control plane. Four #207 planes
//! (LSP/CP x left/right) are copied on the device, #171 lanes provide the only
//! coordinates, #87 samples each plane, and a lane-parallel port of
//! `interpol_mod.f90:1302-1316,1549-1550` applies the #119 time arithmetic.
//! The caller owns encoding, submission, device error scopes and completion.
//! Any nonzero shared status invalidates the entire step; no lane is published.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::meteorology::accumulation::RATE_HANDOFF_UNIT;

use super::{
    buffers::ParticleBuffers,
    create_horizontal_output_buffer, create_horizontal_uniform_buffer, download_buffer_typed,
    encode_horizontal_samples,
    meteorology::{
        resident::{QueryPass, ResidentQueryBatch, ResidentQueryKernels, SurfaceAdapterBindings},
        MeteorologyCompositionError,
    },
    precipitation_members::{PrecipitationMemberMetadata, PreparedPrecipitationMembers},
    precipitation_temporal::{
        select_precipitation_memory_pair, PrecipitationMemoryPair, PrecipitationTemporalError,
        TimeParameters, STATUS_NON_FINITE_SAMPLE,
    },
    GpuContext, HorizontalFieldBuffers, HorizontalInterpolationKernel, HorizontalQueryBuffers,
    HorizontalSampleOutput, HorizontalUniforms,
};

const RATES_SHADER: &str = include_str!("../shaders/precipitation_resident_rates.wgsl");
const PUBLISH_SHADER: &str = include_str!("../shaders/precipitation_resident_publish.wgsl");

/// Shared-word bit for any failed #171/#87/#119 lane (the existing #171 fatal bit).
pub const RESIDENT_PRECIPITATION_LANE_FAILURE: u32 = 1;
/// Shared-word bit for a non-finite #87 sample or #119 result.
pub const RESIDENT_PRECIPITATION_NON_FINITE_SAMPLE: u32 = STATUS_NON_FINITE_SAMPLE;
/// The #207 producer status is OR-ed into the shared word shifted by this many bits.
pub const RESIDENT_PRECIPITATION_PRODUCER_SHIFT: u32 = 8;
/// #171 lane reason for an incomplete or non-finite sampled result.
pub const RESIDENT_LANE_SAMPLED_RESULT: u32 = 9;

/// Interleaved `[LSP, CP]` components per lane.
const COMPONENTS: u32 = 2;
/// LSP left, LSP right, CP left, CP right.
const PLANES: usize = 4;

/// Context-lifetime #87 and #227 pipelines; build once, outside encoding.
pub struct ResidentPrecipitationKernels {
    context: Arc<()>,
    horizontal: HorizontalInterpolationKernel,
    rates: QueryPass,
    publish: QueryPass,
}

impl ResidentPrecipitationKernels {
    /// Create the lane-parallel #119 and publication pipelines.
    ///
    /// # Errors
    /// Surfaces #87 initialization and scoped shader/pipeline errors.
    pub fn new(ctx: &GpuContext) -> Result<Self, PrecipitationTemporalError> {
        let horizontal = HorizontalInterpolationKernel::new(ctx)?;
        ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let rates = QueryPass::new(
            ctx,
            "precipitation resident rates",
            RATES_SHADER,
            &[true, true, true, true, true, false, false],
        );
        let publish = QueryPass::new(
            ctx,
            "precipitation resident publish",
            PUBLISH_SHADER,
            &[true, true, false, false],
        );
        let errors: Vec<_> = (0..3)
            .filter_map(|_| pollster::block_on(ctx.device.pop_error_scope()))
            .collect();
        if !errors.is_empty() {
            return Err(MeteorologyCompositionError::ResidentDevice(format!("{errors:?}")).into());
        }
        Ok(Self {
            context: Arc::clone(&ctx.identity),
            horizontal,
            rates,
            publish,
        })
    }
}

/// Encoded GPU operation, in submission order, for the scalability audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidentPrecipitationStage {
    /// #171 device status reset.
    StatusReset,
    /// #171 particle-to-lane producer.
    ParticleProducer,
    /// Device-to-device copy of one selected #207 plane.
    PlaneCopy,
    /// #171 lane adaptation into #87 geometry, with per-lane corner checks.
    ResidentAdapter,
    /// #87 bilinear sampling of one plane at every lane.
    Horizontal,
    /// Lane-parallel #119 time arithmetic.
    TemporalRates,
    /// Batch-wide fail-closed publication.
    Publish,
}

impl ResidentPrecipitationStage {
    /// Whether the stage is a compute dispatch (otherwise a buffer copy).
    #[must_use]
    pub const fn is_dispatch(self) -> bool {
        !matches!(self, Self::PlaneCopy)
    }
}

/// Typed provenance of one pair-shared step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResidentPrecipitationMetadata {
    /// Immutable #207 window and #216 source identities.
    pub members: PrecipitationMemberMetadata,
    /// Unshifted step time shared by every lane.
    pub itime_epoch_seconds: i64,
    /// The one original memory pair of this step.
    pub memory_pair: PrecipitationMemoryPair,
    /// Checked integer `[itime-t0, t1-itime, t1-t0]` before `f32` conversion.
    pub time_differences_seconds: [i32; 3],
    /// Allocated #171 lanes.
    pub capacity: u32,
    /// Eligible active prefix of the batch.
    pub active_count: u32,
    /// Components per lane: `[LSP, CP]`.
    pub component_count: u32,
    /// Output quantity, always `millimeter_per_hour`.
    pub unit: String,
    /// Bytes of the four shared #87 planes, `4 * cell_count * 4`.
    pub plane_bytes: u64,
    /// Bytes of every buffer this step owns, planes included. The batch's own
    /// lane/status buffers and #171's two 16-byte per-encode control uniforms
    /// are owned by the #171 batch path and not counted.
    pub device_bytes: u64,
}

struct SampledPlane {
    field: HorizontalFieldBuffers,
    queries: HorizontalQueryBuffers,
    output: HorizontalSampleOutput,
}

/// Step resources; borrows the shared #207 owner and the exclusive #171 batch.
pub struct PreparedResidentPrecipitationRates<'a, 'source, 'ctx, 'query> {
    producer: &'a PreparedPrecipitationMembers<'source, 'ctx>,
    batch: &'a mut ResidentQueryBatch<'query>,
    planes: [SampledPlane; PLANES],
    uniforms: HorizontalUniforms,
    adapter_params: wgpu::Buffer,
    query_heights: wgpu::Buffer,
    lane_heights: wgpu::Buffer,
    rate_params: wgpu::Buffer,
    publish_params: wgpu::Buffer,
    candidate: wgpu::Buffer,
    output: wgpu::Buffer,
    executed: wgpu::Buffer,
    metadata: ResidentPrecipitationMetadata,
    stages: Vec<ResidentPrecipitationStage>,
    encoded: bool,
}

/// Device-resident `[LSP, CP]` handoff; consumers must honor the shared status.
pub struct EncodedResidentPrecipitationRates<'a> {
    /// Interleaved `[LSP, CP]` per capacity lane, `f32` mm/h; NaN unless published.
    pub rates_mm_per_h: &'a wgpu::Buffer,
    /// #171 shared fatal word followed by one lane reason per capacity lane.
    pub status: &'a wgpu::Buffer,
    /// Exact source/pair/time/layout provenance.
    pub metadata: &'a ResidentPrecipitationMetadata,
    /// Encoded operations in order.
    pub stages: &'a [ResidentPrecipitationStage],
}

/// Validation-boundary readback of a successful step.
#[derive(Debug, Clone)]
pub struct ResidentPrecipitationReadback {
    /// Interleaved `[LSP, CP]` per capacity lane; NaN for every unpublished lane.
    pub rates_mm_per_h: Vec<f32>,
    /// #171 lane reasons: 2 published, 0 inactive or tail.
    pub lane_status: Vec<u32>,
}

fn storage_buffer(ctx: &GpuContext, bytes: u64, label: &str) -> wgpu::Buffer {
    ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn control_uniform(ctx: &GpuContext, words: &[u32], label: &str) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    ctx.device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(words),
            usage: wgpu::BufferUsages::UNIFORM,
        })
}

#[allow(clippy::cast_sign_loss)]
const fn bits(value: i32) -> u32 {
    value as u32
}

/// Prepare one forward step for every lane of a resident #171 batch.
///
/// `producer` must already have encoded its #207 reconstruction; it stays
/// shared and unmodified by every step. All lanes use `itime_epoch_seconds`,
/// so one memory pair applies. `direction` must be +1.
///
/// # Errors
/// Rejects backward time, foreign contexts, unencoded producers, periodic or
/// differing grids, coverage/window/time-range failures and resource limits.
pub fn prepare_resident_precipitation_rates<'a, 'source, 'ctx, 'query>(
    ctx: &GpuContext,
    producer: &'a PreparedPrecipitationMembers<'source, 'ctx>,
    batch: &'a mut ResidentQueryBatch<'query>,
    itime_epoch_seconds: i64,
    direction: i32,
) -> Result<PreparedResidentPrecipitationRates<'a, 'source, 'ctx, 'query>, PrecipitationTemporalError>
{
    if direction != 1 {
        return Err(PrecipitationTemporalError::BackwardTime);
    }
    if !producer.belongs_to(ctx) || !batch.belongs_to(ctx) {
        return Err(PrecipitationTemporalError::Invalid("different GPU context"));
    }
    if producer.encoded_handoff().is_none() {
        return Err(PrecipitationTemporalError::Invalid(
            "#207 members must be encoded before resident sampling",
        ));
    }
    let members = producer.metadata();
    if members.horizontal_grid.is_periodic_x() {
        return Err(PrecipitationTemporalError::Invalid(
            "only regional geometry is established",
        ));
    }
    if batch.grid() != &members.horizontal_grid {
        return Err(PrecipitationTemporalError::Invalid(
            "resident batch grid differs from #207 members",
        ));
    }
    let memory_pair = select_precipitation_memory_pair(members, itime_epoch_seconds)?;
    let time = TimeParameters::new(&memory_pair, itime_epoch_seconds, 0)?;
    let capacity = batch.capacity();
    let lanes = capacity as usize;
    let output_bytes = u64::from(capacity) * u64::from(COMPONENTS) * 4;
    if output_bytes > u64::from(ctx.device.limits().max_storage_buffer_binding_size) {
        return Err(PrecipitationTemporalError::Invalid(
            "output buffer exceeds device limits",
        ));
    }
    let plane = |label: &str| -> Result<SampledPlane, PrecipitationTemporalError> {
        let field = HorizontalFieldBuffers::device_plane(
            ctx,
            &members.horizontal_grid,
            format!("precipitation-resident-227:{label}"),
        )?;
        Ok(SampledPlane {
            queries: HorizontalQueryBuffers::resident(ctx, &field, lanes)?,
            output: create_horizontal_output_buffer(ctx, lanes)?,
            field,
        })
    };
    let planes = [
        plane("lsp-left")?,
        plane("lsp-right")?,
        plane("cp-left")?,
        plane("cp-right")?,
    ];
    let control = [capacity, batch.active_count(), 0, 0];
    let prepared = PreparedResidentPrecipitationRates {
        uniforms: create_horizontal_uniform_buffer(ctx, &members.horizontal_grid, lanes)?,
        adapter_params: batch.surface_adapter_params(ctx)?,
        query_heights: storage_buffer(
            ctx,
            u64::from(capacity) * 4,
            "precipitation_resident_heights",
        ),
        lane_heights: storage_buffer(ctx, 4, "precipitation_resident_unused_columns"),
        rate_params: control_uniform(
            ctx,
            &[
                capacity,
                batch.active_count(),
                bits(time.dt1_seconds),
                bits(time.dt2_seconds),
                bits(time.dt_seconds),
                0,
                0,
                0,
            ],
            "precipitation_resident_rate_params",
        ),
        publish_params: control_uniform(ctx, &control, "precipitation_resident_publish_params"),
        candidate: storage_buffer(ctx, output_bytes, "precipitation_resident_candidates"),
        output: storage_buffer(ctx, output_bytes, "precipitation_resident_rates_mmh"),
        executed: storage_buffer(ctx, 4, "precipitation_resident_executed"),
        metadata: ResidentPrecipitationMetadata {
            members: members.clone(),
            itime_epoch_seconds,
            memory_pair,
            time_differences_seconds: [time.dt1_seconds, time.dt2_seconds, time.dt_seconds],
            capacity,
            active_count: batch.active_count(),
            component_count: COMPONENTS,
            unit: RATE_HANDOFF_UNIT.to_owned(),
            plane_bytes: planes.iter().map(|p| p.field.buffer.size()).sum(),
            device_bytes: 0,
        },
        planes,
        producer,
        batch,
        stages: Vec::new(),
        encoded: false,
    };
    Ok(prepared.with_device_bytes())
}

impl PreparedResidentPrecipitationRates<'_, '_, '_, '_> {
    fn with_device_bytes(mut self) -> Self {
        let planes: u64 = self
            .planes
            .iter()
            .map(|p| p.field.buffer.size() + p.queries.buffer.size() + p.output.buffer.size())
            .sum();
        let step: u64 = [
            &self.adapter_params,
            &self.query_heights,
            &self.lane_heights,
            &self.rate_params,
            &self.publish_params,
            &self.candidate,
            &self.output,
            &self.executed,
        ]
        .iter()
        .map(|b| b.size())
        .sum();
        self.metadata.device_bytes = planes + step + self.uniforms.buffer_size();
        self
    }

    /// Typed provenance, available before encoding.
    #[must_use]
    pub const fn metadata(&self) -> &ResidentPrecipitationMetadata {
        &self.metadata
    }

    /// Encode #171 reset/producer, plane copies, #171 adaptation, #87, #119
    /// and publication into one caller encoder. Nothing is submitted or read back.
    /// A step encodes once; prepare a new step for the next lanes.
    ///
    /// # Errors
    /// Rejects foreign kernels/contexts and particle owner/prefix mismatches
    /// before encoding anything. Discard the encoder after any error.
    pub fn encode_from_particles<'s>(
        &'s mut self,
        ctx: &GpuContext,
        particles: &ParticleBuffers,
        resident: &ResidentQueryKernels<'_>,
        kernels: &ResidentPrecipitationKernels,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<EncodedResidentPrecipitationRates<'s>, PrecipitationTemporalError> {
        // One-shot: a second encode could leave a stale executed flag and
        // stale rates behind an unsubmitted encoder.
        if self.encoded {
            return Err(PrecipitationTemporalError::Invalid(
                "resident precipitation step already encoded",
            ));
        }
        if !Arc::ptr_eq(&kernels.context, &ctx.identity) || !self.producer.belongs_to(ctx) {
            return Err(PrecipitationTemporalError::Invalid(
                "different GPU context or kernels",
            ));
        }
        self.batch
            .encode_particles(ctx, particles, resident, encoder)?;
        self.stages = vec![
            ResidentPrecipitationStage::StatusReset,
            ResidentPrecipitationStage::ParticleProducer,
        ];
        self.encode_sampling(ctx, resident, kernels, encoder)
    }

    fn encode_sampling<'s>(
        &'s mut self,
        ctx: &GpuContext,
        resident: &ResidentQueryKernels<'_>,
        kernels: &ResidentPrecipitationKernels,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<EncodedResidentPrecipitationRates<'s>, PrecipitationTemporalError> {
        let upstream =
            self.producer
                .encoded_handoff()
                .ok_or(PrecipitationTemporalError::Invalid(
                    "#207 members not encoded",
                ))?;
        let pair = &self.metadata.memory_pair;
        let sources = [
            (upstream.large_scale_mm_per_h, &pair.left),
            (upstream.large_scale_mm_per_h, &pair.right),
            (upstream.convective_mm_per_h, &pair.left),
            (upstream.convective_mm_per_h, &pair.right),
        ];
        for (plane, (source, member)) in self.planes.iter().zip(sources) {
            let (offset, bytes) = upstream.metadata.plane_byte_range(member);
            encoder.copy_buffer_to_buffer(source, offset, &plane.field.buffer, 0, bytes);
            self.batch.encode_surface_adapter(
                ctx,
                resident,
                &SurfaceAdapterBindings {
                    params: &self.adapter_params,
                    plane: &plane.field.buffer,
                    horizontal_queries: &plane.queries.buffer,
                    query_heights: &self.query_heights,
                    lane_heights: &self.lane_heights,
                },
                encoder,
            )?;
            encode_horizontal_samples(
                ctx,
                &plane.field,
                &plane.queries,
                &plane.output,
                &self.uniforms,
                &kernels.horizontal,
                encoder,
            )?;
            self.stages.extend([
                ResidentPrecipitationStage::PlaneCopy,
                ResidentPrecipitationStage::ResidentAdapter,
                ResidentPrecipitationStage::Horizontal,
            ]);
        }
        let status = self.batch.status_buffer();
        let capacity = self.metadata.capacity;
        kernels.rates.encode(
            ctx,
            &[
                &self.planes[0].output.buffer,
                &self.planes[1].output.buffer,
                &self.planes[2].output.buffer,
                &self.planes[3].output.buffer,
                upstream.status,
                status,
                &self.candidate,
                &self.rate_params,
            ],
            capacity,
            encoder,
        );
        kernels.publish.encode(
            ctx,
            &[
                &self.candidate,
                status,
                &self.output,
                &self.executed,
                &self.publish_params,
            ],
            capacity,
            encoder,
        );
        self.stages.extend([
            ResidentPrecipitationStage::TemporalRates,
            ResidentPrecipitationStage::Publish,
        ]);
        self.encoded = true;
        Ok(EncodedResidentPrecipitationRates {
            rates_mm_per_h: &self.output,
            status,
            metadata: &self.metadata,
            stages: &self.stages,
        })
    }

    /// Explicit final validation boundary after caller submission/completion.
    ///
    /// # Errors
    /// Rejects unencoded or unexecuted steps, other contexts, any nonzero
    /// shared or #207 status, inconsistent lane status and non-finite or
    /// non-NaN values where the lane status forbids them.
    pub async fn download(
        &self,
        ctx: &GpuContext,
    ) -> Result<ResidentPrecipitationReadback, PrecipitationTemporalError> {
        if !self.encoded || !self.producer.belongs_to(ctx) || !self.batch.belongs_to(ctx) {
            return Err(PrecipitationTemporalError::Invalid(
                "unencoded step or different context",
            ));
        }
        let executed: Vec<u32> =
            download_buffer_typed(ctx, &self.executed, 1, "precipitation_resident_executed")
                .await?;
        if executed != [1] {
            return Err(PrecipitationTemporalError::Invalid(
                "resident precipitation step was not executed",
            ));
        }
        let capacity = self.metadata.capacity as usize;
        let words: Vec<u32> = download_buffer_typed(
            ctx,
            self.batch.status_buffer(),
            capacity + 1,
            "precipitation_resident_status",
        )
        .await?;
        if words[0] != 0 {
            return Err(PrecipitationTemporalError::DeviceStatus(words[0]));
        }
        let producer = self.producer.validation_status(ctx).await?;
        if producer != 0 {
            return Err(PrecipitationTemporalError::DeviceStatus(
                producer << RESIDENT_PRECIPITATION_PRODUCER_SHIFT,
            ));
        }
        let lane_status = words[1..].to_vec();
        let active = self.metadata.active_count as usize;
        let lanes_valid = lane_status
            .iter()
            .enumerate()
            .all(|(lane, s)| *s == 0 || (lane < active && *s == 2));
        if !lanes_valid {
            return Err(PrecipitationTemporalError::DeviceStatus(
                RESIDENT_PRECIPITATION_LANE_FAILURE,
            ));
        }
        let rates: Vec<f32> = download_buffer_typed(
            ctx,
            &self.output,
            capacity * COMPONENTS as usize,
            "precipitation_resident_validation",
        )
        .await?;
        let consistent = rates
            .chunks(COMPONENTS as usize)
            .zip(&lane_status)
            .all(|(values, s)| {
                values
                    .iter()
                    .all(|v| if *s == 2 { v.is_finite() } else { v.is_nan() })
            });
        if !consistent {
            return Err(PrecipitationTemporalError::DeviceStatus(
                RESIDENT_PRECIPITATION_NON_FINITE_SAMPLE,
            ));
        }
        Ok(ResidentPrecipitationReadback {
            rates_mm_per_h: rates,
            lane_status,
        })
    }
}

/// SHA-256 of the lane-parallel #119 kernel for #91 evidence.
#[must_use]
pub fn precipitation_resident_rates_shader_sha256() -> String {
    format!("{:x}", Sha256::digest(RATES_SHADER.as_bytes()))
}

/// SHA-256 of the publication gate kernel for #91 evidence.
#[must_use]
pub fn precipitation_resident_publish_shader_sha256() -> String {
    format!("{:x}", Sha256::digest(PUBLISH_SHADER.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::precipitation_members::member_matches_oracle;

    const CAPACITY: u32 = 6;
    const ACTIVE: u32 = 5;

    struct Case {
        samples: [Vec<f32>; PLANES],
        lane_status: Vec<u32>,
        producer_status: u32,
        dt: [i32; 3],
    }

    struct Run {
        rates: Vec<f32>,
        status: Vec<u32>,
    }

    fn device_buffer(ctx: &GpuContext, contents: &[u8], label: &str) -> wgpu::Buffer {
        use wgpu::util::DeviceExt;
        ctx.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            })
    }

    /// Isolated #227 kernels on synthetic lane-aligned #87 samples.
    fn run(ctx: &GpuContext, kernels: &ResidentPrecipitationKernels, case: &Case) -> Run {
        let samples: Vec<_> = case
            .samples
            .iter()
            .map(|s| device_buffer(ctx, bytemuck::cast_slice(s), "synthetic_sample"))
            .collect();
        let producer = device_buffer(ctx, bytemuck::bytes_of(&case.producer_status), "producer");
        let mut words = vec![0_u32];
        words.extend(&case.lane_status);
        let status = device_buffer(ctx, bytemuck::cast_slice(&words), "status");
        let bytes = u64::from(CAPACITY * COMPONENTS * 4);
        let candidate = storage_buffer(ctx, bytes, "candidate");
        let output = storage_buffer(ctx, bytes, "output");
        let executed = storage_buffer(ctx, 4, "executed");
        let rate_params = control_uniform(
            ctx,
            &[
                CAPACITY,
                ACTIVE,
                bits(case.dt[0]),
                bits(case.dt[1]),
                bits(case.dt[2]),
                0,
                0,
                0,
            ],
            "rates",
        );
        let publish_params = control_uniform(ctx, &[CAPACITY, ACTIVE, 0, 0], "publish");
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        kernels.rates.encode(
            ctx,
            &[
                &samples[0],
                &samples[1],
                &samples[2],
                &samples[3],
                &producer,
                &status,
                &candidate,
                &rate_params,
            ],
            CAPACITY,
            &mut encoder,
        );
        kernels.publish.encode(
            ctx,
            &[&candidate, &status, &output, &executed, &publish_params],
            CAPACITY,
            &mut encoder,
        );
        ctx.queue.submit(Some(encoder.finish()));
        let read = |buffer, count, label| {
            pollster::block_on(download_buffer_typed::<u32>(ctx, buffer, count, label))
                .expect("validation readback")
        };
        assert_eq!(read(&executed, 1, "executed"), [1]);
        Run {
            rates: read(&output, (CAPACITY * COMPONENTS) as usize, "rates")
                .into_iter()
                .map(f32::from_bits)
                .collect(),
            status: read(&status, CAPACITY as usize + 1, "status"),
        }
    }

    fn original(r0: f32, r1: f32, [dt1, dt2, dt]: [i32; 3]) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let (dt1, mut dt2, mut dt) = (dt1 as f32, dt2 as f32, dt as f32);
        if dt == 0.0 && dt1 == 0.0 {
            dt = 1.0;
            dt2 = 1.0;
        }
        (r0 * dt2 + r1 * dt1) / (dt / 3.0)
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn test_precipitation_resident_lane_arithmetic_and_fatal_publication() {
        let ctx = pollster::block_on(GpuContext::new()).expect("required #227 WGSL execution");
        let kernels = ResidentPrecipitationKernels::new(&ctx).expect("kernels");
        let lanes = CAPACITY as usize;
        let ramp = |offset: f32| (0..lanes).map(|l| offset + l as f32).collect::<Vec<_>>();
        // Lane 3 is inactive inside the prefix; lane 5 is tail.
        let valid = Case {
            samples: [ramp(4.0), ramp(8.0), vec![0.5; lanes], vec![1.5; lanes]],
            lane_status: vec![2, 2, 2, 0, 2, 0],
            producer_status: 0,
            dt: [900, 2700, 3600],
        };
        let mut expected_status = vec![0];
        expected_status.extend(&valid.lane_status);
        for dt in [
            [0, 3600, 3600],
            [900, 2700, 3600],
            [3600, 0, 3600],
            [0, 0, 0],
        ] {
            let case = Case {
                dt,
                ..clone_case(&valid)
            };
            let result = run(&ctx, &kernels, &case);
            assert_eq!(result.status, expected_status);
            for lane in 0..lanes {
                let pair = &result.rates[2 * lane..2 * lane + 2];
                if case.lane_status[lane] != 2 {
                    assert!(pair.iter().all(|v| v.is_nan()), "unpublished lane {lane}");
                    continue;
                }
                let expected = [
                    original(case.samples[0][lane], case.samples[1][lane], dt),
                    original(case.samples[2][lane], case.samples[3][lane], dt),
                ];
                // WGSL division is not correctly rounded; bitwise identity is
                // proven against the #119 kernel on the same adapter instead.
                for (actual, expected) in pair.iter().zip(expected) {
                    assert!(
                        member_matches_oracle(f64::from(*actual), f64::from(expected)),
                        "lane {lane} dt {dt:?}: {actual} vs {expected}"
                    );
                }
            }
        }

        let mut injected = clone_case(&valid);
        injected.samples[1][2] = f32::NAN;
        let mut overflow = clone_case(&valid);
        overflow.samples[2][4] = f32::MAX;
        let pending = Case {
            lane_status: vec![2, 1, 2, 0, 2, 0],
            ..clone_case(&valid)
        };
        let producer = Case {
            producer_status: 2,
            ..clone_case(&valid)
        };
        let failures = [
            (injected, 1 | 4, Some((2, RESIDENT_LANE_SAMPLED_RESULT))),
            (overflow, 1 | 4, Some((4, RESIDENT_LANE_SAMPLED_RESULT))),
            (pending, 1, Some((1, RESIDENT_LANE_SAMPLED_RESULT))),
            (producer, 2 << RESIDENT_PRECIPITATION_PRODUCER_SHIFT, None),
        ];
        for (case, fatal, lane) in failures {
            let result = run(&ctx, &kernels, &case);
            assert_eq!(result.status[0], fatal);
            if let Some((lane, reason)) = lane {
                assert_eq!(result.status[lane + 1], reason);
            }
            assert!(
                result.rates.iter().all(|v| v.is_nan()),
                "fatal step must publish no lane and no dry-weather zero"
            );
        }
    }

    fn clone_case(case: &Case) -> Case {
        Case {
            samples: case.samples.clone(),
            lane_status: case.lane_status.clone(),
            ..*case
        }
    }
}
