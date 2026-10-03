//! #76 composition proof, deliberately separate from the #87–#90 scientific matrices.
//! Real device execution is mandatory here: absent adapters never pass by skipping.

use flexpart_gpu::gpu::{
    accumulation::{accumulated_comparison_policy, accumulated_shader_sha256},
    compare_finite_values, download_buffer_typed,
    horizontal::{default_comparison_policy as horizontal_policy, horizontal_shader_sha256},
    meteorology::{
        CanonicalGpuField, IntervalQuantity, MeteorologyCompositionError,
        MeteorologyCompositionKernels, MeteorologyHeight, MeteorologySampleMetadata,
        MeteorologySampleRequest, MeteorologyStage, MeteorologyStageRecord,
        MeteorologyTimeSelection,
    },
    vertical::{
        vertical_model_comparison_policy, vertical_sample_shader_sha256,
        vertical_w_bundle_shader_sha256, vertical_w_comparison_policy,
    },
    ComparisonPolicy, GpuAdapterEvidence, GpuContext, NumericalVerdict,
};
use flexpart_gpu::meteorology::{
    temporal::RequestedSampleTime,
    vertical::{
        reconstruct_vertical_geometry, reconstruct_vertical_geometry_with_motion,
        NativeVerticalMotion, NativeVerticalMotionKind, NativeVerticalMotionProvenance,
        NativeVerticalMotionSign, NativeVerticalMotionUnit,
    },
    Axis, Calendar, Field, FieldId, FieldTime, SignConvention, Snapshot, TemporalKind, Unit,
    VerticalReference, VerticalStaggering,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn base() -> Snapshot {
    serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-column-v1.json"
    ))
    .unwrap()
}

fn time(snapshot: &mut Snapshot, seconds: i64) {
    for field in &mut snapshot.fields {
        if field.time.kind != TemporalKind::Static {
            field.time.valid_time_epoch_seconds = seconds;
        }
    }
}

fn expand(snapshot: &mut Snapshot, nx: usize, ny: usize) {
    snapshot.horizontal_grid.nx = nx;
    snapshot.horizontal_grid.ny = ny;
    for field in &mut snapshot.fields {
        field.shape[0] = nx;
        field.shape[1] = ny;
        field.values = field
            .values
            .iter()
            .flat_map(|v| vec![*v; nx * ny])
            .collect();
    }
}

fn surface_field(
    snapshot: &Snapshot,
    id: FieldId,
    unit: Unit,
    sign: SignConvention,
    values: Vec<f32>,
) -> Field {
    let mut field = snapshot
        .fields
        .iter()
        .find(|f| f.id == FieldId::SurfacePressure)
        .unwrap()
        .clone();
    field.id = id;
    field.unit = unit;
    field.sign = sign;
    field.values = values;
    field
}

