//! #184 preparation-only device proof against every #118 U/V shared row.
use flexpart_gpu::gpu::evidence::GpuEvidenceSchema;
use flexpart_gpu::gpu::{shared_height::*, vertical::sha256_hex, *};
use flexpart_gpu::meteorology::{vertical::reconstruct_vertical_geometry, *};
use serde_json::{json, Value};

fn oracle() -> Value {
    let path = std::env::var("FLEXPART_GPU_SHARED_HEIGHT_ORACLE")
        .unwrap_or_else(|_| "fixtures/interpolation/shared-height-v1/report.json".into());
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn snapshot(memory: usize) -> Snapshot {
    let report = oracle();
    let mut base: Value = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-column-v1.json"
    ))
    .unwrap();
    base["horizontal_grid"]["nx"] = json!(2);
    base["horizontal_grid"]["ny"] = json!(2);
    let coord = &mut base["vertical_coordinate"];
    coord["hybrid_a_interface_pa"] = json!([10000.0, 10000.0, 5000.0, 0.0]);
    coord["hybrid_b_interface"] = json!([0.1, 0.3, 0.7, 1.0]);
    coord["interface_values"] = json!([20000.0, 40000.0, 75000.0, 100000.0]);
    coord["level_values"] = json!([30000.0, 57500.0, 87500.0]);
    let columns: Vec<_> = report["decoded_inputs"]["columns_repeated_in_y"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["memory"] == memory)
        .collect();
    let original = base["fields"].as_array().unwrap().clone();
    let specs = [
        ("surface_pressure", "pascal", "non_negative", None),
        ("temperature", "kelvin", "signed_scalar", Some(0)),
        (
            "specific_humidity",
            "kilogram_per_kilogram",
            "non_negative",
            Some(1),
        ),
        ("wind_u", "meter_per_second", "positive_eastward", Some(2)),
        ("wind_v", "meter_per_second", "positive_northward", Some(3)),
        ("orography", "meter", "signed_scalar", None),
        ("temperature2m", "kelvin", "signed_scalar", None),
        ("dewpoint2m", "kelvin", "signed_scalar", None),
        ("wind_u10m", "meter_per_second", "positive_eastward", None),
        ("wind_v10m", "meter_per_second", "positive_northward", None),
    ];
    let mut fields = Vec::new();
    for (id, unit, sign, lane) in specs {
        let mut f = original[usize::from(lane.is_some())].clone();
        f["id"] = json!(id);
        f["unit"] = json!(unit);
        f["sign"] = json!(sign);
        f["shape"] = if lane.is_some() {
            json!([2, 2, 3])
        } else {
            json!([2, 2])
        };
        let mut values = Vec::new();
        for z in 0..if lane.is_some() { 3 } else { 1 } {
            for _y in 0..2 {
                for (x, c) in columns.iter().enumerate() {
                    let value = match lane {
                        Some(l) => c["rows_t_k_q_kg_kg_u_m_s_v_m_s_omega_pa_s"][3 - z][l].clone(),
                        None => match id {
                            "surface_pressure" => c["pressure_pa"].clone(),
                            "temperature2m" => c["temperature_2m_k"].clone(),
                            "dewpoint2m" => c["dewpoint_2m_k"].clone(),
                            "orography" => report["decoded_inputs"]["terrain_m_asl"][x].clone(),
                            _ => c["rows_t_k_q_kg_kg_u_m_s_v_m_s_omega_pa_s"][0]
                                [if id == "wind_u10m" { 2 } else { 3 }]
                            .clone(),
                        },
                    };
                    values.push(value);
                }
            }
        }
        f["values"] = json!(values);
        f["time"]["valid_time_epoch_seconds"] = json!((memory - 1) * 3600);
        if id == "orography" {
            f["time"]["kind"] = json!("static");
        }
        fields.push(f);
    }
    base["fields"] = json!(fields);
    serde_json::from_value(base).unwrap()
}

