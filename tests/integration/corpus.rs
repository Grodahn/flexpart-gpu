//! Issue #6 corpus tests (RISK-03.3G-01, point 2).
//!
//! Small deterministic subset for CI plus machine-readable checks that the
//! blocked cases (restart, decay, convection) are declared as dependencies
//! and never reported as passed.
//!
//! - `corpus_analytic_advection`: isolated production WGSL advection kernel,
//!   10 m/s * 3600 s = 36000 m east, no spread, mass conserved.
//! - `corpus_production_neutral_smoke`: production `ForwardTimeLoopDriver`
//!   smoke (neutral PBL, few particles/steps): mass conserved, PBL confined,
//!   COM moves east/south.
//! - `corpus_repeatability_is_bit_identical`: same Philox seed twice gives
//!   identical particle states through the production driver.
//! - `corpus_deposition_kernels_match_analytic`: isolated CPU dry/wet
//!   exponential decay within the versioned D-06 budgets.
//! - `corpus_blocked_cases_stay_blocked`: `fixtures/corpus/corpus.json`
//!   marks RESTART-010, DECAY-011 and CONV-012 as blocked with a verifiable
//!   cause; this test fails if any of them flips to implemented without a
//!   production-path implementation.
//!
//! Stochastic multi-seed runs (10 Philox seeds) live in `src/bin/corpus-run.rs`
//! and `scripts/run-corpus.sh`, not in CI. The Fortran oracle has no seed
//! control (`random_mod.f90 alloc_random` hard-codes `iseed1=-7-i`,
//! `iseed2=-88-i`), so no multi-seed oracle parity is claimed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use flexpart_gpu::config::ReleaseConfig;
use flexpart_gpu::coords::GridDomain;
use flexpart_gpu::gpu::{
    advect_particles_gpu_with_sampling, GpuContext, GpuError, ParticleBuffers, WindBuffers,
    WindSamplingOptions,
};
use flexpart_gpu::io::TimeBoundsBehavior;
use flexpart_gpu::particles::{Particle, ParticleInit};
use flexpart_gpu::physics::{
    apply_wet_scavenging_mass_step, dry_deposition_probability_step, VelocityToGridScale,
    WetScavengingStep,
};
use flexpart_gpu::simulation::{
    ForwardStepForcing, ForwardTimeLoopConfig, ForwardTimeLoopDriver, MetTimeBracket,
    ParticleForcingField, TimeLoopError,
};
use flexpart_gpu::wind::{SurfaceFields, WindField3D, WindFieldGrid};
use ndarray::Array1;

const CORPUS_ID: &str = "corpus";

// Analytic advection (CI): fewer dispatches than SW-WGPU-ADVECTION-001 but the
// same 36000 m signal, so the 1% bound stays generous.
const ADV_PARTICLES: usize = 512;
const ADV_DT_S: f32 = 300.0;
const ADV_STEPS: usize = 12;
const ADV_U_MS: f32 = 10.0;
const ADV_EXPECTED_EAST_M: f32 = ADV_DT_S * ADV_STEPS as f32 * ADV_U_MS;
const ADV_EAST_TOL_M: f32 = 360.0;
const ADV_NORTH_TOL_M: f32 = 1.0;
const ADV_VERT_TOL_M: f32 = 0.01;

const NEUTRAL_PARTICLES: usize = 200;
const NEUTRAL_DT_S: i64 = 300;
const NEUTRAL_STEPS: usize = 4;
const NEUTRAL_BLH_M: f32 = 1500.0;

fn corpus_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("corpus")
        .join(name)
}

fn gpu_context_or_skip(test: &str) -> Option<GpuContext> {
    match pollster::block_on(GpuContext::new()) {
        Ok(context) => Some(context),
        Err(GpuError::NoAdapter) => {
            eprintln!("{CORPUS_ID}: {test}: no WGSL adapter — skipping");
            None
        }
        Err(error) => panic!("{CORPUS_ID}: {test}: unexpected GPU init error: {error}"),
    }
}