fn horizontal_values() -> Vec<f32> {
    // Reuse the #71 pinned horizontal field, rather than another interpolation oracle.
    let contract: Value =
        serde_json::from_str(include_str!("../fixtures/interpolation/contract-v1.json")).unwrap();
    let case = contract["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "horizontal-interior")
        .unwrap();
    case["input"].as_array().unwrap()[4..16]
        .iter()
        .map(|v| v.as_str().unwrap().parse().unwrap())
        .collect()
}

fn instant(seconds: i64) -> MeteorologyTimeSelection {
    MeteorologyTimeSelection::Instantaneous(RequestedSampleTime::new(Calendar::Gregorian, seconds))
}

fn request(
    xt: f64,
    yt: f64,
    time: MeteorologyTimeSelection,
    height: Option<MeteorologyHeight>,
) -> MeteorologySampleRequest {
    MeteorologySampleRequest {
        xt,
        yt,
        time,
        height,
    }
}

fn revision() -> String {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

struct Case<'a> {
    id: &'a str,
    source: &'a CanonicalGpuField<'a>,
    request: MeteorologySampleRequest,
    expected: Vec<f32>,
    stages: Vec<MeteorologyStage>,
    policy: ComparisonPolicy,
    source_snapshots: Vec<&'a Snapshot>,
    source_indices: Vec<usize>,
}

fn stage_sequence(stages: &[MeteorologyStageRecord]) -> Vec<MeteorologyStage> {
    stages.iter().map(|s| s.stage).collect()
}

fn expected_records(case: &Case<'_>) -> Vec<MeteorologyStageRecord> {
    use MeteorologyStage::{AccumulatedTransform, Horizontal, Temporal, Vertical, WRemap};
    let mut records = Vec::new();
    if case.stages.contains(&AccumulatedTransform) {
        records.push(MeteorologyStageRecord {
            stage: AccumulatedTransform,
            source_index: None,
            plane_index: None,
        });
    }
    for index in &case.source_indices {
        let spatial = if case.stages.contains(&WRemap) {
            vec![WRemap, Vertical]
        } else if case.stages.contains(&Vertical) {
            vec![Horizontal, Horizontal, Horizontal, Vertical]
        } else {
            vec![Horizontal; case.expected.len()]
        };
        for (plane, stage) in spatial.into_iter().enumerate() {
            records.push(MeteorologyStageRecord {
                stage,
                source_index: Some(*index),
                plane_index: (stage == Horizontal).then_some(plane),
            });
        }
    }
    if case.stages.contains(&Temporal) {
        records.push(MeteorologyStageRecord {
            stage: Temporal,
            source_index: None,
            plane_index: None,
        });
    }
    records
}

fn run_cases(
    ctx: &GpuContext,
    kernels: &MeteorologyCompositionKernels<'_>,
    cases: &[Case<'_>],
) -> Vec<Value> {
    let mut plans: Vec<_> = cases
        .iter()
        .map(|c| c.source.prepare_sample(ctx, c.request).unwrap())
        .collect();
    ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
    ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("#76 caller encoder"),
        });
    let mut outputs = Vec::new();
    for (plan, case) in plans.iter_mut().zip(cases) {
        let handoff = plan.encode(ctx, kernels, &mut encoder).unwrap();
        assert_eq!(
            stage_sequence(handoff.stages),
            case.stages,
            "{} field-specific ordering",
            case.id
        );
        assert_eq!(
            handoff.stages,
            expected_records(case),
            "{} source/plane identity",
            case.id
        );
        assert_eq!(handoff.metadata.value_count, case.expected.len());
        assert_eq!(handoff.metadata.request, case.request);
        assert!(handoff.values.usage().contains(wgpu::BufferUsages::STORAGE));
        assert!(!handoff
            .values
            .usage()
            .contains(wgpu::BufferUsages::MAP_READ));
        // Minimal downstream adapter: a device copy in the same caller encoder.
        // Poisoning the destination ensures missing production writes cannot pass.
        let consumer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("#77 handoff fixture consumer"),
            size: case.expected.len() as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(
            &consumer,
            0,
            bytemuck::cast_slice(&vec![f32::NAN; case.expected.len()]),
        );
        encoder.copy_buffer_to_buffer(
            handoff.values,
            0,
            &consumer,
            0,
            case.expected.len() as u64 * 4,
        );
        outputs.push((
            consumer,
            handoff.metadata.clone(),
            handoff.stages.to_vec(),
            handoff.device_copies,
        ));
    }
    // One submit for every dependent producer and fixture consumer, with no
    // intermediate host observation. Only this test owns submit/wait/readback.
    ctx.queue.submit(Some(encoder.finish()));
    ctx.device.poll(wgpu::Maintain::Wait);
    let errors: Vec<_> = (0..3)
        .filter_map(|_| pollster::block_on(ctx.device.pop_error_scope()))
        .collect();
    assert!(
        errors.is_empty(),
        "scoped composition device errors: {errors:?}"
    );
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/ci-gate/meteorology-composition");
    std::fs::create_dir_all(&directory).unwrap();
    let mut rows = Vec::new();
    for ((consumer, metadata, stages, copies), case) in outputs.into_iter().zip(cases) {
        let actual: Vec<f32> = pollster::block_on(download_buffer_typed(
            ctx,
            &consumer,
            case.expected.len(),
            "#76 final validation consumer",
        ))
        .unwrap();
        let comparison = compare_finite_values(
            &case
                .expected
                .iter()
                .map(|v| f64::from(*v))
                .collect::<Vec<_>>(),
            &actual.iter().map(|v| f64::from(*v)).collect::<Vec<_>>(),
            case.policy,
        )
        .unwrap();
        // Keep the canonical snapshot's typed serialization: f32 shortest-decimal
        // encoding differs from promotion through serde_json::Value's f64 lanes.
        let source_bytes: Vec<_> = case
            .source_snapshots
            .iter()
            .map(|snapshot| serde_json::to_vec(snapshot).unwrap())
            .collect();
        let snapshots: Vec<Value> = source_bytes
            .iter()
            .map(|bytes| serde_json::from_slice(bytes).unwrap())
            .collect();
        let inputs = json!({"snapshots": snapshots, "request": case.request});
        let input_bytes = serde_json::to_vec(&inputs).unwrap();
        let input_path = directory.join(format!("{}-inputs.json", case.id));
        std::fs::write(input_path, &input_bytes).unwrap();
        for (index, bytes) in source_bytes.iter().enumerate() {
            std::fs::write(
                directory.join(format!("{}-source-{index}.json", case.id)),
                bytes,
            )
            .unwrap();
        }
        assert_handoff_identity(&metadata, case);
        let handoff_bytes = serde_json::to_vec(&serde_json::to_value(&metadata).unwrap()).unwrap();
        std::fs::write(
            directory.join(format!("{}-handoff.json", case.id)),
            &handoff_bytes,
        )
        .unwrap();
        let row = json!({
            "case_id": case.id, "metadata": metadata,
            "handoff_sha256": format!("{:x}", Sha256::digest(&handoff_bytes)),
            "input_sha256": format!("{:x}", Sha256::digest(&input_bytes)),
            "encoded_stages": stages, "expected_stages": case.stages,
            "expected_stage_records": expected_records(case),
            "device_to_device_copies": copies,
            "candidate_values": actual, "expected_values": case.expected,
            "comparison": comparison,
            "status": if comparison.verdict == NumericalVerdict::Passed { "passed" } else { "failed" },
        });
        // Preserve failing numerical evidence before raising the test failure.
        std::fs::write(
            directory.join(format!("{}.json", case.id)),
            serde_json::to_vec_pretty(&row).unwrap(),
        )
        .unwrap();
        assert_eq!(
            comparison.verdict,
            NumericalVerdict::Passed,
            "{} final composed output",
            case.id
        );
        rows.push(row);
    }
    rows
}

