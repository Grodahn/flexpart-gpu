//! Issue #89: GPU temporal interpolation parity against the #71 oracle.
//!
//! The WGSL kernel `src/shaders/temporal_interpolation.wgsl` executes the
//! elementwise blend on the device. Host validation reuses
//! `meteorology::temporal::resolve_temporal_bracket`, the single source of
//! truth shared with the #74 CPU candidate, so timestamp, endpoint and
//! fail-closed semantics are identical. Evidence uses the #91
//! `GpuCalculationEvidence` model plus temporal fields.

use flexpart_gpu::gpu::{
    build_temporal_gpu_report, create_temporal_bracket_buffers, create_temporal_output_buffer,
    create_temporal_uniform_buffer, dispatch_temporal_blend_and_wait, download_temporal_output,
    encode_temporal_blend, sample_field_gpu, ComparisonPolicy, GpuAdapterEvidence,
    GpuCalculationEvidence, GpuCalculationPath, GpuCandidateEvidence, GpuEvidenceError,
    GpuEvidenceSchema, GpuExecutionEvidence, GpuExecutionStatus, GpuTemporalError,
    PinnedOracleEvidence, TemporalInterpolationKernel, TEMPORAL_GPU_REPORT_SCHEMA_ID,
};
use flexpart_gpu::meteorology::{
    temporal::{
        resolve_temporal_bracket, OracleDts, OracleQuery, RequestedSampleTime, TemporalApplication,
        TemporalError,
    },
    Axis, Calendar, Field, FieldId, FieldTime, HorizontalGrid, HorizontalStaggering,
    LongitudeDomain, SchemaIdentity, SignConvention, Snapshot, StorageOrder, TemporalKind,
    VerticalCoordinate, VerticalCoordinateKind, VerticalOrdering, VerticalReference,
    VerticalStaggering,
};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

const ABS_TOL: f64 = 1.0e-6;
const REL_TOL: f64 = 1.0e-5;

fn tolerance(oracle: f64) -> f64 {
    ABS_TOL + REL_TOL * oracle.abs()
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    let diff = (actual - expected).abs();
    let allowed = tolerance(expected);
    assert!(
        diff <= allowed,
        "{what}: gpu {actual} != expected {expected} (diff {diff}, tolerance {allowed})"
    );
}

fn try_gpu_context() -> Option<flexpart_gpu::gpu::GpuContext> {
    match pollster::block_on(flexpart_gpu::gpu::GpuContext::new()) {
        Ok(ctx) => Some(ctx),
        Err(flexpart_gpu::gpu::GpuError::NoAdapter) => None,
        Err(err) => panic!("unexpected GPU init error: {err}"),
    }
}

fn grid() -> HorizontalGrid {
    HorizontalGrid {
        nx: 1,
        ny: 1,
        xlon0_deg: 6.0,
        ylat0_deg: 50.0,
        dx_deg: 0.25,
        dy_deg: 0.25,
        longitude_domain: LongitudeDomain::Minus180To180,
    }
}

fn vertical() -> VerticalCoordinate {
    VerticalCoordinate {
        kind: VerticalCoordinateKind::HybridSigmaPressure,
        reference: VerticalReference::ModelNative,
        ordering: VerticalOrdering::Increasing,
        level_values: vec![50000.0, 70000.0, 90000.0],
        interface_values: Some(vec![40000.0, 60000.0, 80000.0, 100000.0]),
        hybrid_a_interface_pa: Some(vec![1000.0, 2000.0, 3000.0, 0.0]),
        hybrid_b_interface: Some(vec![0.39, 0.58, 0.77, 1.0]),
        reference_surface_pressure_pa: Some(100000.0),
        surface_pressure_dependency: Some(FieldId::SurfacePressure),
    }
}

fn snapshot_with(field: Field) -> Snapshot {
    Snapshot {
        schema: SchemaIdentity::default(),
        horizontal_grid: grid(),
        vertical_coordinate: vertical(),
        fields: vec![field],
    }
}

