//! Issue #87: GPU horizontal interpolation parity against the #71 oracle.
//!
//! The WGSL kernel `src/shaders/horizontal_interpolation.wgsl` executes index
//! weight computation, gathering and blending on the
//! device. Host validation preserves cell indices and reuses `meteorology::horizontal` (the #72 contract)
//! so grid, staggering, domain and longitude semantics are identical. Evidence
//! uses the #91 `GpuCalculationEvidence` model plus horizontal fields. CPU #72
//! values appear diagnostically only; the pinned FLEXPART oracle remains
//! authoritative. Production composition uses `encode_horizontal_samples`
//! (caller-owned encoder, no submit/wait/readback/field allocation).

use flexpart_gpu::gpu::{
    build_horizontal_gpu_row, create_horizontal_output_buffer, create_horizontal_query_buffers,
    create_horizontal_uniform_buffer, dispatch_horizontal_samples_and_wait,
    download_horizontal_output, encode_horizontal_samples, horizontal_inputs_sha256,
    horizontal_shader_sha256, sample_horizontal_geographic_gpu, sample_horizontal_gpu,
    ComparisonPolicy, GpuAdapterEvidence, GpuCalculationEvidence, GpuCalculationPath,
    GpuCandidateEvidence, GpuEvidenceError, GpuEvidenceSchema, GpuExecutionEvidence,
    GpuExecutionStatus, GpuHorizontalError, HorizontalFieldBuffers, HorizontalGpuReport,
    HorizontalGpuRow, HorizontalInterpolationKernel, HORIZONTAL_GPU_CANDIDATE_DESCRIPTION,
    HORIZONTAL_GPU_IMPLEMENTATION_ID, HORIZONTAL_GPU_REPORT_SCHEMA_ID,
    HORIZONTAL_ORACLE_OUTPUT_SHA256_GEOGRAPHIC, HORIZONTAL_ORACLE_OUTPUT_SHA256_INTERIOR,
    HORIZONTAL_ORACLE_OUTPUT_SHA256_PERIODIC, HORIZONTAL_ORACLE_REVISION,
};
use flexpart_gpu::meteorology::{
    horizontal::{sample_horizontal, sample_horizontal_geographic},
    HorizontalGrid, HorizontalStaggering, LongitudeDomain, SchemaIdentity,
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
        Err(flexpart_gpu::gpu::GpuError::NoAdapter) => {
            assert_ne!(
                std::env::var("FLEXPART_GPU_SOFTWARE").as_deref(),
                Ok("1"),
                "required horizontal WGSL adapter missing"
            );
            None
        }
        Err(err) => panic!("unexpected GPU init error: {err}"),
    }
}

fn candidate_revision() -> String {
    if let Ok(revision) = std::env::var("FLEXPART_GPU_CANDIDATE_REVISION") {
        let trimmed = revision.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
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

/// Asymmetric oracle fixture field: `f(x,y) = 100 + 100*x + 10*y`, x-fastest.
///
/// Deliberately asymmetric in x vs y so swapped weights or row/column indexing
/// cannot accidentally pass.
fn oracle_field() -> Vec<f32> {
    vec![
        100.0, 200.0, 300.0, 400.0, 110.0, 210.0, 310.0, 410.0, 120.0, 220.0, 320.0, 420.0,
    ]
}

fn regional_grid() -> HorizontalGrid {
    HorizontalGrid {
        nx: 4,
        ny: 3,
        xlon0_deg: 0.0,
        ylat0_deg: 0.0,
        dx_deg: 1.0,
        dy_deg: 1.0,
        longitude_domain: LongitudeDomain::Minus180To180,
    }
}

fn periodic_grid() -> HorizontalGrid {
    HorizontalGrid {
        nx: 4,
        ny: 3,
        xlon0_deg: 0.0,
        ylat0_deg: 0.0,
        dx_deg: 90.0,
        dy_deg: 1.0,
        longitude_domain: LongitudeDomain::ZeroTo360,
    }
}

fn geographic_grid() -> HorizontalGrid {
    HorizontalGrid {
        nx: 4,
        ny: 3,
        xlon0_deg: -2.0,
        ylat0_deg: 48.0,
        dx_deg: 0.25,
        dy_deg: 0.25,
        longitude_domain: LongitudeDomain::Minus180To180,
    }
}

fn comparison_policy() -> ComparisonPolicy {
    ComparisonPolicy::new(ABS_TOL, REL_TOL).expect("test comparison policy must be valid")
}

fn assert_skipped_evidence_fails_closed() {
    let evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "horizontal-gpu/no-adapter".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: "horizontal-gpu".to_string(),
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
fn test_gpu_interior_matches_oracle() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = regional_grid();
    let field = oracle_field();
    let (values, diagnostics) = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(1.25, 0.5)],
        &kernel,
    ))
    .expect("GPU interior must succeed");
    assert_eq!(values.len(), 1);
    assert_close(f64::from(values[0]), 230.0, "GPU interior VALUE");
    assert_eq!(
        (
            diagnostics[0].ix,
            diagnostics[0].jy,
            diagnostics[0].ixp,
            diagnostics[0].jyp
        ),
        (1, 0, 2, 1)
    );
    assert!(!ctx.device_name().is_empty());
    // Adapter provenance must validate; execution proof is WGSL device.
    let adapter = GpuAdapterEvidence::from_context(&ctx);
    adapter
        .validate()
        .expect("adapter provenance must validate");
}

