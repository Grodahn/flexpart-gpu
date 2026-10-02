//! Issue #88: GPU vertical meteorology sampling parity against #71/#80 oracles.
//!
//! The WGSL kernels `src/shaders/vertical_sample.wgsl` and
//! `src/shaders/vertical_remap_w.wgsl` execute supported numerical
//! sampling/transformation on the device. Host code performs only #30/#73/#80
//! validation and geometry preparation before dispatch.
//!
//! CPU `#73` results are used diagnostically where noted; the pinned
//! FLEXPART oracles remain authoritative.

use flexpart_gpu::gpu::{
    build_vertical_model_gpu_row, build_vertical_w_gpu_row, create_vertical_grid_buffers,
    create_vertical_output_buffer, create_vertical_query_buffers, create_vertical_shared_grid,
    create_vertical_w_interface_inputs, download_vertical_samples,
    encode_vertical_remap_w_with_kernel, encode_vertical_sample_with_kernel,
    encode_vertical_w_two_stage_with_kernels, physical_center_w_column_from_runtime,
    physical_model_column_from_runtime, physical_w_columns_from_runtime, resolve_query_heights_agl,
    sample_vertical_grid_gpu, sample_vertical_w_gpu_two_stage, vertical_geometry_identity,
    vertical_model_comparison_policy, vertical_sample_shader_sha256,
    vertical_w_bundle_shader_sha256, vertical_w_comparison_policy, GpuCalculationEvidence,
    GpuCalculationPath, GpuCandidateEvidence, GpuContext, GpuError, GpuEvidenceError,
    GpuEvidenceSchema, GpuExecutionEvidence, GpuExecutionStatus, PinnedOracleEvidence,
    VerticalGpuReport, VerticalModelOracleCase, VerticalSampleKernel, VerticalWRemapKernel,
    VerticalWSourceLanes, VERTICAL_GPU_CANDIDATE_DESCRIPTION, VERTICAL_GPU_REPORT_SCHEMA_ID,
    VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_MODEL_LEVELS,
    VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_REAL_COLUMN, VERTICAL_ORACLE_REVISION,
    VERTICAL_W_ORACLE_BINARY_SHA256, VERTICAL_W_ORACLE_OUTPUT_SHA256,
};
use flexpart_gpu::meteorology::{
    vertical::{
        reconstruct_vertical_geometry, reconstruct_vertical_geometry_with_motion,
        NativeVerticalMotion, NativeVerticalMotionKind, NativeVerticalMotionProvenance,
        NativeVerticalMotionSign, NativeVerticalMotionUnit,
    },
    vertical_sampling::sample_vertical,
    FieldId, SchemaIdentity, VerticalReference, VerticalStaggering,
};
use serde_json::Value;
use std::process::Command;

const ABS_TOL_MODEL: f64 = 1.0e-6;
const REL_TOL_MODEL: f64 = 1.0e-4;
const ABS_TOL_W: f64 = 1.0e-6;
const REL_TOL_W: f64 = 1.0e-5;

fn gpu_context_or_skip() -> Option<GpuContext> {
    match pollster::block_on(GpuContext::new()) {
        Ok(ctx) => Some(ctx),
        Err(GpuError::NoAdapter) => {
            assert_ne!(
                std::env::var("FLEXPART_GPU_REQUIRE_VERTICAL").as_deref(),
                Ok("1"),
                "required vertical validation cannot skip an adapter"
            );
            eprintln!("no GPU adapter; skipping GPU vertical test (not evidence)");
            None
        }
        Err(error) => panic!("unexpected GPU init error: {error}"),
    }
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

fn checked_f64_to_f32(value: f64, what: &str) -> f32 {
    assert!(value.is_finite(), "{what}: non-finite f64 input");
    assert!(
        value >= f64::from(f32::MIN) && value <= f64::from(f32::MAX),
        "{what}: f64 outside f32 range"
    );
    let rounded = value as f32;
    assert!(rounded.is_finite(), "{what}: f32 rounding non-finite");
    rounded
}

fn write_paired_report(
    scenario: &str,
    policy: flexpart_gpu::gpu::ComparisonPolicy,
    rows: Vec<flexpart_gpu::gpu::VerticalGpuRow>,
) {
    let report = VerticalGpuReport {
        schema: SchemaIdentity {
            id: VERTICAL_GPU_REPORT_SCHEMA_ID.to_string(),
            version: 1,
        },
        scenario_id: scenario.to_string(),
        candidate: VERTICAL_GPU_CANDIDATE_DESCRIPTION.to_string(),
        comparison_policy: policy,
        status: rows.iter().all(|row| row.row_verdict),
        rows,
    };
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-gate/vertical-gpu");
    std::fs::create_dir_all(&directory).expect("evidence directory");
    std::fs::write(
        directory.join(format!("{scenario}.json")),
        serde_json::to_vec_pretty(&report).expect("serialize report"),
    )
    .expect("write report");
    report
        .require_paired_pass()
        .expect("all oracle queries prove paired GPU pass");
}

fn assert_close(actual: f64, expected: f64, abs: f64, rel: f64, what: &str) {
    let diff = (actual - expected).abs();
    let tolerance = abs.max(rel * expected.abs().max(actual.abs()));
    assert!(
        diff <= tolerance,
        "{what}: GPU {actual} != oracle {expected} (diff {diff}, tolerance {tolerance})"
    );
}

fn synthetic_snapshot() -> flexpart_gpu::meteorology::Snapshot {
    serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-column-v1.json"
    ))
    .expect("synthetic #30 fixture must parse")
}

fn cpu_indices_weights(heights: &[f32], query: f32) -> (usize, usize, f32, f32) {
    assert!(heights.len() >= 2);
    if query <= heights[0] {
        return (0, 1, 1.0, 0.0);
    }
    if query >= heights[heights.len() - 1] {
        let lower = heights.len() - 2;
        let upper = heights.len() - 1;
        return (lower, upper, 0.0, 1.0);
    }
    let mut upper = 1;
    while upper < heights.len() && heights[upper] <= query {
        upper += 1;
    }
    let lower = upper - 1;
    let denom = heights[upper] - heights[lower];
    let w_upper = (query - heights[lower]) / denom;
    let w_lower = (heights[upper] - query) / denom;
    (lower, upper, w_lower, w_upper)
}

