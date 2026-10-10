//! GPU-resident native-center U/V preparation, scoped to issue #184.
//! Ports pinned FLEXPART 11.1 `verttransform_mod.f90:296-353,487-623` (eta=no),
//! using the existing #30 `verttransform_ecmwf_heights` geometry unchanged.
//!
//! Native #29/#30 data remain unchanged. The run target retains its initial source;
//! each prepared member retains both source and target until the final consumer.
//! Encoding never submits, polls or downloads. Consumers must bind both statuses
//! and accept values only when target and every required column have status 2.
//! Pending=0/1, invalid=3, nonfinite result=4 are failures, never usable outputs.

use super::{
    meteorology::{snapshot_hash, validate_runtime_binding, MeteorologyCompositionError},
    vertical::{physical_model_column_from_runtime, sha256_hex, vertical_geometry_identity},
    GpuContext,
};
use crate::meteorology::{
    vertical::{VerticalRuntimeView, VerticalTransformProvenance},
    FieldId, FieldTime, HorizontalGrid, HorizontalStaggering, Requirements, Snapshot, TemporalKind,
};
use bytemuck::{Pod, Zeroable};
use serde::Serialize;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// Exact shader source retained for scientific execution hashes.
pub const SHARED_HEIGHT_SHADER: &str = include_str!("../shaders/shared_height_uv.wgsl");
const INITIAL_SELECTION_PRESSURE_PA: f32 = 100_000.0;
type Error = MeteorologyCompositionError;
fn invalid(message: &'static str) -> Error {
    Error::Incompatible(message)
}

/// Explicit initializer choices; only the direct #118 regional mother route is supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedHeightInitialization {
    /// First load, regional mother grid, no restart or nests.
    RegionalMother,
    /// Requires a separately proved persisted-height contract.
    Restart,
    /// Requires a separately proved nested initializer.
    Nested,
    /// Requires a separately proved global/polar initializer.
    GlobalOrPolar,
}

/// Immutable source identity binding canonical fields to #30 geometry and time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SharedHeightSourceIdentity {
    /// Opaque run-local identity of the existing `GpuContext` token, retained by the source.
    pub device_context_identity: String,
    /// Complete immutable #29 snapshot hash, including field values and time.
    pub snapshot_sha256: String,
    /// Existing #30 geometry hash; no reconstruction is performed here.
    pub geometry_identity: String,
    /// Exact #30 transformation lineage.
    pub geometry_provenance: VerticalTransformProvenance,
    /// Instantaneous U/V source time.
    pub time: FieldTime,
    /// Native grid including origin and spacing.
    pub grid: HorizontalGrid,
    /// Native count including artificial ground.
    pub levels: u32,
    /// Hash of explicitly uploaded [height,U,V,ps] f32 lanes.
    pub input_sha256: String,
}

/// Immutable explicit H2D source owner. Clone shares buffers, never overwrites a member.
#[derive(Clone)]
pub struct SharedHeightSource(Arc<Source>);
struct Source {
    context: Arc<()>,
    columns: u32,
    identity: SharedHeightSourceIdentity,
    coordinate_sha256: String,
    native: wgpu::Buffer,
    initial_heights: Vec<f32>,
    initial_pressure_pa: f32,
}

/// Run-lifetime target provenance; host bytes are an audit expectation of the device copy.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SharedHeightTargetIdentity {
    /// Initial mother member, including geometry and time identity.
    pub initial_source: SharedHeightSourceIdentity,
    /// Exact accepted selection rule.
    pub selection_rule: String,
    /// Zero-based x/y of the GPU-selected source column.
    pub selected_xy: [u32; 2],
    /// Expected f32 AGL target bytes from #30, ground first.
    pub height_m_agl: Vec<f32>,
    /// SHA-256 of expected target bytes; validate against actual GPU bytes in evidence.
    pub height_sha256: String,
    /// Complete target provenance identity, including source and selection.
    pub identity_sha256: String,
}

/// Shared GPU target. Its initializer is encoded exactly once by the private constructor.
#[derive(Clone)]
pub struct SharedHeightTarget(Arc<Target>);
struct Target {
    source: SharedHeightSource,
    identity: SharedHeightTargetIdentity,
    heights: wgpu::Buffer,
    status: wgpu::Buffer,
}

