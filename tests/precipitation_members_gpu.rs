//! Issue #207: GPU reconstruction of `numpf=1` precipitation members (flex_extract `RRINT=0` darain).
//!
//! Oracle: `fixtures/accumulation/precipitation-darain-oracle-v1.json`, written by
//! `scripts/precipitation/darain_member_oracle.py` from the #202 raw trace of
//! unmodified pinned `deacc_fluxes` and from direct execution of the pinned
//! `disaggregation.darain`. Every value compared here was read back from WGSL
//! execution; real device execution is mandatory and absent adapters fail.

use std::path::PathBuf;

use flexpart_gpu::gpu::{
    accumulation::transform_accumulated_intervals_gpu,
    download_buffer_typed,
    meteorology::CanonicalGpuField,
    precipitation_members::{
        build_precipitation_member_evidence, darain_shader_sha256, derive_member_sequence,
        member_matches_oracle, prepare_precipitation_members, reconstruct_members_from_rates_gpu,
        DisaggregationMethod, PrecipitationMemberError, PrecipitationMemberKernels,
        PrecipitationMemberPreparation, PrecipitationMemberStage, MINIMUM_INTERVAL_COUNT,
        PRECIPITATION_MEMBER_PREPARATION, STATUS_NON_FINITE_MEMBER,
    },
    GpuAccumulationError, GpuAdapterEvidence, GpuContext,
};
use flexpart_gpu::meteorology::{
    accumulation::{validate_interval_sequence, AccumulatedObservation, AccumulationError},
    Accumulation, Calendar, Field, FieldId, FieldTime, Snapshot, TemporalKind, Unit,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ORACLE: &str = include_str!("../fixtures/accumulation/precipitation-darain-oracle-v1.json");
const ORACLE_SHA256: &str = "54830e945c438e1f0eb5e8eabcf7aa9746891952d19cd9b8c44a1e4b16997f90";
const PARAMETERS: [(&str, FieldId); 2] = [
    ("lsp", FieldId::LargeScalePrecipitation),
    ("cp", FieldId::ConvectivePrecipitation),
];
/// flex_extract `deacc_fluxes` divides GRIB metres by `fak = 1/1000` (EcFlexpart.py:1124-1131).
const FLEX_EXTRACT_METRE_FACTOR: f64 = 1.0 / 1000.0;

fn oracle() -> Value {
    assert_eq!(
        format!("{:x}", Sha256::digest(ORACLE.as_bytes())),
        ORACLE_SHA256,
        "darain oracle fixture drifted"
    );
    serde_json::from_str(ORACLE).expect("oracle fixture parses")
}

fn context() -> GpuContext {
    pollster::block_on(GpuContext::new()).expect("#207 requires real GPU execution; no skip")
}

fn f64s(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_f64().expect("number"))
        .collect()
}

fn i64s(value: &Value) -> Vec<i64> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_i64().expect("integer"))
        .collect()
}

/// Flatten `[row][cell]` oracle values into `[row * cells + cell]`.
fn planes(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .expect("rows")
        .iter()
        .flat_map(f64s)
        .collect()
}

fn as_f32(values: &[f64]) -> Vec<f32> {
    #[allow(clippy::cast_possible_truncation)]
    values.iter().map(|v| *v as f32).collect()
}

fn member_results(members: &Value) -> Vec<f64> {
    members
        .as_array()
        .expect("members")
        .iter()
        .flat_map(|m| f64s(&m["result_mm_per_h"]))
        .collect()
}

fn write_report(name: &str, report: &Value) {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ci-gate/precipitation-members");
    std::fs::create_dir_all(&dir).expect("report directory");
    std::fs::write(
        dir.join(format!("{name}.json")),
        serde_json::to_string_pretty(report).expect("report serializes"),
    )
    .expect("report written");
}

/// Row-wise #71 comparison of device values with the pinned oracle.
fn compare_rows(candidate: &[f32], oracle: &[f64]) -> (Vec<Value>, bool) {
    assert_eq!(candidate.len(), oracle.len(), "lane length");
    let mut pass = true;
    let rows = candidate
        .iter()
        .zip(oracle)
        .enumerate()
        .map(|(index, (c, o))| {
            let c = f64::from(*c);
            let ok = member_matches_oracle(c, *o);
            pass &= ok;
            let abs = (c - o).abs();
            json!({"index": index, "gpu": c, "oracle": o, "abs_diff": abs,
                   "rel_diff": if *o == 0.0 { 0.0 } else { abs / o.abs() },
                   "verdict": if ok { "passed" } else { "failed" }})
        })
        .collect();
    (rows, pass)
}

