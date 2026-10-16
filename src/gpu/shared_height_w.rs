//! Immutable GPU W preparation for #188; pinned verttransform_mod.f90:489-655.
//! Consumes #30 normalized interfaces and #184 target/U/V. Status 2 is the only
//! accepted completed state; 0=pending, 1=remapped, 3=invalid, 4=nonfinite result.
//! Owners retain all dependencies through the last submitted consumer. No
//! encoding method submits, polls, maps or downloads.

use super::{
    meteorology::{snapshot_hash, validate_runtime_binding, MeteorologyCompositionError},
    shared_height::{validate_shared_height_limits, PreparedSharedHeightUv},
    vertical::{sha256_hex, vertical_geometry_identity},
    GpuContext,
};
use crate::meteorology::{
    vertical::{
        NativeVerticalMotionKind, NativeVerticalMotionSign, NativeVerticalMotionUnit,
        NormalizedVerticalMotionProvenance, VerticalRuntimeView,
    },
    Snapshot, VerticalOrdering, VerticalStaggering,
};
use bytemuck::{Pod, Zeroable};
use serde::Serialize;
use std::sync::Arc;
use wgpu::util::DeviceExt;

type Error = MeteorologyCompositionError;
/// Exact interface-remap WGSL retained for scientific hashes.
pub const W_REMAP_SHADER: &str = include_str!("../shaders/shared_height_w_remap.wgsl");
/// Exact slope WGSL retained for scientific hashes.
pub const W_SLOPE_SHADER: &str = include_str!("../shaders/interior_w_slope.wgsl");
/// Completed correction, distinct from native normalization and query adoption.
pub const W_CORRECTION_ID: &str = "flexpart11_regional_interior_w_slope_v1";
fn invalid(reason: &'static str) -> Error {
    Error::Incompatible(reason)
}

/// Source-interface and horizontal identities complement the retained #184 metadata.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SharedHeightWIdentity {
    /// Exact normalized #30 motion lineage; never normalized a second time.
    pub normalization: NormalizedVerticalMotionProvenance,
    /// Hash of physical bottom-to-top [interface AGL m, normalized W m/s] lanes.
    pub interface_input_sha256: String,
    /// Canonical horizontal geometry including regional identity and latitude.
    pub horizontal_sha256: String,
    /// Identity of the exact same-source U/V/target handoff.
    pub prepared_uv_sha256: String,
    /// Identifies the completed transformation requested by production preparation.
    pub correction_algorithm: &'static str,
}

/// Explicit immutable normalized-interface upload retaining its matching prepared U/V.
#[derive(Clone)]
pub struct SharedHeightWSource(Arc<WSource>);
struct WSource {
    uv: Arc<PreparedSharedHeightUv>,
    identity: SharedHeightWIdentity,
    interfaces: wgpu::Buffer,
}

/// Immutable corrected W, m/s positive upward, x-fastest [target-level,y,x].
/// A consumer must require target, U/V and every W column status == 2.
pub struct PreparedSharedHeightW {
    source: SharedHeightWSource,
    fields: WFields,
}
/// Physical output contract bound to the exact native and shared-grid dependencies.
#[derive(Debug, Serialize)]
pub struct PreparedSharedHeightWMetadata<'a> {
    /// Vertical velocity in metres per second, positive upward.
    pub field: crate::meteorology::FieldId,
    /// Physical velocity unit after the already completed #30 normalization.
    pub unit: crate::meteorology::Unit,
    /// Sign of geometric vertical air velocity.
    pub sign: crate::meteorology::SignConvention,
    /// Source interfaces remain distinct from target metric levels.
    pub source_staggering: VerticalStaggering,
    /// Shared targets measure height above local ground.
    pub height_reference: crate::meteorology::VerticalReference,
    /// Scalar indexing shared with #184 U/V.
    pub storage_layout: &'static str,
    /// Applied full-correction algorithm, accepted only with resident status 2.
    pub correction_algorithm: &'static str,
    /// Native interface, horizontal geometry and normalization identity.
    pub source: &'a SharedHeightWIdentity,
    /// Exact source time, context, native geometry and shared target.
    pub prepared_uv: super::shared_height::PreparedSharedHeightMetadata<'a>,
}
struct WFields {
    w: wgpu::Buffer,
    status: wgpu::Buffer,
}