/// Immutable U/V scalar resources in x-fastest [target-level,y,x] order, m/s.
/// Retain this owner through the last submitted consumer; status gates all values.
pub struct PreparedSharedHeightUv {
    source: SharedHeightSource,
    target: SharedHeightTarget,
    u: wgpu::Buffer,
    v: wgpu::Buffer,
    status: wgpu::Buffer,
}

/// Typed physical/storage contract lent with each immutable prepared member.
#[derive(Debug, Serialize)]
pub struct PreparedSharedHeightMetadata<'a> {
    /// Independent U/V scalar fields, in this order.
    pub fields: [FieldId; 2],
    /// Both fields represent velocity in metres per second.
    pub unit: crate::meteorology::Unit,
    /// Preserved eastward/northward signs.
    pub signs: [crate::meteorology::SignConvention; 2],
    /// Accepted cell-center horizontal layout.
    pub horizontal_staggering: HorizontalStaggering,
    /// Centers on the common grid, including artificial ground.
    pub vertical_staggering: crate::meteorology::VerticalStaggering,
    /// Common grid is above local ground, never ASL.
    pub height_reference: crate::meteorology::VerticalReference,
    /// Exact scalar field indexing contract.
    pub storage_layout: &'static str,
    /// Immutable canonical member and its context token.
    pub source: &'a SharedHeightSourceIdentity,
    /// Initial source, selected column, bytes/hash and run-target identity.
    pub target: &'a SharedHeightTargetIdentity,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Parameters {
    columns: u32,
    levels: u32,
    initialize: u32,
    padding: u32,
}

/// Persistent pipeline under the existing `GpuContext`; no separate runtime or submission.
pub struct SharedHeightPreparer {
    context: Arc<()>,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    parameters: wgpu::BindGroupLayout,
    empty: wgpu::BindGroup,
}

/// Check finite dimensions, WGSL index arithmetic, binding and dispatch limits before allocation.
/// # Errors
/// Unsupported sizes/limits are explicit failures without a CPU fallback.
pub fn validate_shared_height_limits(
    limits: &wgpu::Limits,
    columns: u32,
    levels: u32,
) -> Result<(), Error> {
    let words = columns
        .checked_mul(levels)
        .ok_or_else(|| invalid("shared-height index overflow"))?;
    if columns == 0
        || levels < 3
        || columns.div_ceil(64) > limits.max_compute_workgroups_per_dimension
        || limits.max_storage_buffers_per_shader_stage < 6
        || limits.max_bind_groups < 3
        || limits.max_bindings_per_bind_group < 6
        || limits.max_compute_workgroup_size_x < 64
        || limits.max_compute_invocations_per_workgroup < 64
        || limits.max_uniform_buffer_binding_size < 16
        || u64::from(words) * 16
            > limits
                .max_buffer_size
                .min(u64::from(limits.max_storage_buffer_binding_size))
    {
        return Err(invalid(
            "shared-height dimensions or bindings exceed device limits",
        ));
    }
    Ok(())
}

fn storage(ctx: &GpuContext, bytes: u64, label: &str) -> wgpu::Buffer {
    ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}
fn field(snapshot: &Snapshot, id: FieldId) -> Result<&crate::meteorology::Field, Error> {
    snapshot
        .fields
        .iter()
        .find(|f| f.id == id)
        .ok_or(Error::Missing("shared-height field"))
}

fn validate_source_fields(snapshot: &Snapshot, time: &FieldTime) -> Result<(), Error> {
    for id in [
        FieldId::WindU,
        FieldId::WindV,
        FieldId::WindU10m,
        FieldId::WindV10m,
        FieldId::SurfacePressure,
    ] {
        let f = field(snapshot, id)?;
        if f.horizontal_staggering != HorizontalStaggering::CellCenter
            || &f.time != time
            || f.time.kind != TemporalKind::Instantaneous
        {
            return Err(invalid("shared-height staggering or source time"));
        }
    }
    Ok(())
}

