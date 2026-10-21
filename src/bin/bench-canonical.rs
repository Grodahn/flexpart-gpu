//! Issue #199 instrumentation of the actual canonical production driver.
#![warn(clippy::all, clippy::pedantic)]

use anyhow::{ensure, Context, Result};
use flexpart_gpu::{
    config::ReleaseConfig,
    coords::{dx_meters_at_latitude, GridDomain},
    gpu::{GpuAdapterClass, GpuAdapterEvidence},
    meteorology::{
        vertical::{
            reconstruct_vertical_geometry_with_motion, NativeVerticalMotion,
            NativeVerticalMotionKind, NativeVerticalMotionProvenance, NativeVerticalMotionSign,
            NativeVerticalMotionUnit, VerticalTransformResult,
        },
        FieldId, Snapshot, TemporalKind, VerticalStaggering, FIELD_SPECS,
    },
    particles::Particle,
    physics::VelocityToGridScale,
    simulation::{
        CanonicalMeteorologyBracket, CanonicalMeteorologySlot, ForwardStepForcing,
        ForwardTimeLoopConfig, ForwardTimeLoopDriver, MetTimeBracket,
    },
    wind::{SurfaceFields, WindField3D},
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc, time::Instant};

const START_SECONDS: i64 = 1_704_067_200;
const COVERAGE_SECONDS: i64 = 86_400;
const MAX_WARMUP_STEPS: usize = 1_000;
const PARTICLE_MASS_KG: f32 = 1.0;

#[derive(Deserialize)]
struct Scenario {
    steps: usize,
    dt_seconds: i64,
    source_snapshot: String,
    nx: usize,
    ny: usize,
    xlon0_deg: f64,
    ylat0_deg: f64,
    dx_deg: f32,
    dy_deg: f32,
    u_m_s: f32,
    v_m_s: f32,
    w_m_s: f32,
    release_lon_deg: f64,
    release_lat_deg: f32,
    release_z_m: f64,
    mass_kg_per_particle: f64,
    surface: SurfaceInput,
    philox_key: [u32; 2],
    initial_philox_counter: [u32; 4],
}

#[derive(Deserialize)]
struct SurfaceInput {
    surface_pressure_pa: f32,
    temperature_2m_k: f32,
    dewpoint_2m_k: f32,
    mixing_height_m: f32,
    friction_velocity_m_s: f32,
    tropopause_height_m: f32,
    all_other_fields: f32,
}

fn benchmark_surface(s: &Scenario) -> Result<SurfaceFields> {
    ensure!(
        s.surface.all_other_fields == 0.0,
        "unconfigured surface fields must be zero"
    );
    let mut surface = SurfaceFields::zeros(s.nx, s.ny);
    surface
        .surface_pressure_pa
        .fill(s.surface.surface_pressure_pa);
    surface.temperature_2m_k.fill(s.surface.temperature_2m_k);
    surface.dewpoint_2m_k.fill(s.surface.dewpoint_2m_k);
    surface.mixing_height_m.fill(s.surface.mixing_height_m);
    surface
        .friction_velocity_ms
        .fill(s.surface.friction_velocity_m_s);
    surface
        .tropopause_height_m
        .fill(s.surface.tropopause_height_m);
    Ok(surface)
}

