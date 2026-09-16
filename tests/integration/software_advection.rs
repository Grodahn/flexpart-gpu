//! Software WGSL advection smoke test (SW-WGPU-ADVECTION-001).
//!
//! Infrastructure smoke test, not a FLEXPART scientific parity test. It proves
//! that the real WGSL compute path runs on a software fallback adapter
//! (Mesa Lavapipe / LLVMpipe, D3D12 WARP) without hardware.
//!
//! Scenario:
//! - 4096 particles released near 50 N / 10 E at 100 m AGL,
//! - uniform wind (u = +10 m/s, v = 0, w = 0),
//! - 3600 s simulation with 60 s steps (60 advection dispatches),
//! - turbulence, convection, wet/dry deposition, settling, and decay disabled
//!   (pure mean-wind advection dispatches only).
//!
//! Analytical expectation: 10 m/s * 3600 s = 36000 m eastward displacement,
//! no mean north/south or vertical motion.

use flexpart_gpu::coords::{distance_meters, geo_to_grid, GeoCoord, GridCoord, GridDomain};
use flexpart_gpu::gpu::{
    advect_particles_gpu_with_sampling, GpuAdapterOptions, GpuContext, GpuError, ParticleBuffers,
    WindBuffers, WindSamplingOptions,
};
use flexpart_gpu::particles::{Particle, ParticleInit};
use flexpart_gpu::physics::VelocityToGridScale;
use flexpart_gpu::wind::WindField3D;

/// Test identifier for reporting.
const TEST_ID: &str = "SW-WGPU-ADVECTION-001";

const PARTICLE_COUNT: usize = 4096;
const START_LON_DEG: f64 = 10.0;
const START_LAT_DEG: f64 = 50.0;
const START_Z_M: f32 = 100.0;

const U_WIND_MS: f32 = 10.0;
const V_WIND_MS: f32 = 0.0;
const W_WIND_MS: f32 = 0.0;

const DT_SECONDS: f32 = 60.0;
const STEP_COUNT: usize = 60;
const TOTAL_SECONDS: f32 = DT_SECONDS * STEP_COUNT as f32;

const NX: usize = 64;
const NY: usize = 64;
const NZ: usize = 8;
const DX_DEG: f64 = 0.1;
const DY_DEG: f64 = 0.1;
const XLON0_DEG: f64 = 6.0;
const YLAT0_DEG: f64 = 47.0;

const WIND_HEIGHTS_M: [f32; 8] = [0.0, 50.0, 100.0, 200.0, 500.0, 1500.0, 3000.0, 5000.0];

const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Eastward displacement tolerance [m]: 1 % of the 36 km signal.
///
/// Uniform-wind Petterssen advection is exact up to f32 rounding, so this
/// bound is generous by design and must not be loosened to hide regressions.
const EAST_DISPLACEMENT_TOLERANCE_M: f32 = 360.0;
/// Allowed absolute north/south drift [m].
const NORTH_DRIFT_TOLERANCE_M: f32 = 1.0;
/// Allowed absolute vertical drift [m].
const VERTICAL_DRIFT_TOLERANCE_M: f32 = 0.01;

fn release_domain() -> GridDomain {
    GridDomain {
        xlon0: XLON0_DEG,
        ylat0: YLAT0_DEG,
        dx: DX_DEG,
        dy: DY_DEG,
        nx: NX,
        ny: NY,
    }
}

fn uniform_wind() -> WindField3D {
    let mut field = WindField3D::zeros(NX, NY, NZ);
    field.u_ms.fill(U_WIND_MS);
    field.v_ms.fill(V_WIND_MS);
    field.w_ms.fill(W_WIND_MS);
    field
}

fn velocity_scale() -> VelocityToGridScale {
    let lat_rad = (START_LAT_DEG as f32).to_radians();
    let dx_m = EARTH_RADIUS_M * f64::from(lat_rad.cos()) * (DX_DEG * std::f64::consts::PI / 180.0);
    let dy_m = EARTH_RADIUS_M * (DY_DEG * std::f64::consts::PI / 180.0);
    let mut level_heights_m = [0.0_f32; 16];
    for (index, height) in WIND_HEIGHTS_M.iter().enumerate() {
        level_heights_m[index] = *height;
    }
    VelocityToGridScale {
        x_grid_per_meter: (1.0 / dx_m) as f32,
        y_grid_per_meter: (1.0 / dy_m) as f32,
        z_grid_per_meter: 1.0,
        level_heights_m,
    }
}

fn release_particles(domain: &GridDomain) -> Vec<Particle> {
    let grid = geo_to_grid(
        GeoCoord {
            lat: START_LAT_DEG,
            lon: START_LON_DEG,
        },
        domain,
    );
    let cell_x = grid.x.floor() as i32;
    let cell_y = grid.y.floor() as i32;
    let pos_x = (grid.x - f64::from(cell_x)) as f32;
    let pos_y = (grid.y - f64::from(cell_y)) as f32;
    (0..PARTICLE_COUNT)
        .map(|_| {
            Particle::new(&ParticleInit {
                cell_x,
                cell_y,
                pos_x,
                pos_y,
                pos_z: START_Z_M,
                mass: [1.0, 0.0, 0.0, 0.0],
                release_point: 0,
                class: 0,
                time: 0,
            })
        })
        .collect()
}

