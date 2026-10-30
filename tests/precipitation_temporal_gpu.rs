//! #119 genuine accumulated GRIB -> #90 -> #207 -> #87 -> temporal WGSL proof.
//! Readback happens only after the single composition submission.

use std::path::PathBuf;

#[path = "support/grib202.rs"]
mod grib202;

use flexpart_gpu::gpu::{
    accumulation::accumulated_shader_sha256,
    compare_finite_values, horizontal_shader_sha256,
    meteorology::CanonicalGpuField,
    precipitation_members::{
        darain_shader_sha256, member_matches_oracle, prepare_precipitation_members,
        PrecipitationMemberMetadata, PrecipitationMemberPreparation,
    },
    precipitation_temporal::{
        precipitation_temporal_shader_sha256, prepare_precipitation_time_samples,
        select_precipitation_memory_pair, PrecipitationTemporalKernels, PrecipitationTimeQuery,
    },
    ComparisonPolicy, GpuAdapterEvidence, GpuCalculationEvidence, GpuCalculationPath,
    GpuCandidateEvidence, GpuContext, GpuEvidenceSchema, GpuExecutionEvidence, GpuExecutionStatus,
    PinnedOracleEvidence,
};
use flexpart_gpu::meteorology::{FieldId, Snapshot};
use serde_json::json;

const ORIGINAL: &str = include_str!("../fixtures/interpolation/precipitation-temporal-v1.json");

fn git(args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("git");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("UTF8")
        .trim()
        .to_owned()
}

fn upload<'ctx>(
    ctx: &'ctx GpuContext,
    id: FieldId,
    snapshots: &[Snapshot],
) -> CanonicalGpuField<'ctx> {
    CanonicalGpuField::upload(
        ctx,
        id,
        &snapshots.iter().collect::<Vec<_>>(),
        &vec![None; snapshots.len()],
    )
    .expect("genuine canonical upload")
}

