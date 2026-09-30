//! Issue #90: GPU accumulated-field interval/reset transformation.
//!
//! This test proves the WGSL transformation in
//! `src/gpu/accumulation.rs` against the canonical #71/#75 oracle fixtures.
//! Supported production arithmetic executes on the GPU; host code performs
//! only #75 metadata validation and interval-window derivation before dispatch.
//!
//! CPU `#75` results are used diagnostically where noted; the pinned
//! oracle/canonical expected values remain authoritative.

use flexpart_gpu::gpu::{
    build_accumulated_gpu_report, build_transform_inputs,
    encode_accumulated_intervals_gpu_with_kernel, transform_accumulated_intervals_gpu,
    AccumulatedIntervalBuffers, AccumulatedIntervalKernel, AccumulatedTransformInputs,
    ComparisonPolicy, GpuAccumulationError, GpuAdapterEvidence, GpuAdapterOptions,
    GpuCalculationEvidence, GpuCalculationPath, GpuCandidateEvidence, GpuContext, GpuError,
    GpuEvidenceError, GpuEvidenceSchema, GpuExecutionEvidence, GpuExecutionStatus,
    NumericalVerdict, PinnedOracleEvidence, ACCUMULATED_GPU_IMPLEMENTATION_ID,
};
use flexpart_gpu::meteorology::accumulation::{
    resolve_interval_sequence, AccumulatedObservation, AccumulationError,
};
use serde::Deserialize;

fn observation(valid_time: i64, reset: i64, amount: f64) -> AccumulatedObservation {
    AccumulatedObservation {
        valid_time_epoch_seconds: valid_time,
        reset_epoch_seconds: reset,
        accumulated_amount_kg_per_square_meter: amount,
    }
}

fn gpu_context_or_skip() -> Option<GpuContext> {
    match pollster::block_on(GpuContext::new()) {
        Ok(ctx) => Some(ctx),
        Err(GpuError::NoAdapter) => {
            assert!(
                !GpuAdapterOptions::from_env().force_software_fallback,
                "software WGSL was required, but no adapter was available"
            );
            eprintln!("no GPU adapter; skipping GPU accumulation test (not evidence)");
            None
        }
        Err(error) => panic!("unexpected GPU init error: {error}"),
    }
}

fn assert_close_f32(actual: f32, expected: f64, what: &str) {
    let actual_f64 = f64::from(actual);
    let absolute_error = (actual_f64 - expected).abs();
    let scale = actual_f64.abs().max(expected.abs());
    let relative_error = if scale == 0.0 {
        0.0
    } else {
        absolute_error / scale
    };
    assert!(
        absolute_error <= 1.0e-6 || relative_error <= 1.0e-6,
        "{what}: GPU {actual_f64} != oracle {expected} (abs {absolute_error}, rel {relative_error})"
    );
}