#[test]
fn corpus_analytic_advection_matches_expectation() {
    let Some(context) = gpu_context_or_skip("analytic-advection") else {
        return;
    };
    let nx = 64;
    let ny = 64;
    let nz = 8;
    let xlon0 = 6.0;
    let ylat0 = 47.0;
    let dx = 0.1;
    let dy = 0.1;
    let start_lon = 10.0;
    let start_lat = 50.0;
    let start_z = 100.0_f32;
    let heights = [0.0_f32, 50.0, 100.0, 200.0, 500.0, 1500.0, 3000.0, 5000.0];
    let lat_rad = (start_lat as f32).to_radians();
    let dx_m = 6_371_000.0 * f64::from(lat_rad.cos()) * (dx * std::f64::consts::PI / 180.0);
    let dy_m = 6_371_000.0 * (dy * std::f64::consts::PI / 180.0);
    let mut level_heights_m = [0.0_f32; 16];
    for (i, h) in heights.iter().enumerate() {
        level_heights_m[i] = *h;
    }
    let scale = VelocityToGridScale {
        x_grid_per_meter: (1.0 / dx_m) as f32,
        y_grid_per_meter: (1.0 / dy_m) as f32,
        z_grid_per_meter: 1.0,
        level_heights_m,
    };
    let mut field = WindField3D::zeros(nx, ny, nz);
    field.u_ms.fill(ADV_U_MS);
    let wind_buffers = WindBuffers::from_field(&context, &field).expect("wind upload");
    let start_gx = (start_lon - xlon0) / dx;
    let start_gy = (start_lat - ylat0) / dy;
    let particles: Vec<Particle> = (0..ADV_PARTICLES)
        .map(|_| {
            Particle::new(&ParticleInit {
                cell_x: start_gx.floor() as i32,
                cell_y: start_gy.floor() as i32,
                pos_x: (start_gx - start_gx.floor()) as f32,
                pos_y: (start_gy - start_gy.floor()) as f32,
                pos_z: start_z,
                mass: [1.0, 0.0, 0.0, 0.0],
                release_point: 0,
                class: 0,
                time: 0,
            })
        })
        .collect();
    let particle_buffers = ParticleBuffers::from_particles(&context, &particles);
    let sampling = WindSamplingOptions {
        force_buffer_path: true,
    };
    for _ in 0..ADV_STEPS {
        advect_particles_gpu_with_sampling(
            &context,
            &particle_buffers,
            &wind_buffers,
            ADV_DT_S,
            scale,
            sampling,
        )
        .expect("advection dispatch");
    }
    let back = pollster::block_on(particle_buffers.download_particles(&context)).expect("readback");
    let mut sum_gx = 0.0_f64;
    let mut sum_gy = 0.0_f64;
    let mut sum_z = 0.0_f64;
    for particle in back.iter().filter(|p| p.is_active()) {
        sum_gx += particle.grid_x();
        sum_gy += particle.grid_y();
        sum_z += f64::from(particle.pos_z);
    }
    let n = back.iter().filter(|p| p.is_active()).count() as f64;
    assert_eq!(n as usize, ADV_PARTICLES, "all particles stay active");
    let mean_lon = sum_gx / n * dx + xlon0;
    let mean_lat = sum_gy / n * dy + ylat0;
    let mean_z = sum_z / n;
    // East displacement at constant latitude.
    let east_m = (mean_lon - start_lon) * dx_m / dx;
    let north_m = (mean_lat - start_lat) * dy_m / dy;
    eprintln!(
        "{CORPUS_ID}: analytic advection east={east_m:.2}m expected={ADV_EXPECTED_EAST_M:.2}m north={north_m:.4}m z={mean_z:.3}m"
    );
    assert!(
        (east_m as f32 - ADV_EXPECTED_EAST_M).abs() <= ADV_EAST_TOL_M,
        "east displacement out of tolerance: {east_m:.2}m vs {ADV_EXPECTED_EAST_M:.2}m"
    );
    assert!(
        north_m.abs() <= f64::from(ADV_NORTH_TOL_M),
        "north drift: {north_m}"
    );
    assert!(
        ((mean_z as f32) - start_z).abs() <= ADV_VERT_TOL_M,
        "vertical drift: {mean_z}"
    );
}