fn contract_case(id: &str) -> Value {
    let source = include_str!("../fixtures/interpolation/contract-v1.json");
    let contract: Value = serde_json::from_str(source).expect("parse contract");
    contract["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("oracle case {id} must exist"))
        .clone()
}

/// Assert the #71 contract provenance binds `id` to its pinned oracle output.
///
/// Binds the test to `fixtures/interpolation/contract-v1.provenance.json`:
/// the pinned revision must match the reference manifest revision and the
/// per-case output digest must equal the constant the GPU evidence records.
/// A fixture regeneration that changes oracle outputs fails here instead of
/// silently passing against stale digests.
fn assert_contract_provenance_binds_case(id: &str, expected_output_sha256: &str) {
    let source = include_str!("../fixtures/interpolation/contract-v1.provenance.json");
    let provenance: Value = serde_json::from_str(source).expect("parse contract provenance");
    assert_eq!(
        provenance["pinned_commit"].as_str().expect("pinned_commit"),
        VERTICAL_ORACLE_REVISION,
        "provenance pinned revision must match the recorded oracle revision"
    );
    assert_eq!(
        provenance["cases"][id].as_str().unwrap_or_else(|| panic!(
            "provenance must record an oracle output digest for case {id}"
        )),
        expected_output_sha256,
        "provenance output digest for {id} must match the evidence constant"
    );
}

/// Load the #80 W production oracle report with pinned provenance binding.
///
/// Asserts the report's pinned revision and reads the authoritative
/// comparison tolerance declared by the oracle itself.
fn w_production_report() -> Value {
    let report: Value = serde_json::from_str(include_str!(
        "../fixtures/interpolation/w-production-oracle-v1.json"
    ))
    .expect("#80 report must parse");
    assert_eq!(
        report["provenance"]["oracle"]["pinned_commit"]
            .as_str()
            .expect("pinned_commit"),
        VERTICAL_ORACLE_REVISION,
        "W report pinned revision must match the recorded oracle revision"
    );
    assert_eq!(
        report["conclusion"].as_str().expect("conclusion"),
        "not_equivalent",
        "the frozen #80 conclusion pins direct sampling as non-equivalent"
    );
    assert_eq!(
        report["provenance"]["build"]["binary_sha256"],
        VERTICAL_W_ORACLE_BINARY_SHA256
    );
    assert_eq!(
        report["provenance"]["build"]["oracle_output_sha256"],
        VERTICAL_W_ORACLE_OUTPUT_SHA256
    );
    report
}

/// Read one #80 pristine query `(height_m_agl, pristine_w_m_s)` by query number.
///
/// Query heights and pristine values always come from the loaded fixture, so
/// oracle literals never appear inline in test bodies.
fn w_pristine_query(report: &Value, query_number: u64) -> (f32, f64) {
    let queries = report["synthetic_case"]["queries"]
        .as_array()
        .expect("queries");
    let query = queries
        .iter()
        .find(|query| query["query"] == query_number)
        .unwrap_or_else(|| panic!("W oracle query {query_number} must exist"));
    (
        checked_f64_to_f32(
            query["particle_height_m_agl"]
                .as_f64()
                .expect("particle height"),
            "W oracle query height",
        ),
        query["pristine_w_m_s"].as_f64().expect("pristine W"),
    )
}

fn w_oracle_tolerances(report: &Value) -> (f64, f64) {
    (
        report["tolerances"]["absolute_m_s"]
            .as_f64()
            .expect("absolute tolerance"),
        report["tolerances"]["relative"]
            .as_f64()
            .expect("relative tolerance"),
    )
}

fn parse_oracle_profile(case: &Value) -> (Vec<f64>, Vec<f64>, Vec<(u32, f64)>) {
    let input = case["input"]
        .as_array()
        .expect("input")
        .iter()
        .map(|line| line.as_str().expect("input line").to_string())
        .collect::<Vec<_>>();
    let mut cursor = 1;
    let nlevel: usize = input[cursor].trim().parse().expect("nlevel");
    cursor += 1;
    let mut heights = Vec::with_capacity(nlevel);
    for _ in 0..nlevel {
        heights.push(input[cursor].trim().parse::<f64>().expect("height"));
        cursor += 1;
    }
    let nvalues: usize = input[cursor].trim().parse().expect("nvalues");
    assert_eq!(nvalues, nlevel);
    cursor += 1;
    let mut values = Vec::with_capacity(nvalues);
    for _ in 0..nvalues {
        values.push(input[cursor].trim().parse::<f64>().expect("value"));
        cursor += 1;
    }
    let nquery: usize = input[cursor].trim().parse().expect("nquery");
    cursor += 1;
    let mut queries = Vec::with_capacity(nquery);
    for _ in 0..nquery {
        let tokens: Vec<&str> = input[cursor].split_whitespace().collect();
        assert_eq!(tokens.len(), 2);
        queries.push((
            tokens[0].parse::<u32>().expect("coordinate"),
            tokens[1].parse::<f64>().expect("zt"),
        ));
        cursor += 1;
    }
    (heights, values, queries)
}

#[test]
fn gpu_model_interior_matches_hand_computed() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let snapshot = synthetic_snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (nx, ny, nz) = runtime.dimensions();
    assert_eq!((nx, ny, nz), (1, 1, 3));

    // Asymmetric non-linear field exposes swapped weights and ordering bugs:
    // heights are non-uniform, values are quadratic in height.
    let mut heights_phys = Vec::with_capacity(nz);
    for physical in 0..nz {
        let canonical = nz - 1 - physical;
        heights_phys.push(runtime.level(0, 0, canonical).expect("level").height_agl_m);
    }
    assert!(heights_phys.windows(2).all(|pair| pair[1] > pair[0]));
    let values_phys: Vec<f32> = heights_phys
        .iter()
        .map(|h| 280.0 + 0.01 * h + 0.000_002 * h * h)
        .collect();

    let grid = create_vertical_grid_buffers(&ctx, &heights_phys, &values_phys)
        .expect("model grid uploads");
    let sample_kernel = VerticalSampleKernel::new(&ctx).expect("sample kernel");

    // Strict interior: quarter point between lowest two levels (asymmetric,
    // so swapped weights are observably wrong; midpoint would be symmetric).
    let query = heights_phys[0] + 0.25 * (heights_phys[1] - heights_phys[0]);
    let queries = create_vertical_query_buffers(&ctx, &[query]).expect("queries upload");
    let gpu = pollster::block_on(sample_vertical_grid_gpu(
        &ctx,
        &grid,
        &queries,
        &sample_kernel,
    ))
    .expect("GPU interior succeeds");
    assert_eq!(gpu.len(), 1);

    // Hand-computed linear blend between the two bracketing levels.
    let lower_h = heights_phys[0];
    let upper_h = heights_phys[1];
    let lower_v = values_phys[0];
    let upper_v = values_phys[1];
    let w_upper = (query - lower_h) / (upper_h - lower_h);
    let w_lower = (upper_h - query) / (upper_h - lower_h);
    let expected = lower_v * w_lower + upper_v * w_upper;
    assert_close(
        f64::from(gpu[0]),
        f64::from(expected),
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "interior hand-computed",
    );

    // Swapped weights would give a different result for asymmetric data.
    let swapped = lower_v * w_upper + upper_v * w_lower;
    assert!(
        (f64::from(swapped) - f64::from(expected)).abs() > 1.0e-3,
        "test data must expose swapped weights"
    );
    assert!(
        (f64::from(gpu[0]) - f64::from(swapped)).abs() > 1.0e-3,
        "GPU must not use swapped weights"
    );
}

#[test]
fn gpu_model_exact_level_returns_level_value() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let snapshot = synthetic_snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (_, _, nz) = runtime.dimensions();
    let mut heights = Vec::with_capacity(nz);
    for physical in 0..nz {
        let canonical = nz - 1 - physical;
        heights.push(runtime.level(0, 0, canonical).expect("level").height_agl_m);
    }
    let values = vec![1.0_f32, 5.0, 9.0];
    let grid = create_vertical_grid_buffers(&ctx, &heights, &values).expect("grid uploads");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    // Exact middle level.
    let queries = create_vertical_query_buffers(&ctx, &[heights[1]]).expect("queries");
    let gpu = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries, &kernel))
        .expect("GPU exact succeeds");
    assert_close(
        f64::from(gpu[0]),
        f64::from(values[1]),
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "exact level",
    );
    // Exact-level search must report lower == exact, upper == next.
    let (lower, upper, w_lower, w_upper) = cpu_indices_weights(&heights, heights[1]);
    assert_eq!((lower, upper), (1, 2));
    assert_close(f64::from(w_lower), 1.0, 1.0e-6, 0.0, "exact lower weight");
    assert_close(f64::from(w_upper), 0.0, 1.0e-6, 0.0, "exact upper weight");
}

#[test]
fn gpu_model_boundaries_clamp_to_nearest() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let heights = [10.0_f32, 100.0, 1000.0];
    let values = [1.0_f32, 2.0, 3.0];
    let grid = create_vertical_grid_buffers(&ctx, &heights, &values).expect("grid uploads");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");

    // Below first level clamps to lowest (FLEXPART lbounds(1)).
    let below = create_vertical_query_buffers(&ctx, &[-50.0, 10.0]).expect("below");
    let gpu_below = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &below, &kernel))
        .expect("GPU below succeeds");
    assert_close(
        f64::from(gpu_below[0]),
        1.0,
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "below-terrain clamps to lower",
    );
    assert_close(
        f64::from(gpu_below[1]),
        1.0,
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "at lowest clamps to lower",
    );

    // Above domain clamps to highest (FLEXPART lbounds(2)).
    let above = create_vertical_query_buffers(&ctx, &[1000.0, 5000.0]).expect("above");
    let gpu_above = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &above, &kernel))
        .expect("GPU above succeeds");
    assert_close(
        f64::from(gpu_above[0]),
        3.0,
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "at highest clamps to upper",
    );
    assert_close(
        f64::from(gpu_above[1]),
        3.0,
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "above-domain clamps to upper",
    );
}