#[test]
fn test_gpu_exact_grid_point_selects_single_corner() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = regional_grid();
    let field = oracle_field();
    let (values, diagnostics) = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(2.0, 1.0)],
        &kernel,
    ))
    .expect("GPU exact grid point must succeed");
    assert_eq!(
        (
            diagnostics[0].ix,
            diagnostics[0].jy,
            diagnostics[0].ixp,
            diagnostics[0].jyp
        ),
        (2, 1, 3, 2)
    );
    assert_eq!(diagnostics[0].weights, [1.0, 0.0, 0.0, 0.0]);
    assert_close(f64::from(values[0]), 310.0, "GPU exact VALUE");
}

#[test]
fn test_gpu_edge_adjacent_and_corner() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = regional_grid();
    let field = oracle_field();
    // North-edge exact last row collapses jyp to jy.
    let (values, diagnostics) = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(2.9, 2.0)],
        &kernel,
    ))
    .expect("GPU north edge must succeed");
    assert_eq!(
        (
            diagnostics[0].ix,
            diagnostics[0].jy,
            diagnostics[0].ixp,
            diagnostics[0].jyp
        ),
        (2, 2, 3, 2)
    );
    assert_close(f64::from(values[0]), 410.0, "GPU north-edge VALUE");

    // Origin corner selects a single corner.
    let (corner, _) = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(0.0, 0.0)],
        &kernel,
    ))
    .expect("GPU origin corner must succeed");
    assert_close(f64::from(corner[0]), 100.0, "GPU origin VALUE");
}

#[test]
fn test_gpu_periodic_seam_uses_duplicate_column() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = periodic_grid();
    let field = oracle_field();
    let (values, diagnostics) = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(3.2, 1.5), (3.9, 0.5), (0.4, 2.0)],
        &kernel,
    ))
    .expect("GPU periodic seam must succeed");
    assert_eq!(
        (
            diagnostics[0].ix,
            diagnostics[0].jy,
            diagnostics[0].ixp,
            diagnostics[0].jyp
        ),
        (3, 1, 4, 2)
    );
    assert!(diagnostics[0].is_periodic_x);
    assert_close(f64::from(values[0]), 355.0, "GPU seam VALUE");
    assert_close(f64::from(values[1]), 135.0, "GPU near-seam VALUE");
    assert_close(f64::from(values[2]), 160.0, "GPU north-edge periodic VALUE");
}

#[test]
fn test_gpu_geographic_mapping_matches_grid_path() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = geographic_grid();
    let field = oracle_field();
    let (values, diagnostics) = pollster::block_on(sample_horizontal_geographic_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(-1.6875, 48.125), (-1.375, 48.375)],
        &kernel,
    ))
    .expect("GPU geographic must succeed");
    assert_close(f64::from(values[0]), 230.0, "GPU geographic VALUE 0");
    assert_close(f64::from(values[1]), 365.0, "GPU geographic VALUE 1");
    assert_eq!((diagnostics[0].ix, diagnostics[0].jy), (1, 0));
    assert_eq!((diagnostics[1].ix, diagnostics[1].jy), (2, 1));
}

#[test]
fn test_gpu_asymmetric_field_exposes_swapped_weights() {
    // f(x,y) = 100 + 100*x + 10*y is deliberately asymmetric: swapping x/y
    // weights changes the interior value far beyond tolerance.
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = regional_grid();
    let field = oracle_field();
    let (values, _) = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(1.25, 0.5)],
        &kernel,
    ))
    .expect("GPU asymmetric must succeed");
    let correct = f64::from(values[0]);
    assert_close(correct, 230.0, "GPU asymmetric VALUE");
    // Swapped-weight value would blend transposed corners: p1*f00 + p3*f10 +
    // p2*f01 + p4*f11 = 0.375*100 + 0.375*200 + 0.125*110 + 0.125*210 = 152.5.
    // That differs by 77.5, far outside tolerance, so a swap cannot pass.
    let swapped = 0.375 * 100.0 + 0.375 * 200.0 + 0.125 * 110.0 + 0.125 * 210.0;
    assert!(
        (correct - swapped).abs() > tolerance(230.0),
        "asymmetric field must expose swapped x/y weights"
    );
}

