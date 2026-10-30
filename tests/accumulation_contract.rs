use flexpart_gpu::meteorology::accumulation::{
    build_accumulation_report, resolve_interval_sequence, AccumulatedObservation,
    AccumulationError, Verdict,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Pinned SHA-256 of the checked-in accumulation contract fixture.
///
/// Recompute from the repository's canonical LF representation after
/// regenerating the fixture and update the value together with the fixture.
/// The test normalizes checkout line endings so Git's Windows CRLF conversion
/// cannot change the scientific artifact identity.
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
        AccumulationError::IntervalDurationOutOfRange { .. } => "interval_duration_out_of_range",
        AccumulationError::PackingDecreaseRejected { .. } => "packing_decrease_rejected",
        AccumulationError::SourceCountMismatch { .. } => "source_count_mismatch",
    }
}

#[test]
fn checked_in_accumulation_fixture_digest_is_pinned() {
    let source = include_str!("../fixtures/accumulation/contract-v1.json");
    let canonical_source = source.replace("\r\n", "\n");
    let digest = format!("{:x}", Sha256::digest(canonical_source.as_bytes()));
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
    assert_eq!(report.verdict, Verdict::Passed);
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
        assert!(
            matches!(report.verdict, Verdict::Failed(_)),
            "case `{}` report must fail",
            case.id
        );
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

// --- #216 bounded admission of GRIB-packing decreases (#219) -----------------

#[path = "support/grib202.rs"]
mod grib202;

mod packing_216 {
    use super::grib202;
    use flexpart_gpu::meteorology::accumulation::{
        build_accumulation_report_with_sources, classify_packing_decrease,
        resolve_interval_sequence, resolve_interval_sequence_with_sources, AccumulatedObservation,
        AccumulatedSeries, AccumulationError, PackingDecreaseCode, PackingDecreasePair,
        PackingDecreaseVerdict, PackingObservation, Verdict,
    };
    use flexpart_gpu::meteorology::{FieldId, GribPackingProvenance, SourceQuantization, Unit};
    use serde_json::Value;

    const GRID_ID: &str = "regular_ll_3x3_lat52_lon10_step1";

    fn token<T: serde::de::DeserializeOwned>(text: &str) -> T {
        serde_json::from_value(Value::String(text.to_owned())).expect("frozen token")
    }

    /// Control field tokens; the non-precipitation control names the canonical
    /// `sensible_heat_flux` field with its GRIB-style long name.
    fn field_id(text: &str) -> FieldId {
        match text {
            "surface_sensible_heat_flux" => FieldId::SensibleHeatFlux,
            other => token(other),
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn canonical_value(value: &Value) -> Option<f32> {
        let number = match value {
            Value::Null => return None,
            Value::String(text) => match text.as_str() {
                "NaN" => f64::NAN,
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                other => panic!("unknown token {other}"),
            },
            other => other.as_f64().expect("number"),
        };
        let narrowed = number as f32;
        assert!(
            !number.is_finite() || f64::from(narrowed) == number,
            "control values are f32"
        );
        Some(narrowed)
    }

    fn packing(side: &Value) -> Option<GribPackingProvenance> {
        (!side["packing"].is_null()).then(|| {
            serde_json::from_value(side["packing"].clone())
                .expect("frozen packing keys deserialize into the production carrier")
        })
    }

    fn observation<'a>(
        side: &'a Value,
        packing: Option<&'a GribPackingProvenance>,
    ) -> PackingObservation<'a> {
        PackingObservation {
            canonical_value: canonical_value(&side["canonical_value"]),
            valid_time_epoch_seconds: side["valid_time_s"].as_i64(),
            reset_epoch_seconds: side["reset_s"].as_i64(),
            grid_id: side["grid_id"].as_str().expect("grid"),
            source_id: side["source_id"].as_str(),
            source_message_sha256: side["source_message_sha256"].as_str(),
            packing,
        }
    }

    #[test]
    fn test_accumulation_216_all_controls_match_production_classifier() {
        let decision = grib202::decision();
        let controls = decision["negative_controls"].as_array().expect("controls");
        assert_eq!(controls.len(), 32, "the complete #216 control set");
        let mut admitted = 0;
        for control in controls {
            let id = control["id"].as_str().expect("id");
            let pair = &control["pair"];
            let (prev, cur) = (packing(&pair["previous"]), packing(&pair["current"]));
            let classification = classify_packing_decrease(&PackingDecreasePair {
                field_id: field_id(pair["field"].as_str().expect("field")),
                canonical_unit: token::<Unit>(pair["canonical_unit"].as_str().expect("unit")),
                previous: observation(&pair["previous"], prev.as_ref()),
                current: observation(&pair["current"], cur.as_ref()),
            });
            assert_eq!(
                classification.verdict,
                token::<PackingDecreaseVerdict>(control["expected_verdict"].as_str().expect("v")),
                "{id}"
            );
            assert_eq!(
                classification.code,
                token::<PackingDecreaseCode>(control["expected_code"].as_str().expect("c")),
                "{id}"
            );
            assert_eq!(
                classification.admitted.is_some(),
                classification.verdict == PackingDecreaseVerdict::AdmitBounded,
                "{id}: a bound accompanies exactly the admissions"
            );
            admitted += usize::from(classification.admitted.is_some());
        }
        assert_eq!(admitted, 1, "only bound_largest_admitted is admitted");
    }

    struct Series {
        observations: Vec<AccumulatedObservation>,
        sources: Vec<SourceQuantization>,
    }

    fn series(parameter: usize, cell: usize) -> Series {
        let observations = grib202::observations();
        Series {
            observations: observations
                .iter()
                .map(|o| AccumulatedObservation {
                    valid_time_epoch_seconds: o.valid_s,
                    reset_epoch_seconds: o.reset_s,
                    accumulated_amount_kg_per_square_meter: f64::from(
                        o.parameters[parameter].0[cell],
                    ),
                })
                .collect(),
            sources: observations
                .iter()
                .map(|o| o.parameters[parameter].1.clone())
                .collect(),
        }
    }

    fn with_sources<'a>(
        parameter: usize,
        cell: usize,
        series: &'a Series,
        sources: &'a [Option<&'a SourceQuantization>],
    ) -> AccumulatedSeries<'a> {
        AccumulatedSeries {
            field_id: [
                FieldId::LargeScalePrecipitation,
                FieldId::ConvectivePrecipitation,
            ][parameter],
            canonical_unit: Unit::KilogramPerSquareMeter,
            grid_id: GRID_ID,
            cell_index: cell,
            observations: &series.observations,
            sources,
        }
    }

    fn assert_run_telescoping(
        observations: &[AccumulatedObservation],
        resolved: &flexpart_gpu::meteorology::accumulation::IntervalSequence,
    ) {
        let (mut run_sum, mut run_projected) = (0.0, 0.0);
        for (k, product) in resolved.intervals.iter().enumerate() {
            run_sum += product.amount_kg_per_square_meter.0;
            if let Some(admission) = resolved
                .quantization_admissions
                .iter()
                .find(|a| a.interval_index == k)
            {
                run_projected -= admission.signed_difference_kg_per_square_meter;
            }
            let run_end = k + 1 == observations.len()
                || observations[k + 1].reset_epoch_seconds != observations[k].reset_epoch_seconds;
            if run_end {
                let total = observations[k].accumulated_amount_kg_per_square_meter;
                assert!(
                    (run_sum - (total + run_projected)).abs() <= 1e-12 * total.max(1.0),
                    "final total + projected decreases"
                );
                (run_sum, run_projected) = (0.0, 0.0);
            }
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_accumulation_216_genuine_rows_admitted_with_exact_bound() {
        let decision = grib202::decision();
        let rows = decision["offending_rows"].as_array().expect("rows");
        let admitted_rows = grib202::admitted_rows(&decision);
        assert_eq!(admitted_rows.len(), 3);
        let mut admissions = Vec::new();
        for parameter in 0..2 {
            for cell in 0..9 {
                let data = series(parameter, cell);
                let sources: Vec<_> = data.sources.iter().map(Some).collect();
                let resolved = resolve_interval_sequence_with_sources(&with_sources(
                    parameter, cell, &data, &sources,
                ))
                .expect("genuine series admitted");
                let strict = resolve_interval_sequence(&data.observations);
                let expected: Vec<_> = admitted_rows
                    .iter()
                    .filter(|(p, c, _)| (*p, *c) == (parameter, cell))
                    .collect();
                assert_eq!(resolved.quantization_admissions.len(), expected.len());
                if let Some((_, _, index)) = expected.first() {
                    assert!(
                        matches!(strict, Err(AccumulationError::NegativeDelta { index: i, .. }) if i == *index),
                        "without records the decrease remains NegativeDelta"
                    );
                } else {
                    let strict = strict.expect("unchanged series");
                    assert_eq!(
                        strict.intervals, resolved.intervals,
                        "normal intervals unchanged"
                    );
                }
                assert_run_telescoping(&data.observations, &resolved);
                for admission in resolved.quantization_admissions {
                    let product = resolved.intervals[admission.interval_index];
                    for value in [
                        product.amount_kg_per_square_meter.0,
                        product.rate_kg_per_square_meter_per_second.0,
                        product.rate_millimeter_per_hour.0,
                    ] {
                        assert_eq!(value.to_bits(), 0.0_f64.to_bits(), "+0.0 projection");
                    }
                    admissions.push(admission);
                }
            }
        }
        assert_eq!(admissions.len(), 3);
        for (admission, row) in admissions.iter().zip(rows) {
            let exact =
                |path: [&str; 2], key: &str| -> Value { row[path[0]][path[1]][key].clone() };
            assert_eq!(
                Some(admission.cell_index as u64),
                row["canonical_cell"].as_u64()
            );
            assert_eq!(
                admission.interval_end_epoch_seconds,
                row["interval_end_s"].as_i64().expect("end") + grib202::ORACLE_EPOCH_UNIX_S
            );
            assert_eq!(
                admission.bound_exact,
                exact(["bounds", "canonical_kg_per_m2"], "exact")
            );
            assert_eq!(
                Some(admission.bound_kg_per_square_meter),
                exact(["bounds", "canonical_kg_per_m2"], "float").as_f64()
            );
            assert_eq!(
                admission.signed_difference_exact,
                exact(["signed_difference", "canonical_kg_per_m2"], "exact")
            );
            assert_eq!(
                Some(admission.signed_difference_kg_per_square_meter),
                exact(["signed_difference", "canonical_kg_per_m2"], "float").as_f64()
            );
            assert_eq!(
                admission.message_sha256(),
                (
                    row["previous"]["message_sha256"].as_str().expect("prev"),
                    row["current"]["message_sha256"].as_str().expect("cur")
                )
            );
            assert_eq!(
                admission.decision,
                "flexpart-gpu/grib-packing-decrease-decision/v1"
            );
        }
    }

    #[test]
    fn test_accumulation_216_report_records_admission_instead_of_failure() {
        let (parameter, cell, index) = grib202::admitted_rows(&grib202::decision())[0];
        let data = series(parameter, cell);
        let sources: Vec<_> = data.sources.iter().map(Some).collect();
        let report = build_accumulation_report_with_sources(
            "large_scale_precipitation",
            &with_sources(parameter, cell, &data, &sources),
            None,
        );
        assert_eq!(report.verdict, Verdict::Passed);
        let row = &report.intervals[index];
        assert_eq!(row.verdict, Verdict::Passed);
        assert_eq!(
            row.derived_amount_kg_per_square_meter.map(f64::to_bits),
            Some(0)
        );
        assert!(row.quantization_admission.is_some());
        let serialized = serde_json::to_value(&report.intervals[0]).expect("row");
        assert!(
            serialized.get("quantization_admission").is_none(),
            "an absent admission is not serialized"
        );
        let short = &sources[..sources.len() - 1];
        let mismatched = build_accumulation_report_with_sources(
            "large_scale_precipitation",
            &with_sources(parameter, cell, &data, short),
            None,
        );
        assert!(
            matches!(mismatched.verdict, Verdict::Failed(_)),
            "a source-count mismatch fails closed"
        );
    }

    type Mutation = fn(&mut [SourceQuantization], &mut [AccumulatedObservation], usize);

    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_accumulation_216_provenance_mutations_fail_closed() {
        use PackingDecreaseCode as Code;
        use PackingDecreaseVerdict as V;
        let (parameter, cell, index) = grib202::admitted_rows(&grib202::decision())[0];
        let cases: Vec<(&str, Mutation, V, Code)> = vec![
            (
                "wrong message hash",
                |s, _, i| s[i].packing.message_sha256 = "0".repeat(64),
                V::Reject,
                Code::PackingProvenanceMismatch,
            ),
            (
                "wrong param",
                |s, _, i| s[i].packing.param_id = 143,
                V::Reject,
                Code::PackingProvenanceMismatch,
            ),
            (
                "era5 class",
                |s, _, i| s[i].packing.mars_class = Some("EA".into()),
                V::Unresolved,
                Code::UnsupportedSourceClass,
            ),
            (
                "absent class",
                |s, _, i| s[i - 1].packing.mars_class = None,
                V::Unresolved,
                Code::UnsupportedSourceClass,
            ),
            (
                "grib2",
                |s, _, i| s[i].packing.encoding = "grib2_grid_simple".into(),
                V::Unresolved,
                Code::UnsupportedPacking,
            ),
            (
                "bitmap",
                |s, _, i| s[i].packing.bitmap_present = true,
                V::Unresolved,
                Code::UnsupportedPacking,
            ),
            (
                "D != 0",
                |s, _, i| s[i].packing.decimal_scale_factor = 1,
                V::Unresolved,
                Code::UnsupportedPacking,
            ),
            (
                "bits 0",
                |s, _, i| s[i].packing.bits_per_value = 0,
                V::Unresolved,
                Code::UnsupportedPacking,
            ),
            (
                "conversion",
                |s, _, i| s[i].packing.canonical_conversion = "f32_decode".into(),
                V::Unresolved,
                Code::UnsupportedConversion,
            ),
            (
                "source unit",
                |s, _, i| s[i].packing.source_unit = "mm".into(),
                V::Reject,
                Code::UnitMismatch,
            ),
            (
                "scale",
                |s, _, i| s[i].packing.canonical_per_source_unit = 1,
                V::Reject,
                Code::UnitMismatch,
            ),
            (
                "time reassociation",
                |s, _, i| s[i].packing.step_seconds += 10_800,
                V::Reject,
                Code::RunIdentityMismatch,
            ),
            (
                "identical packing",
                |s, _, i| s[i - 1].packing.binary_scale_factor = s[i].packing.binary_scale_factor,
                V::Reject,
                Code::IdenticalPackingParameters,
            ),
            (
                "bound exceeded",
                |_, o, i| {
                    o[i].accumulated_amount_kg_per_square_meter = f64::from(0.899_998_6_f32);
                },
                V::Reject,
                Code::DecreaseExceedsQuantizationBound,
            ),
            (
                "source identity",
                |s, _, i| s[i].source_id = "ecmf-table128-param143".into(),
                V::Reject,
                Code::IdentityMismatch,
            ),
            (
                "not an f32",
                |_, o, i| o[i].accumulated_amount_kg_per_square_meter += 1e-12,
                V::Unresolved,
                Code::UnsupportedConversion,
            ),
        ];
        for (what, mutate, verdict, code) in cases {
            let mut data = series(parameter, cell);
            mutate(&mut data.sources, &mut data.observations, index);
            let sources: Vec<_> = data.sources.iter().map(Some).collect();
            let error = resolve_interval_sequence_with_sources(&with_sources(
                parameter, cell, &data, &sources,
            ))
            .expect_err(what);
            assert!(
                matches!(error, AccumulationError::PackingDecreaseRejected { index: i, verdict: v, code: c, .. }
                    if i == index && v == verdict && c == code),
                "{what}: {error:?}"
            );
        }
        // Field reassociation: the CP records attached to the LSP series.
        let data = series(parameter, cell);
        let other = series(1, cell).sources;
        let sources: Vec<_> = other.iter().map(Some).collect();
        let error =
            resolve_interval_sequence_with_sources(&with_sources(parameter, cell, &data, &sources))
                .expect_err("CP records on LSP");
        assert!(
            matches!(
                error,
                AccumulationError::PackingDecreaseRejected {
                    code: Code::PackingProvenanceMismatch,
                    ..
                }
            ),
            "{error:?}"
        );
        // Missing provenance on one or both observations.
        let mut sources: Vec<_> = data.sources.iter().map(Some).collect();
        sources[index - 1] = None;
        let error =
            resolve_interval_sequence_with_sources(&with_sources(parameter, cell, &data, &sources))
                .expect_err("one-sided record");
        assert!(
            matches!(
                error,
                AccumulationError::PackingDecreaseRejected {
                    verdict: V::Reject,
                    code: Code::IdentityMismatch,
                    ..
                }
            ),
            "{error:?}"
        );
        let none = vec![None; data.observations.len()];
        let error =
            resolve_interval_sequence_with_sources(&with_sources(parameter, cell, &data, &none))
                .expect_err("no records");
        assert!(
            matches!(error, AccumulationError::NegativeDelta { index: i, .. } if i == index),
            "{error:?}"
        );
    }
}