fn field_time(calendar: Calendar, kind: TemporalKind, epoch_seconds: i64) -> FieldTime {
    FieldTime {
        calendar,
        kind,
        valid_time_epoch_seconds: epoch_seconds,
        interval_start_epoch_seconds: None,
        interval_end_epoch_seconds: None,
        accumulation: None,
    }
}

fn wind_snapshot(epoch_seconds: i64, value: f32) -> Snapshot {
    snapshot_with(Field {
        id: FieldId::WindU,
        shape: vec![1, 1],
        axis_order: vec![Axis::X, Axis::Y],
        storage_order: StorageOrder::XFastest,
        unit: flexpart_gpu::meteorology::Unit::MeterPerSecond,
        sign: SignConvention::PositiveEastward,
        horizontal_staggering: HorizontalStaggering::CellCenter,
        vertical_staggering: VerticalStaggering::NotApplicable,
        time: field_time(
            Calendar::Gregorian,
            TemporalKind::Instantaneous,
            epoch_seconds,
        ),
        values: vec![value],
    })
}

fn temperature_snapshot(epoch_seconds: i64, values: Vec<f32>) -> Snapshot {
    snapshot_with(Field {
        id: FieldId::Temperature,
        shape: vec![1, 1, 3],
        axis_order: vec![Axis::X, Axis::Y, Axis::Z],
        storage_order: StorageOrder::XFastest,
        unit: flexpart_gpu::meteorology::Unit::Kelvin,
        sign: SignConvention::SignedScalar,
        horizontal_staggering: HorizontalStaggering::CellCenter,
        vertical_staggering: VerticalStaggering::LevelCenter,
        time: field_time(
            Calendar::Gregorian,
            TemporalKind::Instantaneous,
            epoch_seconds,
        ),
        values,
    })
}

fn gregorian(epoch_seconds: i64) -> RequestedSampleTime {
    RequestedSampleTime::new(Calendar::Gregorian, epoch_seconds)
}

fn comparison_policy() -> ComparisonPolicy {
    ComparisonPolicy::new(ABS_TOL, REL_TOL).expect("test comparison policy must be valid")
}

fn candidate_revision() -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("read candidate Git revision");
    assert!(output.status.success(), "git rev-parse HEAD must succeed");
    String::from_utf8(output.stdout)
        .expect("Git revision must be UTF-8")
        .trim()
        .to_string()
}

fn pinned_oracle_queries() -> Vec<OracleQuery> {
    vec![
        OracleQuery {
            requested_time_epoch_seconds: 0,
            element_index: 0,
            oracle_value: 10.0,
            oracle_dts: Some(OracleDts {
                dt1_seconds: 0.0,
                dt2_seconds: 3600.0,
                inverse_span_per_second: 0.000_277_777_784_503_996_37,
            }),
            expected_application: Some(TemporalApplication::FirstEndpoint),
        },
        OracleQuery {
            requested_time_epoch_seconds: 1800,
            element_index: 0,
            oracle_value: 15.0,
            oracle_dts: Some(OracleDts {
                dt1_seconds: 1800.0,
                dt2_seconds: 1800.0,
                inverse_span_per_second: 0.000_277_777_784_503_996_37,
            }),
            expected_application: Some(TemporalApplication::LinearInterior),
        },
        OracleQuery {
            requested_time_epoch_seconds: 3600,
            element_index: 0,
            oracle_value: 20.0,
            oracle_dts: Some(OracleDts {
                dt1_seconds: 3600.0,
                dt2_seconds: 0.0,
                inverse_span_per_second: 0.000_277_777_784_503_996_37,
            }),
            expected_application: Some(TemporalApplication::LastEndpoint),
        },
    ]
}