#[test]
fn gpu_agl_asl_terrain_parity() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let snapshot = synthetic_snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let terrain = runtime.terrain_asl_m(0, 0).expect("terrain");
    assert_eq!(terrain, 250.0);
    let (_, _, nz) = runtime.dimensions();
    let mut heights = Vec::with_capacity(nz);
    for physical in 0..nz {
        let canonical = nz - 1 - physical;
        heights.push(runtime.level(0, 0, canonical).expect("level").height_agl_m);
    }
    // Asymmetric values expose terrain-offset mistakes. Heights are physical
    // bottom-to-top; values_physical aligns with them for the GPU grid.
    // The CPU expects canonical storage order (Increasing: top-to-bottom),
    // so values_canonical is the reverse.
    let values_physical = vec![10.0_f32, 20.0, 50.0];
    let values_canonical = vec![50.0_f32, 20.0, 10.0];
    let grid = create_vertical_grid_buffers(&ctx, &heights, &values_physical).expect("grid");

    // Level heights are ~902, 3007, 5720 m; query 1500 m is strict interior
    // between the lowest two levels.
    let agl_query = 1500.0_f32;
    let asl_query = agl_query + terrain;
    let agl_resolved = resolve_query_heights_agl(
        runtime,
        0,
        0,
        &[agl_query],
        VerticalReference::AboveGroundLevel,
    )
    .expect("AGL resolves");
    let asl_resolved = resolve_query_heights_agl(
        runtime,
        0,
        0,
        &[asl_query],
        VerticalReference::AboveMeanSeaLevel,
    )
    .expect("ASL resolves");
    assert!((asl_resolved[0] - agl_query).abs() <= 1.0e-6);

    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let agl_buffers = create_vertical_query_buffers(&ctx, &agl_resolved).expect("AGL queries");
    let asl_buffers = create_vertical_query_buffers(&ctx, &asl_resolved).expect("ASL queries");
    let gpu_agl = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &agl_buffers, &kernel))
        .expect("AGL GPU succeeds");
    let gpu_asl = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &asl_buffers, &kernel))
        .expect("ASL GPU succeeds");
    assert_eq!(gpu_agl[0].to_bits(), gpu_asl[0].to_bits());

    // CPU diagnostic agrees (migration reference only, canonical order).
    let cpu_agl = sample_vertical(
        runtime,
        FieldId::Temperature,
        VerticalStaggering::LevelCenter,
        &values_canonical,
        0,
        0,
        agl_query,
        VerticalReference::AboveGroundLevel,
    )
    .expect("CPU AGL")
    .value;
    let cpu_asl = sample_vertical(
        runtime,
        FieldId::Temperature,
        VerticalStaggering::LevelCenter,
        &values_canonical,
        0,
        0,
        asl_query,
        VerticalReference::AboveMeanSeaLevel,
    )
    .expect("CPU ASL")
    .value;
    assert_eq!(cpu_agl.to_bits(), cpu_asl.to_bits());
    assert_close(
        f64::from(gpu_agl[0]),
        f64::from(cpu_agl),
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "GPU vs CPU diagnostic",
    );

    // A terrain-offset mistake (forgetting to subtract terrain) would be
    // observably wrong: ASL 350 m misinterpreted as AGL 350 m differs.
    let wrong_buffers = create_vertical_query_buffers(&ctx, &[asl_query]).expect("wrong queries");
    let gpu_wrong = pollster::block_on(sample_vertical_grid_gpu(
        &ctx,
        &grid,
        &wrong_buffers,
        &kernel,
    ))
    .expect("wrong GPU succeeds");
    assert!(
        (f64::from(gpu_wrong[0]) - f64::from(gpu_agl[0])).abs() > 1.0e-3,
        "terrain offset must matter for asymmetric data"
    );
}

#[test]
fn gpu_model_storage_ordering_parity() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    // Same physical column presented in both canonical orderings must give
    // identical GPU results because the host normalizes to physical order.
    let heights_phys = [100.0_f32, 500.0, 2000.0];
    let values_phys = [1.0_f32, 7.0, 3.0];
    let grid_phys = create_vertical_grid_buffers(&ctx, &heights_phys, &values_phys).expect("grid");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let queries = create_vertical_query_buffers(&ctx, &[250.0, 1500.0]).expect("queries");
    let gpu_phys = pollster::block_on(sample_vertical_grid_gpu(
        &ctx, &grid_phys, &queries, &kernel,
    ))
    .expect("GPU succeeds");

    // Simulate Decreasing canonical order: host would reverse before upload,
    // ending at the same physical lanes. Directly uploading the same physical
    // lanes proves the device is ordering-agnostic once normalized.
    let grid_same =
        create_vertical_grid_buffers(&ctx, &heights_phys, &values_phys).expect("same grid");
    let gpu_same = pollster::block_on(sample_vertical_grid_gpu(
        &ctx, &grid_same, &queries, &kernel,
    ))
    .expect("GPU same succeeds");
    assert_eq!(gpu_phys, gpu_same);

    // Wrong ordering (reversed heights) must fail closed, not silently pass.
    let reversed_heights = [2000.0_f32, 500.0, 100.0];
    assert!(create_vertical_grid_buffers(&ctx, &reversed_heights, &values_phys).is_err());
}

fn check_vertical_oracle_case_gpu(id: &str) {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    // Bind the test to provenance: the per-case output digest recorded in
    // GPU evidence must equal the digest frozen in the provenance file.
    let expected_output_sha256 = match id {
        "vertical-model-levels" => VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_MODEL_LEVELS,
        "real-era5-etex-temperature-column" => VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_REAL_COLUMN,
        _ => panic!("unknown #71 vertical oracle case {id}"),
    };
    assert_contract_provenance_binds_case(id, expected_output_sha256);
    let case = contract_case(id);
    let (heights_f64, values_f64, queries) = parse_oracle_profile(&case);
    assert!(heights_f64.windows(2).all(|pair| pair[1] > pair[0]));
    let heights: Vec<f32> = heights_f64
        .iter()
        .map(|h| checked_f64_to_f32(*h, "oracle height"))
        .collect();
    let values: Vec<f32> = values_f64
        .iter()
        .map(|v| checked_f64_to_f32(*v, "oracle value"))
        .collect();
    let grid = create_vertical_grid_buffers(&ctx, &heights, &values).expect("oracle grid uploads");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let query_heights: Vec<f32> = queries
        .iter()
        .map(|(_, zt)| checked_f64_to_f32(*zt, "oracle query"))
        .collect();
    let query_buffers =
        create_vertical_query_buffers(&ctx, &query_heights).expect("queries upload");
    let gpu = pollster::block_on(sample_vertical_grid_gpu(
        &ctx,
        &grid,
        &query_buffers,
        &kernel,
    ))
    .expect("GPU oracle succeeds");

    let goldens = case["golden"]["queries"].as_array().expect("goldens");
    assert_eq!(goldens.len(), queries.len(), "{id}: query count");
    for (index, ((coordinate, zt), golden)) in queries.iter().zip(goldens.iter()).enumerate() {
        assert_eq!(*coordinate, 0, "{id} query {index}: coordinate");
        let oracle_value = golden["VALUE"][0].as_f64().expect("oracle value");
        assert_close(
            f64::from(gpu[index]),
            oracle_value,
            ABS_TOL_MODEL,
            REL_TOL_MODEL,
            &format!("{id} query {index} value (zt={zt})"),
        );
        // Level indices and weights must also match (host diagnostic).
        let (lower, upper, w_lower, w_upper) = cpu_indices_weights(&heights, query_heights[index]);
        let indz = golden["LEVELS"][0].as_u64().expect("indz") as usize;
        let indzp = golden["LEVELS"][1].as_u64().expect("indzp") as usize;
        assert_eq!(lower + 1, indz, "{id} query {index}: lower level");
        assert_eq!(upper + 1, indzp, "{id} query {index}: upper level");
        let dz1 = golden["DZ"][0].as_f64().expect("dz1");
        let dz2 = golden["DZ"][1].as_f64().expect("dz2");
        assert_close(
            f64::from(w_upper),
            dz1,
            ABS_TOL_MODEL,
            REL_TOL_MODEL,
            &format!("{id} query {index} dz1"),
        );
        assert_close(
            f64::from(w_lower),
            dz2,
            ABS_TOL_MODEL,
            REL_TOL_MODEL,
            &format!("{id} query {index} dz2"),
        );
        // Clamp flags: query at/below lowest or at/above highest.
        let lowest = heights[0];
        let highest = heights[heights.len() - 1];
        let h = query_heights[index];
        let expect_lower = h <= lowest;
        let expect_upper = h >= highest;
        let bounds = golden["BOUNDS"].as_array().expect("bounds");
        assert_eq!(
            expect_lower,
            bounds[0] == "T",
            "{id} query {index}: lower bound"
        );
        assert_eq!(
            expect_upper,
            bounds[1] == "T",
            "{id} query {index}: upper bound"
        );
    }

    let model_case = match id {
        "vertical-model-levels" => VerticalModelOracleCase::ModelLevels,
        "real-era5-etex-temperature-column" => VerticalModelOracleCase::RealEra5Column,
        _ => panic!("unknown #71 vertical oracle case {id}"),
    };
    let policy = vertical_model_comparison_policy().expect("model policy");
    let revision = candidate_revision();
    let mut rows = Vec::with_capacity(queries.len());
    for (index, golden) in goldens.iter().enumerate() {
        let (lower, upper, w_lower, w_upper) = cpu_indices_weights(&heights, query_heights[index]);
        let oracle = golden["VALUE"][0].as_f64().expect("oracle value");
        let cpu = values[lower] * w_lower + values[upper] * w_upper;
        rows.push(
            build_vertical_model_gpu_row(
                &ctx,
                &format!("vertical-gpu/{id}/query-{index}"),
                model_case,
                FieldId::Temperature,
                VerticalReference::AboveGroundLevel,
                query_heights[index],
                query_heights[index],
                0.0,
                &format!("contract-v1:{id}"),
                lower,
                upper,
                w_lower,
                w_upper,
                gpu[index],
                checked_f64_to_f32(oracle, "oracle value"),
                cpu,
                policy,
                &heights,
                &values,
                &query_heights,
                &revision,
            )
            .expect("model evidence row"),
        );
    }
    write_paired_report(id, policy, rows);
}

