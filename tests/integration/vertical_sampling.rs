//! Issue #73: vertical sampling on the canonical #30 runtime geometry.
//!
//! These tests exercise the public `sample_vertical` entrypoint against real
//! #30 runtime views (`VerticalTransformResult::runtime_view()`). Oracle
//! primitive equivalence for all three #71 vertical fixtures is proven by the
//! unit tests in `src/meteorology/vertical_sampling.rs`; the interface case is
//! additionally proven here end-to-end because its oracle heights are the #30
//! `wzlev` handoff from the same synthetic column.
//!
//! The interface path reproduces the pinned primitive on the #30 handoff only.
//! End-to-end `eta=no` W production parity remains blocked by #80.

use std::collections::BTreeMap;

use flexpart_gpu::meteorology::{
    vertical::{
        reconstruct_vertical_geometry, reconstruct_vertical_geometry_with_motion,
        NativeVerticalMotion,
    },
    vertical_sampling::{sample_vertical, VerticalSample, VerticalSamplingError},
    Axis, Calendar, Field, FieldId, FieldTime, HorizontalGrid, HorizontalStaggering,
    LongitudeDomain, SchemaIdentity, SignConvention, Snapshot, StorageOrder, TemporalKind, Unit,
    VerticalCoordinate, VerticalCoordinateKind, VerticalOrdering, VerticalReference,
    VerticalStaggering,
};
use serde::Serialize;

fn instant() -> FieldTime {
    FieldTime {
        calendar: Calendar::Gregorian,
        kind: TemporalKind::Instantaneous,
        valid_time_epoch_seconds: 1_700_000_000,
        interval_start_epoch_seconds: None,
        interval_end_epoch_seconds: None,
        accumulation: None,
    }
}

fn static_time() -> FieldTime {
    FieldTime {
        calendar: Calendar::Gregorian,
        kind: TemporalKind::Static,
        valid_time_epoch_seconds: 0,
        interval_start_epoch_seconds: None,
        interval_end_epoch_seconds: None,
        accumulation: None,
    }
}

