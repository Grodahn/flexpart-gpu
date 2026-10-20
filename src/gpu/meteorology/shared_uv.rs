//! Typed #184 resource adoption; all numerical sampling stays with #87/#88/#89.

use super::{
    resident, resolve_temporal_bracket, snapshot_hash, validate_runtime_binding,
    vertical_geometry_identity, CanonicalGpuField, FieldId, GpuContext, HorizontalFieldBuffers,
    HorizontalStaggering, MeteorologyCompositionError, MeteorologyHeight,
    PreparedMeteorologySample, RequestedSampleTime, Requirements, RuntimeColumns, Snapshot,
    SourceMember, TemporalKind, VerticalGridBuffers, VerticalQueryBuffers, VerticalReference,
    VerticalRuntimeView, VerticalSampleOutput, VerticalStaggering, VALUE_BYTES,
};
use crate::gpu::shared_height::{PreparedSharedHeightUv, SharedHeightTargetIdentity};
use crate::gpu::vertical::{
    create_vertical_output_buffer, create_vertical_query_buffers, create_vertical_shared_grid,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// Explicit proof route and authoritative target identity for accepted U/V queries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedHeightQueryIdentity {
    /// Scientific preparation route, never inferred from matching height values.
    pub preparation: String,
    /// Full run target identity, including the initial source and selection rule.
    pub target_identity_sha256: String,
    /// Common AGL f32 target byte hash.
    pub target_height_sha256: String,
    /// Per-member native upload hashes, aligned with selected source snapshots.
    pub source_input_sha256: Vec<String>,
    /// Exact corrected-interface lineage for a complete prepared vector, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrected_w: Vec<super::shared_w::SharedHeightWQueryIdentity>,
}

