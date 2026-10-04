use std::collections::BTreeMap;

use flexpart_gpu::config::ReleaseConfig;
use flexpart_gpu::coords::GridDomain;
use flexpart_gpu::gpu::GpuError;
use flexpart_gpu::io::TimeBoundsBehavior;
use flexpart_gpu::particles::{
    ParticleSpatialSortOptions, SpatialSortBounds, SpatialSortBoundsMode,
};
use flexpart_gpu::physics::VelocityToGridScale;
use flexpart_gpu::simulation::{
    ForwardSpatialSortConfig, ForwardStepForcing, ForwardTimeLoopConfig, ForwardTimeLoopDriver,
    MetTimeBracket, TimeLoopError,
};
use flexpart_gpu::wind::{uniform_wind_field, SurfaceFields, WindFieldGrid};
use ndarray::Array1;

// WARP can crash when several forward tests create devices concurrently.
static GPU_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_gpu_tests() -> std::sync::MutexGuard<'static, ()> {
    // Preserve the original failure if another test panics while holding the lock.
    GPU_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn synthetic_wind_grid(nx: usize, ny: usize, nz: usize) -> WindFieldGrid {
    WindFieldGrid::new(
        nx,
        ny,
        nz,
        nz,
        nz,
        1.0,
        1.0,
        0.0,
        0.0,
        Array1::from_iter((0..nz).map(|k| k as f32)),
    )
}

fn synthetic_surface_fields(nx: usize, ny: usize) -> SurfaceFields {
    let mut surface = SurfaceFields::zeros(nx, ny);
    surface.surface_pressure_pa.fill(101_325.0);
    surface.u10_ms.fill(2.0);
    surface.v10_ms.fill(0.0);
    surface.temperature_2m_k.fill(290.0);
    surface.dewpoint_2m_k.fill(285.0);
    surface.precip_large_scale_mm_h.fill(0.0);
    surface.precip_convective_mm_h.fill(0.0);
    surface.sensible_heat_flux_w_m2.fill(0.0);
    surface.solar_radiation_w_m2.fill(0.0);
    surface.surface_stress_n_m2.fill(0.0);
    surface.friction_velocity_ms.fill(0.0);
    surface.convective_velocity_scale_ms.fill(0.0);
    surface.mixing_height_m.fill(1_000.0);
    surface.tropopause_height_m.fill(10_000.0);
    surface.inv_obukhov_length_per_m.fill(0.0);
    surface
}