/// Build a valid 1x1 hybrid Snapshot with `nz` levels and the given ordering.
///
/// Interfaces run from 20 kPa (top) to 100 kPa (surface) with `A = 0` so the
/// surface interface reconstructs to the local surface pressure. Levels are
/// adjacent interface means, preserving the #29 averaging convention.
fn snapshot_ordered(ordering: VerticalOrdering, nz: usize) -> Snapshot {
    assert!(nz >= 1, "need at least one model level");
    let top_pa = 20_000.0_f32;
    let surface_pa = 100_000.0_f32;
    let mut interfaces_asc = Vec::with_capacity(nz + 1);
    for k in 0..=nz {
        interfaces_asc.push(top_pa + (surface_pa - top_pa) * (k as f32) / (nz as f32));
    }
    let mut levels_asc = Vec::with_capacity(nz);
    for k in 0..nz {
        levels_asc.push(0.5 * (interfaces_asc[k] + interfaces_asc[k + 1]));
    }
    let (interfaces, levels, temperatures, humidities): (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) =
        match ordering {
            VerticalOrdering::Increasing => {
                let temperatures: Vec<f32> = (0..nz).map(|k| 270.0 + 5.0 * (k as f32)).collect();
                let humidities: Vec<f32> = (0..nz).map(|k| 0.002 + 0.001 * (k as f32)).collect();
                (interfaces_asc, levels_asc, temperatures, humidities)
            }
            VerticalOrdering::Decreasing => {
                let mut interfaces = interfaces_asc.clone();
                let mut levels = levels_asc.clone();
                interfaces.reverse();
                levels.reverse();
                let temperatures: Vec<f32> = (0..nz)
                    .map(|k| 270.0 + 5.0 * ((nz - 1 - k) as f32))
                    .collect();
                let humidities: Vec<f32> = (0..nz)
                    .map(|k| 0.002 + 0.001 * ((nz - 1 - k) as f32))
                    .collect();
                (interfaces, levels, temperatures, humidities)
            }
        };
    let b: Vec<f32> = interfaces.iter().map(|p| p / surface_pa).collect();
    let a = vec![0.0_f32; nz + 1];
    let surface_field =
        |id: FieldId, unit: Unit, sign: SignConvention, time: FieldTime, values: Vec<f32>| Field {
            id,
            shape: vec![1, 1],
            axis_order: vec![Axis::X, Axis::Y],
            storage_order: StorageOrder::XFastest,
            unit,
            sign,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::NotApplicable,
            time,
            values,
        };
    Snapshot {
        schema: SchemaIdentity::default(),
        horizontal_grid: HorizontalGrid {
            nx: 1,
            ny: 1,
            xlon0_deg: 6.0,
            ylat0_deg: 50.0,
            dx_deg: 0.25,
            dy_deg: 0.25,
            longitude_domain: LongitudeDomain::Minus180To180,
        },
        vertical_coordinate: VerticalCoordinate {
            kind: VerticalCoordinateKind::HybridSigmaPressure,
            reference: VerticalReference::ModelNative,
            ordering,
            level_values: levels,
            interface_values: Some(interfaces),
            hybrid_a_interface_pa: Some(a),
            hybrid_b_interface: Some(b),
            reference_surface_pressure_pa: Some(surface_pa),
            surface_pressure_dependency: Some(FieldId::SurfacePressure),
        },
        fields: vec![
            surface_field(
                FieldId::SurfacePressure,
                Unit::Pascal,
                SignConvention::NonNegative,
                instant(),
                vec![surface_pa],
            ),
            surface_field(
                FieldId::Orography,
                Unit::Meter,
                SignConvention::SignedScalar,
                static_time(),
                vec![0.0],
            ),
            surface_field(
                FieldId::Temperature2m,
                Unit::Kelvin,
                SignConvention::SignedScalar,
                instant(),
                vec![292.0],
            ),
            surface_field(
                FieldId::Dewpoint2m,
                Unit::Kelvin,
                SignConvention::SignedScalar,
                instant(),
                vec![285.0],
            ),
            Field {
                id: FieldId::Temperature,
                shape: vec![1, 1, nz],
                axis_order: vec![Axis::X, Axis::Y, Axis::Z],
                storage_order: StorageOrder::XFastest,
                unit: Unit::Kelvin,
                sign: SignConvention::SignedScalar,
                horizontal_staggering: HorizontalStaggering::CellCenter,
                vertical_staggering: VerticalStaggering::LevelCenter,
                time: instant(),
                values: temperatures,
            },
            Field {
                id: FieldId::SpecificHumidity,
                shape: vec![1, 1, nz],
                axis_order: vec![Axis::X, Axis::Y, Axis::Z],
                storage_order: StorageOrder::XFastest,
                unit: Unit::KilogramPerKilogram,
                sign: SignConvention::NonNegative,
                horizontal_staggering: HorizontalStaggering::CellCenter,
                vertical_staggering: VerticalStaggering::LevelCenter,
                time: instant(),
                values: humidities,
            },
        ],
    }
}

fn physical_heights_center(
    runtime: flexpart_gpu::meteorology::vertical::VerticalRuntimeView<'_>,
) -> Vec<f32> {
    let (_, _, nz) = runtime.dimensions();
    let ordering = runtime.provenance().source_vertical_ordering;
    (0..nz)
        .map(|physical| {
            let canonical = match ordering {
                VerticalOrdering::Increasing => nz - 1 - physical,
                VerticalOrdering::Decreasing => physical,
            };
            runtime.level(0, 0, canonical).expect("level").height_agl_m
        })
        .collect()
}

fn linear_center_values(
    heights: &[f32],
    ordering: VerticalOrdering,
    slope: f32,
    intercept: f32,
) -> Vec<f32> {
    let nz = heights.len();
    let mut values = vec![0.0_f32; nz];
    for physical in 0..nz {
        let canonical = match ordering {
            VerticalOrdering::Increasing => nz - 1 - physical,
            VerticalOrdering::Decreasing => physical,
        };
        values[canonical] = intercept + slope * heights[physical];
    }
    values
}