fn lane_evidence(
    case_id: &str,
    adapter: &GpuAdapterEvidence,
    input_sha256: &str,
    oracle: &[f64],
    candidate: &[f32],
) -> Value {
    let (rows, rows_pass) = compare_rows(candidate, oracle);
    let evidence =
        build_precipitation_member_evidence(case_id, adapter, input_sha256, oracle, candidate)
            .expect("evidence builds");
    evidence
        .require_paired_pass()
        .expect("GPU members match the pinned darain oracle");
    assert!(rows_pass, "{case_id}: a row fails the #71 rule");
    json!({"rows": rows, "verdict": "passed", "gpu_evidence": evidence})
}

// --- canonical snapshot construction for the production path -----------------

fn snapshot_base(nx: usize, ny: usize, xlon0: f64, ylat0: f64) -> (Snapshot, Field) {
    let mut snapshot: Snapshot = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-column-v1.json"
    ))
    .expect("base snapshot");
    let mut template = snapshot
        .fields
        .iter()
        .find(|f| f.id == FieldId::SurfacePressure)
        .expect("surface template")
        .clone();
    // The hybrid coordinate requires surface pressure; #207 never reads it.
    let mut pressure = template.clone();
    pressure.shape = vec![nx, ny];
    pressure.values = vec![pressure.values[0]; nx * ny];
    snapshot.fields = vec![pressure];
    snapshot.horizontal_grid.nx = nx;
    snapshot.horizontal_grid.ny = ny;
    snapshot.horizontal_grid.xlon0_deg = xlon0;
    snapshot.horizontal_grid.ylat0_deg = ylat0;
    snapshot.horizontal_grid.dx_deg = 1.0;
    snapshot.horizontal_grid.dy_deg = 1.0;
    template.shape = vec![nx, ny];
    template.unit = Unit::KilogramPerSquareMeter;
    (snapshot, template)
}

/// One accumulated observation: `(reset, valid, lsp values, cp values)` in kg/m2.
type Observation = (i64, i64, Vec<f32>, Vec<f32>);

fn accumulated_time(reset: i64, valid: i64) -> FieldTime {
    FieldTime {
        calendar: Calendar::Gregorian,
        kind: TemporalKind::AccumulatedSinceReset,
        valid_time_epoch_seconds: valid,
        interval_start_epoch_seconds: Some(reset),
        interval_end_epoch_seconds: Some(valid),
        accumulation: Some(Accumulation {
            reset_epoch_seconds: reset,
        }),
    }
}

fn snapshots(grid: (usize, usize, f64, f64), observations: &[Observation]) -> Vec<Snapshot> {
    let (base, template) = snapshot_base(grid.0, grid.1, grid.2, grid.3);
    observations
        .iter()
        .map(|(reset, valid, lsp, cp)| {
            let mut snapshot = base.clone();
            snapshot.fields[0].time.valid_time_epoch_seconds = *valid;
            for (id, values) in [
                (FieldId::LargeScalePrecipitation, lsp),
                (FieldId::ConvectivePrecipitation, cp),
            ] {
                let mut field = template.clone();
                field.id = id;
                field.time = accumulated_time(*reset, *valid);
                field.values.clone_from(values);
                snapshot.fields.push(field);
            }
            snapshot
        })
        .collect()
}

fn production_observations(case: &Value) -> Vec<Observation> {
    let rows = |name: &str| {
        case["parameters"][name]["accumulated"]
            .as_array()
            .expect("accumulated")
            .clone()
    };
    rows("lsp")
        .iter()
        .zip(rows("cp").iter())
        .map(|(l, c)| {
            assert_eq!(
                (&l["reset_s"], &l["valid_s"]),
                (&c["reset_s"], &c["valid_s"])
            );
            (
                l["reset_s"].as_i64().expect("reset"),
                l["valid_s"].as_i64().expect("valid"),
                as_f32(&f64s(&l["values_kg_per_m2"])),
                as_f32(&f64s(&c["values_kg_per_m2"])),
            )
        })
        .collect()
}

fn upload<'c>(ctx: &'c GpuContext, id: FieldId, sources: &[Snapshot]) -> CanonicalGpuField<'c> {
    try_upload(ctx, id, sources).expect("canonical accumulated upload")
}

fn try_upload<'c>(
    ctx: &'c GpuContext,
    id: FieldId,
    sources: &[Snapshot],
) -> Result<CanonicalGpuField<'c>, flexpart_gpu::gpu::meteorology::MeteorologyCompositionError> {
    let refs: Vec<&Snapshot> = sources.iter().collect();
    CanonicalGpuField::upload(ctx, id, &refs, &vec![None; refs.len()])
}