/// Diagnostic outputs cannot be passed as a production prepared-W owner.
/// Zeroed controls change factors only; positive production geometry stays mandatory.
pub struct DiagnosticSharedHeightW {
    source: SharedHeightWSource,
    fields: WFields,
}

/// Validation-only controls matching four genuine #186 routine invocations.
#[derive(Debug, Clone, Copy)]
pub enum WValidationControl {
    /// Keep both physical spacing factors.
    Full,
    /// Zero only the Y factor.
    XOnly,
    /// Zero only the X factor.
    YOnly,
    /// Zero both factors after physical geometry validation.
    Zero,
}
impl WValidationControl {
    fn factors(self) -> [u32; 2] {
        match self {
            Self::Full => [1, 1],
            Self::XOnly => [1, 0],
            Self::YOnly => [0, 1],
            Self::Zero => [0, 0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Parameters {
    nx: u32,
    ny: u32,
    levels: u32,
    padding: u32,
    dx_deg: f32,
    dy_deg: f32,
    latitude_origin_deg: f32,
    padding2: f32,
    x_enabled: u32,
    y_enabled: u32,
    padding3: [u32; 2],
}

/// Check all W buffer/index/dispatch limits before any allocation.
/// # Errors
/// Unsupported binding, uniform, allocation or index limits fail closed.
pub fn validate_shared_w_limits(
    limits: &wgpu::Limits,
    nx: u32,
    ny: u32,
    levels: u32,
) -> Result<(), Error> {
    let columns = nx
        .checked_mul(ny)
        .ok_or_else(|| invalid("W column overflow"))?;
    validate_shared_height_limits(limits, columns, levels)?;
    if nx < 2
        || ny < 2
        || limits.max_storage_buffers_per_shader_stage < 8
        || limits.max_bindings_per_bind_group < 8
        || limits.max_uniform_buffer_binding_size < 48
    {
        return Err(invalid("unsupported W dimensions or bindings"));
    }
    Ok(())
}

fn validate_normalization(
    motion: &crate::meteorology::vertical::NormalizedVerticalMotion,
) -> Result<(), Error> {
    let p = motion.provenance();
    if motion.vertical_staggering() != VerticalStaggering::LevelInterface
        || p.source_vertical_staggering != VerticalStaggering::LevelInterface
        || p.output_vertical_staggering != VerticalStaggering::LevelInterface
        || p.source_kind != NativeVerticalMotionKind::PressureVelocityOmega
        || p.source_unit != NativeVerticalMotionUnit::PascalPerSecond
        || p.source_sign != NativeVerticalMotionSign::PositivePressureIncreasing
        || p.algorithm_id != "omega_interface_flexpart11_pinmconv_v1"
    {
        return Err(Error::Unsupported(
            "W requires #30 normalized interface-omega lineage",
        ));
    }
    Ok(())
}

fn physical_interface_lanes(
    runtime: VerticalRuntimeView<'_>,
    motion: &crate::meteorology::vertical::NormalizedVerticalMotion,
) -> Result<Vec<[f32; 2]>, Error> {
    let (nx, ny, nz) = runtime.dimensions();
    let columns = nx * ny;
    let levels = nz + 1;
    if motion.values_ms().len() != columns * levels {
        return Err(invalid("W interface shape"));
    }
    let mut lanes = vec![[0.0; 2]; columns * levels];
    for y in 0..ny {
        for x in 0..nx {
            let column = y * nx + x;
            let mut previous = -1.0;
            for z in 0..levels {
                let canonical = match runtime.provenance().source_vertical_ordering {
                    VerticalOrdering::Increasing => levels - 1 - z,
                    VerticalOrdering::Decreasing => z,
                };
                let height = runtime.interface(x, y, canonical)?.height_agl_m;
                let value = motion.values_ms()[canonical * columns + column];
                if !height.is_finite()
                    || height <= previous
                    || !value.is_finite()
                    || (z == 0 && height != 0.0)
                {
                    return Err(invalid(
                        "nonfinite/unordered W interface geometry or values",
                    ));
                }
                lanes[z * columns + column] = [height, value];
                previous = height;
            }
        }
    }
    Ok(lanes)
}

impl SharedHeightWSource {
    /// Upload matching opaque #30 normalized interface motion, reusing #184's native centers.
    /// No raw motion, corrected W or independently chosen target can enter this API.
    /// # Errors
    /// Reject mismatched source/context/geometry and unsupported motion or geometry.
    pub fn upload(
        ctx: &GpuContext,
        snapshot: &Snapshot,
        runtime: VerticalRuntimeView<'_>,
        uv: Arc<PreparedSharedHeightUv>,
    ) -> Result<Self, Error> {
        uv.require_reuse(ctx, uv.source(), uv.target())?;
        let source = uv.source_identity();
        if snapshot_hash(snapshot)? != source.snapshot_sha256
            || vertical_geometry_identity(runtime) != source.geometry_identity
            || runtime.provenance() != &source.geometry_provenance
        {
            return Err(invalid(
                "W source snapshot/time/geometry differs from prepared U/V",
            ));
        }
        let (nx, ny, nz) = runtime.dimensions();
        let width = u32::try_from(nx).map_err(|_| invalid("W x overflow"))?;
        let row_count = u32::try_from(ny).map_err(|_| invalid("W y overflow"))?;
        validate_shared_w_limits(&ctx.device.limits(), width, row_count, source.levels)?;
        if nz + 1 != source.levels as usize {
            return Err(invalid("W level count"));
        }
        let grid = &source.grid;
        // f64 canonical metadata must remain a supported finite regional grid in WGSL f32.
        #[allow(clippy::cast_possible_truncation)]
        let geometry = [
            grid.dx_deg as f32,
            grid.dy_deg as f32,
            grid.ylat0_deg as f32,
        ];
        #[allow(clippy::cast_precision_loss)]
        let rows_f32 = row_count as f32;
        if !geometry.iter().all(|v| v.is_finite())
            || geometry[0] <= 0.0
            || geometry[1] <= 0.0
            || geometry[2] - geometry[1] / 2.0 <= -90.0
            || geometry[2] + (rows_f32 - 0.5) * geometry[1] >= 90.0
            || grid.is_periodic_x()
        {
            return Err(invalid(
                "W requires finite positive spacing and regional nonpolar geometry",
            ));
        }
        let motion = runtime
            .vertical_velocity()
            .ok_or(Error::Missing("normalized interface W"))?;
        validate_normalization(motion)?;
        if let Some(field) = snapshot
            .fields
            .iter()
            .find(|f| f.id == crate::meteorology::FieldId::VerticalVelocity)
        {
            validate_runtime_binding(snapshot, field, runtime, &source.snapshot_sha256)?;
            if field.time != source.time {
                return Err(invalid("canonical W time differs from prepared U/V"));
            }
        }
        let p = motion.provenance();
        let lanes = physical_interface_lanes(runtime, motion)?;
        let bytes = bytemuck::cast_slice(&lanes);
        let identity = SharedHeightWIdentity {
            normalization: p.clone(),
            interface_input_sha256: sha256_hex(bytes),
            horizontal_sha256: sha256_hex(&serde_json::to_vec(grid)?),
            prepared_uv_sha256: sha256_hex(&serde_json::to_vec(&uv.metadata())?),
            correction_algorithm: W_CORRECTION_ID,
        };
        Ok(Self(Arc::new(WSource {
            uv,
            identity,
            interfaces: ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("explicit normalized W interface upload"),
                    contents: bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                }),
        })))
    }
    /// Borrow immutable motion/geometry/correction identities.
    #[must_use]
    pub fn identity(&self) -> &SharedHeightWIdentity {
        &self.0.identity
    }
    /// Retained same-source U/V and run target, including mandatory statuses.
    #[must_use]
    pub fn uv(&self) -> &PreparedSharedHeightUv {
        &self.0.uv
    }
}

struct Stage {
    pipeline: wgpu::ComputePipeline,
}
impl Stage {
    fn new(ctx: &GpuContext, shader: &str, label: &str) -> Self {
        let module = ctx.load_shader(label, shader);
        Self {
            pipeline: ctx
                .device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: None,
                    module: &module,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                }),
        }
    }
    fn encode(
        &self,
        ctx: &GpuContext,
        buffers: &[&wgpu::Buffer],
        uniform: &wgpu::Buffer,
        columns: u32,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let entries: Vec<_> = buffers
            .iter()
            .zip(0u32..)
            .map(|(b, binding)| wgpu::BindGroupEntry {
                binding,
                resource: b.as_entire_binding(),
            })
            .collect();
        let resources = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("W resident resources"),
            layout: &self.pipeline.get_bind_group_layout(1),
            entries: &entries,
        });
        let empty = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[],
        });
        let params = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.pipeline.get_bind_group_layout(2),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("W preparation"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &empty, &[]);
        pass.set_bind_group(1, &resources, &[]);
        pass.set_bind_group(2, &params, &[]);
        pass.dispatch_workgroups(columns.div_ceil(64), 1, 1);
    }
}

