//! Issue #74: candidate validation against the #71 temporal oracle fixtures and
//! synthetic time-linear fields.
//!
//! The candidate is `meteorology::temporal::sample_field`, which samples an
//! ordered series of canonical #29 snapshots at a requested validity time using
//! the temporal semantics frozen by #71.
//!
//! This test (a) reproduces every frozen query of the `temporal-bilinear` case
//! in `fixtures/interpolation/contract-v1.json` (values `10/15/20`, oracle `DTS`
//! weights) through the canonical candidate, (b) checks synthetic
//! time-linear fields against independently hand-computed expectations, and
//! (c) exercises every fail-closed temporal case. A machine-readable comparison
//! report is written to `target/temporal-comparison-report.json`.

use flexpart_gpu::meteorology::{
    temporal::{
        build_comparison_report, sample_field, OracleQuery, RequestedSampleTime,
        TemporalApplication, TemporalError, Tolerance, Verdict,
    },
    Axis, Calendar, Field, FieldId, FieldTime, HorizontalGrid, HorizontalStaggering,
    LongitudeDomain, SchemaIdentity, SignConvention, Snapshot, StorageOrder, TemporalKind,
    TemporalPolicy, Unit, VerticalCoordinate, VerticalCoordinateKind, VerticalOrdering,
    VerticalReference, VerticalStaggering,
};
use serde_json::Value;
use std::path::Path;

const REL_TOL: f64 = 1.0e-5;
const ABS_TOL: f64 = 1.0e-6;

fn tolerance(oracle: f64) -> f64 {
    ABS_TOL + REL_TOL * oracle.abs()
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    let diff = (actual - expected).abs();
    let allowed = tolerance(expected);
    assert!(
        diff <= allowed,
        "{what}: candidate {actual} != expected {expected} (diff {diff}, tolerance {allowed})"
    );
}