#[test]
fn test_gpu_out_of_domain_fails_closed() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = regional_grid();
    let field = oracle_field();
    for (xt, yt) in [
        (-0.1, 0.5),
        (0.5, -0.1),
        (3.5, 1.0),
        (1.0, 2.5),
        (4.0, 1.0),
        (f64::NAN, 0.5),
        (f64::INFINITY, 0.5),
    ] {
        let error = pollster::block_on(sample_horizontal_gpu(
            &ctx,
            &grid,
            &field,
            HorizontalStaggering::CellCenter,
            &[(xt, yt)],
            &kernel,
        ))
        .expect_err("out-of-domain must fail closed");
        assert!(
            matches!(
                error,
                GpuHorizontalError::Horizontal(
                    flexpart_gpu::meteorology::horizontal::HorizontalError::OutOfDomain { .. }
                        | flexpart_gpu::meteorology::horizontal::HorizontalError::ImpossibleCoordinate { .. }
                )
            ),
            "unexpected error for ({xt}, {yt}): {error:?}"
        );
    }
    // Periodic duplicate endpoint is explicitly out of the supported domain.
    let periodic = periodic_grid();
    let periodic_error = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &periodic,
        &field,
        HorizontalStaggering::CellCenter,
        &[(4.0, 1.0)],
        &kernel,
    ))
    .expect_err("periodic endpoint must fail closed");
    assert!(matches!(
        periodic_error,
        GpuHorizontalError::Horizontal(
            flexpart_gpu::meteorology::horizontal::HorizontalError::OutOfDomain { .. }
        )
    ));
}

#[test]
fn test_gpu_unsupported_staggering_malformed_and_shape_fail_closed() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = regional_grid();
    let field = oracle_field();

    for staggering in [HorizontalStaggering::XFace, HorizontalStaggering::YFace] {
        let error = pollster::block_on(sample_horizontal_gpu(
            &ctx,
            &grid,
            &field,
            staggering,
            &[(1.0, 1.0)],
            &kernel,
        ))
        .expect_err("face staggering must fail closed");
        assert!(matches!(
            error,
            GpuHorizontalError::Horizontal(
                flexpart_gpu::meteorology::horizontal::HorizontalError::UnsupportedStaggering { .. }
            )
        ));
    }

    // Zero-sized dimensions fail closed without dispatch.
    let mut empty_grid = grid.clone();
    empty_grid.nx = 0;
    let empty_error = match HorizontalFieldBuffers::from_grid_and_values(
        &ctx,
        &empty_grid,
        &[],
        HorizontalStaggering::CellCenter,
    ) {
        Err(error) => error,
        Ok(_) => panic!("zero nx must fail closed"),
    };
    assert!(matches!(
        empty_error,
        GpuHorizontalError::Horizontal(
            flexpart_gpu::meteorology::horizontal::HorizontalError::MalformedDimensions { .. }
        )
    ));

    // Shape mismatch fails closed.
    let shape_error = match HorizontalFieldBuffers::from_grid_and_values(
        &ctx,
        &grid,
        &[0.0; 11],
        HorizontalStaggering::CellCenter,
    ) {
        Err(error) => error,
        Ok(_) => panic!("shape mismatch must fail closed"),
    };
    assert!(matches!(
        shape_error,
        GpuHorizontalError::Horizontal(
            flexpart_gpu::meteorology::horizontal::HorizontalError::ShapeMismatch { .. }
        )
    ));

    // Inconsistent metadata fails closed.
    let mut bad_spacing = grid.clone();
    bad_spacing.dx_deg = 0.0;
    let meta_error = match HorizontalFieldBuffers::from_grid_and_values(
        &ctx,
        &bad_spacing,
        &field,
        HorizontalStaggering::CellCenter,
    ) {
        Err(error) => error,
        Ok(_) => panic!("bad spacing must fail closed"),
    };
    assert!(matches!(
        meta_error,
        GpuHorizontalError::Horizontal(
            flexpart_gpu::meteorology::horizontal::HorizontalError::InconsistentGridMetadata { .. }
        )
    ));

    // Empty query sequence cannot prove parity.
    let empty_queries = match create_horizontal_query_buffers(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[],
    ) {
        Err(error) => error,
        Ok(_) => panic!("empty queries must fail closed"),
    };
    assert!(matches!(empty_queries, GpuHorizontalError::EmptyQueries));

    // Oversized output count fails closed without allocation.
    let overflow = match create_horizontal_output_buffer(&ctx, u32::MAX as usize + 1) {
        Err(error) => error,
        Ok(_) => panic!("oversized count must fail closed"),
    };
    assert!(matches!(overflow, GpuHorizontalError::ValueTooLarge { .. }));

    // Counts that fit u32 still must obey the selected device's storage limits.
    assert!(matches!(
        create_horizontal_output_buffer(&ctx, u32::MAX as usize),
        Err(GpuHorizontalError::BufferLimit { .. })
    ));

    // Zero-size output fails closed.
    let zero = match create_horizontal_output_buffer(&ctx, 0) {
        Err(error) => error,
        Ok(_) => panic!("zero must fail"),
    };
    assert!(matches!(zero, GpuHorizontalError::EmptyQueries));
}

