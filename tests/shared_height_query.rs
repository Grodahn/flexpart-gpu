//! #185 public #76/#171 U/V queries against every direct pinned #118 QUERY row.
use flexpart_gpu::{
    gpu::{
        evidence::GpuEvidenceSchema,
        meteorology::{
            resident::{ResidentFixtureState, ResidentQueryBatch, ResidentQueryKernels},
            CanonicalGpuField, MeteorologyCompositionKernels, MeteorologyHeight,
            MeteorologySampleRequest, MeteorologyTimeSelection,
        },
        shared_height::*,
        vertical::sha256_hex,
        *,
    },
    meteorology::{temporal::RequestedSampleTime, vertical::reconstruct_vertical_geometry, *},
    particles::{Particle, ParticleInit},
};
use serde_json::{json, Value};
use std::sync::Arc;
use wgpu::util::DeviceExt;
#[path = "support/shared_height.rs"]
mod fixture;

fn time(t: i64) -> MeteorologyTimeSelection {
    MeteorologyTimeSelection::Instantaneous(RequestedSampleTime::new(Calendar::Gregorian, t))
}

fn particle(row: &Value) -> Particle {
    Particle::new(&ParticleInit {
        cell_x: 0,
        cell_y: 0,
        pos_x: row["x"].as_f64().unwrap() as f32,
        pos_y: row["y"].as_f64().unwrap() as f32,
        pos_z: row["height_m_agl"].as_f64().unwrap() as f32,
        mass: [1.0, 0.0, 0.0, 0.0],
        release_point: 0,
        class: 0,
        time: 0,
    })
}

fn read<T: bytemuck::Pod>(ctx: &GpuContext, buffer: &wgpu::Buffer, count: usize) -> Vec<T> {
    pollster::block_on(download_buffer_typed(
        ctx,
        buffer,
        count,
        "#185 final validation checkpoint",
    ))
    .unwrap()
}

fn compare(actual: f32, row: &Value, component: usize, api: &str) -> Value {
    // The pinned routine emits single-precision velocities. Preserve those exact
    // bits across serde_json's intermediate f64 decoder before reporting metrics.
    let expected = f64::from(row["oracle_uv_w_m_s"][component].as_f64().unwrap() as f32);
    let actual = f64::from(actual);
    let error = (actual - expected).abs();
    let limit = 1e-6 + 1e-5 * actual.abs().max(expected.abs());
    assert!(
        actual.is_finite() && expected.is_finite() && error <= limit,
        "{api} query={} component={component}: {actual} vs {expected}",
        row["query"]
    );
    json!({"api":api,"query":row["query"],"component":component,"actual_m_s":actual,
        "oracle_m_s":expected,"absolute_difference_m_s":error,"allowed_difference_m_s":limit})
}

fn inject(
    ctx: &GpuContext,
    buffer: &wgpu::Buffer,
    word: u32,
    bits: u32,
    encoder: &mut wgpu::CommandEncoder,
) {
    // Fault injection only: scientific inputs and golden values remain unchanged.
    let shader = ctx.load_shader(
        "#185 validation fault",
        r"
        @group(0) @binding(0) var<storage,read_write> words: array<u32>;
        @group(0) @binding(1) var<uniform> fault: vec4<u32>;
        @compute @workgroup_size(1) fn main(){words[fault.x]=fault.y;}
    ",
    );
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let params = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[word, bits, 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: params.as_entire_binding(),
            },
        ],
    });
    let mut pass = encoder.begin_compute_pass(&Default::default());
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.dispatch_workgroups(1, 1, 1);
}

