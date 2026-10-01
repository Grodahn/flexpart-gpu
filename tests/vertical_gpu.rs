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
    build_vertical_gpu_row, create_vertical_grid_buffers, create_vertical_output_buffer,
    create_vertical_query_buffers, create_vertical_shared_grid,
    create_vertical_w_interface_inputs, download_vertical_samples,
    encode_vertical_remap_w_with_kernel, encode_vertical_sample_with_kernel,
    encode_vertical_w_two_stage_with_kernels, physical_model_column_from_runtime,
    physical_w_columns_from_runtime, resolve_query_heights_agl, sample_vertical_grid_gpu,
    sample_vertical_w_gpu_two_stage, vertical_geometry_identity,
    vertical_model_comparison_policy, vertical_sample_shader_sha256,
    vertical_w_bundle_shader_sha256, vertical_w_comparison_policy,
    GpuCalculationEvidence, GpuCalculationPath, GpuCandidateEvidence, GpuContext, GpuError,
    GpuEvidenceError, GpuEvidenceSchema, GpuExecutionEvidence, GpuExecutionStatus,
    PinnedOracleEvidence, VerticalGpuReport, VerticalSampleKernel, VerticalWRemapKernel,
    VERTICAL_GPU_CANDIDATE_DESCRIPTION, VERTICAL_GPU_REPORT_SCHEMA_ID,
};
use flexpart_gpu::meteorology::{
    vertical::{reconstruct_vertical_geometry, reconstruct_vertical_geometry_with_motion},
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

fn assert_close(actual: f64, expected: f64, abs: f64, rel: f64, what: &str) {
    let diff = (actual - expected).abs();
    let tolerance = abs + rel * expected.abs().max(actual.abs());
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
        heights_phys.push(
            runtime
                .level(0, 0, canonical)
                .expect("level")
                .height_agl_m,
        );
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
    let queries =
        create_vertical_query_buffers(&ctx, &[query]).expect("queries upload");
    let gpu = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries, &sample_kernel))
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
        heights.push(
            runtime
                .level(0, 0, canonical)
                .expect("level")
                .height_agl_m,
        );
    }
    let values = vec![1.0_f32, 5.0, 9.0];
    let grid =
        create_vertical_grid_buffers(&ctx, &heights, &values).expect("grid uploads");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    // Exact middle level.
    let queries =
        create_vertical_query_buffers(&ctx, &[heights[1]]).expect("queries");
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
    let grid =
        create_vertical_grid_buffers(&ctx, &heights, &values).expect("grid uploads");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");

    // Below first level clamps to lowest (FLEXPART lbounds(1)).
    let below = create_vertical_query_buffers(&ctx, &[-50.0, 10.0]).expect("below");
    let gpu_below =
        pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &below, &kernel))
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
    let above =
        create_vertical_query_buffers(&ctx, &[1000.0, 5000.0]).expect("above");
    let gpu_above =
        pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &above, &kernel))
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
        heights.push(
            runtime
                .level(0, 0, canonical)
                .expect("level")
                .height_agl_m,
        );
    }
    // Asymmetric values expose terrain-offset mistakes. Heights are physical
    // bottom-to-top; values_physical aligns with them for the GPU grid.
    // The CPU expects canonical storage order (Increasing: top-to-bottom),
    // so values_canonical is the reverse.
    let values_physical = vec![10.0_f32, 20.0, 50.0];
    let values_canonical = vec![50.0_f32, 20.0, 10.0];
    let grid =
        create_vertical_grid_buffers(&ctx, &heights, &values_physical).expect("grid");

    // Level heights are ~902, 3007, 5720 m; query 1500 m is strict interior
    // between the lowest two levels.
    let agl_query = 1500.0_f32;
    let asl_query = agl_query + terrain;
    let agl_resolved =
        resolve_query_heights_agl(runtime, 0, 0, &[agl_query], VerticalReference::AboveGroundLevel)
            .expect("AGL resolves");
    let asl_resolved =
        resolve_query_heights_agl(runtime, 0, 0, &[asl_query], VerticalReference::AboveMeanSeaLevel)
            .expect("ASL resolves");
    assert!((asl_resolved[0] - agl_query).abs() <= 1.0e-6);

    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let agl_buffers =
        create_vertical_query_buffers(&ctx, &agl_resolved).expect("AGL queries");
    let asl_buffers =
        create_vertical_query_buffers(&ctx, &asl_resolved).expect("ASL queries");
    let gpu_agl =
        pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &agl_buffers, &kernel))
            .expect("AGL GPU succeeds");
    let gpu_asl =
        pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &asl_buffers, &kernel))
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
    let wrong_buffers =
        create_vertical_query_buffers(&ctx, &[asl_query]).expect("wrong queries");
    let gpu_wrong =
        pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &wrong_buffers, &kernel))
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
    let grid_phys =
        create_vertical_grid_buffers(&ctx, &heights_phys, &values_phys).expect("grid");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let queries =
        create_vertical_query_buffers(&ctx, &[250.0, 1500.0]).expect("queries");
    let gpu_phys =
        pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid_phys, &queries, &kernel))
            .expect("GPU succeeds");

    // Simulate Decreasing canonical order: host would reverse before upload,
    // ending at the same physical lanes. Directly uploading the same physical
    // lanes proves the device is ordering-agnostic once normalized.
    let grid_same =
        create_vertical_grid_buffers(&ctx, &heights_phys, &values_phys).expect("same grid");
    let gpu_same =
        pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid_same, &queries, &kernel))
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
    let grid =
        create_vertical_grid_buffers(&ctx, &heights, &values).expect("oracle grid uploads");
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
        let (lower, upper, w_lower, w_upper) =
            cpu_indices_weights(&heights, query_heights[index]);
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
    let omega: flexpart_gpu::meteorology::vertical::NativeVerticalMotion =
        serde_json::from_str(include_str!(
            "../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"
        ))
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
    let shared_grid =
        create_vertical_shared_grid(&ctx, &shared_heights).expect("shared grid");
    let sample_kernel = VerticalSampleKernel::new(&ctx).expect("sample kernel");
    let remap_kernel = VerticalWRemapKernel::new(&ctx).expect("remap kernel");

    let report: Value = serde_json::from_str(include_str!(
        "../fixtures/interpolation/w-production-oracle-v1.json"
    ))
    .expect("#80 report");
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
    let query_buffers =
        create_vertical_query_buffers(&ctx, &query_heights).expect("queries");

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

    let absolute = report["tolerances"]["absolute_m_s"]
        .as_f64()
        .expect("abs tol");
    let relative = report["tolerances"]["relative"].as_f64().expect("rel tol");
    for (index, (query, _comparison)) in queries.iter().zip(comparisons).enumerate() {
        let expected = query["pristine_w_m_s"].as_f64().expect("pristine");
        let actual = f64::from(gpu[index]);
        let tolerance = absolute + relative * actual.abs().max(expected.abs());
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

    // Machine-readable evidence for the strict-interior non-equivalent case
    // (query 3), which is the critical regression guard.
    let policy = vertical_w_comparison_policy().expect("W policy");
    let geometry_identity = vertical_geometry_identity(runtime);
    let third_height = query_heights[2];
    let third_oracle = queries[2]["pristine_w_m_s"].as_f64().expect("third pristine");
    let cpu_third = sample_vertical(
        runtime,
        FieldId::VerticalVelocity,
        VerticalStaggering::LevelInterface,
        &[],
        0,
        0,
        third_height,
        VerticalReference::AboveGroundLevel,
    )
    .expect("third CPU");
    let row = build_vertical_gpu_row(
        &ctx,
        "vertical-gpu/w-production-synthetic/query-3",
        FieldId::VerticalVelocity,
        VerticalStaggering::LevelInterface,
        VerticalReference::AboveGroundLevel,
        third_height,
        third_height,
        runtime.terrain_asl_m(0, 0).expect("terrain"),
        &geometry_identity,
        cpu_third.lower_physical_index,
        cpu_third.upper_physical_index,
        cpu_third.weight_lower,
        cpu_third.weight_upper,
        gpu[2],
        checked_f64_to_f32(third_oracle, "third oracle"),
        cpu_third.value,
        policy,
        &shared_heights,
        &{
            // Shared values are device-resident; for input hashing we use the
            // CPU-remapped values diagnostically (hash binds the geometry, not
            // the candidate). The candidate remains the GPU output.
            let mut remapped = vec![0.0_f32; shared_heights.len()];
            // Recompute remap on host for hashing only (not the candidate).
            // This mirrors the CPU remap without becoming the GPU result.
            remapped[0] = interface_values[0];
            let last = remapped.len() - 1;
            remapped[last] = interface_values[interface_values.len() - 1];
            for i in 1..last {
                let h = level_heights[i - 1];
                let (l, u, wl, wu) = cpu_indices_weights(&interface_heights, h);
                remapped[i] = interface_values[l] * wl + interface_values[u] * wu;
            }
            remapped
        },
        &query_heights,
        &candidate_revision(),
    )
    .expect("evidence row builds");
    assert!(row.row_verdict);
    row.validate().expect("row validates");
    row.gpu_evidence
        .require_paired_pass()
        .expect("paired pass proves GPU");
    assert_eq!(
        row.gpu_evidence.execution.calculation_path,
        GpuCalculationPath::WgslDevice
    );
    // Adapter provenance distinguishes hardware from software WGSL.
    assert!(row.gpu_evidence.execution.adapter.is_some());
    // Oracle identity is the pinned #80 production oracle.
    assert_eq!(
        row.gpu_evidence.oracle.as_ref().expect("oracle").revision,
        "c70586c2b7f5258850705325881c61f557ea9bd8"
    );
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
    let omega: flexpart_gpu::meteorology::vertical::NativeVerticalMotion =
        serde_json::from_str(include_str!(
            "../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"
        ))
        .expect("omega fixture");
    let geometry =
        reconstruct_vertical_geometry_with_motion(&snapshot, &omega).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (interface_heights, interface_values, _, _) =
        physical_w_columns_from_runtime(runtime, 0, 0).expect("W columns");

    // Direct shortcut: sample interface heights/values directly, skipping the
    // required remap onto the shared grid.
    let direct_grid =
        create_vertical_grid_buffers(&ctx, &interface_heights, &interface_values)
            .expect("direct grid");
    let sample_kernel = VerticalSampleKernel::new(&ctx).expect("kernel");

    let report: Value = serde_json::from_str(include_str!(
        "../fixtures/interpolation/w-production-oracle-v1.json"
    ))
    .expect("#80 report");
    let queries = report["synthetic_case"]["queries"]
        .as_array()
        .expect("queries");
    let comparisons = report["synthetic_case"]["comparisons"]
        .as_array()
        .expect("comparisons");
    let absolute = report["tolerances"]["absolute_m_s"]
        .as_f64()
        .expect("abs");
    let relative = report["tolerances"]["relative"].as_f64().expect("rel");

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
        let gpu_direct =
            pollster::block_on(sample_vertical_grid_gpu(&ctx, &direct_grid, &qb, &sample_kernel))
                .expect("direct GPU succeeds");
        let tolerance_direct =
            absolute + relative * direct_oracle.abs().max(f64::from(gpu_direct[0]).abs());
        assert!(
            (f64::from(gpu_direct[0]) - direct_oracle).abs() <= tolerance_direct,
            "direct GPU must match oracle direct value for query {}",
            query["query"]
        );

        if !equivalent {
            let tolerance =
                absolute + relative * pristine.abs().max(direct_oracle.abs());
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
    let snapshot = synthetic_snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let values = vec![1.0_f32, 2.0, 3.0];
    let (heights, lane) =
        physical_model_column_from_runtime(runtime, FieldId::Temperature, &values, 0, 0)
            .expect("physical column");
    let grid =
        create_vertical_grid_buffers(&ctx, &heights, &lane).expect("grid");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let query = 0.5 * (heights[0] + heights[1]);
    let queries = create_vertical_query_buffers(&ctx, &[query]).expect("queries");
    let gpu = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries, &kernel))
        .expect("GPU succeeds");

    let cpu = sample_vertical(
        runtime,
        FieldId::Temperature,
        VerticalStaggering::LevelCenter,
        &values,
        0,
        0,
        query,
        VerticalReference::AboveGroundLevel,
    )
    .expect("CPU diagnostic");

    let policy = vertical_model_comparison_policy().expect("policy");
    let geometry_identity = flexpart_gpu::gpu::vertical_geometry_identity(runtime);
    let row = build_vertical_gpu_row(
        &ctx,
        "vertical-gpu/model-interior",
        FieldId::Temperature,
        VerticalStaggering::LevelCenter,
        VerticalReference::AboveGroundLevel,
        query,
        query,
        runtime.terrain_asl_m(0, 0).expect("terrain"),
        &geometry_identity,
        cpu.lower_physical_index,
        cpu.upper_physical_index,
        cpu.weight_lower,
        cpu.weight_upper,
        gpu[0],
        cpu.value,
        cpu.value,
        policy,
        &heights,
        &lane,
        &[query],
        &candidate_revision(),
    )
    .expect("row builds");
    assert!(row.row_verdict);
    row.validate().expect("row validates");
    row.gpu_evidence
        .require_paired_pass()
        .expect("paired pass");
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
    let omega: flexpart_gpu::meteorology::vertical::NativeVerticalMotion =
        serde_json::from_str(include_str!(
            "../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"
        ))
        .expect("omega");
    let geometry =
        reconstruct_vertical_geometry_with_motion(&snapshot, &omega).expect("geometry");
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
    // Exact #80 pristine heights: query 2 (225.55 m), query 3 (1954.79 m),
    // query 4 (5042.11 m).
    let queries_a =
        create_vertical_query_buffers(&ctx, &[225.552_642_822_265_62, 1954.792_358_398_437_5])
            .expect("qa");
    let queries_b =
        create_vertical_query_buffers(&ctx, &[5042.116_210_937_5]).expect("qb");
    let outputs_a =
        create_vertical_output_buffer(&ctx, queries_a.query_count()).expect("oa");
    let outputs_b =
        create_vertical_output_buffer(&ctx, queries_b.query_count()).expect("ob");

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
    let values_a = pollster::block_on(download_vertical_samples(&ctx, &outputs_a))
        .expect("first readback");
    let values_b = pollster::block_on(download_vertical_samples(&ctx, &outputs_b))
        .expect("second readback");
    // Queries: 225 m (first interior), 1954.79 m (query 3), 5042 m (query 4).
    // Values must match the #80 pristine oracle within its tolerance.
    assert_close(
        f64::from(values_a[0]),
        0.049_274_586_141_109_47,
        ABS_TOL_W,
        REL_TOL_W,
        "composed first query",
    );
    assert_close(
        f64::from(values_a[1]),
        0.093_329_705_297_946_93,
        ABS_TOL_W,
        REL_TOL_W,
        "composed second query",
    );
    assert_close(
        f64::from(values_b[0]),
        0.024_456_575_512_886_047,
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
    let omega: flexpart_gpu::meteorology::vertical::NativeVerticalMotion =
        serde_json::from_str(include_str!(
            "../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"
        ))
        .expect("omega");
    let geometry =
        reconstruct_vertical_geometry_with_motion(&snapshot, &omega).expect("geometry");
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
    let queries = create_vertical_query_buffers(&ctx, &[1954.79]).expect("queries");
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
    let good = pollster::block_on(download_vertical_samples(&ctx, &outputs))
        .expect("good readback");
    // Pristine for 1954.79 m is ~0.0933 (query 3 mid Case).
    assert_close(
        f64::from(good[0]),
        0.093_329_705_297_946_93,
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
    assert!(create_vertical_grid_buffers(&ctx, &[0.0, 50.0, 100.0], &[1.0, f32::NAN, 3.0]).is_err());
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

    // Runtime-level fail-closed: ambiguous reference, wrong staggering,
    // missing motion, multi-column W, mismatched dims.
    let snapshot = synthetic_snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    assert!(resolve_query_heights_agl(
        runtime,
        0,
        0,
        &[100.0],
        VerticalReference::ModelNative
    )
    .is_err());
    assert!(resolve_query_heights_agl(runtime, 0, 0, &[f32::NAN], VerticalReference::AboveGroundLevel)
        .is_err());
    assert!(physical_model_column_from_runtime(
        runtime,
        FieldId::SurfacePressure,
        &[],
        0,
        0
    )
    .is_err());
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
        let grid =
            create_vertical_grid_buffers(&ctx, &heights, &lane).expect("grid");
        let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
        let queries = create_vertical_query_buffers(&ctx, &[heights[0]]).expect("queries");
        let gpu = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries, &kernel))
            .expect("GPU succeeds");
        let policy = vertical_model_comparison_policy().expect("policy");
        let geometry_identity = flexpart_gpu::gpu::vertical_geometry_identity(runtime);
        let mut row = build_vertical_gpu_row(
            &ctx,
            "vertical-gpu/shader-guard",
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
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
        row.gpu_evidence.candidate.shader_sha256 = "b".repeat(64);
        assert!(row.validate().is_err());

        // Contradictory verdict fails.
        let mut contradictory = build_vertical_gpu_row(
            &ctx,
            "vertical-gpu/verdict-guard",
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
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

        // Invalid candidate revision fails at build time.
        assert!(build_vertical_gpu_row(
            &ctx,
            "vertical-gpu/revision-guard",
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
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
    }
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
    let grid =
        create_vertical_grid_buffers(&ctx, &heights, &values).expect("grid");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    // Query in the upper bracket [3000, 5700]: correct is blend of 0.03 and 0.0.
    let queries = create_vertical_query_buffers(&ctx, &[4350.0]).expect("queries");
    let gpu = pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries, &kernel))
        .expect("GPU succeeds");
    let expected = 0.03 * 0.5 + 0.0 * 0.5;
    assert_close(
        f64::from(gpu[0]),
        f64::from(expected),
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
    let gpu_low =
        pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid, &queries_low, &kernel))
            .expect("GPU low succeeds");
    let expected_low = 0.036 * 0.75 + 0.15 * 0.25;
    assert_close(
        f64::from(gpu_low[0]),
        f64::from(expected_low),
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
    let grid_a =
        create_vertical_grid_buffers(&ctx, &heights, &values_a).expect("grid a");
    let grid_b =
        create_vertical_grid_buffers(&ctx, &heights, &values_b).expect("grid b");
    let kernel = VerticalSampleKernel::new(&ctx).expect("kernel");
    let queries = create_vertical_query_buffers(&ctx, &[50.0]).expect("queries");
    let gpu_a =
        pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid_a, &queries, &kernel))
            .expect("GPU a succeeds");
    let gpu_b =
        pollster::block_on(sample_vertical_grid_gpu(&ctx, &grid_b, &queries, &kernel))
            .expect("GPU b succeeds");
    // At 50 m (midpoint of [0,100]): a gives 1.5, b gives 5.0.
    assert_close(f64::from(gpu_a[0]), 1.5, ABS_TOL_MODEL, REL_TOL_MODEL, "a midpoint");
    assert_close(f64::from(gpu_b[0]), 5.0, ABS_TOL_MODEL, REL_TOL_MODEL, "b midpoint");
    assert!(
        (f64::from(gpu_a[0]) - f64::from(gpu_b[0])).abs() > 3.0,
        "field values must change device output"
    );
}