fn report_path(class: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "target/ci-gate/precipitation-temporal/{class}.json"
    ))
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_precipitation_temporal_genuine_same_encoder_original_comparison() {
    let requested_class = if std::env::var("FLEXPART_GPU_SOFTWARE").is_ok_and(|v| v == "1") {
        "software_wgsl"
    } else {
        "hardware_gpu"
    };
    let pending = report_path(requested_class);
    std::fs::create_dir_all(pending.parent().expect("parent")).expect("evidence directory");
    std::fs::write(&pending, "{\"verdict\":\"not_executed\"}\n").expect("invalidate before setup");
    assert_eq!(
        grib202::sha256_hex(ORIGINAL.as_bytes()),
        "8654e238cfe94293bb69e681eb4a4a29000bf83d46fe76a34bcaba62d19c8488",
        "original fixture drifted"
    );
    let original = grib202::exact_json(ORIGINAL);
    let snapshots = grib202::snapshots(&grib202::observations());
    let ctx = pollster::block_on(GpuContext::new()).expect("required #119 GPU, no skip");
    let adapter = GpuAdapterEvidence::from_context(&ctx);
    let class = serde_json::to_value(adapter.adapter_class).expect("class");
    let path = report_path(class.as_str().expect("class"));
    std::fs::create_dir_all(path.parent().expect("parent")).expect("evidence directory");
    // Invalidate stale success before any source preparation can fail.
    std::fs::write(&path, "{\"verdict\":\"not_executed\"}\n").expect("invalidate");
    let lsp = upload(&ctx, FieldId::LargeScalePrecipitation, &snapshots);
    let cp = upload(&ctx, FieldId::ConvectivePrecipitation, &snapshots);
    let mut producer = prepare_precipitation_members(
        &ctx,
        &lsp,
        &cp,
        PrecipitationMemberPreparation::flex_extract_rrint0(),
    )
    .expect("#207");
    let mut queries = Vec::new();
    let mut expected = Vec::new();
    let mut case_ids = Vec::new();
    for case in original["cases"].as_array().expect("cases") {
        if case["id"] == "arithmetic-control" {
            continue;
        }
        for row in case["rows"].as_array().expect("original rows") {
            queries.push(PrecipitationTimeQuery {
                xt: row["query"][0].as_f64().expect("x"),
                yt: row["query"][1].as_f64().expect("y"),
                itime_epoch_seconds: grib202::ORACLE_EPOCH_UNIX_S
                    + row["query"][2].as_i64().expect("itime"),
            });
            expected.push([
                row["lsp"].as_f64().expect("LSP"),
                row["cp"].as_f64().expect("CP"),
            ]);
            case_ids.push(case["id"].clone());
        }
    }
    assert_eq!(queries.len(), 588);
    let kernels = PrecipitationTemporalKernels::new(&ctx).expect("#87/#119 kernels");
    let mut plan = prepare_precipitation_time_samples(&ctx, &mut producer, &queries, 1)
        .expect("valid temporal batch");
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("genuine_119_single_encoder"),
        });
    ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let handoff = plan
        .encode(&ctx, &kernels, &mut encoder)
        .expect("resident composition");
    let metadata = handoff.metadata.clone();
    assert_eq!(
        metadata.members.large_scale.quantization_admissions.len(),
        1
    );
    assert_eq!(metadata.members.convective.quantization_admissions.len(), 2);
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    assert!(pollster::block_on(ctx.device.pop_error_scope()).is_none());
    let values = pollster::block_on(plan.download(&ctx)).expect("fatal-checked final rates");
    drop(plan);
    // All producer evidence and member bytes are collected after the final output.
    let members = pollster::block_on(producer.download(&ctx)).expect("validation members");
    let mut member_values = serde_json::Map::new();
    let mut member_bits = serde_json::Map::new();
    let oracle = grib202::oracle();
    for (parameter, actual) in [
        ("lsp", members.large_scale_mm_per_h().expect("lsp")),
        ("cp", members.convective_mm_per_h().expect("cp")),
    ] {
        let planes: Vec<_> = actual
            .chunks(9)
            .map(|v| v.iter().map(|v| f64::from(*v)).collect::<Vec<_>>())
            .collect();
        for (plane, reference) in planes.iter().zip(
            oracle["genuine"]["darain_members"][parameter]
                .as_array()
                .expect("members"),
        ) {
            for (value, expected) in plane
                .iter()
                .zip(reference["result_mm_per_h"].as_array().expect("plane"))
            {
                assert!(member_matches_oracle(
                    *value,
                    expected.as_f64().expect("oracle")
                ));
            }
        }
        member_bits.insert(
            parameter.to_owned(),
            json!(actual
                .chunks(9)
                .map(|p| p
                    .iter()
                    .map(|v| format!("{:08x}", v.to_bits()))
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>()),
        );
        member_values.insert(parameter.to_owned(), json!(planes));
    }
    let rows: Vec<_> = values.chunks(2).zip(&expected).zip(&queries).zip(&case_ids).enumerate().map(|(index, (((actual, original), query), case))| {
        let differences = [(f64::from(actual[0]) - original[0]).abs(), (f64::from(actual[1]) - original[1]).abs()];
        let passed = actual.iter().zip(original).all(|(c, o)| member_matches_oracle(f64::from(*c), *o));
            json!({"index": index, "case": case, "query": query, "candidate": actual.iter().map(|v| f64::from(*v)).collect::<Vec<_>>(), "original": original,
            "candidate_bits": actual.iter().map(|v| format!("{:08x}", v.to_bits())).collect::<Vec<_>>(),
            "absolute_differences": differences, "verdict": if passed {"passed"} else {"failed"}})
    }).collect();
    let passed = rows.iter().all(|r| r["verdict"] == "passed");
    let candidate: Vec<f64> = values.iter().map(|v| f64::from(*v)).collect();
    let expected_flat: Vec<f64> = expected.iter().flatten().copied().collect();
    let input_manifest = serde_json::to_string(&metadata).expect("input manifest");
    let evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "genuine-precipitation-temporal-119".to_owned(),
        candidate: GpuCandidateEvidence {
            implementation_id: "precipitation-temporal-gpu-119".to_owned(),
            revision: git(&["rev-parse", "HEAD"]),
            shader_sha256: precipitation_temporal_shader_sha256(),
            input_sha256: grib202::sha256_hex(input_manifest.as_bytes()),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(adapter.clone()),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(PinnedOracleEvidence {
            implementation_id: "FLEXPART-11.1 interpol_rain numpf=1".to_owned(),
            revision: original["flexpart_revision"]
                .as_str()
                .expect("pin")
                .to_owned(),
            executable_sha256: original["executable_sha256"]
                .as_str()
                .expect("executable")
                .to_owned(),
            output_sha256: grib202::sha256_hex(ORIGINAL.as_bytes()),
        }),
        comparison: compare_finite_values(
            &expected_flat,
            &candidate,
            ComparisonPolicy::new(1e-6, 1e-5).expect("#71 policy"),
        )
        .expect("finite comparison"),
    };
    evidence.require_paired_pass().expect("#91 paired evidence");
    let report = json!({"schema": "flexpart-gpu.precipitation-temporal-device.v1",
        "candidate": {"revision": git(&["rev-parse", "HEAD"]), "worktree_clean": git(&["status", "--porcelain"]).is_empty()},
        "adapter": adapter, "metadata": metadata, "status": members.status(),
        "fixture_sha256": grib202::sha256_hex(ORIGINAL.as_bytes()),
        "darain_oracle_sha256": grib202::ORACLE_SHA256, "decision_216_sha256": grib202::DECISION_SHA256,
        "grib_files_sha256": grib202::grib_file_hashes(&grib202::decision()),
        "shaders": {"accumulated_interval": accumulated_shader_sha256(), "darain": darain_shader_sha256(),
            "horizontal": horizontal_shader_sha256(), "precipitation_temporal": precipitation_temporal_shader_sha256()},
        "encoders": 1, "submissions": 1, "host_transfers_between_stages": 0,
        "stage_order": ["#90 LSP", "#90 CP", "#207 LSP", "#207 CP", "#87 selected planes", "#119"],
        "comparison_policy": {"absolute": 1e-6, "relative": 1e-5}, "unit": "millimeter_per_hour",
        "gpu_evidence": evidence,
        "input_manifest": input_manifest,
        "source_sha256": {"temporal": grib202::sha256_hex(include_bytes!("../src/gpu/precipitation_temporal.rs")),
            "test": grib202::sha256_hex(include_bytes!("precipitation_temporal_gpu.rs")),
            "member": grib202::sha256_hex(include_bytes!("../src/gpu/precipitation_members.rs")),
            "spatial": grib202::sha256_hex(include_bytes!("../src/gpu/horizontal.rs")),
            "rate": grib202::sha256_hex(include_bytes!("../src/gpu/meteorology/interval_rates.rs"))},
        "member_values_mm_per_h": member_values, "member_bits": member_bits, "rows": rows,
        "verdict": if passed {"passed"} else {"failed"}});
    std::fs::write(&path, serde_json::to_string_pretty(&report).expect("JSON")).expect("evidence");
    assert!(
        passed,
        "GPU composition must match unchanged pinned original; {}",
        path.display()
    );
}