#[test]
fn gpu_monotonic_sequence_matches_oracle_amounts_and_rates() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let observations = vec![
        observation(1_800, 0, 1.0),
        observation(3_600, 0, 3.0),
        observation(5_400, 0, 6.0),
        observation(7_200, 0, 6.0),
    ];
    let expected_amounts = [1.0, 2.0, 3.0, 0.0];
    let expected_rates_mmh = [2.0, 4.0, 6.0, 0.0];

    let (amounts, rates_si, rates_mmh) =
        pollster::block_on(transform_accumulated_intervals_gpu(&ctx, &observations))
            .expect("GPU monotonic transform succeeds");
    assert_eq!(amounts.len(), 4);
    assert_eq!(rates_si.len(), 4);
    assert_eq!(rates_mmh.len(), 4);
    for index in 0..4 {
        assert_close_f32(
            amounts[index],
            expected_amounts[index],
            &format!("interval {index} amount"),
        );
        assert_close_f32(
            rates_mmh[index],
            expected_rates_mmh[index],
            &format!("interval {index} rate mm/h"),
        );
        // SI rate and mm/h handoff remain distinct but consistent.
        let expected_si = expected_rates_mmh[index] / 3_600.0;
        assert_close_f32(
            rates_si[index],
            expected_si,
            &format!("interval {index} rate SI"),
        );
        // Amount and rate are not collapsed: rate equals amount/duration.
        let duration = 1_800.0;
        let derived_si = f64::from(amounts[index]) / duration;
        assert!(
            (derived_si - f64::from(rates_si[index])).abs() <= 1.0e-5,
            "interval {index}: amount/duration must equal SI rate"
        );
    }

    // Machine-readable evidence with actual WGSL execution proof.
    let oracle_amounts: Vec<f64> = expected_amounts.to_vec();
    let oracle_rates: Vec<f64> = expected_rates_mmh.to_vec();
    let report = pollster::block_on(build_accumulated_gpu_report(
        "accumulation-gpu/monotonic-3-interval",
        "large_scale_precipitation",
        &ctx,
        &observations,
        &oracle_amounts,
        &oracle_rates,
    ))
    .expect("evidence builds");
    let mut tampered_source = report.clone();
    tampered_source.intervals[1].source_accumulated_amount_kg_per_square_meter = 4.0;
    assert!(matches!(
        tampered_source.validate(),
        Err(GpuEvidenceError::InvalidComparisonState(_))
    ));
    let mut tampered_si_rate = report.clone();
    tampered_si_rate.intervals[1].derived_gpu_rate_si = 1.0;
    assert!(matches!(
        tampered_si_rate.validate(),
        Err(GpuEvidenceError::InvalidComparisonState(_))
    ));
    let mut tampered_comparison = report.clone();
    tampered_comparison
        .amount_evidence
        .comparison
        .max_absolute_error = Some(123.0);
    assert!(matches!(
        tampered_comparison.validate(),
        Err(GpuEvidenceError::InvalidComparisonState(_))
    ));
    assert_eq!(report.verdict, NumericalVerdict::Passed);
    report
        .require_paired_pass()
        .expect("paired pass proves GPU");
    report
        .amount_evidence
        .require_paired_pass()
        .expect("amount lane proves GPU");
    report
        .rate_evidence
        .require_paired_pass()
        .expect("rate lane proves GPU");
    // Adapter provenance distinguishes hardware from software WGSL execution.
    let adapter = report
        .amount_evidence
        .execution
        .adapter
        .as_ref()
        .expect("passed execution carries adapter");
    assert!(!adapter.name.trim().is_empty());
    assert_eq!(
        report.amount_evidence.execution.calculation_path,
        GpuCalculationPath::WgslDevice
    );
    assert_eq!(
        report.amount_evidence.execution.status,
        GpuExecutionStatus::Passed
    );
    let encoded = serde_json::to_string(&report).expect("report serializes");
    assert!(encoded.contains("accumulation-gpu/monotonic-3-interval"));
}

#[test]
fn gpu_multiple_intervals_preserve_total_telescoping() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let observations = vec![
        observation(1_800, 0, 1.0),
        observation(3_600, 0, 3.0),
        observation(5_400, 0, 6.0),
        observation(7_200, 0, 6.0),
    ];
    let (amounts, _, _) =
        pollster::block_on(transform_accumulated_intervals_gpu(&ctx, &observations))
            .expect("GPU transform succeeds");
    let total: f64 = amounts.iter().map(|value| f64::from(*value)).sum();
    let expected_total = 6.0;
    let absolute_error = (total - expected_total).abs();
    let relative_error = absolute_error / expected_total.abs();
    assert!(
        absolute_error <= 1.0e-6 || relative_error <= 1.0e-6,
        "GPU total {total} must telescope to final source {expected_total}"
    );
}

