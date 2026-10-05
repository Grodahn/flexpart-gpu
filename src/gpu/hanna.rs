//! GPU Hanna turbulence parameter dispatch (H-02).
//!
//! Dispatches a WGSL kernel that computes per-particle Hanna (1982) turbulence
//! parameters using local PBL fields:
//! - `sigma_u`, `sigma_v`, `sigma_w`
//! - `d(sigma_w)/dz`, `d(sigma_w^2)/dz`
//! - `TL_u`, `TL_v`, `TL_w`
//!
//! The branch logic and formulas mirror `src/physics/hanna.rs` (H-01), itself
//! ported from FLEXPART `hanna.f90`.

use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::pbl::HannaParams;

use super::{
    download_buffer_typed, render_shader_with_workgroup_size, runtime_workgroup_size,
    GpuBufferError, GpuContext, ParticleBuffers, PblBuffers, WorkgroupKernel,
};

const SHADER_TEMPLATE: &str = include_str!("../shaders/hanna_params.wgsl");

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct HannaDispatchParams {
    pbl_nx: u32,
    pbl_ny: u32,
    particle_count: u32,
    _pad0: u32,
}

/// Errors returned while running the GPU Hanna kernel.
#[derive(Debug, Error)]
pub enum GpuHannaError {
    #[error("PBL grid dimensions must be non-zero, got {shape:?}")]
    ZeroPblShape { shape: (usize, usize) },
    #[error("value for {field} does not fit in u32: {value}")]
    ValueTooLarge { field: &'static str, value: usize },
    #[error("byte-size overflow while preparing {field}")]
    SizeOverflow { field: &'static str },
    #[error("length mismatch for {field}: expected {expected}, got {actual}")]
    LengthMismatch {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("buffer operation failed: {0}")]
    Buffer(#[from] GpuBufferError),
}

fn usize_to_u32(value: usize, field: &'static str) -> Result<u32, GpuHannaError> {
    u32::try_from(value).map_err(|_| GpuHannaError::ValueTooLarge { field, value })
}

fn checked_byte_len<T>(len: usize, field: &'static str) -> Result<usize, GpuHannaError> {
    len.checked_mul(size_of::<T>())
        .ok_or(GpuHannaError::SizeOverflow { field })
}

/// Reusable GPU output buffer holding one [`HannaParams`] entry per particle slot.
/// Capacity may exceed the accessed dispatch prefix; unaccessed slots are retained.
pub struct HannaParamsOutputBuffer {
    pub buffer: wgpu::Buffer,
    particle_count: usize,
}

impl HannaParamsOutputBuffer {
    /// Allocate resident turbulence parameter storage for the simulation capacity.
    pub fn new(ctx: &GpuContext, particle_count: usize) -> Result<Self, GpuHannaError> {
        let output_bytes = checked_byte_len::<HannaParams>(particle_count, "hanna_params_outputs")?;
        let output_size = if output_bytes == 0 {
            4
        } else {
            u64::try_from(output_bytes).map_err(|_| GpuHannaError::SizeOverflow {
                field: "hanna_params_outputs",
            })?
        };
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hanna_params_outputs"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            buffer,
            particle_count,
        })
    }

    #[must_use]
    /// Return the logical number of output slots available to GPU consumers.
    pub fn particle_count(&self) -> usize {
        self.particle_count
    }
}

/// Reusable Hanna dispatch kernel objects (bind group layout + compute pipeline).
pub struct HannaDispatchKernel {
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::ComputePipeline,
    workgroup_size_x: u32,
}

impl HannaDispatchKernel {
    #[must_use]
    pub fn new(ctx: &GpuContext) -> Self {
        let workgroup_size_x = runtime_workgroup_size(ctx, WorkgroupKernel::HannaParams);
        let shader_source = render_shader_with_workgroup_size(SHADER_TEMPLATE, workgroup_size_x);
        let shader = ctx.load_shader("hanna_params_shader", &shader_source);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("hanna_params_bgl"),
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
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 5,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 6,
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
            "hanna_params_pipeline",
            &shader,
            "main",
            &[&bind_group_layout],
        );
        Self {
            bind_group_layout,
            pipeline,
            workgroup_size_x,
        }
    }
}

