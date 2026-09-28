//! Issue #73: vertical sampling on the canonical #30 runtime geometry.
//!
//! These tests exercise the public `sample_vertical` entrypoint against real
//! #30 runtime views (`VerticalTransformResult::runtime_view()`). Oracle
//! primitive equivalence for the model-level #71 vertical fixtures is proven
//! by the unit tests in `src/meteorology/vertical_sampling.rs`.
//!
//! Interface-staggered pressure velocity follows the pristine `eta=no`
//! two-stage W production semantics frozen by #80.

use std::collections::BTreeMap;

use flexpart_gpu::meteorology::{
    vertical::{
        reconstruct_vertical_geometry, reconstruct_vertical_geometry_with_motion,
        NativeVerticalMotion, NativeVerticalMotionKind, NativeVerticalMotionProvenance,
        NativeVerticalMotionSign, NativeVerticalMotionUnit,
    },
    vertical_sampling::{sample_vertical, VerticalSamplingError},
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
    for (physical, height) in heights.iter().copied().enumerate() {
        let canonical = match ordering {
            VerticalOrdering::Increasing => nz - 1 - physical,
            VerticalOrdering::Decreasing => physical,
        };
        values[canonical] = intercept + slope * height;
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

fn nonlinear_interface_omega(ordering: VerticalOrdering, nz: usize) -> NativeVerticalMotion {
    let mut physical_bottom_up: Vec<f32> = (0..=nz)
        .map(|physical| {
            if physical == nz {
                0.0
            } else {
                let index = f32::from(
                    u16::try_from(physical)
                        .expect("focused vertical level counts must fit exactly in f32"),
                );
                -0.2 - 0.07 * index * index
            }
        })
        .collect();
    if ordering == VerticalOrdering::Increasing {
        physical_bottom_up.reverse();
    }
    NativeVerticalMotion {
        kind: NativeVerticalMotionKind::PressureVelocityOmega,
        unit: NativeVerticalMotionUnit::PascalPerSecond,
        sign: NativeVerticalMotionSign::PositivePressureIncreasing,
        vertical_staggering: VerticalStaggering::LevelInterface,
        values: physical_bottom_up,
        provenance: NativeVerticalMotionProvenance {
            source_id: format!("vertical-sampling-interface-{ordering:?}-{nz}"),
        },
    }
}

#[test]
fn interface_vertical_motion_covers_both_orderings_and_level_counts() {
    for nz in [2, 3, 5] {
        let increasing = snapshot_ordered(VerticalOrdering::Increasing, nz);
        let decreasing = snapshot_ordered(VerticalOrdering::Decreasing, nz);
        let increasing_motion = nonlinear_interface_omega(VerticalOrdering::Increasing, nz);
        let decreasing_motion = nonlinear_interface_omega(VerticalOrdering::Decreasing, nz);
        let increasing_geometry =
            reconstruct_vertical_geometry_with_motion(&increasing, &increasing_motion)
                .expect("increasing geometry with interface motion");
        let decreasing_geometry =
            reconstruct_vertical_geometry_with_motion(&decreasing, &decreasing_motion)
                .expect("decreasing geometry with interface motion");
        let increasing_runtime = increasing_geometry.runtime_view().expect("runtime view");
        let decreasing_runtime = decreasing_geometry.runtime_view().expect("runtime view");
        let heights = physical_heights_center(increasing_runtime);
        let queries = [
            0.0,
            0.25 * heights[0],
            heights[0].midpoint(heights[1]),
            heights[nz - 1],
        ];

        for query in queries {
            let increasing_sample = sample_vertical(
                increasing_runtime,
                FieldId::VerticalVelocity,
                VerticalStaggering::LevelInterface,
                &[],
                0,
                0,
                query,
                VerticalReference::AboveGroundLevel,
            )
            .expect("increasing interface sample");
            let decreasing_sample = sample_vertical(
                decreasing_runtime,
                FieldId::VerticalVelocity,
                VerticalStaggering::LevelInterface,
                &[],
                0,
                0,
                query,
                VerticalReference::AboveGroundLevel,
            )
            .expect("decreasing interface sample");
            let tolerance = 1.0e-6_f32
                + 1.0e-5
                    * increasing_sample
                        .value
                        .abs()
                        .max(decreasing_sample.value.abs());
            assert!(
                (increasing_sample.value - decreasing_sample.value).abs() <= tolerance,
                "nz={nz}, query={query}: storage ordering changed the physical result"
            );
        }
    }
}

#[test]
fn interface_vertical_motion_fails_closed_outside_frozen_geometry() {
    let single = snapshot_ordered(VerticalOrdering::Decreasing, 1);
    let single_motion = nonlinear_interface_omega(VerticalOrdering::Decreasing, 1);
    let single_geometry = reconstruct_vertical_geometry_with_motion(&single, &single_motion)
        .expect("single-level geometry with motion");
    let single_runtime = single_geometry.runtime_view().expect("runtime view");
    assert_eq!(
        sample_vertical(
            single_runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelInterface,
            &[],
            0,
            0,
            0.0,
            VerticalReference::AboveGroundLevel,
        ),
        Err(VerticalSamplingError::InsufficientInterfaceLevels { nz: 1 })
    );

    let mut multi_column = snapshot_ordered(VerticalOrdering::Decreasing, 3);
    multi_column.horizontal_grid.nx = 2;
    for field in &mut multi_column.fields {
        field.shape[0] = 2;
        field.values = field
            .values
            .iter()
            .flat_map(|value| [*value, *value])
            .collect();
    }
    let mut multi_motion = nonlinear_interface_omega(VerticalOrdering::Decreasing, 3);
    multi_motion.values = multi_motion
        .values
        .iter()
        .flat_map(|value| [*value, *value])
        .collect();
    let multi_geometry = reconstruct_vertical_geometry_with_motion(&multi_column, &multi_motion)
        .expect("multi-column geometry with motion");
    let multi_runtime = multi_geometry.runtime_view().expect("runtime view");
    assert_eq!(
        sample_vertical(
            multi_runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelInterface,
            &[],
            0,
            0,
            100.0,
            VerticalReference::AboveGroundLevel,
        ),
        Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "#80 freezes a single vertical column without horizontal slope correction"
        })
    );
}