#[test]
fn test_gpu_non_finite_field_and_longitude_convention_fail_closed() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = regional_grid();
    // Non-finite corner fails closed (sampled value would be non-finite).
    let mut bad_field = oracle_field();
    bad_field[0] = f32::NAN;
    let nan_error = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &bad_field,
        HorizontalStaggering::CellCenter,
        &[(0.0, 0.0)],
        &kernel,
    ))
    .expect_err("NaN corner must fail closed");
    assert!(matches!(
        nan_error,
        GpuHorizontalError::Horizontal(
            flexpart_gpu::meteorology::horizontal::HorizontalError::NonFiniteValue { .. }
                | flexpart_gpu::meteorology::horizontal::HorizontalError::NonFiniteResult
        )
    ));

    // Longitude outside the declared convention fails even when the raw numeric
    // mapping would land inside a periodic grid.
    let periodic_zero_origin = HorizontalGrid {
        nx: 4,
        ny: 3,
        xlon0_deg: 0.0,
        ylat0_deg: 0.0,
        dx_deg: 90.0,
        dy_deg: 1.0,
        longitude_domain: LongitudeDomain::Minus180To180,
    };
    let field = vec![1.0_f32; 12];
    let lon_error = pollster::block_on(sample_horizontal_geographic_gpu(
        &ctx,
        &periodic_zero_origin,
        &field,
        HorizontalStaggering::CellCenter,
        &[(270.0, 1.0)],
        &kernel,
    ))
    .expect_err("longitude outside convention must fail closed");
    assert!(matches!(
        lon_error,
        GpuHorizontalError::Horizontal(
            flexpart_gpu::meteorology::horizontal::HorizontalError::LongitudeOutsideConvention { .. }
        )
    ));

    // Impossible latitude fails distinctly from out-of-domain.
    let impossible = pollster::block_on(sample_horizontal_geographic_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(0.0, 95.0)],
        &kernel,
    ))
    .expect_err("impossible latitude must fail closed");
    assert!(matches!(
        impossible,
        GpuHorizontalError::Horizontal(
            flexpart_gpu::meteorology::horizontal::HorizontalError::ImpossibleCoordinate { .. }
        )
    ));
}

