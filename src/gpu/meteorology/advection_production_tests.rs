//! Required-adapter production-driver tests. Source setup reuses the #173 declared canonical fixture.
#![allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
#[allow(clippy::wildcard_imports)]
use super::*;
use crate::gpu::{shared_height::*, shared_height_w::*};
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
use std::sync::Arc;
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
    let values = vec![0.0; snapshot.horizontal_grid.nx * snapshot.horizontal_grid.ny * 4];
    NativeVerticalMotion {
        kind: NativeVerticalMotionKind::PressureVelocityOmega,
        unit: NativeVerticalMotionUnit::PascalPerSecond,
        sign: NativeVerticalMotionSign::PositivePressureIncreasing,
        vertical_staggering: VerticalStaggering::LevelInterface,
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
    ctx: &GpuContext,
    s: [&'a Snapshot; 2],
    r: [&'a VerticalTransformResult; 2],
) -> CanonicalMeteorologyBracket<'a> {
    let views = r.map(|r| r.runtime_view().unwrap());
    let prep = pollster::block_on(SharedHeightPreparer::new(ctx)).unwrap();
    let wp = pollster::block_on(SharedHeightWPreparer::new(ctx)).unwrap();
    let sources = [0, 1].map(|i| SharedHeightSource::upload(ctx, s[i], views[i]).unwrap());
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let target = prep
        .initialize(
            ctx,
            &sources[0],
            SharedHeightInitialization::RegionalMother,
            &mut enc,
        )
        .unwrap();
    let uv = sources.map(|source| Arc::new(prep.prepare(ctx, &source, &target, &mut enc).unwrap()));
    let w = [0, 1].map(|i| {
        let source = SharedHeightWSource::upload(ctx, s[i], views[i], Arc::clone(&uv[i])).unwrap();
        Arc::new(wp.prepare(ctx, &source, &mut enc).unwrap())
    });
    ctx.queue.submit(Some(enc.finish()));
    CanonicalMeteorologyBracket::from_shared_height_vector(s, views, uv, w).unwrap()
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
    s.fields.retain(|f| f.id != FieldId::VerticalVelocity);
    s.fields
        .iter_mut()
        .find(|f| f.id == FieldId::SurfacePressure)
        .unwrap()
        .values
        .fill(101_325.0);
    for (id, source) in [
        (FieldId::WindU10m, FieldId::WindU),
        (FieldId::WindV10m, FieldId::WindV),
    ] {
        let values = s.fields.iter().find(|f| f.id == source).unwrap().values[..32].to_vec();
        let mut field = s
            .fields
            .iter()
            .find(|f| f.id == FieldId::Temperature2m)
            .unwrap()
            .clone();
        field.id = id;
        field.unit = crate::meteorology::Unit::MeterPerSecond;
        field.sign = if id == FieldId::WindU10m {
            crate::meteorology::SignConvention::PositiveEastward
        } else {
            crate::meteorology::SignConvention::PositiveNorthward
        };
        field.values = values;
        s.fields.push(field);
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
                                .prepare_canonical_meteorology(
                                    bracket(driver.meteorology_context(), [&s0, &s1], [&r0, &r1]),
                                    &mut slot,
                                )
                                .unwrap();
                            let adapter = GpuAdapterEvidence::from_context(prepared.context());
                            let owners = std::sync::Arc::clone(prepared.resources());
                            drop(prepared);
                            crate::gpu::advection_resident::TEST_QUERY_FAULT
                                .with(|fault| fault.set(failure));
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
                                .prepare_canonical_meteorology(
                                    bracket(driver.meteorology_context(), [&s0, &s1], [&r0, &r1]),
                                    &mut slot,
                                )
                                .unwrap();
                            let adapter = GpuAdapterEvidence::from_context(prepared.context());
                            let owners = std::sync::Arc::clone(prepared.resources());
                            drop(prepared);
                            crate::gpu::advection_resident::TEST_QUERY_FAULT
                                .with(|fault| fault.set(failure));
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
                            assert_eq!(p.vel_w, 0.0);
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
                            assert_eq!(queries[130 + lane].height_agl_m, 100.0);
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
        let shader=ctx.load_shader("prepared source fault", "@group(0) @binding(0) var<storage,read_write> v:array<u32>; @group(0) @binding(1) var<uniform> offset:vec4<u32>; @compute @workgroup_size(1) fn main(){v[offset.x]=0x7fc00000u;}");
        let pipe = ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None,
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
        use wgpu::util::DeviceExt;
        let params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&[offset as u32, 0, 0, 0]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipe.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: field.members[member]
                        .prepared_values()
                        .unwrap()
                        .as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipe);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        ctx.queue.submit(Some(enc.finish()));
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
            .prepare_canonical_meteorology(
                bracket(driver.meteorology_context(), [&s0, &s1], [&r0, &r1]),
                &mut slot,
            )
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

// Reuse the exact #186 input decoder used by the #188/#190 device proofs.
#[path = "../../../tests/support/interior_w.rs"]
mod interior_fixture;

fn invariant_oracle_snapshot(memory: usize) -> Snapshot {
    let mut source = interior_fixture::snapshot(memory);
    let initial = interior_fixture::snapshot(1);
    for field in &mut source.fields {
        if matches!(
            field.id,
            FieldId::SurfacePressure
                | FieldId::Temperature
                | FieldId::SpecificHumidity
                | FieldId::Temperature2m
                | FieldId::Dewpoint2m
        ) {
            let original = initial.fields.iter().find(|f| f.id == field.id).unwrap();
            for (i, value) in field.values.iter_mut().enumerate() {
                *value = original.values[(i / 9) * 9];
            }
        }
    }
    source
}

#[test]
fn test_shared_vector_real_drivers_oracle_replay_and_reuse() {
    let root = shared_advection_root();
    let root = std::path::Path::new(&root);
    std::fs::create_dir_all(root).unwrap();
    let _ = std::fs::remove_file(root.join("report.json"));
    let snapshots = [invariant_oracle_snapshot(1), invariant_oracle_snapshot(2)];
    let motions = [interior_fixture::native(1), interior_fixture::native(2)];
    let runtimes = [0, 1]
        .map(|i| reconstruct_vertical_geometry_with_motion(&snapshots[i], &motions[i]).unwrap());
    let grid = GridDomain {
        xlon0: -2.0,
        ylat0: 48.0,
        dx: 0.25,
        dy: 0.4,
        nx: 3,
        ny: 3,
    };
    let legacy = uniform_wind_field(
        &WindFieldGrid::new(
            3,
            3,
            3,
            3,
            3,
            0.25,
            0.4,
            -2.0,
            48.0,
            ndarray::Array1::from_vec(vec![0.0, 1.0, 2.0]),
        ),
        1000.0,
        1000.0,
        1000.0,
    );
    let mut surface = SurfaceFields::zeros(3, 3);
    surface.surface_pressure_pa.fill(101_325.0);
    surface.temperature_2m_k.fill(291.0);
    surface.dewpoint_2m_k.fill(284.0);
    surface.mixing_height_m.fill(1000.0);
    let mut rows = Vec::new();
    for (backward, epoch) in [(false, 0), (false, 1800), (true, 3600), (true, 1800)] {
        let timestamp = |seconds: i64| {
            format!(
                "19700101{:02}{:02}{:02}",
                seconds / 3600,
                (seconds % 3600) / 60,
                seconds % 60
            )
        };
        let start = timestamp(epoch);
        let end = timestamp(if backward { epoch - 2 } else { epoch + 2 });
        let heights = [2_684.168_945_312_5, 7_789.012_695_312_5];
        let releases: Vec<_> = heights
            .into_iter()
            .map(|z| ReleaseConfig {
                name: format!("height-{z}"),
                start_time: start.clone(),
                end_time: start.clone(),
                lon: -2.0 + 0.7 * 0.25,
                lat: 48.0 + 0.6 * 0.4,
                z_min: z,
                z_max: z,
                mass_kg: 1.0,
                particle_count: 1,
                species_masses_kg: None,
                raw: BTreeMap::new(),
            })
            .collect();
        let mut forward = if backward {
            None
        } else {
            Some(
                pollster::block_on(ForwardTimeLoopDriver::new(
                    ForwardTimeLoopConfig {
                        start_timestamp: start.clone(),
                        end_timestamp: end.clone(),
                        timestep_seconds: 1,
                        ..Default::default()
                    },
                    &releases,
                    grid.clone(),
                    3,
                ))
                .unwrap(),
            )
        };
        let mut reverse = if backward {
            Some(
                pollster::block_on(BackwardTimeLoopDriver::new(
                    BackwardTimeLoopConfig {
                        start_timestamp: start.clone(),
                        end_timestamp: end.clone(),
                        timestep_seconds: 1,
                        receptors: heights
                            .into_iter()
                            .map(|z| BackwardReceptorConfig {
                                name: format!("height-{z}"),
                                lon: -2.0 + 0.7 * 0.25,
                                lat: 48.0 + 0.6 * 0.4,
                                z_m: z,
                                particle_count: 1,
                                mass_kg: 1.0,
                            })
                            .collect(),
                        ..Default::default()
                    },
                    grid.clone(),
                    3,
                ))
                .unwrap(),
            )
        } else {
            None
        };
        let ctx = if let Some(d) = &forward {
            d.meteorology_context()
        } else {
            reverse.as_ref().unwrap().meteorology_context()
        };
        let source = bracket(
            ctx,
            [&snapshots[0], &snapshots[1]],
            [&runtimes[0], &runtimes[1]],
        );
        let mut slot = CanonicalMeteorologySlot::default();
        let mut previous = None;
        for step in 0..2 {
            let prepared = if let Some(d) = &forward {
                d.prepare_canonical_meteorology(source.clone(), &mut slot)
                    .unwrap()
            } else {
                reverse
                    .as_ref()
                    .unwrap()
                    .prepare_canonical_meteorology(source.clone(), &mut slot)
                    .unwrap()
            };
            assert_eq!(prepared.reused, step == 1);
            let owners = Arc::clone(prepared.resources());
            if let Some(old) = &previous {
                assert!(Arc::ptr_eq(old, &owners));
            }
            previous = Some(Arc::clone(&owners));
            let adapter = GpuAdapterEvidence::from_context(prepared.context());
            let target = serde_json::to_value(
                owners.fields()[0].members[0]
                    .prepared_uv
                    .as_ref()
                    .unwrap()
                    .target()
                    .identity(),
            )
            .unwrap();
            let mut target_preimage = owners.fields()[0].members[0]
                .prepared_uv
                .as_ref()
                .unwrap()
                .target()
                .identity()
                .clone();
            target_preimage.identity_sha256.clear();
            let target_identity_json = serde_json::to_string(&target_preimage).unwrap();
            let preparations: Vec<_> = owners.fields()[0]
                .members
                .iter()
                .map(|m| serde_json::to_value(m.prepared_w.as_ref().unwrap().metadata()).unwrap())
                .collect();
            drop(prepared);
            let met = MetTimeBracket {
                canonical: Some(&owners),
                wind_t0: &legacy,
                wind_t1: &legacy,
                surface_t0: &surface,
                surface_t1: &surface,
                time_t0_seconds: 0,
                time_t1_seconds: 3600,
            };
            let forcing = ForwardStepForcing::default();
            let (particles, status, metadata, values, queries) = if let Some(d) = &mut forward {
                pollster::block_on(d.run_timestep(&met, &forcing)).unwrap();
                pollster::block_on(d.test_advection_state())
            } else {
                let d = reverse.as_mut().unwrap();
                pollster::block_on(d.run_timestep(&met, &forcing)).unwrap();
                pollster::block_on(d.test_advection_state())
            };
            assert!(status.iter().enumerate().all(|(i, s)| if i == 24 {
                *s == 0
            } else {
                matches!(*s, 0 | 2)
            }));
            assert_eq!(metadata.len(), 6);
            for m in &metadata {
                assert_eq!(
                    m.shared_height.as_ref().unwrap().preparation,
                    "shared_height_uvw_190"
                );
            }
            rows.push(json!({"backward":backward,"start_epoch":epoch,"step":step,"capacity":3,"active_count":2,"adapter":adapter,"queries":queries,"sampled_values":values,"status":status,"metadata":metadata,"particle_bytes":bytemuck::cast_slice::<_,u8>(&particles),"target":target,"target_identity_json":target_identity_json,"preparations":preparations,"reused":step==1}));
        }
    }
    let sources = [
        "src/gpu/advection_resident.rs",
        "src/gpu/meteorology.rs",
        "src/gpu/meteorology/resident.rs",
        "src/gpu/meteorology/shared_uv.rs",
        "src/gpu/meteorology/shared_w.rs",
        "src/gpu/shared_height.rs",
        "src/gpu/shared_height_w.rs",
        "src/simulation/timeloop/meteorology.rs",
        "src/simulation/timeloop/forward/operators.rs",
        "src/simulation/timeloop/forward/timestep.rs",
        "src/simulation/timeloop/backward.rs",
        "src/gpu/meteorology/advection_production_tests.rs",
        "tests/support/interior_w.rs",
    ];
    let source_hashes: BTreeMap<_, _> = sources
        .iter()
        .map(|p| {
            (
                *p,
                format!("{:x}", Sha256::digest(std::fs::read(p).unwrap())),
            )
        })
        .collect();
    let shader_names = [
        "shared_height_uv",
        "shared_height_w_remap",
        "interior_w_slope",
        "shared_height_query_status",
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
    let shader_hashes: BTreeMap<_, _> = shader_names
        .iter()
        .map(|p| {
            (
                *p,
                format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(format!("src/shaders/{p}.wgsl")).unwrap())
                ),
            )
        })
        .collect();
    let input=serde_json::to_vec(&json!({"snapshot_json":snapshots.iter().map(|s|serde_json::to_string(s).unwrap()).collect::<Vec<_>>(),"native_motion_json":motions.iter().map(|s|serde_json::to_string(s).unwrap()).collect::<Vec<_>>(),"runtimes":runtimes})).unwrap();
    std::fs::write(root.join("inputs.json"), &input).unwrap();
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    std::fs::write(root.join("report.json"),serde_json::to_vec_pretty(&json!({"schema":"flexpart-gpu.shared-advection.v1","candidate_revision":String::from_utf8(revision.stdout).unwrap().trim(),"candidate_executable_sha256":format!("{:x}",Sha256::digest(std::fs::read(std::env::current_exe().unwrap()).unwrap())),"source_hashes":source_hashes,"shader_hashes":shader_hashes,"input_sha256":format!("{:x}",Sha256::digest(&input)),"validation_configuration":std::env::var("FLEXPART_GPU_VALIDATION").unwrap_or_else(|_|"0".into()),"compaction_configuration":std::env::var("FLEXPART_GPU_COMPACTION").unwrap_or_else(|_|"0".into()),"rows":rows,"execution":"wgsl_device","scientific_verdict":"AWAITING_PINNED_QUERY_REPLAY"})).unwrap()).unwrap();
    eprintln!("SHARED-ADVECTION-197: actual forward/backward prepared-vector queries retained for genuine #186 replay");
}

fn overwrite_words(ctx: &GpuContext, buffer: &wgpu::Buffer, bits: u32, count: u32) {
    let pass=resident::QueryPass::new(ctx,"validation prepared-resource fault",
        "@group(0) @binding(0) var<storage,read_write> words:array<u32>; @group(0) @binding(1) var<uniform> p:vec4<u32>; @compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id:vec3<u32>){if(id.x<p.y){words[id.x]=p.x;}}", &[false]);
    let params = resident::uniform(ctx, &[bits, count, 0, 0]);
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    pass.encode(ctx, &[buffer, &params], count, &mut encoder);
    ctx.queue.submit(Some(encoder.finish()));
}

#[test]
fn test_shared_vector_driver_preparation_failures_and_top_rejection() {
    let root = shared_advection_root();
    let root = std::path::Path::new(&root);
    std::fs::create_dir_all(root).unwrap();
    let _ = std::fs::remove_file(root.join("negative.json"));
    let snapshots = [
        analytic_snapshot(START_SECONDS, false),
        analytic_snapshot(START_SECONDS + 1, false),
    ];
    let runtimes = snapshots
        .each_ref()
        .map(|s| runtime(s, "#197 invariant diagnostic"));
    let top = runtimes[0]
        .runtime_view()
        .unwrap()
        .level(0, 0, 0)
        .unwrap()
        .height_agl_m;
    let grid = GridDomain {
        xlon0: 6.0,
        ylat0: 50.0,
        dx: 0.25,
        dy: 0.25,
        nx: 8,
        ny: 4,
    };
    let wind = uniform_wind_field(
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
    surface.temperature_2m_k.fill(291.0);
    surface.dewpoint_2m_k.fill(284.0);
    surface.mixing_height_m.fill(1000.0);
    let mut evidence = Vec::new();
    for backward in [false, true] {
        for variant in [
            "target-unexecuted",
            "target-failed",
            "uv-pending",
            "uv-invalid-status",
            "w-unexecuted",
            "w-remapped-only",
            "w-failed",
            "w-invalid-status",
            "nonfinite-vector",
            "at-top",
            "above-top",
            "predictor-crossing",
            "corrector-crossing",
        ] {
            let start = if backward {
                "20240101000001"
            } else {
                "20240101000000"
            };
            let end = if backward {
                "20240101000000"
            } else {
                "20240101000001"
            };
            let height_m = match variant {
                "at-top" => top,
                "above-top" => top + 1.0,
                "predictor-crossing" | "corrector-crossing" => top - 0.25,
                _ => 100.0,
            };
            let releases = [ReleaseConfig {
                name: "fault".into(),
                start_time: start.into(),
                end_time: start.into(),
                lon: 6.6875,
                lat: 50.25,
                z_min: f64::from(height_m),
                z_max: f64::from(height_m),
                mass_kg: 1.0,
                particle_count: 2,
                species_masses_kg: None,
                raw: BTreeMap::new(),
            }];
            let mut forward = if backward {
                None
            } else {
                Some(
                    pollster::block_on(ForwardTimeLoopDriver::new(
                        ForwardTimeLoopConfig {
                            start_timestamp: start.into(),
                            end_timestamp: end.into(),
                            timestep_seconds: 1,
                            ..Default::default()
                        },
                        &releases,
                        grid.clone(),
                        3,
                    ))
                    .unwrap(),
                )
            };
            let mut reverse = if backward {
                Some(
                    pollster::block_on(BackwardTimeLoopDriver::new(
                        BackwardTimeLoopConfig {
                            start_timestamp: start.into(),
                            end_timestamp: end.into(),
                            timestep_seconds: 1,
                            receptors: vec![BackwardReceptorConfig {
                                name: "fault".into(),
                                lon: 6.6875,
                                lat: 50.25,
                                z_m: f64::from(height_m),
                                particle_count: 2,
                                mass_kg: 1.0,
                            }],
                            ..Default::default()
                        },
                        grid.clone(),
                        3,
                    ))
                    .unwrap(),
                )
            } else {
                None
            };
            let ctx = if let Some(d) = &forward {
                d.meteorology_context()
            } else {
                reverse.as_ref().unwrap().meteorology_context()
            };
            let source = bracket(
                ctx,
                [&snapshots[0], &snapshots[1]],
                [&runtimes[0], &runtimes[1]],
            );
            let mut slot = CanonicalMeteorologySlot::default();
            let prepared = if let Some(d) = &forward {
                d.prepare_canonical_meteorology(source.clone(), &mut slot)
                    .unwrap()
            } else {
                reverse
                    .as_ref()
                    .unwrap()
                    .prepare_canonical_meteorology(source.clone(), &mut slot)
                    .unwrap()
            };
            let owners = Arc::clone(prepared.resources());
            let ctx = prepared.context();
            let first = &owners.fields()[0].members[0];
            let uv = first.prepared_uv.as_ref().unwrap();
            let corrected_w = first.prepared_w.as_ref().unwrap();
            let adapter = GpuAdapterEvidence::from_context(ctx);
            if variant == "target-unexecuted" {
                let uv: [Arc<PreparedSharedHeightUv>; 2] = [0, 1].map(|i| {
                    Arc::clone(owners.fields()[0].members[i].prepared_uv.as_ref().unwrap())
                });
                let w: [Arc<PreparedSharedHeightW>; 2] = [0, 1].map(|i| {
                    Arc::clone(owners.fields()[0].members[i].prepared_w.as_ref().unwrap())
                });
                let views = runtimes.each_ref().map(|r| r.runtime_view().unwrap());
                let bad = CanonicalMeteorologyBracket::from_shared_height_vector(
                    [&snapshots[0], &snapshots[1]],
                    views,
                    uv.clone(),
                    [Arc::clone(&w[1]), Arc::clone(&w[0])],
                )
                .unwrap();
                assert!(if let Some(d) = &forward {
                    d.prepare_canonical_meteorology(bad, &mut slot).is_err()
                } else {
                    reverse
                        .as_ref()
                        .unwrap()
                        .prepare_canonical_meteorology(bad, &mut slot)
                        .is_err()
                });
                let new_target = bracket(
                    ctx,
                    [&snapshots[0], &snapshots[1]],
                    [&runtimes[0], &runtimes[1]],
                );
                assert!(if let Some(d) = &forward {
                    d.prepare_canonical_meteorology(new_target, &mut slot)
                        .is_err()
                } else {
                    reverse
                        .as_ref()
                        .unwrap()
                        .prepare_canonical_meteorology(new_target, &mut slot)
                        .is_err()
                });
                let missing =
                    crate::meteorology::vertical::reconstruct_vertical_geometry(&snapshots[0])
                        .unwrap();
                assert!(CanonicalMeteorologyBracket::from_shared_height_vector(
                    [&snapshots[0], &snapshots[1]],
                    [missing.runtime_view().unwrap(), views[1]],
                    uv.clone(),
                    w.clone()
                )
                .is_err());
                let mixed = CanonicalMeteorologyBracket::from_shared_height_vector(
                    [&snapshots[1], &snapshots[0]],
                    views,
                    uv.clone(),
                    w.clone(),
                );
                assert!(mixed.is_err());
                let other = pollster::block_on(GpuContext::new()).unwrap();
                let foreign = bracket(
                    &other,
                    [&snapshots[0], &snapshots[1]],
                    [&runtimes[0], &runtimes[1]],
                );
                assert!(if let Some(d) = &forward {
                    d.prepare_canonical_meteorology(foreign, &mut slot).is_err()
                } else {
                    reverse
                        .as_ref()
                        .unwrap()
                        .prepare_canonical_meteorology(foreign, &mut slot)
                        .is_err()
                });
                let reused = if let Some(d) = &forward {
                    d.prepare_canonical_meteorology(source.clone(), &mut slot)
                        .unwrap()
                } else {
                    reverse
                        .as_ref()
                        .unwrap()
                        .prepare_canonical_meteorology(source.clone(), &mut slot)
                        .unwrap()
                };
                assert!(reused.reused && Arc::ptr_eq(&owners, reused.resources()));
            }
            match variant {
                "target-unexecuted" => overwrite_words(ctx, uv.target().status(), 0, 1),
                "target-failed" => overwrite_words(ctx, uv.target().status(), 3, 1),
                "uv-pending" => overwrite_words(ctx, uv.status(), 1, 1),
                "uv-invalid-status" => overwrite_words(ctx, uv.status(), 99, 1),
                "w-unexecuted" => overwrite_words(ctx, corrected_w.status(), 0, 1),
                "w-remapped-only" => overwrite_words(ctx, corrected_w.status(), 1, 1),
                "w-failed" => overwrite_words(ctx, corrected_w.status(), 3, 1),
                "w-invalid-status" => overwrite_words(ctx, corrected_w.status(), 99, 1),
                "nonfinite-vector" => overwrite_words(ctx, corrected_w.w(), f32::NAN.to_bits(), 1),
                "predictor-crossing" | "corrector-crossing" => {
                    let member = if variant == "predictor-crossing" {
                        usize::from(backward)
                    } else {
                        usize::from(!backward)
                    };
                    let buffer = owners.fields()[2].members[member]
                        .prepared_values()
                        .unwrap();
                    overwrite_words(
                        ctx,
                        buffer,
                        if backward {
                            (-1.0_f32).to_bits()
                        } else {
                            1.0_f32.to_bits()
                        },
                        128,
                    );
                }
                _ => (),
            }
            drop(prepared);
            let met = MetTimeBracket {
                canonical: Some(&owners),
                wind_t0: &wind,
                wind_t1: &wind,
                surface_t0: &surface,
                surface_t1: &surface,
                time_t0_seconds: START_SECONDS,
                time_t1_seconds: START_SECONDS + 1,
            };
            let forcing = ForwardStepForcing::default();
            let (initial, particles, status) = if let Some(d) = &mut forward {
                assert!(
                    pollster::block_on(d.run_timestep(&met, &forcing)).is_err(),
                    "{variant}"
                );
                let initial = d.particle_store().as_slice().to_vec();
                let (p, s, _, _, _) = pollster::block_on(d.test_advection_state());
                (initial, p, s)
            } else {
                let d = reverse.as_mut().unwrap();
                assert!(
                    pollster::block_on(d.run_timestep(&met, &forcing)).is_err(),
                    "{variant}"
                );
                let initial = d.particle_store().as_slice().to_vec();
                let (p, s, _, _, _) = pollster::block_on(d.test_advection_state());
                (initial, p, s)
            };
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&initial),
                bytemuck::cast_slice::<_, u8>(&particles),
                "{backward}/{variant}: atomic failure"
            );
            evidence.push(json!({"backward":backward,"variant":variant,"adapter":adapter,"status":status,"initial_bytes":bytemuck::cast_slice::<_,u8>(&initial),"final_bytes":bytemuck::cast_slice::<_,u8>(&particles)}));
        }
    }
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    std::fs::write(root.join("negative.json"),serde_json::to_vec_pretty(&json!({"candidate_revision":String::from_utf8(revision.stdout).unwrap().trim(),"rows":evidence})).unwrap()).unwrap();
    eprintln!("SHARED-ADVECTION-197-NEGATIVE: both real drivers reject failed/incomplete preparation and unsupported top crossings atomically");
}

fn shared_advection_root() -> String {
    std::env::var("FLEXPART_GPU_SHARED_ADVECTION_EVIDENCE").unwrap_or_else(|_| {
        format!(
            "target/ci-gate/shared-advection/validation-{}-compaction-{}",
            std::env::var("FLEXPART_GPU_VALIDATION").unwrap_or_else(|_| "0".into()),
            std::env::var("FLEXPART_GPU_COMPACTION").unwrap_or_else(|_| "0".into())
        )
    })
}