fn assert_handoff_identity(metadata: &MeteorologySampleMetadata, case: &Case<'_>) {
    let history: Vec<_> = if case
        .stages
        .contains(&MeteorologyStage::AccumulatedTransform)
    {
        case.source_snapshots.clone()
    } else {
        case.source_indices
            .iter()
            .map(|i| case.source_snapshots[*i])
            .collect()
    };
    assert_eq!(
        metadata.source_snapshot_sha256,
        history
            .iter()
            .map(|s| format!("{:x}", Sha256::digest(serde_json::to_vec(s).unwrap())))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        metadata.source_times,
        history
            .iter()
            .map(|s| s
                .fields
                .iter()
                .find(|f| f.id == metadata.field_id)
                .unwrap()
                .time
                .clone())
            .collect::<Vec<_>>()
    );
    if let Some(bracket) = &metadata.temporal_bracket {
        assert_eq!(
            [bracket.lower_index, bracket.upper_index],
            case.source_indices.as_slice()
        );
    }
    for provenance in &metadata.geometry_provenance {
        assert!(metadata
            .source_snapshot_sha256
            .contains(&provenance.source_snapshot_sha256));
    }
    assert_eq!(
        metadata.horizontal_grid,
        case.source_snapshots[0].horizontal_grid
    );
    assert_eq!(
        metadata.geometry_identity.len(),
        metadata.geometry_provenance.len()
    );
    if case.stages.contains(&MeteorologyStage::Vertical) {
        assert_eq!(metadata.geometry_identity.len(), 2);
        assert!(metadata.request.height.is_some());
    } else {
        assert!(metadata.geometry_identity.is_empty());
        assert!(metadata.request.height.is_none());
    }
    let field = case.source_snapshots[0]
        .fields
        .iter()
        .find(|f| f.id == metadata.field_id)
        .unwrap();
    assert_eq!(metadata.sign, field.sign);
    assert_eq!(metadata.horizontal_staggering, field.horizontal_staggering);
    assert_eq!(metadata.vertical_staggering, field.vertical_staggering);
    let expected_unit = match case.request.time {
        MeteorologyTimeSelection::AccumulatedInterval { quantity, .. } => match quantity {
            IntervalQuantity::Amount => "kilogram_per_square_meter".to_string(),
            IntervalQuantity::RateSi => "kilogram_per_square_meter_per_second".to_string(),
            IntervalQuantity::RateMillimeterPerHour => "millimeter_per_hour".to_string(),
        },
        _ => serde_json::to_value(field.unit)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string(),
    };
    assert_eq!(metadata.unit, expected_unit);
    assert_eq!(
        metadata.resolved_heights_agl_m.len(),
        metadata.geometry_provenance.len()
    );
}

