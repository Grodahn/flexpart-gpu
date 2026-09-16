//! Per-species deposition and decay validation for multi-nuclide runs (#126).
//!
//! Covers the oracle trio at the process level:
//! - passive tracer (`SPECIES_024`-like): no deposition, no decay;
//! - aerosol (`SPECIES_040`-like): per-species wet scavenging efficiencies;
//! - radionuclide (`SPECIES_021` Xe-133-like): radioactive decay.
//!
//! GPU tests need an adapter (software WARP via `FLEXPART_GPU_SOFTWARE=1`)
//! and run single-threaded (`--test-threads=1`): parallel full-suite runs are
//! unstable on WARP. Tests without an adapter skip gracefully.
//!
//! None of these tests derive PBL state: infinite uniform wind plus explicit
//! surface/PBL prescription (see `physics_validation`) keeps them independent
//! of heat-flux sign conventions.

use std::collections::BTreeMap;

use flexpart_gpu::config::ReleaseConfig;
use flexpart_gpu::config::SpeciesConfig;
use flexpart_gpu::constants::HREF;
use flexpart_gpu::coords::GridDomain;
use flexpart_gpu::gpu::{
    apply_decay_step_gpu, apply_dry_deposition_step_gpu, apply_wet_deposition_step_gpu,
    DecayStepParams, DryDepositionStepParams, GpuContext, GpuError, ParticleBuffers,
    WetDepositionStepParams,
};
use flexpart_gpu::io::TimeBoundsBehavior;
use flexpart_gpu::particles::{Particle, ParticleInit, MAX_SPECIES};
use flexpart_gpu::physics::{
    apply_decay_mass_step, apply_wet_scavenging_mass_step, decay_survival_factor,
    dry_deposition_probability_step, species_decay_constants, VelocityToGridScale,
    WetScavengingStep,
};
use flexpart_gpu::simulation::{
    ForwardStepForcing, ForwardTimeLoopConfig, ForwardTimeLoopDriver, MetTimeBracket,
    ParticleForcingField, TimeLoopError,
};
use flexpart_gpu::wind::{SurfaceFields, WindField3D, WindFieldGrid};

const STEP_COUNT: usize = 12;
const DT_SECONDS: f32 = 60.0;
const PARTICLE_HEIGHT_M: f32 = 5.0;

// Error budgets match `deposition_decay.rs` (not loosened for #126):
// - CPU vs f64 analytical: <= 2e-6 absolute/relative.
// - GPU vs analytical (adapter `exp` rounding): <= 2e-5 absolute/relative.
const CPU_ABS_TOLERANCE: f64 = 2.0e-6;
const CPU_REL_TOLERANCE: f64 = 2.0e-6;
const GPU_ABS_TOLERANCE: f64 = 2.0e-5;
const GPU_REL_TOLERANCE: f64 = 2.0e-5;

fn assert_with_tolerance(label: &str, simulated: f64, analytical: f64, abs_tol: f64, rel_tol: f64) {
    let tolerance = abs_tol.max(rel_tol * analytical.abs());
    let error = (simulated - analytical).abs();
    assert!(
        error <= tolerance,
        "{label} mismatch: simulated={simulated:.9e}, analytical={analytical:.9e}, error={error:.9e}, tolerance={tolerance:.9e}"
    );
}

fn make_particle(mass: [f32; MAX_SPECIES], pos_z: f32) -> Particle {
    Particle::new(&ParticleInit {
        cell_x: 2,
        cell_y: 3,
        pos_x: 0.25,
        pos_y: 0.75,
        pos_z,
        mass,
        release_point: 0,
        class: 0,
        time: 0,
    })
}

/// Oracle-trio species set for process-level tests:
/// slot 0 = Xe-133-like radionuclide (`SPECIES_021`, half-life 453168 s),
/// slot 1 = stable passive tracer (`SPECIES_024`-like),
/// slot 2 = short-lived nuclide, slot 3 = stable.
///
/// Oracle half-life conversion uses the truncated `0.693147` factor for
/// Fortran parity (see `readoptions_mod.f90:2328`).
#[allow(clippy::approx_constant)]
fn trio_species_configs() -> Vec<SpeciesConfig> {
    let half_life = 453_168.0_f64;
    let decaying = SpeciesConfig {
        name: "Xe-133-like".to_string(),
        molecular_weight: None,
        dry_deposition_velocity: None,
        decay_constant: Some(0.693_147 / half_life),
        half_life_s: Some(half_life),
        wet_a_gas: None,
        wet_b_gas: None,
        crain_aero: None,
        csnow_aero: None,
        ccn_aero: None,
        in_aero: None,
        relative_diffusivity: None,
        henry: None,
        surface_reactivity_f0: None,
        particle_density_kg_m3: None,
        mean_diameter_um: None,
        diameter_sigma: None,
        source_file: None,
        raw: BTreeMap::new(),
    };
    let stable = SpeciesConfig {
        name: "stable-tracer".to_string(),
        decay_constant: None,
        half_life_s: None,
        ..decaying.clone()
    };
    let short_lived = SpeciesConfig {
        name: "short-lived".to_string(),
        decay_constant: Some(0.05),
        half_life_s: Some(0.693_147 / 0.05),
        ..decaying.clone()
    };
    vec![decaying, stable.clone(), short_lived, stable]
}