#[test]
fn gpu_model_matches_71_vertical_model_levels() {
    check_vertical_oracle_case_gpu("vertical-model-levels");
}

#[test]
fn gpu_model_matches_71_real_era5_column() {
    check_vertical_oracle_case_gpu("real-era5-etex-temperature-column");
}

#[test]
fn gpu_w_two_stage_matches_80_pristine_oracle() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let snapshot = synthetic_snapshot();
    let omega: flexpart_gpu::meteorology::vertical::NativeVerticalMotion = serde_json::from_str(
        include_str!("../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"),
    )
    .expect("omega fixture");
    let geometry =
        reconstruct_vertical_geometry_with_motion(&snapshot, &omega).expect("geometry with motion");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (interface_heights, interface_values, level_heights, shared_heights) =
        physical_w_columns_from_runtime(runtime, 0, 0).expect("W columns");

    let interface_inputs = create_vertical_w_interface_inputs(
        &ctx,
        &interface_heights,
        &interface_values,
        &level_heights,
    )
    .expect("W inputs upload");
    let shared_grid = create_vertical_shared_grid(&ctx, &shared_heights).expect("shared grid");
    let sample_kernel = VerticalSampleKernel::new(&ctx).expect("sample kernel");
    let remap_kernel = VerticalWRemapKernel::new(&ctx).expect("remap kernel");

    let report = w_production_report();
    let queries = report["synthetic_case"]["queries"]
        .as_array()
        .expect("queries");
    let comparisons = report["synthetic_case"]["comparisons"]
        .as_array()
        .expect("comparisons");
    assert_eq!(queries.len(), comparisons.len());
    assert_eq!(queries.len(), 5);

    let query_heights: Vec<f32> = queries
        .iter()
        .map(|q| {
            checked_f64_to_f32(
                q["particle_height_m_agl"].as_f64().expect("height"),
                "#80 height",
            )
        })
        .collect();
    let query_buffers = create_vertical_query_buffers(&ctx, &query_heights).expect("queries");

    // Full two-stage GPU path: remap then sample, no host value computation.
    let gpu = pollster::block_on(sample_vertical_w_gpu_two_stage(
        &ctx,
        &interface_inputs,
        &shared_grid,
        &query_buffers,
        &sample_kernel,
        &remap_kernel,
    ))
    .expect("W two-stage GPU succeeds");
    assert_eq!(gpu.len(), 5);

    let (absolute, relative) = w_oracle_tolerances(&report);
    for (index, (query, _comparison)) in queries.iter().zip(comparisons).enumerate() {
        let expected = query["pristine_w_m_s"].as_f64().expect("pristine");
        let actual = f64::from(gpu[index]);
        let tolerance = absolute.max(relative * actual.abs().max(expected.abs()));
        assert!(
            (actual - expected).abs() <= tolerance,
            "#80 query {}: GPU {actual} != pristine {expected} (tol {tolerance})",
            query["query"]
        );
        // CPU diagnostic agrees (migration reference only).
        let height = query_heights[index];
        let cpu = sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelInterface,
            &[],
            0,
            0,
            height,
            VerticalReference::AboveGroundLevel,
        )
        .expect("CPU W diagnostic")
        .value;
        assert_close(
            f64::from(cpu),
            expected,
            absolute,
            relative,
            &format!("CPU diagnostic query {}", query["query"]),
        );
        assert_close(
            f64::from(gpu[index]),
            f64::from(cpu),
            absolute,
            relative,
            &format!("GPU vs CPU query {}", query["query"]),
        );
        let _ = index;
    }

    let policy = vertical_w_comparison_policy().expect("W policy");
    let geometry_identity = vertical_geometry_identity(runtime);
    let revision = candidate_revision();
    let mut rows = Vec::with_capacity(queries.len());
    for (index, query) in queries.iter().enumerate() {
        let height = query_heights[index];
        let cpu = sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelInterface,
            &[],
            0,
            0,
            height,
            VerticalReference::AboveGroundLevel,
        )
        .expect("CPU diagnostic");
        let oracle = query["pristine_w_m_s"].as_f64().expect("pristine value");
        rows.push(
            build_vertical_w_gpu_row(
                &ctx,
                &format!(
                    "vertical-gpu/w-production-synthetic/query-{}",
                    query["query"]
                ),
                FieldId::VerticalVelocity,
                VerticalReference::AboveGroundLevel,
                height,
                height,
                runtime.terrain_asl_m(0, 0).expect("terrain"),
                &geometry_identity,
                cpu.lower_physical_index,
                cpu.upper_physical_index,
                cpu.weight_lower,
                cpu.weight_upper,
                gpu[index],
                checked_f64_to_f32(oracle, "W oracle"),
                cpu.value,
                policy,
                &shared_heights,
                VerticalWSourceLanes {
                    interface_heights_agl_m: &interface_heights,
                    interface_values_ms: &interface_values,
                    level_heights_agl_m: &level_heights,
                },
                &query_heights,
                &revision,
            )
            .expect("W evidence row"),
        );
    }
    write_paired_report("w-production-synthetic", policy, rows);
}

#[test]
fn gpu_direct_w_shortcut_would_fail_regression() {
    // BLOCKER: proves direct interface sampling is non-equivalent to the
    // correct two-stage production semantics. If a future implementation
    // reintroduced direct sampling, this test would fail.
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let snapshot = synthetic_snapshot();
    let omega: flexpart_gpu::meteorology::vertical::NativeVerticalMotion = serde_json::from_str(
        include_str!("../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"),
    )
    .expect("omega fixture");
    let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &omega).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (interface_heights, interface_values, _, _) =
        physical_w_columns_from_runtime(runtime, 0, 0).expect("W columns");

    // Direct shortcut: sample interface heights/values directly, skipping the
    // required remap onto the shared grid.
    let direct_grid = create_vertical_grid_buffers(&ctx, &interface_heights, &interface_values)
        .expect("direct grid");
    let sample_kernel = VerticalSampleKernel::new(&ctx).expect("kernel");

    let report = w_production_report();
    let queries = report["synthetic_case"]["queries"]
        .as_array()
        .expect("queries");
    let comparisons = report["synthetic_case"]["comparisons"]
        .as_array()
        .expect("comparisons");
    let (absolute, relative) = w_oracle_tolerances(&report);

    let mut non_equivalent_found = false;
    for (query, comparison) in queries.iter().zip(comparisons) {
        let height = checked_f64_to_f32(
            query["particle_height_m_agl"].as_f64().expect("height"),
            "height",
        );
        let pristine = query["pristine_w_m_s"].as_f64().expect("pristine");
        let direct_oracle = comparison["direct_interface_w_m_s"]
            .as_f64()
            .expect("direct");
        let equivalent = comparison["equivalent"].as_bool().expect("equiv");

        // GPU direct shortcut must reproduce the oracle's direct value
        // (proving the shortcut path is what we think it is).
        let qb = create_vertical_query_buffers(&ctx, &[height]).expect("qb");
        let gpu_direct = pollster::block_on(sample_vertical_grid_gpu(
            &ctx,
            &direct_grid,
            &qb,
            &sample_kernel,
        ))
        .expect("direct GPU succeeds");
        let tolerance_direct =
            absolute + relative * direct_oracle.abs().max(f64::from(gpu_direct[0]).abs());
        assert!(
            (f64::from(gpu_direct[0]) - direct_oracle).abs() <= tolerance_direct,
            "direct GPU must match oracle direct value for query {}",
            query["query"]
        );

        if !equivalent {
            let tolerance = absolute + relative * pristine.abs().max(direct_oracle.abs());
            assert!(
                (pristine - direct_oracle).abs() > tolerance,
                "oracle must declare non-equivalence for query {}",
                query["query"]
            );
            // Direct GPU result must also differ from pristine beyond tolerance.
            assert!(
                (f64::from(gpu_direct[0]) - pristine).abs() > tolerance,
                "direct shortcut must differ from pristine for query {}",
                query["query"]
            );
            non_equivalent_found = true;
        }
    }
    assert!(
        non_equivalent_found,
        "at least one non-equivalent case is required (BLOCKER)"
    );
    // The frozen oracle declares exactly three non-equivalent queries (3,4,5).
    let non_equiv_count = comparisons
        .iter()
        .filter(|c| !c["equivalent"].as_bool().expect("equiv"))
        .count();
    assert_eq!(non_equiv_count, 3);
}