/// Two process kernels sharing the existing context and caller encoder.
pub struct SharedHeightWPreparer {
    context: Arc<()>,
    remap: Stage,
    slope: Stage,
}
impl SharedHeightWPreparer {
    /// Construct fallible pipelines, surfacing scoped validation/OOM/internal failures.
    /// # Errors
    /// Inadequate device limits or shader/pipeline errors fail initialization.
    pub async fn new(ctx: &GpuContext) -> Result<Self, Error> {
        validate_shared_w_limits(&ctx.device.limits(), 2, 2, 3)?;
        for filter in [
            wgpu::ErrorFilter::OutOfMemory,
            wgpu::ErrorFilter::Internal,
            wgpu::ErrorFilter::Validation,
        ] {
            ctx.device.push_error_scope(filter);
        }
        let remap = Stage::new(ctx, W_REMAP_SHADER, "shared-height W remap");
        let slope = Stage::new(ctx, W_SLOPE_SHADER, "interior W slope");
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
            remap,
            slope,
        })
    }
    /// Encode full physical correction only, after normalized-interface remapping.
    /// # Errors
    /// Wrong context or dependency ownership fails before encoding.
    pub fn prepare(
        &self,
        ctx: &GpuContext,
        source: &SharedHeightWSource,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<PreparedSharedHeightW, Error> {
        let fields = self.encode(ctx, source, WValidationControl::Full, encoder)?;
        Ok(PreparedSharedHeightW {
            source: source.clone(),
            fields,
        })
    }
    /// Encode a genuine-oracle diagnostic; the result has no production-owner conversion.
    /// # Errors
    /// Same fail-closed geometry and context rules as full production preparation.
    pub fn prepare_validation_control(
        &self,
        ctx: &GpuContext,
        source: &SharedHeightWSource,
        control: WValidationControl,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<DiagnosticSharedHeightW, Error> {
        let fields = self.encode(ctx, source, control, encoder)?;
        Ok(DiagnosticSharedHeightW {
            source: source.clone(),
            fields,
        })
    }
    fn encode(
        &self,
        ctx: &GpuContext,
        source: &SharedHeightWSource,
        control: WValidationControl,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<WFields, Error> {
        if !Arc::ptr_eq(&self.context, &ctx.identity) {
            return Err(invalid("W preparer context"));
        }
        let uv = source.uv();
        uv.require_reuse(ctx, uv.source(), uv.target())?;
        let identity = uv.source_identity();
        let nx = u32::try_from(identity.grid.nx).map_err(|_| invalid("W nx overflow"))?;
        let ny = u32::try_from(identity.grid.ny).map_err(|_| invalid("W ny overflow"))?;
        validate_shared_w_limits(&ctx.device.limits(), nx, ny, identity.levels)?;
        let columns = nx * ny;
        let allocate = |size, label| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let fields = WFields {
            w: allocate(
                u64::from(columns) * u64::from(identity.levels) * 4,
                "prepared shared W m/s",
            ),
            status: allocate(u64::from(columns) * 4, "corrected W column status"),
        };
        let factors = control.factors();
        #[allow(clippy::cast_possible_truncation)]
        let parameters = Parameters {
            nx,
            ny,
            levels: identity.levels,
            padding: 0,
            dx_deg: identity.grid.dx_deg as f32,
            dy_deg: identity.grid.dy_deg as f32,
            latitude_origin_deg: identity.grid.ylat0_deg as f32,
            padding2: 0.0,
            x_enabled: factors[0],
            y_enabled: factors[1],
            padding3: [0; 2],
        };
        let uniform = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("W physical geometry"),
                contents: bytemuck::bytes_of(&parameters),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        self.remap.encode(
            ctx,
            &[
                &source.0.interfaces,
                uv.target().heights(),
                uv.target().status(),
                &fields.w,
                &fields.status,
            ],
            &uniform,
            columns,
            encoder,
        );
        self.slope.encode(
            ctx,
            &[
                uv.native(),
                uv.u(),
                uv.v(),
                uv.target().heights(),
                uv.target().status(),
                uv.status(),
                &fields.w,
                &fields.status,
            ],
            &uniform,
            columns,
            encoder,
        );
        Ok(fields)
    }
}

impl PreparedSharedHeightW {
    /// Lend immutable physical metadata; completion is checked through the resident statuses.
    #[must_use]
    pub fn metadata(&self) -> PreparedSharedHeightWMetadata<'_> {
        PreparedSharedHeightWMetadata {
            field: crate::meteorology::FieldId::VerticalVelocity,
            unit: crate::meteorology::Unit::MeterPerSecond,
            sign: crate::meteorology::SignConvention::PositiveUpward,
            source_staggering: VerticalStaggering::LevelInterface,
            height_reference: crate::meteorology::VerticalReference::AboveGroundLevel,
            storage_layout: "[target-level,y,x]; x fastest",
            correction_algorithm: W_CORRECTION_ID,
            source: self.source.identity(),
            prepared_uv: self.source.uv().metadata(),
        }
    }
    /// Bind only as read-only m/s, after checking every dependency and W status.
    #[must_use]
    pub fn w(&self) -> &wgpu::Buffer {
        &self.fields.w
    }
    /// Resident completion status; only 2 means full correction completed successfully.
    #[must_use]
    pub fn status(&self) -> &wgpu::Buffer {
        &self.fields.status
    }
    /// Source retains interface upload, prepared U/V and the exact shared target.
    #[must_use]
    pub fn source(&self) -> &SharedHeightWSource {
        &self.source
    }
    /// Exact adjacent-bracket reuse includes the prepared U/V owner, motion and target.
    /// # Errors
    /// Changed source/time/geometry/normalization/target/context rejects reuse.
    pub fn require_reuse(
        &self,
        ctx: &GpuContext,
        source: &SharedHeightWSource,
    ) -> Result<(), Error> {
        let uv = source.uv();
        uv.require_reuse(ctx, uv.source(), uv.target())?;
        if !Arc::ptr_eq(&self.source.0.uv, &source.0.uv)
            || self.source.identity() != source.identity()
        {
            return Err(invalid("incompatible corrected W reuse"));
        }
        Ok(())
    }
}
impl DiagnosticSharedHeightW {
    /// Read-only validation output, never a production owner.
    #[must_use]
    pub fn w(&self) -> &wgpu::Buffer {
        &self.fields.w
    }
    /// Validation completion status; all dependencies must also be accepted.
    #[must_use]
    pub fn status(&self) -> &wgpu::Buffer {
        &self.fields.status
    }
    /// Retain diagnostic dependencies through submitted readback.
    #[must_use]
    pub fn source(&self) -> &SharedHeightWSource {
        &self.source
    }
}
