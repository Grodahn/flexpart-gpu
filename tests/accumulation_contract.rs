use flexpart_gpu::meteorology::accumulation::{
    build_accumulation_report, resolve_interval_sequence, AccumulatedObservation,
    AccumulationError, Verdict,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Pinned SHA-256 of the checked-in accumulation contract fixture.
///
/// Recompute with `Get-FileHash`/`sha256sum` after regenerating the fixture and
/// update the value together with the fixture. The test recomputes the digest
/// from the embedded bytes so a stale pinned value fails loudly.
const ACCUMULATION_FIXTURE_SHA256: &str =
    "4bf7d901be71210dea9a072069d8551318455b4fe9bace8bcb45b04729517ef1";

/// The item-facing half-length and deaccumulation-halving used by the #71
/// oracle. `ftime = 3600` ahead of the "2400" offset; FLEXPART uses `dtt = dt/3`
/// drift with two `dt/2` accumulating halves, both consuming an already
/// normalized mm/h rate. #75 reproduces that handoff from resolver-derived rate
/// grids without reimplementing deaccumulation.
const ORACLE_DT_SECONDS: f64 = 3_600.0;
const ORACLE_DTT_SECONDS: f64 = 1_200.0;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractFixture {
    schema: SchemaRef,
    tolerance_relative: f64,
    source_accumulated_unit: String,
    amount_unit: String,
    rate_si_unit: String,
    rate_handoff_unit: String,
    seconds_per_hour: f64,
    oracle_source: OracleSource,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemaRef {
    id: String,
    version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OracleSource {
    issue: u32,
    fixture: String,
    case_id: String,
    boundary: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    mode: String,
    grid: Option<GridInfo>,
    sample: Option<SampleInfo>,
    golden: Option<GoldenInfo>,
    fields: Option<Vec<FieldRates>>,
    observations: Option<Vec<ScalarObservation>>,
    expected: Option<ExpectedOutcome>,
    expected_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GridInfo {
    nx: usize,
    ny: usize,
    order: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SampleInfo {
    xt: f64,
    yt: f64,
    #[serde(rename = "itime_epoch_seconds")]
    itime_epoch_seconds: i64,
    #[serde(rename = "memtime_epoch_seconds")]
    memtime_epoch_seconds: Vec<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenInfo {
    #[serde(rename = "large_scale_precipitation_mm_per_hour")]
    large_scale_precipitation_mm_per_hour: f64,
    #[serde(rename = "convective_precipitation_mm_per_hour")]
    convective_precipitation_mm_per_hour: f64,
    formula: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldRates {
    id: String,
    observations: Vec<GridObservation>,
    expected_interval_amounts_kg_per_square_meter: Vec<Vec<f64>>,
    expected_rates_mm_per_hour: Vec<Vec<f64>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GridObservation {
    valid_time_epoch_seconds: i64,
    reset_epoch_seconds: i64,
    amounts_kg_per_square_meter: Vec<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScalarObservation {
    valid_time_epoch_seconds: i64,
    reset_epoch_seconds: i64,
    accumulated_amount_kg_per_square_meter: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedOutcome {
    total_amount_kg_per_square_meter: f64,
    detected_reset_count: usize,
    intervals: Vec<ExpectedInterval>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedInterval {
    interval_start_epoch_seconds: i64,
    interval_end_epoch_seconds: i64,
    amount_kg_per_square_meter: f64,
    rate_mm_per_hour: f64,
    reset_applied: bool,
    detected_reset: bool,
}

fn parse_fixture() -> ContractFixture {
    let source = include_str!("../fixtures/accumulation/contract-v1.json");
    serde_json::from_str(source).expect("canonical accumulation fixture must parse")
}

fn case_by_id<'a>(fixture: &'a ContractFixture, id: &str) -> &'a Case {
    fixture
        .cases
        .iter()
        .find(|case| case.id == id)
        .unwrap_or_else(|| panic!("case `{id}` must exist in the fixture"))
}

fn grid_observations(field: &FieldRates) -> Vec<Vec<AccumulatedObservation>> {
    let cell_count = field.observations[0].amounts_kg_per_square_meter.len();
    let mut sequences = vec![Vec::with_capacity(field.observations.len()); cell_count];
    for observation in &field.observations {
        assert_eq!(
            observation.amounts_kg_per_square_meter.len(),
            cell_count,
            "every observation of `{}` carries one amount per cell",
            field.id
        );
        for (cell, amount) in observation
            .amounts_kg_per_square_meter
            .iter()
            .copied()
            .enumerate()
        {
            sequences[cell].push(AccumulatedObservation {
                valid_time_epoch_seconds: observation.valid_time_epoch_seconds,
                reset_epoch_seconds: observation.reset_epoch_seconds,
                accumulated_amount_kg_per_square_meter: amount,
            });
        }
    }
    sequences
}

/// Frozen closed-form bilinear used by the #71 `rain-layer-fields` oracle
/// fixture. Sampling is owned by #73; this test only reuses the arithmetic so
/// the #75 rate handoff reproduces the oracle golden values.
fn bilinear_sample(rates: &[f64], nx: usize, ny: usize, xt: f64, yt: f64) -> f64 {
    assert_eq!(rates.len(), nx * ny, "rate grid must cover the 2-D domain");
    let ix = xt.floor() as usize;
    let jy = yt.floor() as usize;
    let ixp = (ix + 1).min(nx - 1);
    let jyp = (jy + 1).min(ny - 1);
    let dx = xt - ix as f64;
    let dy = yt - jy as f64;
    let v00 = rates[ix + nx * jy];
    let v01 = rates[ixp + nx * jy];
    let v10 = rates[ix + nx * jyp];
    let v11 = rates[ixp + nx * jyp];
    v00 * (1.0 - dx) * (1.0 - dy) + v01 * dx * (1.0 - dy) + v10 * (1.0 - dx) * dy + v11 * dx * dy
}

fn error_variant_name(error: &AccumulationError) -> &'static str {
    match error {
        AccumulationError::EmptySequence => "empty_sequence",
        AccumulationError::NonIncreasingTimestamps { .. } => "non_increasing_timestamps",
        AccumulationError::BackwardsReset { .. } => "backwards_reset",
        AccumulationError::ResetNotBeforeValidTime { .. } => "reset_not_before_valid_time",
        AccumulationError::MidWindowReset { .. } => "mid_window_reset",
        AccumulationError::UncoveredGap { .. } => "uncovered_gap",
        AccumulationError::NegativeDelta { .. } => "negative_delta",
        AccumulationError::NegativeAmount { .. } => "negative_amount",
        AccumulationError::NonFiniteAmount { .. } => "non_finite_amount",
    }
}

#[test]
fn checked_in_accumulation_fixture_digest_is_pinned() {
    let source = include_str!("../fixtures/accumulation/contract-v1.json");
    let digest = format!("{:x}", Sha256::digest(source.as_bytes()));
    assert_eq!(
        digest, ACCUMULATION_FIXTURE_SHA256,
        "fixture changed; regenerate and update the pinned digest together with the provenance"
    );
}

#[test]
fn fixture_declares_canonical_schema_and_units() {
    let fixture = parse_fixture();
    assert_eq!(fixture.schema.id, "flexpart-gpu.accumulation-contract");
    assert_eq!(fixture.schema.version, 1);
    assert!((fixture.tolerance_relative - 1.0e-6).abs() <= f64::EPSILON);
    assert_eq!(fixture.source_accumulated_unit, "kilogram_per_square_meter");
    assert_eq!(fixture.amount_unit, "kilogram_per_square_meter");
    assert_eq!(fixture.rate_si_unit, "kilogram_per_square_meter_per_second");
    assert_eq!(fixture.rate_handoff_unit, "millimeter_per_hour");
    assert!((fixture.seconds_per_hour - 3_600.0).abs() <= f64::EPSILON);
    assert_eq!(fixture.oracle_source.issue, 71);
    assert_eq!(
        fixture.oracle_source.fixture,
        "fixtures/interpolation/contract-v1.json"
    );
    assert_eq!(fixture.oracle_source.case_id, "rain-layer-fields");
    assert!(fixture.oracle_source.boundary.contains("#75"));
}

#[test]
fn oracle_rate_handoff_derives_issue_71_rate_grids_exactly() {
    let fixture = parse_fixture();
    let case = case_by_id(&fixture, "rain-layer-fields-bilinear-rates");
    let grid = case.grid.as_ref().expect("oracle case declares its grid");
    assert_eq!(grid.nx, 4);
    assert_eq!(grid.ny, 3);
    assert_eq!(grid.order, "x_fastest");

    for field in case.fields.as_ref().expect("oracle case declares fields") {
        let sequences = grid_observations(field);
        for (cell, sequence) in sequences.iter().enumerate() {
            let resolved = resolve_interval_sequence(sequence).unwrap_or_else(|error| {
                panic!("field `{}` cell {cell} must resolve: {error}", field.id)
            });
            assert_eq!(resolved.intervals.len(), field.observations.len());

            for (product, (expected_amounts, expected_rates)) in resolved.intervals.iter().zip(
                field
                    .expected_interval_amounts_kg_per_square_meter
                    .iter()
                    .zip(field.expected_rates_mm_per_hour.iter()),
            ) {
                let expected_amount = expected_amounts[cell];
                let expected_rate = expected_rates[cell];
                approx::assert_relative_eq!(
                    product.amount_kg_per_square_meter.0,
                    expected_amount,
                    epsilon = 1.0e-9
                );
                approx::assert_relative_eq!(
                    product.rate_millimeter_per_hour.0,
                    expected_rate,
                    epsilon = 1.0e-9
                );
                approx::assert_relative_eq!(
                    product.rate_kg_per_square_meter_per_second.0,
                    expected_rate / 3_600.0,
                    epsilon = 1.0e-12
                );
            }

            // The leading interval derives from the declared reset start and the
            // run continues with no declared reset in between.
            assert!(resolved.intervals[0].reset_applied);
            assert!(!resolved.intervals[1].reset_applied);
            assert_eq!(resolved.detected_reset_count, 0);

            let last = sequence.last().expect("non-empty run");
            approx::assert_relative_eq!(
                resolved.total_amount_kg_per_square_meter(),
                last.accumulated_amount_kg_per_square_meter,
                epsilon = 1.0e-9
            );
        }
    }
}

#[test]
fn oracle_rate_handoff_bilinear_sample_reproduces_issue_71_golden() {
    let fixture = parse_fixture();
    let case = case_by_id(&fixture, "rain-layer-fields-bilinear-rates");
    let grid = case.grid.as_ref().expect("oracle case declares its grid");
    let sample = case
        .sample
        .as_ref()
        .expect("oracle case declares its sample");
    let golden = case
        .golden
        .as_ref()
        .expect("oracle case declares golden values");
    assert!(
        golden.formula.contains("1200.0"),
        "fixture formula must document the dtt divisor used by the handoff"
    );
    let fields = case.fields.as_ref().expect("oracle case declares fields");
    let cell_count = grid.nx * grid.ny;

    // The #71 frame is [0, 3600] with the query at itime = 1800; each of the two
    // accumulating halves is `dt/2` and FLEXPART's `dtt = dt/3` drift scales the
    // handoff as in the fixture formula.
    assert_eq!(sample.itime_epoch_seconds, 1_800);
    assert_eq!(sample.memtime_epoch_seconds, vec![0, 3_600]);
    let half_seconds = ORACLE_DT_SECONDS / 2.0;

    for field in fields {
        let sequences = grid_observations(field);
        let mut rate_grids: Vec<Vec<f64>> = Vec::with_capacity(field.observations.len());
        for index in 0..field.observations.len() {
            let mut rate_grid = Vec::with_capacity(cell_count);
            for (cell, sequence) in sequences.iter().enumerate() {
                let resolved = resolve_interval_sequence(sequence).unwrap_or_else(|error| {
                    panic!("field `{}` cell {cell} resolves: {error}", field.id)
                });
                rate_grid.push(resolved.intervals[index].rate_millimeter_per_hour.0);
            }
            rate_grids.push(rate_grid);
        }

        let sampled = rate_grids
            .iter()
            .map(|rates| bilinear_sample(rates, grid.nx, grid.ny, sample.xt, sample.yt))
            .collect::<Vec<_>>();
        let handoff =
            sampled.iter().map(|rate| rate * half_seconds).sum::<f64>() / ORACLE_DTT_SECONDS;

        let expected_golden = match field.id.as_str() {
            "large_scale_precipitation" => golden.large_scale_precipitation_mm_per_hour,
            "convective_precipitation" => golden.convective_precipitation_mm_per_hour,
            other => panic!("oracle case declares an unexpected field id `{other}`"),
        };
        approx::assert_relative_eq!(handoff, expected_golden, epsilon = 1.0e-6);
    }
}

#[test]
fn declared_reset_contiguous_case_matches_expected_outcome() {
    let fixture = parse_fixture();
    let case = case_by_id(&fixture, "continuous-run-with-declared-reset");
    let observations = case
        .observations
        .as_ref()
        .expect("declared-reset case declares observations")
        .iter()
        .map(|observation| AccumulatedObservation {
            valid_time_epoch_seconds: observation.valid_time_epoch_seconds,
            reset_epoch_seconds: observation.reset_epoch_seconds,
            accumulated_amount_kg_per_square_meter: observation
                .accumulated_amount_kg_per_square_meter,
        })
        .collect::<Vec<_>>();
    let expected = case
        .expected
        .as_ref()
        .expect("declared-reset case declares expected");

    let resolved =
        resolve_interval_sequence(&observations).expect("declared-reset sequence resolves");
    assert_eq!(resolved.intervals.len(), expected.intervals.len());
    assert_eq!(resolved.detected_reset_count, expected.detected_reset_count);
    approx::assert_relative_eq!(
        resolved.total_amount_kg_per_square_meter(),
        expected.total_amount_kg_per_square_meter,
        epsilon = 1.0e-9
    );

    for (product, expected_interval) in resolved.intervals.iter().zip(&expected.intervals) {
        assert_eq!(
            product.interval_start_epoch_seconds,
            expected_interval.interval_start_epoch_seconds
        );
        assert_eq!(
            product.interval_end_epoch_seconds,
            expected_interval.interval_end_epoch_seconds
        );
        approx::assert_relative_eq!(
            product.amount_kg_per_square_meter.0,
            expected_interval.amount_kg_per_square_meter,
            epsilon = 1.0e-9
        );
        approx::assert_relative_eq!(
            product.rate_millimeter_per_hour.0,
            expected_interval.rate_mm_per_hour,
            epsilon = 1.0e-9
        );
        assert_eq!(product.reset_applied, expected_interval.reset_applied);
    }

    // Report form agrees with the resolver and stays green against the oracle.
    let oracle_rates = expected
        .intervals
        .iter()
        .map(|interval| interval.rate_mm_per_hour)
        .collect::<Vec<_>>();
    let report = build_accumulation_report(
        "large_scale_precipitation",
        &observations,
        Some(&oracle_rates),
    );
    assert_eq!(report.failed_rows, 0);
    for (row, expected_interval) in report.intervals.iter().zip(&expected.intervals) {
        assert_eq!(row.verdict, Verdict::Passed);
        assert_eq!(row.matches_oracle_within_relative_tolerance, Some(true));
        assert_eq!(row.detected_reset, expected_interval.detected_reset);
    }
}

#[test]
fn fail_closed_cases_report_matching_errors() {
    let fixture = parse_fixture();
    let mut checked = 0;
    for case in &fixture.cases {
        if case.mode != "fail_closed" {
            continue;
        }
        let observations = case
            .observations
            .as_ref()
            .expect("fail-closed case declares observations")
            .iter()
            .map(|observation| AccumulatedObservation {
                valid_time_epoch_seconds: observation.valid_time_epoch_seconds,
                reset_epoch_seconds: observation.reset_epoch_seconds,
                accumulated_amount_kg_per_square_meter: observation
                    .accumulated_amount_kg_per_square_meter,
            })
            .collect::<Vec<_>>();
        let expected_error = case
            .expected_error
            .as_ref()
            .expect("fail-closed case declares expected_error");

        let resolved = resolve_interval_sequence(&observations);
        let error = match resolved {
            Ok(_) => panic!("case `{}` must fail closed, not resolve", case.id),
            Err(error) => error,
        };
        assert_eq!(
            error_variant_name(&error),
            expected_error.as_str(),
            "case `{}` must fail with the declared error",
            case.id
        );

        let report = build_accumulation_report(case.id.as_str(), &observations, None);
        assert_eq!(
            report.failed_rows, 1,
            "case `{}` must record exactly one failed row",
            case.id
        );
        let row = &report.intervals[1];
        assert!(
            matches!(row.verdict, Verdict::Failed(_)),
            "case `{}` row must carry a failure verdict",
            case.id
        );
        assert_eq!(row.derived_amount_kg_per_square_meter, None);
        assert_eq!(row.derived_rate_millimeter_per_hour, None);
        checked += 1;
    }

    // The fixture's fail-closed suite must cover every canonical rejection the
    // contract promises for ambiguous/negative metadata.
    assert_eq!(checked, 5);
}