fn assert_skipped_evidence_fails_closed() {
    let evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "temporal-gpu/no-adapter".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: "temporal-gpu".to_string(),
            revision: "0.1.0".to_string(),
            shader_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            input_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_string(),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Skipped,
            calculation_path: GpuCalculationPath::NotExecuted,
            adapter: None,
            failure: None,
            skip_reason: Some("adapter unavailable".to_string()),
        },
        oracle: None,
        comparison: flexpart_gpu::gpu::ComparisonEvidence::not_evaluated(),
    };
    evidence
        .validate()
        .expect("skip is recordable but never passing");
    assert_eq!(
        evidence.require_paired_pass(),
        Err(GpuEvidenceError::NotPassing)
    );
}

#[test]
fn test_gpu_first_endpoint() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = TemporalInterpolationKernel::new(&ctx).expect("temporal pipeline");
    let snapshots = [wind_snapshot(0, 10.0), wind_snapshot(3600, 20.0)];
    let refs: Vec<&Snapshot> = snapshots.iter().collect();

    let sample = pollster::block_on(sample_field_gpu(
        &ctx,
        FieldId::WindU,
        &refs,
        gregorian(0),
        &kernel,
    ))
    .expect("GPU first endpoint must succeed");
    assert_eq!(sample.application, TemporalApplication::FirstEndpoint);
    assert_eq!(sample.source_timestamps.lower_epoch_seconds, 0);
    assert_eq!(sample.source_timestamps.upper_epoch_seconds, 3600);
    assert_close(
        f64::from(sample.values[0]),
        10.0,
        "GPU first endpoint VALUE",
    );
    assert_close(sample.weights.dt1_seconds, 0.0, "GPU first endpoint dt1");
    assert_close(sample.weights.dt2_seconds, 3600.0, "GPU first endpoint dt2");
    assert!(!ctx.device_name().is_empty());
}

#[test]
fn test_gpu_last_endpoint() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = TemporalInterpolationKernel::new(&ctx).expect("temporal pipeline");
    let snapshots = [wind_snapshot(0, 10.0), wind_snapshot(3600, 20.0)];
    let refs: Vec<&Snapshot> = snapshots.iter().collect();

    let sample = pollster::block_on(sample_field_gpu(
        &ctx,
        FieldId::WindU,
        &refs,
        gregorian(3600),
        &kernel,
    ))
    .expect("GPU last endpoint must succeed");
    assert_eq!(sample.application, TemporalApplication::LastEndpoint);
    assert_close(f64::from(sample.values[0]), 20.0, "GPU last endpoint VALUE");
    assert_close(sample.weights.dt1_seconds, 3600.0, "GPU last endpoint dt1");
    assert_close(sample.weights.dt2_seconds, 0.0, "GPU last endpoint dt2");
}

#[test]
fn test_gpu_exact_interior_source_timestamp() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = TemporalInterpolationKernel::new(&ctx).expect("temporal pipeline");
    let snapshots = [
        temperature_snapshot(0, vec![270.0, 280.0, 290.0]),
        temperature_snapshot(1800, vec![272.5, 290.0, 307.5]),
        temperature_snapshot(3600, vec![275.0, 300.0, 325.0]),
    ];
    let refs: Vec<&Snapshot> = snapshots.iter().collect();

    let sample = pollster::block_on(sample_field_gpu(
        &ctx,
        FieldId::Temperature,
        &refs,
        gregorian(1800),
        &kernel,
    ))
    .expect("GPU exact interior must succeed");
    assert_eq!(
        sample.application,
        TemporalApplication::ExactSourceTimestamp
    );
    assert_eq!(sample.source_timestamps.lower_epoch_seconds, 0);
    assert_eq!(sample.source_timestamps.upper_epoch_seconds, 1800);
    assert_eq!(sample.values[0], 272.5);
    assert_eq!(sample.values[1], 290.0);
    let (w_lower, w_upper) = sample.bracket.weights.normalized();
    assert_close(w_lower, 0.0, "exact lower weight");
    assert_close(w_upper, 1.0, "exact upper weight");
}