impl CanonicalGpuField<'_> {
    /// Adopt immutable #184 U/V resources without CPU remapping or prepared-field readback.
    /// Each member must match its exact canonical Snapshot and #30 runtime. The returned
    /// owner retains every prepared member and its single run target through submission.
    /// Preparation may precede queries in the same encoder; its successful device status
    /// is mandatory at consumption, rather than requiring an intermediate host checkpoint.
    /// # Errors
    /// Rejects non-U/V fields, absent resources, source/time/geometry/context mismatch,
    /// independently initialized targets and invalid canonical time sequences.
    pub fn from_shared_height<'a>(
        ctx: &GpuContext,
        field_id: FieldId,
        snapshots: &[&Snapshot],
        runtimes: &[VerticalRuntimeView<'a>],
        prepared: &[Arc<PreparedSharedHeightUv>],
    ) -> Result<CanonicalGpuField<'a>, MeteorologyCompositionError> {
        let invalid =
            || MeteorologyCompositionError::Incompatible("shared-height U/V resource identity");
        if !matches!(field_id, FieldId::WindU | FieldId::WindV)
            || prepared.is_empty()
            || prepared.len() != snapshots.len()
            || prepared.len() != runtimes.len()
        {
            return Err(invalid());
        }
        let requirements = Requirements {
            required_fields: [field_id].into_iter().collect(),
        };
        let mut members = Vec::new();
        for ((snapshot, runtime), resource) in snapshots.iter().zip(runtimes).zip(prepared) {
            snapshot.validate(&requirements)?;
            let field = snapshot.fields.iter().find(|f| f.id == field_id).ok_or(
                MeteorologyCompositionError::Missing("shared-height U/V field"),
            )?;
            let hash = snapshot_hash(snapshot)?;
            validate_runtime_binding(snapshot, field, *runtime, &hash)?;
            let identity = resource.source_identity();
            if !resource.belongs_to(ctx)
                || !resource.shares_target(&prepared[0])
                || identity.snapshot_sha256 != hash
                || identity.geometry_identity != vertical_geometry_identity(*runtime)
                || serde_json::to_value(&identity.geometry_provenance)?
                    != serde_json::to_value(runtime.provenance())?
                || identity.grid != snapshot.horizontal_grid
                || identity.time != field.time
                || snapshot.vertical_coordinate != snapshots[0].vertical_coordinate
                || field.vertical_staggering != VerticalStaggering::LevelCenter
                || field.horizontal_staggering != HorizontalStaggering::CellCenter
                || field.time.kind != TemporalKind::Instantaneous
            {
                return Err(invalid());
            }
            let target: &SharedHeightTargetIdentity = resource.target().identity();
            let plane_values =
                vec![vec![0.0; identity.grid.nx * identity.grid.ny]; target.height_m_agl.len()];
            let planes = plane_values
                .iter()
                .map(|values| {
                    HorizontalFieldBuffers::from_grid_and_values(
                        ctx,
                        &identity.grid,
                        values,
                        field.horizontal_staggering,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            members.push(SourceMember {
                prepared_w: None,
                prepared_uv: Some(Arc::clone(resource)),
                resident_values: None,
                snapshot: (*snapshot).clone(),
                snapshot_sha256: hash,
                field: field.clone(),
                planes,
                plane_values,
                runtime: Some(RuntimeColumns {
                    resident_heights: None,
                    view: *runtime,
                    heights: Vec::new(),
                    terrain: Vec::new(),
                    identity: identity.geometry_identity.clone(),
                    provenance: identity.geometry_provenance.clone(),
                    motion: None,
                }),
                w_inputs: None,
                shared_heights: None,
            });
        }
        resolve_temporal_bracket(
            field_id,
            snapshots,
            RequestedSampleTime::new(
                members[0].field.time.calendar,
                members[0].field.time.valid_time_epoch_seconds,
            ),
        )?;
        Ok(CanonicalGpuField {
            owner_context: Arc::clone(&ctx.identity),
            field_id,
            members,
            accumulation: None,
        })
    }

    pub(super) fn shared_height_identity(
        &self,
        indices: &[usize],
    ) -> Option<SharedHeightQueryIdentity> {
        let first = self.members[*indices.first()?].prepared_uv.as_ref()?;
        Some(SharedHeightQueryIdentity {
            preparation: if self.members[0].prepared_w.is_some() {
                "shared_height_uvw_190"
            } else {
                "shared_height_uv_184"
            }
            .into(),
            corrected_w: indices
                .iter()
                .filter_map(|i| {
                    self.members[*i]
                        .prepared_w
                        .as_ref()
                        .map(|w| super::shared_w::SharedHeightWQueryIdentity::from_prepared(w))
                })
                .collect(),
            target_identity_sha256: first.target().identity().identity_sha256.clone(),
            target_height_sha256: first.target().identity().height_sha256.clone(),
            source_input_sha256: indices
                .iter()
                .map(|i| {
                    self.members[*i]
                        .prepared_uv
                        .as_ref()
                        .expect("all members have one validated representation")
                        .source_identity()
                        .input_sha256
                        .clone()
                })
                .collect(),
        })
    }
}

impl SourceMember<'_> {
    pub(super) fn prepared_values(&self) -> Option<&wgpu::Buffer> {
        if self.field.id == FieldId::VerticalVelocity {
            return self.prepared_w.as_ref().map(|p| p.w());
        }
        self.prepared_uv.as_ref().map(|p| {
            if self.field.id == FieldId::WindU {
                p.u()
            } else {
                p.v()
            }
        })
    }

    pub(super) fn encode_prepared_planes(&self, encoder: &mut wgpu::CommandEncoder) -> usize {
        if let Some(values) = self.prepared_values() {
            let bytes = (self.snapshot.horizontal_grid.nx * self.snapshot.horizontal_grid.ny)
                as u64
                * VALUE_BYTES;
            for (level, plane) in self.planes.iter().enumerate() {
                encoder.copy_buffer_to_buffer(
                    values,
                    level as u64 * bytes,
                    &plane.buffer,
                    0,
                    bytes,
                );
            }
            return self.planes.len();
        }
        0
    }
}

pub(super) struct PreparationStatusKernel(resident::QueryPass);