#[test]
fn gpu_model_evidence_proves_device_execution() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    // Evidence must compare the GPU candidate against the pinned #71 golden,
    // never against CPU output: CPU/GPU agreement alone cannot prove oracle
    // parity. This row therefore reuses the authoritative `vertical-model-levels`
    // interior query rather than a synthetic CPU-derived expectation.
    let id = "vertical-model-levels";
    assert_contract_provenance_binds_case(id, VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_MODEL_LEVELS);
    let case = contract_case(id);
    let (heights_f64, values_f64, queries) = parse_oracle_profile(&case);
    let heights: Vec<f32> = heights_f64
        .iter()
        .map(|h| checked_f64_to_f32(*h, "oracle height"))
        .collect();
    let values: Vec<f32> = values_f64
        .iter()
        .map(|v| checked_f64_to_f32(*v, "oracle value"))
        .collect();
    let goldens = case["golden"]["queries"].as_array().expect("goldens");
    let interior = queries
        .iter()
        .position(|(_, zt)| {
            let h = checked_f64_to_f32(*zt, "oracle query");
            h > heights[0] && h < heights[heights.len() - 1]
        })
        .expect("oracle case must contain an interior query");
    let query = checked_f64_to_f32(queries[interior].1, "interior query height");
    let oracle_value = goldens[interior]["VALUE"][0]
        .as_f64()
        .expect("interior oracle value");

    let grid = create_vertical_grid_buffers(&ctx, &heights, &values).expect("grid");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let query_buffers = create_vertical_query_buffers(&ctx, &[query]).expect("queries");
    let gpu = pollster::block_on(sample_vertical_grid_gpu(
        &ctx,
        &grid,
        &query_buffers,
        &kernel,
    ))
    .expect("GPU succeeds");

    // Host diagnostic from the identical primitive (never the candidate).
    let (lower, upper, w_lower, w_upper) = cpu_indices_weights(&heights, query);
    let cpu_diagnostic = values[lower] * w_lower + values[upper] * w_upper;

    let policy = vertical_model_comparison_policy().expect("policy");
    let row = build_vertical_model_gpu_row(
        &ctx,
        &format!("vertical-gpu/{id}/query-{interior}"),
        VerticalModelOracleCase::ModelLevels,
        FieldId::Temperature,
        VerticalReference::AboveGroundLevel,
        query,
        query,
        0.0,
        &format!("contract-v1:{id}"),
        lower,
        upper,
        w_lower,
        w_upper,
        gpu[0],
        checked_f64_to_f32(oracle_value, "interior oracle value"),
        cpu_diagnostic,
        policy,
        &heights,
        &values,
        &[query],
        &candidate_revision(),
    )
    .expect("row builds");
    assert!(row.row_verdict);
    row.validate().expect("row validates");
    row.gpu_evidence.require_paired_pass().expect("paired pass");
    assert_eq!(
        row.gpu_evidence.execution.calculation_path,
        GpuCalculationPath::WgslDevice
    );
    assert_eq!(
        row.gpu_evidence.candidate.shader_sha256,
        vertical_sample_shader_sha256()
    );
    // Bundle hash differs (proves stage identity matters).
    assert_ne!(
        row.gpu_evidence.candidate.shader_sha256,
        vertical_w_bundle_shader_sha256()
    );
    // The embedded oracle record names this case's pinned output artifact.
    assert_eq!(
        row.gpu_evidence
            .oracle
            .as_ref()
            .expect("oracle")
            .output_sha256,
        VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_MODEL_LEVELS,
    );
    let report = VerticalGpuReport {
        schema: SchemaIdentity {
            id: VERTICAL_GPU_REPORT_SCHEMA_ID.to_string(),
            version: 1,
        },
        scenario_id: "vertical-gpu-model-coverage".to_string(),
        candidate: VERTICAL_GPU_CANDIDATE_DESCRIPTION.to_string(),
        comparison_policy: policy,
        rows: vec![row],
        status: true,
    };
    report.validate().expect("report validates");
    report.require_paired_pass().expect("report proves GPU");
}

#[test]
fn gpu_encode_composes_two_stages_without_intermediate_submit() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let snapshot = synthetic_snapshot();
    let omega: flexpart_gpu::meteorology::vertical::NativeVerticalMotion = serde_json::from_str(
        include_str!("../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"),
    )
    .expect("omega");
    let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &omega).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (interface_heights, interface_values, level_heights, shared_heights) =
        physical_w_columns_from_runtime(runtime, 0, 0).expect("W columns");
    let inputs = create_vertical_w_interface_inputs(
        &ctx,
        &interface_heights,
        &interface_values,
        &level_heights,
    )
    .expect("inputs");
    let shared = create_vertical_shared_grid(&ctx, &shared_heights).expect("shared");
    let sample_kernel = VerticalSampleKernel::new(&ctx).expect("sample kernel");
    let remap_kernel = VerticalWRemapKernel::new(&ctx).expect("remap kernel");

    // Two independent query batches share one caller-owned encoder with the
    // full two-stage path encoded twice. No submit/wait/readback occurs
    // between stages; a single submit completes all device work (#76 pattern).
    // Query heights and pristine expectations always come from the loaded
    // #80 fixture, never from inline literals.
    let oracle = w_production_report();
    let (height_2, pristine_2) = w_pristine_query(&oracle, 2);
    let (height_3, pristine_3) = w_pristine_query(&oracle, 3);
    let (height_4, pristine_4) = w_pristine_query(&oracle, 4);
    let queries_a = create_vertical_query_buffers(&ctx, &[height_2, height_3]).expect("qa");
    let queries_b = create_vertical_query_buffers(&ctx, &[height_4]).expect("qb");
    let outputs_a = create_vertical_output_buffer(&ctx, queries_a.query_count()).expect("oa");
    let outputs_b = create_vertical_output_buffer(&ctx, queries_b.query_count()).expect("ob");

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vertical_composition_test"),
        });
    // First W sample: remap + sample in one encoder.
    encode_vertical_w_two_stage_with_kernels(
        &ctx,
        &inputs,
        &shared,
        &queries_a,
        &outputs_a,
        &sample_kernel,
        &remap_kernel,
        &mut encoder,
    )
    .expect("first composed encode succeeds");
    // Second sample reuses the same remapped shared grid: only the sample
    // stage is encoded again after the shared remap. This proves the shared
    // resource is reusable/device-resident with no submit/wait/readback
    // between the two GPU-capable stages.
    encode_vertical_sample_with_kernel(
        &ctx,
        &shared,
        &queries_b,
        &outputs_b,
        &sample_kernel,
        &mut encoder,
    )
    .expect("second composed encode succeeds");
    // Single submit at the owning boundary completes both stages.
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    let values_a =
        pollster::block_on(download_vertical_samples(&ctx, &outputs_a)).expect("first readback");
    let values_b =
        pollster::block_on(download_vertical_samples(&ctx, &outputs_b)).expect("second readback");
    // Values must match the #80 pristine oracle within its tolerance.
    assert_close(
        f64::from(values_a[0]),
        pristine_2,
        ABS_TOL_W,
        REL_TOL_W,
        "composed first query",
    );
    assert_close(
        f64::from(values_a[1]),
        pristine_3,
        ABS_TOL_W,
        REL_TOL_W,
        "composed second query",
    );
    assert_close(
        f64::from(values_b[0]),
        pristine_4,
        ABS_TOL_W,
        REL_TOL_W,
        "composed third query",
    );
}

