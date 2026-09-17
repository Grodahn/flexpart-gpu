//! GPU per-species radioactive decay dispatch.
//!
//! Ported from FLEXPART decay handling:
//! - `drydepo_mod.f90:337-338` (`decfact = exp(-|lsynctime| * decay)`)
//! - `unc_mod.f90:225` (`exp(-outstep * decay)` in `deposit_decay`)
//!
//! This kernel attenuates each species mass slot of every active particle:
//! `mass_new[s] = mass_old[s] * exp(-lambda_s * |dt|)`.
//!
//! The decay survival factor commutes with deposition survival factors, so
//! dispatch order relative to dry/wet deposition does not affect the result.
//!
//! Buffer contract:
//! - binding 0: `array<Particle>` read-write storage buffer.
//! - binding 1: uniform params `(particle_count, dt_seconds, pad, pad,
//!   decay_constants_s_inv vec4)`.

use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::particles::{Particle, ParticleStore, MAX_SPECIES};

use super::{GpuBufferError, GpuContext, GpuError, ParticleBuffers};

const SHADER_TEMPLATE: &str = include_str!("../shaders/decay.wgsl");

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct DecayDispatchParamsRaw {
    particle_count: u32,
    dt_seconds: f32,
    _pad0: f32,
    _pad1: f32,
    decay_constants_s_inv: [f32; MAX_SPECIES],
}

/// Radioactive decay dispatch parameters.
#[derive(Debug, Clone, Copy)]
pub struct DecayStepParams {
    /// Timestep duration [s]. Absolute value is used in the exponential term.
    pub dt_seconds: f32,
    /// Decay constant per species slot [1/s]. Zero means stable.
    pub decay_constants_s_inv: [f32; MAX_SPECIES],
}

impl Default for DecayStepParams {
    fn default() -> Self {
        Self {
            dt_seconds: 0.0,
            decay_constants_s_inv: [0.0; MAX_SPECIES],
        }
    }
}

/// Whether any species slot decays (used to skip the dispatch entirely).
#[must_use]
pub fn decay_dispatch_needed(decay_constants_s_inv: &[f32; MAX_SPECIES]) -> bool {
    decay_constants_s_inv
        .iter()
        .any(|lambda| lambda.is_finite() && *lambda > 0.0)
}

/// Reusable decay dispatch kernel objects.
pub struct DecayDispatchKernel {
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::ComputePipeline,
    workgroup_size_x: u32,
}

