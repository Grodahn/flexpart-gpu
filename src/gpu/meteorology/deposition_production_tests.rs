//! Required-adapter #114 deposition production regression on the canonical #76/#171 step.
//!
//! The existing dry/wet deposition kernels sample no meteorology: they consume
//! externally supplied process coefficients (`vdep`, `wetscav`, `grfraction`) on
//! the canonical production particle transaction. This test locks that boundary:
//! deposition runs only inside an accepted canonical step, mass/probability
//! semantics are unchanged, the inactive tail is untouched, and an independent
//! canonical meteorology failure publishes neither deposited mass nor probabilities.
#![allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::float_cmp
)]
use super::advection_production_tests::{
    analytic_snapshot, bracket, gas_settling, runtime, START_SECONDS,
};
use super::snapshot_hash;
use crate::{
    config::ReleaseConfig,
    coords::GridDomain,
    gpu::GpuAdapterEvidence,
    particles::{Particle, MAX_SPECIES},
    simulation::{
        BackwardReceptorConfig, BackwardTimeLoopConfig, BackwardTimeLoopDriver,
        CanonicalMeteorologySlot, ForwardStepForcing, ForwardTimeLoopConfig, ForwardTimeLoopDriver,
        MetTimeBracket, ParticleForcingField,
    },
    wind::{uniform_wind_field, SurfaceFields, WindFieldGrid},
};
use approx::assert_relative_eq;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const CAPACITY: usize = 130;
const ACTIVE: usize = 67;
const RELEASE_HEIGHT_M: f64 = 5.0;
/// Existing `FLEXPART` `href` default carried by both driver configurations [m].
const HREF_M: f32 = 15.0;
const WET_LAMBDA_S_INV: f32 = 0.1;
const WET_FRACTION: f32 = 0.5;
/// Existing dry/wet kernel-module comparison bound; #114 adds no tolerance.
const KERNEL_RELATIVE: f32 = 1.0e-6;

fn dry_velocity_m_s(slot: usize) -> f32 {
    0.001 * (slot % 7 + 1) as f32
}

/// Per-slot dry velocities prove slot identity is preserved through the canonical step.
fn deposition_forcing(slot_count: usize) -> ForwardStepForcing {
    ForwardStepForcing {
        dry_deposition_velocity_m_s: vec![ParticleForcingField::PerParticle(
            (0..slot_count).map(dry_velocity_m_s).collect(),
        )],
        wet_scavenging_coefficient_s_inv: vec![ParticleForcingField::Uniform(WET_LAMBDA_S_INV)],
        wet_precipitating_fraction: ParticleForcingField::Uniform(WET_FRACTION),
        ..ForwardStepForcing::default()
    }
}

/// `dry_deposition.wgsl` and `wet_deposition.wgsl` expectations for one active lane.
fn expected_probabilities(slot: usize, height_m: f32, dt_s: f32) -> (f32, f32) {
    let dry = if height_m < 2.0 * HREF_M {
        1.0 - (-dry_velocity_m_s(slot) * dt_s / (2.0 * HREF_M)).exp()
    } else {
        0.0
    };
    let wet = WET_FRACTION * (1.0 - (-WET_LAMBDA_S_INV * dt_s).exp());
    (dry, wet)
}

fn shader_hashes() -> BTreeMap<&'static str, String> {
    ["dry_deposition", "wet_deposition", "advection_step_commit"]
        .into_iter()
        .map(|name| {
            let bytes = std::fs::read(format!("src/shaders/{name}.wgsl")).unwrap();
            (name, format!("{:x}", Sha256::digest(bytes)))
        })
        .collect()
}

struct Outcome {
    initial: Vec<Particle>,
    particles: Vec<Particle>,
    dry: Vec<[f32; MAX_SPECIES]>,
    wet: Vec<[f32; MAX_SPECIES]>,
    adapter: GpuAdapterEvidence,
    failed: bool,
    rejected_missing_canonical: bool,
    rejected_short_forcing: bool,
}