#[test]
fn test_gpu_linear_interior_interpolation() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = TemporalInterpolationKernel::new(&ctx).expect("temporal pipeline");
    let snapshots = [wind_snapshot(0, 10.0), wind_snapshot(3600, 20.0)];
    let refs: Vec<&Snapshot> = snapshots.iter().collect();
    let sample = pollster::block_on(sample_field_gpu(
        &ctx,
        FieldId::WindU,
        &refs,
        gregorian(1800),
        &kernel,
    ))
    .expect("GPU linear interior must succeed");
    assert_eq!(sample.application, TemporalApplication::LinearInterior);
    assert_close(
        f64::from(sample.values[0]),
        15.0,
        "GPU linear interior VALUE",
    );
    let (w_lower, w_upper) = sample.bracket.weights.normalized();
    assert_close(w_lower, 0.5, "midpoint lower weight");
    assert_close(w_upper, 0.5, "midpoint upper weight");

    let synth = [
        temperature_snapshot(0, vec![270.0, 280.0, 290.0]),
        temperature_snapshot(1800, vec![272.5, 290.0, 307.5]),
        temperature_snapshot(3600, vec![275.0, 300.0, 325.0]),
    ];
    let synth_refs: Vec<&Snapshot> = synth.iter().collect();
    let mid = pollster::block_on(sample_field_gpu(
        &ctx,
        FieldId::Temperature,
        &synth_refs,
        gregorian(900),
        &kernel,
    ))
    .expect("GPU synthetic midpoint must succeed");
    assert_eq!(mid.application, TemporalApplication::LinearInterior);
    assert_eq!(mid.values[0], 271.25);
    assert_eq!(mid.values[1], 285.0);
    assert_eq!(mid.values[2], 298.75);
}

#[test]
fn test_gpu_non_finite_output_fails_closed() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = TemporalInterpolationKernel::new(&ctx).expect("temporal pipeline");
    let snapshots = [wind_snapshot(0, f32::MAX), wind_snapshot(3600, f32::MAX)];
    let refs: Vec<&Snapshot> = snapshots.iter().collect();

    let error = pollster::block_on(sample_field_gpu(
        &ctx,
        FieldId::WindU,
        &refs,
        gregorian(1800),
        &kernel,
    ))
    .expect_err("non-finite GPU output must fail closed");
    assert!(matches!(
        error,
        GpuTemporalError::NonFiniteOutputValue { element_index: 0 }
    ));
}

#[test]
fn test_gpu_invalid_duplicate_non_monotonic_fail_closed() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = TemporalInterpolationKernel::new(&ctx).expect("temporal pipeline");

    let duplicate = [wind_snapshot(0, 10.0), wind_snapshot(0, 20.0)];
    let dup_refs: Vec<&Snapshot> = duplicate.iter().collect();
    let dup_err = pollster::block_on(sample_field_gpu(
        &ctx,
        FieldId::WindU,
        &dup_refs,
        gregorian(0),
        &kernel,
    ))
    .expect_err("duplicate must fail closed");
    assert!(
        matches!(
            dup_err,
            GpuTemporalError::Temporal(TemporalError::NonMonotonicTimestamps {
                index: 1,
                previous: 0,
                current: 0
            })
        ),
        "unexpected duplicate error: {dup_err:?}"
    );

    let descending = [wind_snapshot(3600, 10.0), wind_snapshot(0, 20.0)];
    let desc_refs: Vec<&Snapshot> = descending.iter().collect();
    let desc_err = pollster::block_on(sample_field_gpu(
        &ctx,
        FieldId::WindU,
        &desc_refs,
        gregorian(0),
        &kernel,
    ))
    .expect_err("non-monotonic must fail closed");
    assert!(
        matches!(
            desc_err,
            GpuTemporalError::Temporal(TemporalError::NonMonotonicTimestamps {
                index: 1,
                previous: 3600,
                current: 0
            })
        ),
        "unexpected descending error: {desc_err:?}"
    );

    assert_eq!(
        resolve_temporal_bracket(FieldId::WindU, &dup_refs, gregorian(0)),
        Err(TemporalError::NonMonotonicTimestamps {
            index: 1,
            previous: 0,
            current: 0
        })
    );
}