/// Prepare the existing #88 grid/query resources without sampling or deriving geometry.
pub(super) fn prepare_vertical(
    ctx: &GpuContext,
    prepared: &PreparedSharedHeightUv,
    height: Option<MeteorologyHeight>,
) -> Result<
    (
        VerticalGridBuffers,
        VerticalQueryBuffers,
        VerticalSampleOutput,
        f32,
    ),
    MeteorologyCompositionError,
> {
    let height = height.ok_or(MeteorologyCompositionError::Missing(
        "shared-height AGL query",
    ))?;
    if height.reference != VerticalReference::AboveGroundLevel || !height.meters.is_finite() {
        return Err(MeteorologyCompositionError::Unsupported(
            "prepared U/V requires finite AGL height",
        ));
    }
    Ok((
        create_vertical_shared_grid(ctx, &prepared.target().identity().height_m_agl)?,
        create_vertical_query_buffers(ctx, &[height.meters])?,
        create_vertical_output_buffer(ctx, 1)?,
        height.meters,
    ))
}

impl PreparedMeteorologySample<'_, '_> {
    /// Attach the existing #171 finite-result check to prepared host-query status.
    pub(super) fn encode_result_status(
        &self,
        ctx: &GpuContext,
        kernels: &super::MeteorologyCompositionKernels<'_>,
        values: &wgpu::Buffer,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if let Some(status) = &self.status {
            kernels.result_status.encode(
                ctx,
                &[values, status, &resident::uniform(ctx, &[1, 1, 0, 0])],
                1,
                encoder,
            );
        }
    }

    /// Recheck immutable preparation after each query-local reset, preserving source failures.
    pub(super) fn encode_preparation_status(
        &mut self,
        ctx: &GpuContext,
        kernels: &super::MeteorologyCompositionKernels<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if let Some(status) = &self.status {
            // Encoder-local reset is ordered after earlier consumers, even on plan reuse.
            let initial = ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("shared U/V query status reset"),
                    contents: bytemuck::cast_slice(&[0_u32, 2]),
                    usage: wgpu::BufferUsages::COPY_SRC,
                });
            encoder.copy_buffer_to_buffer(&initial, 0, status, 0, 8);
            self.device_copies += 1;
            for index in &self.indices {
                kernels.preparation_status.encode(
                    ctx,
                    &self.source.members[*index],
                    status,
                    encoder,
                );
            }
        }
    }
}

impl PreparationStatusKernel {
    pub(super) fn new(ctx: &GpuContext) -> Result<Self, MeteorologyCompositionError> {
        ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let kernel = Self(resident::QueryPass::new(
            ctx,
            "shared-height query preparation status",
            include_str!("../../shaders/shared_height_query_status.wgsl"),
            &[true, true, false, true],
        ));
        if let Some(error) = pollster::block_on(ctx.device.pop_error_scope()) {
            return Err(MeteorologyCompositionError::ResidentDevice(
                error.to_string(),
            ));
        }
        Ok(kernel)
    }

    pub(super) fn encode(
        &self,
        ctx: &GpuContext,
        member: &SourceMember<'_>,
        status: &wgpu::Buffer,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if let Some(prepared) = &member.prepared_uv {
            // Check every source column in a separate pass before any scientific consumer.
            // This also catches a failed column outside the particle's current stencil.
            let columns = u32::try_from(
                prepared.source_identity().grid.nx * prepared.source_identity().grid.ny,
            )
            .expect("#184 validated column count fits u32");
            let dependencies = if let Some(w) = &member.prepared_w {
                vec![
                    (prepared.u(), prepared.status()),
                    (prepared.v(), prepared.status()),
                    (w.w(), w.status()),
                ]
            } else {
                vec![(
                    member
                        .prepared_values()
                        .expect("validated prepared U/V member"),
                    prepared.status(),
                )]
            };
            // Every component of a complete vector requires all three dependencies.
            for (values, preparation) in dependencies {
                self.0.encode(
                    ctx,
                    &[
                        prepared.target().status(),
                        preparation,
                        status,
                        values,
                        &resident::uniform(ctx, &[0; 4]),
                    ],
                    columns,
                    encoder,
                );
            }
        }
    }
}