impl SharedHeightSource {
    /// Validate immutable canonical U/V, required U10/V10/pressure and matching #30 geometry,
    /// then explicitly upload native lanes. Adds only artificial zero, never derives heights.
    /// # Errors
    /// Rejects missing/nonfinite fields, staggering, time, geometry, pressure and size errors.
    pub fn upload(
        ctx: &GpuContext,
        snapshot: &Snapshot,
        runtime: VerticalRuntimeView<'_>,
    ) -> Result<Self, Error> {
        snapshot.validate(&Requirements {
            required_fields: [
                FieldId::WindU,
                FieldId::WindV,
                FieldId::WindU10m,
                FieldId::WindV10m,
                FieldId::SurfacePressure,
            ]
            .into_iter()
            .collect(),
        })?;
        let hash = snapshot_hash(snapshot)?;
        let u = field(snapshot, FieldId::WindU)?;
        let v = field(snapshot, FieldId::WindV)?;
        validate_runtime_binding(snapshot, u, runtime, &hash)?;
        let grid = &snapshot.horizontal_grid;
        let (nx, ny, nz) = runtime.dimensions();
        let columns = u32::try_from(
            nx.checked_mul(ny)
                .ok_or_else(|| invalid("column overflow"))?,
        )
        .map_err(|_| invalid("column overflow"))?;
        let levels = u32::try_from(nz.checked_add(1).ok_or_else(|| invalid("level overflow"))?)
            .map_err(|_| invalid("level overflow"))?;
        validate_shared_height_limits(&ctx.device.limits(), columns, levels)?;
        if grid.is_periodic_x()
            || grid.ylat0_deg - grid.dy_deg / 2.0 <= -90.0
            || grid.ylat0_deg
                + (f64::from(u32::try_from(ny).map_err(|_| invalid("y count overflow"))?) - 0.5)
                    * grid.dy_deg
                >= 90.0
        {
            return Err(Error::Unsupported("shared-height global/polar geometry"));
        }
        validate_source_fields(snapshot, &u.time)?;
        let u10 = field(snapshot, FieldId::WindU10m)?;
        let v10 = field(snapshot, FieldId::WindV10m)?;
        let pressure = field(snapshot, FieldId::SurfacePressure)?;
        let mut lanes = vec![[0.0_f32; 4]; columns as usize * levels as usize];
        let mut initial_heights = vec![0.0];
        for y in 0..ny {
            for x in 0..nx {
                let column = x + nx * y;
                let ps = pressure.values[column];
                if !ps.is_finite() || ps <= 0.0 {
                    return Err(invalid("invalid shared-height surface pressure"));
                }
                let (heights, us) =
                    physical_model_column_from_runtime(runtime, FieldId::WindU, &u.values, x, y)?;
                let (_, vs) =
                    physical_model_column_from_runtime(runtime, FieldId::WindV, &v.values, x, y)?;
                if heights[0] <= 0.0 {
                    return Err(invalid(
                        "real native levels must lie above artificial ground",
                    ));
                }
                lanes[column] = [0.0, u10.values[column], v10.values[column], ps];
                for z in 0..nz {
                    lanes[(z + 1) * columns as usize + column] = [heights[z], us[z], vs[z], ps];
                }
                if column == 0 {
                    initial_heights.extend(heights);
                }
            }
        }
        let bytes = bytemuck::cast_slice(&lanes);
        let identity = SharedHeightSourceIdentity {
            device_context_identity: sha256_hex(
                format!("{:p}", Arc::as_ptr(&ctx.identity)).as_bytes(),
            ),
            snapshot_sha256: hash,
            geometry_identity: vertical_geometry_identity(runtime),
            geometry_provenance: runtime.provenance().clone(),
            time: u.time.clone(),
            grid: grid.clone(),
            levels,
            input_sha256: sha256_hex(bytes),
        };
        Ok(Self(Arc::new(Source {
            context: Arc::clone(&ctx.identity),
            columns,
            identity,
            coordinate_sha256: sha256_hex(&serde_json::to_vec(&snapshot.vertical_coordinate)?),
            native: ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("shared-height explicit native upload"),
                    contents: bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                }),
            initial_heights,
            initial_pressure_pa: pressure.values[0],
        })))
    }

    /// Borrow exact source provenance without allowing metadata mutation.
    #[must_use]
    pub fn identity(&self) -> &SharedHeightSourceIdentity {
        &self.0.identity
    }
}

impl SharedHeightTarget {
    /// Borrow run-lifetime provenance; expected bytes are verified against GPU output.
    #[must_use]
    pub fn identity(&self) -> &SharedHeightTargetIdentity {
        &self.0.identity
    }
    /// Lend device heights for status-aware consumers and explicit validation readback.
    #[must_use]
    pub fn heights(&self) -> &wgpu::Buffer {
        &self.0.heights
    }
    /// Target status: consumers require word zero == 2 before using any prepared value.
    #[must_use]
    pub fn status(&self) -> &wgpu::Buffer {
        &self.0.status
    }
}