#[test]
fn test_gpu_vs_oracle_parity_with_evidence() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");

    // Authoritative oracle identity: pinned revision plus per-case output digests.
    let contract: Value =
        serde_json::from_str(include_str!("../fixtures/interpolation/contract-v1.json"))
            .expect("parse interpolation contract");
    let provenance: Value = serde_json::from_str(include_str!(
        "../fixtures/interpolation/contract-v1.provenance.json"
    ))
    .expect("parse interpolation provenance");
    assert_eq!(
        provenance["pinned_commit"].as_str(),
        Some(HORIZONTAL_ORACLE_REVISION)
    );
    assert_eq!(
        provenance["cases"]["horizontal-interior"].as_str(),
        Some(HORIZONTAL_ORACLE_OUTPUT_SHA256_INTERIOR)
    );
    assert_eq!(
        provenance["cases"]["horizontal-periodic-wrap"].as_str(),
        Some(HORIZONTAL_ORACLE_OUTPUT_SHA256_PERIODIC)
    );
    assert_eq!(
        provenance["cases"]["horizontal-geographic-interior"].as_str(),
        Some(HORIZONTAL_ORACLE_OUTPUT_SHA256_GEOGRAPHIC)
    );
    let cases = contract["cases"].as_array().expect("cases");

    let policy = comparison_policy();
    let revision = candidate_revision();
    assert_eq!(revision.len(), 40, "candidate revision must be a Git SHA");

    let mut rows: Vec<HorizontalGpuRow> = Vec::new();

    // Grid-index cases: horizontal-interior (2 queries) and periodic (3 queries).
    for case_id in ["horizontal-interior", "horizontal-periodic-wrap"] {
        let case = cases
            .iter()
            .find(|entry| entry["id"] == case_id)
            .unwrap_or_else(|| panic!("missing oracle case {case_id}"));
        let input = case["input"].as_array().expect("input");
        let input: Vec<String> = input
            .iter()
            .map(|value| value.as_str().expect("input line").to_string())
            .collect();
        let dims: Vec<String> = input[1].split_whitespace().map(str::to_string).collect();
        let nx: usize = dims[0].parse().expect("nx");
        let ny: usize = dims[1].parse().expect("ny");
        let grid_tokens: Vec<String> = input[2].split_whitespace().map(str::to_string).collect();
        let longitude_domain = if case_id == "horizontal-periodic-wrap" {
            LongitudeDomain::ZeroTo360
        } else {
            LongitudeDomain::Minus180To180
        };
        let grid = HorizontalGrid {
            nx,
            ny,
            xlon0_deg: grid_tokens[0].parse().expect("xlon0"),
            ylat0_deg: grid_tokens[1].parse().expect("ylat0"),
            dx_deg: grid_tokens[2].parse().expect("dx"),
            dy_deg: grid_tokens[3].parse().expect("dy"),
            longitude_domain,
        };
        let field_start = 4;
        let field: Vec<f32> = input[field_start..field_start + nx * ny]
            .iter()
            .map(|line| line.trim().parse::<f32>().expect("field value"))
            .collect();
        let nquery: usize = input[field_start + nx * ny].parse().expect("nquery");
        let golden_queries = case["golden"]["queries"].as_array().expect("queries");
        assert_eq!(golden_queries.len(), nquery);

        let mut coordinates = Vec::with_capacity(nquery);
        for index in 0..nquery {
            let tokens: Vec<String> = input[field_start + nx * ny + 1 + index]
                .split_whitespace()
                .map(str::to_string)
                .collect();
            coordinates.push((
                tokens[0].parse::<f64>().expect("xt"),
                tokens[1].parse::<f64>().expect("yt"),
            ));
        }
        let (gpu_values, diagnostics) = pollster::block_on(sample_horizontal_gpu(
            &ctx,
            &grid,
            &field,
            HorizontalStaggering::CellCenter,
            &coordinates,
            &kernel,
        ))
        .expect("GPU oracle queries must succeed");

        // Device queries actually dispatched (f32 conversion of validated f64).
        let device_queries: Vec<_> = diagnostics
            .iter()
            .map(|sample| {
                flexpart_gpu::gpu::HorizontalSampleQuery::from_geometry(sample)
                    .expect("device query")
            })
            .collect();

        for (index, golden) in golden_queries.iter().enumerate() {
            let oracle_indices = golden["INDICES"].as_array().expect("INDICES");
            let oracle_indices = [
                oracle_indices[0].as_u64().expect("ix") as usize,
                oracle_indices[1].as_u64().expect("jy") as usize,
                oracle_indices[2].as_u64().expect("ixp") as usize,
                oracle_indices[3].as_u64().expect("jyp") as usize,
            ];
            let oracle_weights = golden["WEIGHTS"].as_array().expect("WEIGHTS");
            let oracle_weights = [
                oracle_weights[0].as_f64().expect("p1"),
                oracle_weights[1].as_f64().expect("p2"),
                oracle_weights[2].as_f64().expect("p3"),
                oracle_weights[3].as_f64().expect("p4"),
            ];
            #[allow(clippy::cast_possible_truncation)]
            let oracle_value = golden["VALUE"][0].as_f64().expect("VALUE") as f32;
            let cpu_value = sample_horizontal(
                &grid,
                &field,
                HorizontalStaggering::CellCenter,
                coordinates[index].0,
                coordinates[index].1,
            )
            .expect("CPU diagnostic must succeed")
            .value;
            assert_close(
                f64::from(gpu_values[index]),
                f64::from(oracle_value),
                &format!("GPU vs oracle {case_id} query {index}"),
            );
            rows.push(
                build_horizontal_gpu_row(
                    &ctx,
                    case_id,
                    &grid,
                    &diagnostics[index],
                    &device_queries[index],
                    gpu_values[index],
                    oracle_indices,
                    oracle_weights,
                    oracle_value,
                    cpu_value,
                    policy,
                    &field,
                    &revision,
                )
                .expect("GPU row must build"),
            );
        }
    }

    // Geographic case: lon/lat inputs mapped by coordtrafo before sampling.
    {
        let case_id = "horizontal-geographic-interior";
        let case = cases
            .iter()
            .find(|entry| entry["id"] == case_id)
            .expect("missing geographic oracle case");
        let input = case["input"].as_array().expect("input");
        let input: Vec<String> = input
            .iter()
            .map(|value| value.as_str().expect("input line").to_string())
            .collect();
        let grid = geographic_grid();
        let field = oracle_field();
        let golden_queries = case["golden"]["queries"].as_array().expect("queries");
        assert_eq!(golden_queries.len(), 2);
        let mut lonlat = Vec::new();
        for index in 0..2 {
            let tokens: Vec<String> = input[4 + 12 + 1 + index]
                .split_whitespace()
                .map(str::to_string)
                .collect();
            lonlat.push((
                tokens[0].parse::<f64>().expect("lon"),
                tokens[1].parse::<f64>().expect("lat"),
            ));
        }
        let (gpu_values, diagnostics) = pollster::block_on(sample_horizontal_geographic_gpu(
            &ctx,
            &grid,
            &field,
            HorizontalStaggering::CellCenter,
            &lonlat,
            &kernel,
        ))
        .expect("GPU geographic oracle must succeed");
        let device_queries: Vec<_> = diagnostics
            .iter()
            .map(|sample| {
                flexpart_gpu::gpu::HorizontalSampleQuery::from_geometry(sample)
                    .expect("device query")
            })
            .collect();
        for (index, golden) in golden_queries.iter().enumerate() {
            let oracle_indices = golden["INDICES"].as_array().expect("INDICES");
            let oracle_indices = [
                oracle_indices[0].as_u64().expect("ix") as usize,
                oracle_indices[1].as_u64().expect("jy") as usize,
                oracle_indices[2].as_u64().expect("ixp") as usize,
                oracle_indices[3].as_u64().expect("jyp") as usize,
            ];
            let oracle_weights = golden["WEIGHTS"].as_array().expect("WEIGHTS");
            let oracle_weights = [
                oracle_weights[0].as_f64().expect("p1"),
                oracle_weights[1].as_f64().expect("p2"),
                oracle_weights[2].as_f64().expect("p3"),
                oracle_weights[3].as_f64().expect("p4"),
            ];
            #[allow(clippy::cast_possible_truncation)]
            let oracle_value = golden["VALUE"][0].as_f64().expect("VALUE") as f32;
            let cpu_value = sample_horizontal_geographic(
                &grid,
                &field,
                HorizontalStaggering::CellCenter,
                lonlat[index].0,
                lonlat[index].1,
            )
            .expect("CPU geographic diagnostic must succeed")
            .value;
            assert_close(
                f64::from(gpu_values[index]),
                f64::from(oracle_value),
                &format!("GPU vs oracle geographic query {index}"),
            );
            // Geographic mapping must converge with the direct grid path.
            assert_close(
                diagnostics[index].xt,
                golden["XY"][0].as_f64().expect("xt"),
                &format!("coordtrafo xt query {index}"),
            );
            rows.push(
                build_horizontal_gpu_row(
                    &ctx,
                    case_id,
                    &grid,
                    &diagnostics[index],
                    &device_queries[index],
                    gpu_values[index],
                    oracle_indices,
                    oracle_weights,
                    oracle_value,
                    cpu_value,
                    policy,
                    &field,
                    &revision,
                )
                .expect("geographic GPU row must build"),
            );
        }
    }

    assert_eq!(
        rows.len(),
        7,
        "oracle coverage must span all pinned queries"
    );
    // Every row must prove WGSL execution with adapter provenance.
    for row in &rows {
        assert!(row.row_verdict, "row {} must pass", row.case_id);
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
        assert_eq!(
            row.gpu_evidence.candidate.implementation_id,
            HORIZONTAL_GPU_IMPLEMENTATION_ID
        );
        assert_eq!(
            row.gpu_evidence.candidate.shader_sha256,
            horizontal_shader_sha256()
        );
        assert_eq!(
            row.gpu_evidence.oracle.as_ref().expect("oracle").revision,
            HORIZONTAL_ORACLE_REVISION
        );
    }

    let all_pass = rows.iter().all(|row| row.row_verdict);
    let report = HorizontalGpuReport {
        schema: SchemaIdentity {
            id: HORIZONTAL_GPU_REPORT_SCHEMA_ID.to_string(),
            version: 1,
        },
        scenario_id: "horizontal-oracle-coverage".to_string(),
        candidate: HORIZONTAL_GPU_CANDIDATE_DESCRIPTION.to_string(),
        comparison_policy: policy,
        rows,
        status: all_pass,
    };
    report.validate().expect("GPU report must validate");
    report
        .require_paired_pass()
        .expect("GPU report must prove paired pass");

    let mut incomplete = report.clone();
    incomplete.rows.pop();
    assert!(incomplete.require_paired_pass().is_err());
    let mut duplicate = report.clone();
    duplicate.rows[0] = duplicate.rows[1].clone();
    assert!(duplicate.require_paired_pass().is_err());

    let encoded = serde_json::to_string_pretty(&report).expect("serialize GPU report");
    let out_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("horizontal-gpu-evidence.json");
    std::fs::create_dir_all(out_path.parent().expect("evidence path has parent"))
        .expect("create GPU evidence directory");
    std::fs::write(&out_path, &encoded).expect("write GPU evidence");
    let value: Value = serde_json::from_str(&encoded).expect("evidence must be valid JSON");
    assert_eq!(
        value["scenario_id"].as_str(),
        Some("horizontal-oracle-coverage")
    );
    assert_eq!(value["status"], true);
    let rows_value = value["rows"].as_array().expect("rows");
    assert_eq!(rows_value.len(), 7);
    for row in rows_value {
        for key in [
            "case_id",
            "grid",
            "sample_xt",
            "sample_yt",
            "candidate_indices",
            "oracle_indices",
            "candidate_weights",
            "oracle_weights",
            "gpu_value",
            "oracle_value",
            "comparison_policy",
            "absolute_difference",
            "value_verdict",
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
            Some(revision.as_str())
        );
    }
}

