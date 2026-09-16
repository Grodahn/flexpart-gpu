//! Diagnosed-PBL vertical confinement test (RISK-03.3G-04).
//!
//! With no operator-provided mixing height, the driver must diagnose the PBL
//! state from the meteorological column (Richardson, oracle-conformant)
//! instead of falling back silently: a stable inversion column traps the
//! plume near the ground, matching the FLEXPART 11.1 stable regime.

use std::collections::BTreeMap;

use flexpart_gpu::config::ReleaseConfig;
use flexpart_gpu::coords::GridDomain;
use flexpart_gpu::gpu::GpuError;
use flexpart_gpu::io::{
    diagnose_missing_mixing_heights, PblComputationOptions, TimeBoundsBehavior,
};
use flexpart_gpu::physics::VelocityToGridScale;
use flexpart_gpu::simulation::{
    ForwardStepForcing, ForwardTimeLoopConfig, ForwardTimeLoopDriver, MetTimeBracket, TimeLoopError,
};
use flexpart_gpu::wind::{SurfaceFields, WindField3D, WindFieldGrid};
use ndarray::Array1;

const TEST_ID: &str = "RISK-03.3G-04";

const NX: usize = 32;
const NY: usize = 32;
const NZ: usize = 8;
const DX_DEG: f64 = 0.1;
const DY_DEG: f64 = 0.1;
const XLON0: f64 = 6.0;
const YLAT0: f64 = 47.0;

const HEIGHTS_M: [f32; 8] = [10.0, 100.0, 500.0, 1500.0, 3000.0, 5000.0, 10000.0, 20000.0];
const LEVEL_TEMPS_K: [f32; 8] = [300.0, 312.0, 320.0, 322.0, 324.0, 326.0, 328.0, 330.0];

const PARTICLE_COUNT: u64 = 2000;
const DT_SECONDS: i64 = 300;
const N_STEPS: usize = 12;

fn level_pressure_pa(height_m: f32) -> f32 {
    101_325.0 * (-9.81 * height_m / (287.05 * 290.0)).exp()
}

fn build_column_wind() -> (WindFieldGrid, WindField3D) {
    let grid = WindFieldGrid::new(
        NX,
        NY,
        NZ,
        NZ,
        NZ,
        DX_DEG as f32,
        DY_DEG as f32,
        XLON0 as f32,
        YLAT0 as f32,
        Array1::from_vec(HEIGHTS_M.to_vec()),
    );
    let mut field = WindField3D::zeros(NX, NY, NZ);
    for k in 0..NZ {
        field.u_ms.slice_mut(ndarray::s![.., .., k]).fill(5.0);
        field.v_ms.slice_mut(ndarray::s![.., .., k]).fill(-3.0);
        field
            .temperature_k
            .slice_mut(ndarray::s![.., .., k])
            .fill(LEVEL_TEMPS_K[k]);
        field
            .specific_humidity
            .slice_mut(ndarray::s![.., .., k])
            .fill(0.005);
        field
            .pressure_pa
            .slice_mut(ndarray::s![.., .., k])
            .fill(level_pressure_pa(HEIGHTS_M[k]));
    }
    (grid, field)
}

/// Surface fields mirroring oracle GRIB surface input with NO provided PBL
/// state: stress present (ustar via scalev), downward heat flux (stable in
/// the oracle/ECMWF sign convention), mixing height unset for diagnosis.
fn undiagnosed_surface() -> SurfaceFields {
    let mut s = SurfaceFields::zeros(NX, NY);
    s.surface_pressure_pa.fill(101_325.0);
    s.u10_ms.fill(5.0);
    s.v10_ms.fill(-3.0);
    s.temperature_2m_k.fill(300.0);
    s.dewpoint_2m_k.fill(295.0);
    s.sensible_heat_flux_w_m2.fill(50.0);
    s.solar_radiation_w_m2.fill(220.0);
    s.surface_stress_n_m2.fill(0.2);
    s.friction_velocity_ms.fill(0.0);
    s.convective_velocity_scale_ms.fill(0.0);
    s.mixing_height_m.fill(0.0);
    s.tropopause_height_m.fill(10_000.0);
    s.inv_obukhov_length_per_m.fill(0.0);
    s
}

fn velocity_scale() -> VelocityToGridScale {
    let mut level_heights_m = [0.0_f32; 16];
    for (i, &v) in HEIGHTS_M.iter().enumerate() {
        level_heights_m[i] = v;
    }
    VelocityToGridScale {
        x_grid_per_meter: 1.0 / 7140.0,
        y_grid_per_meter: 1.0 / 11119.0,
        z_grid_per_meter: 1.0,
        level_heights_m,
    }
}