impl SharedHeightPreparer {
    /// Create a scoped, fallible pipeline on the configured device.
    /// # Errors
    /// Unsupported limits or wgpu validation/OOM/internal errors fail initialization.
    pub async fn new(ctx: &GpuContext) -> Result<Self, Error> {
        validate_shared_height_limits(&ctx.device.limits(), 1, 3)?;
        for filter in [
            wgpu::ErrorFilter::OutOfMemory,
            wgpu::ErrorFilter::Internal,
            wgpu::ErrorFilter::Validation,
        ] {
            ctx.device.push_error_scope(filter);
        }
        let entries: Vec<_> = (0..6)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: binding == 0,
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
                label: Some("shared-height resources"),
                entries: &entries,
            });
        let parameters = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shared-height parameters"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let empty_layout = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("unused shared-height group"),
                entries: &[],
            });
        let empty = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &empty_layout,
            entries: &[],
        });
        let pipeline_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&layout, &empty_layout, &parameters],
                push_constant_ranges: &[],
            });
        let shader = ctx.load_shader("shared-height U/V", SHARED_HEIGHT_SHADER);
        let pipeline = ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("shared-height U/V"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let mut failure = None;
        for _ in 0..3 {
            if let Some(error) = ctx.device.pop_error_scope().await {
                failure = Some(error.to_string());
            }
        }
        if let Some(error) = failure {
            return Err(Error::ResidentDevice(error));
        }
        Ok(Self {
            context: Arc::clone(&ctx.identity),
            pipeline,
            layout,
            parameters,
            empty,
        })
    }

    fn check_context(&self, ctx: &GpuContext, source: &SharedHeightSource) -> Result<(), Error> {
        if !Arc::ptr_eq(&self.context, &ctx.identity)
            || !Arc::ptr_eq(&source.0.context, &ctx.identity)
        {
            return Err(invalid("shared-height device ownership"));
        }
        Ok(())
    }

    /// Encode the accepted GPU column selection/copy once. Retain the returned target for the run.
    /// # Errors
    /// Other initializer variants and pressure-selection fallback fail before producing a target.
    pub fn initialize(
        &self,
        ctx: &GpuContext,
        source: &SharedHeightSource,
        mode: SharedHeightInitialization,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<SharedHeightTarget, Error> {
        self.check_context(ctx, source)?;
        if mode != SharedHeightInitialization::RegionalMother {
            return Err(Error::Unsupported("shared-height initializer variant"));
        }
        if source.0.initial_pressure_pa <= INITIAL_SELECTION_PRESSURE_PA {
            return Err(Error::Unsupported(
                "shared-height (0,0) selection requires ps > 100000 Pa",
            ));
        }
        let mut identity = SharedHeightTargetIdentity {
            initial_source: source.identity().clone(),
            selection_rule:
                "regional_mother_nonrestart_first_y_then_x_ps_gt_100000_at_0_0_ground_zero".into(),
            selected_xy: [0, 0],
            height_sha256: sha256_hex(bytemuck::cast_slice(&source.0.initial_heights)),
            height_m_agl: source.0.initial_heights.clone(),
            identity_sha256: String::new(),
        };
        identity.identity_sha256 = sha256_hex(&serde_json::to_vec(&identity)?);
        let target = SharedHeightTarget(Arc::new(Target {
            source: source.clone(),
            heights: storage(
                ctx,
                u64::from(source.identity().levels) * 4,
                "run shared heights",
            ),
            status: storage(ctx, 4, "run shared height status"),
            identity,
        }));
        let dummy_u = storage(ctx, 4, "initializer unused U");
        let dummy_v = storage(ctx, 4, "initializer unused V");
        let dummy_status = storage(ctx, 4, "initializer unused member status");
        self.encode(
            ctx,
            source,
            &target,
            [&dummy_u, &dummy_v, &dummy_status],
            true,
            encoder,
        );
        Ok(target)
    }

    /// Encode a new immutable member against the retained target in the caller encoder.
    /// Both bracket members can coexist; no target is regenerated or source overwritten.
    /// # Errors
    /// Source/target device, grid, vertical-coordinate or level-count mismatch fails closed.
    pub fn prepare(
        &self,
        ctx: &GpuContext,
        source: &SharedHeightSource,
        target: &SharedHeightTarget,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<PreparedSharedHeightUv, Error> {
        self.check_context(ctx, source)?;
        self.check_context(ctx, &target.0.source)?;
        if source.identity().grid != target.0.source.identity().grid
            || source.identity().levels != target.0.source.identity().levels
            || source.0.coordinate_sha256 != target.0.source.0.coordinate_sha256
        {
            return Err(invalid("shared-height grid/coordinate/count mismatch"));
        }
        let columns = source.0.columns;
        let bytes = u64::from(columns) * u64::from(source.identity().levels) * 4;
        let prepared = PreparedSharedHeightUv {
            source: source.clone(),
            target: target.clone(),
            u: storage(ctx, bytes, "prepared shared U m/s"),
            v: storage(ctx, bytes, "prepared shared V m/s"),
            status: storage(ctx, u64::from(columns) * 4, "shared U/V column status"),
        };
        self.encode(
            ctx,
            source,
            target,
            [&prepared.u, &prepared.v, &prepared.status],
            false,
            encoder,
        );
        Ok(prepared)
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        source: &SharedHeightSource,
        target: &SharedHeightTarget,
        outputs: [&wgpu::Buffer; 3],
        initialize: bool,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let columns = source.0.columns;
        let params = Parameters {
            columns,
            levels: source.identity().levels,
            initialize: u32::from(initialize),
            padding: 0,
        };
        let uniform = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shared-height parameters"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let resources = [
            &source.0.native,
            &target.0.heights,
            &target.0.status,
            outputs[0],
            outputs[1],
            outputs[2],
        ];
        let entries: Vec<_> = resources
            .iter()
            .zip(0_u32..)
            .map(|(buffer, binding)| wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            })
            .collect();
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &entries,
        });
        let params_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.parameters,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("shared-height preparation"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_bind_group(1, &self.empty, &[]);
        pass.set_bind_group(2, &params_group, &[]);
        pass.dispatch_workgroups(if initialize { 1 } else { columns.div_ceil(64) }, 1, 1);
    }
}