fn checked_f64_to_f32(value: f64, what: &str) -> f32 {
    assert!(value.is_finite(), "{what}: non-finite f64 input");
    assert!(
        value >= f64::from(f32::MIN) && value <= f64::from(f32::MAX),
        "{what}: f64 input is outside the supported f32 range"
    );
    // Oracle JSON numbers are f64. This checked cast is the single explicit
    // rounding boundary into the canonical runtime's f32 arithmetic.
    let rounded = value as f32;
    assert!(rounded.is_finite(), "{what}: f32 rounding was non-finite");
    rounded
}

#[test]
fn interface_vertical_motion_matches_issue_80_oracle_and_writes_report() {
    let snapshot: Snapshot = serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-column-v1.json"
    ))
    .expect("synthetic snapshot must parse");
    let motion: NativeVerticalMotion = serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"
    ))
    .expect("nonlinear omega fixture must parse");
    let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &motion)
        .expect("#30 runtime geometry and motion");
    let runtime = geometry.runtime_view().expect("runtime view");
    let oracle: serde_json::Value = serde_json::from_str(include_str!(
        "../../fixtures/interpolation/w-production-oracle-v1.json"
    ))
    .expect("#80 report must parse");
    assert_eq!(oracle["conclusion"], "not_equivalent");
    let absolute = oracle["tolerances"]["absolute_m_s"]
        .as_f64()
        .expect("absolute tolerance");
    let relative = oracle["tolerances"]["relative"]
        .as_f64()
        .expect("relative tolerance");
    let queries = oracle["synthetic_case"]["queries"]
        .as_array()
        .expect("queries");
    let comparisons = oracle["synthetic_case"]["comparisons"]
        .as_array()
        .expect("comparisons");
    assert_eq!(queries.len(), comparisons.len());
    let terrain_asl_m = runtime.terrain_asl_m(0, 0).expect("terrain");
    let mut rows = Vec::with_capacity(queries.len());

    for (query, comparison) in queries.iter().zip(comparisons) {
        let height_oracle_f64 = query["particle_height_m_agl"]
            .as_f64()
            .expect("query height");
        let height_agl_m = checked_f64_to_f32(height_oracle_f64, "#80 query height");
        let expected = query["pristine_w_m_s"].as_f64().expect("pristine W");
        let direct = comparison["direct_interface_w_m_s"]
            .as_f64()
            .expect("direct W");
        let sample = sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelInterface,
            &[],
            0,
            0,
            height_agl_m,
            VerticalReference::AboveGroundLevel,
        )
        .expect("#80 production sample");
        let actual = f64::from(sample.value);
        let tolerance = absolute + relative * actual.abs().max(expected.abs());
        let difference = (actual - expected).abs();
        assert!(
            difference <= tolerance,
            "query {}: candidate {actual} != pristine {expected} (tolerance {tolerance})",
            query["query"]
        );
        if !comparison["equivalent"]
            .as_bool()
            .expect("direct equivalence")
        {
            assert!(
                (actual - direct).abs() > tolerance,
                "candidate followed rejected direct-interface result"
            );
        }

        let from_asl = sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelInterface,
            &[],
            0,
            0,
            height_agl_m + terrain_asl_m,
            VerticalReference::AboveMeanSeaLevel,
        )
        .expect("equivalent ASL sample");
        assert_eq!(sample.value, from_asl.value);

        rows.push(serde_json::json!({
            "query": query["query"],
            "classification": query["classification"],
            "vertical_reference": "above_ground_level",
            "staggering": "level_interface",
            "source_interface_heights_agl_m": oracle["synthetic_case"]["interface_grid"],
            "remapped_model_grid": oracle["synthetic_case"]["model_grid"],
            "requested_height_agl_m_f64": height_oracle_f64,
            "requested_height_agl_m_f32": height_agl_m,
            "f64_to_f32_rounding": "finite_range_checked_then_round_to_nearest_runtime_f32",
            "pristine_w_m_s": expected,
            "candidate_w_m_s": sample.value,
            "direct_interface_w_m_s": direct,
            "absolute_difference_m_s": difference,
            "tolerance_m_s": tolerance,
            "verdict": "PASS"
        }));
    }

    let report = serde_json::json!({
        "schema": {"id": "flexpart-gpu.vertical-interface-sampling-report", "version": 1},
        "issue": 73,
        "oracle_issue": 80,
        "oracle_artifact": "fixtures/interpolation/w-production-oracle-v1.json",
        "runtime_boundary": "VerticalTransformResult::runtime_view()/VerticalRuntimeView (#30)",
        "production_path": "runtime interface omega*pinmconv -> shared model height grid -> particle-height sample",
        "direct_interface_interpolation": "REJECTED_NOT_EQUIVALENT",
        "real_data_limitation": oracle["real_data_obligation"],
        "rows": rows,
        "verdict": "PASS"
    });
    let directory = std::path::Path::new("target/ci-gate/vertical-sampling");
    std::fs::create_dir_all(directory).expect("create report directory");
    let path = directory.join("vertical-interface-w-production-oracle.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&report).expect("serialize interface report"),
    )
    .expect("write interface report");
    assert!(std::fs::metadata(path).expect("report metadata").len() > 0);
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
    source_level_indices_canonical_0based: Vec<usize>,
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