const CONSUMER: &str = r"
@group(0) @binding(0) var<storage,read> u: array<f32>;
@group(0) @binding(1) var<storage,read> v: array<f32>;
@group(0) @binding(2) var<storage,read> heights: array<f32>;
@group(0) @binding(3) var<storage,read> target_status: array<u32>;
@group(0) @binding(4) var<storage,read> status: array<u32>;
@group(0) @binding(5) var<storage,read_write> result: array<vec4<f32>>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
 let i=id.x; if (i>=arrayLength(&u)) {return;}
 result[i]=vec4<f32>(0.0);
 // A single failed or pending column prevents acceptance of the entire source.
 if (target_status[0]!=2u) {return;}
 for (var c=0u;c<arrayLength(&status);c++) {if(status[c]!=2u){return;}}
 let columns=arrayLength(&status);
 result[i]=vec4<f32>(u[i],v[i],heights[i/columns],1.0);
}";

fn consume(
    ctx: &GpuContext,
    prepared: &PreparedSharedHeightUv,
    encoder: &mut wgpu::CommandEncoder,
) -> wgpu::Buffer {
    let output = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fixture status-aware consumer"),
        size: 16 * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let shader = ctx.load_shader("fixture consumer", CONSUMER);
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
        prepared.u(),
        prepared.v(),
        prepared.target().heights(),
        prepared.target().status(),
        prepared.status(),
        &output,
    ];
    let entries: Vec<_> = buffers
        .iter()
        .enumerate()
        .map(|(binding, b)| wgpu::BindGroupEntry {
            binding: binding as u32,
            resource: b.as_entire_binding(),
        })
        .collect();
    let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("fixture consumer"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.dispatch_workgroups(1, 1, 1);
    drop(pass);
    output
}

fn revision() -> String {
    std::env::var("FLEXPART_GPU_CANDIDATE_REVISION").unwrap_or_else(|_| {
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
    })
}