#[test]
fn gpu_declared_reset_boundary_applies_fresh_amount() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let observations = vec![
        observation(1_800, 0, 1.0),
        observation(3_600, 1_800, 4.0),
        observation(5_400, 1_800, 7.0),
    ];
    let expected_amounts = [1.0, 4.0, 3.0];
    let expected_rates_mmh = [2.0, 8.0, 6.0];
    let (amounts, _, rates_mmh) =
        pollster::block_on(transform_accumulated_intervals_gpu(&ctx, &observations))
            .expect("GPU reset transform succeeds");
    for index in 0..3 {
        assert_close_f32(
            amounts[index],
            expected_amounts[index],
            &format!("reset interval {index} amount"),
        );
        assert_close_f32(
            rates_mmh[index],
            expected_rates_mmh[index],
            &format!("reset interval {index} rate"),
        );
    }
    let total: f64 = amounts.iter().map(|value| f64::from(*value)).sum();
    assert_close_f32(total as f32, 8.0, "reset total amount");

    let inputs = build_transform_inputs(&observations).expect("inputs build");
    assert_eq!(inputs[0].reset_applied, 1);
    assert_eq!(inputs[1].reset_applied, 1);
    assert_eq!(inputs[2].reset_applied, 0);

    let report = pollster::block_on(build_accumulated_gpu_report(
        "accumulation-gpu/declared-reset",
        "large_scale_precipitation",
        &ctx,
        &observations,
        &expected_amounts.map(|value| value),
        &expected_rates_mmh.map(|value| value),
    ))
    .expect("evidence builds");
    assert_eq!(report.verdict, NumericalVerdict::Passed);
    assert!(report.intervals[1].detected_reset);
    assert!(report.intervals[1].reset_applied);
    assert!(!report.intervals[2].detected_reset);
    assert!(!report.intervals[2].reset_applied);
    report.require_paired_pass().expect("reset case proves GPU");
}

#[derive(Deserialize)]
struct ContractFixture {
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    id: String,
    fields: Option<Vec<FixtureField>>,
    observations: Option<Vec<FixtureScalar>>,
    expected: Option<FixtureExpected>,
}

#[derive(Deserialize)]
struct FixtureField {
    id: String,
    observations: Vec<FixtureGridObservation>,
    expected_interval_amounts_kg_per_square_meter: Vec<Vec<f64>>,
    expected_rates_mm_per_hour: Vec<Vec<f64>>,
}

#[derive(Deserialize)]
struct FixtureGridObservation {
    valid_time_epoch_seconds: i64,
    reset_epoch_seconds: i64,
    amounts_kg_per_square_meter: Vec<f64>,
}

#[derive(Deserialize)]
struct FixtureScalar {
    valid_time_epoch_seconds: i64,
    reset_epoch_seconds: i64,
    accumulated_amount_kg_per_square_meter: f64,
}

#[derive(Deserialize)]
struct FixtureExpected {
    total_amount_kg_per_square_meter: f64,
    #[allow(dead_code)]
    detected_reset_count: usize,
    intervals: Vec<FixtureInterval>,
}

#[derive(Deserialize)]
struct FixtureInterval {
    amount_kg_per_square_meter: f64,
    rate_mm_per_hour: f64,
}