#[test]
fn species_decay_cpu_matches_analytical_per_species() {
    let configs = trio_species_configs();
    let lambdas = species_decay_constants(&configs);
    let initial = [2.0_f32, 3.0, 1.5, 0.5];
    let mut masses = initial;
    for step in 1..=STEP_COUNT {
        for s in 0..MAX_SPECIES {
            let (remaining, _) = apply_decay_mass_step(masses[s], lambdas[s], DT_SECONDS);
            masses[s] = remaining;
        }
        for s in 0..MAX_SPECIES {
            let expected = f64::from(initial[s])
                * (-f64::from(lambdas[s]) * f64::from(DT_SECONDS) * step as f64).exp();
            assert_with_tolerance(
                "cpu per-species decay",
                f64::from(masses[s]),
                expected,
                CPU_ABS_TOLERANCE,
                CPU_REL_TOLERANCE,
            );
        }
    }

    // Stable slot never loses mass; decaying slots decrease monotonically.
    assert_eq!(masses[1], initial[1]);
    assert!(masses[0] < initial[0]);
    assert!(masses[2] < initial[2]);
}

#[test]
fn species_dry_deposition_gpu_per_species_matches_cpu_when_adapter_available() {
    let ctx = match pollster::block_on(GpuContext::new()) {
        Ok(ctx) => ctx,
        Err(GpuError::NoAdapter) => {
            eprintln!("No GPU adapter found — skipping per-species dry deposition GPU test");
            return;
        }
        Err(err) => panic!("unexpected GPU init error: {err}"),
    };

    // Distinct velocities per lane, including a stable zero lane.
    let lanes = [0.024, 0.0, 0.048, 0.012];
    let particles = vec![make_particle([1.0, 1.0, 1.0, 1.0], PARTICLE_HEIGHT_M)];
    let particle_buffers = ParticleBuffers::from_particles(&ctx, &particles);
    let velocities = vec![lanes];
    let params = DryDepositionStepParams {
        dt_seconds: DT_SECONDS,
        reference_height_m: HREF,
    };

    let probabilities = pollster::block_on(apply_dry_deposition_step_gpu(
        &ctx,
        &particle_buffers,
        &velocities,
        params,
    ))
    .expect("per-species dry deposition GPU step should succeed");

    assert_eq!(probabilities.len(), 1);
    for s in 0..MAX_SPECIES {
        let expected = dry_deposition_probability_step(lanes[s], DT_SECONDS, HREF);
        assert_with_tolerance(
            "gpu per-species dry probability",
            f64::from(probabilities[0][s]),
            f64::from(expected),
            GPU_ABS_TOLERANCE,
            GPU_REL_TOLERANCE,
        );
    }
    // Zero-velocity lane leaves mass untouched while others decay.
    let updated = pollster::block_on(particle_buffers.download_particles(&ctx))
        .expect("particle readback should succeed");
    assert_eq!(updated[0].mass[1], 1.0);
    assert!(updated[0].mass[0] < 1.0);
    assert!(updated[0].mass[2] < updated[0].mass[0]);
}

#[test]
fn species_wet_deposition_gpu_per_species_matches_cpu_when_adapter_available() {
    let ctx = match pollster::block_on(GpuContext::new()) {
        Ok(ctx) => ctx,
        Err(GpuError::NoAdapter) => {
            eprintln!("No GPU adapter found — skipping per-species wet deposition GPU test");
            return;
        }
        Err(err) => panic!("unexpected GPU init error: {err}"),
    };

    // Aerosol-like lanes: strong rain scavenging, disabled lane, weak lane.
    let lanes = [1.6e-3, 0.0, 3.2e-3, 4.0e-4];
    let fraction = 0.6;
    let particles = vec![make_particle([1.0, 2.0, 4.0, 8.0], PARTICLE_HEIGHT_M)];
    let particle_buffers = ParticleBuffers::from_particles(&ctx, &particles);

    let probabilities = pollster::block_on(apply_wet_deposition_step_gpu(
        &ctx,
        &particle_buffers,
        &[lanes],
        &[fraction],
        WetDepositionStepParams {
            dt_seconds: DT_SECONDS,
        },
    ))
    .expect("per-species wet deposition GPU step should succeed");

    assert_eq!(probabilities.len(), 1);
    for s in 0..MAX_SPECIES {
        let (expected_remaining, _) = apply_wet_scavenging_mass_step(
            particles[0].mass[s],
            WetScavengingStep {
                scavenging_coefficient_s_inv: lanes[s],
                dt_seconds: DT_SECONDS,
                precipitating_fraction: fraction,
            },
        );
        let expected_prob = 1.0 - expected_remaining / particles[0].mass[s];
        assert_with_tolerance(
            "gpu per-species wet probability",
            f64::from(probabilities[0][s]),
            f64::from(expected_prob),
            GPU_ABS_TOLERANCE,
            GPU_REL_TOLERANCE,
        );
    }
}