#[test]
fn test_precipitation_temporal_window_context_and_query_rejections() {
    let snapshots = grib202::snapshots(&grib202::observations());
    let ctx = pollster::block_on(GpuContext::new()).expect("required GPU");
    let other = pollster::block_on(GpuContext::new()).expect("independent context");
    let lsp = upload(&ctx, FieldId::LargeScalePrecipitation, &snapshots);
    let cp = upload(&ctx, FieldId::ConvectivePrecipitation, &snapshots);
    let mut producer = prepare_precipitation_members(
        &ctx,
        &lsp,
        &cp,
        PrecipitationMemberPreparation::flex_extract_rrint0(),
    )
    .expect("members");
    let metadata = producer.metadata().clone();
    let first = metadata.members[0].valid_time_epoch_seconds;
    let last = metadata
        .members
        .last()
        .expect("last")
        .valid_time_epoch_seconds;
    let query = PrecipitationTimeQuery {
        xt: 0.5,
        yt: 0.25,
        itime_epoch_seconds: first,
    };
    assert!(prepare_precipitation_time_samples(&other, &mut producer, &[query], 1).is_err());
    for direction in [-1, 0, 2] {
        assert!(
            prepare_precipitation_time_samples(&ctx, &mut producer, &[query], direction).is_err()
        );
    }
    for (x, y) in [
        (f64::NAN, 0.0),
        (0.0, f64::INFINITY),
        (-0.1, 0.0),
        (0.0, -0.1),
        (2.1, 0.0),
        (0.0, 2.1),
    ] {
        assert!(prepare_precipitation_time_samples(
            &ctx,
            &mut producer,
            &[PrecipitationTimeQuery {
                xt: x,
                yt: y,
                ..query
            }],
            1
        )
        .is_err());
    }
    assert!(prepare_precipitation_time_samples(&ctx, &mut producer, &[], 1).is_err());
    for t in [first - 1, last + 1] {
        assert!(select_precipitation_memory_pair(&metadata, t).is_err());
    }
    let pair = select_precipitation_memory_pair(&metadata, first + metadata.delta_seconds)
        .expect("boundary");
    assert_eq!(pair.left.slot, 1);
    assert_eq!(pair.right.slot, 2);
    let pair = select_precipitation_memory_pair(&metadata, last).expect("final duplicate");
    assert_eq!(pair.left, pair.right);
    // Full-window validation includes members and support outside the selected pair.
    for index in 0..metadata.members.len() {
        let mut broken = metadata.clone();
        broken.members[index].valid_time_epoch_seconds += 1;
        assert!(select_precipitation_memory_pair(&broken, first).is_err());
    }
    type MetadataMutation = Box<dyn Fn(&mut PrecipitationMemberMetadata)>;
    let mut mutations: Vec<MetadataMutation> = vec![
        Box::new(|m| {
            m.members.remove(0);
        }),
        Box::new(|m| {
            m.members.pop();
        }),
        Box::new(|m| m.members.swap(1, 2)),
        Box::new(|m| m.members[1] = m.members[0].clone()),
        Box::new(|m| m.intervals[0].start_epoch_seconds += 1),
        Box::new(|m| m.intervals[3].start_epoch_seconds += 3600),
        Box::new(|m| m.intervals[0].end_epoch_seconds = m.intervals[0].start_epoch_seconds),
        Box::new(|m| m.delta_seconds = 0),
        Box::new(|m| m.delta_seconds = -3600),
        Box::new(|m| m.preparation.rrint = 1),
        Box::new(|m| m.preparation.numpf = 3),
        Box::new(|m| m.preparation.purefc = 1),
        Box::new(|m| m.preparation.basetime_hour = Some(0)),
        Box::new(|m| m.unit = "kg_per_square_meter".to_owned()),
        Box::new(|m| m.representation = "interval_mean".to_owned()),
    ];
    for mutate in mutations.drain(..) {
        let mut broken = metadata.clone();
        mutate(&mut broken);
        assert!(select_precipitation_memory_pair(&broken, first).is_err());
    }
    assert!(producer.metadata().member_at(first + 1).is_err());
    let kernels = PrecipitationTemporalKernels::new(&ctx).expect("kernels");
    let foreign = PrecipitationTemporalKernels::new(&other).expect("foreign kernels");
    let mut plan =
        prepare_precipitation_time_samples(&ctx, &mut producer, &[query], 1).expect("plan");
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    assert!(plan.encode(&ctx, &foreign, &mut encoder).is_err());
    plan.encode(&ctx, &kernels, &mut encoder).expect("encode");
    assert!(
        plan.encode(&ctx, &kernels, &mut encoder).is_err(),
        "one-shot batch prevents stale reuse"
    );
    drop(encoder);
    assert!(
        pollster::block_on(plan.download(&ctx)).is_err(),
        "no submission must never publish zero precipitation"
    );
}