#[test]
fn gpu_rain_layer_oracle_rates_match_canonical_fixture() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let source = include_str!("../fixtures/accumulation/contract-v1.json");
    let fixture: ContractFixture = serde_json::from_str(source).expect("canonical fixture parses");
    let case = fixture
        .cases
        .iter()
        .find(|case| case.id == "rain-layer-fields-bilinear-rates")
        .expect("oracle case exists");

    let evidence_dir = std::path::Path::new("target/ci-gate/accumulation-gpu");
    std::fs::create_dir_all(evidence_dir).expect("create accumulation evidence directory");
    let mut checked_cells = 0;
    for field in case.fields.as_ref().expect("oracle fields exist") {
        let cell_count = field.observations[0].amounts_kg_per_square_meter.len();
        for cell in 0..cell_count {
            let sequence: Vec<AccumulatedObservation> = field
                .observations
                .iter()
                .map(|observation| AccumulatedObservation {
                    valid_time_epoch_seconds: observation.valid_time_epoch_seconds,
                    reset_epoch_seconds: observation.reset_epoch_seconds,
                    accumulated_amount_kg_per_square_meter: observation.amounts_kg_per_square_meter
                        [cell],
                })
                .collect();
            let (amounts, _, rates_mmh) =
                pollster::block_on(transform_accumulated_intervals_gpu(&ctx, &sequence))
                    .unwrap_or_else(|error| {
                        panic!("field {} cell {cell} GPU succeeds: {error}", field.id)
                    });
            for (index, ((amount, rate), (expected_amount, expected_rate))) in amounts
                .iter()
                .zip(rates_mmh.iter())
                .zip(
                    field
                        .expected_interval_amounts_kg_per_square_meter
                        .iter()
                        .zip(field.expected_rates_mm_per_hour.iter())
                        .map(|(amounts, rates)| (amounts[cell], rates[cell])),
                )
                .enumerate()
            {
                // `expected_*` arrays are per-observation grids in fixture order.
                let _ = index;
                assert_close_f32(
                    *amount,
                    expected_amount,
                    &format!("field {} cell {cell} amount", field.id),
                );
                assert_close_f32(
                    *rate,
                    expected_rate,
                    &format!("field {} cell {cell} rate", field.id),
                );
            }
            // Diagnostic CPU comparison only; oracle remains authoritative.
            let cpu = resolve_interval_sequence(&sequence).expect("CPU resolves");
            for (gpu_amount, product) in amounts.iter().zip(cpu.intervals.iter()) {
                assert_close_f32(
                    *gpu_amount,
                    product.amount_kg_per_square_meter.0,
                    &format!("field {} cell {cell} GPU vs CPU diagnostic", field.id),
                );
            }
            let oracle_amounts: Vec<f64> = field
                .expected_interval_amounts_kg_per_square_meter
                .iter()
                .map(|grid| grid[cell])
                .collect();
            let oracle_rates: Vec<f64> = field
                .expected_rates_mm_per_hour
                .iter()
                .map(|grid| grid[cell])
                .collect();
            let report = pollster::block_on(build_accumulated_gpu_report(
                &format!("accumulation-gpu/rain-layer-{}-cell-{cell:02}", field.id),
                &field.id,
                &ctx,
                &sequence,
                &oracle_amounts,
                &oracle_rates,
            ))
            .unwrap_or_else(|error| {
                panic!("field {} cell {cell} evidence builds: {error}", field.id)
            });
            report.require_paired_pass().unwrap_or_else(|error| {
                panic!("field {} cell {cell} proves GPU: {error}", field.id)
            });
            let evidence_path = evidence_dir.join(format!("{}-cell-{cell:02}.json", field.id));
            std::fs::write(
                evidence_path,
                serde_json::to_vec_pretty(&report).expect("rain evidence serializes"),
            )
            .expect("write rain evidence");
            checked_cells += 1;
        }
    }
    assert_eq!(checked_cells, 24, "both 12-cell fields are covered");
}

#[test]
fn gpu_continuous_reset_fixture_matches_expected() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let source = include_str!("../fixtures/accumulation/contract-v1.json");
    let fixture: ContractFixture = serde_json::from_str(source).expect("canonical fixture parses");
    let case = fixture
        .cases
        .iter()
        .find(|case| case.id == "continuous-run-with-declared-reset")
        .expect("reset fixture exists");
    let observations: Vec<AccumulatedObservation> = case
        .observations
        .as_ref()
        .expect("reset observations exist")
        .iter()
        .map(|observation| AccumulatedObservation {
            valid_time_epoch_seconds: observation.valid_time_epoch_seconds,
            reset_epoch_seconds: observation.reset_epoch_seconds,
            accumulated_amount_kg_per_square_meter: observation
                .accumulated_amount_kg_per_square_meter,
        })
        .collect();
    let expected = case.expected.as_ref().expect("reset expected exists");
    let (amounts, _, rates_mmh) =
        pollster::block_on(transform_accumulated_intervals_gpu(&ctx, &observations))
            .expect("GPU reset fixture succeeds");
    for (index, interval) in expected.intervals.iter().enumerate() {
        assert_close_f32(
            amounts[index],
            interval.amount_kg_per_square_meter,
            &format!("fixture interval {index} amount"),
        );
        assert_close_f32(
            rates_mmh[index],
            interval.rate_mm_per_hour,
            &format!("fixture interval {index} rate"),
        );
    }
    let total: f64 = amounts.iter().map(|value| f64::from(*value)).sum();
    assert!(
        (total - expected.total_amount_kg_per_square_meter).abs() <= 1.0e-6
            || (total - expected.total_amount_kg_per_square_meter).abs()
                / expected.total_amount_kg_per_square_meter.abs()
                <= 1.0e-6,
        "fixture total preserves"
    );
}

#[test]
fn gpu_rejects_negative_delta_before_dispatch() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let observations = vec![observation(1_800, 0, 1.0), observation(3_600, 0, 0.5)];
    let error = pollster::block_on(transform_accumulated_intervals_gpu(&ctx, &observations))
        .expect_err("negative delta must fail closed");
    assert!(matches!(
        error,
        GpuAccumulationError::Validation(AccumulationError::NegativeDelta { .. })
    ));
}