#[test]
fn test_meteorology_field_specific_composition_device_handoff() {
    use MeteorologyStage::{
        AccumulatedTransform as A, Horizontal as H, Temporal as T, Vertical as V, WRemap as W,
    };
    // Invalidate the aggregate verdict before any fallible device setup or execution.
    // Retained per-case failures cannot coexist with an old passing aggregate.
    let report_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/ci-gate/meteorology-composition/report.json");
    if report_path.exists() {
        std::fs::remove_file(&report_path).unwrap();
    }
    let ctx =
        pollster::block_on(GpuContext::new()).expect("#76 required GPU execution cannot skip");
    let kernels = MeteorologyCompositionKernels::new(&ctx).unwrap();
    let mut s0 = base();
    time(&mut s0, 0);
    expand(&mut s0, 4, 3);
    s0.fields.push(surface_field(
        &s0,
        FieldId::MixingHeight,
        Unit::Meter,
        SignConvention::NonNegative,
        horizontal_values(),
    ));
    let mut s1 = s0.clone();
    time(&mut s1, 3600);
    for value in &mut s1.fields.last_mut().unwrap().values {
        *value += 10.0;
    }
    let mut s2 = s1.clone();
    time(&mut s2, 7200);
    for value in &mut s2.fields.last_mut().unwrap().values {
        *value += 10.0;
    }
    let surface = CanonicalGpuField::upload(
        &ctx,
        FieldId::MixingHeight,
        &[&s0, &s1, &s2],
        &[None, None, None],
    )
    .unwrap();

    let mut m0 = s0.clone();
    let mut wind = surface_field(
        &m0,
        FieldId::WindU,
        Unit::MeterPerSecond,
        SignConvention::PositiveEastward,
        vec![],
    );
    wind.shape.push(3);
    wind.axis_order.push(Axis::Z);
    wind.vertical_staggering = VerticalStaggering::LevelCenter;
    wind.values = [3.0, 2.0, 1.0]
        .into_iter()
        .flat_map(|level| horizontal_values().into_iter().map(move |v| v + level))
        .collect();
    m0.fields.push(wind);
    let mut m1 = m0.clone();
    time(&mut m1, 3600);
    for value in &mut m1.fields.last_mut().unwrap().values {
        *value += 10.0;
    }
    let g0 = reconstruct_vertical_geometry(&m0).unwrap();
    let g1 = reconstruct_vertical_geometry(&m1).unwrap();
    let r0 = g0.runtime_view().unwrap();
    let r1 = g1.runtime_view().unwrap();
    let model = CanonicalGpuField::upload(&ctx, FieldId::WindU, &[&m0, &m1], &[Some(r0), Some(r1)])
        .unwrap();
    let height =
        (r0.level(0, 0, 2).unwrap().height_agl_m + r0.level(0, 0, 1).unwrap().height_agl_m) * 0.5;
    let agl = Some(MeteorologyHeight {
        meters: height,
        reference: VerticalReference::AboveGroundLevel,
    });
    let asl = Some(MeteorologyHeight {
        meters: height + r0.terrain_asl_m(0, 0).unwrap(),
        reference: VerticalReference::AboveMeanSeaLevel,
    });

    let native: NativeVerticalMotion = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"
    ))
    .unwrap();
    let mut w0 = base();
    time(&mut w0, 0);
    let initial = reconstruct_vertical_geometry_with_motion(&w0, &native).unwrap();
    let initial_runtime = initial.runtime_view().unwrap();
    let motion = initial_runtime.vertical_velocity().unwrap();
    let mut wfield = surface_field(
        &w0,
        FieldId::VerticalVelocity,
        Unit::MeterPerSecond,
        SignConvention::PositiveUpward,
        motion.values_ms().to_vec(),
    );
    wfield.shape.push(4);
    wfield.axis_order.push(Axis::Z);
    wfield.vertical_staggering = VerticalStaggering::LevelInterface;
    w0.fields.push(wfield);
    let mut w1 = w0.clone();
    time(&mut w1, 3600);
    let wg0 = reconstruct_vertical_geometry_with_motion(&w0, &native).unwrap();
    let wg1 = reconstruct_vertical_geometry_with_motion(&w1, &native).unwrap();
    let wsource = CanonicalGpuField::upload(
        &ctx,
        FieldId::VerticalVelocity,
        &[&w0, &w1],
        &[
            Some(wg0.runtime_view().unwrap()),
            Some(wg1.runtime_view().unwrap()),
        ],
    )
    .unwrap();
    let w_oracle: Value = serde_json::from_str(include_str!(
        "../fixtures/interpolation/w-production-oracle-v1.json"
    ))
    .unwrap();
    let wquery = &w_oracle["synthetic_case"]["queries"][1];
    let wh = Some(MeteorologyHeight {
        meters: wquery["particle_height_m_agl"].as_f64().unwrap() as f32,
        reference: VerticalReference::AboveGroundLevel,
    });

    // Reuse #88's center-motion input; exact lowest-level sampling isolates
    // runtime binding, physical level reversal and downstream temporal composition.
    let mut center_native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::GeometricVelocity,
        unit: NativeVerticalMotionUnit::MeterPerSecond,
        sign: NativeVerticalMotionSign::PositiveUpward,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values: vec![0.5, -0.25, 0.125],
        provenance: NativeVerticalMotionProvenance {
            source_id: "vertical-gpu-center-w-test".to_string(),
        },
    };
    let mut c0 = base();
    time(&mut c0, 0);
    let mut center_field = surface_field(
        &c0,
        FieldId::VerticalVelocity,
        Unit::MeterPerSecond,
        SignConvention::PositiveUpward,
        center_native.values.clone(),
    );
    center_field.shape.push(3);
    center_field.axis_order.push(Axis::Z);
    center_field.vertical_staggering = VerticalStaggering::LevelCenter;
    c0.fields.push(center_field);
    let cg0 = reconstruct_vertical_geometry_with_motion(&c0, &center_native).unwrap();
    let mut c1 = c0.clone();
    time(&mut c1, 3600);
    center_native.values.iter_mut().for_each(|v| *v += 1.0);
    c1.fields.last_mut().unwrap().values = center_native.values.clone();
    let cg1 = reconstruct_vertical_geometry_with_motion(&c1, &center_native).unwrap();
    let center_source = CanonicalGpuField::upload(
        &ctx,
        FieldId::VerticalVelocity,
        &[&c0, &c1],
        &[
            Some(cg0.runtime_view().unwrap()),
            Some(cg1.runtime_view().unwrap()),
        ],
    )
    .unwrap();
    let center_height = Some(MeteorologyHeight {
        meters: cg0
            .runtime_view()
            .unwrap()
            .level(0, 0, 2)
            .unwrap()
            .height_agl_m,
        reference: VerticalReference::AboveGroundLevel,
    });

    let mut a0 = s0.clone();
    let mut rain = surface_field(
        &a0,
        FieldId::LargeScalePrecipitation,
        Unit::KilogramPerSquareMeter,
        SignConvention::NonNegative,
        horizontal_values(),
    );
    rain.time = FieldTime {
        calendar: Calendar::Gregorian,
        kind: TemporalKind::AccumulatedSinceReset,
        valid_time_epoch_seconds: 3600,
        interval_start_epoch_seconds: Some(0),
        interval_end_epoch_seconds: Some(3600),
        accumulation: Some(flexpart_gpu::meteorology::Accumulation {
            reset_epoch_seconds: 0,
        }),
    };
    time(&mut a0, 3600);
    a0.fields.push(rain);
    let mut a1 = a0.clone();
    time(&mut a1, 7200);
    let rain1 = a1.fields.last_mut().unwrap();
    rain1.time.interval_end_epoch_seconds = Some(7200);
    for value in &mut rain1.values {
        *value += 2.0;
    }
    let mut a2 = a1.clone();
    time(&mut a2, 10800);
    let rain2 = a2.fields.last_mut().unwrap();
    rain2.time.interval_start_epoch_seconds = Some(7200);
    rain2.time.interval_end_epoch_seconds = Some(10800);
    rain2
        .time
        .accumulation
        .as_mut()
        .unwrap()
        .reset_epoch_seconds = 7200;
    rain2.values.fill(4.0);
    let accumulated = CanonicalGpuField::upload(
        &ctx,
        FieldId::LargeScalePrecipitation,
        &[&a0, &a1, &a2],
        &[None, None, None],
    )
    .unwrap();

    let mut ancillary = s0.clone();
    let mut classes = surface_field(
        &ancillary,
        FieldId::LandUseFractions,
        Unit::Fraction,
        SignConvention::NonNegative,
        vec![],
    );
    classes.shape.push(13);
    classes.axis_order.push(Axis::Class);
    classes.time.kind = TemporalKind::Static;
    classes.values = (0..13)
        .flat_map(|class| vec![if class == 2 { 1.0 } else { 0.0 }; 12])
        .collect();
    ancillary.fields.push(classes);
    let class_source =
        CanonicalGpuField::upload(&ctx, FieldId::LandUseFractions, &[&ancillary], &[None]).unwrap();
    let static_source =
        CanonicalGpuField::upload(&ctx, FieldId::Orography, &[&ancillary], &[None]).unwrap();

    let mut flux_snapshot = s0.clone();
    time(&mut flux_snapshot, 3600);
    let mut flux = surface_field(
        &flux_snapshot,
        FieldId::SensibleHeatFlux,
        Unit::WattPerSquareMeter,
        SignConvention::PositiveUpwardFlux,
        horizontal_values(),
    );
    flux.time.kind = TemporalKind::IntervalMean;
    flux.time.interval_start_epoch_seconds = Some(0);
    flux.time.interval_end_epoch_seconds = Some(3600);
    flux_snapshot.fields.push(flux);
    let flux_source =
        CanonicalGpuField::upload(&ctx, FieldId::SensibleHeatFlux, &[&flux_snapshot], &[None])
            .unwrap();

    let mut total_snapshot = a0.clone();
    let total = total_snapshot.fields.last_mut().unwrap();
    total.time.kind = TemporalKind::IntervalTotal;
    total.time.accumulation = None;
    let total_source = CanonicalGpuField::upload(
        &ctx,
        FieldId::LargeScalePrecipitation,
        &[&total_snapshot],
        &[None],
    )
    .unwrap();

    let cases = vec![
        Case {
            id: "instantaneous-surface",
            source: &surface,
            request: request(1.25, 0.5, instant(1800), None),
            expected: vec![235.0],
            stages: vec![H, H, T],
            policy: horizontal_policy().unwrap(),
            source_snapshots: vec![&s0, &s1, &s2],
            source_indices: vec![0, 1],
        },
        Case {
            id: "surface-first-endpoint",
            source: &surface,
            request: request(1.25, 0.5, instant(0), None),
            expected: vec![230.0],
            stages: vec![H, H, T],
            policy: horizontal_policy().unwrap(),
            source_snapshots: vec![&s0, &s1, &s2],
            source_indices: vec![0, 1],
        },
        Case {
            id: "surface-last-endpoint",
            source: &surface,
            request: request(1.25, 0.5, instant(7200), None),
            expected: vec![250.0],
            stages: vec![H, H, T],
            policy: horizontal_policy().unwrap(),
            source_snapshots: vec![&s0, &s1, &s2],
            source_indices: vec![1, 2],
        },
        Case {
            id: "model-agl",
            source: &model,
            request: request(1.25, 0.5, instant(1800), agl),
            expected: vec![236.5],
            stages: vec![H, H, H, V, H, H, H, V, T],
            policy: vertical_model_comparison_policy().unwrap(),
            source_snapshots: vec![&m0, &m1],
            source_indices: vec![0, 1],
        },
        Case {
            id: "surface-later-bracket",
            source: &surface,
            request: request(1.25, 0.5, instant(5400), None),
            expected: vec![245.0],
            stages: vec![H, H, T],
            policy: horizontal_policy().unwrap(),
            source_snapshots: vec![&s0, &s1, &s2],
            source_indices: vec![1, 2],
        },
        Case {
            id: "center-w",
            source: &center_source,
            request: request(0.0, 0.0, instant(1800), center_height),
            expected: vec![0.625],
            stages: vec![H, H, H, V, H, H, H, V, T],
            policy: vertical_model_comparison_policy().unwrap(),
            source_snapshots: vec![&c0, &c1],
            source_indices: vec![0, 1],
        },
        Case {
            id: "model-asl",
            source: &model,
            request: request(1.25, 0.5, instant(1800), asl),
            expected: vec![236.5],
            stages: vec![H, H, H, V, H, H, H, V, T],
            policy: vertical_model_comparison_policy().unwrap(),
            source_snapshots: vec![&m0, &m1],
            source_indices: vec![0, 1],
        },
        Case {
            id: "interface-w",
            source: &wsource,
            request: request(0.0, 0.0, instant(1800), wh),
            expected: vec![wquery["pristine_w_m_s"].as_f64().unwrap() as f32],
            stages: vec![W, V, W, V, T],
            policy: vertical_w_comparison_policy().unwrap(),
            source_snapshots: vec![&w0, &w1],
            source_indices: vec![0, 1],
        },
        Case {
            id: "interval-leading-rate",
            source: &accumulated,
            request: request(
                1.25,
                0.5,
                MeteorologyTimeSelection::AccumulatedInterval {
                    start_epoch_seconds: 0,
                    end_epoch_seconds: 3600,
                    quantity: IntervalQuantity::RateMillimeterPerHour,
                },
                None,
            ),
            expected: vec![230.0],
            stages: vec![A, H],
            policy: accumulated_comparison_policy().unwrap(),
            source_snapshots: vec![&a0, &a1, &a2],
            source_indices: vec![0],
        },
        Case {
            id: "interval-delta-amount",
            source: &accumulated,
            request: request(
                1.25,
                0.5,
                MeteorologyTimeSelection::AccumulatedInterval {
                    start_epoch_seconds: 3600,
                    end_epoch_seconds: 7200,
                    quantity: IntervalQuantity::Amount,
                },
                None,
            ),
            expected: vec![2.0],
            stages: vec![A, H],
            policy: accumulated_comparison_policy().unwrap(),
            source_snapshots: vec![&a0, &a1, &a2],
            source_indices: vec![1],
        },
        Case {
            id: "interval-reset-si-rate",
            source: &accumulated,
            request: request(
                1.25,
                0.5,
                MeteorologyTimeSelection::AccumulatedInterval {
                    start_epoch_seconds: 7200,
                    end_epoch_seconds: 10800,
                    quantity: IntervalQuantity::RateSi,
                },
                None,
            ),
            expected: vec![4.0 / 3600.0],
            stages: vec![A, H],
            policy: accumulated_comparison_policy().unwrap(),
            source_snapshots: vec![&a0, &a1, &a2],
            source_indices: vec![2],
        },
        Case {
            id: "static-class",
            source: &class_source,
            request: request(1.25, 0.5, MeteorologyTimeSelection::Static, None),
            expected: (0..13).map(|i| if i == 2 { 1.0 } else { 0.0 }).collect(),
            stages: vec![H; 13],
            policy: horizontal_policy().unwrap(),
            source_snapshots: vec![&ancillary],
            source_indices: vec![0],
        },
        Case {
            id: "static-scalar",
            source: &static_source,
            request: request(1.25, 0.5, MeteorologyTimeSelection::Static, None),
            expected: vec![250.0],
            stages: vec![H],
            policy: horizontal_policy().unwrap(),
            source_snapshots: vec![&ancillary],
            source_indices: vec![0],
        },
        Case {
            id: "interval-mean-flux",
            source: &flux_source,
            request: request(
                1.25,
                0.5,
                MeteorologyTimeSelection::IntervalMean {
                    start_epoch_seconds: 0,
                    end_epoch_seconds: 3600,
                },
                None,
            ),
            expected: vec![230.0],
            stages: vec![H],
            policy: horizontal_policy().unwrap(),
            source_snapshots: vec![&flux_snapshot],
            source_indices: vec![0],
        },
        Case {
            id: "interval-total",
            source: &total_source,
            request: request(
                1.25,
                0.5,
                MeteorologyTimeSelection::IntervalTotal {
                    start_epoch_seconds: 0,
                    end_epoch_seconds: 3600,
                },
                None,
            ),
            expected: vec![230.0],
            stages: vec![H],
            policy: horizontal_policy().unwrap(),
            source_snapshots: vec![&total_snapshot],
            source_indices: vec![0],
        },
        Case {
            id: "model-below-domain",
            source: &model,
            request: request(
                1.25,
                0.5,
                instant(1800),
                Some(MeteorologyHeight {
                    meters: -10.0,
                    reference: VerticalReference::AboveGroundLevel,
                }),
            ),
            expected: vec![236.0],
            stages: vec![H, H, H, V, H, H, H, V, T],
            policy: vertical_model_comparison_policy().unwrap(),
            source_snapshots: vec![&m0, &m1],
            source_indices: vec![0, 1],
        },
        Case {
            id: "model-above-domain",
            source: &model,
            request: request(
                1.25,
                0.5,
                instant(1800),
                Some(MeteorologyHeight {
                    meters: 100000.0,
                    reference: VerticalReference::AboveGroundLevel,
                }),
            ),
            expected: vec![238.0],
            stages: vec![H, H, H, V, H, H, H, V, T],
            policy: vertical_model_comparison_policy().unwrap(),
            source_snapshots: vec![&m0, &m1],
            source_indices: vec![0, 1],
        },
    ];
    let rows = run_cases(&ctx, &kernels, &cases);
    let report = json!({
        "schema": {"id": "flexpart-gpu.meteorology-composition-evidence", "version": 1},
        "candidate_revision": revision(),
        "composition_source_sha256": format!("{:x}", Sha256::digest(include_bytes!("../src/gpu/meteorology.rs"))),
        "test_source_sha256": format!("{:x}", Sha256::digest(include_bytes!("meteorology_composition.rs"))),
        "execution": {"status": "passed", "calculation_path": "wgsl_device", "adapter": GpuAdapterEvidence::from_context(&ctx)},
        "scientific_claim": "composition of existing stages; no new per-stage or full FLEXPART parity verdict",
        "stage_evidence_references": [
            "target/horizontal-gpu-evidence.json", "target/ci-gate/vertical-gpu/vertical-model-levels.json",
            "target/ci-gate/vertical-gpu/w-production-synthetic.json", "target/temporal-gpu-evidence.json",
            "target/ci-gate/accumulation-gpu/"
        ],
        "stage_shader_sha256": {"horizontal": horizontal_shader_sha256(), "vertical": vertical_sample_shader_sha256(),
            "w_bundle": vertical_w_bundle_shader_sha256(), "temporal": format!("{:x}", Sha256::digest(include_bytes!("../src/shaders/temporal_interpolation.wgsl"))), "accumulation": accumulated_shader_sha256()},
        "residency_proof": {"caller_composition_submissions": 1, "intermediate_readbacks": 0,
            "consumer": "same-encoder device-copy adapter", "source_audit_test": "test_meteorology_encode_surface_has_no_host_completion"},
        "rows": rows, "status": "passed",
    });
    std::fs::write(report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}

