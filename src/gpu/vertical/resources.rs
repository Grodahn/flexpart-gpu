//! Source-lifetime vertical buffers and dispatch-lifetime query/output uploads.
use super::error::GpuVerticalError;
use crate::gpu::GpuContext;
use crate::meteorology::vertical_sampling::VerticalSamplingError;
use sha2::{Digest, Sha256};
use std::mem::size_of;
use wgpu::util::DeviceExt;

/// Device-resident vertical grid for ordinary sampling.
///
/// Heights and values are in physical bottom-to-top order, sourced from the
/// canonical #30 runtime geometry without a second coordinate system. Lifetime:
/// column/source lifetime, reusable across many queries for that column.
pub struct VerticalGridBuffers {
    /// Immutable grid heights AGL in metres, physical bottom-to-top.
    /// Recreate this owner when geometry changes so its identity stays valid.
    pub heights: wgpu::Buffer,
    /// Grid field values, aligned with `heights`.
    pub values: wgpu::Buffer,
    grid_count: usize,
    pub(super) heights_sha256: String,
}

impl VerticalGridBuffers {
    /// Number of grid levels.
    #[must_use]
    pub const fn grid_count(&self) -> usize {
        self.grid_count
    }
}

/// Device-resident W/interface source for the #80 remap stage.
///
/// All three lanes are physical bottom-to-top. Lifetime: column/source
/// lifetime, reusable across remap dispatches for that column.
pub struct VerticalWInterfaceInputs {
    /// Interface heights AGL, length `nz+1`, with `[0] == 0.0`.
    pub interface_heights: wgpu::Buffer,
    /// Interface geometric vertical velocity in m/s, length `nz+1`.
    pub interface_values: wgpu::Buffer,
    /// Immutable model-level heights AGL, length `nz`.
    /// Recreate this owner when geometry changes so its identity stays valid.
    pub level_heights: wgpu::Buffer,
    model_level_count: usize,
    pub(super) shared_heights_sha256: String,
}

impl VerticalWInterfaceInputs {
    /// Number of model levels (`nz`).
    #[must_use]
    pub const fn model_level_count(&self) -> usize {
        self.model_level_count
    }
}

/// Device-resident particle-height queries.
pub struct VerticalQueryBuffers {
    /// Query heights AGL in metres.
    pub buffer: wgpu::Buffer,
    query_count: usize,
}

impl VerticalQueryBuffers {
    /// Number of queries.
    #[must_use]
    pub const fn query_count(&self) -> usize {
        self.query_count
    }
}

/// Device-resident sampled outputs.
pub struct VerticalSampleOutput {
    /// Sampled field values, one per query.
    pub buffer: wgpu::Buffer,
    query_count: usize,
}

impl VerticalSampleOutput {
    /// Number of outputs.
    #[must_use]
    pub const fn query_count(&self) -> usize {
        self.query_count
    }
}

fn storage_usage() -> wgpu::BufferUsages {
    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC
}

/// #171 batched layout: heights[lane * levels + level], values[level * lanes + lane].
/// Query heights and outputs each have one f32 per lane. All are device-produced.
pub(crate) struct VerticalBatchBuffers {
    pub(crate) heights: wgpu::Buffer,
    pub(crate) values: wgpu::Buffer,
    pub(crate) queries: wgpu::Buffer,
    pub(crate) output: wgpu::Buffer,
    pub(crate) levels: u32,
    pub(crate) lanes: u32,
}