fn genuine_observations(genuine: &Value, name: &str, cell: usize) -> Vec<AccumulatedObservation> {
    genuine["accumulated"][name]
        .as_array()
        .expect("observations")
        .iter()
        .map(|o| {
            let metres = f64s(&o["values_m"])[cell];
            // Canonical totals are f32 kg/m2; mirror flex_extract's m -> mm division exactly.
            #[allow(clippy::cast_possible_truncation)]
            let canonical = (metres / FLEX_EXTRACT_METRE_FACTOR) as f32;
            AccumulatedObservation {
                valid_time_epoch_seconds: o["valid_s"].as_i64().expect("valid"),
                reset_epoch_seconds: o["reset_s"].as_i64().expect("reset"),
                accumulated_amount_kg_per_square_meter: f64::from(canonical),
            }
        })
        .collect()
}

// --- Phase 1: #90 versus flex_extract deac_vals --------------------------------

#[test]
fn test_precipitation_members_genuine_input_equivalence_matches_deac_vals() {
    let oracle = oracle();
    let genuine = &oracle["genuine"];
    let ctx = context();
    let dtime_s = genuine["dtime_hours"].as_i64().expect("dtime") * 3_600;
    let negatives = genuine["negative_interval_means"].as_array().expect("list");
    let mut series = Vec::new();
    let (mut admitted, mut rejected) = (0, 0);
    for (name, _) in PARAMETERS {
        let ends = i64s(&genuine["interval_means"][name]["interval_end_s"]);
        let means = genuine["interval_means"][name]["values_mm_per_h"]
            .as_array()
            .expect("means");
        for cell in 0..9 {
            let observations = genuine_observations(genuine, name, cell);
            let expected_negative = negatives
                .iter()
                .find(|n| n["parameter"] == name && n["cell"].as_u64() == Some(cell as u64));
            let result =
                pollster::block_on(transform_accumulated_intervals_gpu(&ctx, &observations));
            if let Some(negative) = expected_negative {
                // #75 fails closed on a same-run decrease that flex_extract passes to darain.
                let end = negative["interval_end_s"].as_i64().expect("end");
                let index = ends.iter().position(|e| *e == end).expect("interval");
                match result {
                    Err(GpuAccumulationError::Validation(AccumulationError::NegativeDelta {
                        index: rejected_index,
                        ..
                    })) => assert_eq!(rejected_index, index, "{name} cell {cell}"),
                    other => panic!("{name} cell {cell}: expected NegativeDelta, got {other:?}"),
                }
                rejected += 1;
                series.push(
                    json!({"parameter": name, "cell": cell, "status": "rejected_by_75",
                    "reason": "same-run negative delta", "interval_end_s": end,
                    "flex_extract_deac_vals_mm_per_h": negative["value_mm_per_h"]}),
                );
                continue;
            }
            let (amounts, _, rates) = result.expect("admitted #90 series");
            let metadata = validate_interval_sequence(&observations).expect("metadata");
            let mut rows = Vec::new();
            for (k, interval) in metadata.iter().enumerate() {
                let deac = f64s(&means[k])[cell];
                assert_eq!(interval.interval_end_epoch_seconds, ends[k]);
                assert_eq!(interval.duration_seconds, dtime_s, "interval duration");
                let hours = f64::from(u16::try_from(dtime_s / 3_600).expect("hours"));
                let rate_ok = member_matches_oracle(f64::from(rates[k]), deac);
                let amount_ok = member_matches_oracle(f64::from(amounts[k]), deac * hours);
                assert!(
                    rate_ok && amount_ok,
                    "{name} cell {cell} interval {k}: #90 {} vs deac {deac}",
                    rates[k]
                );
                rows.push(json!({"interval_end_s": ends[k], "start_s": interval.interval_start_epoch_seconds,
                    "duration_s": interval.duration_seconds, "reset_applied": interval.reset_applied,
                    "source_accumulated_kg_per_m2": observations[k].accumulated_amount_kg_per_square_meter,
                    "gpu_amount_mm": amounts[k], "flex_extract_amount_mm": deac * hours,
                    "gpu_rate_mm_per_h": rates[k], "flex_extract_deac_vals_mm_per_h": deac,
                    "verdict": "passed"}));
            }
            admitted += 1;
            series.push(
                json!({"parameter": name, "cell": cell, "status": "equivalent", "intervals": rows}),
            );
        }
    }
    assert_eq!((admitted, rejected), (15, 3));
    write_report(
        "genuine-input-equivalence",
        &json!({"issue": 207, "case": "genuine-input-equivalence",
            "adapter": GpuAdapterEvidence::from_context(&ctx),
            "oracle_sha256": ORACLE_SHA256, "trace_sha256": oracle["consumed"]["trace_sha256"],
            "admitted_series": admitted, "rejected_series": rejected,
            "verdict": "blocked",
            "blocker": "#75/#90 rejects GRIB-packing same-run decreases that flex_extract deaccumulates (#216); see docs/precipitation-members.md",
            "series": series}),
    );
}