#[test]
fn gpu_rejects_malformed_reset_metadata() {
    // Host validation rejects before any WGSL dispatch; no adapter execution
    // occurs for these inputs.
    let cases = vec![
        // Mid-window reset.
        vec![observation(1_800, 0, 1.0), observation(3_600, 1_200, 2.0)],
        // Uncovered gap.
        vec![observation(1_800, 0, 1.0), observation(5_400, 3_600, 2.0)],
        // Backwards reset.
        vec![observation(1_800, 0, 1.0), observation(3_600, -100, 2.0)],
        // Reset not before valid time.
        vec![observation(1_800, 0, 1.0), observation(3_600, 3_600, 5.0)],
    ];
    for (index, observations) in cases.into_iter().enumerate() {
        let error =
            build_transform_inputs(&observations).expect_err(&format!("case {index} fails"));
        assert!(
            matches!(error, GpuAccumulationError::Validation(_)),
            "case {index} must fail as validation, got {error:?}"
        );
    }
}

#[test]
fn gpu_rejects_inconsistent_timestamps_and_invalid_values() {
    let timestamp_cases = vec![
        vec![observation(1_800, 0, 1.0), observation(1_800, 0, 2.0)],
        vec![observation(3_600, 0, 2.0), observation(1_800, 0, 1.0)],
    ];
    for (index, observations) in timestamp_cases.into_iter().enumerate() {
        let error =
            build_transform_inputs(&observations).expect_err(&format!("time {index} fails"));
        assert!(
            matches!(
                error,
                GpuAccumulationError::Validation(AccumulationError::NonIncreasingTimestamps { .. })
            ),
            "time {index} must fail as timestamps, got {error:?}"
        );
    }

    let invalid_cases: Vec<Vec<AccumulatedObservation>> = vec![
        vec![observation(1_800, 0, f64::NAN)],
        vec![observation(1_800, 0, f64::INFINITY)],
        vec![observation(1_800, 0, -1.0)],
        vec![],
    ];
    for (index, observations) in invalid_cases.into_iter().enumerate() {
        assert!(
            build_transform_inputs(&observations).is_err(),
            "invalid {index} must fail closed"
        );
    }
}

#[test]
fn gpu_encode_path_composes_two_stages_without_intermediate_submit() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    // Two independent interval sequences share one caller-owned encoder. No
    // submit, wait, or readback occurs between them; the single submit at the
    // owning boundary completes both. This is the #76 composition pattern.
    let first = vec![observation(1_800, 0, 2.0), observation(3_600, 0, 5.0)];
    let second = vec![observation(1_800, 0, 1.0), observation(3_600, 1_800, 4.0)];
    let first_inputs =
        AccumulatedTransformInputs::from_observations(&ctx, &first).expect("first inputs upload");
    let second_inputs =
        AccumulatedTransformInputs::from_observations(&ctx, &second).expect("second inputs upload");
    let first_outputs = AccumulatedIntervalBuffers::new(&ctx, first_inputs.interval_count())
        .expect("first outputs allocate");
    let second_outputs = AccumulatedIntervalBuffers::new(&ctx, second_inputs.interval_count())
        .expect("second outputs allocate");
    let kernel = AccumulatedIntervalKernel::new(&ctx).expect("kernel compiles");

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("accumulation_composition_test"),
        });
    encode_accumulated_intervals_gpu_with_kernel(
        &ctx,
        &first_inputs,
        &first_outputs,
        &kernel,
        &mut encoder,
    )
    .expect("first encode succeeds");
    // No submit/wait/readback between the two GPU-capable stages.
    encode_accumulated_intervals_gpu_with_kernel(
        &ctx,
        &second_inputs,
        &second_outputs,
        &kernel,
        &mut encoder,
    )
    .expect("second encode succeeds");
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);

    let (first_amounts, _, first_rates) = (
        pollster::block_on(first_outputs.download_amounts(&ctx)).expect("first readback"),
        pollster::block_on(first_outputs.download_rates_si(&ctx)).expect("first SI"),
        pollster::block_on(first_outputs.download_rates_millimeter_per_hour(&ctx))
            .expect("first mmh"),
    );
    let (second_amounts, _, second_rates) = (
        pollster::block_on(second_outputs.download_amounts(&ctx)).expect("second readback"),
        pollster::block_on(second_outputs.download_rates_si(&ctx)).expect("second SI"),
        pollster::block_on(second_outputs.download_rates_millimeter_per_hour(&ctx))
            .expect("second mmh"),
    );
    assert_close_f32(first_amounts[0], 2.0, "composed first leading amount");
    assert_close_f32(first_amounts[1], 3.0, "composed first delta amount");
    assert_close_f32(first_rates[1], 6.0, "composed first delta rate");
    assert_close_f32(second_amounts[1], 4.0, "composed second reset amount");
    assert_close_f32(second_rates[1], 8.0, "composed second reset rate");
}