fn source(s: &Scenario, seconds: i64) -> Result<(Snapshot, VerticalTransformResult)> {
    let mut snapshot: Snapshot = serde_json::from_slice(&std::fs::read(&s.source_snapshot)?)?;
    ensure!(snapshot.horizontal_grid.nx == 1 && snapshot.horizontal_grid.ny == 1);
    snapshot.horizontal_grid.nx = s.nx;
    snapshot.horizontal_grid.ny = s.ny;
    snapshot.horizontal_grid.xlon0_deg = s.xlon0_deg;
    snapshot.horizontal_grid.ylat0_deg = s.ylat0_deg;
    snapshot.horizontal_grid.dx_deg = f64::from(s.dx_deg);
    snapshot.horizontal_grid.dy_deg = f64::from(s.dy_deg);
    let cells = s.nx.checked_mul(s.ny).context("grid overflow")?;
    for field in &mut snapshot.fields {
        field.shape[0] = s.nx;
        field.shape[1] = s.ny;
        field.values = field.values.iter().flat_map(|v| vec![*v; cells]).collect();
        if field.time.kind != TemporalKind::Static {
            field.time.valid_time_epoch_seconds = seconds;
        }
    }
    let template = snapshot
        .fields
        .iter()
        .find(|f| f.id == FieldId::Temperature)
        .context("temperature template")?
        .clone();
    for (id, value) in [
        (FieldId::WindU, s.u_m_s),
        (FieldId::WindV, s.v_m_s),
        (FieldId::VerticalVelocity, s.w_m_s),
    ] {
        let mut field = template.clone();
        let spec = FIELD_SPECS
            .iter()
            .find(|f| f.id == id)
            .context("field spec")?;
        field.id = id;
        field.unit = spec.unit;
        field.sign = spec.sign;
        field.values.fill(value);
        snapshot.fields.push(field);
    }
    let motion = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::GeometricVelocity,
        unit: NativeVerticalMotionUnit::MeterPerSecond,
        sign: NativeVerticalMotionSign::PositiveUpward,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values: vec![s.w_m_s; template.values.len()],
        provenance: NativeVerticalMotionProvenance {
            source_id: "BENCH-EARLY-11.1-v1".into(),
        },
    };
    let runtime = reconstruct_vertical_geometry_with_motion(&snapshot, &motion)?;
    Ok((snapshot, runtime))
}

fn validate_particles(
    particles: &[Particle],
    count: usize,
    top: f32,
    s: &Scenario,
) -> Result<Value> {
    ensure!(particles.len() == count, "particle capacity differs");
    let max_x = f64::from(u32::try_from(s.nx - 1)?);
    let max_y = f64::from(u32::try_from(s.ny - 1)?);
    let mut z_min = f32::INFINITY;
    let mut z_max = f32::NEG_INFINITY;
    let mut mass = 0.0_f64;
    for p in particles {
        ensure!(p.is_active(), "inactive particle");
        ensure!(
            [p.pos_x, p.pos_y, p.pos_z, p.turb_u, p.turb_v, p.turb_w]
                .iter()
                .all(|v| v.is_finite()),
            "nonfinite state"
        );
        ensure!(
            p.pos_z > 0.0 && p.pos_z < top,
            "unsupported vertical boundary contact"
        );
        ensure!(
            p.grid_x() > 0.0 && p.grid_x() < max_x && p.grid_y() > 0.0 && p.grid_y() < max_y,
            "horizontal boundary contact"
        );
        // No process changes mass in this diagnostic; exact equality adds no tolerance.
        ensure!(
            p.mass.map(f32::to_bits) == [PARTICLE_MASS_KG, 0.0, 0.0, 0.0].map(f32::to_bits),
            "mass changed"
        );
        mass += f64::from(p.mass[0]);
        z_min = z_min.min(p.pos_z);
        z_max = z_max.max(p.pos_z);
    }
    Ok(
        json!({"active_particles":count,"mass_kg":mass,"min_height_m":z_min,
        "max_height_m":z_max,"top_m":top,"finite":true,"no_boundary_contact":true}),
    )
}