#[test]
fn species_decay_gpu_matches_cpu_when_adapter_available() {
    let ctx = match pollster::block_on(GpuContext::new()) {
        Ok(ctx) => ctx,
        Err(GpuError::NoAdapter) => {
            eprintln!("No GPU adapter found — skipping per-species decay GPU test");
            return;
        }
        Err(err) => panic!("unexpected GPU init error: {err}"),
    };

    let configs = trio_species_configs();
    let lambdas = species_decay_constants(&configs);
    let initial = [1.0_f32, 2.0, 4.0, 8.0];
    let particles = vec![make_particle(initial, PARTICLE_HEIGHT_M)];
    let particle_buffers = ParticleBuffers::from_particles(&ctx, &particles);

    pollster::block_on(apply_decay_step_gpu(
        &ctx,
        &particle_buffers,
        DecayStepParams {
            dt_seconds: DT_SECONDS,
            decay_constants_s_inv: lambdas,
        },
    ))
    .expect("per-species decay GPU step should succeed");

    let updated = pollster::block_on(particle_buffers.download_particles(&ctx))
        .expect("particle readback should succeed");
    for s in 0..MAX_SPECIES {
        let expected = initial[s] * decay_survival_factor(lambdas[s], DT_SECONDS);
        assert_with_tolerance(
            "gpu per-species decay mass",
            f64::from(updated[0].mass[s]),
            f64::from(expected),
            GPU_ABS_TOLERANCE,
            GPU_REL_TOLERANCE,
        );
    }
}

// ---------------------------------------------------------------------------
// Timeloop test with explicit PBL prescription (independent of #124).
// ---------------------------------------------------------------------------

const TL_NX: usize = 16;
const TL_NY: usize = 16;
const TL_PARTICLES: u64 = 8;
const TL_STEPS: usize = 4;
const TL_DT: i64 = 60;
const TL_WET_LANES: [f32; MAX_SPECIES] = [1.6e-3, 0.0, 0.0, 0.0];
const TL_DECAY_LANES: [f32; MAX_SPECIES] = [0.01, 0.0, 0.0, 0.0];

fn tl_wind_grid() -> WindFieldGrid {
    WindFieldGrid::new(
        TL_NX,
        TL_NY,
        2,
        2,
        2,
        0.1,
        0.1,
        6.0,
        6.0,
        ndarray::Array1::from_vec(vec![0.0, 10_000.0]),
    )
}

fn tl_uniform_wind(grid: &WindFieldGrid) -> WindField3D {
    let mut field = WindField3D::zeros(grid.nx, grid.ny, grid.nz);
    field.u_ms.fill(1.0);
    field.v_ms.fill(0.0);
    field.w_ms.fill(0.0);
    field
}

/// Explicit surface/PBL prescription: heat-flux sign convention is fixed here
/// (positive-downward oracle/ECMWF value) instead of derived, so this test
/// stays independent of the PBL-vertical-parity work.
fn tl_surface_fields() -> SurfaceFields {
    let mut s = SurfaceFields::zeros(TL_NX, TL_NY);
    s.surface_pressure_pa.fill(101_325.0);
    s.u10_ms.fill(1.0);
    s.v10_ms.fill(0.0);
    s.temperature_2m_k.fill(289.0);
    s.dewpoint_2m_k.fill(284.0);
    s.sensible_heat_flux_w_m2.fill(40.0);
    s.solar_radiation_w_m2.fill(220.0);
    s.surface_stress_n_m2.fill(0.2);
    s.friction_velocity_ms.fill(0.35);
    s.convective_velocity_scale_ms.fill(0.1);
    s.mixing_height_m.fill(1500.0);
    s.tropopause_height_m.fill(10_000.0);
    s.inv_obukhov_length_per_m.fill(0.0);
    s
}