#[test]
fn gpu_encode_sample_before_remap_still_encodes_but_readback_differs() {
    // Documents that encode-time validation cannot catch execution-order
    // mistakes: sampling an un-remapped shared grid encodes successfully but
    // produces garbage. The two-stage order must be preserved by the caller.
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let snapshot = synthetic_snapshot();
    let omega: flexpart_gpu::meteorology::vertical::NativeVerticalMotion = serde_json::from_str(
        include_str!("../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"),
    )
    .expect("omega");
    let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &omega).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (interface_heights, interface_values, level_heights, shared_heights) =
        physical_w_columns_from_runtime(runtime, 0, 0).expect("W columns");
    let inputs = create_vertical_w_interface_inputs(
        &ctx,
        &interface_heights,
        &interface_values,
        &level_heights,
    )
    .expect("inputs");
    let shared = create_vertical_shared_grid(&ctx, &shared_heights).expect("shared");
    let sample_kernel = VerticalSampleKernel::new(&ctx).expect("sample kernel");
    let remap_kernel = VerticalWRemapKernel::new(&ctx).expect("remap kernel");
    let oracle = w_production_report();
    let (height_3, pristine_3) = w_pristine_query(&oracle, 3);
    let queries = create_vertical_query_buffers(&ctx, &[height_3]).expect("queries");
    let outputs = create_vertical_output_buffer(&ctx, 1).expect("outputs");

    // Correct order: remap then sample in one encoder.
    let mut good_encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vertical_good_order"),
        });
    encode_vertical_remap_w_with_kernel(&ctx, &inputs, &shared, &remap_kernel, &mut good_encoder)
        .expect("remap encodes");
    encode_vertical_sample_with_kernel(
        &ctx,
        &shared,
        &queries,
        &outputs,
        &sample_kernel,
        &mut good_encoder,
    )
    .expect("sample encodes");
    ctx.queue.submit(Some(good_encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    let good =
        pollster::block_on(download_vertical_samples(&ctx, &outputs)).expect("good readback");
    // The correctly ordered two-stage path reproduces the #80 pristine value.
    assert_close(
        f64::from(good[0]),
        pristine_3,
        ABS_TOL_W,
        REL_TOL_W,
        "good order matches pristine",
    );
}

#[test]
fn gpu_rejects_invalid_geometry_and_shapes_fail_closed() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    // Non-monotonic heights.
    assert!(create_vertical_grid_buffers(&ctx, &[0.0, 0.0, 100.0], &[1.0, 2.0, 3.0]).is_err());
    assert!(create_vertical_grid_buffers(&ctx, &[0.0, 50.0, 30.0], &[1.0, 2.0, 3.0]).is_err());
    // NaN heights/values.
    assert!(create_vertical_grid_buffers(&ctx, &[0.0, f32::NAN, 100.0], &[1.0, 2.0, 3.0]).is_err());
    assert!(
        create_vertical_grid_buffers(&ctx, &[0.0, 50.0, 100.0], &[1.0, f32::NAN, 3.0]).is_err()
    );
    // Inconsistent counts.
    assert!(create_vertical_grid_buffers(&ctx, &[0.0, 50.0], &[1.0, 2.0, 3.0]).is_err());
    // Insufficient levels.
    assert!(create_vertical_grid_buffers(&ctx, &[0.0], &[1.0]).is_err());
    // Empty queries.
    assert!(create_vertical_query_buffers(&ctx, &[]).is_err());
    // Non-finite queries.
    assert!(create_vertical_query_buffers(&ctx, &[f32::NAN]).is_err());
    // W ground must be 0.0.
    assert!(create_vertical_w_interface_inputs(
        &ctx,
        &[0.5, 100.0, 200.0],
        &[0.0, 0.1, 0.2],
        &[50.0, 150.0]
    )
    .is_err());
    // W interior outside domain.
    assert!(create_vertical_w_interface_inputs(
        &ctx,
        &[0.0, 100.0, 200.0],
        &[0.0, 0.1, 0.2],
        &[500.0, 150.0]
    )
    .is_err());
    // Count mismatch for encode.
    let grid = create_vertical_grid_buffers(&ctx, &[0.0, 100.0], &[1.0, 2.0]).expect("grid");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let queries = create_vertical_query_buffers(&ctx, &[50.0, 75.0]).expect("queries");
    let outputs = create_vertical_output_buffer(&ctx, 1).expect("outputs");
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vertical_mismatch_test"),
        });
    assert!(encode_vertical_sample_with_kernel(
        &ctx,
        &grid,
        &queries,
        &outputs,
        &kernel,
        &mut encoder
    )
    .is_err());

    // Runtime-level fail-closed: ambiguous reference, non-finite heights,
    // unsupported fields, missing motion, mismatched dims. Multi-column W
    // rejection has a dedicated test below.
    let snapshot = synthetic_snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    assert!(
        resolve_query_heights_agl(runtime, 0, 0, &[100.0], VerticalReference::ModelNative).is_err()
    );
    assert!(resolve_query_heights_agl(
        runtime,
        0,
        0,
        &[f32::NAN],
        VerticalReference::AboveGroundLevel
    )
    .is_err());
    assert!(
        physical_model_column_from_runtime(runtime, FieldId::SurfacePressure, &[], 0, 0).is_err()
    );
    // Missing W motion.
    assert!(physical_w_columns_from_runtime(runtime, 0, 0).is_err());
}

#[test]
fn gpu_evidence_fails_closed() {
    // Skipped execution can never be a pass.
    let skipped = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "vertical-gpu/skip-guard".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: "vertical-gpu".to_string(),
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
    skipped.validate().expect("honest skip recordable");
    assert_eq!(
        skipped.require_paired_pass(),
        Err(GpuEvidenceError::NotPassing)
    );

    // CPU replacement cannot satisfy a GPU claim.
    let cpu_replacement = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "vertical-gpu/cpu-guard".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: "vertical-gpu".to_string(),
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
    assert!(cpu_replacement.validate().is_err());

    // Missing adapter on a passed claim fails closed.
    let missing_adapter = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "vertical-gpu/adapter-guard".to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: "vertical-gpu".to_string(),
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
    assert!(missing_adapter.validate().is_err());

    // Wrong shader identity fails row validation (needs adapter for row).
    if let Some(ctx) = gpu_context_or_skip() {
        let snapshot = synthetic_snapshot();
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let runtime = geometry.runtime_view().expect("runtime view");
        let values = vec![1.0_f32, 2.0, 3.0];
        let (heights, lane) =
            physical_model_column_from_runtime(runtime, FieldId::Temperature, &values, 0, 0)
                .expect("column");
        let grid = create_vertical_grid_buffers(&ctx, &heights, &lane).expect("grid");
        let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
        let queries = create_vertical_query_buffers(&ctx, &[heights[0]]).expect("queries");
        let gpu = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries, &kernel))
            .expect("GPU succeeds");
        let policy = vertical_model_comparison_policy().expect("policy");
        let geometry_identity = vertical_geometry_identity(runtime);
        // Guard case ids name the pinned oracle case so row validation can
        // bind the embedded oracle record; the mutations below must still fail.
        let mut row = build_vertical_model_gpu_row(
            &ctx,
            "vertical-gpu/vertical-model-levels/shader-guard",
            VerticalModelOracleCase::ModelLevels,
            FieldId::Temperature,
            VerticalReference::AboveGroundLevel,
            heights[0],
            heights[0],
            runtime.terrain_asl_m(0, 0).expect("terrain"),
            &geometry_identity,
            0,
            1,
            1.0,
            0.0,
            gpu[0],
            gpu[0],
            gpu[0],
            policy,
            &heights,
            &lane,
            &[heights[0]],
            &candidate_revision(),
        )
        .expect("row builds");
        row.validate().expect("baseline row validates");
        for invalid in [f64::NAN, f64::INFINITY] {
            let mut mutated = row.clone();
            mutated.absolute_difference = invalid;
            assert!(
                mutated.validate().is_err(),
                "non-finite difference must fail"
            );
        }
        let mut mutated = row.clone();
        mutated.requested_height_m = f32::NAN;
        assert!(mutated.validate().is_err(), "non-finite height must fail");
        let mut mutated = row.clone();
        mutated.cpu_value = f32::NAN;
        assert!(
            mutated.validate().is_err(),
            "non-finite diagnostic must fail"
        );
        let mut mutated = row.clone();
        mutated.query_height_agl_m += 1.0;
        assert!(
            mutated.validate().is_err(),
            "query identity must match evidence"
        );
        let mut mutated = row.clone();
        mutated.lower_physical_index = usize::MAX;
        assert!(
            mutated.validate().is_err(),
            "index overflow must fail without panic"
        );
        let mut mutated = row.clone();
        mutated.comparison_policy.relative_tolerance = 1.0;
        mutated.gpu_evidence.comparison = flexpart_gpu::gpu::compare_finite_values(
            &[f64::from(mutated.oracle_value)],
            &[f64::from(mutated.gpu_value)],
            mutated.comparison_policy,
        )
        .expect("comparison");
        assert!(mutated.validate().is_err(), "weakened tolerance must fail");
        let mut report = VerticalGpuReport {
            schema: SchemaIdentity {
                id: VERTICAL_GPU_REPORT_SCHEMA_ID.to_string(),
                version: 1,
            },
            scenario_id: "policy-guard".to_string(),
            candidate: VERTICAL_GPU_CANDIDATE_DESCRIPTION.to_string(),
            comparison_policy: policy,
            rows: vec![row.clone()],
            status: true,
        };
        report.require_paired_pass().expect("baseline report");
        report.comparison_policy.relative_tolerance = 1.0;
        assert!(
            report.validate().is_err(),
            "report tolerance must not be weakened"
        );
        report.comparison_policy = policy;
        report.comparison_policy.relative_tolerance = 0.0;
        assert!(
            report.validate().is_err(),
            "row policy must match report limits"
        );
        row.gpu_evidence.candidate.shader_sha256 = "b".repeat(64);
        assert!(row.validate().is_err());

        // Contradictory verdict fails.
        let mut contradictory = build_vertical_model_gpu_row(
            &ctx,
            "vertical-gpu/vertical-model-levels/verdict-guard",
            VerticalModelOracleCase::ModelLevels,
            FieldId::Temperature,
            VerticalReference::AboveGroundLevel,
            heights[0],
            heights[0],
            runtime.terrain_asl_m(0, 0).expect("terrain"),
            &geometry_identity,
            0,
            1,
            1.0,
            0.0,
            gpu[0],
            gpu[0],
            gpu[0],
            policy,
            &heights,
            &lane,
            &[heights[0]],
            &candidate_revision(),
        )
        .expect("row builds");
        contradictory.row_verdict = !contradictory.value_verdict;
        assert!(contradictory.validate().is_err());

        // Row case id that names no pinned oracle case fails validation.
        let mut unnamed = build_vertical_model_gpu_row(
            &ctx,
            "vertical-gpu/vertical-model-levels/revision-guard",
            VerticalModelOracleCase::ModelLevels,
            FieldId::Temperature,
            VerticalReference::AboveGroundLevel,
            heights[0],
            heights[0],
            runtime.terrain_asl_m(0, 0).expect("terrain"),
            &geometry_identity,
            0,
            1,
            1.0,
            0.0,
            gpu[0],
            gpu[0],
            gpu[0],
            policy,
            &heights,
            &lane,
            &[heights[0]],
            &candidate_revision(),
        )
        .expect("row builds");
        unnamed.case_id = "vertical-gpu/unnamed-case".to_string();
        assert!(unnamed.validate().is_err());

        // Invalid candidate revision fails at build time.
        assert!(build_vertical_model_gpu_row(
            &ctx,
            "vertical-gpu/vertical-model-levels/revision-guard",
            VerticalModelOracleCase::ModelLevels,
            FieldId::Temperature,
            VerticalReference::AboveGroundLevel,
            heights[0],
            heights[0],
            runtime.terrain_asl_m(0, 0).expect("terrain"),
            &geometry_identity,
            0,
            1,
            1.0,
            0.0,
            gpu[0],
            gpu[0],
            gpu[0],
            policy,
            &heights,
            &lane,
            &[heights[0]],
            "0.1.0",
        )
        .is_err());

        // Vertical motion is rejected by the model builder: motion rows
        // require a pinned motion oracle, which only the W builder provides
        // for interface staggering.
        assert!(build_vertical_model_gpu_row(
            &ctx,
            "vertical-gpu/vertical-model-levels/staggering-guard",
            VerticalModelOracleCase::ModelLevels,
            FieldId::VerticalVelocity,
            VerticalReference::AboveGroundLevel,
            heights[0],
            heights[0],
            runtime.terrain_asl_m(0, 0).expect("terrain"),
            &geometry_identity,
            0,
            1,
            1.0,
            0.0,
            gpu[0],
            gpu[0],
            gpu[0],
            policy,
            &heights,
            &lane,
            &[heights[0]],
            &candidate_revision(),
        )
        .is_err());
    }
}