async fn create_driver(
    s: &Scenario,
    snapshot: &Snapshot,
    count: usize,
) -> Result<ForwardTimeLoopDriver> {
    let grid = &snapshot.horizontal_grid;
    let release = ReleaseConfig {
        name: "BENCH-EARLY-11.1".into(),
        start_time: "20240101000000".into(),
        end_time: "20240101000000".into(),
        lon: s.release_lon_deg,
        lat: f64::from(s.release_lat_deg),
        z_min: s.release_z_m,
        z_max: s.release_z_m,
        mass_kg: s.mass_kg_per_particle * f64::from(u32::try_from(count)?),
        particle_count: u64::try_from(count)?,
        species_masses_kg: None,
        raw: std::collections::BTreeMap::default(),
    };
    Ok(ForwardTimeLoopDriver::new(
        ForwardTimeLoopConfig {
            start_timestamp: "20240101000000".into(),
            end_timestamp: "20240101235900".into(),
            timestep_seconds: s.dt_seconds,
            velocity_to_grid_scale: VelocityToGridScale {
                x_grid_per_meter: 1.0 / (dx_meters_at_latitude(s.release_lat_deg) * s.dx_deg),
                y_grid_per_meter: 1.0 / (dx_meters_at_latitude(0.0) * s.dy_deg),
                ..VelocityToGridScale::IDENTITY
            },
            sync_particle_store_each_step: true,
            collect_deposition_probabilities_each_step: false,
            philox_key: s.philox_key,
            initial_philox_counter: s.initial_philox_counter,
            ..ForwardTimeLoopConfig::default()
        },
        &[release],
        GridDomain {
            xlon0: grid.xlon0_deg,
            ylat0: grid.ylat0_deg,
            dx: grid.dx_deg,
            dy: grid.dy_deg,
            nx: s.nx,
            ny: s.ny,
        },
        count,
    )
    .await?)
}

async fn warmup_driver(
    driver: &mut ForwardTimeLoopDriver,
    met: &MetTimeBracket<'_>,
    forcing: &ForwardStepForcing,
    s: &Scenario,
    count: usize,
    top: f32,
    warmup_seconds: f64,
) -> Result<(usize, Option<f64>, f64)> {
    let warm = Instant::now();
    let mut warmup_steps = 0;
    let mut cold_step_seconds = None;
    while warm.elapsed().as_secs_f64() < warmup_seconds || warmup_steps < 2 {
        ensure!(
            warmup_steps < MAX_WARMUP_STEPS,
            "warmup exceeds coverage budget"
        );
        let step = Instant::now();
        driver.run_timestep(met, forcing).await?;
        cold_step_seconds.get_or_insert(step.elapsed().as_secs_f64());
        validate_particles(driver.particle_store().as_slice(), count, top, s)?;
        warmup_steps += 1;
    }
    let warmup_wall_seconds = warm.elapsed().as_secs_f64();
    Ok((warmup_steps, cold_step_seconds, warmup_wall_seconds))
}

struct MeasuredRun {
    timings: Vec<Value>,
    invariants: Value,
    measured_seconds: f64,
    audit_seconds: f64,
    measured_start_epoch_seconds: i64,
}

async fn measure_driver(
    driver: &mut ForwardTimeLoopDriver,
    met: &MetTimeBracket<'_>,
    forcing: &ForwardStepForcing,
    s: &Scenario,
    count: usize,
    top: f32,
) -> Result<MeasuredRun> {
    let measured_start_epoch_seconds = driver.current_time_seconds();
    let measurement = Instant::now();
    let mut timings = Vec::new();
    let mut audit_seconds = 0.0;
    let mut invariants = Value::Null;
    for _ in 0..s.steps {
        let step = Instant::now();
        let report = driver.run_timestep(met, forcing).await?;
        let seconds = step.elapsed().as_secs_f64();
        let audit = Instant::now();
        invariants = validate_particles(driver.particle_store().as_slice(), count, top, s)?;
        audit_seconds += audit.elapsed().as_secs_f64();
        let phases = report.timing.map(|t| {
            json!({"surface_interpolation_ms":t.surf_interp_ms,
            "pbl_preparation_upload_ms":t.pbl_ms + t.pbl_upload_ms,
            "forcing_ms":t.forcing_ms,"deposition_upload_ms":t.dep_upload_ms,
            "previous_wait_ms":t.wait_prev_gpu_ms,"encode_submit_ms":t.gpu_encode_ms,
            "submit_to_poll_ms":t.gpu_exec_ms,"reported_total_ms":t.total_ms})
        });
        timings.push(json!({"wall_seconds":seconds,"phases":phases}));
    }
    driver.finalize().await?;
    let measured_seconds = measurement.elapsed().as_secs_f64();
    Ok(MeasuredRun {
        timings,
        invariants,
        measured_seconds,
        audit_seconds,
        measured_start_epoch_seconds,
    })
}