#[test]
fn timeloop_applies_per_species_wet_and_decay_with_explicit_pbl() {
    let release_grid = GridDomain {
        xlon0: 6.0,
        ylat0: 6.0,
        dx: 0.1,
        dy: 0.1,
        nx: TL_NX,
        ny: TL_NY,
    };
    // Two-species release: 60% species 0, 40% species 1.
    let releases = vec![ReleaseConfig {
        name: "two-species".to_string(),
        start_time: "20240101000000".to_string(),
        end_time: "20240101000000".to_string(),
        lon: 6.7,
        lat: 6.7,
        z_min: 50.0,
        z_max: 50.0,
        mass_kg: 1.0,
        particle_count: TL_PARTICLES,
        species_masses_kg: Some(vec![0.6, 0.4]),
        raw: BTreeMap::new(),
    }];
    let config = ForwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: "20240101000400".to_string(),
        timestep_seconds: TL_DT,
        time_bounds_behavior: TimeBoundsBehavior::Clamp,
        velocity_to_grid_scale: VelocityToGridScale::IDENTITY,
        ..ForwardTimeLoopConfig::default()
    };
    let mut driver = match pollster::block_on(ForwardTimeLoopDriver::new(
        config,
        &releases,
        release_grid,
        TL_PARTICLES as usize,
    )) {
        Ok(driver) => driver,
        Err(TimeLoopError::Gpu(GpuError::NoAdapter)) => {
            eprintln!("No GPU adapter found — skipping per-species timeloop test");
            return;
        }
        Err(err) => panic!("driver initialization failed: {err}"),
    };

    let grid = tl_wind_grid();
    let wind = tl_uniform_wind(&grid);
    let surface = tl_surface_fields();
    let met = MetTimeBracket {
        wind_t0: &wind,
        wind_t1: &wind,
        surface_t0: &surface,
        surface_t1: &surface,
        time_t0_seconds: driver.current_time_seconds(),
        time_t1_seconds: driver.current_time_seconds() + 600,
    };

    // Dry deposition disabled (zero lanes); wet + decay act on every particle
    // regardless of position, so per-slot masses follow closed forms exactly.
    let forcing = ForwardStepForcing {
        dry_deposition_velocity_m_s: vec![ParticleForcingField::Uniform(0.0)],
        wet_scavenging_coefficient_s_inv: vec![ParticleForcingField::Uniform(TL_WET_LANES[0])],
        wet_precipitating_fraction: ParticleForcingField::Uniform(1.0),
        decay_constant_s_inv: vec![TL_DECAY_LANES[0]],
        rho_grad_over_rho: 0.0,
    };

    let reports = pollster::block_on(driver.run_to_end(&met, &forcing))
        .expect("per-species timeloop run should succeed");
    assert_eq!(reports.len(), TL_STEPS + 1);

    // Closed form per slot: wet survival and decay survival commute.
    let wet_survival = (-f64::from(TL_WET_LANES[0]) * TL_DT as f64).exp();
    let decay_survival = (-f64::from(TL_DECAY_LANES[0]) * TL_DT as f64).exp();
    let combined = wet_survival * decay_survival;
    let steps = f64::from(reports.len() as u32);

    let mut slot_mass = [0.0_f64; MAX_SPECIES];
    for particle in driver.particle_store().as_slice() {
        if particle.is_active() {
            for (mass_slot, slot_total) in particle.mass.iter().zip(slot_mass.iter_mut()) {
                *slot_total += f64::from(*mass_slot);
            }
        }
    }
    // Release totals: 0.6 kg slot 0, 0.4 kg slot 1; slots 2-3 stay zero.
    assert_with_tolerance(
        "timeloop species-0 mass",
        slot_mass[0],
        0.6 * combined.powf(steps),
        GPU_ABS_TOLERANCE,
        GPU_REL_TOLERANCE,
    );
    assert_with_tolerance(
        "timeloop species-1 mass",
        slot_mass[1],
        0.4,
        GPU_ABS_TOLERANCE,
        GPU_REL_TOLERANCE,
    );
    assert_eq!(slot_mass[2], 0.0);
    assert_eq!(slot_mass[3], 0.0);

    // Probability reports carry per-species lanes for every slot.
    let capacity = TL_PARTICLES as usize;
    for report in &reports {
        assert_eq!(report.dry_deposition_probability.len(), 0);
        assert_eq!(report.wet_deposition_probability.len(), capacity);
        for lanes in &report.wet_deposition_probability {
            assert!(lanes[0] > 0.0, "species-0 wet probability must be positive");
            assert_eq!(lanes[1], 0.0);
        }
    }
}