#[test]
fn test_gpu_unsupported_extrapolation_fail_closed() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = TemporalInterpolationKernel::new(&ctx).expect("temporal pipeline");
    let snapshots = [wind_snapshot(0, 10.0), wind_snapshot(3600, 20.0)];
    let refs: Vec<&Snapshot> = snapshots.iter().collect();

    let before_err = pollster::block_on(sample_field_gpu(
        &ctx,
        FieldId::WindU,
        &refs,
        gregorian(-3600),
        &kernel,
    ))
    .expect_err("before-first must fail closed");
    assert!(
        matches!(
            before_err,
            GpuTemporalError::Temporal(TemporalError::BeforeFirstCoverage {
                requested: -3600,
                first: 0
            })
        ),
        "unexpected before-first error: {before_err:?}"
    );
    let after_err = pollster::block_on(sample_field_gpu(
        &ctx,
        FieldId::WindU,
        &refs,
        gregorian(7200),
        &kernel,
    ))
    .expect_err("after-last must fail closed");
    assert!(
        matches!(
            after_err,
            GpuTemporalError::Temporal(TemporalError::AfterLastCoverage {
                requested: 7200,
                last: 3600
            })
        ),
        "unexpected after-last error: {after_err:?}"
    );
}

#[test]
fn test_gpu_vs_oracle_parity() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = TemporalInterpolationKernel::new(&ctx).expect("temporal pipeline");
    let snapshots = [wind_snapshot(0, 10.0), wind_snapshot(3600, 20.0)];
    let refs: Vec<&Snapshot> = snapshots.iter().collect();

    let contract: Value =
        serde_json::from_str(include_str!("../fixtures/interpolation/contract-v1.json"))
            .expect("parse interpolation contract");
    let temporal = contract["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == "temporal-bilinear")
        .expect("temporal-bilinear case");
    let golden_queries = temporal["golden"]["queries"].as_array().expect("queries");
    assert_eq!(golden_queries.len(), 3);

    let queries = pinned_oracle_queries();

    let report = pollster::block_on(build_temporal_gpu_report(
        &ctx,
        "oracle-temporal-bilinear",
        &candidate_revision(),
        FieldId::WindU,
        &refs,
        comparison_policy(),
        &queries,
        &kernel,
    ))
    .expect("GPU oracle report must build");

    assert_eq!(
        report.schema.id, TEMPORAL_GPU_REPORT_SCHEMA_ID,
        "report schema must be temporal GPU evidence"
    );
    assert_eq!(
        report.status,
        flexpart_gpu::meteorology::temporal::Verdict::Pass
    );
    assert_eq!(report.rows.len(), 3);
    for (row, golden) in report.rows.iter().zip(golden_queries) {
        let golden_value = golden["VALUE"][0].as_f64().expect("golden value");
        assert_close(
            f64::from(row.gpu_value),
            golden_value,
            "GPU vs oracle VALUE",
        );
        assert_close(
            f64::from(row.oracle_value),
            golden_value,
            "oracle row vs golden",
        );
        assert_eq!(
            row.row_verdict,
            flexpart_gpu::meteorology::temporal::Verdict::Pass
        );
        row.gpu_evidence
            .validate()
            .expect("GPU row evidence must validate");
        row.gpu_evidence
            .require_paired_pass()
            .expect("GPU row must prove paired pass");
        assert_eq!(
            row.gpu_evidence.execution.calculation_path,
            GpuCalculationPath::WgslDevice
        );
        assert!(row.gpu_evidence.execution.adapter.is_some());
    }
    report.validate().expect("GPU report must validate");
    report
        .require_paired_pass()
        .expect("GPU report must prove paired pass");

    let encoded = serde_json::to_string_pretty(&report).expect("serialize GPU report");
    let out_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("temporal-gpu-evidence.json");
    std::fs::create_dir_all(out_path.parent().expect("evidence path has parent"))
        .expect("create GPU evidence directory");
    std::fs::write(&out_path, &encoded).expect("write GPU evidence");
    let value: Value = serde_json::from_str(&encoded).expect("evidence must be valid JSON");
    assert_eq!(value["status"].as_str(), Some("pass"));
    let row = value["rows"]
        .as_array()
        .expect("rows")
        .first()
        .expect("row");
    for key in [
        "field_id",
        "element_index",
        "requested_time_epoch_seconds",
        "source_timestamps",
        "application",
        "weights",
        "gpu_value",
        "oracle_value",
        "comparison_policy",
        "absolute_difference",
        "value_verdict",
        "weights_verdict",
        "application_verdict",
        "row_verdict",
        "gpu_evidence",
    ] {
        assert!(row.get(key).is_some(), "GPU row must record {key}");
    }
    let gpu_evidence = row.get("gpu_evidence").expect("gpu_evidence");
    for key in [
        "schema",
        "case_id",
        "candidate",
        "execution",
        "oracle",
        "comparison",
    ] {
        assert!(
            gpu_evidence.get(key).is_some(),
            "GPU evidence must record {key}"
        );
    }
    assert_eq!(
        gpu_evidence["execution"]["calculation_path"].as_str(),
        Some("wgsl_device")
    );
    assert_eq!(
        gpu_evidence["candidate"]["revision"].as_str(),
        Some(candidate_revision().as_str())
    );
}