/// Compute per-particle Hanna turbulence parameters on the GPU.
///
/// Input contract:
/// - particle horizontal position comes from `Particle` (`cell + frac`)
/// - particle vertical coordinate uses `Particle::pos_z`
/// - PBL fields come from [`PblBuffers`] flattened as row-major `(nx, ny)`
///
/// Inactive particles (`flags & 1 == 0`) return zeroed [`HannaParams`].
pub async fn compute_hanna_params_gpu(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    pbl: &PblBuffers,
) -> Result<Vec<HannaParams>, GpuHannaError> {
    let particle_count = particles.particle_count();
    if particle_count == 0 {
        return Ok(Vec::new());
    }

    let output = HannaParamsOutputBuffer::new(ctx, particle_count)?;
    dispatch_hanna_params_gpu(ctx, particles, pbl, &output)?;
    download_buffer_typed::<HannaParams>(
        ctx,
        &output.buffer,
        particle_count,
        "hanna_params_outputs",
    )
    .await
    .map_err(Into::into)
}

/// Dispatch Hanna parameter computation into a caller-provided GPU output buffer.
///
/// This API is intended for fully GPU-resident workflows where downstream
/// kernels consume Hanna outputs directly without host readback.
pub fn dispatch_hanna_params_gpu(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    pbl: &PblBuffers,
    output: &HannaParamsOutputBuffer,
) -> Result<(), GpuHannaError> {
    let kernel = HannaDispatchKernel::new(ctx);
    dispatch_hanna_params_gpu_with_kernel(ctx, particles, pbl, output, &kernel)
}

/// Dispatch Hanna parameters using a reusable prepared kernel.
pub fn dispatch_hanna_params_gpu_with_kernel(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    pbl: &PblBuffers,
    output: &HannaParamsOutputBuffer,
    kernel: &HannaDispatchKernel,
) -> Result<(), GpuHannaError> {
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("hanna_params_encoder"),
        });
    encode_hanna_params_gpu_with_kernel(ctx, particles, pbl, output, kernel, &mut encoder)?;
    ctx.queue.submit(Some(encoder.finish()));
    Ok(())
}

