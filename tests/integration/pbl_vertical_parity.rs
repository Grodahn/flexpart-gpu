//! Diagnosed-PBL vertical confinement test (RISK-03.3G-04).
//!
//! With no operator-provided mixing height, the driver must diagnose the PBL
//! state from the meteorological column (Richardson, oracle-conformant)
//! instead of falling back silently: a stable inversion column traps the
//! plume near the ground, matching the FLEXPART 11.1 stable regime.
//!
//! This is a confinement check for the new wiring, not a full oracle parity
//! proof: it uses one synthetic inversion column, one seed, and a 1 h window.
//! A matched-formulation oracle rerun with manifests and multi-seed
//! statistics is still required before claiming vertical parity.

use std::collections::BTreeMap;

use flexpart_gpu::config::ReleaseConfig;
use flexpart_gpu::coords::GridDomain;
use flexpart_gpu::gpu::GpuError;
use flexpart_gpu::io::{diagnose_missing_mixing_heights, PblComputationOptions, TimeBoundsBehavior};
use flexpart_gpu::pbl::StabilityClass;
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
    // Grid spacing in meters at the release latitude, mirroring the
    // SW-WGPU-ADVECTION-001 derivation instead of magic constants.
    const EARTH_RADIUS_M: f64 = 6_371_000.0;
    const RELEASE_LAT_DEG: f64 = 49.0;
    let lat_rad = (RELEASE_LAT_DEG as f32).to_radians();
    let dx_m = EARTH_RADIUS_M * f64::from(lat_rad.cos()) * (DX_DEG * std::f64::consts::PI / 180.0);
    let dy_m = EARTH_RADIUS_M * (DY_DEG * std::f64::consts::PI / 180.0);
    let mut level_heights_m = [0.0_f32; 16];
    for (i, &v) in HEIGHTS_M.iter().enumerate() {
        level_heights_m[i] = v;
    }
    VelocityToGridScale {
        x_grid_per_meter: (1.0 / dx_m) as f32,
        y_grid_per_meter: (1.0 / dy_m) as f32,
        z_grid_per_meter: 1.0,
        level_heights_m,
    }
}