#[test]
fn test_forward_timeloop_synthetic_uniform_wind_is_deterministic() {
    let _gpu_test_guard = lock_gpu_tests();
    let config = ForwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: "20240101000002".to_string(),
        timestep_seconds: 1,
        time_bounds_behavior: TimeBoundsBehavior::Strict,
        velocity_to_grid_scale: VelocityToGridScale::IDENTITY,
        ..ForwardTimeLoopConfig::default()
    };

    let releases = vec![ReleaseConfig {
        name: "synthetic_point".to_string(),
        start_time: "20240101000000".to_string(),
        end_time: "20240101000000".to_string(),
        lon: 10.0,
        lat: 5.0,
        z_min: 1.0,
        z_max: 1.0,
        mass_kg: 1.0,
        particle_count: 1,
        species_masses_kg: None,
        raw: BTreeMap::new(),
    }];
    let release_grid = GridDomain {
        xlon0: 0.0,
        ylat0: 0.0,
        dx: 1.0,
        dy: 1.0,
        nx: 64,
        ny: 64,
    };

    let mut driver = match pollster::block_on(ForwardTimeLoopDriver::new(
        config,
        &releases,
        release_grid,
        8,
    )) {
        Ok(driver) => driver,
        Err(TimeLoopError::Gpu(GpuError::NoAdapter)) => return,
        Err(err) => panic!("driver initialization failed: {err}"),
    };

    let wind_grid = synthetic_wind_grid(64, 64, 16);
    let wind_t0 = uniform_wind_field(&wind_grid, 1.0, 0.0, 0.0);
    let wind_t1 = uniform_wind_field(&wind_grid, 3.0, 0.0, 0.0);
    let surface_t0 = synthetic_surface_fields(64, 64);
    let surface_t1 = synthetic_surface_fields(64, 64);
    let met = MetTimeBracket {
        wind_t0: &wind_t0,
        wind_t1: &wind_t1,
        surface_t0: &surface_t0,
        surface_t1: &surface_t1,
        time_t0_seconds: driver.current_time_seconds(),
        time_t1_seconds: driver.current_time_seconds() + 2,
    };

    let forcing = ForwardStepForcing::default();
    let reports =
        pollster::block_on(driver.run_to_end(&met, &forcing)).expect("timeloop run should succeed");

    assert_eq!(reports.len(), 3);
    assert_eq!(reports[0].released_count, 1);
    assert_eq!(reports[1].released_count, 0);
    assert_eq!(reports[2].released_count, 0);

    let expected_alpha = [0.0_f32, 0.5_f32, 1.0_f32];
    for (report, expected) in reports.iter().zip(expected_alpha) {
        assert!(
            (report.interpolation_alpha - expected).abs() < 1.0e-6,
            "unexpected interpolation alpha: got {}, expected {expected}",
            report.interpolation_alpha
        );
        assert!(
            report
                .dry_deposition_probability
                .iter()
                .all(|p| p.iter().all(|v| v.abs() < 1.0e-8)),
            "dry deposition should be zero for zero forcing"
        );
        assert!(
            report
                .wet_deposition_probability
                .iter()
                .all(|p| p.iter().all(|v| v.abs() < 1.0e-8)),
            "wet deposition should be zero for zero forcing"
        );
    }

    let released_slot = reports[0]
        .released_slots
        .first()
        .copied()
        .expect("one slot should be released in first step");
    let particle = driver
        .particle_store()
        .get(released_slot)
        .expect("released slot should still exist");
    assert!(particle.is_active());

    // Particle released at x=10 with u=[1..3] over 3 steps.
    // Pure advection yields x≈16, but Langevin turbulence adds stochastic
    // displacement. We verify the particle moved substantially in +x and
    // stayed near the initial y/z within turbulence bounds.
    assert!(
        particle.grid_x() > 12.0 && particle.grid_x() < 20.0,
        "particle should drift in +x direction: grid_x={}",
        particle.grid_x()
    );
    assert!(
        (particle.grid_y() - 5.0).abs() < 2.0,
        "y should stay near initial: grid_y={}",
        particle.grid_y()
    );
    assert!(
        particle.pos_z >= 0.0 && particle.pos_z < 16.0,
        "z should stay in domain: pos_z={}",
        particle.pos_z
    );
    assert!((particle.mass[0] - 1.0).abs() < 1.0e-6);
}

