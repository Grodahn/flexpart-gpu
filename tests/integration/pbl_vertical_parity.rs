//! PBL vertical transport parity test (neutral + stable cases).
//!
//! Runs forward integration with GPU WGSL path through the full production
//! pipeline (advection + fused Hanna+Langevin + PBL diagnostics) and verifies
//! that PBL diagnostics match independent FLEXPART 11.1 oracle reference values.
//!
//! Cases:
//! 1. Neutral: vanishing sensible heat flux, Richardson-diagnosed hmix (no predefined hmix)
//! 2. Stable: predefined hmix, negative sensible heat flux (from physics_validation)

use std::collections::BTreeMap;

use flexpart_gpu::config::ReleaseConfig;
use flexpart_gpu::coords::GridDomain;
use flexpart_gpu::gpu::GpuError;
use flexpart_gpu::io::{PblComputationOptions, TimeBoundsBehavior};
use flexpart_gpu::physics::VelocityToGridScale;
use flexpart_gpu::pbl::StabilityClass;
use flexpart_gpu::simulation::{
    ForwardStepForcing, ForwardTimeLoopConfig, ForwardTimeLoopDriver, MetTimeBracket,
    TimeLoopError,
};
use flexpart_gpu::wind::{SurfaceFields, WindField3D, WindFieldGrid};
use ndarray::Array1;

const NX: usize = 64;
const NY: usize = 64;
const WIND_NZ: usize = 8;
const DX_DEG: f64 = 0.1;
const DY_DEG: f64 = 0.1;
const XLON0: f64 = 6.0;
const YLAT0: f64 = 6.0;

const RELEASE_LON: f64 = 9.0;
const RELEASE_LAT: f64 = 9.0;
const RELEASE_Z: f64 = 50.0;
const PARTICLE_COUNT: u64 = 500;
const MASS_KG: f64 = 1.0;

const DT: i64 = 300;
const N_STEPS: usize = 12;
const SIM_DURATION_S: i64 = DT * N_STEPS as i64;

const WIND_HEIGHTS: [f32; 8] = [0.0, 100.0, 500.0, 1500.0, 3000.0, 5000.0, 10000.0, 20000.0];

const R_EARTH: f64 = 6_371_000.0;
const PI: f32 = std::f32::consts::PI;

/// Tolerance for PBL diagnostic comparisons against FLEXPART 11.1 oracle.
/// Justified by f32 precision and Monte Carlo convergence in turbulence.
const PBL_REL_TOL: f32 = 5e-3;
const PBL_ABS_TOL: f32 = 1e-2;

/// Load the neutral case fixture with FLEXPART 11.1 oracle reference values.
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

/// Load the stable case fixture with FLEXPART 11.1 oracle reference values.
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

/// Build a uniform wind grid for the given dimensions.
fn wind_grid(nx: usize, ny: usize, nz: usize) -> WindFieldGrid {
    WindFieldGrid::new(
        nx, ny, nz, nz, nz,
        DX_DEG as f32, DY_DEG as f32,
        XLON0 as f32, YLAT0 as f32,
        Array1::from_vec(WIND_HEIGHTS.to_vec()),
    )
}

/// Build a uniform wind field with constant u, v, w.
fn uniform_wind(grid: &WindFieldGrid, u: f32, v: f32, w: f32) -> WindField3D {
    let mut field = WindField3D::zeros(grid.nx, grid.ny, grid.nz);
    field.u_ms.fill(u);
    field.v_ms.fill(v);
    field.w_ms.fill(w);
    field
}