#[test]
fn test_shared_height_every_pinned_uv_row_and_reuse() {
    let ctx = pollster::block_on(GpuContext::new()).expect("required device execution cannot skip");
    let preparer = pollster::block_on(SharedHeightPreparer::new(&ctx)).unwrap();
    let snapshots = [snapshot(1), snapshot(2)];
    let geometry: Vec<_> = snapshots
        .iter()
        .map(|s| reconstruct_vertical_geometry(s).unwrap())
        .collect();
    let sources: Vec<_> = snapshots
        .iter()
        .zip(&geometry)
        .map(|(s, g)| SharedHeightSource::upload(&ctx, s, g.runtime_view().unwrap()).unwrap())
        .collect();
    ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
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
        .map(|s| preparer.prepare(&ctx, s, &target, &mut encoder).unwrap())
        .collect();
    let outputs: Vec<_> = prepared
        .iter()
        .map(|p| consume(&ctx, p, &mut encoder))
        .collect();
    // Retained Arc owners keep uploads and the target alive after caller handles disappear.
    prepared[1]
        .require_reuse(&ctx, &sources[1], &target)
        .unwrap();
    assert!(prepared[0]
        .require_reuse(&ctx, &sources[1], &target)
        .is_err());
    assert_eq!(
        prepared[0].target().identity(),
        prepared[1].target().identity()
    );
    drop(sources);
    drop(target);
    ctx.queue.submit(Some(encoder.finish()));
    let report = oracle();
    let provenance = &report["provenance"];
    let pin: Value = serde_json::from_str(include_str!("../reference/flexpart-11.1.json")).unwrap();
    assert_eq!(provenance["pinned_commit"], pin["pinned_commit"]);
    assert_eq!(provenance["checkout_clean"], true);
    let mut rows = Vec::new();
    let mut actual_values = Vec::new();
    let mut expected_values = Vec::new();
    for (memory, output) in outputs.iter().enumerate() {
        let actual = pollster::block_on(download_buffer_typed::<[f32; 4]>(
            &ctx,
            output,
            16,
            "shared-height consumer",
        ))
        .unwrap();
        for z in 0..4 {
            for y in 0..2 {
                for x in 0..2 {
                    let lane = actual[z * 4 + y * 2 + x];
                    assert_eq!(lane[3], 1.0, "consumer must check both statuses");
                    let expected = report["shared_rows"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| r["memory"] == memory + 1 && r["x"] == x && r["level"] == z + 1)
                        .unwrap();
                    let oracle_height = expected["height_m_agl"].as_f64().unwrap();
                    if z == 0 || z == 3 {
                        for component in 0..2 {
                            assert_eq!(
                                lane[component],
                                expected["uv_w_m_s"][component].as_f64().unwrap() as f32,
                                "endpoint copies must be exact"
                            );
                        }
                    }
                    assert!(
                        (f64::from(lane[2]) - oracle_height).abs()
                            <= 0.02_f64.max(1e-5 * oracle_height.abs())
                    );
                    for component in 0..2 {
                        let e = expected["uv_w_m_s"][component].as_f64().unwrap();
                        let a = f64::from(lane[component]);
                        let difference = (a - e).abs();
                        let limit = 1e-6 + 1e-5 * a.abs().max(e.abs());
                        assert!(
                            a.is_finite() && e.is_finite() && difference <= limit,
                            "memory={} z={} x={} component={} actual={} expected={} diff={}",
                            memory + 1,
                            z,
                            x,
                            component,
                            a,
                            e,
                            difference
                        );
                        rows.push(json!({"memory":memory+1,"x":x,"y":y,"level":z+1,"component":component,"actual_m_s":a,"oracle_m_s":e,"absolute_difference_m_s":difference,"allowed_difference_m_s":limit}));
                        actual_values.push(a);
                        expected_values.push(e);
                    }
                }
            }
        }
        let status = pollster::block_on(download_buffer_typed::<u32>(
            &ctx,
            prepared[memory].status(),
            4,
            "member status",
        ))
        .unwrap();
        assert_eq!(status, vec![2; 4]);
    }
    let gpu_target = pollster::block_on(download_buffer_typed::<f32>(
        &ctx,
        prepared[0].target().heights(),
        4,
        "target",
    ))
    .unwrap();
    assert_eq!(
        sha256_hex(bytemuck::cast_slice(&gpu_target)),
        prepared[0].target().identity().height_sha256
    );
    assert!(pollster::block_on(ctx.device.pop_error_scope()).is_none());
    // Adjacent [t1,t2] bracket reuses t1 exactly; t2 changes fields but keeps the run target.
    let same_source =
        SharedHeightSource::upload(&ctx, &snapshots[1], geometry[1].runtime_view().unwrap())
            .unwrap();
    prepared[1]
        .require_reuse(&ctx, &same_source, prepared[0].target())
        .unwrap();
    let mut next = snapshots[1].clone();
    for field in &mut next.fields {
        if field.time.kind != TemporalKind::Static {
            field.time.valid_time_epoch_seconds = 7200;
        }
        if field.id == FieldId::WindU10m {
            field.values[0] += 12.0;
        }
    }
    let next_geometry = reconstruct_vertical_geometry(&next).unwrap();
    let next_source =
        SharedHeightSource::upload(&ctx, &next, next_geometry.runtime_view().unwrap()).unwrap();
    assert!(prepared[1]
        .require_reuse(&ctx, &next_source, prepared[0].target())
        .is_err());
    let mut next_encoder = ctx.device.create_command_encoder(&Default::default());
    let next_prepared = preparer
        .prepare(&ctx, &next_source, prepared[0].target(), &mut next_encoder)
        .unwrap();
    assert_eq!(
        next_prepared.target().identity(),
        prepared[0].target().identity()
    );
    let next_output = consume(&ctx, &next_prepared, &mut next_encoder);
    ctx.queue.submit(Some(next_encoder.finish()));
    let next_values = pollster::block_on(download_buffer_typed::<[f32; 4]>(
        &ctx,
        &next_output,
        16,
        "next bracket",
    ))
    .unwrap();
    assert_eq!(next_values[0][0], 20.0);
    assert_eq!(next_values[0][3], 1.0);
    // Generic evidence is a stricter OR diagnostic; the exact #80 combined rule is checked per row above.
    let evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "SHARED-HEIGHT-UV-184".into(),
        candidate: GpuCandidateEvidence {
            implementation_id: "shared_height_uv_preparation_only".into(),
            revision: revision(),
            shader_sha256: sha256_hex(SHARED_HEIGHT_SHADER.as_bytes()),
            input_sha256: sha256_hex(&serde_json::to_vec(&snapshots).unwrap()),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(GpuAdapterEvidence::from_context(&ctx)),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(PinnedOracleEvidence {
            implementation_id: "FLEXPART-11.1 direct #118 shared_rows".into(),
            revision: provenance["pinned_commit"].as_str().unwrap().into(),
            executable_sha256: provenance["artifacts_sha256"]["build/w-production-oracle"]
                .as_str()
                .unwrap()
                .into(),
            output_sha256: provenance["artifacts_sha256"]["output.txt"]
                .as_str()
                .unwrap()
                .into(),
        }),
        comparison: compare_finite_values(
            &expected_values,
            &actual_values,
            ComparisonPolicy::new(1e-6, 1e-5).unwrap(),
        )
        .unwrap(),
    };
    evidence.require_paired_pass().unwrap();
    let mut missing = evidence.clone();
    missing.execution.adapter = None;
    assert!(missing.require_paired_pass().is_err());
    let mut skipped = evidence.clone();
    skipped.execution.status = GpuExecutionStatus::Skipped;
    assert!(skipped.require_paired_pass().is_err());
    let directory = std::env::var("FLEXPART_GPU_SHARED_HEIGHT_EVIDENCE")
        .unwrap_or_else(|_| "target/ci-gate/shared-height-gpu".into());
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        format!("{directory}/inputs.json"),
        serde_json::to_vec(&snapshots).unwrap(),
    )
    .unwrap();
    let record = json!({"scope":"U/V preparation only; no production consumer migration","gpu_evidence":evidence,"comparison_formula":"abs(a-b) <= 1e-6 + 1e-5*max(abs(a),abs(b))","rows":rows,"actual_target_height_m_agl":gpu_target,"target":prepared[0].target().identity(),"sources":prepared.iter().map(|p|p.source_identity()).collect::<Vec<_>>(),"serialized_sources":snapshots.iter().map(|s|serde_json::to_string(s).unwrap()).collect::<Vec<_>>(),"oracle_provenance":provenance,"consumer_shader_sha256":sha256_hex(CONSUMER.as_bytes()),"one_encoder_one_submission":true,"adjacent_bracket_reuse_checked":true,"changed_source_unchanged_target_checked":true});
    let mut record = record;
    record["resource_metadata"] = json!(prepared
        .iter()
        .map(PreparedSharedHeightUv::metadata)
        .collect::<Vec<_>>());
    std::fs::write(
        format!("{directory}/comparison.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    println!(
        "SHARED-HEIGHT-UV-184 compared_values=64 adapter={} software={}",
        ctx.device_name(),
        ctx.is_software_adapter()
    );
}