#[test]
fn test_diagnosed_stable_pbl_traps_plume_near_ground() {
    // Independent expectation first: this strong inversion column must
    // diagnose near the oracle minimum (hmixmin = 100 m). The 100-250 m band
    // below is anchored to that oracle bound, not just to whatever the
    // function under test returns.
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
    assert!(
        (100.0..=250.0).contains(&diagnosed_hmix),
        "{TEST_ID}: strong inversion must diagnose near hmixmin, got {diagnosed_hmix:.1} m",
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
            panic!(
                "{TEST_ID}: no WGSL adapter found; run with a software fallback \
                 (FLEXPART_GPU_SOFTWARE=1 / WGPU_FORCE_FALLBACK_ADAPTER=1). \
                 Skipping would turn the confinement gate green without a dispatch."
            );
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
    // The absolute 250 m bound below anchors the strong-inversion scenario
    // to the oracle hmixmin regime independently of the self-computed ceiling.
    assert!(
        max_z <= diagnosed_hmix + 100.0,
        "{}: plume escaped the diagnosed PBL: max_z={:.1}, hmix={:.1}",
        TEST_ID,
        max_z,
        diagnosed_hmix
    );
    assert!(
        max_z <= 250.0,
        "{TEST_ID}: stable plume must stay near hmixmin, got max_z={max_z:.1} m",
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

// ============================================================================
// Oracle Parity Tests (Issue #8: neutral base case + stable reference)
// ============================================================================

const PARITY_NX: usize = 64;
const PARITY_NY: usize = 64;
const PARITY_WIND_NZ: usize = 8;
const PARITY_DX_DEG: f64 = 0.1;
const PARITY_DY_DEG: f64 = 0.1;
const PARITY_XLON0: f64 = 6.0;
const PARITY_YLAT0: f64 = 6.0;

const PARITY_RELEASE_LON: f64 = 9.0;
const PARITY_RELEASE_LAT: f64 = 9.0;
const PARITY_RELEASE_Z: f64 = 50.0;
const PARITY_PARTICLE_COUNT: u64 = 500;
const PARITY_MASS_KG: f64 = 1.0;

const PARITY_DT: i64 = 300;
const PARITY_N_STEPS: usize = 12;
const PARITY_SIM_DURATION_S: i64 = PARITY_DT * PARITY_N_STEPS as i64;

const PARITY_WIND_HEIGHTS: [f32; 8] = [0.0, 100.0, 500.0, 1500.0, 3000.0, 5000.0, 10000.0, 20000.0];

const PARITY_R_EARTH: f64 = 6_371_000.0;
const PARITY_PI: f32 = std::f32::consts::PI;

const PARITY_PBL_REL_TOL: f32 = 5e-3;
const PARITY_PBL_ABS_TOL: f32 = 1e-2;

fn load_neutral_fixture() -> serde_json::Value {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set");
    let path = std::path::Path::new(&manifest_dir)
        .join("tests")
        .join("fixtures")
        .join("pbl_vertical_parity")
        .join("neutral_case_fixture.json");
    let content = std::fs::read_to_string(path).expect("fixture must exist");
    serde_json::from_str(&content).expect("fixture must be valid JSON")
}

fn load_stable_fixture() -> serde_json::Value {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set");
    let path = std::path::Path::new(&manifest_dir)
        .join("tests")
        .join("fixtures")
        .join("pbl_vertical_parity")
        .join("stable_case_fixture.json");
    let content = std::fs::read_to_string(path).expect("fixture must exist");
    serde_json::from_str(&content).expect("fixture must be valid JSON")
}

fn parity_wind_grid(nx: usize, ny: usize, nz: usize) -> WindFieldGrid {
    WindFieldGrid::new(
        nx, ny, nz, nz, nz,
        PARITY_DX_DEG as f32, PARITY_DY_DEG as f32,
        PARITY_XLON0 as f32, PARITY_YLAT0 as f32,
        Array1::from_vec(PARITY_WIND_HEIGHTS.to_vec()),
    )
}

fn parity_uniform_wind(grid: &WindFieldGrid, u: f32, v: f32, w: f32) -> WindField3D {
    let mut field = WindField3D::zeros(grid.nx, grid.ny, grid.nz);
    field.u_ms.fill(u);
    field.v_ms.fill(v);
    field.w_ms.fill(w);
    field
}

fn parity_neutral_surface_fields() -> SurfaceFields {
    let mut s = SurfaceFields::zeros(PARITY_NX, PARITY_NY);
    s.surface_pressure_pa.fill(101_325.0);
    s.temperature_2m_k.fill(298.0);
    s.u10_ms.fill(4.0);
    s.v10_ms.fill(1.0);
    s.dewpoint_2m_k.fill(293.0);
    s.sensible_heat_flux_w_m2.fill(0.0);
    s.solar_radiation_w_m2.fill(200.0);
    s.surface_stress_n_m2.fill(0.3);
    s.friction_velocity_ms.fill(0.0);
    s.convective_velocity_scale_ms.fill(0.0);
    s.mixing_height_m.fill(0.0);
    s.tropopause_height_m.fill(10_000.0);
    s.inv_obukhov_length_per_m.fill(0.0);
    s
}

fn parity_stable_surface_fields() -> SurfaceFields {
    let mut s = SurfaceFields::zeros(PARITY_NX, PARITY_NY);
    s.surface_pressure_pa.fill(101_325.0);
    s.temperature_2m_k.fill(289.0);
    s.u10_ms.fill(5.0);
    s.v10_ms.fill(-3.0);
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

fn parity_velocity_scale() -> VelocityToGridScale {
    let lat_rad = (PARITY_RELEASE_LAT as f32) * PARITY_PI / 180.0;
    let dx_m = PARITY_R_EARTH * f64::from(lat_rad.cos()) * (PARITY_DX_DEG * std::f64::consts::PI / 180.0);
    let dy_m = PARITY_R_EARTH * (PARITY_DY_DEG * std::f64::consts::PI / 180.0);
    VelocityToGridScale {
        x_grid_per_meter: (1.0 / dx_m) as f32,
        y_grid_per_meter: (1.0 / dy_m) as f32,
        z_grid_per_meter: 1.0,
        level_heights_m: {
            let mut h = [0.0_f32; 16];
            for (i, &v) in PARITY_WIND_HEIGHTS.iter().enumerate() {
                h[i] = v;
            }
            h
        },
    }
}

fn verify_pbl_diagnostics(
    fixture: &serde_json::Value,
    pbl_state: &flexpart_gpu::pbl::PblState,
    case_name: &str,
) {
    let oracle = &fixture["oracle_reference_values"];

    let ref_ustar = oracle["friction_velocity_m_s"].as_f64().expect("ref ustar") as f32;
    let ref_oli = oracle["inverse_obukhov_length_per_m"].as_f64().expect("ref oli") as f32;
    let ref_hmix = oracle["mixing_height_m"].as_f64().expect("ref hmix") as f32;
    let ref_wstar = oracle["convective_velocity_scale_m_s"].as_f64().expect("ref wstar") as f32;
    let ref_stability = oracle["stability_class"].as_str().expect("ref stability");
    let ref_bulk_ri = oracle["bulk_richardson_number"].as_f64();

    let ustar = pbl_state.ustar[[0, 0]];
    let oli = pbl_state.oli[[0, 0]];
    let hmix = pbl_state.hmix[[0, 0]];
    let wstar = pbl_state.wstar[[0, 0]];
    let sshf = pbl_state.sshf[[0, 0]];
    let stability = pbl_state.stability_at(0, 0);

    println!("=== {} PBL Diagnostics ===", case_name);
    println!("  ustar:  computed={:.4}, oracle={:.4}", ustar, ref_ustar);
    println!("  oli:    computed={:.6}, oracle={:.6}", oli, ref_oli);
    println!("  hmix:   computed={:.1}, oracle={:.1}", hmix, ref_hmix);
    println!("  wstar:  computed={:.4}, oracle={:.4}", wstar, ref_wstar);
    println!("  sshf:   computed={:.1}", sshf);
    println!("  stability: computed={:?}, oracle={}", stability, ref_stability);
    if let Some(ri) = ref_bulk_ri {
        println!("  bulk_ri: oracle={:.6}", ri);
    }

    let ustar_rel_err = (ustar - ref_ustar).abs() / ref_ustar.max(1e-6);
    assert!(
        ustar_rel_err <= PARITY_PBL_REL_TOL || (ustar - ref_ustar).abs() <= PARITY_PBL_ABS_TOL,
        "{}: ustar mismatch: computed={:.4}, oracle={:.4}, rel_err={:.2e} (tol={:.2e})",
        case_name, ustar, ref_ustar, ustar_rel_err, PARITY_PBL_REL_TOL
    );

    let oli_abs_err = (oli - ref_oli).abs();
    assert!(
        oli_abs_err <= PARITY_PBL_ABS_TOL,
        "{}: oli mismatch: computed={:.6}, oracle={:.6}, abs_err={:.2e} (tol={:.2e})",
        case_name, oli, ref_oli, oli_abs_err, PARITY_PBL_ABS_TOL
    );

    let hmix_rel_err = (hmix - ref_hmix).abs() / ref_hmix.max(1.0);
    assert!(
        hmix_rel_err <= PARITY_PBL_REL_TOL || (hmix - ref_hmix).abs() <= PARITY_PBL_ABS_TOL.max(10.0),
        "{}: hmix mismatch: computed={:.1}, oracle={:.1}, rel_err={:.2e} (tol={:.2e})",
        case_name, hmix, ref_hmix, hmix_rel_err, PARITY_PBL_REL_TOL
    );

    if ref_wstar > 0.0 {
        let wstar_rel_err = (wstar - ref_wstar).abs() / ref_wstar;
        assert!(
            wstar_rel_err <= PARITY_PBL_REL_TOL || (wstar - ref_wstar).abs() <= PARITY_PBL_ABS_TOL,
            "{}: wstar mismatch: computed={:.4}, oracle={:.4}, rel_err={:.2e}",
            case_name, wstar, ref_wstar, wstar_rel_err
        );
    } else {
        assert!(
            wstar.abs() <= PARITY_PBL_ABS_TOL,
            "{}: wstar should be near zero for stable/neutral: computed={:.4}",
            case_name, wstar
        );
    }

    let expected_stability = match ref_stability {
        "Stable" => StabilityClass::Stable,
        "Unstable" => StabilityClass::Unstable,
        "Neutral" => StabilityClass::Neutral,
        _ => panic!("unknown stability class: {}", ref_stability),
    };
    assert_eq!(
        stability, expected_stability,
        "{}: stability mismatch: computed={:?}, oracle={}",
        case_name, stability, ref_stability
    );

    if case_name == "neutral" {
        assert!(
            sshf.abs() <= 1.0,
            "neutral case: sshf should be ~0, got {:.1}",
            sshf
        );
    }

    println!("{}: PBL diagnostics PASS", case_name);
}

async fn run_case(
    case_name: &str,
    surface_t0: SurfaceFields,
    surface_t1: SurfaceFields,
    fixture: &serde_json::Value,
) {
    let release_grid = GridDomain {
        xlon0: PARITY_XLON0,
        ylat0: PARITY_YLAT0,
        dx: PARITY_DX_DEG,
        dy: PARITY_DY_DEG,
        nx: PARITY_NX,
        ny: PARITY_NY,
    };

    let releases = vec![ReleaseConfig {
        name: case_name.to_string(),
        start_time: "20240101000000".to_string(),
        end_time: "20240101000000".to_string(),
        lon: PARITY_RELEASE_LON,
        lat: PARITY_RELEASE_LAT,
        z_min: PARITY_RELEASE_Z,
        z_max: PARITY_RELEASE_Z,
        mass_kg: PARITY_MASS_KG,
        particle_count: PARITY_PARTICLE_COUNT,
        raw: BTreeMap::new(),
    }];

    let config = ForwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: format!(
            "20240101{:02}{:02}00",
            PARITY_SIM_DURATION_S / 3600,
            (PARITY_SIM_DURATION_S % 3600) / 60
        ),
        timestep_seconds: PARITY_DT,
        time_bounds_behavior: TimeBoundsBehavior::Clamp,
        velocity_to_grid_scale: parity_velocity_scale(),
        pbl_options: PblComputationOptions::default(),
        sync_particle_store_each_step: true,
        collect_deposition_probabilities_each_step: false,
        ..ForwardTimeLoopConfig::default()
    };

    let mut driver = match ForwardTimeLoopDriver::new(
        config,
        &releases,
        release_grid,
        PARITY_PARTICLE_COUNT as usize,
    ).await {
        Ok(d) => d,
        Err(TimeLoopError::Gpu(GpuError::NoAdapter)) => {
            panic!("GPU adapter required for PBL vertical parity test — no adapter available");
        }
        Err(err) => panic!("driver init failed: {err}"),
    };

    let wg = parity_wind_grid(PARITY_NX, PARITY_NY, PARITY_WIND_NZ);
    let wind_t0 = parity_uniform_wind(&wg, 4.0, 1.0, 0.0);
    let wind_t1 = parity_uniform_wind(&wg, 4.0, 1.0, 0.0);

    let start_secs = 1_704_067_200_i64;
    let met = MetTimeBracket {
        wind_t0: &wind_t0,
        wind_t1: &wind_t1,
        surface_t0: &surface_t0,
        surface_t1: &surface_t1,
        time_t0_seconds: start_secs,
        time_t1_seconds: start_secs + PARITY_SIM_DURATION_S,
    };

    let forcing = ForwardStepForcing::default();
    let reports = driver.run_to_end(&met, &forcing).await
        .expect("simulation should complete");

    assert_eq!(
        reports.len(),
        PARITY_N_STEPS + 1,
        "should run correct number of steps"
    );

    let store = driver.particle_store();
    let particles = store.as_slice();
    let active: Vec<_> = particles.iter().filter(|p| p.is_active()).collect();
    let active_count = active.len();

    assert_eq!(
        active_count, PARITY_PARTICLE_COUNT as usize,
        "{}: all particles should remain active", case_name
    );

    let release_grid_x = (PARITY_RELEASE_LON - PARITY_XLON0) / PARITY_DX_DEG;
    let release_grid_y = (PARITY_RELEASE_LAT - PARITY_YLAT0) / PARITY_DY_DEG;
    let mean_gx: f64 =
        active.iter().map(|p| p.grid_x()).sum::<f64>() / active_count as f64;
    let mean_gy: f64 =
        active.iter().map(|p| p.grid_y()).sum::<f64>() / active_count as f64;

    assert!(
        (mean_gx - release_grid_x).abs() > 0.1,
        "{}: particles should be advected in x: mean_gx={:.2}, release_x={:.2}",
        case_name, mean_gx, release_grid_x
    );
    assert!(
        (mean_gy - release_grid_y).abs() > 0.1,
        "{}: particles should be advected in y: mean_gy={:.2}, release_y={:.2}",
        case_name, mean_gy, release_grid_y
    );

    let zs: Vec<f32> = active.iter().map(|p| p.pos_z).collect();
    let min_z = zs.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_z = zs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mean_z: f32 = zs.iter().sum::<f32>() / zs.len() as f32;
    let var_z: f32 = zs.iter().map(|z| (z - mean_z).powi(2)).sum::<f32>() / zs.len() as f32;
    let std_z = var_z.sqrt();

    println!("{}: particle z stats: min={:.1}m, max={:.1}m, mean={:.1}m, std={:.1}m",
             case_name, min_z, max_z, mean_z, std_z);
    assert!(
        min_z >= 0.0,
        "{}: no particle below ground", case_name
    );
    assert!(
        max_z <= fixture["oracle_reference_values"]["mixing_height_m"].as_f64().unwrap() as f32 + 10.0,
        "{}: particles exceed mixing height", case_name
    );
    assert!(
        std_z > 5.0,
        "{}: particles should have vertical spread from turbulence: std_z={:.1}",
        case_name, std_z
    );

    let pbl_state = driver.pbl_buffers()[driver.pbl_write_index() ^ 1]
        .download_state(&driver.gpu_context())
        .await
        .expect("PBL readback should succeed");

    verify_pbl_diagnostics(fixture, &pbl_state, case_name);
}

#[test]
fn pbl_vertical_parity_neutral_richardson_fallback() {
    let fixture = load_neutral_fixture();
    let surface = parity_neutral_surface_fields();

    pollster::block_on(async {
        run_case("neutral", surface.clone(), surface, &fixture).await;
    });
}

#[test]
fn pbl_vertical_parity_stable_predefined_hmix() {
    let fixture = load_stable_fixture();
    let surface = parity_stable_surface_fields();

    pollster::block_on(async {
        run_case("stable", surface.clone(), surface, &fixture).await;
    });
}