impl PreparedSharedHeightUv {
    /// Lend explicit physical metadata together with exact source and target provenance.
    #[must_use]
    pub fn metadata(&self) -> PreparedSharedHeightMetadata<'_> {
        PreparedSharedHeightMetadata {
            fields: [FieldId::WindU, FieldId::WindV],
            unit: crate::meteorology::Unit::MeterPerSecond,
            signs: [
                crate::meteorology::SignConvention::PositiveEastward,
                crate::meteorology::SignConvention::PositiveNorthward,
            ],
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: crate::meteorology::VerticalStaggering::LevelCenter,
            height_reference: crate::meteorology::VerticalReference::AboveGroundLevel,
            storage_layout: "[target-level,y,x]; x fastest",
            source: self.source_identity(),
            target: self.target().identity(),
        }
    }
    /// Check exact reuse across adjacent brackets, including source fields/time/geometry and target owner.
    /// # Errors
    /// Any incompatible source, target or context prevents reuse of this member.
    pub fn require_reuse(
        &self,
        ctx: &GpuContext,
        source: &SharedHeightSource,
        target: &SharedHeightTarget,
    ) -> Result<(), Error> {
        if !Arc::ptr_eq(&self.source.0.context, &ctx.identity)
            || !Arc::ptr_eq(&source.0.context, &ctx.identity)
            || self.source.identity() != source.identity()
            || !Arc::ptr_eq(&self.target.0, &target.0)
        {
            return Err(invalid("incompatible shared-height member reuse"));
        }
        Ok(())
    }
    /// Read-only scalar U binding, m/s in [target-level,y,x] storage order.
    #[must_use]
    pub fn u(&self) -> &wgpu::Buffer {
        &self.u
    }
    /// Read-only scalar V binding, m/s in [target-level,y,x] storage order.
    #[must_use]
    pub fn v(&self) -> &wgpu::Buffer {
        &self.v
    }
    /// One status per column; every required column must equal 2 before acceptance.
    #[must_use]
    pub fn status(&self) -> &wgpu::Buffer {
        &self.status
    }
    /// Retained run target, including its mandatory status resource.
    #[must_use]
    pub fn target(&self) -> &SharedHeightTarget {
        &self.target
    }
    /// Exact member identity for device handoff and provenance.
    #[must_use]
    pub fn source_identity(&self) -> &SharedHeightSourceIdentity {
        self.source.identity()
    }
}