fn as_f64(value: &Value) -> f64 {
    value.as_f64().expect("golden value must be a number")
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
        unit: Unit::MeterPerSecond,
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
        unit: Unit::Kelvin,
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

fn oracle_query(
    requested_time_epoch_seconds: i64,
    element_index: usize,
    oracle_value: f32,
    expected_application: TemporalApplication,
) -> OracleQuery {
    OracleQuery {
        expected_application: Some(expected_application),
        ..OracleQuery::new(requested_time_epoch_seconds, element_index, oracle_value)
    }
}

fn synthetic_series() -> [Snapshot; 3] {
    [
        temperature_snapshot(0, vec![270.0, 280.0, 290.0]),
        temperature_snapshot(1800, vec![272.5, 290.0, 307.5]),
        temperature_snapshot(3600, vec![275.0, 300.0, 325.0]),
    ]
}

/// Freeze of `temporal_interpolation` (interpol_mod.f90:531-537) for
/// `memtime = [0, 3600]`, used to re-derive the #71 goldens independently.
fn flexpart_temporal_value(itime: f64, time1: f64, time2: f64) -> f64 {
    (time1 * (3600.0 - itime) + time2 * itime) / 3600.0
}

#[test]
fn test_temporal_candidate_matches_frozen_oracle() {
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

    let snapshots = [wind_snapshot(0, 10.0), wind_snapshot(3600, 20.0)];
    let snapshot_refs: Vec<&Snapshot> = snapshots.iter().collect();

    for (index, query) in golden_queries.iter().enumerate() {
        let itime = as_f64(&query["ITIME"][0]);
        let dts = &query["DTS"];
        let golden_value = as_f64(&query["VALUE"][0]);

        let sample = sample_field(FieldId::WindU, &snapshot_refs, gregorian(itime as i64))
            .expect("canonical temporal sampling must succeed");

        assert_eq!(
            sample.source_timestamps.lower_epoch_seconds, 0,
            "oracle query {index} must stay inside memtime"
        );
        assert_eq!(
            sample.source_timestamps.upper_epoch_seconds, 3600,
            "oracle query {index} must stay inside memtime"
        );
        assert_close(
            f64::from(sample.values[0]),
            golden_value,
            &format!("temporal-bilinear query {index} VALUE"),
        );
        assert_close(
            sample.weights.dt1_seconds,
            as_f64(&dts[0]),
            &format!("temporal-bilinear query {index} dt1"),
        );
        assert_close(
            sample.weights.dt2_seconds,
            as_f64(&dts[1]),
            &format!("temporal-bilinear query {index} dt2"),
        );
        assert_close(
            sample.weights.inverse_span_per_second,
            as_f64(&dts[2]),
            &format!("temporal-bilinear query {index} dtt"),
        );

        // The candidate must reproduce the #71 goldens via the frozen FLEXPART
        // formula, not merely coincide with the query values.
        let rederived = flexpart_temporal_value(itime, 10.0, 20.0);
        assert_close(
            rederived,
            golden_value,
            "golden satisfies FLEXPART closed form",
        );
        assert_close(
            f64::from(sample.values[0]),
            rederived,
            &format!("temporal-bilinear query {index} candidate vs closed form"),
        );
    }
}

#[test]
fn test_temporal_scenario_fixture_matches_the_frozen_contract() {
    let contract: Value =
        serde_json::from_str(include_str!("../fixtures/interpolation/contract-v1.json"))
            .expect("parse interpolation contract");
    let oracle_goldens: Vec<f64> = contract["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == "temporal-bilinear")
        .expect("temporal-bilinear case")["golden"]["queries"]
        .as_array()
        .expect("queries")
        .iter()
        .map(|query| as_f64(&query["VALUE"][0]))
        .collect();

    let scenario: Value = serde_json::from_str(include_str!(
        "../fixtures/temporal/oracle-temporal-bilinear-scenario.json"
    ))
    .expect("parse oracle scenario");
    let scenario_queries = scenario["queries"].as_array().expect("scenario queries");
    assert_eq!(scenario_queries.len(), oracle_goldens.len());
    for (scenario_query, golden) in scenario_queries.iter().zip(oracle_goldens) {
        assert_close(
            scenario_query["oracle_value"]
                .as_f64()
                .expect("oracle value"),
            golden,
            "scenario fixture oracle value must match #71 golden",
        );
    }
}

#[test]
fn test_temporal_synthetic_linear_matches_hand_computed() {
    // Values are linear in time with a distinct slope per level:
    // T[level0] = 270 + (5/3600)*t, T[level1] = 280 + (20/3600)*t,
    // T[level2] = 290 + (35/3600)*t.
    let snapshots = synthetic_series();
    let snapshot_refs: Vec<&Snapshot> = snapshots.iter().collect();

    let run = |t: i64, element: usize| -> (f32, TemporalApplication) {
        let sample = sample_field(FieldId::Temperature, &snapshot_refs, gregorian(t))
            .expect("synthetic linear sampling must succeed");
        (sample.values[element], sample.application)
    };

    // Level 0 slope: 5 K over 3600 s.
    assert_eq!(run(0, 0), (270.0, TemporalApplication::FirstEndpoint));
    assert_eq!(run(900, 0), (271.25, TemporalApplication::LinearInterior));
    assert_eq!(
        run(1800, 0),
        (272.5, TemporalApplication::ExactSourceTimestamp)
    );
    assert_eq!(run(3600, 0), (275.0, TemporalApplication::LastEndpoint));

    // Level 1 slope: 20 K over 3600 s.
    assert_eq!(run(900, 1), (285.0, TemporalApplication::LinearInterior));
    assert_eq!(
        run(1800, 1),
        (290.0, TemporalApplication::ExactSourceTimestamp)
    );

    // Level 2 slope: 35 K over 3600 s.
    assert_eq!(run(900, 2), (298.75, TemporalApplication::LinearInterior));
    assert_eq!(run(3600, 2), (325.0, TemporalApplication::LastEndpoint));

    // The leftmost bracketing pair for an exact interior timestamp is
    // (previous, exact): the exact snapshot must be returned unchanged.
    let exact = sample_field(FieldId::Temperature, &snapshot_refs, gregorian(1800))
        .expect("exact timestamp sampling");
    let (exact_lower, exact_upper) = exact.weights.normalized();
    assert_close(exact_lower, 0.0, "exact timestamp lower weight");
    assert_close(exact_upper, 1.0, "exact timestamp upper weight");
    assert_eq!(exact.values[0], 272.5, "exact timestamp value unchanged");

    // A strict midpoint between consecutive snapshots uses symmetric weights.
    let midpoint = sample_field(FieldId::Temperature, &snapshot_refs, gregorian(900))
        .expect("midpoint sampling");
    let (lower, upper) = midpoint.weights.normalized();
    assert_close(lower, 0.5, "midpoint lower weight");
    assert_close(upper, 0.5, "midpoint upper weight");
}

#[test]
fn test_temporal_build_comparison_report_synthetic_linear() {
    let snapshots = synthetic_series();
    let snapshot_refs: Vec<&Snapshot> = snapshots.iter().collect();

    let queries = vec![
        oracle_query(0, 0, 270.0, TemporalApplication::FirstEndpoint),
        oracle_query(900, 0, 271.25, TemporalApplication::LinearInterior),
        oracle_query(900, 1, 285.0, TemporalApplication::LinearInterior),
        oracle_query(900, 2, 298.75, TemporalApplication::LinearInterior),
        oracle_query(1800, 1, 290.0, TemporalApplication::ExactSourceTimestamp),
        oracle_query(3600, 2, 325.0, TemporalApplication::LastEndpoint),
    ];

    let report = build_comparison_report(
        "synthetic-linear",
        FieldId::Temperature,
        &snapshot_refs,
        Tolerance::new(ABS_TOL, REL_TOL),
        &queries,
    )
    .expect("synthetic comparison report must build");

    assert_eq!(report.status, Verdict::Pass);
    assert_eq!(report.rows.len(), queries.len());
    assert_eq!(report.calendar, Calendar::Gregorian);
    assert_eq!(report.source_timestamps_epoch_seconds, vec![0, 1800, 3600]);
    for row in &report.rows {
        assert_eq!(row.row_verdict, Verdict::Pass, "{row:?}");
    }
}

#[test]
fn test_temporal_machine_readable_report_emits_required_fields() {
    let snapshots = synthetic_series();
    let snapshot_refs: Vec<&Snapshot> = snapshots.iter().collect();
    let queries = vec![oracle_query(
        900,
        1,
        285.0,
        TemporalApplication::LinearInterior,
    )];
    let report = build_comparison_report(
        "machine-report",
        FieldId::Temperature,
        &snapshot_refs,
        Tolerance::new(ABS_TOL, REL_TOL),
        &queries,
    )
    .expect("report must build");

    let encoded = serde_json::to_string_pretty(&report).expect("serialize comparison report");
    let out_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("temporal-comparison-report.json");
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).expect("create target dir");
    }
    std::fs::write(&out_path, encoded).expect("write comparison report");

    let value: Value =
        serde_json::from_str(&std::fs::read_to_string(&out_path).expect("read comparison report"))
            .expect("report must be valid JSON");
    assert_eq!(
        value["schema"]["id"].as_str().expect("schema id"),
        "flexpart-gpu.temporal-interpolation-report"
    );
    assert_eq!(value["status"].as_str(), Some("pass"));
    let row = value["rows"].as_array().expect("rows").last().expect("row");
    for key in [
        "field_id",
        "element_index",
        "requested_time_epoch_seconds",
        "source_timestamps",
        "application",
        "weights",
        "candidate_value",
        "oracle_value",
        "tolerance",
        "absolute_difference",
        "value_verdict",
        "weights_verdict",
        "application_verdict",
        "row_verdict",
    ] {
        assert!(row.get(key).is_some(), "comparison row must record {key}");
    }
}

