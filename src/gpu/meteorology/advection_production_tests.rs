//! Required-adapter production-driver tests. Source setup reuses the #173 declared canonical fixture.
#![allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
#[allow(clippy::wildcard_imports)]
use super::*;
use crate::{
    config::ReleaseConfig,
    coords::GridDomain,
    gpu::GpuAdapterEvidence,
    meteorology::{
        vertical::{
            reconstruct_vertical_geometry_with_motion, NativeVerticalMotion,
            NativeVerticalMotionKind, NativeVerticalMotionProvenance, NativeVerticalMotionSign,
            NativeVerticalMotionUnit, VerticalTransformResult,
        },
        TemporalKind,
    },
    simulation::{
        BackwardReceptorConfig, BackwardTimeLoopConfig, BackwardTimeLoopDriver,
        CanonicalMeteorologyBracket, CanonicalMeteorologySlot, ForwardStepForcing,
        ForwardTimeLoopConfig, ForwardTimeLoopDriver, MetTimeBracket,
    },
    wind::{uniform_wind_field, SurfaceFields, WindFieldGrid},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
const START_SECONDS: i64 = 1_704_067_200;
fn snapshot(seconds: i64, offset: f32) -> Snapshot {
    let mut snapshot: Snapshot = serde_json::from_str(include_str!(
        "../../../fixtures/vertical/synthetic-column-v1.json"
    ))
    .unwrap();
    snapshot.horizontal_grid.nx = 8;
    snapshot.horizontal_grid.ny = 4;
    for field in &mut snapshot.fields {
        field.shape[0] = 8;
        field.shape[1] = 4;
        field.values = field.values.iter().flat_map(|v| [*v; 32]).collect();
        if field.time.kind != TemporalKind::Static {
            field.time.valid_time_epoch_seconds = seconds;
        }
    }
    let template = snapshot
        .fields
        .iter()
        .find(|f| f.id == FieldId::Temperature)
        .unwrap()
        .clone();
    for (id, value) in [
        (FieldId::WindU, 1.0 + offset),
        (FieldId::WindV, 2.0 + offset),
        (FieldId::VerticalVelocity, 0.25 + offset / 4.0),
    ] {
        let mut field = template.clone();
        field.id = id;
        let spec = crate::meteorology::FIELD_SPECS
            .iter()
            .find(|spec| spec.id == id)
            .unwrap();
        field.unit = spec.unit;
        field.sign = spec.sign;
        field.values.fill(value);
        snapshot.fields.push(field);
    }
    snapshot
}

fn native_motion(snapshot: &Snapshot, source_id: &str) -> NativeVerticalMotion {
    let values = snapshot
        .fields
        .iter()
        .find(|f| f.id == FieldId::VerticalVelocity)
        .unwrap()
        .values
        .clone();
    NativeVerticalMotion {
        kind: NativeVerticalMotionKind::GeometricVelocity,
        unit: NativeVerticalMotionUnit::MeterPerSecond,
        sign: NativeVerticalMotionSign::PositiveUpward,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values,
        provenance: NativeVerticalMotionProvenance {
            source_id: source_id.into(),
        },
    }
}

fn runtime(snapshot: &Snapshot, source_id: &str) -> VerticalTransformResult {
    reconstruct_vertical_geometry_with_motion(snapshot, &native_motion(snapshot, source_id))
        .unwrap()
}

fn bracket<'a>(
    s: [&'a Snapshot; 2],
    r: [&'a VerticalTransformResult; 2],
) -> CanonicalMeteorologyBracket<'a> {
    CanonicalMeteorologyBracket::new(s, r.map(|r| r.runtime_view().unwrap())).unwrap()
}

fn analytic_snapshot(seconds: i64, varying: bool) -> Snapshot {
    let mut s = snapshot(seconds, 0.0);
    for f in &mut s.fields {
        for (index, value) in f.values.iter_mut().enumerate() {
            match f.id {
                FieldId::WindU => {
                    *value = if varying {
                        0.2 * (index % 8) as f32 + 0.1 * (seconds - START_SECONDS) as f32
                    } else {
                        0.5
                    }
                }
                FieldId::WindV => *value = 0.125,
                FieldId::VerticalVelocity => *value = 0.25,
                _ => (),
            }
        }
    }
    s
}

#[test]
fn test_production_forward_backward_resident_petterssen_atomicity() {
    let root = std::path::Path::new("target/ci-gate/resident-advection-production");
    std::fs::create_dir_all(root).unwrap();
    let report_path = root.join("report.json");
    let _ = std::fs::remove_file(&report_path);
    let mut rows = Vec::new();
    let validation = std::env::var("FLEXPART_GPU_VALIDATION").unwrap_or_else(|_| "0".into());
    let compaction = std::env::var("FLEXPART_GPU_COMPACTION").unwrap_or_else(|_| "0".into());
    for backward in [false, true] {
        for varying in [false, true] {
            for count in [1, 3, 130] {
                for failure in [None, Some((0, 0)), Some((1, 1)), Some((2, 1))] {
                    let s0 = analytic_snapshot(START_SECONDS, varying);
                    let s1 = analytic_snapshot(START_SECONDS + 1, varying);
                    let r0 = runtime(&s0, "#173 declared synthetic geometry");
                    let r1 = runtime(&s1, "#173 declared synthetic geometry");
                    let input = json!({"snapshot_json":[serde_json::to_string(&s0).unwrap(),serde_json::to_string(&s1).unwrap()],
                        "native_motion_json":[serde_json::to_string(&native_motion(&s0,"#173 declared synthetic geometry")).unwrap(),
                        serde_json::to_string(&native_motion(&s1,"#173 declared synthetic geometry")).unwrap()],"runtimes":[&r0,&r1]});
                    let input_bytes = serde_json::to_vec(&input).unwrap();
                    let input_file = format!("source-varying-{varying}.json");
                    std::fs::write(root.join(&input_file), &input_bytes).unwrap();
                    let input_hash = format!("{:x}", Sha256::digest(&input_bytes));
                    let source = bracket([&s0, &s1], [&r0, &r1]);
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
                    let forcing = ForwardStepForcing {
                        decay_constant_s_inv: vec![0.1],
                        wet_scavenging_coefficient_s_inv: vec![
                            crate::simulation::ParticleForcingField::Uniform(0.1),
                        ],
                        wet_precipitating_fraction:
                            crate::simulation::ParticleForcingField::Uniform(0.5),
                        ..ForwardStepForcing::default()
                    };
                    let mut slot = CanonicalMeteorologySlot::default();
                    let (particles, status, metadata, values, queries, initial, adapter, failed) =
                        if backward {
                            let mut config = BackwardTimeLoopConfig {
                                start_timestamp: "20240101000001".into(),
                                end_timestamp: "20240101000000".into(),
                                timestep_seconds: 1,
                                receptors: vec![BackwardReceptorConfig {
                                    name: "probe".into(),
                                    lon: 6.5625,
                                    lat: 50.25,
                                    z_m: 100.0,
                                    particle_count: count as u64,
                                    mass_kg: 1.0,
                                }],
                                ..BackwardTimeLoopConfig::default()
                            };
                            if failure.is_some() && count > 1 {
                                let mut unaffected = config.receptors[0].clone();
                                unaffected.lon = 7.3125;
                                unaffected.particle_count = (count - 1) as u64;
                                unaffected.mass_kg = (count - 1) as f64 / count as f64;
                                config.receptors[0].particle_count = 1;
                                config.receptors[0].mass_kg = 1.0 / count as f64;
                                config.receptors.push(unaffected);
                            }
                            let mut driver =
                                pollster::block_on(BackwardTimeLoopDriver::new(config, grid, 130))
                                    .expect("required backward WGSL adapter");
                            driver.test_set_active_prefix(count);
                            let prepared = driver
                                .prepare_canonical_meteorology(source, &mut slot)
                                .unwrap();
                            let adapter = GpuAdapterEvidence::from_context(prepared.context());
                            let owners = std::sync::Arc::clone(prepared.resources());
                            poison(&owners, prepared.context(), failure, true);
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
                            let canonical_owner = met.canonical.take();
                            assert!(
                                pollster::block_on(driver.run_timestep(&met, &forcing)).is_err(),
                                "production cannot use legacy fallback"
                            );
                            met.canonical = canonical_owner;
                            assert!(
                                pollster::block_on(
                                    driver.run_legacy_diagnostic_timestep(&met, &forcing)
                                )
                                .is_err(),
                                "canonical fields cannot enter diagnostics"
                            );
                            let failed =
                                pollster::block_on(driver.run_timestep(&met, &forcing)).is_err();
                            let initial = driver.particle_store().as_slice().to_vec();
                            let (particles, status, metadata, values, queries) =
                                pollster::block_on(driver.test_advection_state());
                            if !failed {
                                assert!(
                                    pollster::block_on(driver.run_timestep(&met, &forcing))
                                        .is_err(),
                                    "bracket crossing must fail before submission"
                                );
                                let (after, _, _, _, _) =
                                    pollster::block_on(driver.test_advection_state());
                                assert_eq!(
                                    bytemuck::cast_slice::<_, u8>(&after),
                                    bytemuck::cast_slice::<_, u8>(&particles)
                                );
                            }
                            (
                                particles, status, metadata, values, queries, initial, adapter,
                                failed,
                            )
                        } else {
                            let config = ForwardTimeLoopConfig {
                                start_timestamp: "20240101000000".into(),
                                end_timestamp: "20240101000001".into(),
                                timestep_seconds: 1,
                                ..ForwardTimeLoopConfig::default()
                            };
                            let mut releases = vec![ReleaseConfig {
                                name: "probe".into(),
                                start_time: "20240101000000".into(),
                                end_time: "20240101000000".into(),
                                lon: 6.6875,
                                lat: 50.25,
                                z_min: 100.0,
                                z_max: 100.0,
                                mass_kg: 1.0,
                                particle_count: count as u64,
                                species_masses_kg: None,
                                raw: BTreeMap::new(),
                            }];
                            if failure.is_some() && count > 1 {
                                let mut unaffected = releases[0].clone();
                                unaffected.lon = 7.3125;
                                unaffected.particle_count = (count - 1) as u64;
                                unaffected.mass_kg = (count - 1) as f64 / count as f64;
                                releases[0].particle_count = 1;
                                releases[0].mass_kg = 1.0 / count as f64;
                                releases.push(unaffected);
                            }
                            let mut driver = pollster::block_on(ForwardTimeLoopDriver::new(
                                config, &releases, grid, 130,
                            ))
                            .expect("required forward WGSL adapter");
                            driver.test_set_active_prefix(count);
                            let prepared = driver
                                .prepare_canonical_meteorology(source, &mut slot)
                                .unwrap();
                            let adapter = GpuAdapterEvidence::from_context(prepared.context());
                            let owners = std::sync::Arc::clone(prepared.resources());
                            poison(&owners, prepared.context(), failure, false);
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
                            let canonical_owner = met.canonical.take();
                            assert!(
                                pollster::block_on(driver.run_timestep(&met, &forcing)).is_err(),
                                "production cannot use legacy fallback"
                            );
                            met.canonical = canonical_owner;
                            assert!(
                                pollster::block_on(
                                    driver.run_legacy_diagnostic_timestep(&met, &forcing)
                                )
                                .is_err(),
                                "canonical fields cannot enter diagnostics"
                            );
                            let failed =
                                pollster::block_on(driver.run_timestep(&met, &forcing)).is_err();
                            let initial = driver.particle_store().as_slice().to_vec();
                            let (particles, status, metadata, values, queries) =
                                pollster::block_on(driver.test_advection_state());
                            if !failed {
                                assert!(
                                    pollster::block_on(driver.run_timestep(&met, &forcing))
                                        .is_err(),
                                    "bracket crossing must fail before submission"
                                );
                                let (after, _, _, _, _) =
                                    pollster::block_on(driver.test_advection_state());
                                assert_eq!(
                                    bytemuck::cast_slice::<_, u8>(&after),
                                    bytemuck::cast_slice::<_, u8>(&particles)
                                );
                            }
                            (
                                particles, status, metadata, values, queries, initial, adapter,
                                failed,
                            )
                        };
                    assert_eq!(
                        failed,
                        failure.is_some(),
                        "direction={backward}, failure={failure:?}, statuses={status:?}"
                    );
                    if failure.is_some() {
                        assert_eq!(
                            bytemuck::cast_slice::<_, u8>(&particles),
                            bytemuck::cast_slice::<_, u8>(&initial),
                            "fatal timestep must preserve all particle bytes"
                        );
                        let (component, stage) = failure.unwrap();
                        let region = (stage * 3 + component) * 131;
                        assert_ne!(status[region], 0, "required failing sample status");
                        assert!(matches!(status[region + 1], 3..=9));
                        if count > 1 {
                            assert!(status[region + 2..region + 1 + count].iter().all(|s| *s == 2),
                                "one failed lane must prevent updates to every valid lane across workgroups");
                        }
                    } else {
                        let expected = if varying {
                            if backward {
                                1.805
                            } else {
                                3.405
                            }
                        } else {
                            if backward {
                                1.75
                            } else {
                                3.25
                            }
                        };
                        for p in particles.iter().take(count) {
                            let x = p.cell_x as f32 + p.pos_x;
                            assert_eq!(p.vel_v, 0.125);
                            assert_eq!(p.vel_w, 0.25);
                            let policy =
                                crate::gpu::vertical::vertical_model_comparison_policy().unwrap();
                            assert!(
                                crate::gpu::compare_finite_values(
                                    &[f64::from(expected)],
                                    &[f64::from(x)],
                                    policy
                                )
                                .unwrap()
                                .verdict
                                    == crate::gpu::NumericalVerdict::Passed
                            );
                        }
                    }
                    assert_eq!(metadata.len(), 6);
                    assert!(metadata.iter().all(|m| m.active_count == count as u32));
                    if failure.is_none() {
                        for lane in 0..count {
                            assert_ne!(queries[lane].fraction_x, queries[130 + lane].fraction_x);
                            assert_ne!(metadata[0].time, metadata[3].time);
                            assert_eq!(
                                queries[130 + lane].height_agl_m,
                                100.0 + if backward { -0.25 } else { 0.25 }
                            );
                        }
                    }
                    rows.push(json!({"backward":backward,"varying":varying,"active_count":count,"capacity":130,
                "failure":failure,"mixed_lane_failure":failure.is_some() && count > 1,"status":status,"metadata":metadata,"sampled_values":values,"queries":queries,"adapter":adapter,"atomic_preservation":failure.is_some(),
                "source_snapshot_sha256":[snapshot_hash(&s0).unwrap(),snapshot_hash(&s1).unwrap()],
                "query_roundtrip":false,"owning_submissions":1,"input_file":input_file,"input_sha256":input_hash}));
                }
            }
        }
    }
    let shaders = [
        "advection_predictor_query",
        "advection_corrector",
        "advection_step_guard",
        "advection_step_commit",
        "particle_query",
        "resident_status_reset",
        "resident_query_adapter",
        "horizontal_interpolation",
        "vertical_sample",
        "temporal_interpolation",
        "resident_sample_status",
    ];
    let hashes: BTreeMap<_, _> = shaders
        .iter()
        .map(|name| {
            (
                *name,
                format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(format!("src/shaders/{name}.wgsl")).unwrap())
                ),
            )
        })
        .collect();
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    std::fs::write(report_path,serde_json::to_vec_pretty(&json!({"schema":"flexpart-gpu.resident-advection-production.v1",
        "candidate_revision":String::from_utf8(revision.stdout).unwrap().trim(),"validation_configuration":validation,"compaction_configuration":compaction,"validation_level":"production-path integration and analytical displacement",
        "scientific_verdict":"NO_FULL_FLEXPART_PARITY_CLAIM","shader_sha256":hashes,"rows":rows,"status":"passed"})).unwrap()).unwrap();
    eprintln!("RESIDENT-ADVECTION-112: actual forward/backward production WGSL, two-stage U/V/W, atomic fatal preservation passed");
}