#[test]
fn test_shared_height_negative_inputs_and_ownership() {
    let ctx = pollster::block_on(GpuContext::new()).unwrap();
    let other = pollster::block_on(GpuContext::new()).unwrap();
    let preparer = pollster::block_on(SharedHeightPreparer::new(&ctx)).unwrap();
    let s = snapshot(1);
    let geometry = reconstruct_vertical_geometry(&s).unwrap();
    let runtime = geometry.runtime_view().unwrap();
    let source = SharedHeightSource::upload(&ctx, &s, runtime).unwrap();
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    for id in [
        FieldId::WindU10m,
        FieldId::WindV10m,
        FieldId::SurfacePressure,
    ] {
        let mut missing = s.clone();
        missing.fields.retain(|f| f.id != id);
        assert!(SharedHeightSource::upload(&ctx, &missing, runtime).is_err());
    }
    for value in [f32::NAN, f32::INFINITY] {
        let mut invalid = s.clone();
        invalid
            .fields
            .iter_mut()
            .find(|f| f.id == FieldId::WindU)
            .unwrap()
            .values[0] = value;
        assert!(SharedHeightSource::upload(&ctx, &invalid, runtime).is_err());
    }
    let mut staggered = s.clone();
    let staggered_u = staggered
        .fields
        .iter_mut()
        .find(|f| f.id == FieldId::WindU)
        .unwrap();
    staggered_u.horizontal_staggering = HorizontalStaggering::XFace;
    staggered_u.shape[0] = 3;
    staggered_u.values = vec![1.0; 18];
    let staggered_geometry = reconstruct_vertical_geometry(&staggered).unwrap();
    assert!(SharedHeightSource::upload(
        &ctx,
        &staggered,
        staggered_geometry.runtime_view().unwrap()
    )
    .is_err());
    let mut changed = s.clone();
    changed
        .fields
        .iter_mut()
        .find(|f| f.id == FieldId::WindU)
        .unwrap()
        .values[0] += 1.0;
    assert!(SharedHeightSource::upload(&ctx, &changed, runtime).is_err());
    let changed_geometry = reconstruct_vertical_geometry(&changed).unwrap();
    let changed_source =
        SharedHeightSource::upload(&ctx, &changed, changed_geometry.runtime_view().unwrap())
            .unwrap();
    for mode in [
        SharedHeightInitialization::Restart,
        SharedHeightInitialization::Nested,
        SharedHeightInitialization::GlobalOrPolar,
    ] {
        assert!(preparer
            .initialize(&ctx, &source, mode, &mut encoder)
            .is_err());
    }
    assert!(preparer
        .initialize(
            &other,
            &source,
            SharedHeightInitialization::RegionalMother,
            &mut encoder
        )
        .is_err());
    let target = preparer
        .initialize(
            &ctx,
            &source,
            SharedHeightInitialization::RegionalMother,
            &mut encoder,
        )
        .unwrap();
    let p = preparer
        .prepare(&ctx, &source, &target, &mut encoder)
        .unwrap();
    let changed_p = preparer
        .prepare(&ctx, &changed_source, &target, &mut encoder)
        .unwrap();
    assert_eq!(p.target().identity(), changed_p.target().identity());
    assert!(p.require_reuse(&ctx, &changed_source, &target).is_err());
    assert!(p.require_reuse(&other, &source, &target).is_err());
    let other_target = preparer
        .initialize(
            &ctx,
            &source,
            SharedHeightInitialization::RegionalMother,
            &mut encoder,
        )
        .unwrap();
    assert!(p.require_reuse(&ctx, &source, &other_target).is_err());
    for pressure in [0.0, 100000.0, 99999.0] {
        let mut bad = s.clone();
        bad.fields
            .iter_mut()
            .find(|f| f.id == FieldId::SurfacePressure)
            .unwrap()
            .values[0] = pressure;
        if pressure == 0.0 {
            assert!(reconstruct_vertical_geometry(&bad).is_err());
            continue;
        }
        let g = reconstruct_vertical_geometry(&bad).unwrap();
        let src = SharedHeightSource::upload(&ctx, &bad, g.runtime_view().unwrap()).unwrap();
        assert!(preparer
            .initialize(
                &ctx,
                &src,
                SharedHeightInitialization::RegionalMother,
                &mut encoder
            )
            .is_err());
    }
    let mut bad_time = s.clone();
    bad_time
        .fields
        .iter_mut()
        .find(|f| f.id == FieldId::WindU10m)
        .unwrap()
        .time
        .valid_time_epoch_seconds += 1;
    assert!(SharedHeightSource::upload(&ctx, &bad_time, runtime).is_err());
    let mut fewer = s.clone();
    fewer.vertical_coordinate.level_values.remove(0);
    fewer
        .vertical_coordinate
        .interface_values
        .as_mut()
        .unwrap()
        .remove(0);
    fewer
        .vertical_coordinate
        .hybrid_a_interface_pa
        .as_mut()
        .unwrap()
        .remove(0);
    fewer
        .vertical_coordinate
        .hybrid_b_interface
        .as_mut()
        .unwrap()
        .remove(0);
    for field in &mut fewer.fields {
        if field.shape.len() == 3 {
            field.shape[2] = 2;
            field.values.drain(..4);
        }
    }
    let fewer_geometry = reconstruct_vertical_geometry(&fewer).unwrap();
    let fewer_source =
        SharedHeightSource::upload(&ctx, &fewer, fewer_geometry.runtime_view().unwrap()).unwrap();
    assert!(preparer
        .prepare(&ctx, &fewer_source, &target, &mut encoder)
        .is_err());
    let mut malformed = s.clone();
    malformed.vertical_coordinate.hybrid_a_interface_pa = Some(vec![0.0; 4]);
    malformed.vertical_coordinate.hybrid_b_interface = Some(vec![1.0; 4]);
    assert!(reconstruct_vertical_geometry(&malformed).is_err());
    let mut limits = ctx.device.limits();
    limits.max_storage_buffer_binding_size = 16;
    assert!(validate_shared_height_limits(&limits, 4, 4).is_err());
    assert!(validate_shared_height_limits(&ctx.device.limits(), u32::MAX, 4).is_err());
    let mut binding_limits = ctx.device.limits();
    binding_limits.max_storage_buffers_per_shader_stage = 5;
    assert!(validate_shared_height_limits(&binding_limits, 4, 4).is_err());
    // Unencoded initialization remains pending and cannot be accepted by the fixture consumer.
    let mut discarded = ctx.device.create_command_encoder(&Default::default());
    let pending = preparer
        .initialize(
            &ctx,
            &source,
            SharedHeightInitialization::RegionalMother,
            &mut discarded,
        )
        .unwrap();
    drop(discarded);
    let mut only_prepare = ctx.device.create_command_encoder(&Default::default());
    let failed = preparer
        .prepare(&ctx, &source, &pending, &mut only_prepare)
        .unwrap();
    let output = consume(&ctx, &failed, &mut only_prepare);
    ctx.queue.submit(Some(only_prepare.finish()));
    let values = pollster::block_on(download_buffer_typed::<[f32; 4]>(
        &ctx,
        &output,
        16,
        "failed consumer",
    ))
    .unwrap();
    assert!(values.iter().all(|v| v[3] == 0.0));
    // Finite native values can overflow intermediate f32 arithmetic: reject the whole source.
    let mut overflow = s.clone();
    overflow
        .fields
        .iter_mut()
        .find(|f| f.id == FieldId::WindU)
        .unwrap()
        .values
        .fill(f32::MAX);
    let overflow_geometry = reconstruct_vertical_geometry(&overflow).unwrap();
    let overflow_source =
        SharedHeightSource::upload(&ctx, &overflow, overflow_geometry.runtime_view().unwrap())
            .unwrap();
    let mut overflow_encoder = ctx.device.create_command_encoder(&Default::default());
    // The earlier target initialization was never submitted; initialize another for this check.
    let overflow_target = preparer
        .initialize(
            &ctx,
            &source,
            SharedHeightInitialization::RegionalMother,
            &mut overflow_encoder,
        )
        .unwrap();
    let overflow_p = preparer
        .prepare(
            &ctx,
            &overflow_source,
            &overflow_target,
            &mut overflow_encoder,
        )
        .unwrap();
    let output = consume(&ctx, &overflow_p, &mut overflow_encoder);
    ctx.queue.submit(Some(overflow_encoder.finish()));
    let values = pollster::block_on(download_buffer_typed::<[f32; 4]>(
        &ctx,
        &output,
        16,
        "overflow consumer",
    ))
    .unwrap();
    assert!(values.iter().all(|v| v[3] == 0.0));
}