#[test]
fn test_gpu_evidence_fails_closed_on_contradiction() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = regional_grid();
    let field = oracle_field();
    let policy = comparison_policy();
    let revision = candidate_revision();
    let (gpu_values, diagnostics) = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(1.25, 0.5)],
        &kernel,
    ))
    .expect("GPU sample must succeed");
    let device_query = flexpart_gpu::gpu::HorizontalSampleQuery::new(1.25, 0.5);
    let row = build_horizontal_gpu_row(
        &ctx,
        "horizontal-interior",
        &grid,
        &diagnostics[0],
        &device_query,
        gpu_values[0],
        [1, 0, 2, 1],
        [0.375, 0.125, 0.375, 0.125],
        230.0,
        230.0,
        policy,
        &field,
        &revision,
    )
    .expect("row must build");
    row.validate().expect("honest row must validate");

    // CPU replacement cannot prove a GPU pass.
    let mut cpu_replacement = row.gpu_evidence.clone();
    cpu_replacement.execution.calculation_path = GpuCalculationPath::CpuReplacement;
    assert!(cpu_replacement.validate().is_err());

    // Skipped execution can never prove a pass.
    let skipped = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "horizontal-gpu/skipped".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: "horizontal-gpu".to_string(),
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
    assert_eq!(
        skipped.require_paired_pass(),
        Err(GpuEvidenceError::NotPassing)
    );

    // Mismatched shader identity fails closed.
    let mut bad_shader = row.clone();
    bad_shader.gpu_evidence.candidate.shader_sha256 =
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
    assert!(bad_shader.validate().is_err());

    // Mismatched input identity fails closed (input hash binds grid/field/queries).
    let other_field = vec![1.0_f32; 12];
    let bad_input_hash =
        horizontal_inputs_sha256(&grid, &other_field, std::slice::from_ref(&device_query))
            .expect("hash other inputs");
    let mut bad_input = row.clone();
    bad_input.gpu_evidence.candidate.input_sha256 = bad_input_hash;
    assert!(bad_input.validate().is_err());
    let mut bad_indices = row.clone();
    bad_indices.oracle_indices[0] = 0;
    assert!(bad_indices.validate().is_err());
    let mut bad_weights = row.clone();
    bad_weights.candidate_weights[0] = f64::NAN;
    assert!(bad_weights.validate().is_err());
    let mut bad_difference = row.clone();
    bad_difference.absolute_difference = f64::NAN;
    assert!(bad_difference.validate().is_err());
    let mut bad_coordinate = row.clone();
    bad_coordinate.device_xt = 2.0;
    assert!(bad_coordinate.validate().is_err());

    // Contradictory verdict fails closed.
    let mut contradictory = row.clone();
    contradictory.row_verdict = !contradictory.row_verdict;
    assert!(contradictory.validate().is_err());

    // Contradictory comparison fails closed.
    let mut bad_comparison = row.clone();
    bad_comparison.gpu_evidence.comparison.max_absolute_error = Some(0.25);
    assert!(bad_comparison.validate().is_err());

    // Missing adapter fails closed for a passing claim.
    let mut missing_adapter = row.clone();
    missing_adapter.gpu_evidence.execution.adapter = None;
    assert!(missing_adapter.validate().is_err());

    // Missing oracle fails the paired pass (successful execution with oracle
    // requires a numerical verdict; without oracle the comparison cannot be
    // paired).
    let mut missing_oracle = row.clone();
    missing_oracle.gpu_evidence.oracle = None;
    assert!(missing_oracle.validate().is_err());

    // Invalid candidate revision fails at construction.
    let invalid_revision = build_horizontal_gpu_row(
        &ctx,
        "horizontal-interior",
        &grid,
        &diagnostics[0],
        &device_query,
        gpu_values[0],
        [1, 0, 2, 1],
        [0.375, 0.125, 0.375, 0.125],
        230.0,
        230.0,
        policy,
        &field,
        "0.1.0",
    );
    assert!(matches!(
        invalid_revision,
        Err(GpuHorizontalError::InvalidCandidateRevision { .. })
    ));

    // Unknown oracle case fails closed.
    let unknown_case = build_horizontal_gpu_row(
        &ctx,
        "unknown-case",
        &grid,
        &diagnostics[0],
        &device_query,
        gpu_values[0],
        [1, 0, 2, 1],
        [0.375, 0.125, 0.375, 0.125],
        230.0,
        230.0,
        policy,
        &field,
        &revision,
    );
    assert!(matches!(
        unknown_case,
        Err(GpuHorizontalError::OracleContract { .. })
    ));
}