// --- Phase 2/5: kernel against genuine and direct pinned darain ---------------

fn raw_case(ctx: &GpuContext, case_id: &str, means: &Value, members: &Value) -> Value {
    let rows = means.as_array().expect("intervals");
    let cells = f64s(&rows[0]).len();
    let rates = as_f32(&planes(means));
    let readback = pollster::block_on(reconstruct_members_from_rates_gpu(
        ctx,
        &rates,
        rows.len(),
        cells,
    ))
    .expect("GPU darain executes");
    assert_eq!(readback.status(), 0, "{case_id}: device status");
    let oracle_values = member_results(members);
    json!({"case": case_id, "intervals": rows.len(), "cells": cells,
        "members": rows.len() - 3, "input_sha256": readback.input_sha256(),
        "shader_sha256": darain_shader_sha256(), "adapter": readback.adapter(),
        "lane": lane_evidence(case_id, readback.adapter(), readback.input_sha256(),
            &oracle_values, readback.members_mm_per_h())})
}

#[test]
fn test_precipitation_members_genuine_darain_matches_pinned_oracle() {
    let oracle = oracle();
    let genuine = &oracle["genuine"];
    let ctx = context();
    let dtime_s = genuine["dtime_hours"].as_i64().expect("dtime") * 3_600;
    let s0 = genuine["sequence_start_s"].as_i64().expect("s0");
    let rejected_leading = genuine["rejected_leading_persistence"]["member_time_s"]
        .as_i64()
        .expect("leading");
    let mut lanes = Vec::new();
    for (name, _) in PARAMETERS {
        let members = &genuine["darain_members"][name];
        let times: Vec<i64> = members
            .as_array()
            .expect("members")
            .iter()
            .map(|m| m["member_time_s"].as_i64().expect("time"))
            .collect();
        let ends = i64s(&genuine["interval_means"][name]["interval_end_s"]);
        let records: Vec<_> = ends
            .iter()
            .map(
                |end| flexpart_gpu::gpu::meteorology::AccumulatedIntervalMetadataRecord {
                    start_epoch_seconds: end - dtime_s,
                    end_epoch_seconds: *end,
                    duration_seconds: dtime_s,
                    reset_applied: false,
                    detected_reset: false,
                },
            )
            .collect();
        let derived: Vec<i64> = derive_member_sequence(&records)
            .expect("genuine envelope")
            .members
            .iter()
            .map(|m| m.valid_time_epoch_seconds)
            .collect();
        assert_eq!(
            derived, times,
            "member timestamps follow the original writer"
        );
        assert_eq!(times[0], s0 + 2 * dtime_s);
        assert!(
            !times.contains(&rejected_leading),
            "leading persistence is never a member"
        );
        lanes.push(raw_case(
            &ctx,
            &format!("genuine-darain-{name}"),
            &genuine["interval_means"][name]["values_mm_per_h"],
            members,
        ));
    }
    write_report(
        "genuine-darain",
        &json!({"issue": 207, "case": "genuine-darain", "oracle_sha256": ORACLE_SHA256,
            "trace_sha256": oracle["consumed"]["trace_sha256"],
            "negative_inputs_clipped": genuine["negative_interval_means"],
            "lanes": lanes, "verdict": "passed"}),
    );
}

#[test]
fn test_precipitation_members_direct_branches_match_pinned_darain() {
    let oracle = oracle();
    let ctx = context();
    let mut lanes = Vec::new();
    for case in ["darain_branches", "minimum_support"] {
        let data = &oracle["direct"]["cases"][case];
        let cells: Vec<&str> = data["cells"]
            .as_array()
            .expect("cells")
            .iter()
            .map(|c| c.as_str().expect("cell"))
            .collect();
        for required in [
            "zero",
            "constant",
            "alternating_dry_first",
            "alternating_wet_first",
            "isolated_burst",
            "negative_mixed",
            "negative_all",
            "negative_zero_denominators",
        ] {
            assert!(cells.contains(&required), "{case} lacks {required}");
        }
        lanes.push(raw_case(
            &ctx,
            case,
            &data["interval_means_mm_per_h"],
            &data["darain_members"],
        ));
    }
    write_report(
        "direct-darain",
        &json!({"issue": 207, "case": "direct-darain", "oracle_sha256": ORACLE_SHA256,
            "routine": oracle["direct"]["routine"], "lanes": lanes, "verdict": "passed"}),
    );
}