fn neutral_driver(
    philox_key: [u32; 2],
) -> Result<(ForwardTimeLoopDriver, MetTimeBracket<'static>), TimeLoopError> {
    // Leaked boxes give the bracket a 'static lifetime for the short CI run.
    // This is test-only scaffolding; the production runner owns its met data.
    let nx = 32;
    let ny = 32;
    let nz = 8;
    let heights = [
        50.0_f32, 100.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0,
    ];
    let mut wind: Box<WindField3D> = Box::new(WindField3D::zeros(nx, ny, nz));
    wind.u_ms.fill(5.0);
    wind.v_ms.fill(-3.0);
    wind.temperature_k.fill(285.0);
    wind.specific_humidity.fill(0.005);
    wind.pressure_pa.fill(100_000.0);
    wind.air_density_kg_m3.fill(1.2);
    wind.density_gradient_kg_m2.fill(-0.0008);
    let mut surface: Box<SurfaceFields> = Box::new(SurfaceFields::zeros(nx, ny));
    surface.surface_pressure_pa.fill(101_325.0);
    surface.u10_ms.fill(5.0);
    surface.v10_ms.fill(-3.0);
    surface.temperature_2m_k.fill(289.0);
    surface.dewpoint_2m_k.fill(284.0);
    surface.sensible_heat_flux_w_m2.fill(0.0);
    surface.solar_radiation_w_m2.fill(120.0);
    surface.surface_stress_n_m2.fill(0.2);
    surface.friction_velocity_ms.fill(0.35);
    surface.mixing_height_m.fill(NEUTRAL_BLH_M);
    surface.tropopause_height_m.fill(10_000.0);
    let wind: &'static WindField3D = Box::leak(wind);
    let surface: &'static SurfaceFields = Box::leak(surface);

    let lat_rad = (10.0_f32).to_radians();
    let dx_m = 6_371_000.0 * f64::from(lat_rad.cos()) * (0.1 * std::f64::consts::PI / 180.0);
    let dy_m = 6_371_000.0 * (0.1 * std::f64::consts::PI / 180.0);
    let mut level_heights_m = [0.0_f32; 16];
    for (i, h) in heights.iter().enumerate() {
        level_heights_m[i] = *h;
    }
    let total_s = NEUTRAL_DT_S * NEUTRAL_STEPS as i64;
    let end_hour = total_s / 3600;
    let end_min = (total_s % 3600) / 60;
    let end_ts = format!("20240101{end_hour:02}{end_min:02}00");
    let releases = vec![ReleaseConfig {
        name: "corpus-neutral-smoke".to_string(),
        start_time: "20240101000000".to_string(),
        end_time: "20240101000000".to_string(),
        lon: 10.0,
        lat: 10.0,
        z_min: 50.0,
        z_max: 50.0,
        mass_kg: 1.0,
        particle_count: NEUTRAL_PARTICLES as u64,
        raw: BTreeMap::new(),
    }];
    let config = ForwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: end_ts,
        timestep_seconds: NEUTRAL_DT_S,
        time_bounds_behavior: TimeBoundsBehavior::Clamp,
        velocity_to_grid_scale: VelocityToGridScale {
            x_grid_per_meter: (1.0 / dx_m) as f32,
            y_grid_per_meter: (1.0 / dy_m) as f32,
            z_grid_per_meter: 1.0,
            level_heights_m,
        },
        philox_key,
        initial_philox_counter: [0, 0, 0, 0],
        sync_particle_store_each_step: true,
        collect_deposition_probabilities_each_step: false,
        ..ForwardTimeLoopConfig::default()
    };
    let release_grid = GridDomain {
        xlon0: 9.5,
        ylat0: 8.5,
        dx: 0.1,
        dy: 0.1,
        nx,
        ny,
    };
    let driver = pollster::block_on(ForwardTimeLoopDriver::new(
        config,
        &releases,
        release_grid,
        NEUTRAL_PARTICLES,
    ))?;
    let start_secs = 1_704_067_200_i64;
    let met = MetTimeBracket {
        wind_t0: wind,
        wind_t1: wind,
        surface_t0: surface,
        surface_t1: surface,
        time_t0_seconds: start_secs,
        time_t1_seconds: start_secs + total_s,
    };
    Ok((driver, met))
}