#[test]
fn test_shared_height_interior_above_native_top_copies_top() {
    let ctx = pollster::block_on(GpuContext::new()).unwrap();
    let preparer = pollster::block_on(SharedHeightPreparer::new(&ctx)).unwrap();
    let initial = snapshot(1);
    let initial_geometry = reconstruct_vertical_geometry(&initial).unwrap();
    let source =
        SharedHeightSource::upload(&ctx, &initial, initial_geometry.runtime_view().unwrap())
            .unwrap();
    let mut cold = initial.clone();
    for field in &mut cold.fields {
        if matches!(
            field.id,
            FieldId::Temperature | FieldId::Temperature2m | FieldId::Dewpoint2m
        ) {
            field.values.fill(100.0);
        }
    }
    let cold_geometry = reconstruct_vertical_geometry(&cold).unwrap();
    let cold_source =
        SharedHeightSource::upload(&ctx, &cold, cold_geometry.runtime_view().unwrap()).unwrap();
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    let target = preparer
        .initialize(
            &ctx,
            &source,
            SharedHeightInitialization::RegionalMother,
            &mut encoder,
        )
        .unwrap();
    let p = preparer
        .prepare(&ctx, &cold_source, &target, &mut encoder)
        .unwrap();
    let output = consume(&ctx, &p, &mut encoder);
    ctx.queue.submit(Some(encoder.finish()));
    let values = pollster::block_on(download_buffer_typed::<[f32; 4]>(
        &ctx,
        &output,
        16,
        "interior above native top",
    ))
    .unwrap();
    for x in 0..2 {
        for y in 0..2 {
            assert!(
                cold_geometry
                    .runtime_view()
                    .unwrap()
                    .level(x, y, 0)
                    .unwrap()
                    .height_agl_m
                    < target.identity().height_m_agl[2]
            );
            let index = 8 + y * 2 + x;
            assert_eq!(values[index][0], if x == 0 { 42.0 } else { -29.0 });
            assert_eq!(values[index][1], if x == 0 { -13.0 } else { 2.0 });
            assert_eq!(values[index][3], 1.0);
        }
    }
}