#[test]
fn test_encode_composition_boundary_hides_nothing() {
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = regional_grid();
    let field = oracle_field();

    // Persistent field resource reused across two dispatches: no per-query field
    // allocation is needed for the second batch.
    let fields = HorizontalFieldBuffers::from_grid_and_values(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
    )
    .expect("field H2D");

    let (queries_mid, _) = create_horizontal_query_buffers(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(1.25, 0.5)],
    )
    .expect("queries H2D");
    let (queries_end, _) = create_horizontal_query_buffers(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(2.0, 1.0)],
    )
    .expect("queries H2D");
    let output_mid = create_horizontal_output_buffer(&ctx, 1).expect("output alloc");
    let output_end = create_horizontal_output_buffer(&ctx, 1).expect("output alloc");
    let uniforms_mid = create_horizontal_uniform_buffer(&ctx, &grid, 1).expect("uniforms");
    let uniforms_end = create_horizontal_uniform_buffer(&ctx, &grid, 1).expect("uniforms");

    // Compose two sampling batches into one caller-owned encoder with a single
    // submission: no hidden submit/wait/readback between device stages.
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("horizontal_composition_test"),
        });
    encode_horizontal_samples(
        &ctx,
        &fields,
        &queries_mid,
        &output_mid,
        &uniforms_mid,
        &kernel,
        &mut encoder,
    )
    .expect("first encode must succeed");
    // Mismatched uniforms fail closed instead of silently sampling the wrong grid.
    let other_grid = periodic_grid();
    let bad_uniforms =
        create_horizontal_uniform_buffer(&ctx, &other_grid, 1).expect("other uniforms");
    encode_horizontal_samples(
        &ctx,
        &fields,
        &queries_end,
        &output_end,
        &bad_uniforms,
        &kernel,
        &mut encoder,
    )
    .expect_err("grid/uniform mismatch must fail closed");
    encode_horizontal_samples(
        &ctx,
        &fields,
        &queries_end,
        &output_end,
        &uniforms_end,
        &kernel,
        &mut encoder,
    )
    .expect("second valid encode must succeed");
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);

    let values_mid =
        pollster::block_on(download_horizontal_output(&ctx, &output_mid)).expect("D2H mid");
    let values_end =
        pollster::block_on(download_horizontal_output(&ctx, &output_end)).expect("D2H end");
    assert_close(f64::from(values_mid[0]), 230.0, "composed mid VALUE");
    assert_close(f64::from(values_end[0]), 310.0, "composed end VALUE");

    // Convenience dispatch path agrees with composed encode path.
    let output_conv = create_horizontal_output_buffer(&ctx, 1).expect("output alloc");
    dispatch_horizontal_samples_and_wait(
        &ctx,
        &fields,
        &queries_mid,
        &output_conv,
        &uniforms_mid,
        &kernel,
    )
    .expect("dispatch convenience must succeed");
    let values_conv =
        pollster::block_on(download_horizontal_output(&ctx, &output_conv)).expect("D2H conv");
    assert_close(f64::from(values_conv[0]), 230.0, "dispatch VALUE");

    let adapter = GpuAdapterEvidence::from_context(&ctx);
    adapter
        .validate()
        .expect("adapter provenance must validate");
}