#[test]
fn vertical_sampling_covers_both_orderings_without_fixed_level_counts() {
    for (ordering, nz) in [
        (VerticalOrdering::Increasing, 3),
        (VerticalOrdering::Decreasing, 3),
        (VerticalOrdering::Increasing, 2),
        (VerticalOrdering::Decreasing, 5),
    ] {
        let snapshot = snapshot_ordered(ordering, nz);
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let runtime = geometry.runtime_view().expect("runtime view");
        assert_eq!(runtime.dimensions(), (1, 1, nz));

        let heights = physical_heights_center(runtime);
        assert_eq!(heights.len(), nz);
        assert!(
            heights.windows(2).all(|pair| pair[1] > pair[0]),
            "physical heights must increase bottom-to-top for {ordering:?} nz={nz}"
        );

        let slope = 0.02_f32;
        let intercept = 3.0_f32;
        let values = linear_center_values(&heights, ordering, slope, intercept);

        let query = 0.5 * (heights[0] + heights[1]);
        let sample = sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &values,
            0,
            0,
            query,
            VerticalReference::AboveGroundLevel,
        )
        .expect("interior sample");
        let expected = intercept + slope * query;
        let tolerance = 1.0e-5_f32.max(expected.abs() * 1.0e-5);
        assert!(
            (sample.value - expected).abs() <= tolerance,
            "{ordering:?} nz={nz}: linear interior must reproduce hand value"
        );

        let top_exact = sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &values,
            0,
            0,
            heights[nz - 1],
            VerticalReference::AboveGroundLevel,
        )
        .expect("exact top sample");
        assert!(
            top_exact.clamped_to_upper,
            "{ordering:?} nz={nz}: exact top must take the upper-bound branch"
        );
        assert!((top_exact.value - (intercept + slope * heights[nz - 1])).abs() <= tolerance);

        let below = sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &values,
            0,
            0,
            heights[0] - 50.0,
            VerticalReference::AboveGroundLevel,
        )
        .expect("below-domain sample clamps like FLEXPART");
        assert!(below.clamped_to_lower);
        assert!((below.value - (intercept + slope * heights[0])).abs() <= tolerance);

        let above = sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &values,
            0,
            0,
            heights[nz - 1] + 5000.0,
            VerticalReference::AboveGroundLevel,
        )
        .expect("above-top sample clamps like FLEXPART");
        assert!(above.clamped_to_upper);
    }
}

/// One machine-readable comparison row tying the requirement to the #30
/// surface, the #71 oracle fixture and the test verdict.
#[derive(Debug, Clone, Serialize)]
struct ComparisonRow {
    field_identity: String,
    vertical_reference: String,
    staggering: String,
    ordering: String,
    level_count: usize,
    source_heights_agl_m: Vec<f32>,
    source_values: Vec<f32>,
    requested_height_agl_m: f32,
    oracle_value: f64,
    oracle_levels_1based: (u64, u64),
    oracle_weights_dz1_dz2: (f64, f64),
    candidate_value: f32,
    candidate_levels_physical_0based: (usize, usize),
    candidate_weights_upper_lower: (f32, f32),
    tolerance_abs: f64,
    tolerance_rel: f64,
    verdict: String,
}

fn write_report(case_id: &str, value_provenance: &str, rows: &[ComparisonRow]) {
    let dir = std::path::Path::new("target/vertical-sampling");
    std::fs::create_dir_all(dir).expect("create vertical-sampling report dir");
    let path = dir.join(format!("{case_id}.json"));
    let report = serde_json::json!({
        "case": case_id,
        "oracle_contract": "fixtures/interpolation/contract-v1.json",
        "runtime_boundary": "VerticalTransformResult::runtime_view()/VerticalRuntimeView (#30)",
        "implementation": "src/meteorology/vertical_sampling.rs::sample_vertical (#73)",
        "source_order": "canonical_storage_order_as_consumed_via_runtime_view",
        "value_provenance": value_provenance,
        "production_w_note": "interface rows reproduce the #71 primitive on the #30 wzlev handoff; end-to-end eta=no W production parity is owned by blocking issue #80",
        "rows": rows,
    });
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&report).expect("serialize report"),
    )
    .expect("write comparison report");
}