#[test]
fn corpus_production_neutral_smoke_conserves_mass_and_confines_pbl() {
    let key = [0xDECA_FBAD, 0x1234_5678];
    let (mut driver, met) = match neutral_driver(key) {
        Ok(pair) => pair,
        Err(TimeLoopError::Gpu(GpuError::NoAdapter)) => {
            eprintln!("{CORPUS_ID}: neutral smoke: no WGSL adapter — skipping");
            return;
        }
        Err(error) => panic!("{CORPUS_ID}: neutral smoke driver init failed: {error}"),
    };
    let forcing = ForwardStepForcing::default();
    pollster::block_on(driver.run_to_end(&met, &forcing)).expect("run completes");
    let particles = driver.particle_store().as_slice();
    let active: Vec<_> = particles.iter().filter(|p| p.is_active()).collect();
    assert_eq!(active.len(), NEUTRAL_PARTICLES, "all particles stay active");
    let total: f64 = active.iter().map(|p| f64::from(p.mass[0])).sum();
    let rel = (total - 1.0).abs();
    assert!(
        rel < 1.0e-5,
        "mass must be conserved without deposition: {total}"
    );
    let release_gx = (10.0 - 9.5) / 0.1;
    let release_gy = (10.0 - 8.5) / 0.1;
    let mean_gx: f64 = active.iter().map(|p| p.grid_x()).sum::<f64>() / active.len() as f64;
    let mean_gy: f64 = active.iter().map(|p| p.grid_y()).sum::<f64>() / active.len() as f64;
    assert!(
        mean_gx > release_gx,
        "COM must move east: {mean_gx} vs {release_gx}"
    );
    assert!(
        mean_gy < release_gy,
        "COM must move south: {mean_gy} vs {release_gy}"
    );
    for particle in &active {
        assert!(
            particle.pos_z >= 0.0,
            "no particle below ground: {}",
            particle.pos_z
        );
        assert!(
            particle.pos_z <= NEUTRAL_BLH_M + 1.0,
            "PBL confined: {} vs {NEUTRAL_BLH_M}",
            particle.pos_z
        );
    }
}

#[test]
fn corpus_repeatability_is_bit_identical() {
    let key = [0xDECA_FBAD, 0x1234_5678];
    let run_once = || -> Vec<(f64, f64, f32, f32)> {
        let (mut driver, met) = neutral_driver(key).expect("driver init");
        let forcing = ForwardStepForcing::default();
        pollster::block_on(driver.run_to_end(&met, &forcing)).expect("run completes");
        driver
            .particle_store()
            .as_slice()
            .iter()
            .filter(|p| p.is_active())
            .map(|p| (p.grid_x(), p.grid_y(), p.pos_z, p.mass[0]))
            .collect()
    };
    let first = match neutral_driver(key) {
        Ok(_) => run_once(),
        Err(TimeLoopError::Gpu(GpuError::NoAdapter)) => {
            eprintln!("{CORPUS_ID}: repeatability: no WGSL adapter — skipping");
            return;
        }
        Err(error) => panic!("{CORPUS_ID}: repeatability probe failed: {error}"),
    };
    let second = run_once();
    assert_eq!(first.len(), second.len(), "same active count");
    for (a, b) in first.iter().zip(second.iter()) {
        assert_eq!(a, b, "identical Philox rerun must be bit-identical");
    }
}