#[test]
fn test_dispatch_rounding_with_many_queries() {
    // Dispatch rounding: more queries than one workgroup exercises the 1-D
    // spill path shared with other kernels.
    let Some(ctx) = try_gpu_context() else {
        assert_skipped_evidence_fails_closed();
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("horizontal pipeline");
    let grid = regional_grid();
    // Linear field f(x,y) = 7 + 2x + 3y is reproduced exactly by bilinear
    // interpolation; check many points to exercise dispatch rounding.
    let field: Vec<f32> = (0..3)
        .flat_map(|y| (0..4).map(move |x| 7.0 + 2.0 * x as f32 + 3.0 * y as f32))
        .collect();
    let mut coordinates = Vec::new();
    for index in 0..300 {
        let xt = f64::from(index % 30) * 0.1;
        let yt = f64::from((index / 30) % 20) * 0.1;
        coordinates.push((xt, yt));
    }
    let (gpu_values, _) = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &coordinates,
        &kernel,
    ))
    .expect("many-query dispatch must succeed");
    assert_eq!(gpu_values.len(), coordinates.len());
    for (value, (xt, yt)) in gpu_values.iter().zip(coordinates.iter()) {
        assert_close(
            f64::from(*value),
            7.0 + 2.0 * xt + 3.0 * yt,
            "many-query linear VALUE",
        );
    }
}

#[test]
fn test_gpu_f64_cell_survives_coordinate_rounding() {
    let Some(ctx) = try_gpu_context() else {
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("pipeline");
    let grid = periodic_grid();
    let field = oracle_field();
    // Whole-coordinate f32 conversion rounds this supported point to nx.
    let xt = 4.0 - 1.0e-8;
    let (values, _) = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(xt, 0.5)],
        &kernel,
    ))
    .expect("valid seam query");
    let expected = sample_horizontal(&grid, &field, HorizontalStaggering::CellCenter, xt, 0.5)
        .expect("CPU diagnostic");
    assert_close(
        f64::from(values[0]),
        f64::from(expected.value),
        "seam rounding",
    );

    let grid = regional_grid();
    let mut field = oracle_field();
    // Cell selection must stay below x=1, so column 2 is never sampled.
    field[2] = f32::NAN;
    field[6] = f32::NAN;
    let xt = 1.0 - 1.0e-8;
    let (values, _) = pollster::block_on(sample_horizontal_gpu(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
        &[(xt, 0.5)],
        &kernel,
    ))
    .expect("valid cell query");
    assert_close(f64::from(values[0]), 205.0, "cell rounding");
}

#[test]
fn test_encode_rejects_queries_validated_for_other_inputs() {
    let Some(ctx) = try_gpu_context() else {
        return;
    };
    let kernel = HorizontalInterpolationKernel::new(&ctx).expect("pipeline");
    let grid = regional_grid();
    let field = oracle_field();
    let fields = HorizontalFieldBuffers::from_grid_and_values(
        &ctx,
        &grid,
        &field,
        HorizontalStaggering::CellCenter,
    )
    .expect("field upload");
    let output = create_horizontal_output_buffer(&ctx, 1).expect("output");
    let uniforms = create_horizontal_uniform_buffer(&ctx, &grid, 1).expect("uniforms");
    let mut other_grid = grid.clone();
    other_grid.dx_deg = 0.5;
    for (query_grid, query_field) in [(&other_grid, field.clone()), (&grid, vec![1.0; 12])] {
        let (queries, _) = create_horizontal_query_buffers(
            &ctx,
            query_grid,
            &query_field,
            HorizontalStaggering::CellCenter,
            &[(1.25, 0.5)],
        )
        .expect("queries");
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        assert!(matches!(
            encode_horizontal_samples(
                &ctx,
                &fields,
                &queries,
                &output,
                &uniforms,
                &kernel,
                &mut encoder
            ),
            Err(GpuHorizontalError::MismatchedGrid)
        ));
    }
}