fn poison(
    owners: &crate::simulation::CanonicalMeteorologyResources<'_>,
    ctx: &GpuContext,
    failure: Option<(usize, usize)>,
    backward: bool,
) {
    if let Some((component, stage)) = failure {
        // Mutate only a device source corner after canonical validation, never fabricate host metadata.
        let member = if backward { 1 - stage } else { stage };
        let field = &owners.fields()[component];
        let offset = if stage == 0 {
            10
        } else if backward {
            9
        } else {
            12
        };
        ctx.queue.write_buffer(
            field.members[member].resident_values.as_ref().unwrap(),
            offset * 4,
            bytemuck::bytes_of(&f32::NAN),
        );
    }
}

#[test]
fn test_forward_interior_time_deferred_failure_and_top_boundary() {
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
    let releases: Vec<_> = ["20240101000001", "20240101000002"]
        .into_iter()
        .map(|time| ReleaseConfig {
            name: time.into(),
            start_time: time.into(),
            end_time: time.into(),
            lon: 6.6875,
            lat: 50.25,
            z_min: 100.0,
            z_max: 100.0,
            mass_kg: 1.0,
            particle_count: 1,
            species_masses_kg: None,
            raw: BTreeMap::new(),
        })
        .collect();
    let root = std::path::Path::new("target/ci-gate/resident-advection-production");
    std::fs::create_dir_all(root).unwrap();
    let mut evidence = Vec::new();
    for variant in ["interior", "deferred-fatal", "changing-top"] {
        let s0 = analytic_snapshot(START_SECONDS, true);
        let mut s1 = analytic_snapshot(START_SECONDS + 4, true);
        if variant == "changing-top" {
            for field in &mut s1.fields {
                if field.id == FieldId::Temperature {
                    for value in &mut field.values {
                        *value += 1.0;
                    }
                }
            }
        }
        let r0 = runtime(&s0, "#173 declared synthetic geometry");
        let r1 = runtime(&s1, "#173 declared synthetic geometry");
        let input_file = format!("source-{variant}.json");
        let inputs = serde_json::to_vec(&json!({"snapshot_json":[serde_json::to_string(&s0).unwrap(),serde_json::to_string(&s1).unwrap()],
            "native_motion_json":[serde_json::to_string(&native_motion(&s0,"#173 declared synthetic geometry")).unwrap(),
                serde_json::to_string(&native_motion(&s1,"#173 declared synthetic geometry")).unwrap()],"runtimes":[&r0,&r1]})).unwrap();
        let input_sha256 = format!("{:x}", Sha256::digest(&inputs));
        std::fs::write(root.join(&input_file), &inputs).unwrap();
        let source = bracket([&s0, &s1], [&r0, &r1]);
        let config = ForwardTimeLoopConfig {
            start_timestamp: "20240101000001".into(),
            end_timestamp: "20240101000002".into(),
            timestep_seconds: 1,
            sync_particle_store_each_step: false,
            collect_deposition_probabilities_each_step: false,
            ..ForwardTimeLoopConfig::default()
        };
        let mut driver = pollster::block_on(ForwardTimeLoopDriver::new(
            config,
            &releases,
            grid.clone(),
            3,
        ))
        .expect("required forward WGSL adapter for deferred transaction");
        let mut slot = CanonicalMeteorologySlot::default();
        let prepared = driver
            .prepare_canonical_meteorology(source, &mut slot)
            .unwrap();
        let adapter = GpuAdapterEvidence::from_context(prepared.context());
        let owners = std::sync::Arc::clone(prepared.resources());
        if variant == "deferred-fatal" {
            poison(&owners, prepared.context(), Some((0, 0)), false);
        }
        drop(prepared);
        let met = MetTimeBracket {
            canonical: Some(&owners),
            wind_t0: &legacy_wind,
            wind_t1: &legacy_wind,
            surface_t0: &surface,
            surface_t1: &surface,
            time_t0_seconds: START_SECONDS,
            time_t1_seconds: START_SECONDS + 4,
        };
        let forcing = ForwardStepForcing::default();
        let result = pollster::block_on(driver.run_timestep(&met, &forcing));
        if variant == "changing-top" {
            assert!(
                result.is_err(),
                "undefined changing-top transport must fail closed"
            );
            assert!(owners.fields()[0].resident_advection_top_m().is_err());
            evidence.push(json!({"variant":variant,"adapter":adapter,"rejected":true,"input_file":input_file,"input_sha256":input_sha256}));
            continue;
        }
        let initial = driver.particle_store().as_slice().to_vec();
        let (particles, status, metadata, values, queries) =
            pollster::block_on(driver.test_advection_state());
        if variant == "deferred-fatal" {
            if std::env::var("FLEXPART_GPU_COMPACTION").as_deref() == Ok("1") {
                assert!(
                    result.is_err(),
                    "compaction observes status at its existing count checkpoint"
                );
            } else {
                assert!(
                    result.is_ok(),
                    "deferred mode has no immediate particle readback"
                );
            }
            assert!(pollster::block_on(driver.finalize()).is_err());
            assert!(pollster::block_on(driver.accumulate_concentration_grid(
                crate::gpu::ConcentrationGridShape {
                    nx: 8,
                    ny: 4,
                    nz: 1
                },
                crate::gpu::ConcentrationGriddingParams::default()
            ))
            .is_err());
            assert!(pollster::block_on(driver.run_timestep(&met, &forcing)).is_err());
            assert_eq!(
                driver
                    .particle_store()
                    .as_slice()
                    .iter()
                    .filter(|p| p.is_active())
                    .count(),
                1,
                "next release must not run after fatal status"
            );
            let (after, _, _, _, _) = pollster::block_on(driver.test_advection_state());
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&after),
                bytemuck::cast_slice::<_, u8>(&initial)
            );
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&particles),
                bytemuck::cast_slice::<_, u8>(&initial)
            );
        } else {
            result.unwrap();
            pollster::block_on(driver.finalize()).unwrap();
            let policy = crate::gpu::vertical::vertical_model_comparison_policy().unwrap();
            let actual = [
                particles[0].cell_x as f32 + particles[0].pos_x,
                values[0],
                values[9],
            ];
            assert_eq!(
                crate::gpu::compare_finite_values(
                    &[3.515, 0.65, 0.88],
                    &actual.map(f64::from),
                    policy
                )
                .unwrap()
                .verdict,
                crate::gpu::NumericalVerdict::Passed
            );
            assert_ne!(metadata[0].time, metadata[3].time);
            assert_ne!(queries[0].fraction_x, queries[3].fraction_x);
            assert!(status.iter().step_by(4).take(6).all(|v| *v == 0));
        }
        evidence.push(json!({"variant":variant,"adapter":adapter,"status":status,"metadata":metadata,
            "sampled_values":values,"queries":queries,"atomic_preservation":variant=="deferred-fatal","input_file":input_file,"input_sha256":input_sha256}));
    }
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    std::fs::write(
        root.join("interior-deferred-boundary.json"),
        serde_json::to_vec_pretty(&json!({"candidate_revision":String::from_utf8(revision.stdout).unwrap().trim(),"rows":evidence})).unwrap(),
    )
    .unwrap();
    eprintln!("RESIDENT-ADVECTION-112-DEFERRED: interior sampling, atomic deferred failure, next release/output rejection and changing-top guard passed");
}
