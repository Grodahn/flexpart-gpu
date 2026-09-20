//! Corpus candidate runner (Issue #6, point 2).
//!
//! Executes versioned corpus cases from `fixtures/corpus/cases/*.json`
//! through the productive WGSL path (`ForwardTimeLoopDriver`) or, for
//! `ADV-ANA-001`, through the isolated production WGSL advection kernel
//! (turbulence disabled by construction). Writes raw per-seed particle
//! outputs and machine-readable metrics under `target/corpus/candidate/`.
//!
//! Raw outputs are run artifacts, never references. Provenance (hashes,
//! revisions, build, adapter, seeds) is recorded by `scripts/run-corpus.sh`
//! in `target/corpus/run_manifest.json`.
//!
//! Usage:
//!   cargo run --release --bin corpus-run -- --case WIND-UNI-002 --seeds 10
//!   cargo run --release --bin corpus-run -- --all --seeds 2 --out-dir target/corpus/candidate

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use flexpart_gpu::config::ReleaseConfig;
use flexpart_gpu::coords::GridDomain;
use flexpart_gpu::gpu::{
    advect_particles_gpu_with_sampling, GpuContext, ParticleBuffers, WindBuffers,
    WindSamplingOptions,
};
use flexpart_gpu::validation::candidate_physics::CandidatePhysicsProfile;
use flexpart_gpu::particles::{Particle, ParticleInit};
use flexpart_gpu::physics::VelocityToGridScale;
use flexpart_gpu::simulation::{
    ForwardStepForcing, ForwardTimeLoopConfig, ForwardTimeLoopDriver, MetTimeBracket,
    ParticleForcingField,
};
use flexpart_gpu::validation::case::{
    CandidatePhiloxDerivation, ReleaseTiming, SourceGeometry, ValidationCaseManifest, WindSpec,
};
use flexpart_gpu::wind::{SurfaceFields, WindField3D, WindFieldGrid};
use ndarray::Array1;
use serde::Serialize;

const R_EARTH_M: f64 = 6_371_000.0;

const DRIVER_CASES: &[&str] = &[
    "WIND-UNI-002",
    "WIND-SHEAR-003",
    "PBL-STABLE-004",
    "PBL-NEUTRAL-005",
    "PBL-UNSTABLE-006",
    "DRY-007",
    "WET-008",
    "REPEAT-009",
];

#[derive(Serialize)]
struct ParticleRecord {
    lon_deg: f64,
    lat_deg: f64,
    z_m: f32,
    mass_kg: f64,
}

/// Tolerance for the deposited-mass budget closure, reusing the versioned
/// mass-conservation bound (`fixtures/corpus/thresholds.json`,
/// `mass_conservation_rel`): advection and turbulence conserve mass up to
/// f32 summation order, so airborne + deposited reservoirs must recover the
/// initial mass within the same bound.
const BUDGET_CLOSURE_TOLERANCE_REL: f64 = 1.0e-5;

#[derive(Serialize)]
struct SeedOutput {
    case_id: String,
    seed_index: u32,
    philox_key: [u32; 2],
    philox_counter: [u32; 4],
    adapter: String,
    is_software_adapter: bool,
    candidate_revision: String,
    particle_count: usize,
    active_particles: usize,
    particles: Vec<ParticleRecord>,
    metrics: CandidateMetrics,
    /// Cumulative dry-deposited mass [kg] over the run, summed per step from
    /// the driver-reported per-slot removal probabilities applied to the
    /// pre-step slot masses (dry step runs before the wet step, so wet
    /// removal applies to the post-dry mass). Zero when dry deposition is off.
    deposited_dry_kg: f64,
    /// Cumulative wet-deposited mass [kg], accounted like dry deposition.
    /// Zero when wet deposition is off.
    deposited_wet_kg: f64,
    /// Relative budget closure error:
    /// |airborne + deposited_dry + deposited_wet - initial| / initial.
    budget_closure_rel_error: f64,
    /// `closed` when the error is within BUDGET_CLOSURE_TOLERANCE_REL,
    /// otherwise `open` (never silently passed as conserved).
    budget_status: String,
}

#[derive(Serialize)]
struct CandidateMetrics {
    total_mass_kg: f64,
    initial_mass_kg: f64,
    mass_conservation_rel_error: f64,
    com_lon_deg: f64,
    com_lat_deg: f64,
    com_z_m: f64,
    cov_east_m2: f64,
    cov_north_m2: f64,
    cov_z_m2: f64,
    cov_east_north_m2: f64,
    cov_east_z_m2: f64,
    cov_north_z_m2: f64,
    horizontal_eigenvalues_m2: [f64; 2],
    z_min_m: f64,
    z_p10_m: f64,
    z_p50_m: f64,
    z_p90_m: f64,
    z_max_m: f64,
    z_mean_m: f64,
    z_std_m: f64,
}

fn parse_args() -> (Vec<String>, Option<usize>, PathBuf, PathBuf) {
    let mut cases: Vec<String> = Vec::new();
    let mut all = false;
    let mut seeds: Option<usize> = None;
    let mut out_dir = PathBuf::from("target/corpus/candidate");
    let mut fixtures = PathBuf::from("fixtures/corpus/cases");
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--case" => {
                let value = args.next().expect("--case needs a value");
                cases.push(value);
            }
            "--all" => all = true,
            "--seeds" => {
                let value = args.next().expect("--seeds needs a value");
                let parsed: usize = value.parse().expect("seeds must be a number");
                seeds = Some(parsed);
            }
            "--out-dir" => {
                out_dir = PathBuf::from(args.next().expect("--out-dir needs a value"));
            }
            "--fixtures" => {
                fixtures = PathBuf::from(args.next().expect("--fixtures needs a value"));
            }
            other => panic!("unknown argument: {other}"),
        }
    }
    if all {
        cases = DRIVER_CASES.iter().map(|s| s.to_string()).collect();
        cases.push("ADV-ANA-001".to_string());
    }
    if cases.is_empty() {
        panic!("specify --case <ID> (repeatable) or --all");
    }
    (cases, seeds, out_dir, fixtures)
}

