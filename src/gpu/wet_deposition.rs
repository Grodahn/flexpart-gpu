//! GPU per-species wet deposition probability dispatch (D-04).
//!
//! Ported from FLEXPART wet-deposition mass-loss update in `wetdepo.f90`:
//! `p = grfraction * (1 - exp(-wetscav * |dt|))`, applied per species slot.
//!
//! This kernel computes per-particle per-species wet-deposition probability
//! and applies the corresponding survival factor to each species mass slot:
//! `mass_new[s] = mass_old[s] * (1 - p_s)`.
//!
//! MVP assumption:
//! - D-03 CPU logic supplies per-particle per-species `wetscav` and shared
//!   per-particle `grfraction` inputs.
//! - This GPU stage applies only the shared wetdepo mass-loss contract, which
//!   keeps parity for probability/mass attenuation while deferring full
//!   below-cloud/in-cloud coefficient branching to upstream CPU code.
//!
//! Buffer contract:
//! - binding 0: `array<Particle>` read-write storage buffer.
//! - binding 1: `array<vec4<f32>>` scavenging coefficient (`wetscav`) per particle per species [1/s].
//! - binding 2: `array<f32>` precipitating fraction (`grfraction`) per particle [-], shared across species.
//! - binding 3: `array<vec4<f32>>` wet-deposition probability output per particle per species [-].
//! - binding 4: uniform params `(particle_count, dt_seconds, pad0, pad1)`.

use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::particles::{Particle, ParticleStore, MAX_SPECIES};

use super::{
    download_buffer_typed, render_shader_with_workgroup_size, runtime_workgroup_size,
    GpuBufferError, GpuContext, GpuError, ParticleBuffers, WorkgroupKernel,
};

const SHADER_TEMPLATE: &str = include_str!("../shaders/wet_deposition.wgsl");

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct WetDepositionDispatchParamsRaw {
    particle_count: u32,
    dt_seconds: f32,
    _pad0: f32,
    _pad1: f32,
}

/// Wet deposition dispatch parameters.
#[derive(Debug, Clone, Copy)]
pub struct WetDepositionStepParams {
    /// Timestep duration [s]. Absolute value is used in the exponential term.
    pub dt_seconds: f32,
}

impl Default for WetDepositionStepParams {
    fn default() -> Self {
        Self { dt_seconds: 0.0 }
    }
}

/// Reusable wet-deposition dispatch kernel objects.
pub struct WetDepositionDispatchKernel {
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::ComputePipeline,
    workgroup_size_x: u32,
}

impl WetDepositionDispatchKernel {
    #[must_use]
    pub fn new(ctx: &GpuContext) -> Self {
        let workgroup_size_x = runtime_workgroup_size(ctx, WorkgroupKernel::WetDeposition);
        let shader_source = render_shader_with_workgroup_size(SHADER_TEMPLATE, workgroup_size_x);
        let shader = ctx.load_shader("wet_deposition_shader", &shader_source);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("wet_deposition_bgl"),
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
            "wet_deposition_pipeline",
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

/// Typed IO buffers for wet deposition dispatch.
///
/// Scavenging-coefficient and probability buffers hold one `vec4` per particle
/// slot, lane `s` carrying species slot `s` ([`MAX_SPECIES`] lanes, unused
/// lanes zero). The precipitating fraction is geometric (shared sub-grid
/// precipitating area) and stays a single scalar per particle.
pub struct WetDepositionIoBuffers {
    pub scavenging_coefficient_s_inv: wgpu::Buffer,
    pub precipitating_fraction: wgpu::Buffer,
    pub wet_deposition_probability: wgpu::Buffer,
    particle_count: usize,
}

impl WetDepositionIoBuffers {
    pub fn from_inputs(
        ctx: &GpuContext,
        scavenging_coefficient_s_inv: &[[f32; MAX_SPECIES]],
        precipitating_fraction: &[f32],
    ) -> Result<Self, GpuWetDepositionError> {
        if scavenging_coefficient_s_inv.len() != precipitating_fraction.len() {
            return Err(GpuWetDepositionError::LengthMismatch {
                field: "precipitating_fraction",
                expected: scavenging_coefficient_s_inv.len(),
                actual: precipitating_fraction.len(),
            });
        }

        let particle_count = scavenging_coefficient_s_inv.len();
        let scavenging_buffer = create_species_input_buffer(
            &ctx.device,
            "wet_dep_scavenging_coefficient_s_inv",
            scavenging_coefficient_s_inv,
        );
        let precipitating_fraction_buffer = create_storage_input_buffer(
            &ctx.device,
            "wet_dep_precipitating_fraction",
            precipitating_fraction,
        );

        let probability_byte_len =
            checked_byte_len::<[f32; MAX_SPECIES]>(particle_count, "wet_deposition_probability")?;
        let probability_size = if probability_byte_len == 0 {
            4
        } else {
            u64::try_from(probability_byte_len).map_err(|_| {
                GpuWetDepositionError::SizeOverflow {
                    field: "wet_deposition_probability",
                }
            })?
        };
        let probability_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wet_dep_probability"),
            size: probability_size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            scavenging_coefficient_s_inv: scavenging_buffer,
            precipitating_fraction: precipitating_fraction_buffer,
            wet_deposition_probability: probability_buffer,
            particle_count,
        })
    }

