//! Issue #207: GPU reconstruction of `numpf=1` precipitation members (flex_extract `RRINT=0` darain).
//!
//! Oracle: `fixtures/accumulation/precipitation-darain-oracle-v1.json`, written by
//! `scripts/precipitation/darain_member_oracle.py` from the #202 raw trace of
//! unmodified pinned `deacc_fluxes` and from direct execution of the pinned
//! `disaggregation.darain`. Every value compared here was read back from WGSL
//! execution; real device execution is mandatory and absent adapters fail.

use std::path::PathBuf;

#[path = "support/grib202.rs"]
mod grib202;

use flexpart_gpu::gpu::{
    accumulation::{accumulated_shader_sha256, transform_accumulated_intervals_gpu},
    download_buffer_typed,
    meteorology::CanonicalGpuField,
    precipitation_members::{
        darain_shader_sha256, derive_member_sequence, member_matches_oracle,
        prepare_precipitation_members, reconstruct_members_from_rates_gpu, DisaggregationMethod,
        PrecipitationMemberError, PrecipitationMemberKernels, PrecipitationMemberPreparation,
        PrecipitationMemberStage, MINIMUM_INTERVAL_COUNT, PRECIPITATION_MEMBER_PREPARATION,
        STATUS_NON_FINITE_MEMBER,
    },
    transform_classified_intervals_gpu, AccumulatedTransformInput, AccumulatedTransformInputs,
    ClassifiedTransformInputs, GpuAccumulationError, GpuAdapterEvidence, GpuCalculationEvidence,
    GpuContext, TRANSFORM_MODE_PACKING_ADMISSION,
};
use flexpart_gpu::meteorology::{
    accumulation::{AccumulatedObservation, AccumulatedSeries, AccumulationError},
    Accumulation, Calendar, Field, FieldId, FieldTime, Snapshot, SourceQuantization, TemporalKind,
    Unit,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ORACLE: &str = include_str!("../fixtures/accumulation/precipitation-darain-oracle-v1.json");
const ORACLE_SHA256: &str = "54830e945c438e1f0eb5e8eabcf7aa9746891952d19cd9b8c44a1e4b16997f90";
const PARAMETERS: [(&str, FieldId); 2] = [
    ("lsp", FieldId::LargeScalePrecipitation),
    ("cp", FieldId::ConvectivePrecipitation),
];

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
    evidence: Result<GpuCalculationEvidence, PrecipitationMemberError>,
    oracle: &[f64],
    candidate: &[f32],
) -> Value {
    let (rows, rows_pass) = compare_rows(candidate, oracle);
    let evidence = evidence.expect("evidence builds from a valid device readback");
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

fn lsp_record(snapshot: &mut Snapshot) -> &mut SourceQuantization {
    snapshot
        .fields
        .iter_mut()
        .find(|f| f.id == FieldId::LargeScalePrecipitation)
        .and_then(|f| f.source_quantization.as_mut())
        .expect("genuine LSP record")
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

// --- Phase 1: #90 versus flex_extract deac_vals (#216 admission, #219) --------

/// `max(deac_vals, 0)`, the representation every original consumer uses (#216).
fn projected_deac(deac: f64) -> f64 {
    if deac < 0.0 {
        0.0
    } else {
        deac
    }
}

fn genuine_series(
    observations: &[grib202::Observation],
    parameter: usize,
    cell: usize,
) -> (Vec<AccumulatedObservation>, Vec<SourceQuantization>) {
    observations
        .iter()
        .map(|o| {
            (
                AccumulatedObservation {
                    valid_time_epoch_seconds: o.valid_s,
                    reset_epoch_seconds: o.reset_s,
                    accumulated_amount_kg_per_square_meter: f64::from(
                        o.parameters[parameter].0[cell],
                    ),
                },
                o.parameters[parameter].1.clone(),
            )
        })
        .unzip()
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_precipitation_members_genuine_input_equivalence_matches_deac_vals() {
    let decision = grib202::decision();
    let oracle = grib202::oracle();
    let genuine = &oracle["genuine"];
    let ctx = context();
    let dtime_s = genuine["dtime_hours"].as_i64().expect("dtime") * 3_600;
    let admitted_rows = grib202::admitted_rows(&decision);
    let observations = grib202::observations();
    let mut series = Vec::new();
    let mut admitted = 0;
    for (parameter, (name, field_id)) in PARAMETERS.into_iter().enumerate() {
        let ends = i64s(&genuine["interval_means"][name]["interval_end_s"]);
        let means = genuine["interval_means"][name]["values_mm_per_h"]
            .as_array()
            .expect("means");
        for cell in 0..9 {
            let (accumulated, records) = genuine_series(&observations, parameter, cell);
            let expected_admission = admitted_rows
                .iter()
                .find(|(p, c, _)| (*p, *c) == (parameter, cell))
                .map(|(_, _, index)| *index);
            // Without source records the #75 rejection is unchanged.
            let strict =
                pollster::block_on(transform_accumulated_intervals_gpu(&ctx, &accumulated));
            match (expected_admission, &strict) {
                (
                    Some(index),
                    Err(GpuAccumulationError::Validation(AccumulationError::NegativeDelta {
                        index: rejected,
                        ..
                    })),
                ) => {
                    assert_eq!(*rejected, index, "{name} cell {cell}");
                }
                (None, Ok(_)) => {}
                (_, other) => panic!("{name} cell {cell}: strict path {other:?}"),
            }
            let sources: Vec<_> = records.iter().map(Some).collect();
            let classified = ClassifiedTransformInputs::from_series(&AccumulatedSeries {
                field_id,
                canonical_unit: Unit::KilogramPerSquareMeter,
                grid_id: "regular_ll_3x3_lat52_lon10_step1",
                cell_index: cell,
                observations: &accumulated,
                sources: &sources,
            })
            .expect("genuine series classified");
            let (amounts, rates_si, rates) =
                pollster::block_on(transform_classified_intervals_gpu(&ctx, &classified))
                    .expect("device transform");
            let mut rows = Vec::new();
            for (k, interval) in classified.intervals().iter().enumerate() {
                let deac = f64s(&means[k])[cell];
                assert_eq!(
                    interval.interval_end_epoch_seconds,
                    ends[k] + grib202::ORACLE_EPOCH_UNIX_S
                );
                assert_eq!(interval.duration_seconds, dtime_s, "interval duration");
                let hours = f64::from(u16::try_from(dtime_s / 3_600).expect("hours"));
                let is_admitted = expected_admission == Some(k);
                let mode = classified.inputs()[k].reset_applied;
                assert_eq!(
                    mode == TRANSFORM_MODE_PACKING_ADMISSION,
                    is_admitted,
                    "{name} cell {cell} interval {k}: mode {mode}"
                );
                let expected = projected_deac(deac);
                if is_admitted {
                    for value in [amounts[k], rates_si[k], rates[k]] {
                        assert_eq!(value.to_bits(), 0, "{name} cell {cell}: +0.0 projection");
                    }
                }
                let rate_ok = member_matches_oracle(f64::from(rates[k]), expected);
                let amount_ok = member_matches_oracle(f64::from(amounts[k]), expected * hours);
                assert!(
                    rate_ok && amount_ok && rates[k].to_bits() != (-0.0_f32).to_bits(),
                    "{name} cell {cell} interval {k}: #90 {} vs max(deac, 0) {expected}",
                    rates[k]
                );
                rows.push(json!({"interval_end_s": ends[k], "start_s": interval.interval_start_epoch_seconds,
                    "duration_s": interval.duration_seconds, "reset_applied": interval.reset_applied,
                    "transform_mode": mode,
                    "source_accumulated_kg_per_m2": accumulated[k].accumulated_amount_kg_per_square_meter,
                    "gpu_amount_mm": amounts[k], "gpu_rate_si": rates_si[k],
                    "gpu_rate_mm_per_h": rates[k], "flex_extract_deac_vals_mm_per_h": deac,
                    "expected_rate_mm_per_h": expected, "verdict": "passed"}));
            }
            admitted += classified.admissions().len();
            series.push(
                json!({"parameter": name, "cell": cell, "status": "equivalent",
                "admissions": classified.admissions(), "intervals": rows}),
            );
        }
    }
    assert_eq!((series.len(), admitted), (18, 3));
    write_report(
        "genuine-input-equivalence",
        &json!({"issue": 219, "case": "genuine-input-equivalence",
            "adapter": GpuAdapterEvidence::from_context(&ctx),
            "oracle_sha256": ORACLE_SHA256, "decision_sha256": grib202::DECISION_SHA256,
            "trace_sha256": oracle["consumed"]["trace_sha256"],
            "equivalent_series": series.len(), "admitted_intervals": admitted,
            "rule": "#90 rate = max(deac_vals, 0): +0.0 for the three #216 admissions, #71 policy elsewhere",
            "verdict": "passed", "series": series}),
    );
}

// --- Phase 1b: genuine #202 -> #90 -> #207 production replay (#219) ------------

fn git(args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("git");
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_precipitation_members_genuine_production_replay_same_encoder() {
    let decision = grib202::decision();
    let oracle = grib202::oracle();
    let genuine = &oracle["genuine"];
    let observations = grib202::observations();
    let sources = grib202::snapshots(&observations);
    let ctx = context();
    let adapter = GpuAdapterEvidence::from_context(&ctx);

    // 1-3: both genuine fields enter through the production upload.
    let lsp = upload(&ctx, FieldId::LargeScalePrecipitation, &sources);
    let cp = upload(&ctx, FieldId::ConvectivePrecipitation, &sources);
    let expected_rows = grib202::admitted_rows(&decision);
    let frozen_rows = decision["offending_rows"].as_array().expect("rows");
    let mut admissions = Vec::new();
    for (parameter, field) in [&lsp, &cp].into_iter().enumerate() {
        for admission in field.quantization_admissions() {
            let position = expected_rows
                .iter()
                .position(|row| *row == (parameter, admission.cell_index, admission.interval_index))
                .expect("only the #216 rows are admitted");
            let frozen = &frozen_rows[position];
            assert_eq!(
                admission.bound_exact,
                frozen["bounds"]["canonical_kg_per_m2"]["exact"]
                    .as_str()
                    .expect("B")
            );
            assert_eq!(
                admission.signed_difference_exact,
                frozen["signed_difference"]["canonical_kg_per_m2"]["exact"]
                    .as_str()
                    .expect("d")
            );
            admissions.push(admission.clone());
        }
    }
    assert_eq!(admissions.len(), 3, "exactly the three #216 rows");

    // 4-6: #90 (LSP, CP) and #207 (LSP, CP) in one caller-owned encoder.
    let kernels = PrecipitationMemberKernels::new(&ctx).expect("kernels");
    let mut prepared = prepare_precipitation_members(
        &ctx,
        &lsp,
        &cp,
        PrecipitationMemberPreparation::flex_extract_rrint0(),
    )
    .expect("genuine envelope");
    let metadata = prepared.metadata().clone();
    assert_eq!(
        metadata.large_scale.quantization_admissions,
        lsp.quantization_admissions()
    );
    assert_eq!(
        metadata.convective.quantization_admissions,
        cp.quantization_admissions()
    );
    let mut stripped = metadata.clone();
    stripped.large_scale.quantization_admissions.clear();
    assert_ne!(
        stripped.input_sha256().expect("hash"),
        metadata.input_sha256().expect("hash"),
        "the admission provenance is part of the member input identity"
    );
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("issue_219_genuine_same_encoder"),
        });
    let stages = prepared
        .encode(&ctx, &kernels, &mut encoder)
        .expect("encode")
        .stages
        .to_vec();
    assert_eq!(
        stages,
        vec![
            PrecipitationMemberStage::AccumulatedTransform(FieldId::LargeScalePrecipitation),
            PrecipitationMemberStage::AccumulatedTransform(FieldId::ConvectivePrecipitation),
            PrecipitationMemberStage::DarainReconstruction(FieldId::LargeScalePrecipitation),
            PrecipitationMemberStage::DarainReconstruction(FieldId::ConvectivePrecipitation),
        ]
    );
    // 7: the only submission.
    ctx.queue.submit(Some(encoder.finish()));

    // 8-9: status, then validation-boundary readback only.
    let readback = pollster::block_on(prepared.download(&ctx)).expect("readback");
    readback
        .require_valid_status()
        .expect("device status valid");
    assert_eq!(
        readback.input_sha256(),
        metadata.input_sha256().expect("hash")
    );
    let dtime_s = genuine["dtime_hours"].as_i64().expect("dtime") * 3_600;
    let s0 = genuine["sequence_start_s"].as_i64().expect("s0") + grib202::ORACLE_EPOCH_UNIX_S;
    assert_eq!(
        (
            metadata.delta_seconds,
            metadata.sequence_start_epoch_seconds
        ),
        (dtime_s, s0)
    );
    assert_eq!(metadata.intervals.len(), 16, "no interval is missing");
    for pair in metadata.intervals.windows(2) {
        assert_eq!(
            pair[0].end_epoch_seconds, pair[1].start_epoch_seconds,
            "contiguous"
        );
    }
    let mut lanes = Vec::new();
    let mut series_pass = 0;
    let mut member_values = 0;
    for (parameter, ((name, field_id), (members, source))) in PARAMETERS
        .into_iter()
        .zip([
            (readback.large_scale_mm_per_h().expect("lsp"), &lsp),
            (readback.convective_mm_per_h().expect("cp"), &cp),
        ])
        .enumerate()
    {
        let grid_view = source.interval_rate_grid().expect("rate grid");
        assert_eq!(grid_view.field_id, field_id);
        let rates: Vec<f32> = pollster::block_on(download_buffer_typed(
            &ctx,
            grid_view.rates_mm_per_h,
            16 * 9,
            "issue_90_rates",
        ))
        .expect("#90 rates");
        let means = &genuine["interval_means"][name]["values_mm_per_h"];
        let mut rate_rows = Vec::new();
        for cell in 0..9 {
            let mut series_ok = true;
            for k in 0..16 {
                let deac = f64s(&means[k])[cell];
                let rate = rates[k * 9 + cell];
                let admitted = expected_rows.contains(&(parameter, cell, k));
                let expected = projected_deac(deac);
                let ok = member_matches_oracle(f64::from(rate), expected)
                    && rate.to_bits() != (-0.0_f32).to_bits()
                    && (!admitted || rate.to_bits() == 0);
                series_ok &= ok;
                rate_rows.push(
                    json!({"cell": cell, "interval": k, "gpu_rate_mm_per_h": rate,
                    "gpu_rate_bits": format!("{:08x}", rate.to_bits()),
                    "flex_extract_deac_vals_mm_per_h": deac, "expected_mm_per_h": expected,
                    "admitted_216": admitted, "verdict": if ok { "passed" } else { "failed" }}),
                );
            }
            assert!(series_ok, "{name} cell {cell}: #90 series");
            series_pass += 1;
        }
        let oracle_members = genuine["darain_members"][name].as_array().expect("members");
        assert_eq!(metadata.members.len(), oracle_members.len());
        for (member, expected) in metadata.members.iter().zip(oracle_members) {
            let t = member.valid_time_epoch_seconds;
            assert_eq!(
                t,
                expected["member_time_s"].as_i64().expect("T") + grib202::ORACLE_EPOCH_UNIX_S,
                "exact member timestamp"
            );
            assert_eq!(
                (
                    member.support_start_epoch_seconds,
                    member.support_end_epoch_seconds
                ),
                (t - 2 * dtime_s, t + 2 * dtime_s),
                "four-interval support"
            );
        }
        let oracle_values = member_results(&genuine["darain_members"][name]);
        assert_eq!(oracle_values.len(), 117);
        member_values += oracle_values.len();
        let case_id = format!("genuine-production-replay-{name}");
        let lane = lane_evidence(
            &case_id,
            readback.evidence(&case_id, field_id, &oracle_values),
            &oracle_values,
            members,
        );
        lanes.push(json!({"parameter": name, "interval_rates_90": rate_rows,
            "admissions_216": source.quantization_admissions(), "members": lane}));
    }
    assert_eq!((series_pass, member_values), (18, 234));
    let class = serde_json::to_value(adapter.adapter_class).expect("class");
    let class = class.as_str().expect("class token");
    let grib_inputs: serde_json::Map<String, Value> = grib202::grib_file_hashes(&decision)
        .into_iter()
        .map(|(name, sha)| (name, Value::String(sha)))
        .collect();
    let references = &decision["references"];
    write_report(
        &format!("genuine-production-replay-{class}"),
        &json!({"schema": {"id": "flexpart-gpu.genuine-production-replay-219", "version": 1},
            "issue": 219, "closes_gate_of": 207, "case": "genuine-production-replay",
            "candidate": {"revision": std::env::var("FLEXPART_GPU_CANDIDATE_REVISION")
                    .unwrap_or_else(|_| git(&["rev-parse", "HEAD"])),
                "worktree_clean": git(&["status", "--porcelain"]).is_empty()},
            "references": {"flex_extract": references["flex_extract"]["pinned_commit"],
                "flexpart": references["flexpart"]["pinned_commit"]},
            "decision": {"schema": decision["schema"], "sha256": grib202::DECISION_SHA256,
                "verdict": decision["decision"]["verdict"]},
            "inputs": {"grib_files_sha256": grib_inputs, "darain_oracle_sha256": ORACLE_SHA256,
                "source_snapshot_sha256": metadata.source_snapshot_sha256,
                "horizontal_grid": metadata.horizontal_grid, "input_sha256": metadata.input_sha256().expect("hash")},
            "adapter": adapter,
            "shaders": {"accumulated_interval": accumulated_shader_sha256(), "darain": darain_shader_sha256()},
            "path": ["CanonicalGpuField::upload(LSP)", "CanonicalGpuField::upload(CP)",
                "prepare_precipitation_members", "encode #90 LSP/CP + #207 darain LSP/CP", "submit", "status", "readback"],
            "stages": stages, "encoders": 1, "submissions": 1, "host_transfers_between_stages": 0,
            "admissions_216": admissions,
            "counts": {"interval_series": series_pass, "interval_series_required": 18,
                "darain_members": member_values, "darain_members_required": 234, "admissions": 3},
            "comparison_policy": {"absolute_tolerance": 1e-6, "relative_tolerance": 1e-5},
            "metadata": metadata, "lanes": lanes, "verdict": "passed"}),
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
    readback
        .require_valid_status()
        .expect("device status valid");
    let oracle_values = member_results(members);
    json!({"case": case_id, "intervals": rows.len(), "cells": cells,
        "members": rows.len() - 3, "input_sha256": readback.input_sha256(),
        "shader_sha256": darain_shader_sha256(), "adapter": readback.adapter(),
        "lane": lane_evidence(case_id, readback.evidence(case_id, &oracle_values),
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
        let case_id = format!("production-handoff-{name}");
        let oracle_values = member_results(&params["darain_members"]);
        assert_eq!(readback.input_sha256(), input_sha256);
        let lane = lane_evidence(
            &case_id,
            readback.evidence(&case_id, field_id, &oracle_values),
            &oracle_values,
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
    assert!(
        overflow.evidence("overflow", &[0.0]).is_err(),
        "no evidence from a flagged readback"
    );
    rows.push(json!({"case": "device_overflow_status", "code": "non_finite", "status": overflow.status()}));

    // Genuine #202 example on the production path (#216/#219): without source
    // records the #75 rejection is unchanged; malformed, mismatched, reassociated
    // or unsupported records fail closed at `CanonicalGpuField::upload`.
    let genuine = grib202::observations();
    let (parameter, cell, index) = grib202::admitted_rows(&grib202::decision())[0];
    assert_eq!((parameter, cell), (0, 1), "LSP row of the #216 artifact");
    type GenuineMutation = fn(&mut [Snapshot], usize);
    let mutations: [(&str, &str, GenuineMutation); 6] = [
        ("genuine_202_without_provenance", "decreased", |s, _| {
            for snapshot in s.iter_mut() {
                for field in &mut snapshot.fields {
                    field.source_quantization = None;
                }
            }
        }),
        (
            "genuine_202_wrong_message_hash",
            "PackingProvenanceMismatch",
            |s, i| {
                lsp_record(&mut s[i]).packing.message_sha256 = "0".repeat(64);
            },
        ),
        (
            "genuine_202_era5_class",
            "UnsupportedSourceClass",
            |s, i| {
                lsp_record(&mut s[i]).packing.mars_class = Some("EA".to_owned());
            },
        ),
        (
            "genuine_202_missing_class",
            "UnsupportedSourceClass",
            |s, i| {
                lsp_record(&mut s[i - 1]).packing.mars_class = None;
            },
        ),
        (
            "genuine_202_cp_record_on_lsp",
            "IdentityMismatch",
            |s, i| {
                let cp = s[i]
                    .fields
                    .iter()
                    .find(|f| f.id == FieldId::ConvectivePrecipitation)
                    .and_then(|f| f.source_quantization.clone());
                *lsp_record(&mut s[i]) = cp.expect("CP record");
            },
        ),
        (
            "genuine_202_malformed_record",
            "malformed source-quantization",
            |s, i| {
                lsp_record(&mut s[i])
                    .packing
                    .message_sha256
                    .make_ascii_uppercase();
            },
        ),
    ];
    for (case, expected, mutate) in mutations {
        let mut snapshots = grib202::snapshots(&genuine);
        mutate(&mut snapshots, index);
        let error = try_upload(&ctx, FieldId::LargeScalePrecipitation, &snapshots)
            .err()
            .unwrap_or_else(|| panic!("{case}: upload must fail closed"));
        assert!(error.to_string().contains(expected), "{case}: {error}");
        rows.push(json!({"case": case, "code": expected, "stage": "canonical upload", "error": error.to_string()}));
    }
    // A forged admission mode never reaches the device.
    let forged = AccumulatedTransformInput {
        curr_amount_kg_per_square_meter: 0.5,
        prev_amount_kg_per_square_meter: 1.0,
        duration_seconds: 10_800.0,
        reset_applied: TRANSFORM_MODE_PACKING_ADMISSION,
    };
    let forged_error = AccumulatedTransformInputs::from_inputs(&ctx, &[forged])
        .err()
        .expect("forged admission mode rejected");
    rows.push(json!({"case": "forged_admission_mode", "code": "invalid_transform_input", "stage": "#90 upload", "error": forged_error.to_string()}));

    write_report(
        "fail-closed",
        &json!({"issue": 207, "case": "fail-closed", "rows": rows, "verdict": "passed"}),
    );
}