// --- Phase 3/6: production #90 -> #207 handoff in one encoder ----------------

#[test]
#[allow(clippy::too_many_lines)]
fn test_precipitation_members_production_handoff_same_encoder() {
    let oracle = oracle();
    let case = &oracle["direct"]["production_handoff"];
    let grid = (3, 2, 10.0, 50.0);
    let sources = snapshots(grid, &production_observations(case));
    let ctx = context();
    let lsp = upload(&ctx, FieldId::LargeScalePrecipitation, &sources);
    let cp = upload(&ctx, FieldId::ConvectivePrecipitation, &sources);
    let kernels = PrecipitationMemberKernels::new(&ctx).expect("kernels");
    let mut prepared = prepare_precipitation_members(
        &ctx,
        &lsp,
        &cp,
        PrecipitationMemberPreparation::flex_extract_rrint0(),
    )
    .expect("supported envelope");

    let dtime_s = 3 * 3_600;
    let metadata = prepared.metadata().clone();
    assert_eq!(metadata.representation, PRECIPITATION_MEMBER_PREPARATION);
    assert_eq!(metadata.unit, "millimeter_per_hour");
    assert_eq!(
        (
            metadata.delta_seconds,
            metadata.sequence_start_epoch_seconds
        ),
        (dtime_s, 0)
    );
    assert_eq!(metadata.cell_count, 6);
    assert_eq!(metadata.intervals.len(), 7);
    assert!(metadata.intervals[4].reset_applied, "second declared run");
    assert_eq!(metadata.source_snapshot_sha256.len(), 7);
    let times: Vec<i64> = metadata
        .members
        .iter()
        .map(|m| m.valid_time_epoch_seconds)
        .collect();
    assert_eq!(
        times,
        vec![2 * dtime_s, 3 * dtime_s, 4 * dtime_s, 5 * dtime_s]
    );
    for member in &metadata.members {
        let t = member.valid_time_epoch_seconds;
        assert_eq!(
            (
                member.support_start_epoch_seconds,
                member.support_end_epoch_seconds
            ),
            (t - 2 * dtime_s, t + 2 * dtime_s)
        );
        assert_eq!(member.support_intervals[1].end_epoch_seconds, t);
    }
    for (time, code) in [
        (dtime_s, "outside_coverage"),     // leading persistence position
        (6 * dtime_s, "outside_coverage"), // BASETIME trailing position
        (2 * dtime_s + 1_800, "member_spacing_mismatch"),
    ] {
        let error = metadata.member_at(time).expect_err("no member");
        assert_eq!(error.rejection_code(), Some(code));
    }

    // #119-style handoff: select (T_j, T_j+1) by metadata only, copy on device.
    let first = metadata.member_at(3 * dtime_s).expect("T_j").clone();
    let second = metadata.member_at(4 * dtime_s).expect("T_j+1").clone();
    let (first_offset, plane) = metadata.plane_byte_range(&first);
    let (second_offset, _) = metadata.plane_byte_range(&second);
    let consumer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("issue_119_handoff_consumer"),
        size: 4 * plane,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("issue_207_same_encoder"),
        });
    let (stages, member_bytes, status_bytes) = {
        let encoded = prepared
            .encode(&ctx, &kernels, &mut encoder)
            .expect("encode");
        for (index, buffer) in [encoded.large_scale_mm_per_h, encoded.convective_mm_per_h]
            .into_iter()
            .enumerate()
        {
            let base = index as u64 * 2 * plane;
            encoder.copy_buffer_to_buffer(buffer, first_offset, &consumer, base, plane);
            encoder.copy_buffer_to_buffer(buffer, second_offset, &consumer, base + plane, plane);
        }
        (
            encoded.stages.to_vec(),
            encoded.large_scale_mm_per_h.size(),
            encoded.status.size(),
        )
    };
    assert_eq!(
        stages,
        vec![
            PrecipitationMemberStage::AccumulatedTransform(FieldId::LargeScalePrecipitation),
            PrecipitationMemberStage::AccumulatedTransform(FieldId::ConvectivePrecipitation),
            PrecipitationMemberStage::DarainReconstruction(FieldId::LargeScalePrecipitation),
            PrecipitationMemberStage::DarainReconstruction(FieldId::ConvectivePrecipitation),
        ]
    );
    ctx.queue.submit(Some(encoder.finish()));

    // Validation boundary only after the single submission.
    let readback = pollster::block_on(prepared.download(&ctx)).expect("readback");
    readback
        .require_valid_status()
        .expect("device status valid");
    let consumer_values: Vec<f32> =
        pollster::block_on(download_buffer_typed(&ctx, &consumer, 24, "consumer"))
            .expect("consumer");
    let input_sha256 = metadata.input_sha256().expect("input hash");
    let mut lanes = Vec::new();
    for ((name, field_id), (values, source)) in PARAMETERS.into_iter().zip([
        (readback.large_scale_mm_per_h().expect("lsp"), &lsp),
        (readback.convective_mm_per_h().expect("cp"), &cp),
    ]) {
        let params = &case["parameters"][name];
        let grid_view = source.interval_rate_grid().expect("rate grid");
        assert_eq!(grid_view.field_id, field_id);
        let rates: Vec<f32> = pollster::block_on(download_buffer_typed(
            &ctx,
            grid_view.rates_mm_per_h,
            7 * 6,
            "issue_90_rates",
        ))
        .expect("#90 rates");
        let (rate_rows, rates_ok) =
            compare_rows(&rates, &planes(&params["interval_means_mm_per_h"]));
        assert!(
            rates_ok,
            "{name}: #90 rates equal the oracle's darain inputs"
        );
        let lane = lane_evidence(
            &format!("production-handoff-{name}"),
            readback.adapter(),
            &input_sha256,
            &member_results(&params["darain_members"]),
            values,
        );
        let index = usize::from(name == "cp");
        let cells = 6;
        let start = index * 2 * cells;
        assert_eq!(
            &consumer_values[start..start + cells],
            &values[first.slot * cells..(first.slot + 1) * cells]
        );
        assert_eq!(
            &consumer_values[start + cells..start + 2 * cells],
            &values[second.slot * cells..(second.slot + 1) * cells]
        );
        lanes.push(json!({"parameter": name, "darain_inputs_from_90": rate_rows, "members": lane}));
    }
    write_report(
        "production-handoff",
        &json!({"issue": 207, "case": "production-handoff", "oracle_sha256": ORACLE_SHA256,
            "adapter": readback.adapter(), "shader_sha256": darain_shader_sha256(),
            "input_sha256": input_sha256, "stages": stages, "submissions": 1,
            "host_transfers_between_stages": 0,
            "buffers": {"lsp_members_bytes": member_bytes, "cp_members_bytes": member_bytes,
                "status_bytes": status_bytes, "member_count": metadata.members.len(), "cell_count": 6,
                "layout": "[slot * cell_count + cell], x-fastest"},
            "handoff_119": {"pair": [first.valid_time_epoch_seconds, second.valid_time_epoch_seconds],
                "device_copies": 4, "verdict": "passed"},
            "metadata": metadata, "lanes": lanes, "verdict": "passed"}),
    );
}