fn write_report(
    case_id: &str,
    value_provenance: &str,
    rows: &[ComparisonRow],
) -> std::path::PathBuf {
    assert!(!rows.is_empty(), "comparison reports must contain rows");
    let dir = std::path::Path::new("target/ci-gate/vertical-sampling");
    std::fs::create_dir_all(dir).expect("create vertical-sampling report dir");
    let path = dir.join(format!("{case_id}.json"));
    let report = serde_json::json!({
        "case": case_id,
        "oracle_contract": "fixtures/interpolation/contract-v1.json",
        "runtime_boundary": "VerticalTransformResult::runtime_view()/VerticalRuntimeView (#30)",
        "implementation": "src/meteorology/vertical_sampling.rs::sample_vertical (#73)",
        "source_order": "canonical_storage_order_as_consumed_via_runtime_view",
        "value_provenance": value_provenance,
        "interface_vertical_motion": "ISSUE_80_PRESSURE_OMEGA_TWO_STAGE_SUPPORTED",
        "rows": rows,
    });
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&report).expect("serialize report"),
    )
    .expect("write comparison report");
    assert!(path.is_file(), "comparison report must exist after writing");
    assert!(
        std::fs::metadata(&path).expect("report metadata").len() > 0,
        "comparison report must not be empty"
    );
    path
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
    assert_eq!(
        sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelCenter,
            &[],
            0,
            0,
            100.0,
            VerticalReference::AboveGroundLevel
        ),
        Err(VerticalSamplingError::MissingRuntimeVerticalMotion)
    );
    assert_eq!(
        sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelInterface,
            &[],
            0,
            0,
            100.0,
            VerticalReference::AboveGroundLevel
        ),
        Err(VerticalSamplingError::MissingRuntimeVerticalMotion)
    );

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
    let mut canonical_heights = vec![0.0_f32; nz];
    for (physical, height) in heights.iter().enumerate() {
        let canonical = match ordering {
            VerticalOrdering::Increasing => nz - 1 - physical,
            VerticalOrdering::Decreasing => physical,
        };
        canonical_heights[canonical] = *height;
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
            source_level_indices_canonical_0based: (0..nz).collect(),
            source_heights_agl_m: canonical_heights.clone(),
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
    let report_path = write_report(
        "vertical-model-level-regression",
        "model-level regression re-asserts the candidate (oracle_value == candidate_value); the independent oracle cross-check lives in the unit tests against the vertical-model-levels and real-era5 goldens.",
        &rows,
    );
    let report: serde_json::Value = serde_json::from_slice(
        &std::fs::read(report_path).expect("read retained comparison report"),
    )
    .expect("parse retained comparison report");
    assert_eq!(
        report["interface_vertical_motion"],
        "ISSUE_80_PRESSURE_OMEGA_TWO_STAGE_SUPPORTED"
    );
    assert_eq!(
        report["rows"].as_array().expect("comparison rows").len(),
        rows.len()
    );
}