#[test]
fn test_shared_height_all_public_query_rows_identity_and_fatal_atomicity() {
    let ctx = pollster::block_on(GpuContext::new()).expect("#185 requires actual WGSL execution");
    let kernels = MeteorologyCompositionKernels::new(&ctx).unwrap();
    let resident = ResidentQueryKernels::new(&ctx).unwrap();
    let preparer = pollster::block_on(SharedHeightPreparer::new(&ctx)).unwrap();
    let snapshots = [fixture::snapshot(1), fixture::snapshot(2)];
    let geometries: Vec<_> = snapshots
        .iter()
        .map(|s| reconstruct_vertical_geometry(s).unwrap())
        .collect();
    let views: Vec<_> = geometries
        .iter()
        .map(|g| g.runtime_view().unwrap())
        .collect();
    let refs: Vec<_> = snapshots.iter().collect();
    let sources: Vec<_> = refs
        .iter()
        .zip(&views)
        .map(|(s, v)| SharedHeightSource::upload(&ctx, s, *v).unwrap())
        .collect();
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    let target = preparer
        .initialize(
            &ctx,
            &sources[0],
            SharedHeightInitialization::RegionalMother,
            &mut encoder,
        )
        .unwrap();
    let prepared: Vec<_> = sources
        .iter()
        .map(|s| Arc::new(preparer.prepare(&ctx, s, &target, &mut encoder).unwrap()))
        .collect();
    let fields: Vec<_> = [FieldId::WindU, FieldId::WindV]
        .iter()
        .map(|id| {
            CanonicalGpuField::from_shared_height(&ctx, *id, &refs, &views, &prepared).unwrap()
        })
        .collect();
    let oracle = fixture::oracle();
    let queries = oracle["comparisons"].as_array().unwrap();
    assert_eq!(queries.len(), 9);
    let mut outputs = Vec::new();
    // All preparation, host queries, particle producers, samplers and consumers share
    // this caller encoder. Only final consumer/status checkpoints are downloaded.
    let mut host_plans = Vec::new();
    for field in &fields {
        for row in queries {
            host_plans.push(
                field
                    .prepare_sample(
                        &ctx,
                        MeteorologySampleRequest {
                            xt: row["x"].as_f64().unwrap(),
                            yt: row["y"].as_f64().unwrap(),
                            height: Some(MeteorologyHeight {
                                meters: row["height_m_agl"].as_f64().unwrap() as f32,
                                reference: VerticalReference::AboveGroundLevel,
                            }),
                            time: time(row["time_s"].as_i64().unwrap()),
                        },
                    )
                    .unwrap(),
            );
        }
    }
    for plan in &mut host_plans {
        let sample = plan.encode(&ctx, &kernels, &mut encoder).unwrap();
        let result = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("#76 final query output"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(sample.values, 0, &result, 0, 4);
        outputs.push((
            result,
            sample.status.unwrap().clone(),
            serde_json::to_value(sample.metadata).unwrap(),
        ));
    }
    let mut resident_resources = Vec::new();
    for field in &fields {
        for row in queries {
            resident_resources.push((
                field,
                row,
                ParticleBuffers::from_particles(&ctx, &[particle(row)]),
                ResidentQueryBatch::new(&ctx, &snapshots[0].horizontal_grid, 1, 1).unwrap(),
                ResidentFixtureState::upload(&ctx, &[-999.0]).unwrap(),
            ));
        }
    }
    let mut resident_plans: Vec<_> = resident_resources
        .iter_mut()
        .map(|(field, row, particles, batch, state)| {
            (
                field
                    .prepare_resident(&ctx, batch, time(row["time_s"].as_i64().unwrap()))
                    .unwrap(),
                particles,
                state,
            )
        })
        .collect();
    let mut resident_results = Vec::new();
    for (plan, particles, state) in &mut resident_plans {
        let sample = plan
            .encode_from_particles(&ctx, particles, &kernels, &resident, &mut encoder)
            .unwrap();
        sample
            .encode_fixture_consumer(&ctx, &resident, state, &mut encoder)
            .unwrap();
        resident_results.push((
            state.values.clone(),
            sample.status.clone(),
            serde_json::to_value(sample.metadata).unwrap(),
        ));
    }
    ctx.queue.submit(Some(encoder.finish()));
    let mut rows = Vec::new();
    for (index, (result, status, _)) in outputs.iter().enumerate() {
        assert_eq!(read::<u32>(&ctx, status, 2), [0, 2]);
        rows.push(compare(
            read::<f32>(&ctx, result, 1)[0],
            &queries[index % 9],
            index / 9,
            "#76",
        ));
    }
    for (index, (state, status, _)) in resident_results.iter().enumerate() {
        assert_eq!(read::<u32>(&ctx, status, 2), [0, 2]);
        rows.push(compare(
            read::<f32>(&ctx, state, 1)[0],
            &queries[index % 9],
            index / 9,
            "#171",
        ));
    }
    let mut negatives = Vec::new();
    for id in [
        FieldId::Temperature,
        FieldId::VerticalVelocity,
        FieldId::WindU10m,
    ] {
        assert!(CanonicalGpuField::from_shared_height(&ctx, id, &refs, &views, &prepared).is_err());
    }
    assert!(
        CanonicalGpuField::from_shared_height(&ctx, FieldId::WindU, &refs, &views, &[]).is_err()
    );
    let reversed = [Arc::clone(&prepared[1]), Arc::clone(&prepared[0])];
    assert!(
        CanonicalGpuField::from_shared_height(&ctx, FieldId::WindU, &refs, &views, &reversed)
            .is_err()
    );
    assert!(CanonicalGpuField::from_shared_height(
        &ctx,
        FieldId::WindU,
        &refs,
        &[views[1], views[0]],
        &prepared
    )
    .is_err());
    let mut alternate_encoder = ctx.device.create_command_encoder(&Default::default());
    let alternate_target = preparer
        .initialize(
            &ctx,
            &sources[0],
            SharedHeightInitialization::RegionalMother,
            &mut alternate_encoder,
        )
        .unwrap();
    let alternate = Arc::new(
        preparer
            .prepare(&ctx, &sources[1], &alternate_target, &mut alternate_encoder)
            .unwrap(),
    );
    assert!(CanonicalGpuField::from_shared_height(
        &ctx,
        FieldId::WindU,
        &refs,
        &views,
        &[Arc::clone(&prepared[0]), alternate]
    )
    .is_err());
    let other = pollster::block_on(GpuContext::new()).unwrap();
    assert!(CanonicalGpuField::from_shared_height(
        &other,
        FieldId::WindU,
        &refs,
        &views,
        &prepared
    )
    .is_err());
    drop(other);
    let native = CanonicalGpuField::upload(
        &ctx,
        FieldId::WindU,
        &refs,
        &views.iter().copied().map(Some).collect::<Vec<_>>(),
    )
    .unwrap();
    let request = MeteorologySampleRequest {
        xt: 0.25,
        yt: 0.625,
        height: Some(MeteorologyHeight {
            meters: 1000.0,
            reference: VerticalReference::AboveGroundLevel,
        }),
        time: time(1800),
    };
    assert!(native.prepare_sample(&ctx, request).is_err());
    let mut asl = request;
    asl.height.as_mut().unwrap().reference = VerticalReference::AboveMeanSeaLevel;
    assert!(fields[0].prepare_sample(&ctx, asl).is_err());
    for t in [-1, 3601] {
        let mut r = request;
        r.time = time(t);
        assert!(fields[0].prepare_sample(&ctx, r).is_err());
    }
    negatives.push(json!({"case":"static-identities-scope-time-asl","passed":true}));

    // Reuse the same prepared source and target in the next exact source bracket.
    let mut next = snapshots[1].clone();
    for f in &mut next.fields {
        if f.time.kind != TemporalKind::Static {
            f.time.valid_time_epoch_seconds = 7200;
        }
    }
    let next_geometry = reconstruct_vertical_geometry(&next).unwrap();
    let next_view = next_geometry.runtime_view().unwrap();
    let next_source = SharedHeightSource::upload(&ctx, &next, next_view).unwrap();
    let mut next_encoder = ctx.device.create_command_encoder(&Default::default());
    let next_prepared = Arc::new(
        preparer
            .prepare(&ctx, &next_source, &target, &mut next_encoder)
            .unwrap(),
    );
    let adjacent = CanonicalGpuField::from_shared_height(
        &ctx,
        FieldId::WindU,
        &[&snapshots[1], &next],
        &[views[1], next_view],
        &[Arc::clone(&prepared[1]), next_prepared],
    )
    .unwrap();
    let mut adjacent_request = request;
    adjacent_request.time = time(3600);
    let mut plan = adjacent.prepare_sample(&ctx, adjacent_request).unwrap();
    let sample = plan.encode(&ctx, &kernels, &mut next_encoder).unwrap();
    let mut retained_plan = fields[0].prepare_sample(&ctx, adjacent_request).unwrap();
    let retained_sample = retained_plan
        .encode(&ctx, &kernels, &mut next_encoder)
        .unwrap();
    ctx.queue.submit(Some(next_encoder.finish()));
    assert_eq!(read::<u32>(&ctx, sample.status.unwrap(), 2), [0, 2]);
    let adjacent_value = read::<f32>(&ctx, sample.values, 1)[0];
    let retained_value = read::<f32>(&ctx, retained_sample.values, 1)[0];
    assert_eq!(adjacent_value, retained_value);
    assert!(adjacent_value.is_finite());
    negatives
        .push(json!({"case":"adjacent-bracket-reuse","value_m_s":adjacent_value,"retained_value_m_s":retained_value,"passed":true}));

    // Cross-workgroup failure must prevent every fixture write, including valid lanes.
    // The same owner is reused after a fatal batch with new active-prefix controls.
    let mut batch = ResidentQueryBatch::new(&ctx, &snapshots[0].horizontal_grid, 130, 130).unwrap();
    for (case, active, invalid) in [
        ("cross-workgroup", 130, true),
        ("inactive-tail", 64, true),
        ("reuse-after-fatal", 130, false),
        ("empty-prefix", 0, true),
        ("inactive-interior", 130, false),
        ("nonfinite-query", 130, false),
    ] {
        batch.set_active_count(active).unwrap();
        let mut ps = vec![particle(&queries[0]); 130];
        ps[129].cell_x = if invalid { -1 } else { 0 };
        if case == "inactive-interior" {
            ps[1].flags = 0;
        }
        if case == "nonfinite-query" {
            ps[129].pos_z = f32::NAN;
        }
        let mut particles = ParticleBuffers::from_particles(&ctx, &ps);
        particles.set_dispatch_count(active as usize);
        let state = ResidentFixtureState::upload(&ctx, &vec![-999.0; 130]).unwrap();
        let mut plan = fields[0]
            .prepare_resident(&ctx, &mut batch, time(1800))
            .unwrap();
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        let sample = plan
            .encode_from_particles(&ctx, &particles, &kernels, &resident, &mut encoder)
            .unwrap();
        sample
            .encode_fixture_consumer(&ctx, &resident, &state, &mut encoder)
            .unwrap();
        ctx.queue.submit(Some(encoder.finish()));
        drop(plan);
        let status = pollster::block_on(batch.observe_status(&ctx)).unwrap();
        let values = read::<f32>(&ctx, &state.values, 130);
        let fatal = (invalid && active == 130) || case == "nonfinite-query";
        assert_eq!(status.fatal, fatal);
        assert_eq!(status.require_success().is_err(), fatal);
        for (lane, value) in values.iter().enumerate() {
            assert_eq!(
                *value == -999.0,
                fatal || lane >= active as usize || (case == "inactive-interior" && lane == 1)
            );
        }
        negatives.push(json!({"case":case,"status":status,"values":values,"passed":true}));
    }
    // Discarding the encoder that prepared a new source leaves its immutable status
    // unexecuted. No host success flag can turn this into accepted meteorology.
    let mut discarded = ctx.device.create_command_encoder(&Default::default());
    let pending = Arc::new(
        preparer
            .prepare(&ctx, &sources[1], &target, &mut discarded)
            .unwrap(),
    );
    drop(discarded);
    let failed = CanonicalGpuField::from_shared_height(
        &ctx,
        FieldId::WindU,
        &refs,
        &views,
        &[Arc::clone(&prepared[0]), pending],
    )
    .unwrap();
    let state = ResidentFixtureState::upload(&ctx, &[-999.0]).unwrap();
    let mut batch = ResidentQueryBatch::new(&ctx, &snapshots[0].horizontal_grid, 1, 1).unwrap();
    let particles = ParticleBuffers::from_particles(&ctx, &[particle(&queries[0])]);
    let mut plan = failed
        .prepare_resident(&ctx, &mut batch, time(1800))
        .unwrap();
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    let sample = plan
        .encode_from_particles(&ctx, &particles, &kernels, &resident, &mut encoder)
        .unwrap();
    sample
        .encode_fixture_consumer(&ctx, &resident, &state, &mut encoder)
        .unwrap();
    ctx.queue.submit(Some(encoder.finish()));
    drop(plan);
    let status = pollster::block_on(batch.observe_status(&ctx)).unwrap();
    assert!(status.require_success().is_err());
    assert_eq!(read::<f32>(&ctx, &state.values, 1), [-999.0]);
    negatives.push(json!({"case":"unexecuted-preparation","status":status,"passed":true}));

    for (case, kind) in [
        ("pending-column", 1),
        ("failed-column", 3),
        ("nonfinite-preparation", 4),
        ("failed-target", 5),
        ("nonfinite-corner", 6),
        ("incomplete-lane", 7),
    ] {
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        // Independent target prevents injected failures from changing valid reused sources.
        let fault_target = preparer
            .initialize(
                &ctx,
                &sources[0],
                SharedHeightInitialization::RegionalMother,
                &mut encoder,
            )
            .unwrap();
        let fault_prepared: Vec<_> = sources
            .iter()
            .map(|s| {
                Arc::new(
                    preparer
                        .prepare(&ctx, s, &fault_target, &mut encoder)
                        .unwrap(),
                )
            })
            .collect();
        if kind <= 4 {
            inject(&ctx, fault_prepared[1].status(), 3, kind, &mut encoder);
        }
        if kind == 5 {
            inject(&ctx, fault_target.status(), 0, 3, &mut encoder);
        }
        if kind == 6 {
            inject(
                &ctx,
                fault_prepared[1].u(),
                0,
                f32::NAN.to_bits(),
                &mut encoder,
            );
        }
        let field = CanonicalGpuField::from_shared_height(
            &ctx,
            FieldId::WindU,
            &refs,
            &views,
            &fault_prepared,
        )
        .unwrap();
        let mut batch =
            ResidentQueryBatch::new(&ctx, &snapshots[0].horizontal_grid, 130, 130).unwrap();
        let particles = ParticleBuffers::from_particles(&ctx, &vec![particle(&queries[0]); 130]);
        let state = ResidentFixtureState::upload(&ctx, &vec![-999.0; 130]).unwrap();
        let mut plan = field
            .prepare_resident(&ctx, &mut batch, time(1800))
            .unwrap();
        let sample = plan
            .encode_from_particles(&ctx, &particles, &kernels, &resident, &mut encoder)
            .unwrap();
        if kind == 7 {
            inject(&ctx, sample.status, 130, 1, &mut encoder);
            // Exercise the existing final result gate against a missing final lane.
            let shader = ctx.load_shader(
                "existing #171 final checkpoint",
                include_str!("../src/shaders/resident_sample_status.wgsl"),
            );
            let pipeline = ctx
                .device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: None,
                    layout: None,
                    module: &shader,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                });
            let params = ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&[130_u32, 130, 0, 0]),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
            let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: sample.values.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: sample.status.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: params.as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(3, 1, 1);
        }
        sample
            .encode_fixture_consumer(&ctx, &resident, &state, &mut encoder)
            .unwrap();
        ctx.queue.submit(Some(encoder.finish()));
        drop(plan);
        let status = pollster::block_on(batch.observe_status(&ctx)).unwrap();
        assert!(status.require_success().is_err(), "{case}");
        assert_eq!(read::<f32>(&ctx, &state.values, 130), vec![-999.0; 130]);
        // #76 carries the same mandatory preparation/final status at its host boundary.
        if kind != 7 {
            let mut plan = field.prepare_sample(&ctx, request).unwrap();
            let mut encoder = ctx.device.create_command_encoder(&Default::default());
            let sample = plan.encode(&ctx, &kernels, &mut encoder).unwrap();
            ctx.queue.submit(Some(encoder.finish()));
            assert_ne!(
                read::<u32>(&ctx, sample.status.unwrap(), 2)[0],
                0,
                "#76 {case}"
            );
        }
        negatives.push(json!({"case":case,"status":status,"passed":true}));
    }

    let actual_target = read::<f32>(&ctx, target.heights(), 4);
    assert_eq!(
        sha256_hex(bytemuck::cast_slice(&actual_target)),
        target.identity().height_sha256
    );
    let mut target_preimage = target.identity().clone();
    target_preimage.identity_sha256.clear();
    let serialized_target = serde_json::to_string(&target_preimage).unwrap();
    let serialized_sources: Vec<_> = snapshots
        .iter()
        .map(|s| serde_json::to_string(s).unwrap())
        .collect();
    let query_inputs: Vec<[i64; 6]> = queries
        .iter()
        .map(|q| {
            let p = particle(q);
            [
                q["time_s"].as_i64().unwrap(),
                i64::from(p.cell_x),
                i64::from(p.cell_y),
                i64::from(p.pos_x.to_bits()),
                i64::from(p.pos_y.to_bits()),
                i64::from(p.pos_z.to_bits()),
            ]
        })
        .collect();
    let query_input_sha256 = sha256_hex(&serde_json::to_vec(&query_inputs).unwrap());
    let input = serde_json::to_vec(&snapshots).unwrap();
    let shaders = [
        "shared_height_uv",
        "shared_height_query_status",
        "resident_query_adapter",
        "horizontal_interpolation",
        "vertical_sample",
        "temporal_interpolation",
        "particle_query",
        "resident_status_reset",
        "resident_sample_status",
        "resident_fixture_consumer",
    ];
    let shader_hashes: serde_json::Map<String, Value> = shaders
        .iter()
        .map(|s| {
            (
                s.to_string(),
                json!(sha256_hex(
                    &std::fs::read(format!("src/shaders/{s}.wgsl")).unwrap()
                )),
            )
        })
        .collect();
    let source_paths = [
        "src/gpu/meteorology.rs",
        "src/gpu/meteorology/resident.rs",
        "src/gpu/meteorology/shared_uv.rs",
        "src/gpu/shared_height.rs",
        "tests/shared_height_query.rs",
        "tests/support/shared_height.rs",
    ];
    let source_hashes: serde_json::Map<String, Value> = source_paths
        .iter()
        .map(|p| (p.to_string(), json!(sha256_hex(&std::fs::read(p).unwrap()))))
        .collect();
    let evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "SHARED-HEIGHT-UV-QUERY-185".into(),
        candidate: GpuCandidateEvidence {
            implementation_id: "canonical_shared_height_uv_queries".into(),
            revision: fixture::revision(),
            shader_sha256: sha256_hex(&serde_json::to_vec(&shader_hashes).unwrap()),
            input_sha256: sha256_hex(&input),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(GpuAdapterEvidence::from_context(&ctx)),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(PinnedOracleEvidence {
            implementation_id: "FLEXPART-11.1 genuine #118 QUERY".into(),
            revision: oracle["provenance"]["pinned_commit"]
                .as_str()
                .unwrap()
                .into(),
            executable_sha256: oracle["provenance"]["artifacts_sha256"]
                ["build/w-production-oracle"]
                .as_str()
                .unwrap()
                .into(),
            output_sha256: oracle["provenance"]["artifacts_sha256"]["output.txt"]
                .as_str()
                .unwrap()
                .into(),
        }),
        comparison: compare_finite_values(
            &rows
                .iter()
                .map(|r| r["oracle_m_s"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            &rows
                .iter()
                .map(|r| r["actual_m_s"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            ComparisonPolicy {
                absolute_tolerance: 1e-6,
                relative_tolerance: 1e-5,
            },
        )
        .unwrap(),
    };
    evidence.require_paired_pass().unwrap();
    let directory = std::env::var("FLEXPART_GPU_SHARED_QUERY_EVIDENCE")
        .unwrap_or_else(|_| "target/ci-gate/shared-height-query".into());
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(format!("{directory}/inputs.json"), input).unwrap();
    std::fs::write(format!("{directory}/report.json"),serde_json::to_vec_pretty(&json!({
        "schema":{"id":"flexpart-gpu.shared-height-query","version":1},"evidence":evidence,
        "rows":rows,"negative_cases":negatives,"shaders":shader_hashes,"oracle_provenance":oracle["provenance"],
        "query_inputs":query_inputs,"query_input_sha256":query_input_sha256,
        "serialized_sources":serialized_sources,"serialized_target_identity":serialized_target,
        "actual_target_height_m_agl":actual_target,"source_hashes":source_hashes,
        "targets":prepared.iter().map(|p|p.target().identity()).collect::<Vec<_>>(),
        "sources":prepared.iter().map(|p|p.source_identity()).collect::<Vec<_>>(),
        "host_metadata":outputs.iter().map(|r|&r.2).collect::<Vec<_>>(),
        "resident_metadata":resident_results.iter().map(|r|&r.2).collect::<Vec<_>>(),
        "one_encoder_one_submission":true,"scope":"U/V query integration only"
    })).unwrap()).unwrap();
    println!(
        "SHARED-HEIGHT-UV-QUERY-185 rows=36 adapter={:?}",
        ctx.adapter_info()
    );
}