// --- Phase 4: fail-closed matrix ------------------------------------------------

fn expect_code(
    result: Result<impl Sized, PrecipitationMemberError>,
    code: &str,
    what: &str,
) -> Value {
    let Err(error) = result else {
        panic!("{what}: unexpectedly admitted");
    };
    assert_eq!(error.rejection_code(), Some(code), "{what}: {error}");
    json!({"case": what, "code": code, "error": error.to_string()})
}

fn uniform_observations(count: i64, delta: i64, total: f32) -> Vec<Observation> {
    (0..count)
        .map(|k| {
            #[allow(clippy::cast_precision_loss)]
            let value = total * (k + 1) as f32;
            (0, (k + 1) * delta, vec![value; 6], vec![value / 2.0; 6])
        })
        .collect()
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_precipitation_members_fail_closed_matrix() {
    let ctx = context();
    let grid = (3, 2, 10.0, 50.0);
    let delta = 3 * 3_600;
    let sources = snapshots(grid, &uniform_observations(6, delta, 0.75));
    let lsp = upload(&ctx, FieldId::LargeScalePrecipitation, &sources);
    let cp = upload(&ctx, FieldId::ConvectivePrecipitation, &sources);
    let supported = PrecipitationMemberPreparation::flex_extract_rrint0();
    prepare_precipitation_members(&ctx, &lsp, &cp, supported).expect("baseline admitted");
    let mut rows = Vec::new();

    for (what, preparation) in [
        (
            "rrint1_ia3",
            PrecipitationMemberPreparation {
                rrint: 1,
                method: DisaggregationMethod::Ia3,
                ..supported
            },
        ),
        (
            "purefc1",
            PrecipitationMemberPreparation {
                purefc: 1,
                ..supported
            },
        ),
        (
            "basetime_boundary",
            PrecipitationMemberPreparation {
                basetime_hour: Some(0),
                ..supported
            },
        ),
        (
            "numpf3",
            PrecipitationMemberPreparation {
                numpf: 3,
                ..supported
            },
        ),
        (
            "dapoly",
            PrecipitationMemberPreparation {
                method: DisaggregationMethod::Dapoly,
                ..supported
            },
        ),
    ] {
        rows.push(expect_code(
            prepare_precipitation_members(&ctx, &lsp, &cp, preparation),
            "unsupported_preparation",
            what,
        ));
    }
    rows.push(expect_code(
        prepare_precipitation_members(&ctx, &cp, &lsp, supported),
        "identity_mismatch",
        "swapped_field_order",
    ));

    let short = snapshots(grid, &uniform_observations(4, delta, 0.75));
    let (sl, sc) = (
        upload(&ctx, FieldId::LargeScalePrecipitation, &short),
        upload(&ctx, FieldId::ConvectivePrecipitation, &short),
    );
    rows.push(expect_code(
        prepare_precipitation_members(&ctx, &sl, &sc, supported),
        "insufficient_support",
        "four_intervals",
    ));
    assert_eq!(MINIMUM_INTERVAL_COUNT, 5);

    let half_hour = snapshots(grid, &uniform_observations(6, 1_800, 0.75));
    let (hl, hc) = (
        upload(&ctx, FieldId::LargeScalePrecipitation, &half_hour),
        upload(&ctx, FieldId::ConvectivePrecipitation, &half_hour),
    );
    rows.push(expect_code(
        prepare_precipitation_members(&ctx, &hl, &hc, supported),
        "unequal_or_non_integer_hour_delta",
        "non_integer_hour_delta",
    ));

    let mut uneven = uniform_observations(6, delta, 0.75);
    uneven[5].1 += 3_600;
    let uneven = snapshots(grid, &uneven);
    let (ul, uc) = (
        upload(&ctx, FieldId::LargeScalePrecipitation, &uneven),
        upload(&ctx, FieldId::ConvectivePrecipitation, &uneven),
    );
    rows.push(expect_code(
        prepare_precipitation_members(&ctx, &ul, &uc, supported),
        "unequal_or_non_integer_hour_delta",
        "unequal_delta",
    ));

    // A missing observation inside a run becomes a 2Δ interval, never a zero interval.
    let mut missing = uniform_observations(7, delta, 0.75);
    missing.remove(3);
    let missing = snapshots(grid, &missing);
    let (ml, mc) = (
        upload(&ctx, FieldId::LargeScalePrecipitation, &missing),
        upload(&ctx, FieldId::ConvectivePrecipitation, &missing),
    );
    rows.push(expect_code(
        prepare_precipitation_members(&ctx, &ml, &mc, supported),
        "unequal_or_non_integer_hour_delta",
        "missing_interval",
    ));

    // A declared reset after an uncovered gap is rejected by #75 before #207.
    let mut gap = uniform_observations(6, delta, 0.75);
    for (index, observation) in gap.iter_mut().enumerate().skip(3) {
        observation.0 = 4 * delta;
        observation.1 = (i64::try_from(index).expect("index") + 2) * delta;
    }
    let gap_error = try_upload(
        &ctx,
        FieldId::LargeScalePrecipitation,
        &snapshots(grid, &gap),
    )
    .err()
    .expect("gap rejected");
    rows.push(json!({"case": "noncontiguous_reset_gap", "code": "gap_or_overlap", "stage": "#75 upload", "error": gap_error.to_string()}));

    let mut other_values = uniform_observations(6, delta, 0.75);
    // Identical CP values, but CP comes from snapshots whose LSP lane differs.
    other_values[5].2[0] += 0.5;
    let other = snapshots(grid, &other_values);
    let other_cp = upload(&ctx, FieldId::ConvectivePrecipitation, &other);
    rows.push(expect_code(
        prepare_precipitation_members(&ctx, &lsp, &other_cp, supported),
        "identity_mismatch",
        "mixed_source_snapshots",
    ));

    let shifted = snapshots((3, 2, 11.0, 50.0), &uniform_observations(6, delta, 0.75));
    let shifted_cp = upload(&ctx, FieldId::ConvectivePrecipitation, &shifted);
    rows.push(expect_code(
        prepare_precipitation_members(&ctx, &lsp, &shifted_cp, supported),
        "identity_mismatch",
        "mixed_grid",
    ));

    let other_ctx = context();
    let foreign_cp = upload(&other_ctx, FieldId::ConvectivePrecipitation, &sources);
    rows.push(expect_code(
        prepare_precipitation_members(&ctx, &lsp, &foreign_cp, supported),
        "identity_mismatch",
        "mixed_context",
    ));

    // ERA5 class EA-style interval totals carry no #202 accumulation evidence.
    let mut totals = sources.clone();
    for (index, snapshot) in totals.iter_mut().enumerate() {
        for field in snapshot
            .fields
            .iter_mut()
            .filter(|f| f.id != FieldId::SurfacePressure)
        {
            let end = (i64::try_from(index).expect("index") + 1) * delta;
            field.time = FieldTime {
                calendar: Calendar::Gregorian,
                kind: TemporalKind::IntervalTotal,
                valid_time_epoch_seconds: end,
                interval_start_epoch_seconds: Some(end - delta),
                interval_end_epoch_seconds: Some(end),
                accumulation: None,
            };
            field.values.fill(0.75);
        }
    }
    let (tl, tc) = (
        upload(&ctx, FieldId::LargeScalePrecipitation, &totals),
        upload(&ctx, FieldId::ConvectivePrecipitation, &totals),
    );
    rows.push(expect_code(
        prepare_precipitation_members(&ctx, &tl, &tc, supported),
        "unsupported_preparation",
        "interval_total_convention",
    ));

    let mut nan = uniform_observations(6, delta, 0.75);
    nan[2].2[1] = f32::NAN;
    let nan_error = try_upload(
        &ctx,
        FieldId::LargeScalePrecipitation,
        &snapshots(grid, &nan),
    )
    .err()
    .expect("NaN rejected");
    rows.push(json!({"case": "non_finite_accumulation", "code": "non_finite", "stage": "canonical upload", "error": nan_error.to_string()}));

    let mut raw = vec![1.0_f32; 5 * 2];
    raw[7] = f32::INFINITY;
    rows.push(expect_code(
        pollster::block_on(reconstruct_members_from_rates_gpu(&ctx, &raw, 5, 2)),
        "non_finite",
        "non_finite_raw_rate",
    ));
    rows.push(expect_code(
        pollster::block_on(reconstruct_members_from_rates_gpu(&ctx, &[1.0; 8], 4, 2)),
        "insufficient_support",
        "raw_four_intervals",
    ));

    // f32 overflow inside darain is flagged by the device status word, never accepted.
    let overflow = pollster::block_on(reconstruct_members_from_rates_gpu(&ctx, &[3.0e38; 5], 5, 1))
        .expect("dispatch");
    assert_ne!(
        overflow.status() & STATUS_NON_FINITE_MEMBER,
        0,
        "device status flags overflow"
    );
    rows.push(json!({"case": "device_overflow_status", "code": "non_finite", "status": overflow.status()}));

    // Genuine #202 example on the production path: blocked by #75 (see Phase 1).
    let oracle = oracle();
    let genuine = &oracle["genuine"];
    let cells = |name: &str, k: usize| -> Vec<f32> {
        (0..9)
            .map(|cell| {
                genuine_observations(genuine, name, cell)[k].accumulated_amount_kg_per_square_meter
            })
            .collect::<Vec<f64>>()
            .iter()
            .map(|v| as_f32(&[*v])[0])
            .collect()
    };
    let times = genuine_observations(genuine, "lsp", 0);
    let genuine_obs: Vec<Observation> = times
        .iter()
        .enumerate()
        .map(|(k, o)| {
            (
                o.reset_epoch_seconds,
                o.valid_time_epoch_seconds,
                cells("lsp", k),
                cells("cp", k),
            )
        })
        .collect();
    let genuine_error = try_upload(
        &ctx,
        FieldId::LargeScalePrecipitation,
        &snapshots((3, 3, 10.0, 50.0), &genuine_obs),
    )
    .err()
    .expect("genuine LSP blocked by #75");
    assert!(
        genuine_error.to_string().contains("negative")
            || genuine_error.to_string().contains("decreas"),
        "{genuine_error}"
    );
    rows.push(json!({"case": "genuine_202_production_path", "code": "blocked_by_75", "error": genuine_error.to_string()}));

    write_report(
        "fail-closed",
        &json!({"issue": 207, "case": "fail-closed", "rows": rows, "verdict": "passed"}),
    );
}