#[test]
fn test_meteorology_composition_rejects_incompatible_inputs() {
    let ctx = pollster::block_on(GpuContext::new()).expect("#76 cannot skip");
    let mut lower = base();
    time(&mut lower, 0);
    let mut upper = lower.clone();
    time(&mut upper, 3600);
    let source = CanonicalGpuField::upload(
        &ctx,
        FieldId::SurfacePressure,
        &[&lower, &upper],
        &[None, None],
    )
    .unwrap();
    for query in [
        request(-0.1, 0.0, instant(1800), None),
        request(0.0, 0.1, instant(1800), None),
        request(f64::NAN, 0.0, instant(1800), None),
        request(0.0, 0.0, instant(-1), None),
        request(0.0, 0.0, instant(3601), None),
        request(0.0, 0.0, MeteorologyTimeSelection::Static, None),
        request(
            0.0,
            0.0,
            instant(1800),
            Some(MeteorologyHeight {
                meters: 1.0,
                reference: VerticalReference::AboveGroundLevel,
            }),
        ),
        request(
            0.0,
            0.0,
            MeteorologyTimeSelection::Instantaneous(RequestedSampleTime::new(
                Calendar::ProlepticGregorian,
                1800,
            )),
            None,
        ),
    ] {
        assert!(
            source.prepare_sample(&ctx, query).is_err(),
            "reject {query:?}"
        );
    }
    assert!(CanonicalGpuField::upload(&ctx, FieldId::SurfacePressure, &[], &[]).is_err());
    assert!(CanonicalGpuField::upload(&ctx, FieldId::SurfacePressure, &[&lower], &[None]).is_err());
    assert!(CanonicalGpuField::upload(
        &ctx,
        FieldId::SurfacePressure,
        &[&lower, &lower],
        &[None, None]
    )
    .is_err());
    assert!(
        CanonicalGpuField::upload(&ctx, FieldId::WindU, &[&lower, &upper], &[None, None]).is_err()
    );
    assert!(CanonicalGpuField::upload(
        &ctx,
        FieldId::Temperature,
        &[&lower, &upper],
        &[None, None]
    )
    .is_err());
    let mut malformed = upper.clone();
    malformed.fields[0].unit = Unit::Meter;
    assert!(CanonicalGpuField::upload(
        &ctx,
        FieldId::SurfacePressure,
        &[&lower, &malformed],
        &[None, None]
    )
    .is_err());
    malformed = upper.clone();
    malformed.fields[0].values.clear();
    assert!(CanonicalGpuField::upload(
        &ctx,
        FieldId::SurfacePressure,
        &[&lower, &malformed],
        &[None, None]
    )
    .is_err());
    malformed = upper.clone();
    malformed.horizontal_grid.xlon0_deg += 1.0;
    assert!(CanonicalGpuField::upload(
        &ctx,
        FieldId::SurfacePressure,
        &[&lower, &malformed],
        &[None, None]
    )
    .is_err());
    let g0 = reconstruct_vertical_geometry(&lower).unwrap();
    let g1 = reconstruct_vertical_geometry(&upper).unwrap();
    let r0 = g0.runtime_view().unwrap();
    let r1 = g1.runtime_view().unwrap();
    assert!(matches!(
        CanonicalGpuField::upload(
            &ctx,
            FieldId::Temperature,
            &[&lower, &upper],
            &[Some(r1), Some(r0)]
        ),
        Err(MeteorologyCompositionError::Incompatible(_))
    ));
    let model = CanonicalGpuField::upload(
        &ctx,
        FieldId::Temperature,
        &[&lower, &upper],
        &[Some(r0), Some(r1)],
    )
    .unwrap();
    assert!(model
        .prepare_sample(&ctx, request(0.0, 0.0, instant(1800), None))
        .is_err());
    for height in [
        MeteorologyHeight {
            meters: 1.0,
            reference: VerticalReference::ModelNative,
        },
        MeteorologyHeight {
            meters: f32::INFINITY,
            reference: VerticalReference::AboveGroundLevel,
        },
    ] {
        assert!(model
            .prepare_sample(&ctx, request(0.0, 0.0, instant(1800), Some(height)))
            .is_err());
    }
    let mut nonuniform0 = lower.clone();
    expand(&mut nonuniform0, 2, 1);
    nonuniform0
        .fields
        .iter_mut()
        .find(|f| f.id == FieldId::Temperature)
        .unwrap()
        .values[1] += 5.0;
    let mut nonuniform1 = nonuniform0.clone();
    time(&mut nonuniform1, 3600);
    let ng0 = reconstruct_vertical_geometry(&nonuniform0).unwrap();
    let ng1 = reconstruct_vertical_geometry(&nonuniform1).unwrap();
    let unsupported = CanonicalGpuField::upload(
        &ctx,
        FieldId::Temperature,
        &[&nonuniform0, &nonuniform1],
        &[
            Some(ng0.runtime_view().unwrap()),
            Some(ng1.runtime_view().unwrap()),
        ],
    )
    .unwrap();
    assert!(matches!(
        unsupported.prepare_sample(
            &ctx,
            request(
                0.5,
                0.0,
                instant(1800),
                Some(MeteorologyHeight {
                    meters: 2000.0,
                    reference: VerticalReference::AboveGroundLevel
                })
            )
        ),
        Err(MeteorologyCompositionError::Unsupported(_))
    ));
    // Integer position needs only the local column, so zero-weight neighbors
    // must not force a new geometry rule or a rejection.
    unsupported
        .prepare_sample(
            &ctx,
            request(
                0.0,
                0.0,
                instant(1800),
                Some(MeteorologyHeight {
                    meters: 2000.0,
                    reference: VerticalReference::AboveGroundLevel,
                }),
            ),
        )
        .unwrap();
    let mut flux0 = lower.clone();
    flux0.fields.push(surface_field(
        &flux0,
        FieldId::SensibleHeatFlux,
        Unit::WattPerSquareMeter,
        SignConvention::PositiveUpwardFlux,
        vec![1.0],
    ));
    let mut flux1 = flux0.clone();
    time(&mut flux1, 3600);
    assert!(
        CanonicalGpuField::upload(
            &ctx,
            FieldId::SensibleHeatFlux,
            &[&flux0, &flux1],
            &[None, None]
        )
        .is_err(),
        "#89 rejects instantaneous surface-flux policy; tracked by #120"
    );
    let mut rain0 = lower.clone();
    time(&mut rain0, 3600);
    let mut rain = surface_field(
        &rain0,
        FieldId::LargeScalePrecipitation,
        Unit::KilogramPerSquareMeter,
        SignConvention::NonNegative,
        vec![2.0],
    );
    rain.time.kind = TemporalKind::AccumulatedSinceReset;
    rain.time.interval_start_epoch_seconds = Some(0);
    rain.time.interval_end_epoch_seconds = Some(3600);
    rain.time.accumulation = Some(flexpart_gpu::meteorology::Accumulation {
        reset_epoch_seconds: 0,
    });
    rain0.fields.push(rain);
    let rain_source =
        CanonicalGpuField::upload(&ctx, FieldId::LargeScalePrecipitation, &[&rain0], &[None])
            .unwrap();
    assert!(rain_source
        .prepare_sample(&ctx, request(0.0, 0.0, instant(3600), None))
        .is_err());
    assert!(rain_source
        .prepare_sample(
            &ctx,
            request(
                0.0,
                0.0,
                MeteorologyTimeSelection::AccumulatedInterval {
                    start_epoch_seconds: 1,
                    end_epoch_seconds: 3600,
                    quantity: IntervalQuantity::Amount
                },
                None
            )
        )
        .is_err());
    let mut rain1 = rain0.clone();
    time(&mut rain1, 7200);
    rain1
        .fields
        .last_mut()
        .unwrap()
        .time
        .interval_end_epoch_seconds = Some(7200);
    rain1.fields.last_mut().unwrap().values[0] = 1.0;
    assert!(
        CanonicalGpuField::upload(
            &ctx,
            FieldId::LargeScalePrecipitation,
            &[&rain0, &rain1],
            &[None, None]
        )
        .is_err(),
        "same-run negative delta cannot become a reset"
    );
    let mut malformed_reset = rain0.clone();
    malformed_reset.fields.last_mut().unwrap().time.accumulation = None;
    assert!(CanonicalGpuField::upload(
        &ctx,
        FieldId::LargeScalePrecipitation,
        &[&malformed_reset],
        &[None]
    )
    .is_err());
    let other = pollster::block_on(GpuContext::new()).unwrap();
    assert!(source
        .prepare_sample(&other, request(0.0, 0.0, instant(1800), None))
        .is_err());
    let kernels = MeteorologyCompositionKernels::new(&other).unwrap();
    let mut plan = source
        .prepare_sample(&ctx, request(0.0, 0.0, instant(1800), None))
        .unwrap();
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    assert!(plan.encode(&ctx, &kernels, &mut encoder).is_err());
}