impl VerticalBatchBuffers {
    pub(crate) fn new(ctx: &GpuContext, levels: u32, lanes: u32) -> Result<Self, GpuVerticalError> {
        if levels < 2 || lanes == 0 {
            return Err(GpuVerticalError::EmptyQueries);
        }
        let column_bytes = u64::from(levels) * u64::from(lanes) * 4;
        let limits = ctx.device.limits();
        if column_bytes
            > limits
                .max_buffer_size
                .min(u64::from(limits.max_storage_buffer_binding_size))
            || u64::from(levels) * u64::from(lanes) > u64::from(u32::MAX)
        {
            return Err(GpuVerticalError::SizeOverflow {
                field: "batched columns",
            });
        }
        let buffer = |label, size| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: storage_usage(),
                mapped_at_creation: false,
            })
        };
        Ok(Self {
            heights: buffer("batched AGL heights", column_bytes),
            values: buffer("batched horizontal values", column_bytes),
            queries: buffer("batched resident heights", u64::from(lanes) * 4),
            output: buffer("batched vertical samples", u64::from(lanes) * 4),
            levels,
            lanes,
        })
    }
}

fn height_geometry_sha256(heights: impl IntoIterator<Item = f32>) -> String {
    let mut hash = Sha256::new();
    for height in heights {
        // Signed zero denotes the same physical ground boundary.
        let normalized = if height == 0.0 { 0.0 } else { height };
        hash.update(normalized.to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn checked_byte_len(len: usize, field: &'static str) -> Result<u64, GpuVerticalError> {
    let bytes = len
        .checked_mul(size_of::<f32>())
        .ok_or(GpuVerticalError::SizeOverflow { field })?;
    u64::try_from(bytes).map_err(|_| GpuVerticalError::SizeOverflow { field })
}

pub(super) fn usize_to_u32(value: usize, field: &'static str) -> Result<u32, GpuVerticalError> {
    u32::try_from(value).map_err(|_| GpuVerticalError::ValueTooLarge { field, value })
}

pub(super) fn validate_finite_lane(
    values: &[f32],
    field: &'static str,
) -> Result<(), GpuVerticalError> {
    for value in values {
        if !value.is_finite() {
            return Err(GpuVerticalError::NonFiniteDeviceValue { field });
        }
    }
    Ok(())
}

/// Validate that `heights` is strictly increasing bottom-to-top.
///
/// The reported `index` is the lane-relative position of the lower element of
/// the first non-increasing pair. Column-aware callers
/// ([`super::preparation::physical_model_column_from_runtime`],
/// [`super::preparation::physical_w_columns_from_runtime`]) report true column coordinates
/// instead; this helper only serves column-free buffer constructors, which
/// document the lane-relative convention in their errors.
pub(super) fn validate_strictly_increasing(heights: &[f32]) -> Result<(), GpuVerticalError> {
    for (index, pair) in heights.windows(2).enumerate() {
        if !(pair[1] > pair[0]) {
            return Err(VerticalSamplingError::MalformedGeometry {
                x: 0,
                y: 0,
                index,
                lower_m: pair[0],
                upper_m: pair[1],
            }
            .into());
        }
    }
    Ok(())
}

pub(super) fn validate_grid_columns(
    heights_agl_m: &[f32],
    values: &[f32],
) -> Result<(), GpuVerticalError> {
    if heights_agl_m.len() < 2 || values.len() < 2 {
        return Err(VerticalSamplingError::InsufficientLevels {
            nz: heights_agl_m.len().min(values.len()),
        }
        .into());
    }
    if heights_agl_m.len() != values.len() {
        return Err(GpuVerticalError::CountMismatch {
            field: "vertical_grid",
            grids: heights_agl_m.len(),
            queries: values.len(),
            outputs: heights_agl_m.len(),
        });
    }
    validate_finite_lane(heights_agl_m, "grid_heights_agl_m")?;
    validate_finite_lane(values, "grid_values")?;
    validate_strictly_increasing(heights_agl_m)?;
    Ok(())
}

pub(super) fn validate_interface_columns(
    interface_heights_agl_m: &[f32],
    interface_values_ms: &[f32],
    level_heights_agl_m: &[f32],
) -> Result<usize, GpuVerticalError> {
    let nz = level_heights_agl_m.len();
    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientInterfaceLevels { nz }.into());
    }
    if interface_heights_agl_m.len() != nz + 1 || interface_values_ms.len() != nz + 1 {
        return Err(GpuVerticalError::LevelCountMismatch {
            field: "vertical_w_interfaces",
            interfaces: interface_heights_agl_m.len(),
            levels: nz,
            shared: nz + 1,
        });
    }
    validate_finite_lane(interface_heights_agl_m, "interface_heights_agl_m")?;
    validate_finite_lane(interface_values_ms, "interface_values_ms")?;
    validate_finite_lane(level_heights_agl_m, "level_heights_agl_m")?;
    validate_strictly_increasing(interface_heights_agl_m)?;
    validate_strictly_increasing(level_heights_agl_m)?;
    if interface_heights_agl_m[0] != 0.0 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the #80 shared height grid requires a zero-metre AGL ground boundary",
        }
        .into());
    }
    let interface_top = interface_heights_agl_m[nz];
    for (index, height) in level_heights_agl_m.iter().enumerate().take(nz - 1) {
        if *height <= interface_heights_agl_m[0] || *height >= interface_top {
            return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
                reason: "an interior shared model height is outside the W/interface domain",
            }
            .into());
        }
        let _ = index;
    }
    let top_level = level_heights_agl_m[nz - 1];
    let previous_shared = if nz >= 2 {
        level_heights_agl_m[nz - 2]
    } else {
        0.0
    };
    if top_level <= previous_shared || top_level > interface_top {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the top shared model height is incompatible with the W/interface domain",
        }
        .into());
    }

    Ok(nz)
}