fn candidate_revision() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Load the canonical v2 manifest.
///
/// Only `schema_version` 2 is accepted. Legacy v1 documents are frozen and
/// unsupported (see `fixtures/corpus/cases/MIGRATION_NOTES.md`); they are
/// rejected fail-closed with a version error, never silently adapted.
fn load_case_manifest(fixture_path: &Path, text: &str) -> Result<ValidationCaseManifest, String> {
    ValidationCaseManifest::parse(text, fixture_path)
        .map_err(|e| format!("invalid case manifest {}: {e}", fixture_path.display()))
}

/// Resolve the candidate Philox key/counter for one seed.
///
/// The canonical typed manifest is the only source. Stochastic cases without
/// an identity are rejected before any GPU execution. No default key is ever
/// substituted. Seed/repeat semantics come from the manifest's typed,
/// versioned `candidate_philox.derivation`, never from hard-coded case IDs.
fn resolve_candidate_seed(
    case_id: &str,
    manifest: &ValidationCaseManifest,
    seed_index: u32,
) -> Result<([u32; 2], [u32; 4]), String> {
    manifest
        .candidate_seed_identity(seed_index)
        .map_err(|e| format!("case {case_id}: {e}"))
}

/// Resolve how many seeds to run.
///
/// The declared manifest count is authoritative. An explicit `--seeds N`
/// selects a leading subset (`N <= declared`) for quick checks; it can
/// never silently expand the ensemble or override the derivation.
fn resolve_ensemble_count(
    case_id: &str,
    manifest: &ValidationCaseManifest,
    cli_seeds: Option<usize>,
) -> Result<usize, String> {
    let declared = if let Some(candidate) = &manifest.stochastic.candidate_philox {
        usize::try_from(candidate.count)
            .map_err(|_| format!("case {case_id}: candidate_philox.count out of range"))?
    } else if manifest.physics_switches.turbulence {
        return Err(format!(
            "case {case_id}: physics_switches.turbulence=true requires stochastic.candidate_philox"
        ));
    } else {
        1
    };
    if declared == 0 {
        return Err(format!("case {case_id}: declared ensemble count must be > 0"));
    }
    match cli_seeds {
        None => Ok(declared),
        Some(0) => Err(format!("case {case_id}: --seeds must be > 0")),
        Some(requested) if requested > declared => Err(format!(
            "case {case_id}: --seeds {requested} exceeds declared ensemble count {declared}; refusing to invent seeds"
        )),
        Some(requested) => Ok(requested),
    }
}

fn end_timestamp(start: &str, total_s: i64) -> Result<String, String> {
    let start_seconds = parse_timestamp_seconds(start)?;
    format_timestamp_seconds(
        start_seconds
            .checked_add(total_s)
            .ok_or_else(|| format!("timestamp overflow for {start} + {total_s}s"))?,
    )
}