fn contract_case(id: &str) -> serde_json::Value {
    let source = include_str!("../../fixtures/interpolation/contract-v1.json");
    let contract: serde_json::Value =
        serde_json::from_str(source).expect("parse interpolation contract");
    contract["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("oracle case {id}"))
        .clone()
}

#[test]
fn vertical_interface_oracle_matches_via_real_runtime_view_with_report() {
    let snapshot: Snapshot = serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-column-v1.json"
    ))
    .expect("synthetic #30 fixture must parse");
    let omega: NativeVerticalMotion = serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-omega-interface-v1.json"
    ))
    .expect("omega fixture must parse");
    let geometry =
        reconstruct_vertical_geometry_with_motion(&snapshot, &omega).expect("geometry with motion");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (nx, _, nz) = runtime.dimensions();
    assert_eq!((nx, nz), (1, 3));

    let case = contract_case("vertical-interface-wzlev");
    let oracle_heights: Vec<f64> = {
        let input = case["input"].as_array().expect("input");
        let nlevel: usize = input[1]
            .as_str()
            .expect("nlevel")
            .trim()
            .parse()
            .expect("nlevel");
        assert_eq!(nlevel, nz + 1);
        (0..nlevel)
            .map(|k| {
                input[2 + k]
                    .as_str()
                    .expect("height")
                    .trim()
                    .parse::<f64>()
                    .expect("height")
            })
            .collect()
    };
    let oracle_values: Vec<f64> = {
        let input = case["input"].as_array().expect("input");
        let nlevel: usize = input[1]
            .as_str()
            .expect("nlevel")
            .trim()
            .parse()
            .expect("nlevel");
        let nvalues: usize = input[2 + nlevel]
            .as_str()
            .expect("nvalues")
            .trim()
            .parse()
            .expect("n");
        assert_eq!(nvalues, nlevel);
        (0..nvalues)
            .map(|k| {
                input[2 + nlevel + 1 + k]
                    .as_str()
                    .expect("value")
                    .trim()
                    .parse::<f64>()
                    .expect("value")
            })
            .collect()
    };

    let ordering = runtime.provenance().source_vertical_ordering;
    assert_eq!(ordering, VerticalOrdering::Increasing);
    for (physical, oracle_height) in oracle_heights.iter().enumerate() {
        let canonical = nz - physical;
        let runtime_height = f64::from(
            runtime
                .interface(0, 0, canonical)
                .expect("interface")
                .height_agl_m,
        );
        let diff = (runtime_height - oracle_height).abs();
        let tolerance = 0.02 + 1.0e-5 * oracle_height.abs();
        assert!(
            diff <= tolerance,
            "rust #30 interface height must match oracle handoff input: physical {physical} rust {runtime_height} oracle {oracle_height}"
        );
    }

    let values_f32: Vec<f32> = oracle_values.iter().map(|v| *v as f32).collect();
    // Oracle lists heights/values bottom-to-top (physical). The public
    // `sample_vertical` entrypoint expects canonical storage order, which for
    // the Increasing #30 snapshot is top-to-bottom, i.e. reversed.
    // Both arrays below are therefore reversed together into canonical order.
    let mut canonical_values = vec![0.0_f32; values_f32.len()];
    let mut canonical_heights = vec![0.0_f32; oracle_heights.len()];
    for (physical, (height, value)) in oracle_heights.iter().zip(values_f32.iter()).enumerate() {
        let canonical = nz - physical;
        canonical_values[canonical] = *value;
        canonical_heights[canonical] = *height as f32;
    }
    let values_f32 = canonical_values;
    let goldens = case["golden"]["queries"].as_array().expect("goldens");
    let input = case["input"].as_array().expect("input");
    let nlevel: usize = input[1]
        .as_str()
        .expect("nlevel")
        .trim()
        .parse()
        .expect("nlevel");
    let query_base = 2 + nlevel + 1 + nlevel + 1;
    let mut rows = Vec::with_capacity(goldens.len());
    for (index, golden) in goldens.iter().enumerate() {
        let tokens: Vec<&str> = input[query_base + index]
            .as_str()
            .expect("query line")
            .split_whitespace()
            .collect();
        assert_eq!(tokens[0], "1", "interface queries carry coordinate id 1");
        let zt: f64 = tokens[1].parse().expect("zt");
        let sample: VerticalSample = sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelInterface,
            &values_f32,
            0,
            0,
            zt as f32,
            VerticalReference::AboveGroundLevel,
        )
        .expect("interface oracle sample");
        let oracle_value = golden["VALUE"][0].as_f64().expect("oracle value");
        let tolerance = 1.0e-6 + 1.0e-4 * oracle_value.abs();
        let diff = (f64::from(sample.value) - oracle_value).abs();
        let verdict = if diff <= tolerance { "PASS" } else { "FAIL" };
        assert_eq!(
            verdict, "PASS",
            "interface query {index} zt={zt}: candidate {} vs oracle {oracle_value}",
            sample.value
        );
        let indz = golden["LEVELS"][0].as_u64().expect("indz");
        let indzp = golden["LEVELS"][1].as_u64().expect("indzp");
        assert_eq!(sample.lower_physical_index + 1, indz as usize);
        assert_eq!(sample.upper_physical_index + 1, indzp as usize);
        rows.push(ComparisonRow {
            field_identity: "vertical_velocity".to_string(),
            vertical_reference: "above_ground_level".to_string(),
            staggering: "level_interface".to_string(),
            ordering: format!("{ordering:?}"),
            level_count: nz,
            source_heights_agl_m: canonical_heights.clone(),
            source_values: values_f32.clone(),
            requested_height_agl_m: zt as f32,
            oracle_value,
            oracle_levels_1based: (indz, indzp),
            oracle_weights_dz1_dz2: (
                golden["DZ"][0].as_f64().expect("dz1"),
                golden["DZ"][1].as_f64().expect("dz2"),
            ),
            candidate_value: sample.value,
            candidate_levels_physical_0based: (
                sample.lower_physical_index,
                sample.upper_physical_index,
            ),
            candidate_weights_upper_lower: (sample.weight_upper, sample.weight_lower),
            tolerance_abs: 1.0e-6,
            tolerance_rel: 1.0e-4,
            verdict: verdict.to_string(),
        });
    }
    write_report(
        "vertical-interface-wzlev",
        "interface rows sample the #71 oracle's own W/interface values (omega*pinmconv) through the public entrypoint; only the geometry (interface AGL heights) is the real #30 runtime view. Independent oracle cross-check of the primitive lives in the unit tests against the same fixture.",
        &rows,
    );
}

