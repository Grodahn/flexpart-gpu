//! Vertical sample/remap passes encoded into caller-owned command encoders.
use super::error::GpuVerticalError;
use super::pipeline::{VerticalSampleKernel, VerticalWRemapKernel};
use super::resources::{
    usize_to_u32, VerticalGridBuffers, VerticalQueryBuffers, VerticalSampleOutput,
    VerticalWInterfaceInputs,
};
use crate::gpu::GpuContext;
use crate::meteorology::vertical_sampling::VerticalSamplingError;
use bytemuck::{Pod, Zeroable};
use std::mem::size_of;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct VerticalSampleParamsRaw {
    grid_count: u32,
    query_count: u32,
    height_lane_stride: u32,
    value_level_stride: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct VerticalRemapParamsRaw {
    model_level_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

/// Encode the existing #88 equations with explicit batched addressing only.
pub(crate) fn encode_vertical_batch(
    ctx: &GpuContext,
    batch: &super::resources::VerticalBatchBuffers,
    kernel: &VerticalSampleKernel,
    encoder: &mut wgpu::CommandEncoder,
) {
    let params = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("batched vertical strides"),
            contents: bytemuck::bytes_of(&VerticalSampleParamsRaw {
                grid_count: batch.levels,
                query_count: batch.lanes,
                height_lane_stride: batch.levels,
                value_level_stride: batch.lanes,
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let buffers = [
        &batch.heights,
        &batch.values,
        &batch.queries,
        &batch.output,
        &params,
    ];
    let entries: Vec<_> = buffers
        .iter()
        .enumerate()
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding: u32::try_from(binding).expect("five bindings fit u32"),
            resource: buffer.as_entire_binding(),
        })
        .collect();
    let bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("batched vertical sample"),
        layout: &kernel.bind_group_layout,
        entries: &entries,
    });
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("batched vertical sample"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&kernel.pipeline);
    pass.set_bind_group(0, &bg, &[]);
    crate::gpu::dispatch_1d(&mut pass, batch.lanes, kernel.workgroup_size_x);
}

/// Encode ordinary METRE-mode sampling into a caller-owned encoder.
///
/// This is the production composition path for later #76 use. It submits
/// nothing, waits for nothing, allocates no field buffers, and reads nothing
/// back. The caller owns submission and lifetime: `grid`, `queries`,
/// `outputs`, and `kernel` must live until the encoded work completes.
///
/// # Errors
/// Returns [`GpuVerticalError`] for mismatched counts or oversized values.
pub fn encode_vertical_sample_with_kernel(
    ctx: &GpuContext,
    grid: &VerticalGridBuffers,
    queries: &VerticalQueryBuffers,
    outputs: &VerticalSampleOutput,
    kernel: &VerticalSampleKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuVerticalError> {
    if queries.query_count() != outputs.query_count() {
        return Err(GpuVerticalError::CountMismatch {
            field: "vertical_sample",
            grids: grid.grid_count(),
            queries: queries.query_count(),
            outputs: outputs.query_count(),
        });
    }
    if grid.grid_count() < 2 {
        return Err(VerticalSamplingError::InsufficientLevels {
            nz: grid.grid_count(),
        }
        .into());
    }
    if queries.query_count() == 0 {
        return Err(GpuVerticalError::EmptyQueries);
    }
    let grid_count_u32 = usize_to_u32(grid.grid_count(), "grid_count")?;
    let query_count_u32 = usize_to_u32(queries.query_count(), "query_count")?;
    debug_assert_eq!(
        size_of::<VerticalSampleParamsRaw>() % 16,
        0,
        "vertical sample uniform params must stay 16-byte aligned"
    );
    let raw = VerticalSampleParamsRaw {
        grid_count: grid_count_u32,
        query_count: query_count_u32,
        height_lane_stride: 0,
        value_level_stride: 0,
    };
    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_sample_params"),
            contents: bytemuck::bytes_of(&raw),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vertical_sample_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: grid.heights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: grid.values.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: queries.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: outputs.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("vertical_sample_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&kernel.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        crate::gpu::dispatch_1d(&mut pass, query_count_u32, kernel.workgroup_size_x);
    }
    Ok(())
}

/// Encode the #80 W/interface-to-shared-grid remap into a caller-owned encoder.
///
/// This is the first stage of the two-stage W production path. It submits
/// nothing, waits for nothing, and reads nothing back. `inputs`,
/// `shared_grid`, and `kernel` must live until the encoded work completes.
/// The caller must encode [`encode_vertical_sample_with_kernel`] afterwards
/// on `shared_grid` to complete the production sample; the intermediate
/// `shared_grid.values` stays device-resident between the two stages.
///
/// # Errors
/// Returns [`GpuVerticalError`] for inconsistent counts or a shared height
/// grid that does not match the source's `[ground, model levels]` geometry.
pub fn encode_vertical_remap_w_with_kernel(
    ctx: &GpuContext,
    inputs: &VerticalWInterfaceInputs,
    shared_grid: &VerticalGridBuffers,
    kernel: &VerticalWRemapKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuVerticalError> {
    let nz = inputs.model_level_count();
    if shared_grid.grid_count() != nz + 1 {
        return Err(GpuVerticalError::LevelCountMismatch {
            field: "vertical_w_shared",
            interfaces: nz + 1,
            levels: nz,
            shared: shared_grid.grid_count(),
        });
    }
    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientInterfaceLevels { nz }.into());
    }
    if shared_grid.heights_sha256 != inputs.shared_heights_sha256 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "shared grid heights differ from the W source model heights",
        }
        .into());
    }
    let nz_u32 = usize_to_u32(nz, "model_level_count")?;
    debug_assert_eq!(
        size_of::<VerticalRemapParamsRaw>() % 16,
        0,
        "vertical remap uniform params must stay 16-byte aligned"
    );
    let raw = VerticalRemapParamsRaw {
        model_level_count: nz_u32,
        _pad0: 0,
        _pad1: 0,
        _pad2: 0,
    };
    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_remap_w_params"),
            contents: bytemuck::bytes_of(&raw),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vertical_remap_w_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: inputs.interface_heights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: inputs.interface_values.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: inputs.level_heights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: shared_grid.values.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("vertical_remap_w_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&kernel.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        let shared_count_u32 = nz_u32 + 1;
        crate::gpu::dispatch_1d(&mut pass, shared_count_u32, kernel.workgroup_size_x);
    }
    Ok(())
}

/// Encode the full two-stage W production path into one caller-owned encoder.
///
/// Production composition entrypoint for later #76 use: remap is encoded
/// first, then the dependent sample on the shared grid, with no submit, wait,
/// or readback between them. The intermediate stays device-resident.
///
/// # Errors
/// Returns [`GpuVerticalError`] for inconsistent counts.
pub fn encode_vertical_w_two_stage_with_kernels(
    ctx: &GpuContext,
    inputs: &VerticalWInterfaceInputs,
    shared_grid: &VerticalGridBuffers,
    queries: &VerticalQueryBuffers,
    outputs: &VerticalSampleOutput,
    sample_kernel: &VerticalSampleKernel,
    remap_kernel: &VerticalWRemapKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuVerticalError> {
    encode_vertical_remap_w_with_kernel(ctx, inputs, shared_grid, remap_kernel, encoder)?;
    encode_vertical_sample_with_kernel(ctx, shared_grid, queries, outputs, sample_kernel, encoder)
}