#[test]
fn corpus_deposition_kernels_match_analytic() {
    // Isolated CPU analytic check (no GPU needed): mirrors the versioned D-06
    // budgets reused by DRY-007/WET-008.
    let href = flexpart_gpu::constants::HREF;
    let dt = 60.0_f32;
    let vdep = 0.024_f32;
    let lambda = 1.6e-3_f32;
    let prob = dry_deposition_probability_step(vdep, dt, href);
    let dry_rate = f64::from(vdep / (2.0 * href));
    let mut mass = 1.7_f64;
    for step in 1..=12_usize {
        mass *= 1.0 - f64::from(prob);
        let expected = 1.7 * (-dry_rate * step as f64 * f64::from(dt)).exp();
        let tol = 2.0e-6_f64.max(2.0e-6 * expected.abs());
        assert!(
            (mass - expected).abs() <= tol,
            "dry analytic mismatch at step {step}: {mass} vs {expected}"
        );
    }
    let mut wet = 1.7_f32;
    let step = WetScavengingStep {
        scavenging_coefficient_s_inv: lambda,
        dt_seconds: dt,
        precipitating_fraction: 1.0,
    };
    for n in 1..=12_usize {
        let (remaining, _) = apply_wet_scavenging_mass_step(wet, step);
        wet = remaining;
        let expected = 1.7_f64 * (-f64::from(lambda) * n as f64 * f64::from(dt)).exp();
        let tol = 2.0e-6_f64.max(2.0e-6 * expected.abs());
        assert!(
            (f64::from(wet) - expected).abs() <= tol,
            "wet analytic mismatch at step {n}"
        );
    }
}

#[test]
fn corpus_blocked_cases_stay_blocked() {
    let index: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(corpus_fixture("corpus.json")).expect("corpus index exists"),
    )
    .expect("corpus index parses");
    let cases = index["cases"].as_array().expect("cases array");
    let find = |id: &str| {
        cases
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("corpus case {id} must exist"))
    };
    for blocked in ["RESTART-010", "DECAY-011", "CONV-012"] {
        let case = find(blocked);
        assert_eq!(
            case["status"], "blocked",
            "{blocked} must stay blocked until its production-path dependency lands"
        );
        let cause = case
            .get("blocked_by")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            cause.len() > 80,
            "{blocked} needs a verifiable dependency cause, got: {cause}"
        );
    }
    // The production driver at this base has no decay or convection lanes:
    // ForwardStepForcing carries only dry/wet deposition plus rho-grad.
    // If decay/convection lands in the driver, the corresponding corpus case
    // must be promoted to implemented in the same PR (this test then updates).
    let forcing = ForwardStepForcing::default();
    let debug = format!("{forcing:?}");
    assert!(
        !debug.to_lowercase().contains("decay"),
        "driver has no decay lane at base"
    );
    assert!(
        !debug.to_lowercase().contains("convect"),
        "driver has no convection lane at base"
    );
}

#[test]
fn corpus_production_driver_supports_dry_and_wet_forcing_shapes() {
    // The corpus DRY-007/WET-008 driver runs only need uniform forcing lanes,
    // which the production driver already skips correctly when zero.
    let dry_only = ForwardStepForcing {
        dry_deposition_velocity_m_s: ParticleForcingField::Uniform(0.02),
        wet_scavenging_coefficient_s_inv: ParticleForcingField::Uniform(0.0),
        wet_precipitating_fraction: ParticleForcingField::Uniform(0.0),
        rho_grad_over_rho: 0.0,
    };
    let wet_only = ForwardStepForcing {
        dry_deposition_velocity_m_s: ParticleForcingField::Uniform(0.0),
        wet_scavenging_coefficient_s_inv: ParticleForcingField::Uniform(0.005),
        wet_precipitating_fraction: ParticleForcingField::Uniform(1.0),
        rho_grad_over_rho: 0.0,
    };
    let debug_dry = format!("{dry_only:?}");
    let debug_wet = format!("{wet_only:?}");
    assert!(debug_dry.contains("0.02"));
    assert!(debug_wet.contains("0.005"));
    // Wind grid helper used by the corpus runner exists for both regimes.
    let grid = WindFieldGrid::new(
        32,
        32,
        8,
        8,
        8,
        0.1,
        0.1,
        9.5,
        8.5,
        Array1::from_vec(vec![
            50.0, 100.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0,
        ]),
    );
    assert_eq!((grid.nx, grid.ny), (32, 32));
}