fn mean_geo(particles: &[Particle], domain: &GridDomain) -> GeoCoord {
    let mut sum_x = 0.0_f64;
    let mut sum_y = 0.0_f64;
    let mut active = 0_usize;
    for particle in particles.iter().filter(|p| p.is_active()) {
        sum_x += particle.grid_x();
        sum_y += particle.grid_y();
        active += 1;
    }
    assert_eq!(active, PARTICLE_COUNT, "all particles must stay active");
    let mean = GridCoord {
        x: sum_x / active as f64,
        y: sum_y / active as f64,
    };
    GeoCoord {
        lon: mean.x.mul_add(domain.dx, domain.xlon0),
        lat: mean.y.mul_add(domain.dy, domain.ylat0),
    }
}

#[test]
fn test_sw_wgpu_advection_001_constant_wind_displacement() {
    let domain = release_domain();
    let start = GeoCoord {
        lat: START_LAT_DEG,
        lon: START_LON_DEG,
    };

    // Explicitly request the software fallback adapter so this smoke test
    // exercises the SW-WGPU path even on machines with a hardware GPU.
    // Skip when no adapter is available at all.
    let context = match pollster::block_on(GpuContext::with_options(GpuAdapterOptions::software())) {
        Ok(context) => context,
        Err(GpuError::NoAdapter) => {
            eprintln!("{TEST_ID}: no WGSL adapter found — skipping smoke test");
            return;
        }
        Err(error) => panic!("{TEST_ID}: unexpected GPU init error: {error}"),
    };
    eprintln!(
        "{TEST_ID}: adapter={} backend={:?} type={:?} software={}",
        context.device_name(),
        context.backend(),
        context.adapter_type(),
        context.is_software_adapter()
    );

    let field = uniform_wind();
    let wind_buffers = WindBuffers::from_field(&context, &field).expect("wind upload succeeds");
    let particles = release_particles(&domain);
    let particle_buffers = ParticleBuffers::from_particles(&context, &particles);
    let scale = velocity_scale();
    let sampling = WindSamplingOptions {
        force_buffer_path: true,
    };

    for _ in 0..STEP_COUNT {
        advect_particles_gpu_with_sampling(
            &context,
            &particle_buffers,
            &wind_buffers,
            DT_SECONDS,
            scale,
            sampling,
        )
        .expect("WGSL advection dispatch succeeds");
    }

    let advected = pollster::block_on(particle_buffers.download_particles(&context))
        .expect("particle readback succeeds");
    assert_eq!(advected.len(), PARTICLE_COUNT);

    let end = mean_geo(&advected, &domain);
    let east_m = distance_meters(
        GeoCoord {
            lat: start.lat,
            lon: start.lon,
        },
        GeoCoord {
            lat: start.lat,
            lon: end.lon,
        },
    );
    let north_m = distance_meters(
        GeoCoord {
            lat: start.lat,
            lon: end.lon,
        },
        end,
    ) * (end.lat - start.lat).signum() as f32;
    let mean_z: f32 =
        advected.iter().map(|p| p.pos_z).sum::<f32>() / advected.len() as f32;

    let expected_east_m = U_WIND_MS * TOTAL_SECONDS;
    eprintln!("{TEST_ID}: start=({:.5}E, {:.5}N, {:.1}m)", start.lon, start.lat, START_Z_M);
    eprintln!("{TEST_ID}: end=({:.5}E, {:.5}N, {:.3}m)", end.lon, end.lat, mean_z);
    eprintln!("{TEST_ID}: east={east_m:.2}m expected={expected_east_m:.2}m north={north_m:.4}m");
    eprintln!(
        "{TEST_ID}: software_adapter={} (timings must not be used as GPU performance values)",
        context.is_software_adapter()
    );

    assert!(
        (east_m - expected_east_m).abs() <= EAST_DISPLACEMENT_TOLERANCE_M,
        "{TEST_ID}: eastward displacement out of tolerance: got {east_m:.2}m, expected {expected_east_m:.2}m"
    );
    assert!(
        north_m.abs() <= NORTH_DRIFT_TOLERANCE_M,
        "{TEST_ID}: unexpected north/south drift: {north_m:.4}m"
    );
    assert!(
        (mean_z - START_Z_M).abs() <= VERTICAL_DRIFT_TOLERANCE_M,
        "{TEST_ID}: unexpected vertical motion: mean_z={mean_z:.4}m"
    );

    // All particles see identical uniform wind, so the plume must not spread.
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    for particle in advected.iter().filter(|p| p.is_active()) {
        min_x = min_x.min(particle.grid_x());
        max_x = max_x.max(particle.grid_x());
    }
    assert!(
        (max_x - min_x) < 1.0e-3,
        "{TEST_ID}: uniform wind must not spread the plume in x: span={}",
        max_x - min_x
    );

    // Mass slots are untouched by pure advection.
    for particle in advected.iter().filter(|p| p.is_active()) {
        assert!(
            (particle.mass[0] - 1.0).abs() <= 1.0e-6,
            "{TEST_ID}: particle mass must be conserved without deposition"
        );
        assert!((particle.mass[1..]).iter().all(|m| *m == 0.0));
    }
}