#[test]
fn test_production_deposition_consumes_coefficients_on_canonical_transaction() {
    let root = std::path::Path::new("target/ci-gate/deposition-meteorology");
    std::fs::create_dir_all(root).unwrap();
    let grid = GridDomain {
        xlon0: 6.0,
        ylat0: 50.0,
        dx: 0.25,
        dy: 0.25,
        nx: 8,
        ny: 4,
    };
    let legacy_wind = uniform_wind_field(
        &WindFieldGrid::new(
            8,
            4,
            3,
            3,
            3,
            0.25,
            0.25,
            6.0,
            50.0,
            ndarray::Array1::from_vec(vec![0.0, 1.0, 2.0]),
        ),
        1000.0,
        1000.0,
        1000.0,
    );
    let mut surface = SurfaceFields::zeros(8, 4);
    surface.surface_pressure_pa.fill(101_325.0);
    surface.temperature_2m_k.fill(290.0);
    surface.dewpoint_2m_k.fill(285.0);
    surface.mixing_height_m.fill(1000.0);
    let s0 = analytic_snapshot(START_SECONDS, true);
    let s1 = analytic_snapshot(START_SECONDS + 1, true);
    let r0 = runtime(&s0, "#173 declared synthetic geometry");
    let r1 = runtime(&s1, "#173 declared synthetic geometry");
    let forcing = deposition_forcing(CAPACITY);
    let short_forcing = deposition_forcing(CAPACITY - 1);
    let mut rows = Vec::new();
    let mut adapter_class = None;
    for backward in [false, true] {
        for failure in [None, Some((0, 0))] {
            let mut slot = CanonicalMeteorologySlot::default();
            let outcome = if backward {
                let config = BackwardTimeLoopConfig {
                    transport_settling: Some(gas_settling(1)),
                    start_timestamp: "20240101000001".into(),
                    end_timestamp: "20240101000000".into(),
                    timestep_seconds: 1,
                    receptors: vec![BackwardReceptorConfig {
                        species_masses_kg: None,
                        name: "probe".into(),
                        lon: 6.5625,
                        lat: 50.25,
                        z_m: RELEASE_HEIGHT_M,
                        particle_count: ACTIVE as u64,
                        mass_kg: ACTIVE as f64,
                    }],
                    ..BackwardTimeLoopConfig::default()
                };
                let mut driver =
                    pollster::block_on(BackwardTimeLoopDriver::new(config, grid.clone(), CAPACITY))
                        .expect("required backward WGSL adapter");
                driver.test_set_active_prefix(ACTIVE);
                let prepared = driver
                    .prepare_canonical_meteorology(
                        bracket(driver.meteorology_context(), [&s0, &s1], [&r0, &r1]),
                        &mut slot,
                    )
                    .unwrap();
                let adapter = GpuAdapterEvidence::from_context(prepared.context());
                let owners = std::sync::Arc::clone(prepared.resources());
                drop(prepared);
                let mut met = MetTimeBracket {
                    canonical: Some(&owners),
                    wind_t0: &legacy_wind,
                    wind_t1: &legacy_wind,
                    surface_t0: &surface,
                    surface_t1: &surface,
                    time_t0_seconds: START_SECONDS,
                    time_t1_seconds: START_SECONDS + 1,
                };
                let canonical = met.canonical.take();
                let rejected_missing_canonical =
                    pollster::block_on(driver.run_timestep(&met, &forcing)).is_err();
                met.canonical = canonical;
                let rejected_short_forcing =
                    pollster::block_on(driver.run_timestep(&met, &short_forcing)).is_err();
                let initial = pollster::block_on(driver.test_raw_particles());
                crate::gpu::advection_resident::TEST_QUERY_FAULT.with(|f| f.set(failure));
                let failed = pollster::block_on(driver.run_timestep(&met, &forcing)).is_err();
                let particles = pollster::block_on(driver.test_raw_particles());
                Outcome {
                    initial,
                    particles,
                    dry: Vec::new(),
                    wet: Vec::new(),
                    adapter,
                    failed,
                    rejected_missing_canonical,
                    rejected_short_forcing,
                }
            } else {
                let config = ForwardTimeLoopConfig {
                    transport_settling: Some(gas_settling(1)),
                    start_timestamp: "20240101000000".into(),
                    end_timestamp: "20240101000001".into(),
                    timestep_seconds: 1,
                    collect_deposition_probabilities_each_step: true,
                    ..ForwardTimeLoopConfig::default()
                };
                let releases = vec![ReleaseConfig {
                    name: "probe".into(),
                    start_time: "20240101000000".into(),
                    end_time: "20240101000000".into(),
                    lon: 6.6875,
                    lat: 50.25,
                    z_min: RELEASE_HEIGHT_M,
                    z_max: RELEASE_HEIGHT_M,
                    mass_kg: ACTIVE as f64,
                    particle_count: ACTIVE as u64,
                    species_masses_kg: None,
                    raw: BTreeMap::new(),
                }];
                let mut driver = pollster::block_on(ForwardTimeLoopDriver::new(
                    config,
                    &releases,
                    grid.clone(),
                    CAPACITY,
                ))
                .expect("required forward WGSL adapter");
                driver.test_set_active_prefix(ACTIVE);
                let prepared = driver
                    .prepare_canonical_meteorology(
                        bracket(driver.meteorology_context(), [&s0, &s1], [&r0, &r1]),
                        &mut slot,
                    )
                    .unwrap();
                let adapter = GpuAdapterEvidence::from_context(prepared.context());
                let owners = std::sync::Arc::clone(prepared.resources());
                drop(prepared);
                let mut met = MetTimeBracket {
                    canonical: Some(&owners),
                    wind_t0: &legacy_wind,
                    wind_t1: &legacy_wind,
                    surface_t0: &surface,
                    surface_t1: &surface,
                    time_t0_seconds: START_SECONDS,
                    time_t1_seconds: START_SECONDS + 1,
                };
                let canonical = met.canonical.take();
                let rejected_missing_canonical =
                    pollster::block_on(driver.run_timestep(&met, &forcing)).is_err();
                met.canonical = canonical;
                let rejected_short_forcing =
                    pollster::block_on(driver.run_timestep(&met, &short_forcing)).is_err();
                let initial = pollster::block_on(driver.test_raw_particles());
                crate::gpu::advection_resident::TEST_QUERY_FAULT.with(|f| f.set(failure));
                let result = pollster::block_on(driver.run_timestep(&met, &forcing));
                let particles = pollster::block_on(driver.test_raw_particles());
                let (failed, dry, wet) = match result {
                    Ok(report) => (
                        false,
                        report.dry_deposition_probability,
                        report.wet_deposition_probability,
                    ),
                    Err(_) => (true, Vec::new(), Vec::new()),
                };
                Outcome {
                    initial,
                    particles,
                    dry,
                    wet,
                    adapter,
                    failed,
                    rejected_missing_canonical,
                    rejected_short_forcing,
                }
            };
            assert!(
                outcome.rejected_missing_canonical,
                "deposition cannot run without #76 canonical meteorology"
            );
            assert!(
                outcome.rejected_short_forcing,
                "coefficient length must match particle capacity"
            );
            assert_eq!(outcome.failed, failure.is_some(), "backward={backward}");
            // Each probe lane carries 1 kg; the rejected preparations deposited nothing.
            assert!(outcome.initial[..ACTIVE].iter().all(|p| p.mass[0] == 1.0));
            let mut sampled = Vec::new();
            if failure.is_some() {
                assert_eq!(
                    bytemuck::cast_slice::<_, u8>(&outcome.particles),
                    bytemuck::cast_slice::<_, u8>(&outcome.initial),
                    "an independent canonical failure must publish no deposited mass"
                );
                assert!(outcome.dry.is_empty() && outcome.wet.is_empty());
            } else {
                for (slot, (after, before)) in
                    outcome.particles.iter().zip(&outcome.initial).enumerate()
                {
                    if slot >= ACTIVE {
                        assert_eq!(
                            bytemuck::bytes_of(after),
                            bytemuck::bytes_of(before),
                            "inactive tail slot {slot} must remain untouched"
                        );
                        continue;
                    }
                    let (dry, wet) = expected_probabilities(slot, after.pos_z, 1.0);
                    assert!(after.pos_z < 2.0 * HREF_M, "lane {slot} left the dry layer");
                    let expected_mass = before.mass[0] * (1.0 - dry) * (1.0 - wet);
                    assert_relative_eq!(
                        after.mass[0],
                        expected_mass,
                        max_relative = KERNEL_RELATIVE
                    );
                    assert_eq!(&after.mass[1..], &before.mass[1..]);
                    if !backward {
                        assert_relative_eq!(
                            outcome.dry[slot][0],
                            dry,
                            max_relative = KERNEL_RELATIVE
                        );
                        assert_relative_eq!(
                            outcome.wet[slot][0],
                            wet,
                            max_relative = KERNEL_RELATIVE
                        );
                        assert_eq!(&outcome.dry[slot][1..], &[0.0; MAX_SPECIES - 1]);
                        assert_eq!(&outcome.wet[slot][1..], &[0.0; MAX_SPECIES - 1]);
                    }
                    if slot < 3 {
                        sampled.push(json!({"slot":slot,"height_agl_m":after.pos_z,
                            "dry_velocity_m_s":dry_velocity_m_s(slot),"wet_scavenging_s_inv":WET_LAMBDA_S_INV,
                            "wet_precipitating_fraction":WET_FRACTION,"expected_dry_probability":dry,
                            "expected_wet_probability":wet,"mass_before_kg":before.mass[0],"mass_after_kg":after.mass[0]}));
                    }
                }
                if !backward {
                    assert!(outcome.dry[ACTIVE..]
                        .iter()
                        .all(|p| *p == [0.0; MAX_SPECIES]));
                    assert!(outcome.wet[ACTIVE..]
                        .iter()
                        .all(|p| *p == [0.0; MAX_SPECIES]));
                }
            }
            adapter_class.get_or_insert_with(|| {
                serde_json::to_value(outcome.adapter.adapter_class)
                    .unwrap()
                    .as_str()
                    .unwrap_or("unknown")
                    .to_owned()
            });
            rows.push(json!({"backward":backward,"capacity":CAPACITY,"active_count":ACTIVE,
                "failure":failure,"failed":outcome.failed,"atomic_preservation":failure.is_some(),
                "missing_canonical_rejected":outcome.rejected_missing_canonical,
                "short_forcing_rejected":outcome.rejected_short_forcing,
                "adapter":outcome.adapter,"sampled_lanes":sampled,
                "deposition_meteorology_inputs":"none: coefficients are externally supplied process inputs",
                "source_snapshot_sha256":[snapshot_hash(&s0).unwrap(),snapshot_hash(&s1).unwrap()]}));
        }
    }
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let class = adapter_class.unwrap_or_else(|| "unknown".into());
    std::fs::write(
        root.join(format!("report-{class}.json")),
        serde_json::to_vec_pretty(&json!({
            "schema":"flexpart-gpu.deposition-meteorology-production.v1",
            "issue":114,
            "candidate_revision":String::from_utf8(revision.stdout).unwrap().trim(),
            "validation_level":"production-path integration (forward and backward canonical drivers)",
            "flexpart_anchors":["advance_mod.f90 via drydepo_mod.f90:827-865 drydepo_probability",
                "wetdepo_mod.f90:17-202 wetdepo mass-loss form"],
            "migration_verdict":"NO_DEPOSITION_METEOROLOGY_CONSUMER_TO_MIGRATE",
            "scientific_verdict":"NO_DEPOSITION_PARITY_CLAIM",
            "shader_sha256":shader_hashes(),
            "rows":rows,
            "status":"passed"}))
        .unwrap(),
    )
    .unwrap();
    eprintln!("DEPOSITION-METEOROLOGY-114: canonical production deposition boundary and atomic failure passed on {class}");
}