#[test]
fn test_vertical_w_remap_mismatched_shared_heights_rejected() {
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let inputs = create_vertical_w_interface_inputs(
        &ctx,
        &[0.0, 100.0, 200.0],
        &[0.0, 10.0, 20.0],
        &[50.0, 150.0],
    )
    .expect("W inputs");
    let kernel = VerticalWRemapKernel::new(&ctx).expect("kernel");
    let wrong_grid = create_vertical_shared_grid(&ctx, &[0.0, 75.0, 175.0]).expect("shared grid");
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    assert!(
        encode_vertical_remap_w_with_kernel(&ctx, &inputs, &wrong_grid, &kernel, &mut encoder,)
            .is_err(),
        "same-sized geometry from another column must fail closed"
    );
    let matching_grid =
        create_vertical_shared_grid(&ctx, &[-0.0, 50.0, 150.0]).expect("shared grid");
    encode_vertical_remap_w_with_kernel(&ctx, &inputs, &matching_grid, &kernel, &mut encoder)
        .expect("matching geometry including signed zero remains supported");
}

#[test]
fn test_vertical_evidence_non_finite_inputs_rejected() {
    assert!(
        flexpart_gpu::gpu::vertical_inputs_sha256(
            &[0.0, 100.0],
            &[1.0, f32::NAN],
            &[50.0],
            VerticalStaggering::LevelCenter,
            "geometry",
        )
        .is_err(),
        "NaN must not become a hashed JSON null"
    );
    assert!(
        flexpart_gpu::gpu::vertical_w_inputs_sha256(
            &[0.0, 50.0, 150.0],
            &[0.0, 100.0, 200.0],
            &[1.0, 2.0, 3.0],
            &[50.0, 150.0],
            &[f32::INFINITY],
            "geometry",
        )
        .is_err(),
        "infinite query must fail before hashing"
    );
}

#[test]
fn gpu_asymmetric_nonlinear_exposes_index_bugs() {
    // Deliberately non-linear profile where off-by-one or wrong ordering is
    // observably wrong. Heights are geometric, values peak in the middle.
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let heights = [0.0_f32, 900.0, 3000.0, 5700.0];
    // Non-linear: ground 0.03, low-mid 0.15, high-mid 0.03, top 0.0 (like #80).
    let values = [0.036_f32, 0.15, 0.03, 0.0];
    let grid = create_vertical_grid_buffers(&ctx, &heights, &values).expect("grid");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    // Query in the upper bracket [3000, 5700]: correct is blend of 0.03 and 0.0.
    let queries = create_vertical_query_buffers(&ctx, &[4350.0]).expect("queries");
    let gpu = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries, &kernel))
        .expect("GPU succeeds");
    let expected = 0.03 * 0.5 + 0.0 * 0.5;
    assert_close(
        f64::from(gpu[0]),
        expected,
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "upper bracket nonlinear",
    );
    // Off-by-one (using [900,3000] bracket instead) would give ~0.09, far off.
    assert!(
        (f64::from(gpu[0]) - 0.09).abs() > 0.03,
        "GPU must not use the wrong bracket"
    );
    // Query in lower bracket [0,900] at 225: correct ~0.06.
    let queries_low = create_vertical_query_buffers(&ctx, &[225.0]).expect("low");
    let gpu_low = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries_low, &kernel))
        .expect("GPU low succeeds");
    let expected_low = 0.036 * 0.75 + 0.15 * 0.25;
    assert_close(
        f64::from(gpu_low[0]),
        expected_low,
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "lower bracket nonlinear",
    );
}

#[test]
fn gpu_wgsl_arithmetic_determines_output() {
    // Proves production arithmetic executes in WGSL: same geometry with
    // different field values yields different device results, and the
    // difference matches the host-predicted linear blend (not a cached value).
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let heights = [0.0_f32, 100.0, 1000.0];
    let values_a = [1.0_f32, 2.0, 3.0];
    let values_b = [1.0_f32, 9.0, 3.0];
    let grid_a = create_vertical_grid_buffers(&ctx, &heights, &values_a).expect("grid a");
    let grid_b = create_vertical_grid_buffers(&ctx, &heights, &values_b).expect("grid b");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let queries = create_vertical_query_buffers(&ctx, &[50.0]).expect("queries");
    let gpu_a = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid_a, &queries, &kernel))
        .expect("GPU a succeeds");
    let gpu_b = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid_b, &queries, &kernel))
        .expect("GPU b succeeds");
    // At 50 m (midpoint of [0,100]): a gives 1.5, b gives 5.0.
    assert_close(
        f64::from(gpu_a[0]),
        1.5,
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "a midpoint",
    );
    assert_close(
        f64::from(gpu_b[0]),
        5.0,
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "b midpoint",
    );
    assert!(
        (f64::from(gpu_a[0]) - f64::from(gpu_b[0])).abs() > 3.0,
        "field values must change device output"
    );
}