/// Create device-resident grid buffers from validated physical columns.
///
/// Heights must be finite and strictly increasing bottom-to-top; values must
/// be finite; at least two levels are required. This is the low-level
/// primitive used by both #71 oracle validation and #30 runtime paths.
///
/// # Errors
/// Returns [`GpuVerticalError`] for empty, mismatched, non-finite, or
/// non-monotonic inputs.
pub fn create_vertical_grid_buffers(
    ctx: &GpuContext,
    heights_agl_m: &[f32],
    values: &[f32],
) -> Result<VerticalGridBuffers, GpuVerticalError> {
    validate_grid_columns(heights_agl_m, values)?;
    let _ = checked_byte_len(heights_agl_m.len(), "vertical_grid")?;

    let heights = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_grid_heights"),
            contents: bytemuck::cast_slice(heights_agl_m),
            usage: storage_usage(),
        });
    let value_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_grid_values"),
            contents: bytemuck::cast_slice(values),
            usage: storage_usage(),
        });
    Ok(VerticalGridBuffers {
        heights,
        values: value_buffer,
        grid_count: heights_agl_m.len(),
        heights_sha256: height_geometry_sha256(heights_agl_m.iter().copied()),
    })
}

/// Create device-resident W/interface inputs from validated physical columns.
///
/// Interface heights must be strictly increasing with `[0] == 0.0`;
/// interface values and level heights must be finite; level heights must be
/// strictly increasing; `interface.len() == levels.len() + 1` with at least
/// two model levels. Interior shared heights must lie strictly inside the
/// interface domain, matching the CPU `remap_interface_motion_to_model_grid`
/// preconditions.
///
/// # Errors
/// Returns [`GpuVerticalError`] for any malformed geometry or shape mismatch.
pub fn create_vertical_w_interface_inputs(
    ctx: &GpuContext,
    interface_heights_agl_m: &[f32],
    interface_values_ms: &[f32],
    level_heights_agl_m: &[f32],
) -> Result<VerticalWInterfaceInputs, GpuVerticalError> {
    let nz = validate_interface_columns(
        interface_heights_agl_m,
        interface_values_ms,
        level_heights_agl_m,
    )?;
    let _ = checked_byte_len(nz + 1, "vertical_w_interfaces")?;
    let interface_heights = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_w_interface_heights"),
            contents: bytemuck::cast_slice(interface_heights_agl_m),
            usage: storage_usage(),
        });
    let interface_values = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_w_interface_values"),
            contents: bytemuck::cast_slice(interface_values_ms),
            usage: storage_usage(),
        });
    let level_heights = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_w_level_heights"),
            contents: bytemuck::cast_slice(level_heights_agl_m),
            usage: storage_usage(),
        });
    Ok(VerticalWInterfaceInputs {
        interface_heights,
        interface_values,
        level_heights,
        model_level_count: nz,
        shared_heights_sha256: height_geometry_sha256(
            std::iter::once(0.0).chain(level_heights_agl_m.iter().copied()),
        ),
    })
}