#[test]
fn test_precipitation_temporal_device_producer_failure_invalidates_entire_batch() {
    let mut snapshots = grib202::snapshots(&grib202::observations());
    for snapshot in &mut snapshots {
        for field in &mut snapshot.fields {
            if [
                FieldId::LargeScalePrecipitation,
                FieldId::ConvectivePrecipitation,
            ]
            .contains(&field.id)
            {
                field.values.fill(f32::MAX);
                field.source_quantization = None;
                let start = field.time.valid_time_epoch_seconds - 10_800;
                field.time.interval_start_epoch_seconds = Some(start);
                field
                    .time
                    .accumulation
                    .as_mut()
                    .expect("reset")
                    .reset_epoch_seconds = start;
            }
        }
    }
    let ctx = pollster::block_on(GpuContext::new()).expect("GPU");
    let lsp = upload(&ctx, FieldId::LargeScalePrecipitation, &snapshots);
    let cp = upload(&ctx, FieldId::ConvectivePrecipitation, &snapshots);
    let mut producer = prepare_precipitation_members(
        &ctx,
        &lsp,
        &cp,
        PrecipitationMemberPreparation::flex_extract_rrint0(),
    )
    .expect("finite source");
    let t = producer.metadata().members[0].valid_time_epoch_seconds;
    let kernels = PrecipitationTemporalKernels::new(&ctx).expect("kernels");
    let queries = [
        PrecipitationTimeQuery {
            xt: 0.0,
            yt: 0.0,
            itime_epoch_seconds: t,
        },
        PrecipitationTimeQuery {
            xt: 1.25,
            yt: 0.5,
            itime_epoch_seconds: t + 1,
        },
    ];
    let mut plan =
        prepare_precipitation_time_samples(&ctx, &mut producer, &queries, 1).expect("plan");
    assert!(
        pollster::block_on(plan.download(&ctx)).is_err(),
        "missing encoded producer resource"
    );
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    plan.encode(&ctx, &kernels, &mut encoder).expect("encode");
    ctx.queue.submit(Some(encoder.finish()));
    assert!(
        matches!(
            pollster::block_on(plan.download(&ctx)),
            Err(
                flexpart_gpu::gpu::precipitation_temporal::PrecipitationTemporalError::DeviceStatus(
                    _
                )
            )
        ),
        "overflow in #207 must reject every output"
    );
}