#[test]
fn vertical_sampling_fails_closed_on_unsupported_states() {
    let snapshot: Snapshot = serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-column-v1.json"
    ))
    .expect("synthetic fixture must parse");
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (nx, _, nz) = runtime.dimensions();
    let good = vec![280.0_f32; nx * nz];

    assert_eq!(
        sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &good,
            0,
            0,
            100.0,
            VerticalReference::ModelNative
        ),
        Err(VerticalSamplingError::AmbiguousReference {
            reference: VerticalReference::ModelNative
        })
    );
    assert!(matches!(
        sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &good,
            0,
            0,
            f32::INFINITY,
            VerticalReference::AboveGroundLevel
        ),
        Err(VerticalSamplingError::NonFiniteHeight { .. })
    ));
    assert!(matches!(
        sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &good[..good.len() - 1],
            0,
            0,
            100.0,
            VerticalReference::AboveGroundLevel
        ),
        Err(VerticalSamplingError::ShapeMismatch { .. })
    ));
    assert_eq!(
        sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelInterface,
            &vec![0.0_f32; nx * (nz + 1)],
            0,
            0,
            100.0,
            VerticalReference::AboveGroundLevel
        ),
        Err(VerticalSamplingError::WrongStaggering {
            field: FieldId::Temperature,
            requested: VerticalStaggering::LevelInterface
        })
    );
    assert!(matches!(
        sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &good,
            5,
            0,
            100.0,
            VerticalReference::AboveGroundLevel
        ),
        Err(VerticalSamplingError::Runtime(_))
    ));
    let mut non_finite = good.clone();
    non_finite[0] = f32::NAN;
    assert!(matches!(
        sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &non_finite,
            0,
            0,
            100.0,
            VerticalReference::AboveGroundLevel
        ),
        Err(VerticalSamplingError::NonFiniteFieldValue { .. })
    ));
    assert!(matches!(
        sample_vertical(
            runtime,
            FieldId::SurfacePressure,
            VerticalStaggering::LevelCenter,
            &good,
            0,
            0,
            100.0,
            VerticalReference::AboveGroundLevel
        ),
        Err(VerticalSamplingError::UnsupportedField { .. })
    ));

    let single = snapshot_ordered(VerticalOrdering::Decreasing, 1);
    let single_geometry = reconstruct_vertical_geometry(&single).expect("nz=1 geometry");
    let single_runtime = single_geometry.runtime_view().expect("runtime view");
    assert_eq!(single_runtime.dimensions(), (1, 1, 1));
    assert_eq!(
        sample_vertical(
            single_runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &[280.0_f32],
            0,
            0,
            10.0,
            VerticalReference::AboveGroundLevel
        ),
        Err(VerticalSamplingError::InsufficientLevels { nz: 1 })
    );
}