/// Create the device-resident shared `[ground, model levels]` grid.
///
/// `shared_heights_agl_m` must be `[0.0, level_heights...]` in physical
/// bottom-to-top order with length `nz+1`. Heights are uploaded; values are
/// allocated uninitialized and must be written by
/// [`super::encode::encode_vertical_remap_w_with_kernel`] before sampling.
///
/// # Errors
/// Returns [`GpuVerticalError`] for malformed shared heights.
pub fn create_vertical_shared_grid(
    ctx: &GpuContext,
    shared_heights_agl_m: &[f32],
) -> Result<VerticalGridBuffers, GpuVerticalError> {
    if shared_heights_agl_m.len() < 3 {
        return Err(VerticalSamplingError::InsufficientInterfaceLevels {
            nz: shared_heights_agl_m.len().saturating_sub(1),
        }
        .into());
    }
    validate_finite_lane(shared_heights_agl_m, "shared_heights_agl_m")?;
    validate_strictly_increasing(shared_heights_agl_m)?;
    if shared_heights_agl_m[0] != 0.0 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the #80 shared height grid requires a zero-metre AGL ground boundary",
        }
        .into());
    }
    let size = checked_byte_len(shared_heights_agl_m.len(), "vertical_shared_values")?;
    let heights = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_shared_heights"),
            contents: bytemuck::cast_slice(shared_heights_agl_m),
            usage: storage_usage(),
        });
    let values = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vertical_shared_values"),
        size,
        usage: storage_usage(),
        mapped_at_creation: false,
    });
    Ok(VerticalGridBuffers {
        heights,
        values,
        grid_count: shared_heights_agl_m.len(),
        heights_sha256: height_geometry_sha256(shared_heights_agl_m.iter().copied()),
    })
}

/// Create device-resident query buffers from validated AGL heights.
///
/// # Errors
/// Returns [`GpuVerticalError`] for empty or non-finite queries.
pub fn create_vertical_query_buffers(
    ctx: &GpuContext,
    query_heights_agl_m: &[f32],
) -> Result<VerticalQueryBuffers, GpuVerticalError> {
    if query_heights_agl_m.is_empty() {
        return Err(GpuVerticalError::EmptyQueries);
    }
    validate_finite_lane(query_heights_agl_m, "query_heights_agl_m")?;
    let _ = checked_byte_len(query_heights_agl_m.len(), "vertical_queries")?;
    let buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_query_heights"),
            contents: bytemuck::cast_slice(query_heights_agl_m),
            usage: storage_usage(),
        });
    Ok(VerticalQueryBuffers {
        buffer,
        query_count: query_heights_agl_m.len(),
    })
}

/// Allocate device-resident sampled outputs for `query_count` queries.
///
/// # Errors
/// Returns [`GpuVerticalError`] for zero or overflowing counts.
pub fn create_vertical_output_buffer(
    ctx: &GpuContext,
    query_count: usize,
) -> Result<VerticalSampleOutput, GpuVerticalError> {
    if query_count == 0 {
        return Err(GpuVerticalError::EmptyQueries);
    }
    let size = checked_byte_len(query_count, "vertical_outputs")?;
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vertical_sample_outputs"),
        size,
        usage: storage_usage(),
        mapped_at_creation: false,
    });
    Ok(VerticalSampleOutput {
        buffer,
        query_count,
    })
}