#[test]
fn test_gpu_evidence_fails_closed_on_contradiction() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = TemporalInterpolationKernel::new(&ctx).expect("temporal pipeline");
    let snapshots = [wind_snapshot(0, 10.0), wind_snapshot(3600, 20.0)];
    let refs: Vec<&Snapshot> = snapshots.iter().collect();
    let queries = pinned_oracle_queries();
    let mut report = pollster::block_on(build_temporal_gpu_report(
        &ctx,
        "oracle-temporal-bilinear",
        &candidate_revision(),
        FieldId::WindU,
        &refs,
        comparison_policy(),
        &queries,
        &kernel,
    ))
    .expect("report must build");

    let mut fabricated_queries = pinned_oracle_queries();
    fabricated_queries[1].oracle_value = 15.5;
    let fabricated = pollster::block_on(build_temporal_gpu_report(
        &ctx,
        "oracle-temporal-bilinear",
        &candidate_revision(),
        FieldId::WindU,
        &refs,
        comparison_policy(),
        &fabricated_queries,
        &kernel,
    ));
    assert!(matches!(
        fabricated,
        Err(GpuTemporalError::OracleContract { .. })
    ));

    let invalid_revision = pollster::block_on(build_temporal_gpu_report(
        &ctx,
        "oracle-temporal-bilinear",
        "0.1.0",
        FieldId::WindU,
        &refs,
        comparison_policy(),
        &queries,
        &kernel,
    ));
    assert!(matches!(
        invalid_revision,
        Err(GpuTemporalError::InvalidCandidateRevision { .. })
    ));

    let mut cpu_replacement = report.rows[0].gpu_evidence.clone();
    cpu_replacement.execution.calculation_path = GpuCalculationPath::CpuReplacement;
    assert!(cpu_replacement.validate().is_err());

    let mut contradictory_comparison = report.rows[0].clone();
    contradictory_comparison
        .gpu_evidence
        .comparison
        .max_absolute_error = Some(0.25);
    assert!(contradictory_comparison.validate().is_err());

    let original_revision = report.rows[1].gpu_evidence.candidate.revision.clone();
    report.rows[1].gpu_evidence.candidate.revision =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
    assert!(report.validate().is_err());
    report.rows[1].gpu_evidence.candidate.revision = original_revision;

    let skipped = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "temporal-gpu/skipped".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: "temporal-gpu".to_string(),
            revision: "0.1.0".to_string(),
            shader_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            input_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_string(),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Skipped,
            calculation_path: GpuCalculationPath::NotExecuted,
            adapter: None,
            failure: None,
            skip_reason: Some("adapter unavailable".to_string()),
        },
        oracle: Some(PinnedOracleEvidence {
            implementation_id: "FLEXPART-11.1".to_string(),
            revision: "rev".to_string(),
            executable_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            output_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_string(),
        }),
        comparison: flexpart_gpu::gpu::ComparisonEvidence::not_evaluated(),
    };
    assert_eq!(
        skipped.require_paired_pass(),
        Err(GpuEvidenceError::NotPassing)
    );

    report.rows[0].row_verdict = flexpart_gpu::meteorology::temporal::Verdict::Fail;
    assert!(report.validate().is_err());

    let mut missing_adapter = report.rows[0].gpu_evidence.clone();
    let mut honest = report.rows[0].clone();
    honest.row_verdict = flexpart_gpu::meteorology::temporal::Verdict::Pass;
    missing_adapter.execution.adapter = None;
    honest.gpu_evidence = missing_adapter;
    assert!(honest.validate().is_err());

    assert!(ComparisonPolicy::new(f64::NAN, 0.0).is_err());
}