#[test]
fn test_meteorology_encode_surface_has_no_host_completion() {
    let composition = include_str!("../src/gpu/meteorology.rs");
    let body = composition;
    for forbidden in [
        ".submit(",
        ".poll(",
        "block_on(",
        "download_",
        "map_async(",
        "dispatch_",
    ] {
        assert!(
            !body.contains(forbidden),
            "ordinary encode must not contain {forbidden}"
        );
    }
    for (source, start, end) in [
        (
            include_str!("../src/gpu/horizontal.rs"),
            "pub fn encode_horizontal_samples(",
            "pub fn dispatch_horizontal_samples_and_wait(",
        ),
        (
            include_str!("../src/gpu/temporal.rs"),
            "pub fn encode_temporal_blend(",
            "pub fn dispatch_temporal_blend_and_wait(",
        ),
        (
            include_str!("../src/gpu/vertical.rs"),
            "pub fn encode_vertical_sample_with_kernel(",
            "pub fn dispatch_vertical_sample_with_kernel(",
        ),
        (
            include_str!("../src/gpu/accumulation.rs"),
            "pub fn encode_accumulated_intervals_gpu_with_kernel(",
            "pub fn dispatch_accumulated_intervals_gpu_with_kernel(",
        ),
    ] {
        let encode = source
            .split(start)
            .nth(1)
            .unwrap()
            .split(end)
            .next()
            .unwrap();
        for forbidden in [
            ".submit(",
            ".poll(",
            "block_on(",
            "download_buffer",
            "map_async(",
        ] {
            assert!(
                !encode.contains(forbidden),
                "called stage encode contains {forbidden}"
            );
        }
    }
}
