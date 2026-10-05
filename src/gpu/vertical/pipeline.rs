//! Reusable vertical pipelines with the #88 shader layouts and creation-time wait.
use super::error::{pop_device_error_scopes, push_device_error_scopes, GpuVerticalError};
use crate::gpu::GpuContext;

/// WGSL source for the ordinary METRE-mode particle-height sample.
pub(super) const SAMPLE_SHADER_SOURCE: &str = include_str!("../../shaders/vertical_sample.wgsl");

/// WGSL source for the #80 W/interface-to-shared-grid remap.
pub(super) const REMAP_SHADER_SOURCE: &str = include_str!("../../shaders/vertical_remap_w.wgsl");

/// Workgroup width for both vertical kernels.
///
/// A fixed width mirrors `gpu::horizontal`/`gpu::temporal` and avoids a
/// shared-infrastructure change while #87 proceeds concurrently.
const WORKGROUP_SIZE_X: u32 = 64;

/// Reusable WGSL kernel for ordinary METRE-mode particle-height sampling.
pub struct VerticalSampleKernel {
    pub(super) bind_group_layout: wgpu::BindGroupLayout,
    pub(super) pipeline: wgpu::ComputePipeline,
    pub(super) workgroup_size_x: u32,
}

impl VerticalSampleKernel {
    /// Compile the WGSL sampling kernel once per context.
    ///
    /// # Errors
    /// Returns [`GpuVerticalError::Device`] when shader or pipeline creation
    /// raises a scoped `wgpu` error.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuVerticalError> {
        push_device_error_scopes(&ctx.device);
        let shader = ctx.load_shader("vertical_sample_shader", SAMPLE_SHADER_SOURCE);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("vertical_sample_bgl"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                    ],
                });
        let pipeline = ctx.create_compute_pipeline(
            "vertical_sample_pipeline",
            &shader,
            "main",
            &[&bind_group_layout],
        );
        let kernel = Self {
            bind_group_layout,
            pipeline,
            workgroup_size_x: WORKGROUP_SIZE_X,
        };
        let _ = ctx.device.poll(wgpu::Maintain::Wait);
        pop_device_error_scopes(&ctx.device, "pipeline creation")?;
        Ok(kernel)
    }

    /// Workgroup width used for 1-D dispatch.
    #[must_use]
    pub const fn workgroup_size_x(&self) -> u32 {
        self.workgroup_size_x
    }
}

/// Reusable WGSL kernel for the #80 W/interface-to-shared-grid remap.
pub struct VerticalWRemapKernel {
    pub(super) bind_group_layout: wgpu::BindGroupLayout,
    pub(super) pipeline: wgpu::ComputePipeline,
    pub(super) workgroup_size_x: u32,
}

impl VerticalWRemapKernel {
    /// Compile the WGSL remap kernel once per context.
    ///
    /// # Errors
    /// Returns [`GpuVerticalError::Device`] when shader or pipeline creation
    /// raises a scoped `wgpu` error.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuVerticalError> {
        push_device_error_scopes(&ctx.device);
        let shader = ctx.load_shader("vertical_remap_w_shader", REMAP_SHADER_SOURCE);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("vertical_remap_w_bgl"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                    ],
                });
        let pipeline = ctx.create_compute_pipeline(
            "vertical_remap_w_pipeline",
            &shader,
            "main",
            &[&bind_group_layout],
        );
        let kernel = Self {
            bind_group_layout,
            pipeline,
            workgroup_size_x: WORKGROUP_SIZE_X,
        };
        let _ = ctx.device.poll(wgpu::Maintain::Wait);
        pop_device_error_scopes(&ctx.device, "pipeline creation")?;
        Ok(kernel)
    }

    /// Workgroup width used for 1-D dispatch.
    #[must_use]
    pub const fn workgroup_size_x(&self) -> u32 {
        self.workgroup_size_x
    }
}