#[test]
fn test_encode_composition_boundary_hides_nothing() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = TemporalInterpolationKernel::new(&ctx).expect("temporal pipeline");
    let snapshots = [wind_snapshot(0, 10.0), wind_snapshot(3600, 20.0)];
    let refs: Vec<&Snapshot> = snapshots.iter().collect();

    let bracket_mid = resolve_temporal_bracket(FieldId::WindU, &refs, gregorian(1800))
        .expect("bracket must resolve");
    let bracket_end = resolve_temporal_bracket(FieldId::WindU, &refs, gregorian(3600))
        .expect("bracket must resolve");

    let lower = [10.0_f32];
    let upper = [20.0_f32];
    let bracket_buffers =
        create_temporal_bracket_buffers(&ctx, &lower, &upper, 0, 3600).expect("bracket H2D");
    let output_mid = create_temporal_output_buffer(&ctx, 1).expect("output alloc");
    let output_end = create_temporal_output_buffer(&ctx, 1).expect("output alloc");
    let uniforms_mid = create_temporal_uniform_buffer(&ctx, &bracket_mid).expect("uniforms");
    let uniforms_end = create_temporal_uniform_buffer(&ctx, &bracket_end).expect("uniforms");

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("temporal_composition_test"),
        });
    encode_temporal_blend(
        &ctx,
        &bracket_buffers,
        &output_mid,
        &uniforms_mid,
        &bracket_mid,
        &kernel,
        &mut encoder,
    )
    .expect("first encode must succeed");
    encode_temporal_blend(
        &ctx,
        &bracket_buffers,
        &output_end,
        &uniforms_mid,
        &bracket_end,
        &kernel,
        &mut encoder,
    )
    .expect_err("uniforms from another bracket must fail closed");
    encode_temporal_blend(
        &ctx,
        &bracket_buffers,
        &output_end,
        &uniforms_end,
        &bracket_end,
        &kernel,
        &mut encoder,
    )
    .expect("second valid encode must succeed");
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);

    let values_mid =
        pollster::block_on(download_temporal_output(&ctx, &output_mid)).expect("D2H mid");
    let values_end =
        pollster::block_on(download_temporal_output(&ctx, &output_end)).expect("D2H end");
    assert_close(f64::from(values_mid[0]), 15.0, "composed mid VALUE");
    assert_close(f64::from(values_end[0]), 20.0, "composed end VALUE");

    let output_conv = create_temporal_output_buffer(&ctx, 1).expect("output alloc");
    dispatch_temporal_blend_and_wait(
        &ctx,
        &bracket_buffers,
        &output_conv,
        &uniforms_mid,
        &bracket_mid,
        &kernel,
    )
    .expect("dispatch convenience must succeed");
    let values_conv =
        pollster::block_on(download_temporal_output(&ctx, &output_conv)).expect("D2H conv");
    assert_close(f64::from(values_conv[0]), 15.0, "dispatch VALUE");

    let adapter = GpuAdapterEvidence::from_context(&ctx);
    adapter
        .validate()
        .expect("adapter provenance must validate");
}