#[test]
fn test_forward_timeloop_optional_spatial_sort_reorders_particle_slots() {
    let _gpu_test_guard = lock_gpu_tests();
    let config = ForwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: "20240101000001".to_string(),
        timestep_seconds: 1,
        time_bounds_behavior: TimeBoundsBehavior::Strict,
        velocity_to_grid_scale: VelocityToGridScale::IDENTITY,
        spatial_sort: Some(ForwardSpatialSortConfig {
            interval_steps: 1,
            sort_options: ParticleSpatialSortOptions {
                bits_per_axis: 10,
                bounds_mode: SpatialSortBoundsMode::Explicit(SpatialSortBounds {
                    x_min: 0.0,
                    x_max: 64.0,
                    y_min: 0.0,
                    y_max: 64.0,
                    z_min: 0.0,
                    z_max: 1000.0,
                }),
                include_inverse_map: false,
            },
        }),
        ..ForwardTimeLoopConfig::default()
    };

    let releases = vec![
        ReleaseConfig {
            name: "high_x".to_string(),
            start_time: "20240101000000".to_string(),
            end_time: "20240101000000".to_string(),
            lon: 30.0,
            lat: 5.0,
            z_min: 1.0,
            z_max: 1.0,
            mass_kg: 1.0,
            particle_count: 1,
            species_masses_kg: None,
            raw: BTreeMap::new(),
        },
        ReleaseConfig {
            name: "low_x".to_string(),
            start_time: "20240101000000".to_string(),
            end_time: "20240101000000".to_string(),
            lon: 10.0,
            lat: 5.0,
            z_min: 1.0,
            z_max: 1.0,
            mass_kg: 1.0,
            particle_count: 1,
            species_masses_kg: None,
            raw: BTreeMap::new(),
        },
    ];
    let release_grid = GridDomain {
        xlon0: 0.0,
        ylat0: 0.0,
        dx: 1.0,
        dy: 1.0,
        nx: 64,
        ny: 64,
    };

    let mut driver = match pollster::block_on(ForwardTimeLoopDriver::new(
        config,
        &releases,
        release_grid,
        8,
    )) {
        Ok(driver) => driver,
        Err(TimeLoopError::Gpu(GpuError::NoAdapter)) => return,
        Err(err) => panic!("driver initialization failed: {err}"),
    };

    let wind_grid = synthetic_wind_grid(64, 64, 16);
    let wind_t0 = uniform_wind_field(&wind_grid, 0.0, 0.0, 0.0);
    let wind_t1 = uniform_wind_field(&wind_grid, 0.0, 0.0, 0.0);
    let surface_t0 = synthetic_surface_fields(64, 64);
    let surface_t1 = synthetic_surface_fields(64, 64);
    let met = MetTimeBracket {
        wind_t0: &wind_t0,
        wind_t1: &wind_t1,
        surface_t0: &surface_t0,
        surface_t1: &surface_t1,
        time_t0_seconds: driver.current_time_seconds(),
        time_t1_seconds: driver.current_time_seconds() + 1,
    };

    let forcing = ForwardStepForcing::default();
    let reports =
        pollster::block_on(driver.run_to_end(&met, &forcing)).expect("timeloop run should succeed");
    assert_eq!(reports.len(), 2);

    let slot0 = driver.particle_store().get(0).expect("slot 0 exists");
    let slot1 = driver.particle_store().get(1).expect("slot 1 exists");
    assert_eq!(
        slot0.release_point, 1,
        "spatial sort should place low-x particle first"
    );
    assert_eq!(slot1.release_point, 0);
}

#[test]
fn test_forward_timeloop_operator_call_order_is_preserved() {
    // Source order covers commuting mass operators that final mass alone cannot distinguish.
    let source = include_str!("../src/simulation/timeloop/forward/operators.rs");
    let start = source
        .find("label: Some(\"forward_timeloop_step_encoder\")")
        .expect("forward encoder");
    let source = &source[start..];
    let mut offset = 0;
    for operation in [
        "encode_pbl_diagnostics_gpu_with_kernel(",
        "encode_advection_dual_wind_gpu_with_kernel(",
        "encode_langevin_fused_gpu(",
        "encode_hanna_params_gpu_with_kernel(",
        "encode_update_particles_turbulence_langevin_gpu_with_hanna_buffer_and_kernel(",
        "encode_dry_deposition_probability_gpu_with_kernel(",
        "encode_wet_deposition_probability_gpu_with_kernel(",
        "encode_decay_gpu_with_kernel(",
        "encode_compaction_with_reorder(",
        "self.gpu_context.queue.submit(",
    ] {
        assert_eq!(
            source.matches(operation).count(),
            1,
            "operator {operation} must occur exactly once in its preserved branch"
        );
        offset += source[offset..]
            .find(operation)
            .unwrap_or_else(|| panic!("missing/out-of-order {operation}"))
            + operation.len();
    }
    let coordinator = include_str!("../src/simulation/timeloop/forward/timestep.rs");
    let mut offset = 0;
    for boundary in [
        "self.apply_spatial_sort_if_enabled()",
        "self.release_manager.inject_and_upload_for_time(",
        "self.prepare_meteorology(",
        "self.prepare_forcing(",
        "self.gpu_context.device.poll(wgpu::Maintain::Wait)",
        "self.submit_operators(",
        "self.gpu_submission_pending = true",
        "self.pbl_write_index = 1 - self.pbl_write_index",
        "self.philox_counter = next_philox_counter",
        "let dry_probability =",
        ".download_probabilities(",
        "let wet_probability =",
        ".download_probabilities(",
        ".download_active_count(",
        "self.sync_store_from_gpu().await?",
        "let report = ForwardStepReport",
        "self.step_index += 1",
        ".saturating_add(",
    ] {
        offset += coordinator[offset..]
            .find(boundary)
            .unwrap_or_else(|| panic!("missing/out-of-order timestep boundary {boundary}"))
            + boundary.len();
    }
}

