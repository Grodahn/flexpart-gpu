//! #190 complete-vector adoption through existing #76/#171 APIs.
use flexpart_gpu::{
    gpu::{
        evidence::GpuEvidenceSchema,
        meteorology::{
            resident::{ResidentFixtureState, ResidentQueryBatch, ResidentQueryKernels},
            CanonicalGpuField, MeteorologyCompositionKernels, MeteorologyHeight,
            MeteorologySampleRequest, MeteorologyTimeSelection,
        },
        shared_height::*,
        shared_height_w::*,
        vertical::sha256_hex,
        *,
    },
    meteorology::{temporal::RequestedSampleTime, vertical::*, *},
    particles::{Particle, ParticleInit},
};
use serde_json::{json, Value};
use std::sync::Arc;
use wgpu::util::DeviceExt;
#[path = "support/interior_w.rs"]
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
        "#190 final validation checkpoint",
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
        "#190 validation fault",
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

const VECTOR_CONSUMER: &str = r"
@group(0) @binding(0) var<storage,read> u:array<f32>;
@group(0) @binding(1) var<storage,read> v:array<f32>;
@group(0) @binding(2) var<storage,read> w:array<f32>;
@group(0) @binding(3) var<storage,read> us:array<u32>;
@group(0) @binding(4) var<storage,read> vs:array<u32>;
@group(0) @binding(5) var<storage,read> ws:array<u32>;
@group(0) @binding(6) var<storage,read_write> output:array<vec4<f32>>;
@group(0) @binding(7) var<uniform> counts:vec4<u32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
 let lane=id.x; if(lane>=counts.x){return;}
 if(us[0]!=0u||vs[0]!=0u||ws[0]!=0u){return;}
 // Independent dispatches completed before this consumer: reject incomplete
 // lanes anywhere in the batch before any workgroup can publish a vector.
 for(var i=1u;i<=counts.x;i++) {
   if(us[i]==1u||vs[i]==1u||ws[i]==1u||us[i]>2u||vs[i]>2u||ws[i]>2u){return;}
   if(us[i]!=vs[i]||us[i]!=ws[i]){return;}
 }
 if(lane>=counts.y||us[lane+1u]!=2u){return;}
 output[lane]=vec4<f32>(u[lane],v[lane],w[lane],1.0);
}";

fn vector_consumer(
    ctx: &GpuContext,
    values: &[wgpu::Buffer],
    statuses: &[wgpu::Buffer],
    capacity: u32,
    active: u32,
    encoder: &mut wgpu::CommandEncoder,
) -> wgpu::Buffer {
    let output = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("#190 atomic full-vector fixture"),
            contents: bytemuck::cast_slice(&vec![[-999.0_f32; 4]; capacity as usize]),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
    let params = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[capacity, active, 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let shader = ctx.load_shader("#190 bounded vector fixture", VECTOR_CONSUMER);
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
    let buffers = [
        &values[0],
        &values[1],
        &values[2],
        &statuses[0],
        &statuses[1],
        &statuses[2],
        &output,
        &params,
    ];
    let entries: Vec<_> = buffers
        .iter()
        .enumerate()
        .map(|(i, b)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: b.as_entire_binding(),
        })
        .collect();
    let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });
    let mut pass = encoder.begin_compute_pass(&Default::default());
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.dispatch_workgroups(capacity.div_ceil(64), 1, 1);
    drop(pass);
    output
}