    #[must_use]
    pub fn particle_count(&self) -> usize {
        self.particle_count
    }

    pub fn upload_scavenging_coefficient(
        &self,
        ctx: &GpuContext,
        scavenging_coefficient_s_inv: &[[f32; MAX_SPECIES]],
    ) -> Result<(), GpuWetDepositionError> {
        if scavenging_coefficient_s_inv.len() != self.particle_count {
            return Err(GpuWetDepositionError::LengthMismatch {
                field: "scavenging_coefficient_s_inv",
                expected: self.particle_count,
                actual: scavenging_coefficient_s_inv.len(),
            });
        }
        if scavenging_coefficient_s_inv.is_empty() {
            return Ok(());
        }
        ctx.queue.write_buffer(
            &self.scavenging_coefficient_s_inv,
            0,
            bytemuck::cast_slice(scavenging_coefficient_s_inv),
        );
        Ok(())
    }

    pub fn upload_precipitating_fraction(
        &self,
        ctx: &GpuContext,
        precipitating_fraction: &[f32],
    ) -> Result<(), GpuWetDepositionError> {
        if precipitating_fraction.len() != self.particle_count {
            return Err(GpuWetDepositionError::LengthMismatch {
                field: "precipitating_fraction",
                expected: self.particle_count,
                actual: precipitating_fraction.len(),
            });
        }
        if precipitating_fraction.is_empty() {
            return Ok(());
        }
        ctx.queue.write_buffer(
            &self.precipitating_fraction,
            0,
            bytemuck::cast_slice(precipitating_fraction),
        );
        Ok(())
    }

    pub async fn download_probabilities(
        &self,
        ctx: &GpuContext,
    ) -> Result<Vec<[f32; MAX_SPECIES]>, GpuWetDepositionError> {
        if self.particle_count == 0 {
            return Ok(Vec::new());
        }
        download_buffer_typed::<[f32; MAX_SPECIES]>(
            ctx,
            &self.wet_deposition_probability,
            self.particle_count,
            "wet_deposition_probability",
        )
        .await
        .map_err(Into::into)
    }
}

