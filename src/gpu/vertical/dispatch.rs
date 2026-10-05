//! Standalone vertical completion and explicit validation/output readback boundaries.
use super::encode::{encode_vertical_remap_w_with_kernel, encode_vertical_sample_with_kernel};
use super::error::{pop_device_error_scopes, push_device_error_scopes, GpuVerticalError};
use super::pipeline::{VerticalSampleKernel, VerticalWRemapKernel};
use super::resources::{
    create_vertical_output_buffer, VerticalGridBuffers, VerticalQueryBuffers, VerticalSampleOutput,
    VerticalWInterfaceInputs,
};
use crate::gpu::{download_buffer_typed, GpuContext};

/// Dispatch ordinary sampling with an isolated encoder.
///
/// Convenience path for isolated oracle validation only. Production
/// composition must use [`encode_vertical_sample_with_kernel`] instead.
///
/// # Errors
/// Forwards [`GpuVerticalError`] from encoding.
pub fn dispatch_vertical_sample_with_kernel(
    ctx: &GpuContext,
    grid: &VerticalGridBuffers,
    queries: &VerticalQueryBuffers,
    outputs: &VerticalSampleOutput,
    kernel: &VerticalSampleKernel,
) -> Result<(), GpuVerticalError> {
    push_device_error_scopes(&ctx.device);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vertical_sample_encoder"),
        });
    if let Err(error) =
        encode_vertical_sample_with_kernel(ctx, grid, queries, outputs, kernel, &mut encoder)
    {
        let _ = pop_device_error_scopes(&ctx.device, "dispatch preparation");
        return Err(error);
    }
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    pop_device_error_scopes(&ctx.device, "dispatch")
}

/// Dispatch the W remap stage with an isolated encoder.
///
/// Convenience path for isolated oracle validation only.
///
/// # Errors
/// Forwards [`GpuVerticalError`] from encoding.
pub fn dispatch_vertical_remap_w_with_kernel(
    ctx: &GpuContext,
    inputs: &VerticalWInterfaceInputs,
    shared_grid: &VerticalGridBuffers,
    kernel: &VerticalWRemapKernel,
) -> Result<(), GpuVerticalError> {
    push_device_error_scopes(&ctx.device);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vertical_remap_w_encoder"),
        });
    if let Err(error) =
        encode_vertical_remap_w_with_kernel(ctx, inputs, shared_grid, kernel, &mut encoder)
    {
        let _ = pop_device_error_scopes(&ctx.device, "dispatch preparation");
        return Err(error);
    }
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    pop_device_error_scopes(&ctx.device, "dispatch")
}

/// Explicit D2H readback of sampled outputs (validation/output boundary only).
///
/// # Errors
/// Forwards [`GpuVerticalError::Buffer`] from staging and mapping.
pub async fn download_vertical_samples(
    ctx: &GpuContext,
    outputs: &VerticalSampleOutput,
) -> Result<Vec<f32>, GpuVerticalError> {
    Ok(download_buffer_typed::<f32>(
        ctx,
        &outputs.buffer,
        outputs.query_count(),
        "vertical_sample_outputs",
    )
    .await?)
}

/// Sample one physical grid on the GPU and read back (isolated validation).
///
/// Convenience helper for oracle tests only. Production code must keep grid
/// and shared resources device-resident through `encode_*` composition.
///
/// # Errors
/// Returns [`GpuVerticalError`] for encoding or readback failure. No CPU
/// fallback is performed for supported inputs.
pub async fn sample_vertical_grid_gpu(
    ctx: &GpuContext,
    grid: &VerticalGridBuffers,
    queries: &VerticalQueryBuffers,
    kernel: &VerticalSampleKernel,
) -> Result<Vec<f32>, GpuVerticalError> {
    let outputs = create_vertical_output_buffer(ctx, queries.query_count())?;
    dispatch_vertical_sample_with_kernel(ctx, grid, queries, &outputs, kernel)?;
    let values = download_vertical_samples(ctx, &outputs).await?;
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(GpuVerticalError::NonFiniteOutputValue { query_index: index });
        }
    }
    Ok(values)
}

/// Execute the full two-stage W production path on the GPU and read back.
///
/// Remap and sample execute as two separate WGSL dispatches sharing one
/// caller-visible sequence but no host materialization of the intermediate:
/// this helper encodes both into isolated encoders in order (remap before
/// sample) for test clarity. Production composition should encode both into
/// a single caller-owned encoder without intermediate submit/wait.
///
/// # Errors
/// Returns [`GpuVerticalError`] for encoding, dispatch, or readback failure.
pub async fn sample_vertical_w_gpu_two_stage(
    ctx: &GpuContext,
    inputs: &VerticalWInterfaceInputs,
    shared_grid: &VerticalGridBuffers,
    queries: &VerticalQueryBuffers,
    sample_kernel: &VerticalSampleKernel,
    remap_kernel: &VerticalWRemapKernel,
) -> Result<Vec<f32>, GpuVerticalError> {
    dispatch_vertical_remap_w_with_kernel(ctx, inputs, shared_grid, remap_kernel)?;
    let outputs = create_vertical_output_buffer(ctx, queries.query_count())?;
    dispatch_vertical_sample_with_kernel(ctx, shared_grid, queries, &outputs, sample_kernel)?;
    let values = download_vertical_samples(ctx, &outputs).await?;
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(GpuVerticalError::NonFiniteOutputValue { query_index: index });
        }
    }
    Ok(values)
}