#[test]
fn test_temporal_fail_closed_cases() {
    let before = [wind_snapshot(0, 10.0), wind_snapshot(3600, 20.0)];
    let before_refs: Vec<&Snapshot> = before.iter().collect();
    assert_eq!(
        sample_field(FieldId::WindU, &before_refs, gregorian(-3600)),
        Err(TemporalError::BeforeFirstCoverage {
            requested: -3600,
            first: 0
        })
    );
    assert_eq!(
        sample_field(FieldId::WindU, &before_refs, gregorian(7200)),
        Err(TemporalError::AfterLastCoverage {
            requested: 7200,
            last: 3600
        })
    );

    let single = [wind_snapshot(0, 10.0)];
    let single_refs: Vec<&Snapshot> = single.iter().collect();
    assert_eq!(
        sample_field(FieldId::WindU, &single_refs, gregorian(0)),
        Err(TemporalError::InsufficientTemporalCoverage { snapshots: 1 })
    );

    let duplicate = [wind_snapshot(0, 10.0), wind_snapshot(0, 20.0)];
    let duplicate_refs: Vec<&Snapshot> = duplicate.iter().collect();
    assert_eq!(
        sample_field(FieldId::WindU, &duplicate_refs, gregorian(0)),
        Err(TemporalError::NonMonotonicTimestamps {
            index: 1,
            previous: 0,
            current: 0
        })
    );

    let descending = [wind_snapshot(3600, 10.0), wind_snapshot(0, 20.0)];
    let descending_refs: Vec<&Snapshot> = descending.iter().collect();
    assert_eq!(
        sample_field(FieldId::WindU, &descending_refs, gregorian(0)),
        Err(TemporalError::NonMonotonicTimestamps {
            index: 1,
            previous: 3600,
            current: 0
        })
    );

    let missing = [
        wind_snapshot(0, 10.0),
        temperature_snapshot(3600, vec![275.0; 3]),
    ];
    let missing_refs: Vec<&Snapshot> = missing.iter().collect();
    assert_eq!(
        sample_field(FieldId::WindU, &missing_refs, gregorian(0)),
        Err(TemporalError::MissingFieldSnapshot {
            snapshot_index: 1,
            field_id: FieldId::WindU
        })
    );

    let mut non_instantaneous = wind_snapshot(0, 10.0);
    non_instantaneous.fields[0].time.kind = TemporalKind::IntervalMean;
    let interval_series = [non_instantaneous, wind_snapshot(3600, 20.0)];
    let interval_refs: Vec<&Snapshot> = interval_series.iter().collect();
    assert_eq!(
        sample_field(FieldId::WindU, &interval_refs, gregorian(0)),
        Err(TemporalError::NonInstantaneousSnapshot {
            snapshot_index: 0,
            field_id: FieldId::WindU,
            kind: TemporalKind::IntervalMean
        })
    );

    let static_spec = snapshot_with(Field {
        id: FieldId::Orography,
        shape: vec![1, 1],
        axis_order: vec![Axis::X, Axis::Y],
        storage_order: StorageOrder::XFastest,
        unit: Unit::Meter,
        sign: SignConvention::SignedScalar,
        horizontal_staggering: HorizontalStaggering::CellCenter,
        vertical_staggering: VerticalStaggering::NotApplicable,
        time: field_time(Calendar::Gregorian, TemporalKind::Static, 0),
        values: vec![250.0],
    });
    let static_series = [static_spec, wind_snapshot(3600, 20.0)];
    let static_refs: Vec<&Snapshot> = static_series.iter().collect();
    assert_eq!(
        sample_field(FieldId::Orography, &static_refs, gregorian(0)),
        Err(TemporalError::UnsupportedTemporalPolicy {
            field_id: FieldId::Orography,
            temporal_policy: TemporalPolicy::Static
        })
    );

    let mut precipitation = wind_snapshot(0, 10.0);
    precipitation.fields[0].id = FieldId::LargeScalePrecipitation;
    precipitation.fields[0].unit = Unit::KilogramPerSquareMeter;
    precipitation.fields[0].sign = SignConvention::NonNegative;
    let rain_series = [precipitation, wind_snapshot(3600, 20.0)];
    let rain_refs: Vec<&Snapshot> = rain_series.iter().collect();
    assert_eq!(
        sample_field(FieldId::LargeScalePrecipitation, &rain_refs, gregorian(0)),
        Err(TemporalError::UnsupportedTemporalPolicy {
            field_id: FieldId::LargeScalePrecipitation,
            temporal_policy: TemporalPolicy::PrecipitationAmount
        })
    );

    let mismatched_calendar = [wind_snapshot(0, 10.0), wind_snapshot(3600, 20.0)];
    let mismatched_refs: Vec<&Snapshot> = mismatched_calendar.iter().collect();
    assert_eq!(
        sample_field(
            FieldId::WindU,
            &mismatched_refs,
            RequestedSampleTime::new(Calendar::ProlepticGregorian, 1800)
        ),
        Err(TemporalError::CalendarMismatch {
            requested: Calendar::ProlepticGregorian,
            series: Calendar::Gregorian
        })
    );

    let mut proleptic = wind_snapshot(3600, 20.0);
    proleptic.fields[0].time.calendar = Calendar::ProlepticGregorian;
    let mixed_series = [wind_snapshot(0, 10.0), proleptic];
    let mixed_refs: Vec<&Snapshot> = mixed_series.iter().collect();
    assert_eq!(
        sample_field(FieldId::WindU, &mixed_refs, gregorian(0)),
        Err(TemporalError::InconsistentCalendar {
            calendar_a: Calendar::Gregorian,
            calendar_b: Calendar::ProlepticGregorian
        })
    );

    let mut reshaped = wind_snapshot(3600, 20.0);
    reshaped.fields[0].values.push(5.0);
    reshaped.fields[0].shape = vec![1, 2];
    let shape_series = [wind_snapshot(0, 10.0), reshaped];
    let shape_refs: Vec<&Snapshot> = shape_series.iter().collect();
    assert_eq!(
        sample_field(FieldId::WindU, &shape_refs, gregorian(0)),
        Err(TemporalError::InconsistentFieldShape {
            field_id: FieldId::WindU,
            snapshot_index: 1,
            expected: 1,
            actual: 2
        })
    );

    let report_snapshots = synthetic_series();
    let report_refs: Vec<&Snapshot> = report_snapshots.iter().collect();
    let report_queries = [OracleQuery::new(900, 5, 285.0)];
    assert_eq!(
        build_comparison_report(
            "out-of-bounds",
            FieldId::Temperature,
            &report_refs,
            Tolerance::new(ABS_TOL, REL_TOL),
            &report_queries,
        ),
        Err(TemporalError::OutOfBoundsElement {
            element_index: 5,
            length: 3
        })
    );

    let mut restaggered = wind_snapshot(3600, 20.0);
    restaggered.fields[0].vertical_staggering = VerticalStaggering::LevelCenter;
    let stagger_series = [wind_snapshot(0, 10.0), restaggered];
    let stagger_refs: Vec<&Snapshot> = stagger_series.iter().collect();
    assert_eq!(
        sample_field(FieldId::WindU, &stagger_refs, gregorian(0)),
        Err(TemporalError::InconsistentStaggering {
            field_id: FieldId::WindU,
            snapshot_index: 1
        })
    );

    let non_finite = [wind_snapshot(0, f32::INFINITY), wind_snapshot(3600, 20.0)];
    let non_finite_refs: Vec<&Snapshot> = non_finite.iter().collect();
    assert_eq!(
        sample_field(FieldId::WindU, &non_finite_refs, gregorian(0)),
        Err(TemporalError::NonFiniteValue {
            field_id: FieldId::WindU,
            snapshot_index: 0,
            element_index: 0
        })
    );

    // Finite f32 inputs with [0, 1] weights cannot overflow the f32 range:
    // the blend stays far within half a ULP of the input magnitude, so
    // f32::MAX round-trips exactly. NonFiniteResult remains a defensive guard
    // for future weight schemes, not a reachable outcome of this blend.
    let extreme = [wind_snapshot(0, f32::MAX), wind_snapshot(3600, f32::MAX)];
    let extreme_refs: Vec<&Snapshot> = extreme.iter().collect();
    let extreme_sample = sample_field(FieldId::WindU, &extreme_refs, gregorian(1800))
        .expect("extreme finite inputs must sample");
    assert_eq!(extreme_sample.values[0], f32::MAX);

    assert_eq!(
        sample_field(FieldId::SensibleHeatFlux, &before_refs, gregorian(1800)),
        Err(TemporalError::UnsupportedTemporalPolicy {
            field_id: FieldId::SensibleHeatFlux,
            temporal_policy: TemporalPolicy::SurfaceFluxRate
        })
    );
}

#[test]
fn test_temporal_report_binary_produces_machine_readable_reports() {
    use std::process::Command;

    let output_dir = tempfile::tempdir().expect("temporary output dir");
    for scenario in [
        "oracle-temporal-bilinear-scenario.json",
        "synthetic-linear-scenario.json",
    ] {
        let input = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("temporal")
            .join(scenario);
        let output = output_dir.path().join(format!("{scenario}.report.json"));

        let status = Command::new(env!("CARGO_BIN_EXE_temporal-interpolation-report"))
            .arg(&input)
            .arg(&output)
            .status()
            .expect("run temporal-interpolation-report binary");
        assert!(status.success(), "{scenario} report must pass");

        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(&output).expect("read binary report"))
                .expect("binary report must be valid JSON");
        assert_eq!(
            value["schema"]["id"].as_str(),
            Some("flexpart-gpu.temporal-interpolation-report")
        );
        assert_eq!(value["status"].as_str(), Some("pass"));
        assert!(
            !value["rows"].as_array().expect("rows").is_empty(),
            "{scenario} must emit rows"
        );
    }
}