#[allow(clippy::too_many_arguments)]
fn run_vector(
    ctx: &GpuContext,
    fields: &[CanonicalGpuField<'_>; 3],
    particles: &[Particle],
    active: u32,
    kernels: &MeteorologyCompositionKernels<'_>,
    resident: &ResidentQueryKernels<'_>,
    mut encoder: wgpu::CommandEncoder,
    fault: Option<(usize, u32, u32)>,
) -> Value {
    let capacity = particles.len() as u32;
    let mut input = ParticleBuffers::from_particles(ctx, particles);
    input.set_dispatch_count(active as usize);
    let grid = fixture::snapshot(1).horizontal_grid;
    let mut batches: Vec<_> = (0..3)
        .map(|_| ResidentQueryBatch::new(ctx, &grid, capacity, active).unwrap())
        .collect();
    let mut plans: Vec<_> = fields
        .iter()
        .zip(&mut batches)
        .map(|(f, b)| f.prepare_resident(ctx, b, time(1800)).unwrap())
        .collect();
    let mut values = Vec::new();
    let mut statuses = Vec::new();
    for plan in &mut plans {
        let sample = plan
            .encode_from_particles(ctx, &input, kernels, resident, &mut encoder)
            .unwrap();
        values.push(sample.values.clone());
        statuses.push(sample.status.clone());
    }
    if let Some((component, word, bits)) = fault {
        inject(ctx, &statuses[component], word, bits, &mut encoder);
    }
    let output = vector_consumer(ctx, &values, &statuses, capacity, active, &mut encoder);
    ctx.queue.submit(Some(encoder.finish()));
    let output = read::<[f32; 4]>(ctx, &output, particles.len());
    drop(plans);
    let statuses: Vec<_> = batches
        .iter()
        .map(|b| pollster::block_on(b.observe_status(ctx)).unwrap())
        .collect();
    json!({"statuses":statuses,"output":output})
}
#[test]
fn test_shared_vector_all_public_query_rows_identity_and_fatal_atomicity() {
    let ctx = pollster::block_on(GpuContext::new()).expect("#190 requires actual WGSL execution");
    for filter in [
        wgpu::ErrorFilter::OutOfMemory,
        wgpu::ErrorFilter::Internal,
        wgpu::ErrorFilter::Validation,
    ] {
        ctx.device.push_error_scope(filter);
    }
    let kernels = MeteorologyCompositionKernels::new(&ctx).unwrap();
    let resident = ResidentQueryKernels::new(&ctx).unwrap();
    let preparer = pollster::block_on(SharedHeightPreparer::new(&ctx)).unwrap();
    let snapshots = [fixture::snapshot(1), fixture::snapshot(2)];
    let geometries: Vec<_> = snapshots
        .iter()
        .enumerate()
        .map(|(i, s)| {
            reconstruct_vertical_geometry_with_motion(s, &fixture::native(i + 1)).unwrap()
        })
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
    let w_preparer = pollster::block_on(SharedHeightWPreparer::new(&ctx)).unwrap();
    let w_sources: Vec<_> = refs
        .iter()
        .zip(&views)
        .zip(&prepared)
        .map(|((s, g), uv)| SharedHeightWSource::upload(&ctx, s, *g, Arc::clone(uv)).unwrap())
        .collect();
    let w: Vec<_> = w_sources
        .iter()
        .map(|s| Arc::new(w_preparer.prepare(&ctx, s, &mut encoder).unwrap()))
        .collect();
    let fields =
        CanonicalGpuField::from_shared_height_vector(&ctx, &refs, &views, &prepared, &w).unwrap();
    let oracle = fixture::oracle();
    let queries: Vec<Value> = oracle["scientific"]["queries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|q| q["key"][0] == "full")
        .map(|q| {
            let v = &q["time_x_y_agl_asl_uv_w"];
            json!({"query":q["key"][1],"time_s":v[0].as_f64().unwrap() as i64,
            "x":v[1],"y":v[2],"height_m_agl":v[3],"oracle_uv_w_m_s":[v[5],v[6],v[7]]})
        })
        .collect();
    assert_eq!(queries.len(), 15);
    let mut outputs = Vec::new();
    // All preparation, host queries, particle producers, samplers and consumers share
    // this caller encoder. Only final consumer/status checkpoints are downloaded.
    let mut host_plans = Vec::new();
    for field in &fields {
        for row in &queries {
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
        for row in &queries {
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
    let vector_outputs: Vec<_> = (0..15)
        .map(|q| {
            vector_consumer(
                &ctx,
                &(0..3)
                    .map(|c| resident_results[c * 15 + q].0.clone())
                    .collect::<Vec<_>>(),
                &(0..3)
                    .map(|c| resident_results[c * 15 + q].1.clone())
                    .collect::<Vec<_>>(),
                1,
                1,
                &mut encoder,
            )
        })
        .collect();
    ctx.queue.submit(Some(encoder.finish()));
    let mut vector_rows = Vec::new();
    for (q, output) in vector_outputs.iter().enumerate() {
        let value = read::<[f32; 4]>(&ctx, output, 1)[0];
        assert_eq!(value[3], 1.0);
        for (c, component) in value.iter().take(3).enumerate() {
            vector_rows.push(compare(*component, &queries[q], c, "fixture"));
        }
    }
    let mut rows = Vec::new();
    for (index, (result, status, _)) in outputs.iter().enumerate() {
        assert_eq!(read::<u32>(&ctx, status, 2), [0, 2]);
        rows.push(compare(
            read::<f32>(&ctx, result, 1)[0],
            &queries[index % 15],
            index / 15,
            "#76",
        ));
    }
    for (index, (state, status, _)) in resident_results.iter().enumerate() {
        assert_eq!(read::<u32>(&ctx, status, 2), [0, 2]);
        rows.push(compare(
            read::<f32>(&ctx, state, 1)[0],
            &queries[index % 15],
            index / 15,
            "#171",
        ));
    }

    let mut negatives = Vec::new();
    macro_rules! rejected {
        ($name:expr,$expression:expr)=>{{assert!($expression.is_err(),"{}",$name);
            negatives.push(json!({"case":$name,"passed":true}));}};
    }
    rejected!(
        "missing-w",
        CanonicalGpuField::from_shared_height_vector(&ctx, &refs, &views, &prepared, &[])
    );
    rejected!(
        "missing-uv",
        CanonicalGpuField::from_shared_height_vector(&ctx, &refs, &views, &[], &w)
    );
    rejected!(
        "reversed-w",
        CanonicalGpuField::from_shared_height_vector(
            &ctx,
            &refs,
            &views,
            &prepared,
            &[Arc::clone(&w[1]), Arc::clone(&w[0])]
        )
    );
    rejected!(
        "reversed-uv",
        CanonicalGpuField::from_shared_height_vector(
            &ctx,
            &refs,
            &views,
            &[Arc::clone(&prepared[1]), Arc::clone(&prepared[0])],
            &w
        )
    );
    rejected!(
        "wrong-geometry",
        CanonicalGpuField::from_shared_height_vector(
            &ctx,
            &refs,
            &[views[1], views[0]],
            &prepared,
            &w
        )
    );
    let mut changed = snapshots[1].clone();
    changed.fields[0].values[0] += 1.0;
    rejected!(
        "wrong-source",
        CanonicalGpuField::from_shared_height_vector(
            &ctx,
            &[&snapshots[0], &changed],
            &views,
            &prepared,
            &w
        )
    );
    let mut changed = snapshots[1].clone();
    for f in &mut changed.fields {
        f.time.valid_time_epoch_seconds += 1;
    }
    rejected!(
        "wrong-time",
        CanonicalGpuField::from_shared_height_vector(
            &ctx,
            &[&snapshots[0], &changed],
            &views,
            &prepared,
            &w
        )
    );
    let mut changed = snapshots[1].clone();
    changed.horizontal_grid.dx_deg *= 2.0;
    rejected!(
        "wrong-horizontal",
        CanonicalGpuField::from_shared_height_vector(
            &ctx,
            &[&snapshots[0], &changed],
            &views,
            &prepared,
            &w
        )
    );
    let mut motion = fixture::native(2);
    motion.provenance.source_id.push_str("-other");
    let different = reconstruct_vertical_geometry_with_motion(&snapshots[1], &motion).unwrap();
    rejected!(
        "wrong-normalization",
        CanonicalGpuField::from_shared_height_vector(
            &ctx,
            &refs,
            &[views[0], different.runtime_view().unwrap()],
            &prepared,
            &w
        )
    );
    let mut scratch = ctx.device.create_command_encoder(&Default::default());
    let alternate_target = preparer
        .initialize(
            &ctx,
            &sources[0],
            SharedHeightInitialization::RegionalMother,
            &mut scratch,
        )
        .unwrap();
    let alternate_uv: Vec<_> = sources
        .iter()
        .map(|s| {
            Arc::new(
                preparer
                    .prepare(&ctx, s, &alternate_target, &mut scratch)
                    .unwrap(),
            )
        })
        .collect();
    rejected!(
        "wrong-target",
        CanonicalGpuField::from_shared_height_vector(&ctx, &refs, &views, &alternate_uv, &w)
    );
    let duplicate_uv: Vec<_> = sources
        .iter()
        .map(|s| Arc::new(preparer.prepare(&ctx, s, &target, &mut scratch).unwrap()))
        .collect();
    rejected!(
        "different-uv-owner",
        CanonicalGpuField::from_shared_height_vector(&ctx, &refs, &views, &duplicate_uv, &w)
    );
    // Diagnostic correction types deliberately have no conversion into PreparedSharedHeightW.
    let other = pollster::block_on(GpuContext::new()).unwrap();
    rejected!(
        "wrong-context",
        CanonicalGpuField::from_shared_height_vector(&other, &refs, &views, &prepared, &w)
    );
    drop(other);
    let request = MeteorologySampleRequest {
        xt: 0.7,
        yt: 0.6,
        height: Some(MeteorologyHeight {
            meters: queries[2]["height_m_agl"].as_f64().unwrap() as f32,
            reference: VerticalReference::AboveGroundLevel,
        }),
        time: time(1800),
    };
    for (component, field) in fields.iter().enumerate() {
        rejected!(
            format!("asl-{component}"),
            field.prepare_sample(
                &ctx,
                MeteorologySampleRequest {
                    height: Some(MeteorologyHeight {
                        meters: 100.0,
                        reference: VerticalReference::AboveMeanSeaLevel
                    }),
                    ..request
                }
            )
        );
        rejected!(
            format!("outside-time-{component}"),
            field.prepare_sample(
                &ctx,
                MeteorologySampleRequest {
                    time: time(3601),
                    ..request
                }
            )
        );
    }
    rejected!(
        "invalid-capacity",
        ResidentQueryBatch::new(&ctx, &snapshots[0].horizontal_grid, 0, 0)
    );
    rejected!(
        "invalid-prefix",
        ResidentQueryBatch::new(&ctx, &snapshots[0].horizontal_grid, 1, 2)
    );
    let native_field = CanonicalGpuField::upload(
        &ctx,
        FieldId::WindU,
        &refs,
        &views.iter().copied().map(Some).collect::<Vec<_>>(),
    )
    .unwrap();
    rejected!(
        "native-nonuniform",
        native_field.prepare_sample(&ctx, request)
    );
    // A missing W field would reject at schema validation and never exercise
    // the native interface path's unsupported multi-column geometry boundary.
    let native_snapshots: Vec<_> = snapshots
        .iter()
        .zip(&geometries)
        .map(|(snapshot, geometry)| {
            let mut snapshot = snapshot.clone();
            let mut field = snapshot
                .fields
                .iter()
                .find(|f| f.id == FieldId::WindU)
                .unwrap()
                .clone();
            field.id = FieldId::VerticalVelocity;
            field.sign = SignConvention::PositiveUpward;
            field.vertical_staggering = VerticalStaggering::LevelInterface;
            field.shape[0] += 1;
            field.values = geometry
                .runtime_view()
                .unwrap()
                .vertical_velocity()
                .unwrap()
                .values_ms()
                .to_vec();
            snapshot.fields.push(field);
            snapshot.validate(&Requirements::advection()).unwrap();
            snapshot
        })
        .collect();
    let native_geometries: Vec<_> = native_snapshots
        .iter()
        .enumerate()
        .map(|(i, s)| {
            reconstruct_vertical_geometry_with_motion(s, &fixture::native(i + 1)).unwrap()
        })
        .collect();
    let native_error = CanonicalGpuField::upload(
        &ctx,
        FieldId::VerticalVelocity,
        &native_snapshots.iter().collect::<Vec<_>>(),
        &native_geometries
            .iter()
            .map(|g| Some(g.runtime_view().unwrap()))
            .collect::<Vec<_>>(),
    )
    .err()
    .expect("native multi-column interface W must reject");
    let flexpart_gpu::gpu::meteorology::MeteorologyCompositionError::Vertical(
        flexpart_gpu::gpu::vertical::GpuVerticalError::Sampling(
            flexpart_gpu::meteorology::vertical_sampling::VerticalSamplingError::UnsupportedInterfaceRuntime {reason}
        )
    ) = native_error else {panic!("unexpected native W rejection: {native_error}")};
    assert_eq!(
        reason,
        "#80 freezes a single vertical column without horizontal slope correction"
    );
    negatives.push(json!({"case":"native-interface-w","passed":true,"rejection":reason}));
    for (case, active, mut parts, fault, expected_valid) in [
        (
            "cross-workgroup",
            130,
            vec![particle(&queries[2]); 130],
            None,
            false,
        ),
        (
            "inactive-tail",
            64,
            vec![particle(&queries[2]); 130],
            None,
            true,
        ),
        (
            "empty-prefix",
            0,
            vec![particle(&queries[2]); 130],
            None,
            true,
        ),
        (
            "inactive-interior",
            130,
            vec![particle(&queries[2]); 130],
            None,
            true,
        ),
        (
            "nonfinite-query",
            130,
            vec![particle(&queries[2]); 130],
            None,
            false,
        ),
        (
            "incomplete-w-lane",
            130,
            vec![particle(&queries[2]); 130],
            Some((2, 130, 1)),
            false,
        ),
    ] {
        if case == "cross-workgroup" || case == "inactive-tail" {
            parts[129].cell_x = -1;
        }
        if case == "nonfinite-query" {
            parts[129].pos_z = f32::NAN;
        }
        if case == "inactive-interior" {
            parts[1].flags = 0;
        }
        let result = run_vector(
            &ctx,
            &fields,
            &parts,
            active,
            &kernels,
            &resident,
            ctx.device.create_command_encoder(&Default::default()),
            fault,
        );
        let statuses = result["statuses"].as_array().unwrap();
        for status in statuses {
            assert_eq!(
                status["fatal"],
                !expected_valid && case != "incomplete-w-lane"
            );
        }
        for (i, value) in result["output"].as_array().unwrap().iter().enumerate() {
            if !expected_valid || i >= active as usize || (case == "inactive-interior" && i == 1) {
                assert_eq!(*value, json!(vec![-999.0; 4]));
            } else {
                assert_eq!(value[3], 1.0);
            }
        }
        negatives.push(json!({"case":case,"passed":true,"result":result}));
    }
    // Pending or invalid W must also block otherwise valid U/V, outside the query stencil.
    ctx.queue.submit(Some(scratch.finish()));
    for (case, kind) in [
        ("unexecuted-w", 0),
        ("remapped-w", 1),
        ("failed-w", 3),
        ("nonfinite-w-status", 4),
        ("nonfinite-w-corner", 5),
        ("failed-uv", 6),
        ("failed-target", 7),
        ("nonfinite-v-corner", 8),
    ] {
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        let fault_uv: Vec<_> = sources
            .iter()
            .map(|s| {
                Arc::new(
                    preparer
                        .prepare(&ctx, s, &alternate_target, &mut encoder)
                        .unwrap(),
                )
            })
            .collect();
        // Initialize this separate target before its dependents, in the same caller encoder.
        // The earlier scratch encoder was intentionally never submitted.
        let fault_w_sources: Vec<_> = refs
            .iter()
            .zip(&views)
            .zip(&fault_uv)
            .map(|((s, g), uv)| SharedHeightWSource::upload(&ctx, s, *g, Arc::clone(uv)).unwrap())
            .collect();
        let mut unused = ctx.device.create_command_encoder(&Default::default());
        let fault_w: Vec<_> = fault_w_sources
            .iter()
            .map(|s| {
                Arc::new(
                    w_preparer
                        .prepare(&ctx, s, if kind == 0 { &mut unused } else { &mut encoder })
                        .unwrap(),
                )
            })
            .collect();
        if (1..=4).contains(&kind) {
            inject(&ctx, fault_w[1].status(), 8, kind, &mut encoder);
        }
        if kind == 5 {
            inject(&ctx, fault_w[1].w(), 8, f32::NAN.to_bits(), &mut encoder);
        }
        if kind == 6 {
            inject(&ctx, fault_uv[1].status(), 8, 3, &mut encoder);
        }
        if kind == 7 {
            inject(&ctx, alternate_target.status(), 0, 3, &mut encoder);
        }
        if kind == 8 {
            inject(&ctx, fault_uv[1].v(), 8, f32::NAN.to_bits(), &mut encoder);
        }
        let fields =
            CanonicalGpuField::from_shared_height_vector(&ctx, &refs, &views, &fault_uv, &fault_w)
                .unwrap();
        let result = run_vector(
            &ctx,
            &fields,
            &vec![particle(&queries[2]); 130],
            130,
            &kernels,
            &resident,
            encoder,
            None,
        );
        for status in result["statuses"].as_array().unwrap() {
            assert_eq!(status["fatal"], true, "{case}");
        }
        for value in result["output"].as_array().unwrap() {
            assert_eq!(*value, json!(vec![-999.0; 4]), "{case}");
        }
        for field in &fields {
            let mut plan = field.prepare_sample(&ctx, request).unwrap();
            let mut encoder = ctx.device.create_command_encoder(&Default::default());
            let sample = plan.encode(&ctx, &kernels, &mut encoder).unwrap();
            ctx.queue.submit(Some(encoder.finish()));
            assert_ne!(
                read::<u32>(&ctx, sample.status.unwrap(), 2)[0],
                0,
                "host {case}"
            );
        }
        negatives.push(json!({"case":case,"passed":true,"result":result}));
        // Restore the dedicated test target for the next independently injected failure.
        if kind == 7 {
            let mut reset = ctx.device.create_command_encoder(&Default::default());
            inject(&ctx, alternate_target.status(), 0, 2, &mut reset);
            ctx.queue.submit(Some(reset.finish()));
        }
    }

    let mut next = snapshots[1].clone();
    for f in &mut next.fields {
        if f.time.kind != TemporalKind::Static {
            f.time.valid_time_epoch_seconds = 7200;
        }
    }
    let next_geometry =
        reconstruct_vertical_geometry_with_motion(&next, &fixture::native(2)).unwrap();
    let next_view = next_geometry.runtime_view().unwrap();
    let next_source = SharedHeightSource::upload(&ctx, &next, next_view).unwrap();
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    let next_uv = Arc::new(
        preparer
            .prepare(&ctx, &next_source, &target, &mut encoder)
            .unwrap(),
    );
    let next_w_source =
        SharedHeightWSource::upload(&ctx, &next, next_view, Arc::clone(&next_uv)).unwrap();
    let next_w = Arc::new(
        w_preparer
            .prepare(&ctx, &next_w_source, &mut encoder)
            .unwrap(),
    );
    let adjacent = CanonicalGpuField::from_shared_height_vector(
        &ctx,
        &[&snapshots[1], &next],
        &[views[1], next_view],
        &[Arc::clone(&prepared[1]), next_uv],
        &[Arc::clone(&w[1]), next_w],
    )
    .unwrap();
    let mut adjacent_plans: Vec<_> = adjacent
        .iter()
        .map(|f| {
            f.prepare_sample(
                &ctx,
                MeteorologySampleRequest {
                    time: time(3600),
                    ..request
                },
            )
            .unwrap()
        })
        .collect();
    let mut retained_plans: Vec<_> = fields
        .iter()
        .map(|f| {
            f.prepare_sample(
                &ctx,
                MeteorologySampleRequest {
                    time: time(3600),
                    ..request
                },
            )
            .unwrap()
        })
        .collect();
    let mut reused = Vec::new();
    for (a, b) in adjacent_plans.iter_mut().zip(&mut retained_plans) {
        let a = a.encode(&ctx, &kernels, &mut encoder).unwrap();
        let b = b.encode(&ctx, &kernels, &mut encoder).unwrap();
        reused.push((
            a.values.clone(),
            b.values.clone(),
            a.status.unwrap().clone(),
            b.status.unwrap().clone(),
        ));
    }
    ctx.queue.submit(Some(encoder.finish()));
    let reused_values: Vec<_> = reused
        .iter()
        .map(|(a, b, sa, sb)| {
            assert_eq!(read::<u32>(&ctx, sa, 2), [0, 2]);
            assert_eq!(read::<u32>(&ctx, sb, 2), [0, 2]);
            let a = read::<f32>(&ctx, a, 1)[0];
            let b = read::<f32>(&ctx, b, 1)[0];
            assert_eq!(a, b);
            [a, b]
        })
        .collect();
    negatives.push(json!({"case":"adjacent-bracket-reuse","passed":true,"values":reused_values}));
    for _ in 0..3 {
        assert!(pollster::block_on(ctx.device.pop_error_scope()).is_none());
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
    let motions = [fixture::native(1), fixture::native(2)];
    let input =
        serde_json::to_vec(&json!({"snapshots":snapshots,"native_motion":motions})).unwrap();
    let native_interface_lanes: Vec<Vec<[f32; 2]>> = views
        .iter()
        .map(|g| {
            let motion = g.vertical_velocity().unwrap();
            (0..4)
                .flat_map(|z| {
                    (0..9).map(move |c| {
                        [
                            g.interface(c % 3, c / 3, 3 - z).unwrap().height_agl_m,
                            motion.values_ms()[(3 - z) * 9 + c],
                        ]
                    })
                })
                .collect()
        })
        .collect();
    let shaders = [
        "shared_height_uv",
        "shared_height_w_remap",
        "interior_w_slope",
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
        "src/gpu/meteorology/shared_w.rs",
        "src/gpu/shared_height_w.rs",
        "src/gpu/shared_height.rs",
        "tests/shared_height_vector.rs",
        "tests/support/interior_w.rs",
    ];
    let source_hashes: serde_json::Map<String, Value> = source_paths
        .iter()
        .map(|p| (p.to_string(), json!(sha256_hex(&std::fs::read(p).unwrap()))))
        .collect();
    let evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "SHARED-HEIGHT-UVW-QUERY-190".into(),
        candidate: GpuCandidateEvidence {
            implementation_id: "canonical_shared_height_uvw_queries".into(),
            revision: std::env::var("FLEXPART_GPU_CANDIDATE_REVISION").unwrap_or_else(|_| {
                String::from_utf8(
                    std::process::Command::new("git")
                        .args(["rev-parse", "HEAD"])
                        .output()
                        .unwrap()
                        .stdout,
                )
                .unwrap()
                .trim()
                .into()
            }),
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
            implementation_id: "FLEXPART-11.1 genuine #186 full QUERY".into(),
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
    let directory = std::env::var("FLEXPART_GPU_SHARED_VECTOR_EVIDENCE")
        .unwrap_or_else(|_| "target/ci-gate/shared-height-vector".into());
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(format!("{directory}/inputs.json"), input).unwrap();
    std::fs::write(format!("{directory}/report.json"),serde_json::to_vec_pretty(&json!({
        "schema":{"id":"flexpart-gpu.shared-height-vector","version":1},"evidence":evidence,
        "rows":rows,"vector_rows":vector_rows,"vector_consumer_sha256":sha256_hex(VECTOR_CONSUMER.as_bytes()),
        "w_sources":w_sources.iter().map(|s|s.identity()).collect::<Vec<_>>(),
        "uv_metadata":prepared.iter().map(|s|s.metadata()).collect::<Vec<_>>(),
        "w_metadata":w.iter().map(|s|s.metadata()).collect::<Vec<_>>(),
        "serialized_uv_metadata":prepared.iter().map(|s|serde_json::to_string(&s.metadata()).unwrap()).collect::<Vec<_>>(),
        "serialized_native_motion":motions.iter().map(|s|serde_json::to_string(s).unwrap()).collect::<Vec<_>>(),
        "serialized_grids":snapshots.iter().map(|s|serde_json::to_string(&s.horizontal_grid).unwrap()).collect::<Vec<_>>(),
        "native_interface_lanes":native_interface_lanes,
        "final_uv_status":prepared.iter().map(|p|read::<u32>(&ctx,p.status(),9)).collect::<Vec<_>>(),
        "actual_prepared_w":w.iter().map(|p|read::<f32>(&ctx,p.w(),36)).collect::<Vec<_>>(),
        "final_w_status":w.iter().map(|p|read::<u32>(&ctx,p.status(),9)).collect::<Vec<_>>(),
        "scoped_device_errors":[],"intermediate_d2h_count":0,
        "candidate_executable_sha256":sha256_hex(&std::fs::read(std::env::current_exe().unwrap()).unwrap()),"negative_cases":negatives,"shaders":shader_hashes,"oracle_provenance":oracle["provenance"],
        "query_inputs":query_inputs,"query_input_sha256":query_input_sha256,
        "serialized_sources":serialized_sources,"serialized_target_identity":serialized_target,
        "actual_target_height_m_agl":actual_target,"source_hashes":source_hashes,
        "targets":prepared.iter().map(|p|p.target().identity()).collect::<Vec<_>>(),
        "sources":prepared.iter().map(|p|p.source_identity()).collect::<Vec<_>>(),
        "host_metadata":outputs.iter().map(|r|&r.2).collect::<Vec<_>>(),
        "resident_metadata":resident_results.iter().map(|r|&r.2).collect::<Vec<_>>(),
        "one_encoder_one_submission":true,"scope":"U/V/W query integration only"
    })).unwrap()).unwrap();
    println!(
        "SHARED-HEIGHT-UVW-QUERY-190 rows=90 adapter={:?}",
        ctx.adapter_info()
    );
}