#[test]
fn test_diagnosed_stable_pbl_traps_plume_near_ground() {
    // Reference diagnosis on the identical inputs (self-calibrating bound:
    // the run must respect the diagnosed ceiling, whatever its value).
    let (_, column_field) = build_column_wind();
    let probe_surface = undiagnosed_surface();
    let mut probe_hmix = probe_surface.mixing_height_m.clone();
    let mut probe_heights = [0.0_f32; 16];
    for (i, &v) in HEIGHTS_M.iter().enumerate() {
        probe_heights[i] = v;
    }
    let outcome = diagnose_missing_mixing_heights(
        &mut probe_hmix,
        &probe_surface.surface_pressure_pa,
        &probe_surface.temperature_2m_k,
        &probe_surface.dewpoint_2m_k,
        &probe_surface.surface_stress_n_m2,
        &column_field.temperature_k,
        &column_field.specific_humidity,
        &column_field.u_ms,
        &column_field.v_ms,
        &column_field.pressure_pa,
        &probe_heights,
        &probe_surface.sensible_heat_flux_w_m2,
    );
    assert_eq!(
        outcome.diagnosed,
        NX * NY,
        "{TEST_ID}: stable column must diagnose everywhere"
    );
    let diagnosed_hmix = probe_hmix[[0, 0]];
    assert!(
        (100.0..=4500.0).contains(&diagnosed_hmix),
        "{}: diagnosed hmix out of oracle bounds: {}",
        TEST_ID,
        diagnosed_hmix
    );

    let release_grid = GridDomain {
        xlon0: XLON0,
        ylat0: YLAT0,
        dx: DX_DEG,
        dy: DY_DEG,
        nx: NX,
        ny: NY,
    };
    let releases = vec![ReleaseConfig {
        name: "stable-trap".to_string(),
        start_time: "20240101000000".to_string(),
        end_time: "20240101000000".to_string(),
        lon: 8.0,
        lat: 49.0,
        z_min: 50.0,
        z_max: 50.0,
        mass_kg: 1.0,
        particle_count: PARTICLE_COUNT,
        raw: BTreeMap::new(),
    }];
    let total_seconds = DT_SECONDS * N_STEPS as i64;
    let config = ForwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: format!(
            "20240101{:02}{:02}00",
            total_seconds / 3600,
            (total_seconds % 3600) / 60
        ),
        timestep_seconds: DT_SECONDS,
        time_bounds_behavior: TimeBoundsBehavior::Clamp,
        velocity_to_grid_scale: velocity_scale(),
        sync_particle_store_each_step: true,
        collect_deposition_probabilities_each_step: false,
        pbl_options: PblComputationOptions::default(),
        ..ForwardTimeLoopConfig::default()
    };
    let mut driver = match pollster::block_on(ForwardTimeLoopDriver::new(
        config,
        &releases,
        release_grid,
        PARTICLE_COUNT as usize,
    )) {
        Ok(d) => d,
        Err(TimeLoopError::Gpu(GpuError::NoAdapter)) => {
            eprintln!("{TEST_ID}: no WGSL adapter - skipping test");
            return;
        }
        Err(err) => panic!("{TEST_ID}: driver init failed: {err}"),
    };

    let (_grid, wind) = build_column_wind();
    let surface = undiagnosed_surface();
    let start_secs = 1_704_067_200_i64;
    let met = MetTimeBracket {
        wind_t0: &wind,
        wind_t1: &wind,
        surface_t0: &surface,
        surface_t1: &surface,
        time_t0_seconds: start_secs,
        time_t1_seconds: start_secs + total_seconds,
    };
    pollster::block_on(driver.run_to_end(&met, &ForwardStepForcing::default()))
        .expect("simulation completes");

    let store = driver.particle_store();
    let active: Vec<_> = store.as_slice().iter().filter(|p| p.is_active()).collect();
    assert_eq!(active.len(), PARTICLE_COUNT as usize);
    let max_z = active.iter().map(|p| p.pos_z).fold(0.0_f32, f32::max);
    let mean_z: f32 = active.iter().map(|p| p.pos_z).sum::<f32>() / active.len() as f32;
    eprintln!(
        "{}: diagnosed_hmix={:.1} mean_z={:.1} max_z={:.1}",
        TEST_ID, diagnosed_hmix, mean_z, max_z
    );
    // The plume must respect the diagnosed ceiling (plus one output level of
    // numerical margin) and stay trapped far below the neutral baseline.
    assert!(
        max_z <= diagnosed_hmix + 100.0,
        "{}: plume escaped the diagnosed PBL: max_z={:.1}, hmix={:.1}",
        TEST_ID,
        max_z,
        diagnosed_hmix
    );
    assert!(
        mean_z < diagnosed_hmix,
        "{}: plume not trapped: mean_z={:.1}, hmix={:.1}",
        TEST_ID,
        mean_z,
        diagnosed_hmix
    );
}

#[test]
fn test_level_pressure_helper_is_monotonic() {
    let mut previous = f32::INFINITY;
    for &h in &HEIGHTS_M {
        let p = level_pressure_pa(h);
        assert!(p < previous && p > 0.0, "pressure must fall with height");
        previous = p;
    }
}