#[test]
fn test_forward_timeloop_transport_precedes_deposition_and_reports_precede_advance() {
    let _gpu_test_guard = lock_gpu_tests();
    use flexpart_gpu::simulation::ParticleForcingField;

    let releases = vec![ReleaseConfig {
        name: "order_probe".to_string(),
        start_time: "20240101000000".to_string(),
        end_time: "20240101000000".to_string(),
        lon: 10.0,
        lat: 5.0,
        z_min: 1.0,
        z_max: 1.0,
        mass_kg: 1.0,
        particle_count: 1,
        species_masses_kg: None,
        raw: BTreeMap::new(),
    }];
    let grid = GridDomain {
        xlon0: 0.0,
        ylat0: 0.0,
        dx: 1.0,
        dy: 1.0,
        nx: 64,
        ny: 64,
    };
    let config = ForwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: "20240101000001".to_string(),
        dry_reference_height_m: 2.0,
        time_bounds_behavior: TimeBoundsBehavior::Strict,
        ..ForwardTimeLoopConfig::default()
    };
    // Required regression: missing device execution must fail, never become a successful skip.
    let mut driver = pollster::block_on(ForwardTimeLoopDriver::new(config, &releases, grid, 8))
        .expect("order regression requires a WGSL adapter");
    let start = driver.current_time_seconds();
    let wind_grid = synthetic_wind_grid(64, 64, 64);
    let still = uniform_wind_field(&wind_grid, 0.0, 0.0, 0.0);
    let upward = uniform_wind_field(&wind_grid, 0.0, 0.0, 10.0);
    let surface = synthetic_surface_fields(64, 64);
    let forcing = ForwardStepForcing {
        dry_deposition_velocity_m_s: vec![ParticleForcingField::Uniform(0.8)],
        wet_scavenging_coefficient_s_inv: vec![ParticleForcingField::Uniform(0.3)],
        wet_precipitating_fraction: ParticleForcingField::Uniform(0.5),
        decay_constant_s_inv: vec![0.1],
        rho_grad_over_rho: 0.0,
    };
    let mut expected_mass = 1.0_f32;
    for (index, wind) in [&still, &upward].into_iter().enumerate() {
        let met = MetTimeBracket {
            wind_t0: wind,
            wind_t1: wind,
            surface_t0: &surface,
            surface_t1: &surface,
            time_t0_seconds: start + index as i64,
            time_t1_seconds: start + index as i64 + 1,
        };
        let report = pollster::block_on(driver.run_timestep(&met, &forcing)).expect("GPU step");
        assert_eq!(report.step_index, index);
        assert_eq!(report.simulation_time_seconds, start + index as i64);
        assert_eq!(report.timestamp, format!("2024010100000{index}"));
        assert_eq!(report.released_count, usize::from(index == 0));
        assert_eq!(driver.current_time_seconds(), start + index as i64 + 1);
        let dry_probability = if index == 0 {
            1.0 - (-0.2_f32).exp()
        } else {
            0.0
        };
        let wet_probability = 0.5 * (1.0 - (-0.3_f32).exp());
        assert!((report.dry_deposition_probability[0][0] - dry_probability).abs() < 1.0e-5);
        assert!((report.wet_deposition_probability[0][0] - wet_probability).abs() < 1.0e-5);
        expected_mass *= (1.0 - dry_probability) * (1.0 - wet_probability) * (-0.1_f32).exp();
        let particle = driver.particle_store().get(0).expect("released particle");
        assert!((particle.mass[0] - expected_mass).abs() < 1.0e-5);
        if index == 1 {
            assert!(
                particle.pos_z > 4.0,
                "transport must precede the dry-deposition height check"
            );
        }
    }
    assert!(!driver.has_remaining_steps());
    let met = MetTimeBracket {
        wind_t0: &upward,
        wind_t1: &upward,
        surface_t0: &surface,
        surface_t1: &surface,
        time_t0_seconds: start,
        time_t1_seconds: start + 3,
    };
    assert!(matches!(
        pollster::block_on(driver.run_timestep(&met, &forcing)),
        Err(TimeLoopError::SimulationComplete)
    ));
    assert_eq!(driver.current_time_seconds(), start + 2);
    pollster::block_on(driver.finalize()).expect("finalize");
    eprintln!("TIMELOOP-ORDER-126: WGSL transport/deposition/decay executed; inclusive end and report/advance verified");
}