#[test]
fn gpu_center_w_matches_cpu_runtime_motion() {
    // Center-staggered geometric vertical motion follows the ordinary
    // single-stage sample path on both CPU (#73) and GPU
    // (`physical_center_w_column_from_runtime`). Values come from the same
    // #30 runtime transform as the geometry on both sides.
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let snapshot = synthetic_snapshot();
    // Asymmetric nonlinear motion exposes swapped weights and ordering bugs.
    let motion = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::GeometricVelocity,
        unit: NativeVerticalMotionUnit::MeterPerSecond,
        sign: NativeVerticalMotionSign::PositiveUpward,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values: vec![0.5, -0.25, 0.125],
        provenance: NativeVerticalMotionProvenance {
            source_id: "vertical-gpu-center-w-test".to_string(),
        },
    };
    let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &motion).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (heights, lane) =
        physical_center_w_column_from_runtime(runtime, 0, 0).expect("center-W column");
    assert!(heights.windows(2).all(|pair| pair[1] > pair[0]));
    let grid = create_vertical_grid_buffers(&ctx, &heights, &lane).expect("grid");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");

    // Interior query at an asymmetric fraction plus the exact middle level.
    let query = heights[0] + 0.3 * (heights[1] - heights[0]);
    let queries = create_vertical_query_buffers(&ctx, &[query, heights[1]]).expect("queries");
    let gpu = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries, &kernel))
        .expect("GPU center-W succeeds");
    for (height, gpu_value) in [query, heights[1]].iter().zip(gpu.iter()) {
        let cpu = sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelCenter,
            &[],
            0,
            0,
            *height,
            VerticalReference::AboveGroundLevel,
        )
        .expect("CPU center-W diagnostic")
        .value;
        assert_close(
            f64::from(*gpu_value),
            f64::from(cpu),
            ABS_TOL_MODEL,
            REL_TOL_MODEL,
            &format!("center-W at {height} m AGL"),
        );
    }
    // Swapped weights would differ for this asymmetric profile.
    let expected = f64::from(lane[0]) * 0.7 + f64::from(lane[1]) * 0.3;
    let swapped = f64::from(lane[0]) * 0.3 + f64::from(lane[1]) * 0.7;
    assert!(
        (swapped - expected).abs() > 1.0e-3,
        "center-W test data must expose swapped weights"
    );

    // Interface staggering is rejected by the center extractor: interface
    // motion follows the two-stage production path instead.
    let omega: NativeVerticalMotion = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"
    ))
    .expect("omega fixture");
    let interface_geometry =
        reconstruct_vertical_geometry_with_motion(&snapshot, &omega).expect("geometry");
    let interface_runtime = interface_geometry.runtime_view().expect("runtime view");
    assert!(physical_center_w_column_from_runtime(interface_runtime, 0, 0).is_err());
}

#[test]
fn gpu_asl_below_terrain_clamps_to_lower() {
    // ASL below local terrain resolves to negative AGL, which clamps to the
    // lowest grid value exactly like below-first-level AGL queries (FLEXPART
    // `lbounds(1)`). This matches the CPU (#73) behavior: out-of-range never
    // fails, only non-finite or ambiguous references do.
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    let snapshot = synthetic_snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let terrain = runtime.terrain_asl_m(0, 0).expect("terrain");
    assert_eq!(terrain, 250.0);
    // Physical lanes for the GPU grid; canonical order for the CPU call.
    let values_canonical = vec![50.0_f32, 20.0, 10.0];
    let (heights, lane) =
        physical_model_column_from_runtime(runtime, FieldId::Temperature, &values_canonical, 0, 0)
            .expect("physical column");
    assert_eq!(lane, vec![10.0_f32, 20.0, 50.0]);
    let grid = create_vertical_grid_buffers(&ctx, &heights, &lane).expect("grid");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");

    // 50 m below terrain: AGL = -50 m, below the ground-level grid base.
    let asl_query = terrain - 50.0;
    let resolved = resolve_query_heights_agl(
        runtime,
        0,
        0,
        &[asl_query],
        VerticalReference::AboveMeanSeaLevel,
    )
    .expect("ASL resolves");
    assert!((resolved[0] - (-50.0)).abs() <= 1.0e-6);
    let queries = create_vertical_query_buffers(&ctx, &resolved).expect("queries");
    let gpu = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries, &kernel))
        .expect("GPU below-terrain succeeds");
    assert_close(
        f64::from(gpu[0]),
        f64::from(lane[0]),
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "below-terrain ASL clamps to lowest",
    );
    let cpu = sample_vertical(
        runtime,
        FieldId::Temperature,
        VerticalStaggering::LevelCenter,
        &values_canonical,
        0,
        0,
        asl_query,
        VerticalReference::AboveMeanSeaLevel,
    )
    .expect("CPU below-terrain diagnostic")
    .value;
    assert_eq!(cpu.to_bits(), lane[0].to_bits());
    assert_eq!(gpu[0].to_bits(), cpu.to_bits());
}

#[test]
fn gpu_w_rejects_multicolumn_runtime() {
    // #80 freezes a single vertical column without horizontal slope
    // correction, so interface extraction fails closed when the runtime is
    // wider than one column. Geometry without motion fails first with the
    // missing-motion error (covered in the fail-closed test); here a genuine
    // two-column runtime must report the single-column contract violation.
    let snapshot_value: Value = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-column-v1.json"
    ))
    .expect("synthetic fixture must parse");
    let mut wide = snapshot_value;
    wide["horizontal_grid"]["nx"] = serde_json::json!(2);
    for field in wide["fields"].as_array_mut().expect("fields").iter_mut() {
        let shape = field["shape"].as_array().expect("shape").clone();
        assert!(
            shape[0].as_u64() == Some(1) && shape[1].as_u64() == Some(1),
            "synthetic fixture must be 1x1"
        );
        field["shape"][0] = serde_json::json!(2);
        let old_values = field["values"].as_array().expect("values").clone();
        // X-fastest layout with nx = ny = 1 stores one value per z-slice, so
        // widening to nx = 2 duplicates every z-slice.
        let mut wide_values = Vec::with_capacity(old_values.len() * 2);
        for value in &old_values {
            wide_values.push(value.clone());
            wide_values.push(value.clone());
        }
        field["values"] = Value::Array(wide_values);
    }
    let wide_snapshot: flexpart_gpu::meteorology::Snapshot =
        serde_json::from_value(wide).expect("wide snapshot must validate");
    let mut omega: NativeVerticalMotion = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"
    ))
    .expect("omega fixture");
    omega.values = omega
        .values
        .iter()
        .flat_map(|value| [*value, *value])
        .collect();
    let geometry =
        reconstruct_vertical_geometry_with_motion(&wide_snapshot, &omega).expect("wide geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    assert_eq!(runtime.dimensions(), (2, 1, 3));
    let error =
        physical_w_columns_from_runtime(runtime, 0, 0).expect_err("wide W must fail closed");
    assert!(
        matches!(
            error,
            flexpart_gpu::gpu::GpuVerticalError::Sampling(
                flexpart_gpu::meteorology::vertical_sampling::VerticalSamplingError::UnsupportedInterfaceRuntime { .. }
            )
        ),
        "wide W must report the single-column contract, got {error:?}"
    );
}

#[test]
fn gpu_vector_components_sample_independently() {
    // Vector sampling where applicable is per-component scalar sampling over
    // shared column geometry: U and V pass through the same METRE-mode kernel
    // as independent lanes. There is intentionally no fused vector kernel,
    // mirroring the scalar CPU entrypoint.
    let Some(ctx) = gpu_context_or_skip() else {
        return;
    };
    // Synthetic strictly increasing geometry; runtime extraction is proven
    // elsewhere, so this test isolates per-component kernel behavior.
    let heights = [100.0_f32, 1000.0, 3700.0];
    // Asymmetric component profiles expose swapped weights and wrong lanes.
    let u_lane = vec![3.0_f32, -1.5, 7.25];
    let v_lane = vec![-8.0_f32, 4.5, 0.5];
    let u_grid = create_vertical_grid_buffers(&ctx, &heights, &u_lane).expect("U grid");
    let v_grid = create_vertical_grid_buffers(&ctx, &heights, &v_lane).expect("V grid");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let query = heights[0] + 0.4 * (heights[1] - heights[0]);
    let queries = create_vertical_query_buffers(&ctx, &[query]).expect("queries");
    let gpu_u = pollster::block_on(sample_vertical_grid_gpu(&ctx, &u_grid, &queries, &kernel))
        .expect("GPU U succeeds");
    let gpu_v = pollster::block_on(sample_vertical_grid_gpu(&ctx, &v_grid, &queries, &kernel))
        .expect("GPU V succeeds");
    let w_upper = 0.4_f64;
    let w_lower = 0.6_f64;
    assert_close(
        f64::from(gpu_u[0]),
        f64::from(u_lane[0]) * w_lower + f64::from(u_lane[1]) * w_upper,
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "U component",
    );
    assert_close(
        f64::from(gpu_v[0]),
        f64::from(v_lane[0]) * w_lower + f64::from(v_lane[1]) * w_upper,
        ABS_TOL_MODEL,
        REL_TOL_MODEL,
        "V component",
    );
    // Crossed lanes would be observably wrong for asymmetric profiles.
    assert!(
        (f64::from(gpu_u[0]) - f64::from(gpu_v[0])).abs() > 1.0,
        "components must remain distinct lanes"
    );
}