/// Build surface fields for the neutral case (vanishing sensible heat flux,
/// no predefined hmix, profile point for Richardson diagnostic).
fn neutral_surface_fields() -> SurfaceFields {
    let mut s = SurfaceFields::zeros(NX, NY);
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

/// Build surface fields for the stable case (predefined hmix, negative sensible heat flux).
fn stable_surface_fields() -> SurfaceFields {
    let mut s = SurfaceFields::zeros(NX, NY);
    s.surface_pressure_pa.fill(101_325.0);
    s.temperature_2m_k.fill(289.0);
    s.u10_ms.fill(5.0);
    s.v10_ms.fill(-3.0);
    s.dewpoint_2m_k.fill(284.0);
    s.sensible_heat_flux_w_m2.fill(-40.0);
    s.solar_radiation_w_m2.fill(220.0);
    s.surface_stress_n_m2.fill(0.2);
    s.friction_velocity_ms.fill(0.35);
    s.convective_velocity_scale_ms.fill(0.1);
    s.mixing_height_m.fill(1500.0);
    s.tropopause_height_m.fill(10_000.0);
    s.inv_obukhov_length_per_m.fill(0.0);
    s
}

/// Build velocity-to-grid scale conversion.
fn velocity_scale() -> VelocityToGridScale {
    let lat_rad = (RELEASE_LAT as f32) * PI / 180.0;
    let dx_m = R_EARTH * f64::from(lat_rad.cos()) * (DX_DEG * std::f64::consts::PI / 180.0);
    let dy_m = R_EARTH * (DY_DEG * std::f64::consts::PI / 180.0);
    VelocityToGridScale {
        x_grid_per_meter: (1.0 / dx_m) as f32,
        y_grid_per_meter: (1.0 / dy_m) as f32,
        z_grid_per_meter: 1.0,
        level_heights_m: {
            let mut h = [0.0_f32; 16];
            for (i, &v) in WIND_HEIGHTS.iter().enumerate() {
                h[i] = v;
            }
            h
        },
    }
}

/// Verify PBL diagnostics from a GPU run against FLEXPART 11.1 oracle reference.
fn verify_pbl_diagnostics(
    fixture: &serde_json::Value,
    pbl_state: &flexpart_gpu::pbl::PblState,
    case_name: &str,
) {
    let oracle = &fixture["oracle_reference_values"];

    // Extract reference values
    let ref_ustar = oracle["friction_velocity_m_s"].as_f64().expect("ref ustar") as f32;
    let ref_oli = oracle["inverse_obukhov_length_per_m"].as_f64().expect("ref oli") as f32;
    let ref_hmix = oracle["mixing_height_m"].as_f64().expect("ref hmix") as f32;
    let ref_wstar = oracle["convective_velocity_scale_m_s"].as_f64().expect("ref wstar") as f32;
    let ref_stability = oracle["stability_class"].as_str().expect("ref stability");
    let ref_bulk_ri = oracle["bulk_richardson_number"].as_f64();

    // Sample the center grid cell (index 0,0 for uniform fields)
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

    // Verify ustar
    let ustar_rel_err = (ustar - ref_ustar).abs() / ref_ustar.max(1e-6);
    assert!(
        ustar_rel_err <= PBL_REL_TOL || (ustar - ref_ustar).abs() <= PBL_ABS_TOL,
        "{}: ustar mismatch: computed={:.4}, oracle={:.4}, rel_err={:.2e} (tol={:.2e})",
        case_name, ustar, ref_ustar, ustar_rel_err, PBL_REL_TOL
    );

    // Verify oli (inverse Obukhov length)
    let oli_abs_err = (oli - ref_oli).abs();
    assert!(
        oli_abs_err <= PBL_ABS_TOL,
        "{}: oli mismatch: computed={:.6}, oracle={:.6}, abs_err={:.2e} (tol={:.2e})",
        case_name, oli, ref_oli, oli_abs_err, PBL_ABS_TOL
    );

    // Verify hmix (mixing height)
    let hmix_rel_err = (hmix - ref_hmix).abs() / ref_hmix.max(1.0);
    assert!(
        hmix_rel_err <= PBL_REL_TOL || (hmix - ref_hmix).abs() <= PBL_ABS_TOL.max(10.0),
        "{}: hmix mismatch: computed={:.1}, oracle={:.1}, rel_err={:.2e} (tol={:.2e})",
        case_name, hmix, ref_hmix, hmix_rel_err, PBL_REL_TOL
    );

    // Verify wstar
    if ref_wstar > 0.0 {
        let wstar_rel_err = (wstar - ref_wstar).abs() / ref_wstar;
        assert!(
            wstar_rel_err <= PBL_REL_TOL || (wstar - ref_wstar).abs() <= PBL_ABS_TOL,
            "{}: wstar mismatch: computed={:.4}, oracle={:.4}, rel_err={:.2e}",
            case_name, wstar, ref_wstar, wstar_rel_err
        );
    } else {
        assert!(
            wstar.abs() <= PBL_ABS_TOL,
            "{}: wstar should be near zero for stable/neutral: computed={:.4}",
            case_name, wstar
        );
    }

    // Verify stability class
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

    // Verify sensible heat flux (should be ~0 for neutral case)
    if case_name == "neutral" {
        assert!(
            sshf.abs() <= 1.0,
            "neutral case: sshf should be ~0, got {:.1}",
            sshf
        );
    }

    println!("{}: PBL diagnostics PASS", case_name);
}

/// Run a forward timeloop simulation with the given surface fields and verify PBL.
async fn run_case(
    case_name: &str,
    surface_t0: SurfaceFields,
    surface_t1: SurfaceFields,
    fixture: &serde_json::Value,
) {
    let release_grid = GridDomain {
        xlon0: XLON0,
        ylat0: YLAT0,
        dx: DX_DEG,
        dy: DY_DEG,
        nx: NX,
        ny: NY,
    };

    let releases = vec![ReleaseConfig {
        name: case_name.to_string(),
        start_time: "20240101000000".to_string(),
        end_time: "20240101000000".to_string(),
        lon: RELEASE_LON,
        lat: RELEASE_LAT,
        z_min: RELEASE_Z,
        z_max: RELEASE_Z,
        mass_kg: MASS_KG,
        particle_count: PARTICLE_COUNT,
        raw: BTreeMap::new(),
    }];

    let config = ForwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: format!(
            "20240101{:02}{:02}00",
            SIM_DURATION_S / 3600,
            (SIM_DURATION_S % 3600) / 60
        ),
        timestep_seconds: DT,
        time_bounds_behavior: TimeBoundsBehavior::Clamp,
        velocity_to_grid_scale: velocity_scale(),
        pbl_options: PblComputationOptions::default(),
        sync_particle_store_each_step: true,
        collect_deposition_probabilities_each_step: false,
        ..ForwardTimeLoopConfig::default()
    };

    let mut driver = match ForwardTimeLoopDriver::new(
        config,
        &releases,
        release_grid,
        PARTICLE_COUNT as usize,
    ).await {
        Ok(d) => d,
        Err(TimeLoopError::Gpu(GpuError::NoAdapter)) => {
            panic!("GPU adapter required for PBL vertical parity test — no adapter available");
        }
        Err(err) => panic!("driver init failed: {err}"),
    };

    let wg = wind_grid(NX, NY, WIND_NZ);
    let wind_t0 = uniform_wind(&wg, 4.0, 1.0, 0.0);
    let wind_t1 = uniform_wind(&wg, 4.0, 1.0, 0.0);

    let start_secs = 1_704_067_200_i64;
    let met = MetTimeBracket {
        wind_t0: &wind_t0,
        wind_t1: &wind_t1,
        surface_t0: &surface_t0,
        surface_t1: &surface_t1,
        time_t0_seconds: start_secs,
        time_t1_seconds: start_secs + SIM_DURATION_S,
    };

    let forcing = ForwardStepForcing::default();
    let reports = driver.run_to_end(&met, &forcing).await
        .expect("simulation should complete");

    assert_eq!(
        reports.len(),
        N_STEPS + 1,
        "should run correct number of steps"
    );

    // Verify particles were actually transported
    let store = driver.particle_store();
    let particles = store.as_slice();
    let active: Vec<_> = particles.iter().filter(|p| p.is_active()).collect();
    let active_count = active.len();

    assert_eq!(
        active_count, PARTICLE_COUNT as usize,
        "{}: all particles should remain active", case_name
    );

    // Verify horizontal transport occurred (wind is non-zero)
    let release_grid_x = (RELEASE_LON - XLON0) / DX_DEG;
    let release_grid_y = (RELEASE_LAT - YLAT0) / DY_DEG;
    let mean_gx: f64 =
        active.iter().map(|p| p.grid_x()).sum::<f64>() / active_count as f64;
    let mean_gy: f64 =
        active.iter().map(|p| p.grid_y()).sum::<f64>() / active_count as f64;

    assert!(
        (mean_gx - release_grid_x).abs() > 0.1,
        "{}: particles should be advected in x (u=4): mean_gx={:.2}, release_x={:.2}",
        case_name, mean_gx, release_grid_x
    );
    assert!(
        (mean_gy - release_grid_y).abs() > 0.1,
        "{}: particles should be advected in y (v=1): mean_gy={:.2}, release_y={:.2}",
        case_name, mean_gy, release_grid_y
    );

    // Verify vertical mixing occurred (particles spread from release height)
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

    // Download PBL state from GPU (use the last written buffer)
    let pbl_state = driver.pbl_buffers()[driver.pbl_write_index() ^ 1]
        .download_state(&driver.gpu_context())
        .await
        .expect("PBL readback should succeed");

    // Verify PBL diagnostics against oracle reference
    verify_pbl_diagnostics(fixture, &pbl_state, case_name);
}

#[test]
fn pbl_vertical_parity_neutral_richardson_fallback() {
    let fixture = load_neutral_fixture();
    let surface = neutral_surface_fields();

    pollster::block_on(async {
        run_case("neutral", surface.clone(), surface, &fixture).await;
    });
}

#[test]
fn pbl_vertical_parity_stable_predefined_hmix() {
    let fixture = load_stable_fixture();
    let surface = stable_surface_fields();

    pollster::block_on(async {
        run_case("stable", surface.clone(), surface, &fixture).await;
    });
}