/// Encode Hanna parameter dispatch into a caller-provided command encoder.
/// The logical output range and actual storage must cover the particle dispatch
/// prefix. Only that prefix is written; capacity and submission ownership stay unchanged.
pub fn encode_hanna_params_gpu_with_kernel(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    pbl: &PblBuffers,
    output: &HannaParamsOutputBuffer,
    kernel: &HannaDispatchKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuHannaError> {
    let particle_count = particles.particle_count();
    if particle_count == 0 {
        return Ok(());
    }
    if output.particle_count < particle_count {
        return Err(GpuHannaError::LengthMismatch {
            field: "hanna_params_outputs",
            expected: particle_count,
            actual: output.particle_count,
        });
    }
    let required_bytes = checked_byte_len::<HannaParams>(particle_count, "hanna_params_outputs")?;
    let required_bytes =
        u64::try_from(required_bytes).map_err(|_| GpuHannaError::SizeOverflow {
            field: "hanna_params_outputs",
        })?;
    if output.buffer.size() < required_bytes {
        return Err(GpuHannaError::LengthMismatch {
            field: "hanna_params_outputs_storage",
            expected: particle_count,
            actual: usize::try_from(output.buffer.size() / size_of::<HannaParams>() as u64)
                .unwrap_or(usize::MAX),
        });
    }

    let (pbl_nx, pbl_ny) = pbl.shape;
    if pbl_nx == 0 || pbl_ny == 0 {
        return Err(GpuHannaError::ZeroPblShape { shape: pbl.shape });
    }

    let params = HannaDispatchParams {
        pbl_nx: usize_to_u32(pbl_nx, "pbl_nx")?,
        pbl_ny: usize_to_u32(pbl_ny, "pbl_ny")?,
        particle_count: usize_to_u32(particle_count, "particle_count")?,
        _pad0: 0,
    };

    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hanna_params_dispatch_params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hanna_params_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: particles.particle_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: pbl.ustar.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: pbl.wstar.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pbl.hmix.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: pbl.oli.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: output.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });
    {
        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hanna_params_pass"),
            timestamp_writes: None,
        });
        cpass.set_pipeline(&kernel.pipeline);
        cpass.set_bind_group(0, &bind_group, &[]);
        super::dispatch_1d(&mut cpass, params.particle_count, kernel.workgroup_size_x);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;
    use bytemuck::Zeroable;

    use super::*;
    use crate::gpu::GpuError;
    use crate::particles::{Particle, ParticleInit, ParticleStore, MAX_SPECIES};
    use crate::pbl::PblState;
    use crate::physics::{compute_hanna_params, obukhov_length_from_inverse, HannaInputs};

    fn make_particle(x: f32, y: f32, z: f32) -> Particle {
        let cell_x = x.floor() as i32;
        let cell_y = y.floor() as i32;
        Particle::new(&ParticleInit {
            cell_x,
            cell_y,
            pos_x: x - cell_x as f32,
            pos_y: y - cell_y as f32,
            pos_z: z,
            mass: [0.0; MAX_SPECIES],
            release_point: 0,
            class: 0,
            time: 0,
        })
    }

    fn expected_from_cpu(pbl: &PblState, particle: &Particle) -> HannaParams {
        if !particle.is_active() {
            return HannaParams::zeroed();
        }

        let (nx, ny) = pbl.shape();
        let x = (particle.cell_x as f32 + particle.pos_x).clamp(0.0, (nx - 1) as f32);
        let y = (particle.cell_y as f32 + particle.pos_y).clamp(0.0, (ny - 1) as f32);
        let i = x.floor() as usize;
        let j = y.floor() as usize;

        compute_hanna_params(HannaInputs {
            ust: pbl.ustar[[i, j]],
            wst: pbl.wstar[[i, j]],
            ol: obukhov_length_from_inverse(pbl.oli[[i, j]]),
            h: pbl.hmix[[i, j]],
            z: particle.pos_z,
        })
    }

    fn assert_scalar_close(actual: f32, expected: f32) {
        if expected.is_infinite() {
            assert!(actual.is_infinite());
            assert_eq!(actual.is_sign_positive(), expected.is_sign_positive());
        } else {
            assert_relative_eq!(actual, expected, epsilon = 5.0e-5, max_relative = 5.0e-5);
        }
    }

    fn assert_hanna_close(actual: &HannaParams, expected: &HannaParams) {
        assert_scalar_close(actual.ust, expected.ust);
        assert_scalar_close(actual.wst, expected.wst);
        assert_scalar_close(actual.ol, expected.ol);
        assert_scalar_close(actual.h, expected.h);
        assert_scalar_close(actual.zeta, expected.zeta);
        assert_scalar_close(actual.sigu, expected.sigu);
        assert_scalar_close(actual.sigv, expected.sigv);
        assert_scalar_close(actual.sigw, expected.sigw);
        assert_scalar_close(actual.dsigwdz, expected.dsigwdz);
        assert_scalar_close(actual.dsigw2dz, expected.dsigw2dz);
        assert_scalar_close(actual.tlu, expected.tlu);
        assert_scalar_close(actual.tlv, expected.tlv);
        assert_scalar_close(actual.tlw, expected.tlw);
    }

    fn assert_langevin_particle_close(actual: &Particle, expected: &Particle) {
        // Retain the existing Langevin position and turbulent-velocity tolerances.
        assert_relative_eq!(actual.pos_z, expected.pos_z, epsilon = 1.0e-7);
        for (actual_velocity, expected_velocity) in [
            (actual.turb_u, expected.turb_u),
            (actual.turb_v, expected.turb_v),
            (actual.turb_w, expected.turb_w),
        ] {
            assert_relative_eq!(
                actual_velocity,
                expected_velocity,
                epsilon = 5.0e-5,
                max_relative = 5.0e-5
            );
        }
        let mut unchanged_fields = *actual;
        unchanged_fields.pos_z = expected.pos_z;
        unchanged_fields.turb_u = expected.turb_u;
        unchanged_fields.turb_v = expected.turb_v;
        unchanged_fields.turb_w = expected.turb_w;
        assert_eq!(
            bytemuck::bytes_of(&unchanged_fields),
            bytemuck::bytes_of(expected)
        );
    }

    #[test]
    fn test_hanna_langevin_capacity_output_serves_active_prefix() {
        use crate::gpu::{
            encode_update_particles_turbulence_langevin_gpu_with_hanna_buffer_and_kernel,
            LangevinDispatchKernel,
        };
        use crate::physics::{philox_counter_add, LangevinStep};
        use sha2::{Digest, Sha256};

        let ctx = pollster::block_on(GpuContext::new()).expect("prefix regression requires WGSL");
        let hanna_kernel = HannaDispatchKernel::new(&ctx);
        let langevin_kernel = LangevinDispatchKernel::new(&ctx);
        let mut pbl = PblState::new(3, 1);
        for slot in 0..3 {
            pbl.ustar[[slot, 0]] = 0.25 + slot as f32 * 0.05;
            pbl.wstar[[slot, 0]] = if slot == 0 { 1.8 } else { 0.0 };
            pbl.hmix[[slot, 0]] = 900.0;
            pbl.oli[[slot, 0]] = [-0.02, 0.0, 0.01][slot];
        }
        let pbl_buffers = PblBuffers::from_state(&ctx, &pbl).expect("PBL upload");
        let key = [0xDECA_FBAD, 0x1234_5678];
        let counter = [7, 9, 11, 13];
        let mut input_identities = std::collections::HashSet::new();
        for active in [1, 3, 8] {
            for substeps in [0, 4] {
                let particles: Vec<_> = (0..8)
                    .map(|slot| {
                        let mut particle =
                            make_particle((slot % 3) as f32, 0.0, 80.0 + slot as f32 * 13.0);
                        particle.release_point = 100 + slot as i32;
                        particle.mass = [1.0 + slot as f32; MAX_SPECIES];
                        particle.turb_u = slot as f32 * 0.1;
                        particle.turb_v = -0.2;
                        particle.turb_w = 0.3;
                        if slot >= active {
                            particle.deactivate();
                        }
                        particle
                    })
                    .collect();
                let mut prefix = ParticleBuffers::from_particles(&ctx, &particles);
                prefix.set_dispatch_count(active);
                let control = ParticleBuffers::from_particles(&ctx, &particles);
                let output = HannaParamsOutputBuffer::new(&ctx, 8).expect("resident output");
                let control_output = HannaParamsOutputBuffer::new(&ctx, 8).expect("control output");
                let sentinel: Vec<HannaParams> = (0..8)
                    .map(|slot| HannaParams {
                        sigu: 123.0 + slot as f32,
                        ..HannaParams::zeroed()
                    })
                    .collect();
                ctx.queue
                    .write_buffer(&output.buffer, 0, bytemuck::cast_slice(&sentinel));
                let step = LangevinStep {
                    n_substeps: substeps,
                    ..LangevinStep::legacy(1.0, 2.5e-4)
                };
                let mut encoder = ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                for (buffers, result) in [(&prefix, &output), (&control, &control_output)] {
                    encode_hanna_params_gpu_with_kernel(
                        &ctx,
                        buffers,
                        &pbl_buffers,
                        result,
                        &hanna_kernel,
                        &mut encoder,
                    )
                    .expect("Hanna prefix encode");
                    let next = encode_update_particles_turbulence_langevin_gpu_with_hanna_buffer_and_kernel(
                        &ctx, buffers, &result.buffer, result.particle_count(), step, key, counter, &langevin_kernel, &mut encoder,
                    ).expect("Langevin prefix encode");
                    assert_eq!(
                        next,
                        philox_counter_add(
                            counter,
                            buffers.particle_count() as u64 * if substeps <= 2 { 1 } else { 2 }
                        )
                    );
                }
                ctx.queue.submit(Some(encoder.finish()));
                let actual =
                    pollster::block_on(prefix.download_particles(&ctx)).expect("prefix readback");
                let expected =
                    pollster::block_on(control.download_particles(&ctx)).expect("control readback");
                let hanna = pollster::block_on(download_buffer_typed::<HannaParams>(
                    &ctx,
                    &output.buffer,
                    8,
                    "prefix",
                ))
                .expect("Hanna readback");
                let control_hanna = pollster::block_on(download_buffer_typed::<HannaParams>(
                    &ctx,
                    &control_output.buffer,
                    8,
                    "control",
                ))
                .expect("control Hanna readback");
                assert_eq!(prefix.capacity(), 8);
                assert_eq!(output.particle_count(), 8);
                for slot in 0..8 {
                    let identity = actual[slot].release_point;
                    let reference = expected
                        .iter()
                        .find(|p| p.release_point == identity)
                        .expect("particle identity retained");
                    assert_langevin_particle_close(&actual[slot], reference);
                    assert_eq!(actual[slot].mass, particles[slot].mass);
                    if slot < active {
                        assert_hanna_close(&hanna[slot], &control_hanna[slot]);
                    } else {
                        assert_eq!(
                            bytemuck::bytes_of(&hanna[slot]),
                            bytemuck::bytes_of(&sentinel[slot])
                        );
                        assert_eq!(
                            bytemuck::bytes_of(&actual[slot]),
                            bytemuck::bytes_of(&particles[slot])
                        );
                    }
                }
                // Hash the finite normalized handoff, including both dispatches,
                // so different PBL/RNG/substep inputs cannot share a particle-only identity.
                let mut input_hash = Sha256::new();
                input_hash.update(b"hanna-langevin-prefix-139-v1");
                input_hash.update(8_u32.to_le_bytes());
                input_hash.update(bytemuck::cast_slice::<Particle, u8>(&particles));
                input_hash.update(bytemuck::bytes_of(&HannaDispatchParams {
                    pbl_nx: 3,
                    pbl_ny: 1,
                    particle_count: active as u32,
                    _pad0: 0,
                }));
                for field in [&pbl.ustar, &pbl.wstar, &pbl.hmix, &pbl.oli] {
                    for value in field {
                        input_hash.update(value.to_le_bytes());
                    }
                }
                input_hash.update(bytemuck::bytes_of(&key));
                input_hash.update(bytemuck::bytes_of(&counter));
                input_hash.update(step.dt_seconds.to_le_bytes());
                input_hash.update(step.rho_grad_over_rho.to_le_bytes());
                input_hash.update(step.n_substeps.to_le_bytes());
                input_hash.update(step.min_height_m.to_le_bytes());
                input_hash.update(bytemuck::cast_slice::<HannaParams, u8>(&sentinel));
                let input_sha256 = format!("{:x}", input_hash.finalize());
                assert!(
                    input_identities.insert(input_sha256.clone()),
                    "dispatch cases need distinct input identities"
                );
                eprintln!("HANNA-PREFIX-139: adapter={:?} capacity=8 active={active} substeps={substeps} key={key:?} counter={counter:?} step={step:?} input_sha256={input_sha256} inputs={particles:?} pbl={pbl:?} sentinel={sentinel:?} hanna={hanna:?} control_hanna={control_hanna:?} outputs={actual:?} control={expected:?}", ctx.adapter_info());
            }
        }
    }

    #[test]
    fn test_hanna_langevin_undersized_prefix_fails_before_submission() {
        use crate::gpu::{
            encode_update_particles_turbulence_langevin_gpu_with_hanna_buffer_and_kernel,
            GpuLangevinError, LangevinDispatchKernel,
        };
        use crate::physics::LangevinStep;

        let ctx = pollster::block_on(GpuContext::new()).expect("bounds regression requires WGSL");
        let kernel = HannaDispatchKernel::new(&ctx);
        let consumer = LangevinDispatchKernel::new(&ctx);
        let mut particles =
            ParticleBuffers::from_particles(&ctx, &[make_particle(0.0, 0.0, 80.0); 8]);
        particles.set_dispatch_count(3);
        let pbl = PblBuffers::from_state(&ctx, &PblState::new(1, 1)).expect("PBL");
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for slots in [0, 1, 2] {
            let output = HannaParamsOutputBuffer::new(&ctx, slots).expect("short output");
            assert!(
                matches!(encode_hanna_params_gpu_with_kernel(&ctx, &particles, &pbl, &output, &kernel, &mut encoder), Err(GpuHannaError::LengthMismatch { field: "hanna_params_outputs", expected: 3, actual }) if actual == slots)
            );
            assert!(
                matches!(encode_update_particles_turbulence_langevin_gpu_with_hanna_buffer_and_kernel(&ctx, &particles, &output.buffer, slots, LangevinStep::legacy(1.0, 0.0), [1, 2], [0; 4], &consumer, &mut encoder), Err(GpuLangevinError::MismatchedInputLengths { particle_slots: 3, hanna_params }) if hanna_params == slots)
            );
        }
        for bytes in [4, size_of::<HannaParams>() as u64 * 3 - 4] {
            let mut output = HannaParamsOutputBuffer::new(&ctx, 8).expect("logical capacity");
            output.buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("undersized Hanna storage"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            assert!(matches!(
                encode_hanna_params_gpu_with_kernel(
                    &ctx,
                    &particles,
                    &pbl,
                    &output,
                    &kernel,
                    &mut encoder
                ),
                Err(GpuHannaError::LengthMismatch {
                    field: "hanna_params_outputs_storage",
                    expected: 3,
                    ..
                })
            ));
            assert!(
                matches!(encode_update_particles_turbulence_langevin_gpu_with_hanna_buffer_and_kernel(&ctx, &particles, &output.buffer, 8, LangevinStep::legacy(1.0, 0.0), [1, 2], [0; 4], &consumer, &mut encoder), Err(GpuLangevinError::InsufficientHannaStorage { actual, .. }) if actual == bytes)
            );
        }
        // Finishing an empty encoder proves rejection happened before any invalid binding.
        let _commands = encoder.finish();
        eprintln!("HANNA-BOUNDS-139: logical ranges 0/1/2 and actual storage below active=3 rejected before submission");
    }

    #[test]
    fn gpu_hanna_matches_cpu_reference_for_stability_regimes() {
        let ctx = match pollster::block_on(GpuContext::new()) {
            Ok(ctx) => ctx,
            Err(GpuError::NoAdapter) => return,
            Err(err) => panic!("unexpected GPU init error: {err}"),
        };

        let mut pbl = PblState::new(3, 1);
        // Unstable (L = -50 m)
        pbl.ustar[[0, 0]] = 0.30;
        pbl.wstar[[0, 0]] = 1.80;
        pbl.hmix[[0, 0]] = 900.0;
        pbl.oli[[0, 0]] = -0.02;
        // Neutral (|1/L| < 1e-5 => L = +inf)
        pbl.ustar[[1, 0]] = 0.25;
        pbl.wstar[[1, 0]] = 0.0;
        pbl.hmix[[1, 0]] = 800.0;
        pbl.oli[[1, 0]] = 0.0;
        // Stable (L = +100 m)
        pbl.ustar[[2, 0]] = 0.35;
        pbl.wstar[[2, 0]] = 0.0;
        pbl.hmix[[2, 0]] = 700.0;
        pbl.oli[[2, 0]] = 0.01;

        let mut store = ParticleStore::with_capacity(4);
        store
            .add(make_particle(0.2, 0.0, 120.0))
            .expect("slot 0 available");
        store
            .add(make_particle(1.4, 0.0, 80.0))
            .expect("slot 1 available");
        store
            .add(make_particle(2.0, 0.0, 200.0))
            .expect("slot 2 available");
        // Slot 3 stays zeroed/inactive by design.

        let pbl_buffers = PblBuffers::from_state(&ctx, &pbl).expect("pbl upload succeeds");
        let particle_buffers = ParticleBuffers::from_store(&ctx, &store);

        let actual = pollster::block_on(compute_hanna_params_gpu(
            &ctx,
            &particle_buffers,
            &pbl_buffers,
        ))
        .expect("gpu hanna dispatch succeeds");

        assert_eq!(actual.len(), 4);
        let particles = store.as_slice();
        for idx in 0..3 {
            let expected = expected_from_cpu(&pbl, &particles[idx]);
            assert_hanna_close(&actual[idx], &expected);
        }

        let inactive_expected = HannaParams::zeroed();
        assert_eq!(actual[3].ust, inactive_expected.ust);
        assert_eq!(actual[3].wst, inactive_expected.wst);
        assert_eq!(actual[3].ol, inactive_expected.ol);
        assert_eq!(actual[3].h, inactive_expected.h);
        assert_eq!(actual[3].zeta, inactive_expected.zeta);
        assert_eq!(actual[3].sigu, inactive_expected.sigu);
        assert_eq!(actual[3].sigv, inactive_expected.sigv);
        assert_eq!(actual[3].sigw, inactive_expected.sigw);
        assert_eq!(actual[3].dsigwdz, inactive_expected.dsigwdz);
        assert_eq!(actual[3].dsigw2dz, inactive_expected.dsigw2dz);
        assert_eq!(actual[3].tlu, inactive_expected.tlu);
        assert_eq!(actual[3].tlv, inactive_expected.tlv);
        assert_eq!(actual[3].tlw, inactive_expected.tlw);
    }
}