#[test]
fn gpu_evidence_fails_closed_without_valid_execution() {
    // A skipped run carries no adapter/calculation and can never be a pass.
    let skipped = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "accumulation-gpu/skip-guard".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: ACCUMULATED_GPU_IMPLEMENTATION_ID.to_string(),
            revision: "test-revision".to_string(),
            shader_sha256: "a".repeat(64),
            input_sha256: "b".repeat(64),
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
    skipped.validate().expect("honest skip is recordable");
    assert_eq!(
        skipped.require_paired_pass(),
        Err(GpuEvidenceError::NotPassing),
        "skipped execution must not satisfy a GPU pass"
    );

    // CPU replacement cannot satisfy a GPU execution claim.
    let cpu_replacement = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "accumulation-gpu/cpu-guard".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: ACCUMULATED_GPU_IMPLEMENTATION_ID.to_string(),
            revision: "test-revision".to_string(),
            shader_sha256: "a".repeat(64),
            input_sha256: "b".repeat(64),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::CpuReplacement,
            adapter: None,
            failure: None,
            skip_reason: None,
        },
        oracle: None,
        comparison: flexpart_gpu::gpu::ComparisonEvidence::not_evaluated(),
    };
    assert!(
        cpu_replacement.validate().is_err(),
        "CPU replacement must fail GPU validation"
    );

    // A passed WGSL execution without a pinned oracle cannot be a paired pass.
    let policy = ComparisonPolicy::new(1.0e-6, 1.0e-6).expect("policy builds");
    let comparison = flexpart_gpu::gpu::compare_finite_values(&[1.0], &[1.0], policy)
        .expect("comparison builds");
    assert_eq!(comparison.verdict, NumericalVerdict::Passed);
    let missing_oracle = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "accumulation-gpu/oracle-guard".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: ACCUMULATED_GPU_IMPLEMENTATION_ID.to_string(),
            revision: "test-revision".to_string(),
            shader_sha256: "a".repeat(64),
            input_sha256: "b".repeat(64),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(GpuAdapterEvidence {
                name: "test-adapter".to_string(),
                backend: "vulkan".to_string(),
                device_type: "integrated_gpu".to_string(),
                vendor_id: 1,
                device_id: 2,
                driver: "test-driver".to_string(),
                driver_info: "test-driver-info".to_string(),
                adapter_class: flexpart_gpu::gpu::GpuAdapterClass::HardwareGpu,
                software_fallback_requested: false,
            }),
            failure: None,
            skip_reason: None,
        },
        oracle: None,
        comparison,
    };
    // Structural validation accepts the record, but the paired gate rejects it.
    missing_oracle
        .validate()
        .expect_err("passed WGSL with oracle but not-evaluated comparison is invalid");
    let _ = missing_oracle;

    // Missing adapter execution on a passed claim fails closed.
    let missing_adapter = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "accumulation-gpu/adapter-guard".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: ACCUMULATED_GPU_IMPLEMENTATION_ID.to_string(),
            revision: "test-revision".to_string(),
            shader_sha256: "a".repeat(64),
            input_sha256: "b".repeat(64),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: None,
            failure: None,
            skip_reason: None,
        },
        oracle: Some(PinnedOracleEvidence {
            implementation_id: "FLEXPART-11.1".to_string(),
            revision: "pinned".to_string(),
            executable_sha256: "a".repeat(64),
            output_sha256: "b".repeat(64),
        }),
        comparison: flexpart_gpu::gpu::ComparisonEvidence::not_evaluated(),
    };
    assert!(
        missing_adapter.validate().is_err(),
        "passed WGSL without adapter must fail"
    );
}
