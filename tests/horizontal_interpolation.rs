//! Issue #72: candidate validation against #71 horizontal oracle fixtures.
//!
//! The candidate is `meteorology::horizontal` (x/y bilinear sampling only).
//! This test reproduces every frozen horizontal query from
//! `fixtures/interpolation/contract-v1.json`, compares candidate indices,
//! weights and values against the direct FLEXPART oracle, and emits a
//! machine-readable comparison report. Synthetic linear fields with
//! hand-computable expectations are covered alongside the oracle fixtures.

use flexpart_gpu::meteorology::{
    horizontal::{
        grid_index_to_lonlat, sample_horizontal, sample_horizontal_geographic, HorizontalError,
    },
    HorizontalGrid, HorizontalStaggering, LongitudeDomain,
};
use serde::Serialize;
use std::collections::HashSet;

const REL_TOL: f64 = 1.0e-5;
const ABS_TOL: f64 = 1.0e-6;

fn tolerance(oracle: f64) -> f64 {
    ABS_TOL + REL_TOL * oracle.abs()
}

fn is_close(candidate: f64, oracle: f64) -> bool {
    (candidate - oracle).abs() <= tolerance(oracle)
}

fn assert_close(candidate: f64, oracle: f64, what: &str) {
    let diff = (candidate - oracle).abs();
    let allowed = tolerance(oracle);
    assert!(
        diff <= allowed,
        "{what}: candidate {candidate} != oracle {oracle} (diff {diff}, tolerance {allowed})"
    );
}

/// One machine-readable comparison row required by #72.
#[derive(Debug, Clone, Serialize)]
struct ComparisonRow {
    field_identity: String,
    case_id: String,
    source_grid: SourceGrid,
    sample_lon_deg: f64,
    sample_lat_deg: f64,
    sample_xt: f64,
    sample_yt: f64,
    candidate_indices: [usize; 4],
    oracle_indices: [usize; 4],
    candidate_weights: [f64; 4],
    oracle_weights: [f64; 4],
    oracle_value: f64,
    candidate_value: f64,
    tolerance: f64,
    verdict: String,
}

#[derive(Debug, Clone, Serialize)]
struct SourceGrid {
    nx: usize,
    ny: usize,
    xlon0_deg: f64,
    ylat0_deg: f64,
    dx_deg: f64,
    dy_deg: f64,
    periodic_x: bool,
    longitude_domain: LongitudeDomain,
    staggering: HorizontalStaggering,
}

#[derive(Debug, Serialize)]
struct ComparisonReport {
    contract: String,
    candidate: String,
    tolerance_rule: String,
    verdict: String,
    failed_rows: usize,
    rows: Vec<ComparisonRow>,
}

fn comparison_passes(
    candidate_indices: [usize; 4],
    oracle_indices: [usize; 4],
    candidate_weights: [f64; 4],
    oracle_weights: [f64; 4],
    candidate_value: f64,
    oracle_value: f64,
) -> bool {
    candidate_indices == oracle_indices
        && candidate_weights
            .iter()
            .zip(oracle_weights)
            .all(|(candidate, oracle)| is_close(*candidate, oracle))
        && is_close(candidate_value, oracle_value)
}

fn parse_f64(token: &str, what: &str) -> f64 {
    token
        .trim()
        .parse::<f64>()
        .unwrap_or_else(|_| panic!("invalid f64 in {what}: {token:?}"))
}