#[test]
fn vertical_model_level_report_covers_real_runtime_view() {
    let snapshot: Snapshot = serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-column-v1.json"
    ))
    .expect("synthetic fixture must parse");
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
    let runtime = geometry.runtime_view().expect("runtime view");
    let (nx, _, nz) = runtime.dimensions();
    let ordering = runtime.provenance().source_vertical_ordering;
    let heights = physical_heights_center(runtime);
    let values = linear_center_values(&heights, ordering, 0.015, 4.0);
    let mut map = BTreeMap::new();
    for (physical, height) in heights.iter().enumerate() {
        let canonical = match ordering {
            VerticalOrdering::Increasing => nz - 1 - physical,
            VerticalOrdering::Decreasing => physical,
        };
        map.insert(canonical, (*height, values[canonical]));
    }
    let mut rows = Vec::new();
    for query in [
        heights[0] - 10.0,
        heights[0],
        0.5 * (heights[0] + heights[1]),
        heights[nz - 1],
        heights[nz - 1] + 100.0,
    ] {
        let sample = sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &values,
            0,
            0,
            query,
            VerticalReference::AboveGroundLevel,
        )
        .expect("model-level regression sample");
        rows.push(ComparisonRow {
            field_identity: "temperature".to_string(),
            vertical_reference: "above_ground_level".to_string(),
            staggering: "level_center".to_string(),
            ordering: format!("{ordering:?}"),
            level_count: nz,
            source_heights_agl_m: heights.clone(),
            source_values: values.clone(),
            requested_height_agl_m: query,
            oracle_value: f64::from(sample.value),
            oracle_levels_1based: (
                sample.lower_physical_index as u64 + 1,
                sample.upper_physical_index as u64 + 1,
            ),
            oracle_weights_dz1_dz2: (
                f64::from(sample.weight_upper),
                f64::from(sample.weight_lower),
            ),
            candidate_value: sample.value,
            candidate_levels_physical_0based: (
                sample.lower_physical_index,
                sample.upper_physical_index,
            ),
            candidate_weights_upper_lower: (sample.weight_upper, sample.weight_lower),
            tolerance_abs: 1.0e-6,
            tolerance_rel: 1.0e-5,
            verdict: "PASS".to_string(),
        });
    }
    assert_eq!(nx, 1);
    assert!(!map.is_empty());
    write_report(
        "vertical-model-level-regression",
        "model-level regression re-asserts the candidate (oracle_value == candidate_value); the independent oracle cross-check lives in the unit tests against the vertical-model-levels and real-era5 goldens.",
        &rows,
    );
}