#[test]
fn test_forward_timeloop_deferred_readback_preserves_gpu_output_and_cached_host_mass() {
    let _gpu_test_guard = lock_gpu_tests();
    use flexpart_gpu::gpu::{ConcentrationGridShape, ConcentrationGriddingParams};
    let config = ForwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: "20240101000002".to_string(),
        sync_particle_store_each_step: false,
        collect_deposition_probabilities_each_step: false,
        ..ForwardTimeLoopConfig::default()
    };
    let releases = vec![ReleaseConfig {
        name: "deferred".to_string(),
        start_time: "20240101000000".to_string(),
        end_time: "20240101000000".to_string(),
        lon: 10.0,
        lat: 5.0,
        z_min: 1.0,
        z_max: 1.0,
        mass_kg: 1.0,
        particle_count: 1,
        species_masses_kg: None,
        raw: BTreeMap::new(),
    }];
    let grid = GridDomain {
        xlon0: 0.0,
        ylat0: 0.0,
        dx: 1.0,
        dy: 1.0,
        nx: 64,
        ny: 64,
    };
    let mut driver = pollster::block_on(ForwardTimeLoopDriver::new(config, &releases, grid, 8))
        .expect("deferred output regression requires a WGSL adapter");
    let start = driver.current_time_seconds();
    let wind = uniform_wind_field(&synthetic_wind_grid(64, 64, 16), 1.0, 0.0, 0.0);
    let surface = synthetic_surface_fields(64, 64);
    let met = MetTimeBracket {
        wind_t0: &wind,
        wind_t1: &wind,
        surface_t0: &surface,
        surface_t1: &surface,
        time_t0_seconds: start,
        time_t1_seconds: start + 2,
    };
    let forcing = ForwardStepForcing {
        decay_constant_s_inv: vec![0.1],
        ..ForwardStepForcing::default()
    };
    let reports =
        pollster::block_on(driver.run_to_end(&met, &forcing)).expect("deferred forward run");
    assert_eq!(reports.len(), 3);
    assert!(reports.iter().all(
        |r| r.dry_deposition_probability.is_empty() && r.wet_deposition_probability.is_empty()
    ));
    // Finalize drains the device, but never refreshes the intentionally cached host mass.
    assert_eq!(
        driver.particle_store().get(0).expect("host particle").mass[0],
        1.0
    );
    let output = pollster::block_on(driver.accumulate_concentration_grid(
        ConcentrationGridShape {
            nx: 64,
            ny: 64,
            nz: 16,
        },
        ConcentrationGriddingParams::default(),
    ))
    .expect("explicit GPU output boundary");
    assert_eq!(output.particle_count_per_cell.iter().sum::<u32>(), 1);
    assert!((output.concentration_mass_kg.iter().sum::<f32>() - (-0.3_f32).exp()).abs() < 2.0e-6);
    assert_eq!(
        driver.particle_store().get(0).expect("host particle").mass[0],
        1.0
    );
    pollster::block_on(driver.finalize()).expect("idempotent finalize");
    eprintln!("TIMELOOP-DEFERRED-126: WGSL output observes three decay steps; host cache remains unchanged");
}