impl DecayDispatchKernel {
    #[must_use]
    pub fn new(ctx: &GpuContext) -> Self {
        let workgroup_size_x = super::runtime_workgroup_size(ctx, super::WorkgroupKernel::Decay);
        let shader_source =
            super::render_shader_with_workgroup_size(SHADER_TEMPLATE, workgroup_size_x);
        let shader = ctx.load_shader("decay_shader", &shader_source);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("decay_bgl"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
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
        let pipeline =
            ctx.create_compute_pipeline("decay_pipeline", &shader, "main", &[&bind_group_layout]);
        Self {
            bind_group_layout,
            pipeline,
            workgroup_size_x,
        }
    }
}

/// Errors returned by GPU decay dispatch.
#[derive(Debug, Error)]
pub enum GpuDecayError {
    #[error("value for {field} does not fit in u32: {value}")]
    ValueTooLarge { field: &'static str, value: usize },
    #[error("invalid dt_seconds for decay: {dt_seconds}")]
    InvalidTimeStep { dt_seconds: f32 },
    #[error("buffer operation failed: {0}")]
    Buffer(#[from] GpuBufferError),
}

/// Errors for the higher-level decay particle workflow helper.
#[derive(Debug, Error)]
pub enum GpuDecayWorkflowError {
    #[error("gpu initialization failed: {0}")]
    Gpu(#[from] GpuError),
    #[error("decay dispatch failed: {0}")]
    Decay(#[from] GpuDecayError),
    #[error("particle buffer readback failed: {0}")]
    Buffer(#[from] GpuBufferError),
}

fn usize_to_u32(value: usize, field: &'static str) -> Result<u32, GpuDecayError> {
    u32::try_from(value).map_err(|_| GpuDecayError::ValueTooLarge { field, value })
}

/// Dispatch radioactive decay attenuation in place on GPU particle masses.
///
/// # Errors
///
/// Returns [`GpuDecayError::InvalidTimeStep`] for non-finite `dt_seconds`.
pub fn dispatch_decay_gpu(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    params: DecayStepParams,
) -> Result<(), GpuDecayError> {
    let kernel = DecayDispatchKernel::new(ctx);
    dispatch_decay_gpu_with_kernel(ctx, particles, params, &kernel)
}

/// Dispatch radioactive decay using a reusable prepared kernel.
///
/// # Errors
///
/// Returns [`GpuDecayError::InvalidTimeStep`] for non-finite `dt_seconds`.
pub fn dispatch_decay_gpu_with_kernel(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    params: DecayStepParams,
    kernel: &DecayDispatchKernel,
) -> Result<(), GpuDecayError> {
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("decay_encoder"),
        });
    encode_decay_gpu_with_kernel(ctx, particles, params, kernel, &mut encoder)?;
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    Ok(())
}

/// Encode radioactive decay dispatch into a caller-provided command encoder.
///
/// # Errors
///
/// Returns [`GpuDecayError::InvalidTimeStep`] for non-finite `dt_seconds`.
pub fn encode_decay_gpu_with_kernel(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    params: DecayStepParams,
    kernel: &DecayDispatchKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuDecayError> {
    let particle_count = particles.particle_count();
    if particle_count == 0 {
        return Ok(());
    }
    if !params.dt_seconds.is_finite() {
        return Err(GpuDecayError::InvalidTimeStep {
            dt_seconds: params.dt_seconds,
        });
    }

    debug_assert_eq!(Particle::GPU_SIZE, 96);
    debug_assert_eq!(
        size_of::<DecayDispatchParamsRaw>() % 16,
        0,
        "decay uniform params must stay 16-byte aligned"
    );

    let raw_params = DecayDispatchParamsRaw {
        particle_count: usize_to_u32(particle_count, "particle_count")?,
        dt_seconds: params.dt_seconds,
        _pad0: 0.0,
        _pad1: 0.0,
        decay_constants_s_inv: params.decay_constants_s_inv,
    };

    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("decay_params"),
            contents: bytemuck::bytes_of(&raw_params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("decay_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: particles.particle_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });
    {
        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("decay_pass"),
            timestamp_writes: None,
        });
        cpass.set_pipeline(&kernel.pipeline);
        cpass.set_bind_group(0, &bind_group, &[]);
        super::dispatch_1d(
            &mut cpass,
            raw_params.particle_count,
            kernel.workgroup_size_x,
        );
    }
    Ok(())
}

/// Minimal callable decay API for the GPU particle update workflow.
///
/// Particle masses are attenuated in place on the GPU particle buffer.
///
/// # Errors
///
/// Forwards [`GpuDecayError`] from dispatch (e.g. non-finite `dt_seconds`).
pub async fn apply_decay_step_gpu(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    params: DecayStepParams,
) -> Result<(), GpuDecayError> {
    dispatch_decay_gpu(ctx, particles, params)
}

/// Workflow helper that updates a CPU particle store via GPU when available.
///
/// Returns `Ok(true)` when a GPU adapter exists and the kernel runs,
/// `Ok(false)` when no GPU adapter is available (graceful skip).
///
/// # Errors
///
/// Forwards [`GpuDecayWorkflowError`] from GPU dispatch or readback.
pub async fn apply_decay_step_workflow(
    particles: &mut ParticleStore,
    params: DecayStepParams,
) -> Result<bool, GpuDecayWorkflowError> {
    let ctx = match GpuContext::new().await {
        Ok(ctx) => ctx,
        Err(GpuError::NoAdapter) => return Ok(false),
        Err(err) => return Err(err.into()),
    };

    let gpu_particles = ParticleBuffers::from_store(&ctx, particles);
    apply_decay_step_gpu(&ctx, &gpu_particles, params).await?;

    let updated_particles = gpu_particles.download_particles(&ctx).await?;
    particles.as_mut_slice().copy_from_slice(&updated_particles);
    particles.recount_active();

    Ok(true)
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;
    use crate::gpu::GpuError;
    use crate::particles::ParticleInit;
    use crate::physics::{apply_decay_mass_step, decay_survival_factor};

    // Test-only coordinates are small positives, so float-to-int casts are
    // exact; mirrors the particle helpers in the deposition module tests.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn particle_at(x: f32, y: f32, z: f32, mass: [f32; MAX_SPECIES]) -> Particle {
        let cell_x = x.floor() as i32;
        let cell_y = y.floor() as i32;
        Particle::new(&ParticleInit {
            cell_x,
            cell_y,
            pos_x: x - cell_x as f32,
            pos_y: y - cell_y as f32,
            pos_z: z,
            mass,
            release_point: 0,
            class: 0,
            time: 0,
        })
    }

    #[test]
    fn gpu_decay_matches_cpu_reference_per_species() {
        let ctx = match pollster::block_on(GpuContext::new()) {
            Ok(ctx) => ctx,
            Err(GpuError::NoAdapter) => return,
            Err(err) => panic!("unexpected GPU init error: {err}"),
        };

        let mut particles = vec![
            particle_at(1.2, 2.3, 2.0, [1.0, 2.0, 4.0, 8.0]),
            particle_at(0.5, 0.25, 30.0, [3.0, 1.5, 0.5, 0.25]),
            particle_at(2.0, 4.0, 10.0, [5.0, 5.0, 5.0, 5.0]),
        ];
        particles[2].deactivate();

        // Decaying, stable, fast-decaying, and sentinel-ish zero lanes.
        let lambdas = [0.01, 0.0, 0.05, -1.0];
        let params = DecayStepParams {
            dt_seconds: 60.0,
            decay_constants_s_inv: lambdas,
        };

        let expected_mass: Vec<[f32; MAX_SPECIES]> = particles
            .iter()
            .map(|particle| {
                let mut mass = [0.0; MAX_SPECIES];
                if particle.is_active() {
                    for s in 0..MAX_SPECIES {
                        let (remaining, _) =
                            apply_decay_mass_step(particle.mass[s], lambdas[s], params.dt_seconds);
                        mass[s] = remaining;
                    }
                } else {
                    mass = particle.mass;
                }
                mass
            })
            .collect();

        let particle_buffers = ParticleBuffers::from_particles(&ctx, &particles);
        pollster::block_on(apply_decay_step_gpu(&ctx, &particle_buffers, params))
            .expect("gpu decay succeeds");

        let updated = pollster::block_on(particle_buffers.download_particles(&ctx))
            .expect("particle readback succeeds");
        for (particle, expected) in updated.iter().zip(expected_mass.iter()) {
            for s in 0..MAX_SPECIES {
                assert_relative_eq!(
                    particle.mass[s],
                    expected[s],
                    epsilon = 1.0e-6,
                    max_relative = 1.0e-6
                );
            }
        }

        // Spot-check one lane against the closed form.
        let survival = decay_survival_factor(0.01, 60.0);
        assert_relative_eq!(
            updated[0].mass[0],
            1.0 * survival,
            epsilon = 1.0e-6,
            max_relative = 1.0e-6
        );
        assert_relative_eq!(updated[0].mass[1], 2.0, epsilon = 1.0e-6);
    }

    #[test]
    fn decay_dispatch_needed_flags_stable_vectors() {
        assert!(!decay_dispatch_needed(&[0.0; MAX_SPECIES]));
        assert!(!decay_dispatch_needed(&[-1.0, 0.0, 0.0, f32::NAN]));
        assert!(decay_dispatch_needed(&[0.0, 1.0e-6, 0.0, 0.0]));
    }

    #[test]
    fn workflow_api_gracefully_skips_without_adapter() {
        let mut store = ParticleStore::with_capacity(2);
        store
            .add(particle_at(0.5, 0.5, 1.0, [1.0; MAX_SPECIES]))
            .expect("slot 0 available");
        store
            .add(particle_at(1.5, 1.5, 2.0, [1.0; MAX_SPECIES]))
            .expect("slot 1 available");

        let initial: Vec<[f32; MAX_SPECIES]> = store.as_slice().iter().map(|p| p.mass).collect();
        let result = pollster::block_on(apply_decay_step_workflow(
            &mut store,
            DecayStepParams {
                dt_seconds: 30.0,
                decay_constants_s_inv: [0.01, 0.0, 0.0, 0.0],
            },
        ))
        .expect("workflow api should not fail on missing adapter");

        if result {
            assert!(store.as_slice()[0].mass[0] < initial[0][0]);
            assert!((store.as_slice()[0].mass[1] - initial[0][1]).abs() < f32::EPSILON);
        } else {
            let after: Vec<[f32; MAX_SPECIES]> = store.as_slice().iter().map(|p| p.mass).collect();
            assert_eq!(after, initial);
        }
    }
}