fn parse_timestamp_seconds(value: &str) -> Result<i64, String> {
    if value.len() != 14 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{value:?} must be YYYYMMDDHHMMSS"));
    }
    let part = |start: usize, end: usize| -> Result<u32, String> {
        value[start..end]
            .parse::<u32>()
            .map_err(|_| format!("invalid timestamp component in {value:?}"))
    };
    let year = part(0, 4)?;
    let month = part(4, 6)?;
    let day = part(6, 8)?;
    let hour = part(8, 10)?;
    let minute = part(10, 12)?;
    let second = part(12, 14)?;
    let leap = |y: u32| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    if year == 0 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return Err(format!("invalid Gregorian timestamp {value:?}"));
    }
    let month_days = [
        31_u32,
        if leap(year) { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    if day == 0 || day > month_days[(month - 1) as usize] {
        return Err(format!("invalid Gregorian timestamp {value:?}"));
    }
    Ok(days_from_civil(year as i32, month, day) * 86_400
        + i64::from(hour) * 3_600
        + i64::from(minute) * 60
        + i64::from(second))
}

fn format_timestamp_seconds(seconds: i64) -> Result<String, String> {
    let days = seconds.div_euclid(86_400);
    let sod = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(1..=9999).contains(&year) {
        return Err(format!("timestamp out of range: {seconds}"));
    }
    let hour = sod / 3_600;
    let minute = (sod % 3_600) / 60;
    let second = sod % 60;
    Ok(format!(
        "{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}"
    ))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let mut y = i64::from(year);
    let m = i64::from(month);
    let d = i64::from(day);
    if m <= 2 {
        y -= 1;
    }
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    if m <= 2 {
        y += 1;
    }
    (y as i32, m as u32, d as u32)
}

fn velocity_scale(
    lat_deg: f64,
    dx_deg: f64,
    dy_deg: f64,
    heights: &[f32],
) -> VelocityToGridScale {
    let lat_rad = lat_deg * std::f64::consts::PI / 180.0;
    let dx_m = R_EARTH_M * lat_rad.cos() * (dx_deg * std::f64::consts::PI / 180.0);
    let dy_m = R_EARTH_M * (dy_deg * std::f64::consts::PI / 180.0);
    let mut level_heights_m = [0.0_f32; 16];
    for (i, h) in heights.iter().take(16).enumerate() {
        level_heights_m[i] = *h;
    }
    VelocityToGridScale {
        x_grid_per_meter: (1.0 / dx_m) as f32,
        y_grid_per_meter: (1.0 / dy_m) as f32,
        z_grid_per_meter: 1.0,
        level_heights_m,
    }
}

fn build_wind(
    nx: usize,
    ny: usize,
    nz: usize,
    manifest: &ValidationCaseManifest,
    profile: &CandidatePhysicsProfile,
) -> Result<WindField3D, String> {
    let mut field = WindField3D::zeros(nx, ny, nz);
    match &manifest.wind {
        WindSpec::Uniform { u_m_s, v_m_s, w_m_s } => {
            field.u_ms.fill(*u_m_s);
            field.v_ms.fill(*v_m_s);
            field.w_ms.fill(*w_m_s);
        }
        WindSpec::LinearShear {
            u0_m_s,
            u_shear_per_s,
            v_m_s,
            w_m_s,
        } => {
            for k in 0..nz {
                let z = manifest
                    .domain
                    .wind_heights_m
                    .get(k)
                    .copied()
                    .ok_or_else(|| format!("case {}: missing wind height {k}", manifest.case_id))?;
                let u = *u0_m_s + *u_shear_per_s * z;
                for i in 0..nx {
                    for j in 0..ny {
                        field.u_ms[[i, j, k]] = u;
                        field.v_ms[[i, j, k]] = *v_m_s;
                        field.w_ms[[i, j, k]] = *w_m_s;
                    }
                }
            }
        }
        WindSpec::RealWeather { .. } => {
            return Err(format!(
                "case {}: real-weather cases are not executed by corpus-run",
                manifest.case_id
            ));
        }
    }
    let met = profile.synthetic_meteorology;
    field.temperature_k.fill(met.temperature_k);
    field.specific_humidity.fill(met.specific_humidity);
    field.pressure_pa.fill(met.pressure_pa);
    field.air_density_kg_m3.fill(met.air_density_kg_m3);
    field.density_gradient_kg_m2.fill(met.density_gradient_kg_m2);
    Ok(field)
}

fn build_surface(
    nx: usize,
    ny: usize,
    manifest: &ValidationCaseManifest,
) -> Result<SurfaceFields, String> {
    let spec = manifest.surface.as_ref().ok_or_else(|| {
        format!(
            "case {}: synthetic driver requires explicit surface fields",
            manifest.case_id
        )
    })?;
    let mut surface = SurfaceFields::zeros(nx, ny);
    surface.surface_pressure_pa.fill(spec.surface_pressure_pa);
    surface.u10_ms.fill(spec.u10_m_s);
    surface.v10_ms.fill(spec.v10_m_s);
    surface.temperature_2m_k.fill(spec.temperature_2m_k);
    surface.dewpoint_2m_k.fill(spec.dewpoint_2m_k);
    surface.precip_large_scale_mm_h.fill(spec.precip_large_scale_mm_h);
    surface.precip_convective_mm_h.fill(spec.precip_convective_mm_h);
    surface.sensible_heat_flux_w_m2.fill(spec.sensible_heat_flux_w_m2);
    surface.solar_radiation_w_m2.fill(spec.solar_radiation_w_m2);
    surface.surface_stress_n_m2.fill(spec.surface_stress_n_m2);
    surface.friction_velocity_ms.fill(spec.friction_velocity_m_s);
    surface
        .convective_velocity_scale_ms
        .fill(spec.convective_velocity_scale_m_s);
    surface.mixing_height_m.fill(spec.mixing_height_m);
    surface.tropopause_height_m.fill(spec.tropopause_height_m);
    surface
        .inv_obukhov_length_per_m
        .fill(spec.inv_obukhov_length_per_m);
    Ok(surface)
}

fn forcing_for_case(manifest: &ValidationCaseManifest) -> ForwardStepForcing {
    let (dry, wet_lambda, wet_frac) = manifest.deposition.as_ref().map_or(
        (0.0, 0.0, 0.0),
        |deposition| {
            (
                deposition.dry_deposition_velocity_m_s,
                deposition.wet_scavenging_coefficient_s_inv,
                deposition.wet_precipitating_fraction,
            )
        },
    );
    ForwardStepForcing {
        dry_deposition_velocity_m_s: vec![ParticleForcingField::Uniform(dry)],
        wet_scavenging_coefficient_s_inv: vec![ParticleForcingField::Uniform(wet_lambda)],
        wet_precipitating_fraction: ParticleForcingField::Uniform(wet_frac),
        decay_constant_s_inv: vec![0.0],
        rho_grad_over_rho: 0.0,
    }
}

fn candidate_dry_reference_height_m(
    manifest: &ValidationCaseManifest,
    profile: &CandidatePhysicsProfile,
) -> Result<f32, String> {
    if manifest.physics_switches.dry_deposition {
        return manifest
            .deposition
            .as_ref()
            .and_then(|d| d.dry_reference_height_m)
            .ok_or_else(|| {
                format!(
                    "case {}: dry deposition requires deposition.dry_reference_height_m",
                    manifest.case_id
                )
            });
    }
    Ok(profile.inactive_dry_reference_height_m)
}

fn compute_metrics(
    records: &[ParticleRecord],
    initial_mass: f64,
    xlon0: f64,
    ylat0: f64,
) -> CandidateMetrics {
    let n = records.len() as f64;
    let total: f64 = records.iter().map(|r| r.mass_kg).sum();
    let com_lon = records.iter().map(|r| r.lon_deg).sum::<f64>() / n;
    let com_lat = records.iter().map(|r| r.lat_deg).sum::<f64>() / n;
    let com_z = records.iter().map(|r| r.z_m as f64).sum::<f64>() / n;
    // Meters relative to center latitude for covariance.
    let lat_rad = com_lat * std::f64::consts::PI / 180.0;
    let m_per_deg_lon = R_EARTH_M * lat_rad.cos() * std::f64::consts::PI / 180.0;
    let m_per_deg_lat = R_EARTH_M * std::f64::consts::PI / 180.0;
    let east: Vec<f64> = records
        .iter()
        .map(|r| (r.lon_deg - com_lon) * m_per_deg_lon)
        .collect();
    let north: Vec<f64> = records
        .iter()
        .map(|r| (r.lat_deg - com_lat) * m_per_deg_lat)
        .collect();
    let up: Vec<f64> = records.iter().map(|r| r.z_m as f64 - com_z).collect();
    let var = |v: &[f64]| v.iter().map(|x| x * x).sum::<f64>() / n;
    let cov = |a: &[f64], b: &[f64]| a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f64>() / n;
    let c_ee = var(&east);
    let c_nn = var(&north);
    let c_zz = var(&up);
    let c_en = cov(&east, &north);
    // Eigenvalues of the 2x2 horizontal covariance.
    let trace = c_ee + c_nn;
    let det = c_ee * c_nn - c_en * c_en;
    let disc = (trace * trace - 4.0 * det).max(0.0).sqrt();
    let eig = [(trace + disc) / 2.0, (trace - disc) / 2.0];
    let mut zs: Vec<f64> = records.iter().map(|r| r.z_m as f64).collect();
    zs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let quantile = |p: f64| {
        let pos = p * (zs.len() - 1) as f64;
        let lo = pos.floor() as usize;
        let hi = pos.ceil() as usize;
        zs[lo] + (zs[hi] - zs[lo]) * (pos - lo as f64)
    };
    let mean_z = com_z;
    let std_z = c_zz.sqrt();
    let _ = (xlon0, ylat0);
    CandidateMetrics {
        total_mass_kg: total,
        initial_mass_kg: initial_mass,
        mass_conservation_rel_error: if initial_mass > 0.0 {
            (total - initial_mass).abs() / initial_mass
        } else {
            0.0
        },
        com_lon_deg: com_lon,
        com_lat_deg: com_lat,
        com_z_m: com_z,
        cov_east_m2: c_ee,
        cov_north_m2: c_nn,
        cov_z_m2: c_zz,
        cov_east_north_m2: c_en,
        cov_east_z_m2: cov(&east, &up),
        cov_north_z_m2: cov(&north, &up),
        horizontal_eigenvalues_m2: eig,
        z_min_m: zs[0],
        z_p10_m: quantile(0.10),
        z_p50_m: quantile(0.50),
        z_p90_m: quantile(0.90),
        z_max_m: zs[zs.len() - 1],
        z_mean_m: mean_z,
        z_std_m: std_z,
    }
}

fn run_advective_case(
    case_id: &str,
    case: &serde_json::Value,
    out_dir: &Path,
    revision: &str,
) -> Result<(), String> {
    let domain = &case["domain"];
    let nx = domain["nx"].as_u64().unwrap_or(64) as usize;
    let ny = domain["ny"].as_u64().unwrap_or(64) as usize;
    let nz = domain["nz"].as_u64().unwrap_or(8) as usize;
    let heights: Vec<f32> = domain["wind_heights_m"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
        .unwrap_or_else(|| vec![0.0; nz]);
    let release = &case["release"];
    // Normalized point geometry; the isolated advection kernel has no
    // box support, so non-point sources fail closed here.
    let geometry = release.get("geometry").ok_or_else(|| {
        format!("case {case_id}: missing release.geometry")
    })?;
    if geometry.get("kind").and_then(|v| v.as_str()) != Some("point") {
        return Err(format!(
            "case {case_id}: advective path requires a point release geometry"
        ));
    }
    let start_lon = geometry.get("lon_deg").and_then(|v| v.as_f64()).ok_or_else(|| {
        format!("case {case_id}: release.geometry.lon_deg missing or not a number")
    })?;
    let start_lat = geometry.get("lat_deg").and_then(|v| v.as_f64()).ok_or_else(|| {
        format!("case {case_id}: release.geometry.lat_deg missing or not a number")
    })?;
    let start_z = geometry.get("z_m").and_then(|v| v.as_f64()).ok_or_else(|| {
        format!("case {case_id}: release.geometry.z_m missing or not a number")
    })? as f32;
    let count = release["particle_count"].as_u64().ok_or_else(|| {
        format!("case {case_id}: release.particle_count missing or not a number")
    })? as usize;
    let u = case["wind"]["u_m_s"].as_f64().unwrap_or(10.0) as f32;
    let dt = case["integration"]["dt_s"].as_f64().unwrap_or(60.0) as f32;
    let steps = case["integration"]["steps"].as_u64().unwrap_or(60) as usize;
    let xlon0 = domain["xlon0_deg"].as_f64().unwrap_or(6.0);
    let ylat0 = domain["ylat0_deg"].as_f64().unwrap_or(47.0);
    let dx = domain["dx_deg"].as_f64().unwrap_or(0.1);
    let dy = domain["dy_deg"].as_f64().unwrap_or(0.1);

    let context =
        pollster::block_on(GpuContext::new()).map_err(|e| format!("no WGSL adapter: {e}"))?;
    let adapter = format!(
        "{} backend={:?} type={:?}",
        context.device_name(),
        context.backend(),
        context.adapter_type()
    );
    let software = context.is_software_adapter();

    let grid_domain = GridDomain {
        xlon0,
        ylat0,
        dx,
        dy,
        nx,
        ny,
    };
    let lat_rad = (start_lat as f32).to_radians();
    let dx_m = R_EARTH_M * f64::from(lat_rad.cos()) * (dx * std::f64::consts::PI / 180.0);
    let dy_m = R_EARTH_M * (dy * std::f64::consts::PI / 180.0);
    let mut level_heights_m = [0.0_f32; 16];
    for (i, h) in heights.iter().take(16).enumerate() {
        level_heights_m[i] = *h;
    }
    let scale = VelocityToGridScale {
        x_grid_per_meter: (1.0 / dx_m) as f32,
        y_grid_per_meter: (1.0 / dy_m) as f32,
        z_grid_per_meter: 1.0,
        level_heights_m,
    };
    let mut field = WindField3D::zeros(nx, ny, nz);
    field.u_ms.fill(u);
    let wind_buffers =
        WindBuffers::from_field(&context, &field).map_err(|e| format!("wind upload: {e}"))?;
    let start_gx = (start_lon - xlon0) / dx;
    let start_gy = (start_lat - ylat0) / dy;
    let particles: Vec<Particle> = (0..count)
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
    for _ in 0..steps {
        advect_particles_gpu_with_sampling(
            &context,
            &particle_buffers,
            &wind_buffers,
            dt,
            scale,
            sampling,
        )
        .map_err(|e| format!("advection dispatch: {e}"))?;
    }
    let back = pollster::block_on(particle_buffers.download_particles(&context))
        .map_err(|e| format!("readback: {e}"))?;
    let records: Vec<ParticleRecord> = back
        .iter()
        .filter(|p| p.is_active())
        .map(|p| {
            let gx = p.grid_x();
            let gy = p.grid_y();
            ParticleRecord {
                lon_deg: gx * dx + xlon0,
                lat_deg: gy * dy + ylat0,
                z_m: p.pos_z,
                mass_kg: f64::from(p.mass[0]),
            }
        })
        .collect();
    let initial = count as f64;
    let metrics = compute_metrics(&records, initial, xlon0, ylat0);
    // Pure advection moves no mass between reservoirs.
    let adv_closure = metrics.mass_conservation_rel_error;
    let budget_status = if adv_closure <= BUDGET_CLOSURE_TOLERANCE_REL {
        "closed"
    } else {
        "open"
    }
    .to_string();
    let output = SeedOutput {
        case_id: case_id.to_string(),
        seed_index: 0,
        philox_key: [0, 0],
        philox_counter: [0, 0, 0, 0],
        adapter: adapter.clone(),
        is_software_adapter: software,
        candidate_revision: revision.to_string(),
        particle_count: count,
        active_particles: records.len(),
        particles: records,
        metrics,
        deposited_dry_kg: 0.0,
        deposited_wet_kg: 0.0,
        budget_closure_rel_error: adv_closure,
        budget_status,
    };
    let case_dir = out_dir.join(case_id);
    fs::create_dir_all(&case_dir).map_err(|e| format!("mkdir: {e}"))?;
    let path = case_dir.join("seed_000.json");
    fs::write(&path, serde_json::to_string_pretty(&output).unwrap())
        .map_err(|e| format!("write: {e}"))?;
    eprintln!("{case_id}: wrote {} (adapter: {adapter})", path.display());
    let _ = grid_domain;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn run_driver_case(
    case_id: &str,
    manifest: &ValidationCaseManifest,
    profile: &CandidatePhysicsProfile,
    seed_index: u32,
    out_dir: &Path,
    revision: &str,
) -> Result<(), String> {
    let domain = &manifest.domain;
    let nx = usize::try_from(domain.nx).map_err(|_| format!("case {case_id}: domain.nx out of range"))?;
    let ny = usize::try_from(domain.ny).map_err(|_| format!("case {case_id}: domain.ny out of range"))?;
    let nz = usize::try_from(domain.nz).map_err(|_| format!("case {case_id}: domain.nz out of range"))?;
    let heights = domain.wind_heights_m.clone();
    let release = &manifest.release;
    let (lon, lat, z_min, z_max) = match &release.geometry {
        SourceGeometry::Point { lon_deg, lat_deg, z_m } => (
            f64::from(*lon_deg), f64::from(*lat_deg), f64::from(*z_m), f64::from(*z_m)),
        SourceGeometry::Box { lon_min_deg, lon_max_deg, lat_min_deg, lat_max_deg, z_min_m, z_max_m } => {
            if lon_min_deg != lon_max_deg || lat_min_deg != lat_max_deg {
                return Err(format!("case {case_id}: box lon/lat ranges are not supported by the candidate ReleaseConfig (only vertical ranges)"));
            }
            (*lon_min_deg, *lat_min_deg, f64::from(*z_min_m), f64::from(*z_max_m))
        }
    };
    let count = u64::from(release.particle_count);
    let mass_total = f64::from(release.inventory.quantity_kg);
    let integration = &manifest.integration;
    let start = integration.start.as_str();
    if integration.dt_s.fract() != 0.0 || integration.total_s.fract() != 0.0 {
        return Err(format!("case {case_id}: integration dt_s/total_s must be whole integer seconds"));
    }
    let dt = integration.dt_s as i64;
    let total_s = integration.total_s as i64;
    let end = end_timestamp(start, total_s)?;
    let xlon0 = f64::from(domain.xlon0_deg);
    let ylat0 = f64::from(domain.ylat0_deg);
    let dx = f64::from(domain.dx_deg);
    let dy = f64::from(domain.dy_deg);

    // Fail-closed Philox resolution happens before any GPU work below.
    // Identical-repeat semantics come from the manifest flag, never from
    // hard-coded case IDs.
    let (key, counter) = resolve_candidate_seed(case_id, manifest, seed_index)?;

    let release_grid = GridDomain { xlon0, ylat0, dx, dy, nx, ny };
    let release_at = match &release.timing {
        ReleaseTiming::Instant { at } => at.as_str(),
        ReleaseTiming::Window { .. } => {
            return Err(format!("case {case_id}: corpus-run candidate driver supports only instant releases"));
        }
    };
    let releases = vec![ReleaseConfig {
        name: case_id.to_string(),
        start_time: release_at.to_string(),
        end_time: release_at.to_string(),
        lon, lat, z_min, z_max,
        mass_kg: mass_total,
        particle_count: count,
        species_masses_kg: None,
        raw: BTreeMap::new(),
    }];
    let config = ForwardTimeLoopConfig {
        start_timestamp: start.to_string(),
        end_timestamp: end.clone(),
        timestep_seconds: dt,
        time_bounds_behavior: profile.time_bounds_behavior(),
        velocity_to_grid_scale: velocity_scale(lat, dx, dy, &heights),
        pbl_options: profile.pbl_options(),
        dry_reference_height_m: candidate_dry_reference_height_m(manifest, profile)?,
        philox_key: key,
        initial_philox_counter: counter,
        spatial_sort: None,
        sync_particle_store_each_step: true,
        collect_deposition_probabilities_each_step: true,
    };
    // Probe the adapter that the driver will select (same env-driven
    // selection as ForwardTimeLoopDriver::new). The driver owns its context
    // privately, so record the probe identity as run provenance.
    let probe =
        pollster::block_on(GpuContext::new()).map_err(|e| format!("no WGSL adapter: {e}"))?;
    let adapter = format!(
        "{} backend={:?} type={:?}",
        probe.device_name(),
        probe.backend(),
        probe.adapter_type()
    );
    let software = probe.is_software_adapter();
    drop(probe);
    let mut driver = pollster::block_on(ForwardTimeLoopDriver::new(
        config,
        &releases,
        release_grid,
        count as usize,
    ))
    .map_err(|e| format!("driver init: {e}"))?;

    let wind = build_wind(nx, ny, nz, manifest, profile)?;
    let surface = build_surface(nx, ny, manifest)?;
    let grid = WindFieldGrid::new(
        nx, ny, nz, nz, nz, dx as f32, dy as f32, xlon0 as f32, ylat0 as f32,
        Array1::from_vec(heights.clone()),
    );
    let _ = grid;
    let start_secs = driver.current_time_seconds();
    let end_secs = driver.end_time_seconds();
    if end_secs <= start_secs {
        return Err(format!("case {case_id}: candidate met bracket requires end > start, got {start_secs}..{end_secs}"));
    }
    let met = MetTimeBracket {
        wind_t0: &wind, wind_t1: &wind, surface_t0: &surface, surface_t1: &surface,
        time_t0_seconds: start_secs, time_t1_seconds: end_secs,
    };
    let forcing = forcing_for_case(manifest);
    // Step the driver manually so per-process deposited reservoirs can be
    // accumulated from the reported per-slot removal probabilities. The
    // driver applies dry deposition before wet deposition within each step,
    // therefore wet removal is charged against the post-dry mass. Probability
    // vectors are slot-aligned with the particle store; corpus cases neither
    // deactivate particles nor enable compaction, so slot indices are stable
    // across steps (asserted below).
    let mut deposited_dry_kg = 0.0_f64;
    let mut deposited_wet_kg = 0.0_f64;
    pollster::block_on(async {
        while driver.has_remaining_steps() {
            let pre_masses: Vec<f64> = driver
                .particle_store()
                .as_slice()
                .iter()
                .filter(|p| p.is_active())
                .map(|p| f64::from(p.mass[0]))
                .collect();
            let pre_active = pre_masses.len();
            let report = driver.run_timestep(&met, &forcing).await.map_err(|e| format!("run_timestep: {e}"))?;
            let post_active = driver
                .particle_store()
                .as_slice()
                .iter()
                .filter(|p| p.is_active())
                .count();
            // The release manager injects all corpus particles during the
            // first step, so the store is empty beforehand; releases carry a
            // uniform per-particle mass of mass_total/count.
            let pre_masses = if pre_active == 0 && post_active > 0 {
                vec![mass_total / post_active as f64; post_active]
            } else {
                pre_masses
            };
            if post_active != pre_masses.len() {
                return Err(format!(
                    "particle sink during {case_id} seed {seed_index}: {} -> {post_active} active",
                    pre_masses.len(),
                ));
            }
            let dry_probs = report.dry_deposition_probability;
            let wet_probs = report.wet_deposition_probability;
            for (slot, pre) in pre_masses.iter().enumerate() {
                let p_dry = dry_probs.get(slot).map_or(0.0, |lanes| lanes[0]) as f64;
                let p_wet = wet_probs.get(slot).map_or(0.0, |lanes| lanes[0]) as f64;
                let removed_dry = pre * p_dry;
                let removed_wet = (pre - removed_dry) * p_wet;
                deposited_dry_kg += removed_dry;
                deposited_wet_kg += removed_wet;
            }
            let _ = report;
        }
        driver.finalize().await.map_err(|e| format!("finalize: {e}"))?;
        Ok::<(), String>(())
    })?;
    let store = driver.particle_store();
    let records: Vec<ParticleRecord> = store
        .as_slice()
        .iter()
        .filter(|p| p.is_active())
        .map(|p| {
            let gx = p.grid_x();
            let gy = p.grid_y();
            ParticleRecord {
                lon_deg: gx * dx + xlon0,
                lat_deg: gy * dy + ylat0,
                z_m: p.pos_z,
                mass_kg: f64::from(p.mass[0]),
            }
        })
        .collect();
    if records.is_empty() {
        return Err("no active particles after run".to_string());
    }
    let metrics = compute_metrics(&records, mass_total, xlon0, ylat0);
    let budget_closure_rel_error = if mass_total > 0.0 {
        (metrics.total_mass_kg + deposited_dry_kg + deposited_wet_kg - mass_total).abs() / mass_total
    } else {
        0.0
    };
    let budget_status = if budget_closure_rel_error <= BUDGET_CLOSURE_TOLERANCE_REL {
        "closed"
    } else {
        "open"
    }
    .to_string();
    let output = SeedOutput {
        case_id: case_id.to_string(),
        seed_index,
        philox_key: key,
        philox_counter: counter,
        adapter: adapter.clone(),
        is_software_adapter: software,
        candidate_revision: revision.to_string(),
        particle_count: count as usize,
        active_particles: records.len(),
        particles: records,
        metrics,
        deposited_dry_kg,
        deposited_wet_kg,
        budget_closure_rel_error,
        budget_status,
    };
    let case_dir = out_dir.join(case_id);
    fs::create_dir_all(&case_dir).map_err(|e| format!("mkdir: {e}"))?;
    let path = case_dir.join(format!("seed_{seed_index:03}.json"));
    fs::write(&path, serde_json::to_string(&output).unwrap()).map_err(|e| format!("write: {e}"))?;
    eprintln!(
        "{case_id} seed {seed_index}: {} active, air {:.6} kg, dry-dep {:.6} kg, wet-dep {:.6} kg, closure {:.2e} ({}) (adapter: {adapter})",
        output.active_particles,
        output.metrics.total_mass_kg,
        output.deposited_dry_kg,
        output.deposited_wet_kg,
        output.budget_closure_rel_error,
        output.budget_status,
    );
    Ok(())
}

fn main() {
    env_logger::init();
    let (cases, cli_seeds, out_dir, fixtures) = parse_args();
    let revision = candidate_revision();
    fs::create_dir_all(&out_dir).expect("create output directory");
    for case_id in &cases {
        let fixture_path = fixtures.join(format!("{case_id}.json"));
        let text = fs::read_to_string(&fixture_path)
            .unwrap_or_else(|_| panic!("read fixture {}", fixture_path.display()));
        // Canonical v2 manifest first: legacy v1 is rejected fail-closed.
        let manifest = load_case_manifest(&fixture_path, &text)
            .unwrap_or_else(|e| panic!("{e}"));
        if manifest.case_id != *case_id {
            panic!(
                "fixture {} declares case_id {}, expected {case_id}",
                fixture_path.display(),
                manifest.case_id
            );
        }
        let candidate_profile = CandidatePhysicsProfile::load(&manifest.candidate_physics_profile)
            .unwrap_or_else(|e| panic!("{}: {e}", manifest.case_id));
        // ADV-ANA-001 still uses the isolated advection kernel; its raw JSON is
        // parsed only for that legacy kernel adapter. Driver cases consume the
        // typed manifest exclusively.
        let case: serde_json::Value = serde_json::from_str(&text).expect("parse case fixture");
        if case_id == "ADV-ANA-001" {
            run_advective_case(case_id, &case, &out_dir, &revision).expect("advective case failed");
        } else {
            // Declared ensemble count is authoritative; --seeds may only
            // select a leading subset and is rejected before any GPU work.
            let run_seeds = resolve_ensemble_count(case_id, &manifest, cli_seeds)
                .unwrap_or_else(|e| panic!("{e}"));
            for seed in 0..run_seeds as u32 {
                run_driver_case(
                    case_id, &manifest, &candidate_profile, seed, &out_dir, &revision,
                )
                    .unwrap_or_else(|e| panic!("{case_id} seed {seed} failed: {e}"));
            }
        }
    }
    eprintln!(
        "corpus-run complete: {} case(s) under {}",
        cases.len(),
        out_dir.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_path(case_id: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join(format!("{case_id}.json"))
    }

    fn load_manifest(case_id: &str) -> ValidationCaseManifest {
        let path = fixture_path(case_id);
        let text = fs::read_to_string(&path).expect("read fixture");
        load_case_manifest(&path, &text).expect("v2 manifest")
    }

    #[test]
    fn wind_uni_002_seed_zero_uses_declared_key() {
        let manifest = load_manifest("WIND-UNI-002");
        let candidate = manifest
            .stochastic
            .candidate_philox
            .as_ref()
            .expect("declared identity");
        assert_eq!(candidate.base_key, [3737180555, 305419896]);
        let (key, counter) =
            resolve_candidate_seed("WIND-UNI-002", &manifest, 0).expect("seed 0");
        assert_eq!(key, [3737180555, 305419896]);
        assert_eq!(counter, [0, 0, 0, 0]);
    }

    #[test]
    fn wind_uni_002_seed_n_uses_wrapping_derivation() {
        let manifest = load_manifest("WIND-UNI-002");
        let (key, _) =
            resolve_candidate_seed("WIND-UNI-002", &manifest, 7).expect("seed 7");
        assert_eq!(key, [3737180555u32.wrapping_add(7), 305419896]);
    }

    #[test]
    fn runner_uses_declared_derivation_policy() {
        let mut manifest = load_manifest("WIND-UNI-002");
        let candidate = manifest
            .stochastic
            .candidate_philox
            .as_mut()
            .expect("declared identity");
        candidate.derivation = CandidatePhiloxDerivation::ReuseBaseIdentityV1;
        let a = resolve_candidate_seed("WIND-UNI-002", &manifest, 0).expect("seed 0");
        let b = resolve_candidate_seed("WIND-UNI-002", &manifest, 7).expect("seed 7");
        assert_eq!(a, b, "runner must execute the declared derivation policy");
    }

    #[test]
    fn stochastic_case_without_identity_is_rejected_before_execution() {
        let manifest = load_manifest("WIND-UNI-002");
        let mut hacked = manifest.clone();
        hacked.stochastic.candidate_philox = None;
        hacked.stochastic.oracle_seed = None;
        let err = resolve_candidate_seed("WIND-UNI-002", &hacked, 0).expect_err("must fail");
        assert!(err.contains("no stochastic"), "unexpected: {err}");
        let err = resolve_ensemble_count("WIND-UNI-002", &hacked, None).expect_err("must fail");
        assert!(err.contains("ensemble count"), "unexpected: {err}");
    }

    #[test]
    fn deterministic_ensemble_count_is_not_case_id_specific() {
        let mut manifest = load_manifest("ADV-ANA-001");
        manifest.case_id = "ARBITRARY-DETERMINISTIC-123".to_string();
        assert!(manifest.stochastic.candidate_philox.is_none());
        assert!(!manifest.physics_switches.turbulence);
        assert_eq!(
            resolve_ensemble_count("ARBITRARY-DETERMINISTIC-123", &manifest, None)
                .expect("deterministic case"),
            1
        );
    }

    #[test]
    fn repeat_009_reuses_identical_key() {
        let manifest = load_manifest("REPEAT-009");
        let candidate = manifest
            .stochastic
            .candidate_philox
            .as_ref()
            .expect("REPEAT-009 declares an identity");
        assert_eq!(
            candidate.derivation,
            CandidatePhiloxDerivation::ReuseBaseIdentityV1
        );
        assert_eq!(candidate.count, 2);
        let (key0, _) = resolve_candidate_seed("REPEAT-009", &manifest, 0).expect("seed 0");
        let (key1, _) = resolve_candidate_seed("REPEAT-009", &manifest, 1).expect("seed 1");
        assert_eq!(key0, key1);
        assert_eq!(key0, [3737180555, 305419896]);
        let count = resolve_ensemble_count("REPEAT-009", &manifest, None).expect("count");
        assert_eq!(count, 2);
    }

    #[test]
    fn candidate_runner_uses_manifest_dry_reference_height() {
        let mut manifest = load_manifest("DRY-007");
        let profile = CandidatePhysicsProfile::load(&manifest.candidate_physics_profile).expect("profile");
        manifest.deposition.as_mut().expect("dry deposition").dry_reference_height_m = Some(23.0);
        assert_eq!(candidate_dry_reference_height_m(&manifest, &profile).expect("href"), 23.0);
    }

    #[test]
    fn velocity_scale_uses_declared_grid_spacing() {
        let heights = [0.0_f32, 100.0];
        let fine = velocity_scale(50.0, 0.1, 0.1, &heights);
        let coarse = velocity_scale(50.0, 0.2, 0.25, &heights);
        assert!(coarse.x_grid_per_meter < fine.x_grid_per_meter);
        assert!(coarse.y_grid_per_meter < fine.y_grid_per_meter);
    }

    #[test]
    fn end_timestamp_is_not_hard_coded_to_2024_01_01() {
        assert_eq!(end_timestamp("20240228235900", 120).expect("timestamp"), "20240229000100");
    }

    #[test]
    fn shear_surface_wind_is_explicit_and_preserved() {
        let manifest = load_manifest("WIND-SHEAR-003");
        let surface = manifest.surface.as_ref().expect("surface");
        assert_eq!(surface.u10_m_s, 5.0);
        assert_eq!(surface.v10_m_s, 0.0);
    }

    #[test]
    fn legacy_v1_document_is_rejected() {
        let path = fixture_path("WIND-UNI-002");
        let legacy = r#"{"version": 1, "case_id": "WIND-UNI-002"}"#;
        let err = load_case_manifest(&path, legacy).expect_err("v1 must fail");
        assert!(err.contains("SchemaVersionMismatch") || err.contains("schema"), "unexpected: {err}");
    }

    #[test]
    fn cli_seeds_cannot_exceed_declared_ensemble() {
        let manifest = load_manifest("WIND-UNI-002");
        // Declared count is 10.
        let full = resolve_ensemble_count("WIND-UNI-002", &manifest, None)
            .expect("declared");
        assert_eq!(full, 10);
        // Explicit subset is allowed.
        let subset =
            resolve_ensemble_count("WIND-UNI-002", &manifest, Some(2))
                .expect("subset");
        assert_eq!(subset, 2);
        // Silent expansion is rejected.
        let err = resolve_ensemble_count("WIND-UNI-002", &manifest, Some(11))
            .expect_err("must fail");
        assert!(err.contains("exceeds declared"), "unexpected: {err}");
    }

    #[test]
    fn adv_ana_001_needs_no_rng_identity() {
        let manifest = load_manifest("ADV-ANA-001");
        assert!(manifest.stochastic.candidate_philox.is_none());
        manifest.validate().expect("ADV-ANA-001 stays valid");
        let err = resolve_candidate_seed("ADV-ANA-001", &manifest, 0)
            .expect_err("deterministic case must not resolve a seed");
        assert!(err.contains("no stochastic"), "unexpected: {err}");
    }
}
