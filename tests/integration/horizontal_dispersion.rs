//! Isolated horizontal Langevin tests (RISK-03.3G-02).
//!
//! These tests check a standalone horizontal update against discrete
//! Ornstein-Uhlenbeck theory. They do not exercise the fused production
//! trajectory or establish parity with FLEXPART 11.1. See the status and
//! remaining validation work in docs/validation-report.md.

use flexpart_gpu::gpu::{
    advect_particles_gpu_with_sampling, update_particles_turbulence_langevin_gpu,
    GpuAdapterOptions, GpuContext, GpuError, ParticleBuffers, WindBuffers, WindSamplingOptions,
};
use flexpart_gpu::particles::{Particle, ParticleInit};
use flexpart_gpu::pbl::HannaParams;
use flexpart_gpu::physics::{compute_hanna_params, HannaInputs, LangevinStep, VelocityToGridScale};
use flexpart_gpu::wind::WindField3D;

const TEST_ID: &str = "RISK-03.3G-02";

const PARTICLE_COUNT: usize = 16384;
const START_X: f32 = 20.0;
const START_Y: f32 = 20.0;
const START_Z: f32 = 50.0;

const PBL_H: f32 = 1000.0;
const DT_SECONDS: f32 = 10.0;

fn release_particles() -> Vec<Particle> {
    let cell_x = START_X.floor() as i32;
    let cell_y = START_Y.floor() as i32;
    (0..PARTICLE_COUNT)
        .map(|_| {
            Particle::new(&ParticleInit {
                cell_x,
                cell_y,
                pos_x: START_X - cell_x as f32,
                pos_y: START_Y - cell_y as f32,
                pos_z: START_Z,
                mass: [1.0, 0.0, 0.0, 0.0],
                release_point: 0,
                class: 0,
                time: 0,
            })
        })
        .collect()
}

fn uniform_field(nx: usize, ny: usize, nz: usize, u: f32) -> WindField3D {
    let mut field = WindField3D::zeros(nx, ny, nz);
    field.u_ms.fill(u);
    field
}

fn gpu_context() -> Option<GpuContext> {
    match pollster::block_on(GpuContext::with_options(GpuAdapterOptions::software())) {
        Ok(context) => Some(context),
        Err(GpuError::NoAdapter) => {
            eprintln!("{TEST_ID}: no WGSL adapter found - skipping test");
            None
        }
        Err(error) => panic!("{TEST_ID}: unexpected GPU init error: {error}"),
    }
}

fn sample_variance(values: &[f64]) -> f64 {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0)
}

/// Run two advection+Langevin steps and return the x sample variance.
///
/// Legacy Langevin mode (`n_substeps = 0`) updates turb_u/v/w only: particle
/// height stays exactly at `START_Z`, so the Hanna inputs are constant and
/// the exact discrete Ornstein-Uhlenbeck variance applies.
fn two_step_variance(context: &GpuContext, hanna: HannaParams, wind_u_grid_per_s: f32) -> f64 {
    let field = uniform_field(64, 64, 8, wind_u_grid_per_s);
    let wind_buffers = WindBuffers::from_field(context, &field).expect("wind upload succeeds");
    let particle_buffers = ParticleBuffers::from_particles(context, &release_particles());
    let sampling = WindSamplingOptions {
        force_buffer_path: true,
    };
    let hanna_vec = vec![hanna; PARTICLE_COUNT];
    let step = LangevinStep {
        dt_seconds: DT_SECONDS,
        rho_grad_over_rho: 0.0,
        n_substeps: 0,
        min_height_m: 0.0,
    };
    let mut counter = [0_u32, 0_u32, 0_u32, 0_u32];
    for _ in 0..2 {
        advect_particles_gpu_with_sampling(
            context,
            &particle_buffers,
            &wind_buffers,
            DT_SECONDS,
            VelocityToGridScale::IDENTITY,
            sampling,
        )
        .expect("WGSL advection succeeds");
        counter = update_particles_turbulence_langevin_gpu(
            context,
            &particle_buffers,
            &hanna_vec,
            step,
            [0x1234_5678, 0x9ABC_DEF0],
            counter,
        )
        .expect("WGSL langevin succeeds");
    }

    let back = pollster::block_on(particle_buffers.download_particles(context))
        .expect("readback succeeds");
    let xs: Vec<f64> = back.iter().map(|p| p.grid_x()).collect();
    sample_variance(&xs)
}

#[test]
fn test_horizontal_langevin_matches_discrete_theory_across_regimes() {
    let Some(context) = gpu_context() else {
        return;
    };

    // Two turbulence intensities spanning the oracle-diagnosed (~0.34) and
    // near-laminar regimes: the kernel must match theory in both, proving
    // the spread lever is the INPUT, not the math.
    for ust in [0.01_f32, 0.35_f32] {
        let hanna: HannaParams = compute_hanna_params(HannaInputs {
            ust,
            wst: 0.0,
            ol: f32::INFINITY,
            h: PBL_H,
            z: START_Z,
        });
        let ratio = DT_SECONDS / hanna.tlu;
        // Discrete Ornstein-Uhlenbeck step variance from the Fortran update
        // (turbulence_mod.f90): 2*dt/TL below 0.5, else (1 - exp(-2*dt/TL)).
        // Step 1 advects with turb=0 (no spread); step 2 spreads with the
        // step-1 fluctuation, scaled by (dt in grid units)^2 (IDENTITY scale).
        let step_var_factor = if ratio < 0.5 {
            2.0 * ratio
        } else {
            1.0 - (-2.0 * ratio).exp()
        };
        let expected_var_x =
            f64::from(DT_SECONDS * DT_SECONDS * hanna.sigu * hanna.sigu * step_var_factor);

        let var_x = two_step_variance(&context, hanna, 0.05);
        eprintln!("{TEST_ID}: ust={ust} var_x={var_x:.6e} expected={expected_var_x:.6e}");
        let relative = (var_x - expected_var_x).abs() / expected_var_x;
        // Sample variance over 16384 draws has relative SE ~1.1%; +/-5% is a
        // statistically justified bound, not a loosened tolerance.
        assert!(
            relative < 0.05,
            "{TEST_ID}: ust={ust}: horizontal spread out of theory bounds: \
             var_x={var_x:.6e}, expected={expected_var_x:.6e}"
        );
    }
}
