//! #188 direct pinned four-control proof: preparation only, no query adoption.
use flexpart_gpu::gpu::evidence::GpuEvidenceSchema;
use flexpart_gpu::gpu::{shared_height::*, shared_height_w::*, vertical::sha256_hex, *};
use flexpart_gpu::meteorology::{vertical::*, *};
use serde_json::{json, Value};
use std::sync::Arc;

fn oracle() -> Value {
    let path = std::env::var("FLEXPART_GPU_INTERIOR_W_ORACLE")
        .unwrap_or_else(|_| "fixtures/interpolation/interior-w-v1/report.json".into());
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn snapshot(memory: usize) -> Snapshot {
    let report = oracle();
    let mut base: Value = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-column-v1.json"
    ))
    .unwrap();
    base["horizontal_grid"]["nx"] = json!(3);
    base["horizontal_grid"]["ny"] = json!(3);
    let coord = &mut base["vertical_coordinate"];
    coord["hybrid_a_interface_pa"] = json!([10000.0, 10000.0, 5000.0, 0.0]);
    coord["hybrid_b_interface"] = json!([0.1, 0.3, 0.7, 1.0]);
    coord["interface_values"] = json!([20000.0, 40000.0, 75000.0, 100000.0]);
    coord["level_values"] = json!([30000.0, 57500.0, 87500.0]);
    base["horizontal_grid"]["xlon0_deg"] = json!(-2.0);
    base["horizontal_grid"]["ylat0_deg"] = json!(48.0);
    base["horizontal_grid"]["dx_deg"] = json!(0.25);
    base["horizontal_grid"]["dy_deg"] = json!(0.4);
    let columns: Vec<_> = report["scientific"]["decoded_inputs"]["columns"]
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
            json!([3, 3, 3])
        } else {
            json!([3, 3])
        };
        let mut values = Vec::new();
        for z in 0..if lane.is_some() { 3 } else { 1 } {
            for y in 0..3 {
                for x in 0..3 {
                    let c = columns[y * 3 + x];
                    let value = match lane {
                        Some(l) => c["center_t_q_u_v_interface_omega"][3 - z][l].clone(),
                        None => match id {
                            "surface_pressure" => c["surface_pa_t_k_td_k"][0].clone(),
                            "temperature2m" => c["surface_pa_t_k_td_k"][1].clone(),
                            "dewpoint2m" => c["surface_pa_t_k_td_k"][2].clone(),
                            "orography" => report["scientific"]["decoded_inputs"]
                                ["terrain_m_asl_y_x"][y][x]
                                .clone(),
                            _ => c["center_t_q_u_v_interface_omega"][0]
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

fn native(memory: usize) -> NativeVerticalMotion {
    let report = oracle();
    let columns: Vec<_> = report["scientific"]["decoded_inputs"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["memory"] == memory)
        .collect();
    let mut values = Vec::new();
    for z in (0..4).rev() {
        for c in &columns {
            values.push(c["center_t_q_u_v_interface_omega"][z][4].as_f64().unwrap() as f32);
        }
    }
    NativeVerticalMotion {
        kind: NativeVerticalMotionKind::PressureVelocityOmega,
        unit: NativeVerticalMotionUnit::PascalPerSecond,
        sign: NativeVerticalMotionSign::PositivePressureIncreasing,
        vertical_staggering: VerticalStaggering::LevelInterface,
        values,
        provenance: NativeVerticalMotionProvenance {
            source_id: format!("interior-w-186-time-{memory}"),
        },
    }
}
const CONSUMER: &str = r"
@group(0) @binding(0) var<storage,read> w:array<f32>;
@group(0) @binding(1) var<storage,read> u:array<f32>;
@group(0) @binding(2) var<storage,read> v:array<f32>;
@group(0) @binding(3) var<storage,read> target_status:array<u32>;
@group(0) @binding(4) var<storage,read> uv_status:array<u32>;
@group(0) @binding(5) var<storage,read> w_status:array<u32>;
@group(0) @binding(6) var<storage,read_write> result:array<vec4<f32>>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id:vec3<u32>){
 let i=id.x;if(i>=arrayLength(&w)){return;}
 result[i]=vec4<f32>(0.0);
 if(target_status[0]!=2u){return;}
 for(var c=0u;c<arrayLength(&uv_status);c++){
   if(uv_status[c]!=2u||w_status[c]!=2u){return;}
 }
 result[i]=vec4<f32>(w[i],u[i],v[i],1.0);
}";
fn consume(
    ctx: &GpuContext,
    source: &SharedHeightWSource,
    w: &wgpu::Buffer,
    status: &wgpu::Buffer,
    encoder: &mut wgpu::CommandEncoder,
) -> wgpu::Buffer {
    let output = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("status-aware W fixture consumer"),
        size: 36 * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let shader = ctx.load_shader("W fixture consumer", CONSUMER);
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
    let uv = source.uv();
    let buffers = [
        w,
        uv.u(),
        uv.v(),
        uv.target().status(),
        uv.status(),
        status,
        &output,
    ];
    let entries: Vec<_> = buffers
        .iter()
        .zip(0u32..)
        .map(|(b, binding)| wgpu::BindGroupEntry {
            binding,
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
    pass.dispatch_workgroups(1, 1, 1);
    drop(pass);
    output
}
fn read(ctx: &GpuContext, b: &wgpu::Buffer) -> Vec<[f32; 4]> {
    pollster::block_on(download_buffer_typed(
        ctx,
        b,
        36,
        "final W validation readback",
    ))
    .unwrap()
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
fn close(a: f64, b: f64) {
    assert!(
        a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-6 + 1e-5 * a.abs().max(b.abs()),
        "actual={a} expected={b} diff={}",
        (a - b).abs()
    );
}
fn reference(report: &Value, variant: &str, m: usize, x: usize, y: usize, k: usize) -> Vec<f64> {
    report["scientific"]["shared_rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == json!([variant, m, x, y, k]))
        .unwrap()["height_uv_w"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| f64::from(v.as_f64().unwrap() as f32))
        .collect()
}
#[test]
fn test_shared_w_every_fortran_control_row_and_retained_reuse() {
    let ctx =
        pollster::block_on(GpuContext::new()).expect("required W device execution cannot skip");
    let uv_preparer = pollster::block_on(SharedHeightPreparer::new(&ctx)).unwrap();
    let preparer = pollster::block_on(SharedHeightWPreparer::new(&ctx)).unwrap();
    for filter in [
        wgpu::ErrorFilter::OutOfMemory,
        wgpu::ErrorFilter::Internal,
        wgpu::ErrorFilter::Validation,
    ] {
        ctx.device.push_error_scope(filter);
    }
    let snapshots = [snapshot(1), snapshot(2)];
    let motions = [native(1), native(2)];
    let geometry: Vec<_> = snapshots
        .iter()
        .zip(&motions)
        .map(|(s, n)| reconstruct_vertical_geometry_with_motion(s, n).unwrap())
        .collect();
    let sources: Vec<_> = snapshots
        .iter()
        .zip(&geometry)
        .map(|(s, g)| SharedHeightSource::upload(&ctx, s, g.runtime_view().unwrap()).unwrap())
        .collect();
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    let target = uv_preparer
        .initialize(
            &ctx,
            &sources[0],
            SharedHeightInitialization::RegionalMother,
            &mut encoder,
        )
        .unwrap();
    let uv: Vec<_> = sources
        .iter()
        .map(|s| Arc::new(uv_preparer.prepare(&ctx, s, &target, &mut encoder).unwrap()))
        .collect();
    let w_sources: Vec<_> = snapshots
        .iter()
        .zip(&geometry)
        .zip(&uv)
        .map(|((s, g), uv)| {
            SharedHeightWSource::upload(&ctx, s, g.runtime_view().unwrap(), Arc::clone(uv)).unwrap()
        })
        .collect();
    let prepared: Vec<_> = w_sources
        .iter()
        .map(|s| preparer.prepare(&ctx, s, &mut encoder).unwrap())
        .collect();
    let outputs: Vec<_> = prepared
        .iter()
        .map(|p| consume(&ctx, p.source(), p.w(), p.status(), &mut encoder))
        .collect();
    let controls = [
        ("x", WValidationControl::XOnly),
        ("y", WValidationControl::YOnly),
        ("zero", WValidationControl::Zero),
    ];
    let mut diagnostics = Vec::new();
    let mut diagnostic_outputs = Vec::new();
    for (name, control) in controls {
        for (m, s) in w_sources.iter().enumerate() {
            let p = preparer
                .prepare_validation_control(&ctx, s, control, &mut encoder)
                .unwrap();
            diagnostic_outputs.push((
                name,
                m + 1,
                consume(&ctx, p.source(), p.w(), p.status(), &mut encoder),
            ));
            diagnostics.push(p);
        }
    }
    prepared[1].require_reuse(&ctx, &w_sources[1]).unwrap();
    assert!(prepared[0].require_reuse(&ctx, &w_sources[1]).is_err());
    let reuploaded = SharedHeightWSource::upload(
        &ctx,
        &snapshots[1],
        geometry[1].runtime_view().unwrap(),
        Arc::clone(&uv[1]),
    )
    .unwrap();
    prepared[1].require_reuse(&ctx, &reuploaded).unwrap();
    drop(reuploaded);
    drop(w_sources);
    drop(uv);
    drop(sources);
    drop(target);
    ctx.queue.submit(Some(encoder.finish()));
    let report = oracle();
    let mut rows = Vec::new();
    let mut actuals = Vec::new();
    let mut expected = Vec::new();
    let mut all = std::collections::BTreeMap::new();
    for (m, b) in outputs.iter().enumerate() {
        all.insert(("full", m + 1), read(&ctx, b));
    }
    for (variant, m, b) in &diagnostic_outputs {
        all.insert((*variant, *m), read(&ctx, b));
    }
    for ((variant, m), values) in &all {
        for k in 0..4 {
            for y in 0..3 {
                for x in 0..3 {
                    let a = values[k * 9 + y * 3 + x];
                    assert_eq!(a[3], 1.0, "all statuses must accept");
                    let r = reference(&report, variant, *m, x, y, k + 1);
                    if *variant == "full" || *variant == "zero" {
                        close(f64::from(a[0]), r[3]);
                    }
                    close(f64::from(a[1]), r[1]);
                    close(f64::from(a[2]), r[2]);
                    if k == 0 || k == 3 {
                        let runtime = geometry[*m - 1].runtime_view().unwrap();
                        let canonical = 3 - k;
                        assert_eq!(
                            a[0],
                            runtime.vertical_velocity().unwrap().values_ms()
                                [canonical * 9 + y * 3 + x],
                            "exact normalized native endpoint copy"
                        );
                    }
                    let diff = (f64::from(a[0]) - r[3]).abs();
                    rows.push(json!({"key":[variant,m,x,y,k+1],"actual_m_s":a[0],"oracle_m_s":r[3],"difference_m_s":diff,"prepared_uv_m_s":[a[1],a[2]],"consumer_accepted":a[3]}));
                    if *variant == "full" {
                        actuals.push(f64::from(a[0]));
                        expected.push(r[3]);
                    }
                }
            }
        }
    }
    let mut contributions = Vec::new();
    for m in 1..=2 {
        for k in 1..3 {
            let i = k * 9 + 4;
            let value = |variant| f64::from(all[&(variant, m)][i][0]);
            let oracle_value = |variant| reference(&report, variant, m, 1, 1, k + 1)[3];
            let zero = value("zero");
            let rx = oracle_value("x") - oracle_value("zero");
            let ry = oracle_value("y") - oracle_value("zero");
            let x = value("x") - zero;
            let y = value("y") - zero;
            let full = value("full") - zero;
            close(x, rx);
            close(y, ry);
            close(full, oracle_value("full") - oracle_value("zero"));
            close(full, x + y);
            assert!(
                x.abs() > 1e-6 + 1e-5 * x.abs() && y.abs() > 1e-6 + 1e-5 * y.abs() && x * y > 0.0
            );
            // Both omitted and doubled correction fail the frozen full result.
            assert!(
                (zero - oracle_value("full")).abs()
                    > 1e-6 + 1e-5 * zero.abs().max(oracle_value("full").abs())
            );
            assert!(
                (zero + 2.0 * full - oracle_value("full")).abs()
                    > 1e-6 + 1e-5 * (zero + 2.0 * full).abs().max(oracle_value("full").abs())
            );
            contributions.push(json!({"memory":m,"level":k+1,"x_m_s":x,"y_m_s":y,"combined_m_s":full,"oracle_x_m_s":rx,"oracle_y_m_s":ry,"additive_residual_m_s":full-x-y}));
        }
    }
    for m in 1..=2 {
        for k in 0..4 {
            for y in 0..3 {
                for x in 0..3 {
                    let i = k * 9 + y * 3 + x;
                    if k == 0 || k == 3 || x == 0 || x == 2 || y == 0 || y == 2 {
                        for name in ["x", "y", "zero"] {
                            assert_eq!(all[&("full", m)][i][0], all[&(name, m)][i][0]);
                        }
                    }
                }
            }
        }
    }
    let target_values = pollster::block_on(download_buffer_typed::<f32>(
        &ctx,
        prepared[0].source().uv().target().heights(),
        4,
        "final target audit",
    ))
    .unwrap();
    assert_eq!(
        sha256_hex(bytemuck::cast_slice(&target_values)),
        prepared[0].source().uv().target().identity().height_sha256
    );
    let mut final_w_status = Vec::new();
    let mut final_uv_status = Vec::new();
    for p in &prepared {
        let s = pollster::block_on(download_buffer_typed::<u32>(
            &ctx,
            p.status(),
            9,
            "final W status",
        ))
        .unwrap();
        assert_eq!(s, vec![2; 9]);
        final_w_status.push(s);
        final_uv_status.push(
            pollster::block_on(download_buffer_typed::<u32>(
                &ctx,
                p.source().uv().status(),
                9,
                "final U/V status",
            ))
            .unwrap(),
        );
    }
    for _ in 0..3 {
        assert!(pollster::block_on(ctx.device.pop_error_scope()).is_none());
    }
    let provenance = &report["provenance"];
    let evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "INTERIOR-W-PREP-188".into(),
        candidate: GpuCandidateEvidence {
            implementation_id: "shared_height_w_preparation_only".into(),
            revision: revision(),
            shader_sha256: sha256_hex(
                format!("{SHARED_HEIGHT_SHADER}\n{W_REMAP_SHADER}\n{W_SLOPE_SHADER}").as_bytes(),
            ),
            input_sha256: sha256_hex(
                &serde_json::to_vec(&json!({"snapshots":snapshots,"native_motion":motions}))
                    .unwrap(),
            ),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(GpuAdapterEvidence::from_context(&ctx)),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(PinnedOracleEvidence {
            implementation_id: "FLEXPART-11.1 genuine #186 four-control SHARED".into(),
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
            &expected,
            &actuals,
            ComparisonPolicy::new(1e-6, 1e-5).unwrap(),
        )
        .unwrap(),
    };
    evidence.require_paired_pass().unwrap();
    let mut missing = evidence.clone();
    missing.execution.adapter = None;
    assert!(missing.require_paired_pass().is_err());
    let directory = std::env::var("FLEXPART_GPU_INTERIOR_W_EVIDENCE")
        .unwrap_or_else(|_| "target/ci-gate/interior-w-gpu".into());
    std::fs::create_dir_all(&directory).unwrap();
    let inputs = json!({"snapshots":snapshots,"native_motion":motions});
    std::fs::write(
        format!("{directory}/inputs.json"),
        serde_json::to_vec(&inputs).unwrap(),
    )
    .unwrap();
    for diagnostic in report["scientific"]["normalization_diagnostics"]
        .as_array()
        .unwrap()
    {
        let m = diagnostic["memory"].as_u64().unwrap() as usize;
        let k = diagnostic["level"].as_u64().unwrap() as usize;
        let actual = f64::from(all[&("zero", m)][(k - 1) * 9 + 4][0]);
        close(actual, diagnostic["direct_zero_m_s"].as_f64().unwrap());
        for key in ["omitted_diagnostic", "double_normalization_diagnostic_m_s"] {
            let wrong = diagnostic[key].as_f64().unwrap();
            assert!((actual - wrong).abs() > 1e-6 + 1e-5 * actual.abs().max(wrong.abs()));
        }
    }
    let mut native_lanes = Vec::new();
    for g in &geometry {
        let runtime = g.runtime_view().unwrap();
        let mut lanes = Vec::new();
        for z in (0..4).rev() {
            for y in 0..3 {
                for x in 0..3 {
                    let i = z * 9 + y * 3 + x;
                    lanes.push([
                        runtime.interface(x, y, z).unwrap().height_agl_m,
                        runtime.vertical_velocity().unwrap().values_ms()[i],
                    ]);
                }
            }
        }
        native_lanes.push(lanes);
    }
    let candidate_executable = std::env::current_exe().unwrap();
    let candidate_executable_sha256 = sha256_hex(&std::fs::read(&candidate_executable).unwrap());
    let record = json!({
        "candidate_executable_sha256":candidate_executable_sha256,
        "source_sha256":{
            "src/gpu/shared_height_w.rs":sha256_hex(include_bytes!("../src/gpu/shared_height_w.rs")),
            "src/gpu/shared_height.rs":sha256_hex(include_bytes!("../src/gpu/shared_height.rs")),
            "tests/shared_height_w_gpu.rs":sha256_hex(include_bytes!("shared_height_w_gpu.rs"))
        },"scope":"W preparation only; query and production consumer adoption excluded","gpu_evidence":evidence,
        "rows":rows,"contributions":contributions,"oracle_provenance":provenance,
        "consumer_shader_sha256":sha256_hex(CONSUMER.as_bytes()),"submission_count":1,"intermediate_d2h_count":0,
        "final_w_status":final_w_status,"final_uv_status":final_uv_status,
        "target":prepared[0].source().uv().target().identity(),"actual_target_height_m_agl":target_values,
        "w_metadata":prepared.iter().map(PreparedSharedHeightW::metadata).collect::<Vec<_>>(),
        "w_sources":prepared.iter().map(|p|p.source().identity()).collect::<Vec<_>>(),
        "uv_metadata":prepared.iter().map(|p|p.source().uv().metadata()).collect::<Vec<_>>(),
        "serialized_uv_metadata":prepared.iter().map(|p|serde_json::to_string(&p.source().uv().metadata()).unwrap()).collect::<Vec<_>>(),
        "serialized_snapshots":snapshots.iter().map(|s|serde_json::to_string(s).unwrap()).collect::<Vec<_>>(),
        "serialized_native_motion":motions.iter().map(|n|serde_json::to_string(n).unwrap()).collect::<Vec<_>>(),
        "serialized_grids":snapshots.iter().map(|s|serde_json::to_string(&s.horizontal_grid).unwrap()).collect::<Vec<_>>(),
        "native_interface_lanes":native_lanes,"adjacent_reuse_checked":true,"retained_dependencies_executed":true});
    std::fs::write(
        format!("{directory}/comparison.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    println!(
        "INTERIOR-W-PREP-188 values={} max_error={:?} adapter={} software={}",
        rows.len(),
        evidence.comparison.max_absolute_error,
        ctx.device_name(),
        ctx.is_software_adapter()
    );
}

#[test]
fn test_shared_w_fail_closed_inputs_context_lifetimes_and_status() {
    let ctx = pollster::block_on(GpuContext::new()).unwrap();
    let other = pollster::block_on(GpuContext::new()).unwrap();
    let up = pollster::block_on(SharedHeightPreparer::new(&ctx)).unwrap();
    let wp = pollster::block_on(SharedHeightWPreparer::new(&ctx)).unwrap();
    let s = snapshot(1);
    let n = native(1);
    let g = reconstruct_vertical_geometry_with_motion(&s, &n).unwrap();
    let r = g.runtime_view().unwrap();
    let src = SharedHeightSource::upload(&ctx, &s, r).unwrap();
    let mut e = ctx.device.create_command_encoder(&Default::default());
    let target = up
        .initialize(
            &ctx,
            &src,
            SharedHeightInitialization::RegionalMother,
            &mut e,
        )
        .unwrap();
    let uv = Arc::new(up.prepare(&ctx, &src, &target, &mut e).unwrap());
    let ws = SharedHeightWSource::upload(&ctx, &s, r, Arc::clone(&uv)).unwrap();
    assert!(SharedHeightWSource::upload(&other, &s, r, Arc::clone(&uv)).is_err());
    assert!(wp.prepare(&other, &ws, &mut e).is_err());
    let p = wp.prepare(&ctx, &ws, &mut e).unwrap();
    assert!(p.require_reuse(&other, &ws).is_err());
    // Wrong time/source/geometry are distinct from normalized-motion lineage changes.
    for field_id in [
        FieldId::WindU,
        FieldId::Temperature,
        FieldId::SurfacePressure,
    ] {
        let mut changed = s.clone();
        changed
            .fields
            .iter_mut()
            .find(|f| f.id == field_id)
            .unwrap()
            .values[0] += 1.0;
        let cg = reconstruct_vertical_geometry_with_motion(&changed, &n).unwrap();
        assert!(SharedHeightWSource::upload(
            &ctx,
            &changed,
            cg.runtime_view().unwrap(),
            Arc::clone(&uv)
        )
        .is_err());
        assert!(SharedHeightWSource::upload(&ctx, &changed, r, Arc::clone(&uv)).is_err());
    }
    let mut timed = s.clone();
    for f in &mut timed.fields {
        if f.time.kind != TemporalKind::Static {
            f.time.valid_time_epoch_seconds += 1;
        }
    }
    let tg = reconstruct_vertical_geometry_with_motion(&timed, &n).unwrap();
    assert!(
        SharedHeightWSource::upload(&ctx, &timed, tg.runtime_view().unwrap(), Arc::clone(&uv))
            .is_err()
    );
    let mut lineage = n.clone();
    lineage.provenance.source_id.push_str("-changed");
    let lg = reconstruct_vertical_geometry_with_motion(&s, &lineage).unwrap();
    let ls =
        SharedHeightWSource::upload(&ctx, &s, lg.runtime_view().unwrap(), Arc::clone(&uv)).unwrap();
    assert!(p.require_reuse(&ctx, &ls).is_err());
    // A different U/V preparation instance/target cannot reuse this prepared W.
    let target2 = up
        .initialize(
            &ctx,
            &src,
            SharedHeightInitialization::RegionalMother,
            &mut e,
        )
        .unwrap();
    let uv2 = Arc::new(up.prepare(&ctx, &src, &target2, &mut e).unwrap());
    let ws2 = SharedHeightWSource::upload(&ctx, &s, r, uv2).unwrap();
    assert!(p.require_reuse(&ctx, &ws2).is_err());
    let without = reconstruct_vertical_geometry(&s).unwrap();
    assert!(SharedHeightWSource::upload(
        &ctx,
        &s,
        without.runtime_view().unwrap(),
        Arc::clone(&uv)
    )
    .is_err());
    // Opaque normalized types exclude raw omega and completed W from the upload signature.
    // Geometric identity, including re-fed corrected values, has no accepted #80 lineage.
    for staggering in [
        VerticalStaggering::LevelCenter,
        VerticalStaggering::LevelInterface,
    ] {
        let mut raw = n.clone();
        raw.kind = NativeVerticalMotionKind::GeometricVelocity;
        raw.unit = NativeVerticalMotionUnit::MeterPerSecond;
        raw.sign = NativeVerticalMotionSign::PositiveUpward;
        raw.vertical_staggering = staggering;
        raw.values = vec![
            0.1;
            if staggering == VerticalStaggering::LevelCenter {
                27
            } else {
                36
            }
        ];
        let rg = reconstruct_vertical_geometry_with_motion(&s, &raw).unwrap();
        assert!(
            SharedHeightWSource::upload(&ctx, &s, rg.runtime_view().unwrap(), Arc::clone(&uv))
                .is_err()
        );
    }
    for value in [f32::NAN, f32::INFINITY] {
        let mut bad = n.clone();
        bad.values[0] = value;
        assert!(reconstruct_vertical_geometry_with_motion(&s, &bad).is_err());
    }
    let mut eta = n.clone();
    eta.kind = NativeVerticalMotionKind::EtaCoordinateVelocity;
    eta.unit = NativeVerticalMotionUnit::PerSecond;
    eta.sign = NativeVerticalMotionSign::PositiveEtaIncreasing;
    assert!(reconstruct_vertical_geometry_with_motion(&s, &eta).is_err());
    for mode in [
        SharedHeightInitialization::Restart,
        SharedHeightInitialization::Nested,
        SharedHeightInitialization::GlobalOrPolar,
    ] {
        assert!(up.initialize(&ctx, &src, mode, &mut e).is_err());
    }
    for value in [0.0, -1.0, f64::INFINITY, f64::NAN] {
        let mut bad = s.clone();
        bad.horizontal_grid.dx_deg = value;
        assert!(SharedHeightSource::upload(&ctx, &bad, r).is_err());
    }
    for grid_mode in 0..2 {
        let mut bad = s.clone();
        if grid_mode == 0 {
            bad.horizontal_grid.ylat0_deg = 89.9;
        } else {
            bad.horizontal_grid.dx_deg = 120.0;
        }
        let bg = reconstruct_vertical_geometry_with_motion(&bad, &n);
        if let Ok(bg) = bg {
            assert!(SharedHeightSource::upload(&ctx, &bad, bg.runtime_view().unwrap()).is_err());
        }
    }
    let mut malformed = s.clone();
    malformed.vertical_coordinate.hybrid_a_interface_pa = Some(vec![0.0; 4]);
    malformed.vertical_coordinate.hybrid_b_interface = Some(vec![1.0; 4]);
    assert!(reconstruct_vertical_geometry_with_motion(&malformed, &n).is_err());
    let mut limits = ctx.device.limits();
    limits.max_storage_buffers_per_shader_stage = 7;
    assert!(validate_shared_w_limits(&limits, 3, 3, 4).is_err());
    limits = ctx.device.limits();
    limits.max_storage_buffer_binding_size = 32;
    assert!(validate_shared_w_limits(&limits, 3, 3, 4).is_err());
    assert!(validate_shared_w_limits(&ctx.device.limits(), u32::MAX, 3, 4).is_err());
    // Discarding the encoder leaves all resources pending; the fixture must refuse them.
    drop(e);
    let mut consumer_encoder = ctx.device.create_command_encoder(&Default::default());
    let pending = consume(&ctx, p.source(), p.w(), p.status(), &mut consumer_encoder);
    ctx.queue.submit(Some(consumer_encoder.finish()));
    assert!(read(&ctx, &pending).iter().all(|v| v[3] == 0.0));
    // A different, colder source is valid #30/#184 but unbracketed on the run target.
    let mut cold = s.clone();
    for f in &mut cold.fields {
        if matches!(
            f.id,
            FieldId::Temperature | FieldId::Temperature2m | FieldId::Dewpoint2m
        ) {
            f.values.fill(100.0);
        }
    }
    let cg = reconstruct_vertical_geometry_with_motion(&cold, &n).unwrap();
    let cs = SharedHeightSource::upload(&ctx, &cold, cg.runtime_view().unwrap()).unwrap();
    let mut e = ctx.device.create_command_encoder(&Default::default());
    let target = up
        .initialize(
            &ctx,
            &src,
            SharedHeightInitialization::RegionalMother,
            &mut e,
        )
        .unwrap();
    let cuv = Arc::new(up.prepare(&ctx, &cs, &target, &mut e).unwrap());
    let cw = SharedHeightWSource::upload(&ctx, &cold, cg.runtime_view().unwrap(), cuv).unwrap();
    let failed = wp.prepare(&ctx, &cw, &mut e).unwrap();
    let output = consume(&ctx, failed.source(), failed.w(), failed.status(), &mut e);
    ctx.queue.submit(Some(e.finish()));
    assert!(read(&ctx, &output).iter().all(|v| v[3] == 0.0));
    assert!(pollster::block_on(download_buffer_typed::<u32>(
        &ctx,
        failed.status(),
        9,
        "unbracketed status"
    ))
    .unwrap()
    .iter()
    .any(|s| *s != 2));
    let mut overflow = n.clone();
    overflow.values.fill(-f32::MAX);
    let og = reconstruct_vertical_geometry_with_motion(&s, &overflow).unwrap();
    let mut e = ctx.device.create_command_encoder(&Default::default());
    let target = up
        .initialize(
            &ctx,
            &src,
            SharedHeightInitialization::RegionalMother,
            &mut e,
        )
        .unwrap();
    let ouv = Arc::new(up.prepare(&ctx, &src, &target, &mut e).unwrap());
    let os = SharedHeightWSource::upload(&ctx, &s, og.runtime_view().unwrap(), ouv).unwrap();
    let op = wp.prepare(&ctx, &os, &mut e).unwrap();
    let output = consume(&ctx, op.source(), op.w(), op.status(), &mut e);
    ctx.queue.submit(Some(e.finish()));
    assert!(read(&ctx, &output).iter().all(|v| v[3] == 0.0));
}