fn tokens(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn grid_from_tokens(
    nx: usize,
    ny: usize,
    grid_tokens: &[String],
    longitude_domain: LongitudeDomain,
) -> HorizontalGrid {
    HorizontalGrid {
        nx,
        ny,
        xlon0_deg: parse_f64(&grid_tokens[0], "xlon0"),
        ylat0_deg: parse_f64(&grid_tokens[1], "ylat0"),
        dx_deg: parse_f64(&grid_tokens[2], "dx"),
        dy_deg: parse_f64(&grid_tokens[3], "dy"),
        longitude_domain,
    }
}

fn read_field(input: &[String], cursor: &mut usize, count: usize) -> Vec<f32> {
    let start = *cursor;
    *cursor += count;
    assert!(
        *cursor <= input.len(),
        "fixture input truncated reading field"
    );
    input[start..*cursor]
        .iter()
        .map(|line| {
            line.trim()
                .parse::<f32>()
                .unwrap_or_else(|_| panic!("invalid f32 field value: {line:?}"))
        })
        .collect()
}

fn check_horizontal_case(case: &serde_json::Value, rows: &mut Vec<ComparisonRow>) {
    let case_id = case["id"].as_str().expect("case id").to_string();
    let input = case["input"].as_array().expect("input array");
    let input: Vec<String> = input
        .iter()
        .map(|v| v.as_str().expect("input line").to_string())
        .collect();
    let golden = &case["golden"];
    let queries = golden["queries"].as_array().expect("queries");

    let dims = tokens(&input[1]);
    let nx: usize = dims[0].parse().expect("nx");
    let ny: usize = dims[1].parse().expect("ny");
    let grid_tokens = tokens(&input[2]);
    let longitude_domain = if case_id == "horizontal-periodic-wrap" {
        LongitudeDomain::ZeroTo360
    } else {
        LongitudeDomain::Minus180To180
    };
    let grid = grid_from_tokens(nx, ny, &grid_tokens, longitude_domain);
    let mut cursor = 4;
    let field = read_field(&input, &mut cursor, nx * ny);
    let nquery: usize = input[cursor].parse().expect("nquery");
    cursor += 1;
    assert_eq!(queries.len(), nquery, "{case_id}: query count");

    let periodic_flag = tokens(&input[3])[0].parse::<usize>().expect("periodic");
    let _ = periodic_flag;

    for (index, query) in queries.iter().enumerate() {
        let query_tokens = tokens(&input[cursor + index]);
        let xt = parse_f64(&query_tokens[0], "xt");
        let yt = parse_f64(&query_tokens[1], "yt");

        let sample = sample_horizontal(&grid, &field, HorizontalStaggering::CellCenter, xt, yt)
            .unwrap_or_else(|err| panic!("{case_id} query {index} must sample: {err:?}"));

        let oracle_indices = query["INDICES"].as_array().expect("INDICES");
        let oracle_indices: [usize; 4] = [
            oracle_indices[0].as_u64().expect("ix") as usize,
            oracle_indices[1].as_u64().expect("jy") as usize,
            oracle_indices[2].as_u64().expect("ixp") as usize,
            oracle_indices[3].as_u64().expect("jyp") as usize,
        ];
        let oracle_weights = query["WEIGHTS"].as_array().expect("WEIGHTS");
        let oracle_weights: [f64; 4] = [
            oracle_weights[0].as_f64().expect("p1"),
            oracle_weights[1].as_f64().expect("p2"),
            oracle_weights[2].as_f64().expect("p3"),
            oracle_weights[3].as_f64().expect("p4"),
        ];
        let oracle_value = query["VALUE"][0].as_f64().expect("VALUE");
        let candidate_value = f64::from(sample.value);

        let candidate_indices = [sample.ix, sample.jy, sample.ixp, sample.jyp];
        let passed = comparison_passes(
            candidate_indices,
            oracle_indices,
            sample.weights,
            oracle_weights,
            candidate_value,
            oracle_value,
        );

        let (lon, lat) = grid_index_to_lonlat(&grid, sample.xt, sample.yt);
        rows.push(ComparisonRow {
            field_identity: format!("{case_id}:arbitrary_scalar"),
            case_id: case_id.clone(),
            source_grid: SourceGrid {
                nx,
                ny,
                xlon0_deg: grid.xlon0_deg,
                ylat0_deg: grid.ylat0_deg,
                dx_deg: grid.dx_deg,
                dy_deg: grid.dy_deg,
                periodic_x: sample.is_periodic_x,
                longitude_domain: grid.longitude_domain,
                staggering: HorizontalStaggering::CellCenter,
            },
            sample_lon_deg: lon,
            sample_lat_deg: lat,
            sample_xt: sample.xt,
            sample_yt: sample.yt,
            candidate_indices,
            oracle_indices,
            candidate_weights: sample.weights,
            oracle_weights,
            oracle_value,
            candidate_value,
            tolerance: tolerance(oracle_value),
            verdict: if passed { "pass" } else { "fail" }.to_string(),
        });
    }
}

fn check_geographic_case(case: &serde_json::Value, rows: &mut Vec<ComparisonRow>) {
    let case_id = case["id"].as_str().expect("case id").to_string();
    let input = case["input"].as_array().expect("input array");
    let input: Vec<String> = input
        .iter()
        .map(|v| v.as_str().expect("input line").to_string())
        .collect();
    let golden = &case["golden"];
    let queries = golden["queries"].as_array().expect("queries");

    let dims = tokens(&input[1]);
    let nx: usize = dims[0].parse().expect("nx");
    let ny: usize = dims[1].parse().expect("ny");
    let grid_tokens = tokens(&input[2]);
    let grid = grid_from_tokens(nx, ny, &grid_tokens, LongitudeDomain::Minus180To180);
    let mut cursor = 4;
    let field = read_field(&input, &mut cursor, nx * ny);
    let nquery: usize = input[cursor].parse().expect("nquery");
    cursor += 1;
    assert_eq!(queries.len(), nquery, "{case_id}: query count");

    for (index, query) in queries.iter().enumerate() {
        let query_tokens = tokens(&input[cursor + index]);
        let lon = parse_f64(&query_tokens[0], "lon");
        let lat = parse_f64(&query_tokens[1], "lat");

        let sample =
            sample_horizontal_geographic(&grid, &field, HorizontalStaggering::CellCenter, lon, lat)
                .unwrap_or_else(|err| panic!("{case_id} query {index} must sample: {err:?}"));

        let oracle_lon = query["LONLAT"][0].as_f64().expect("LONLAT lon");
        let oracle_lat = query["LONLAT"][1].as_f64().expect("LONLAT lat");
        assert_close(lon, oracle_lon, &format!("{case_id} query {index} lon"));
        assert_close(lat, oracle_lat, &format!("{case_id} query {index} lat"));

        let oracle_xy = query["XY"].as_array().expect("XY");
        assert_close(
            sample.xt,
            oracle_xy[0].as_f64().expect("xt"),
            &format!("{case_id} query {index} coordtrafo xt"),
        );
        assert_close(
            sample.yt,
            oracle_xy[1].as_f64().expect("yt"),
            &format!("{case_id} query {index} coordtrafo yt"),
        );

        let oracle_indices = query["INDICES"].as_array().expect("INDICES");
        let oracle_indices: [usize; 4] = [
            oracle_indices[0].as_u64().expect("ix") as usize,
            oracle_indices[1].as_u64().expect("jy") as usize,
            oracle_indices[2].as_u64().expect("ixp") as usize,
            oracle_indices[3].as_u64().expect("jyp") as usize,
        ];
        let oracle_weights = query["WEIGHTS"].as_array().expect("WEIGHTS");
        let oracle_weights: [f64; 4] = [
            oracle_weights[0].as_f64().expect("p1"),
            oracle_weights[1].as_f64().expect("p2"),
            oracle_weights[2].as_f64().expect("p3"),
            oracle_weights[3].as_f64().expect("p4"),
        ];
        let oracle_value = query["VALUE"][0].as_f64().expect("VALUE");
        let candidate_value = f64::from(sample.value);

        let candidate_indices = [sample.ix, sample.jy, sample.ixp, sample.jyp];
        let passed = comparison_passes(
            candidate_indices,
            oracle_indices,
            sample.weights,
            oracle_weights,
            candidate_value,
            oracle_value,
        );

        rows.push(ComparisonRow {
            field_identity: format!("{case_id}:arbitrary_scalar"),
            case_id: case_id.clone(),
            source_grid: SourceGrid {
                nx,
                ny,
                xlon0_deg: grid.xlon0_deg,
                ylat0_deg: grid.ylat0_deg,
                dx_deg: grid.dx_deg,
                dy_deg: grid.dy_deg,
                periodic_x: sample.is_periodic_x,
                longitude_domain: grid.longitude_domain,
                staggering: HorizontalStaggering::CellCenter,
            },
            sample_lon_deg: lon,
            sample_lat_deg: lat,
            sample_xt: sample.xt,
            sample_yt: sample.yt,
            candidate_indices,
            oracle_indices,
            candidate_weights: sample.weights,
            oracle_weights,
            oracle_value,
            candidate_value,
            tolerance: tolerance(oracle_value),
            verdict: if passed { "pass" } else { "fail" }.to_string(),
        });
    }
}

fn check_synthetic_linear_rows(rows: &mut Vec<ComparisonRow>) {
    let grid = HorizontalGrid {
        nx: 4,
        ny: 3,
        xlon0_deg: 0.0,
        ylat0_deg: 0.0,
        dx_deg: 1.0,
        dy_deg: 1.0,
        longitude_domain: LongitudeDomain::Minus180To180,
    };
    // Hand-computable field: f(x,y) = 7 + 2x + 3y.
    let field: Vec<f32> = (0..3)
        .flat_map(|y| (0..4).map(move |x| 7.0 + 2.0 * x as f32 + 3.0 * y as f32))
        .collect();
    for (xt, yt) in [(0.0, 0.0), (1.25, 0.5), (2.5, 1.5), (2.9, 2.0), (0.4, 1.0)] {
        let expected = 7.0 + 2.0 * xt + 3.0 * yt;
        let sample = sample_horizontal(&grid, &field, HorizontalStaggering::CellCenter, xt, yt)
            .expect("synthetic linear query must succeed");
        let candidate_value = f64::from(sample.value);
        let candidate_indices = [sample.ix, sample.jy, sample.ixp, sample.jyp];
        let passed = comparison_passes(
            candidate_indices,
            candidate_indices,
            sample.weights,
            sample.weights,
            candidate_value,
            expected,
        );
        let (lon, lat) = grid_index_to_lonlat(&grid, sample.xt, sample.yt);
        rows.push(ComparisonRow {
            field_identity: "synthetic-linear:f(x,y)=7+2x+3y".to_string(),
            case_id: "synthetic-linear".to_string(),
            source_grid: SourceGrid {
                nx: grid.nx,
                ny: grid.ny,
                xlon0_deg: grid.xlon0_deg,
                ylat0_deg: grid.ylat0_deg,
                dx_deg: grid.dx_deg,
                dy_deg: grid.dy_deg,
                periodic_x: sample.is_periodic_x,
                longitude_domain: grid.longitude_domain,
                staggering: HorizontalStaggering::CellCenter,
            },
            sample_lon_deg: lon,
            sample_lat_deg: lat,
            sample_xt: sample.xt,
            sample_yt: sample.yt,
            candidate_indices,
            oracle_indices: candidate_indices,
            candidate_weights: sample.weights,
            oracle_weights: sample.weights,
            oracle_value: expected,
            candidate_value,
            tolerance: tolerance(expected),
            verdict: if passed { "pass" } else { "fail" }.to_string(),
        });
    }
}

#[test]
fn test_horizontal_candidate_matches_frozen_oracle() {
    let out_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("horizontal-comparison-report.json");
    match std::fs::remove_file(&out_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("remove stale comparison report: {error}"),
    }

    let source = include_str!("../fixtures/interpolation/contract-v1.json");
    let contract: serde_json::Value =
        serde_json::from_str(source).expect("parse interpolation contract");
    let cases = contract["cases"].as_array().expect("cases");

    let mut seen = HashSet::new();
    let mut rows: Vec<ComparisonRow> = Vec::new();

    for case in cases {
        let id = case["id"].as_str().expect("case id");
        let mode = case["mode"].as_str().expect("mode");
        match (id, mode) {
            ("horizontal-interior", "horizontal") | ("horizontal-periodic-wrap", "horizontal") => {
                assert!(seen.insert(id), "duplicate horizontal case {id}");
                check_horizontal_case(case, &mut rows);
            }
            ("horizontal-geographic-interior", "horizontal_geographic") => {
                assert!(seen.insert(id), "duplicate geographic case {id}");
                check_geographic_case(case, &mut rows);
            }
            _ => {}
        }
    }

    assert!(seen.contains("horizontal-interior"));
    assert!(seen.contains("horizontal-periodic-wrap"));
    assert!(seen.contains("horizontal-geographic-interior"));

    check_synthetic_linear_rows(&mut rows);

    let failed_rows = rows.iter().filter(|row| row.verdict == "fail").count();
    let report = ComparisonReport {
        contract: "fixtures/interpolation/contract-v1.json".to_string(),
        candidate: "meteorology::horizontal::sample_horizontal[_geographic]".to_string(),
        tolerance_rule: "ABS_TOL 1e-6 + REL_TOL 1e-5 * |oracle|".to_string(),
        verdict: if failed_rows == 0 { "pass" } else { "fail" }.to_string(),
        failed_rows,
        rows,
    };
    let encoded = serde_json::to_string_pretty(&report).expect("serialize report");
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).expect("create target dir");
    }
    std::fs::write(&out_path, encoded).expect("write comparison report");

    let report_value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out_path).expect("read report"))
            .expect("report must be valid JSON");
    let report_rows = report_value["rows"].as_array().expect("rows");
    assert!(
        !report_rows.is_empty(),
        "comparison report must not be empty"
    );
    for row in report_rows {
        for key in [
            "field_identity",
            "source_grid",
            "sample_lon_deg",
            "sample_lat_deg",
            "candidate_indices",
            "oracle_indices",
            "candidate_weights",
            "oracle_weights",
            "oracle_value",
            "candidate_value",
            "tolerance",
            "verdict",
        ] {
            assert!(row.get(key).is_some(), "comparison row must record {key}");
        }
        for key in ["longitude_domain", "staggering"] {
            assert!(
                row["source_grid"].get(key).is_some(),
                "source grid must record {key}"
            );
        }
        let sample_lon_deg = row["sample_lon_deg"].as_f64().expect("sample longitude");
        match row["source_grid"]["longitude_domain"]
            .as_str()
            .expect("longitude domain")
        {
            "minus180_to180" => assert!(
                (-180.0..=180.0).contains(&sample_lon_deg),
                "reported longitude must satisfy minus180_to180"
            ),
            "zero_to360" => assert!(
                (0.0..360.0).contains(&sample_lon_deg),
                "reported longitude must satisfy zero_to360"
            ),
            domain => panic!("unexpected longitude domain {domain}"),
        }
    }
    assert_eq!(
        report_value["verdict"], "pass",
        "horizontal comparison report contains failed rows"
    );
    assert_eq!(report_value["failed_rows"], 0);
}