fn canonical_top(
    view: flexpart_gpu::meteorology::vertical::VerticalRuntimeView<'_>,
) -> Result<f32> {
    Ok((0..view.dimensions().2)
        .map(|k| view.level(0, 0, k).map(|p| p.height_agl_m))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .fold(f32::NEG_INFINITY, f32::max))
}

async fn run(manifest: &Path, count: usize, output: &Path) -> Result<()> {
    let total = Instant::now();
    let count_f64 = f64::from(u32::try_from(count)?);
    let manifest_bytes = std::fs::read(manifest)?;
    let document: Value = serde_json::from_slice(&manifest_bytes)?;
    let s: Scenario = serde_json::from_value(document["candidate"].clone())?;
    let warmup_seconds = document["warmup_seconds"]
        .as_f64()
        .context("warmup_seconds")?;
    ensure!(count > 0 && s.steps > 0 && s.nx > 1 && s.ny > 1 && s.dt_seconds > 0);
    ensure!(warmup_seconds.is_finite() && warmup_seconds >= 2.0);
    ensure!(
        s.w_m_s == 0.0,
        "Stage A requires zero mean W to audit both transport stages"
    );
    ensure!(
        s.mass_kg_per_particle.to_bits() == f64::from(PARTICLE_MASS_KG).to_bits(),
        "Stage A requires one kg per particle"
    );
    let steps_f64 = f64::from(u32::try_from(s.steps)?);
    let dt_f64 = f64::from(i32::try_from(s.dt_seconds)?);
    let prep = Instant::now();
    let (s0, r0) = source(&s, START_SECONDS)?;
    let (s1, r1) = source(&s, START_SECONDS + COVERAGE_SECONDS)?;
    let view = r0.runtime_view()?;
    let top = canonical_top(view)?;
    let source_seconds = prep.elapsed().as_secs_f64();
    let init = Instant::now();
    let mut driver = create_driver(&s, &s0, count).await?;
    let init_seconds = init.elapsed().as_secs_f64();
    let upload = Instant::now();
    let mut slot = CanonicalMeteorologySlot::default();
    let prepared = driver.prepare_canonical_meteorology(
        CanonicalMeteorologyBracket::new([&s0, &s1], [r0.runtime_view()?, r1.runtime_view()?])?,
        &mut slot,
    )?;
    let adapter = GpuAdapterEvidence::from_context(prepared.context());
    adapter.validate()?;
    ensure!(
        adapter.adapter_class == GpuAdapterClass::HardwareGpu,
        "performance requires hardware GPU: {adapter:?}"
    );
    let owners = Arc::clone(prepared.resources());
    let identity = serde_json::to_value(owners.bracket().identity())?;
    drop(prepared);
    let upload_seconds = upload.elapsed().as_secs_f64();
    let wind = WindField3D::zeros(s.nx, s.ny, 3);
    let surface = benchmark_surface(&s)?;
    let met = MetTimeBracket {
        canonical: Some(&owners),
        wind_t0: &wind,
        wind_t1: &wind,
        surface_t0: &surface,
        surface_t1: &surface,
        time_t0_seconds: START_SECONDS,
        time_t1_seconds: START_SECONDS + COVERAGE_SECONDS,
    };
    let forcing = ForwardStepForcing::default();
    let (warmup_steps, cold_step_seconds, warmup_wall_seconds) =
        warmup_driver(&mut driver, &met, &forcing, &s, count, top, warmup_seconds).await?;
    let MeasuredRun {
        timings,
        invariants,
        measured_seconds,
        audit_seconds,
        measured_start_epoch_seconds,
    } = measure_driver(&mut driver, &met, &forcing, &s, count, top).await?;
    let output_start = Instant::now();
    let particle_path = output.with_extension("particles.bin");
    let bytes = bytemuck::cast_slice(driver.particle_store().as_slice());
    std::fs::write(&particle_path, bytes)?;
    let particle_hash = format!("{:x}", Sha256::digest(bytes));
    let output_seconds = output_start.elapsed().as_secs_f64();
    let report = json!({"schema":"flexpart-gpu.canonical-benchmark-run.v1",
        "regime":"canonical-production","calculation_path":"wgsl_device","adapter":adapter,
        "particles":count,"measured_steps":s.steps,"dt_seconds":s.dt_seconds,
        "measured_start_epoch_seconds":measured_start_epoch_seconds,
        "measured_end_epoch_seconds":driver.current_time_seconds(),
        "warmup_steps":warmup_steps,"warmup_wall_seconds":warmup_wall_seconds,
        "cold_first_step_seconds":cold_step_seconds,"driver_init_seconds":init_seconds,
        "meteorology_source_seconds":source_seconds,"canonical_prepare_upload_seconds":upload_seconds,
        "canonical_identity":identity,"normalized_sources":[s0,s1],
        "configuration":document["candidate"],"horizontal_scale":"existing dx_meters_at_latitude at fixed release latitude and equator for x/y",
        "measured_wall_seconds":measured_seconds,"invariant_audit_seconds":audit_seconds,
        "particle_steps_per_second":count_f64 * steps_f64 / measured_seconds,
        "simulated_time_wall_ratio":steps_f64 * dt_f64 / measured_seconds,
        "steps":timings,"invariants":invariants,"particle_output_sha256":particle_hash,
        "particle_output_bytes":bytes.len(),"output_write_and_hash_seconds":output_seconds,
        "host_transfers":"release and canonical uploads; full particle D2H each step; resident status readbacks; no intermediate wind D2H",
        "particle_buffer_bytes_lower_bound":3 * count * Particle::GPU_SIZE,
        "vram_peak_bytes":null,"memory_peak_bytes":null,"gpu_timestamp_seconds":null,
        "sync_mode":"strict per-step host readback; submit_to_poll is host wall time, not GPU timestamps",
        "manifest_sha256":format!("{:x}",Sha256::digest(&manifest_bytes)),
        "end_to_end_before_report_seconds":total.elapsed().as_secs_f64(),
        "scientific_verdict":"NON_COMPARABLE","speedup":null});
    std::fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "usage: bench-canonical MANIFEST PARTICLES OUTPUT.json"
    );
    for (key, value) in [
        ("FLEXPART_GPU_PROFILE", "1"),
        ("FLEXPART_GPU_PBL_CPU", "0"),
        ("FLEXPART_GPU_VALIDATION", "0"),
        ("FLEXPART_GPU_COMPACTION", "0"),
        ("FLEXPART_GPU_SOFTWARE", "0"),
    ] {
        ensure!(
            std::env::var(key).as_deref() == Ok(value),
            "{key} must be explicitly {value}"
        );
    }
    pollster::block_on(run(
        Path::new(&args[1]),
        args[2].parse()?,
        Path::new(&args[3]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flexpart_gpu::particles::ParticleInit;

    #[test]
    fn test_benchmark_invariants_reject_boundary_mass_and_nonfinite_state() {
        let document: Value =
            serde_json::from_str(include_str!("../../fixtures/benchmarks/early-11.1-v1.json"))
                .expect("versioned benchmark manifest");
        let scenario: Scenario = serde_json::from_value(document["candidate"].clone())
            .expect("versioned candidate scenario");
        let original = Particle::new(&ParticleInit {
            cell_x: 16,
            cell_y: 16,
            pos_x: 0.0,
            pos_y: 0.0,
            pos_z: 100.0,
            mass: [1.0, 0.0, 0.0, 0.0],
            release_point: 0,
            class: 0,
            time: 0,
        });
        assert!(validate_particles(&[original], 1, 5000.0, &scenario).is_ok());
        for change in 0..5 {
            let mut invalid = original;
            match change {
                0 => invalid.pos_z = 5000.0,
                1 => invalid.pos_z = 0.0,
                2 => invalid.mass[0] = 0.5,
                3 => invalid.turb_u = f32::NAN,
                _ => invalid.cell_x = 31,
            }
            assert!(validate_particles(&[invalid], 1, 5000.0, &scenario).is_err());
        }
        assert!(validate_particles(&[original], 2, 5000.0, &scenario).is_err());
    }
}