#[test]
fn test_forward_timeloop_preparation_errors_preserve_release_and_retry_state() {
    use flexpart_gpu::io::TemporalInterpolationError;
    use flexpart_gpu::simulation::ParticleForcingField;

    let _gpu_test_guard = lock_gpu_tests();
    let config = ForwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: "20240101000000".to_string(),
        time_bounds_behavior: TimeBoundsBehavior::Strict,
        ..ForwardTimeLoopConfig::default()
    };
    let releases = vec![ReleaseConfig {
        name: "error_retry".to_string(),
        start_time: config.start_timestamp.clone(),
        end_time: config.end_timestamp.clone(),
        lon: 10.0,
        lat: 5.0,
        z_min: 1.0,
        z_max: 1.0,
        mass_kg: 1.0,
        particle_count: 1,
        species_masses_kg: None,
        raw: BTreeMap::new(),
    }];
    let grid = GridDomain {
        xlon0: 0.0,
        ylat0: 0.0,
        dx: 1.0,
        dy: 1.0,
        nx: 64,
        ny: 64,
    };
    let mut driver = pollster::block_on(ForwardTimeLoopDriver::new(config, &releases, grid, 8))
        .expect("error/retry regression requires a WGSL adapter");
    let start = driver.current_time_seconds();
    let wind = uniform_wind_field(&synthetic_wind_grid(64, 64, 16), 0.0, 0.0, 0.0);
    let surface = synthetic_surface_fields(64, 64);
    let mut met = MetTimeBracket {
        wind_t0: &wind,
        wind_t1: &wind,
        surface_t0: &surface,
        surface_t1: &surface,
        time_t0_seconds: start,
        time_t1_seconds: start,
    };
    let forcing = ForwardStepForcing {
        decay_constant_s_inv: vec![0.1],
        ..ForwardStepForcing::default()
    };
    assert!(matches!(
        pollster::block_on(driver.run_timestep(&met, &forcing)),
        Err(TimeLoopError::Temporal(
            TemporalInterpolationError::InvalidTimeBracket { .. }
        ))
    ));
    // Release precedes meteorology validation in the original driver; errors do not roll it back.
    assert_eq!(driver.current_time_seconds(), start);
    assert_eq!(driver.particle_store().active_count(), 1);
    assert_eq!(
        driver
            .particle_store()
            .get(0)
            .expect("released particle")
            .mass[0],
        1.0
    );

    met.time_t1_seconds = start + 1;
    let invalid_species = ForwardStepForcing {
        wet_scavenging_coefficient_s_inv: Vec::new(),
        ..forcing.clone()
    };
    assert!(matches!(
        pollster::block_on(driver.run_timestep(&met, &invalid_species)),
        Err(TimeLoopError::ForcingLengthMismatch {
            field: "wet_scavenging_coefficient_s_inv",
            expected: 1,
            actual: 0
        })
    ));
    let invalid_slots = ForwardStepForcing {
        dry_deposition_velocity_m_s: vec![ParticleForcingField::PerParticle(vec![0.8; 7])],
        ..forcing.clone()
    };
    assert!(matches!(
        pollster::block_on(driver.run_timestep(&met, &invalid_slots)),
        Err(TimeLoopError::ForcingLengthMismatch {
            field: "dry_deposition_velocity_m_s",
            expected: 8,
            actual: 7
        })
    ));
    assert_eq!(driver.current_time_seconds(), start);
    assert_eq!(driver.particle_store().active_count(), 1);
    assert_eq!(
        driver
            .particle_store()
            .get(0)
            .expect("released particle")
            .mass[0],
        1.0
    );

    let report = pollster::block_on(driver.run_timestep(&met, &forcing)).expect("retry GPU step");
    assert_eq!(report.step_index, 0);
    assert_eq!(report.simulation_time_seconds, start);
    assert_eq!(report.timestamp, "20240101000000");
    assert_eq!(report.released_count, 0);
    assert!(report.released_slots.is_empty());
    assert_eq!(report.active_particle_count, 1);
    assert!(
        (driver
            .particle_store()
            .get(0)
            .expect("released particle")
            .mass[0]
            - (-0.1_f32).exp())
        .abs()
            < 1.0e-5
    );
    assert_eq!(driver.current_time_seconds(), start + 1);
    assert!(!driver.has_remaining_steps());
    pollster::block_on(driver.finalize()).expect("finalize");
    eprintln!("TIMELOOP-ERROR-126: bracket/species/slot errors preserve clock and release state; one WGSL retry executed");
}