fn create_species_input_buffer(
    device: &wgpu::Device,
    label: &str,
    data: &[[f32; MAX_SPECIES]],
) -> wgpu::Buffer {
    if data.is_empty() {
        return device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(data),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

fn create_storage_input_buffer(device: &wgpu::Device, label: &str, data: &[f32]) -> wgpu::Buffer {
    if data.is_empty() {
        return device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(data),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

/// Errors returned by GPU wet deposition dispatch.
#[derive(Debug, Error)]
pub enum GpuWetDepositionError {
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
    #[error("invalid dt_seconds for wet deposition: {dt_seconds}")]
    InvalidTimeStep { dt_seconds: f32 },
    #[error("buffer operation failed: {0}")]
    Buffer(#[from] GpuBufferError),
}

/// Errors for the higher-level wet-deposition particle workflow helper.
#[derive(Debug, Error)]
pub enum GpuWetDepositionWorkflowError {
    #[error("gpu initialization failed: {0}")]
    Gpu(#[from] GpuError),
    #[error("wet deposition dispatch failed: {0}")]
    Deposition(#[from] GpuWetDepositionError),
    #[error("particle buffer readback failed: {0}")]
    Buffer(#[from] GpuBufferError),
}

fn usize_to_u32(value: usize, field: &'static str) -> Result<u32, GpuWetDepositionError> {
    u32::try_from(value).map_err(|_| GpuWetDepositionError::ValueTooLarge { field, value })
}

fn checked_byte_len<T>(len: usize, field: &'static str) -> Result<usize, GpuWetDepositionError> {
    len.checked_mul(size_of::<T>())
        .ok_or(GpuWetDepositionError::SizeOverflow { field })
}

/// Dispatch wet deposition probability computation and in-place mass attenuation.
pub fn dispatch_wet_deposition_probability_gpu(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    io: &WetDepositionIoBuffers,
    params: WetDepositionStepParams,
) -> Result<(), GpuWetDepositionError> {
    let kernel = WetDepositionDispatchKernel::new(ctx);
    dispatch_wet_deposition_probability_gpu_with_kernel(ctx, particles, io, params, &kernel)
}

/// Dispatch wet deposition using a reusable prepared kernel.
pub fn dispatch_wet_deposition_probability_gpu_with_kernel(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    io: &WetDepositionIoBuffers,
    params: WetDepositionStepParams,
    kernel: &WetDepositionDispatchKernel,
) -> Result<(), GpuWetDepositionError> {
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("wet_deposition_encoder"),
        });
    encode_wet_deposition_probability_gpu_with_kernel(
        ctx,
        particles,
        io,
        params,
        kernel,
        &mut encoder,
    )?;
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    Ok(())
}

/// Encode wet deposition for the current particle prefix into the caller's encoder.
///
/// Capacity-sized IO remains resident; each resource must cover every accessed
/// slot. Encoding does not submit, synchronize, resize, or read back resources.
///
/// # Errors
///
/// Returns an error for IO or storage shorter than the accessed prefix, an
/// unrepresentable dispatch size, or a nonfinite timestep.
pub fn encode_wet_deposition_probability_gpu_with_kernel(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    io: &WetDepositionIoBuffers,
    params: WetDepositionStepParams,
    kernel: &WetDepositionDispatchKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuWetDepositionError> {
    let particle_count = particles.particle_count();
    if particle_count == 0 {
        return Ok(());
    }
    validate_wet_deposition_prefix(particles, io)?;
    if !params.dt_seconds.is_finite() {
        return Err(GpuWetDepositionError::InvalidTimeStep {
            dt_seconds: params.dt_seconds,
        });
    }

    let raw_params = WetDepositionDispatchParamsRaw {
        particle_count: usize_to_u32(particle_count, "particle_count")?,
        dt_seconds: params.dt_seconds,
        _pad0: 0.0,
        _pad1: 0.0,
    };

    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("wet_deposition_params"),
            contents: bytemuck::bytes_of(&raw_params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("wet_deposition_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: particles.particle_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: io.scavenging_coefficient_s_inv.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: io.precipitating_fraction.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: io.wet_deposition_probability.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });
    {
        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("wet_deposition_pass"),
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

fn validate_wet_deposition_prefix(
    particles: &ParticleBuffers,
    io: &WetDepositionIoBuffers,
) -> Result<(), GpuWetDepositionError> {
    let particle_count = particles.particle_count();
    if io.particle_count < particle_count {
        return Err(GpuWetDepositionError::LengthMismatch {
            field: "wet_deposition_io_buffers",
            expected: particle_count,
            actual: io.particle_count,
        });
    }
    // Public buffer handles can be replaced independently of their slot metadata.
    // Validate actual storage before binding so WGSL robust bounds handling
    // cannot silently discard accesses to undersized resources.
    for (buffer, slot_size, field) in [
        (
            &particles.particle_buffer,
            size_of::<Particle>(),
            "particles",
        ),
        (
            &io.scavenging_coefficient_s_inv,
            size_of::<[f32; MAX_SPECIES]>(),
            "scavenging_coefficient_s_inv",
        ),
        (
            &io.precipitating_fraction,
            size_of::<f32>(),
            "precipitating_fraction",
        ),
        (
            &io.wet_deposition_probability,
            size_of::<[f32; MAX_SPECIES]>(),
            "wet_deposition_probability",
        ),
    ] {
        let required_bytes = particle_count
            .checked_mul(slot_size)
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(GpuWetDepositionError::SizeOverflow { field })?;
        if buffer.size() < required_bytes {
            return Err(GpuWetDepositionError::LengthMismatch {
                field,
                expected: particle_count,
                actual: usize::try_from(buffer.size() / slot_size as u64)
                    .map_err(|_| GpuWetDepositionError::SizeOverflow { field })?,
            });
        }
    }
    Ok(())
}

/// Minimal callable wet-deposition API for the GPU particle update workflow.
///
/// This convenience helper:
/// 1. creates typed scavenging/fraction/probability IO buffers,
/// 2. dispatches the WGSL wet-deposition kernel,
/// 3. returns per-particle per-species wet-deposition probability output.
///
/// Particle masses are attenuated in place on the GPU particle buffer.
pub async fn apply_wet_deposition_step_gpu(
    ctx: &GpuContext,
    particles: &ParticleBuffers,
    scavenging_coefficient_s_inv: &[[f32; MAX_SPECIES]],
    precipitating_fraction: &[f32],
    params: WetDepositionStepParams,
) -> Result<Vec<[f32; MAX_SPECIES]>, GpuWetDepositionError> {
    if scavenging_coefficient_s_inv.len() != particles.particle_count() {
        return Err(GpuWetDepositionError::LengthMismatch {
            field: "scavenging_coefficient_s_inv",
            expected: particles.particle_count(),
            actual: scavenging_coefficient_s_inv.len(),
        });
    }
    if precipitating_fraction.len() != particles.particle_count() {
        return Err(GpuWetDepositionError::LengthMismatch {
            field: "precipitating_fraction",
            expected: particles.particle_count(),
            actual: precipitating_fraction.len(),
        });
    }

    let io = WetDepositionIoBuffers::from_inputs(
        ctx,
        scavenging_coefficient_s_inv,
        precipitating_fraction,
    )?;
    dispatch_wet_deposition_probability_gpu(ctx, particles, &io, params)?;
    io.download_probabilities(ctx).await
}

/// Workflow helper that updates a CPU particle store through the required GPU path.
///
/// A missing adapter is returned as an explicit error; it is never represented
/// as a successful skipped calculation.
///
/// # Errors
///
/// Returns [`GpuWetDepositionWorkflowError`] when GPU initialization,
/// dispatch, or readback fails.
pub async fn apply_wet_deposition_step_workflow(
    particles: &mut ParticleStore,
    scavenging_coefficient_s_inv: &[[f32; MAX_SPECIES]],
    precipitating_fraction: &[f32],
    params: WetDepositionStepParams,
) -> Result<Vec<[f32; MAX_SPECIES]>, GpuWetDepositionWorkflowError> {
    let ctx = GpuContext::new().await?;

    let gpu_particles = ParticleBuffers::from_store(&ctx, particles);
    let probabilities = apply_wet_deposition_step_gpu(
        &ctx,
        &gpu_particles,
        scavenging_coefficient_s_inv,
        precipitating_fraction,
        params,
    )
    .await?;

    let updated_particles = gpu_particles.download_particles(&ctx).await?;
    particles.as_mut_slice().copy_from_slice(&updated_particles);
    particles.recount_active();

    Ok(probabilities)
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;
    use crate::gpu::GpuError;
    use crate::particles::{Particle, ParticleInit, MAX_SPECIES};
    use crate::physics::{wet_scavenging_probability_step, WetScavengingStep};

    fn particle_at(x: f32, y: f32, z: f32, mass0: f32) -> Particle {
        let cell_x = x.floor() as i32;
        let cell_y = y.floor() as i32;
        let mut mass = [0.0; MAX_SPECIES];
        mass[0] = mass0;
        mass[1] = 0.5 * mass0;
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
    fn gpu_wet_deposition_probabilities_match_cpu_reference() {
        let ctx = match pollster::block_on(GpuContext::new()) {
            Ok(ctx) => ctx,
            Err(GpuError::NoAdapter) => return,
            Err(err) => panic!("unexpected GPU init error: {err}"),
        };

        let mut particles = vec![
            particle_at(1.2, 2.3, 2.0, 1.0),
            particle_at(0.5, 0.25, 30.0, 2.0),
            particle_at(3.1, 1.1, 0.2, 3.0),
            particle_at(2.0, 4.0, 10.0, 4.0),
            particle_at(2.2, 5.4, 100.0, 5.0),
        ];
        particles[3].deactivate();

        let scavenging_coefficient_s_inv: Vec<[f32; MAX_SPECIES]> = vec![
            [0.02, 0.01, 0.0, 0.05],
            [0.0, 0.0, 0.0, 0.0],
            [0.15, 0.2, 0.1, 0.0],
            [0.03, 0.03, 0.03, 0.03],
            [-1.0, 0.04, 0.04, 0.04],
        ];
        let precipitating_fraction = vec![0.5, 1.0, 1.2, 0.7, 0.9];
        let params = WetDepositionStepParams { dt_seconds: 60.0 };

        let expected_prob: Vec<[f32; MAX_SPECIES]> = particles
            .iter()
            .zip(
                scavenging_coefficient_s_inv
                    .iter()
                    .zip(precipitating_fraction.iter()),
            )
            .map(|(particle, (lambdas, &fraction))| {
                let mut prob = [0.0; MAX_SPECIES];
                if particle.is_active() {
                    for s in 0..MAX_SPECIES {
                        prob[s] = wet_scavenging_probability_step(WetScavengingStep {
                            scavenging_coefficient_s_inv: lambdas[s],
                            dt_seconds: params.dt_seconds,
                            precipitating_fraction: fraction,
                        });
                    }
                }
                prob
            })
            .collect();

        let expected_mass: Vec<[f32; MAX_SPECIES]> = particles
            .iter()
            .zip(expected_prob.iter())
            .map(|(particle, prob)| {
                let mut mass = [0.0; MAX_SPECIES];
                for s in 0..MAX_SPECIES {
                    mass[s] = particle.mass[s] * (1.0 - prob[s]);
                }
                mass
            })
            .collect();

        let particle_buffers = ParticleBuffers::from_particles(&ctx, &particles);
        let probabilities = pollster::block_on(apply_wet_deposition_step_gpu(
            &ctx,
            &particle_buffers,
            &scavenging_coefficient_s_inv,
            &precipitating_fraction,
            params,
        ))
        .expect("gpu wet deposition succeeds");

        assert_eq!(probabilities.len(), expected_prob.len());
        for (gpu, cpu) in probabilities.iter().zip(expected_prob.iter()) {
            for s in 0..MAX_SPECIES {
                assert_relative_eq!(gpu[s], cpu[s], epsilon = 1.0e-6, max_relative = 1.0e-6);
            }
        }

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
    }

    #[test]
    fn test_wet_deposition_capacity_io_preserves_active_prefix_and_tail() {
        use sha2::{Digest, Sha256};

        let ctx = pollster::block_on(GpuContext::new()).expect("prefix regression requires WGSL");
        let kernel = WetDepositionDispatchKernel::new(&ctx);
        let params = WetDepositionStepParams { dt_seconds: 1.0 };
        for active_count in [1, 3, 8] {
            let particles: Vec<_> = (0..8)
                .map(|slot| {
                    let mut particle = particle_at(0.5, 0.5, 1.0, (slot + 1) as f32);
                    particle.release_point = slot;
                    particle.mass = [1.0, 2.0, 3.0, 4.0].map(|mass| mass * (slot + 1) as f32);
                    if slot as usize >= active_count {
                        particle.deactivate();
                    }
                    particle
                })
                .collect();
            let coefficients: Vec<_> = (0..8)
                .map(|slot| [0.0, 0.1, 0.3, 0.5].map(|lambda| lambda + slot as f32 * 0.02))
                .collect();
            let fractions: Vec<_> = (0..8).map(|slot| 0.2 + slot as f32 * 0.1).collect();
            let mut prefix = ParticleBuffers::from_particles(&ctx, &particles);
            prefix.set_dispatch_count(active_count);
            let control = ParticleBuffers::from_particles(&ctx, &particles);
            let prefix_io = WetDepositionIoBuffers::from_inputs(&ctx, &coefficients, &fractions)
                .expect("prefix IO");
            let control_io = WetDepositionIoBuffers::from_inputs(&ctx, &coefficients, &fractions)
                .expect("control IO");
            let sentinel = vec![[0.75; MAX_SPECIES]; 8];
            ctx.queue.write_buffer(
                &prefix_io.wet_deposition_probability,
                0,
                bytemuck::cast_slice(&sentinel),
            );
            let mut encoder = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            for (buffers, io) in [(&prefix, &prefix_io), (&control, &control_io)] {
                encode_wet_deposition_probability_gpu_with_kernel(
                    &ctx,
                    buffers,
                    io,
                    params,
                    &kernel,
                    &mut encoder,
                )
                .expect("encode");
            }
            ctx.queue.submit(Some(encoder.finish()));
            let updated = pollster::block_on(prefix.download_particles(&ctx)).expect("prefix");
            let expected = pollster::block_on(control.download_particles(&ctx)).expect("control");
            let probability = pollster::block_on(prefix_io.download_probabilities(&ctx))
                .expect("prefix probabilities");
            let control_probability = pollster::block_on(control_io.download_probabilities(&ctx))
                .expect("control probabilities");
            assert_eq!(prefix.capacity(), 8);
            assert_eq!(prefix_io.particle_count(), 8);
            for slot in 0..8 {
                assert_eq!(updated[slot].release_point, particles[slot].release_point);
                assert_eq!(
                    bytemuck::bytes_of(&updated[slot]),
                    bytemuck::bytes_of(&expected[slot])
                );
                if slot < active_count {
                    assert_eq!(probability[slot], control_probability[slot]);
                    for lane in 0..MAX_SPECIES {
                        let analytical_probability = fractions[slot]
                            * (1.0 - (-coefficients[slot][lane] * params.dt_seconds).exp());
                        assert_relative_eq!(
                            probability[slot][lane],
                            analytical_probability,
                            epsilon = 1.0e-6,
                            max_relative = 1.0e-6
                        );
                        assert_relative_eq!(
                            updated[slot].mass[lane],
                            particles[slot].mass[lane] * (1.0 - analytical_probability),
                            epsilon = 1.0e-6,
                            max_relative = 1.0e-6
                        );
                    }
                } else {
                    assert_eq!(
                        bytemuck::bytes_of(&updated[slot]),
                        bytemuck::bytes_of(&particles[slot])
                    );
                    assert_eq!(
                        probability[slot], sentinel[slot],
                        "tail must not be encoded"
                    );
                    assert_eq!(control_probability[slot], [0.0; MAX_SPECIES]);
                }
            }
            let mut input_hash = Sha256::new();
            input_hash.update(bytemuck::cast_slice::<Particle, u8>(&particles));
            input_hash.update(bytemuck::cast_slice::<[f32; MAX_SPECIES], u8>(
                &coefficients,
            ));
            input_hash.update(bytemuck::cast_slice::<f32, u8>(&fractions));
            input_hash.update(bytemuck::bytes_of(&WetDepositionDispatchParamsRaw {
                particle_count: active_count as u32,
                dt_seconds: params.dt_seconds,
                _pad0: 0.0,
                _pad1: 0.0,
            }));
            eprintln!("WET-PREFIX-138: adapter={:?} capacity=8 active={active_count} input_sha256={:x} particles={particles:?} coefficients={coefficients:?} fractions={fractions:?} masses={:?} probabilities={probability:?} control_probabilities={control_probability:?}", ctx.adapter_info(), input_hash.finalize(), updated.iter().map(|p| p.mass).collect::<Vec<_>>());
        }
    }

    #[test]
    fn test_wet_deposition_invalid_prefix_resources_fail_before_submission() {
        let ctx = pollster::block_on(GpuContext::new()).expect("bounds regression requires WGSL");
        let kernel = WetDepositionDispatchKernel::new(&ctx);
        let mut particles =
            ParticleBuffers::from_particles(&ctx, &[particle_at(0.5, 0.5, 1.0, 1.0); 8]);
        particles.set_dispatch_count(3);
        let params = WetDepositionStepParams { dt_seconds: 1.0 };
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let short_io =
            WetDepositionIoBuffers::from_inputs(&ctx, &[[0.3; MAX_SPECIES]; 2], &[0.5; 2])
                .expect("short IO");
        assert!(matches!(
            encode_wet_deposition_probability_gpu_with_kernel(
                &ctx,
                &particles,
                &short_io,
                params,
                &kernel,
                &mut encoder,
            ),
            Err(GpuWetDepositionError::LengthMismatch {
                field: "wet_deposition_io_buffers",
                expected: 3,
                actual: 2,
            })
        ));
        for field in [
            "particles",
            "scavenging_coefficient_s_inv",
            "precipitating_fraction",
            "wet_deposition_probability",
        ] {
            let mut buffers =
                ParticleBuffers::from_particles(&ctx, &[particle_at(0.5, 0.5, 1.0, 1.0); 8]);
            buffers.set_dispatch_count(3);
            let mut io =
                WetDepositionIoBuffers::from_inputs(&ctx, &[[0.3; MAX_SPECIES]; 8], &[0.5; 8])
                    .expect("IO");
            let slot_size = match field {
                "particles" => size_of::<Particle>(),
                "precipitating_fraction" => size_of::<f32>(),
                _ => size_of::<[f32; MAX_SPECIES]>(),
            };
            // Less than three complete slots, despite full-capacity metadata.
            let short = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("undersized wet resource"),
                size: (3 * slot_size - 4) as u64,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            match field {
                "particles" => buffers.particle_buffer = short,
                "scavenging_coefficient_s_inv" => io.scavenging_coefficient_s_inv = short,
                "precipitating_fraction" => io.precipitating_fraction = short,
                _ => io.wet_deposition_probability = short,
            }
            let error = encode_wet_deposition_probability_gpu_with_kernel(
                &ctx,
                &buffers,
                &io,
                params,
                &kernel,
                &mut encoder,
            )
            .expect_err("short actual storage");
            assert!(matches!(error,
                GpuWetDepositionError::LengthMismatch {
                    field: actual_field, expected: 3, actual: 2,
                } if actual_field == field
            ));
        }
        let io = WetDepositionIoBuffers::from_inputs(&ctx, &[[0.3; MAX_SPECIES]; 8], &[0.5; 8])
            .expect("IO");
        for dt_seconds in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(matches!(
                encode_wet_deposition_probability_gpu_with_kernel(
                    &ctx,
                    &particles,
                    &io,
                    WetDepositionStepParams { dt_seconds },
                    &kernel,
                    &mut encoder,
                ),
                Err(GpuWetDepositionError::InvalidTimeStep { .. })
            ));
        }
        // No command buffer is submitted on any invalid path.
        drop(encoder);
        eprintln!("WET-BOUNDS-138: undersized IO/all four storage buffers and nonfinite timesteps rejected before submission; adapter={:?}", ctx.adapter_info());
    }

    #[test]
    fn test_wet_deposition_workflow_runs_or_fails_explicitly() {
        let mut store = ParticleStore::with_capacity(2);
        let p0 = particle_at(0.5, 0.5, 1.0, 1.0);
        let p1 = particle_at(1.5, 1.5, 2.0, 1.0);
        store.add(p0).expect("slot 0 available");
        store.add(p1).expect("slot 1 available");

        let initial_mass: Vec<f32> = store.as_slice().iter().map(|p| p.mass[0]).collect();
        let scavenging = vec![[0.01; MAX_SPECIES], [0.02; MAX_SPECIES]];
        let precipitating_fraction = vec![0.3, 0.7];
        let result = pollster::block_on(apply_wet_deposition_step_workflow(
            &mut store,
            &scavenging,
            &precipitating_fraction,
            WetDepositionStepParams { dt_seconds: 30.0 },
        ));
        match result {
            Ok(probabilities) => assert_eq!(probabilities.len(), 2),
            Err(GpuWetDepositionWorkflowError::Gpu(GpuError::NoAdapter)) => {
                let mass_after_skip: Vec<f32> =
                    store.as_slice().iter().map(|p| p.mass[0]).collect();
                assert_eq!(mass_after_skip, initial_mass);
            }
            Err(error) => panic!("unexpected workflow error: {error}"),
        }
    }
}