#[test]
fn test_horizontal_comparison_verdict_detects_mismatch() {
    assert!(!comparison_passes(
        [0, 0, 1, 1],
        [0, 0, 1, 1],
        [0.25; 4],
        [0.25; 4],
        1.1,
        1.0,
    ));
}

#[test]
fn test_horizontal_unsupported_cases_fail_closed() {
    let grid = HorizontalGrid {
        nx: 4,
        ny: 3,
        xlon0_deg: 0.0,
        ylat0_deg: 0.0,
        dx_deg: 1.0,
        dy_deg: 1.0,
        longitude_domain: LongitudeDomain::Minus180To180,
    };
    let field = vec![1.0_f32; 12];

    // Out-of-domain grid indices fail instead of clamping or wrapping.
    for (xt, yt) in [
        (-0.5, 1.0),
        (-5.0e-10, 1.0),
        (1.0, -0.5),
        (1.0, -5.0e-10),
        (4.0, 1.0),
        (1.0, 3.0),
        (3.001, 1.0),
        (3.0 + 5.0e-10, 1.0),
        (1.0, 2.0 + 5.0e-10),
    ] {
        assert!(
            matches!(
                sample_horizontal(&grid, &field, HorizontalStaggering::CellCenter, xt, yt),
                Err(HorizontalError::OutOfDomain { .. })
            ),
            "out-of-domain ({xt}, {yt}) must fail closed"
        );
    }

    // Periodic duplicate endpoint is out of the supported domain.
    let periodic = HorizontalGrid {
        nx: 4,
        ny: 3,
        xlon0_deg: -180.0,
        ylat0_deg: 0.0,
        dx_deg: 90.0,
        dy_deg: 1.0,
        longitude_domain: LongitudeDomain::Minus180To180,
    };
    assert!(matches!(
        sample_horizontal(
            &periodic,
            &field,
            HorizontalStaggering::CellCenter,
            4.0,
            1.0
        ),
        Err(HorizontalError::OutOfDomain { .. })
    ));

    // Face staggering is not frozen by #71.
    assert!(matches!(
        sample_horizontal(&grid, &[0.0_f32; 15], HorizontalStaggering::XFace, 1.0, 1.0),
        Err(HorizontalError::UnsupportedStaggering { .. })
    ));

    // Malformed dimensions and inconsistent metadata fail.
    let mut empty = grid.clone();
    empty.nx = 0;
    assert!(matches!(
        sample_horizontal(&empty, &[], HorizontalStaggering::CellCenter, 0.0, 0.0),
        Err(HorizontalError::MalformedDimensions { .. })
    ));
    let mut bad_spacing = grid.clone();
    bad_spacing.dx_deg = -1.0;
    assert!(matches!(
        sample_horizontal(
            &bad_spacing,
            &field,
            HorizontalStaggering::CellCenter,
            1.0,
            1.0
        ),
        Err(HorizontalError::InconsistentGridMetadata { .. })
    ));

    // Impossible geographic coordinates fail distinctly from out-of-domain.
    assert!(matches!(
        sample_horizontal_geographic(&grid, &field, HorizontalStaggering::CellCenter, 0.0, 95.0),
        Err(HorizontalError::ImpossibleCoordinate { .. })
    ));

    // Longitudes outside the declared convention fail even when their raw
    // numeric mapping would land inside a periodic grid.
    let periodic_zero_origin = HorizontalGrid {
        nx: 4,
        ny: 3,
        xlon0_deg: 0.0,
        ylat0_deg: 0.0,
        dx_deg: 90.0,
        dy_deg: 1.0,
        longitude_domain: LongitudeDomain::Minus180To180,
    };
    assert!(matches!(
        sample_horizontal_geographic(
            &periodic_zero_origin,
            &field,
            HorizontalStaggering::CellCenter,
            270.0,
            1.0
        ),
        Err(HorizontalError::LongitudeOutsideConvention { .. })
    ));
}
