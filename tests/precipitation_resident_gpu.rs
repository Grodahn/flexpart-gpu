//! #227 pair-shared resident precipitation proof: genuine accumulated GRIB ->
//! #90 -> #207 once -> per-step #171 particle lanes -> #87 -> lane-parallel #119.
//! One encoder and one submission; readback happens only after completion.

use std::path::PathBuf;

#[path = "support/grib202.rs"]
mod grib202;

use flexpart_gpu::gpu::{
    accumulation::accumulated_shader_sha256,
    compare_finite_values, download_buffer_typed, horizontal_shader_sha256,
    meteorology::{
        resident::{ResidentQueryBatch, ResidentQueryKernels},
        CanonicalGpuField,
    },
    precipitation_members::{
        darain_shader_sha256, member_matches_oracle, prepare_precipitation_members,
        PrecipitationMemberKernels, PrecipitationMemberPreparation, PreparedPrecipitationMembers,
        STATUS_NOT_EXECUTED,
    },
    precipitation_resident::{
        precipitation_resident_publish_shader_sha256, precipitation_resident_rates_shader_sha256,
        prepare_resident_precipitation_rates, ResidentPrecipitationKernels,
        ResidentPrecipitationMetadata, ResidentPrecipitationStage,
        RESIDENT_PRECIPITATION_PRODUCER_SHIFT,
    },
    precipitation_temporal::{
        precipitation_temporal_shader_sha256, prepare_precipitation_time_samples,
        PrecipitationTemporalError, PrecipitationTemporalKernels, PrecipitationTimeQuery,
    },
    ComparisonPolicy, GpuAdapterEvidence, GpuCalculationEvidence, GpuCalculationPath,
    GpuCandidateEvidence, GpuContext, GpuEvidenceSchema, GpuExecutionEvidence, GpuExecutionStatus,
    ParticleBuffers, PinnedOracleEvidence,
};
use flexpart_gpu::meteorology::{FieldId, Snapshot};
use flexpart_gpu::particles::{Particle, ParticleInit};
use serde_json::{json, Value};

const ORIGINAL: &str = include_str!("../fixtures/interpolation/precipitation-temporal-v1.json");
const CAPACITY: u32 = 16;
const ACTIVE: u32 = 14;
/// Lanes holding the 12 original positions, in original position order.
const POSITION_LANES: [usize; 12] = [0, 1, 2, 4, 5, 6, 7, 9, 10, 11, 12, 13];
/// Flag-inactive lanes inside the active prefix.
const INACTIVE_LANES: [usize; 2] = [3, 8];
/// Lanes beyond the active prefix.
const TAIL_LANES: [usize; 2] = [14, 15];
/// Scalability control: capacity that must not change the dispatch count.
const LARGE_CAPACITY: u32 = 4096;
/// Step-owned bytes per lane: four #87 query/sample pairs, one adapter
/// height, candidate and published `[LSP, CP]`.
const PER_LANE_BYTES: u64 = 4 * (32 + 4) + 4 + 8 + 8;
/// Upper bound of constant-size control buffers (uniforms, flags).
const CONTROL_BYTES: u64 = 128;
const QUIET_NAN_BITS: u32 = 0x7fc0_0000;

struct Row {
    case: Value,
    x: f64,
    y: f64,
    lsp: f64,
    cp: f64,
}

struct Step {
    itime: i64,
    rows: Vec<Row>,
}

fn git(args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("git");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("UTF8")
        .trim()
        .to_owned()
}

fn candidate_revision() -> String {
    std::env::var("FLEXPART_GPU_CANDIDATE_REVISION").unwrap_or_else(|_| git(&["rev-parse", "HEAD"]))
}

fn upload<'ctx>(
    ctx: &'ctx GpuContext,
    id: FieldId,
    snapshots: &[Snapshot],
) -> CanonicalGpuField<'ctx> {
    CanonicalGpuField::upload(
        ctx,
        id,
        &snapshots.iter().collect::<Vec<_>>(),
        &vec![None; snapshots.len()],
    )
    .expect("genuine canonical upload")
}

fn report_path(class: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "target/ci-gate/precipitation-resident/report-{class}.json"
    ))
}

/// Original rows grouped into forward steps; each step shares one `itime`.
fn original_steps(original: &Value) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    for case in original["cases"].as_array().expect("cases") {
        if case["id"] == "arithmetic-control" {
            continue;
        }
        for row in case["rows"].as_array().expect("rows") {
            let itime =
                grib202::ORACLE_EPOCH_UNIX_S + row["query"][2].as_i64().expect("original itime");
            if steps.last().is_none_or(|s| s.itime != itime) {
                steps.push(Step {
                    itime,
                    rows: Vec::new(),
                });
            }
            steps.last_mut().expect("step").rows.push(Row {
                case: case["id"].clone(),
                x: row["query"][0].as_f64().expect("x"),
                y: row["query"][1].as_f64().expect("y"),
                lsp: row["lsp"].as_f64().expect("LSP"),
                cp: row["cp"].as_f64().expect("CP"),
            });
        }
    }
    steps
}

#[allow(clippy::cast_possible_truncation, clippy::float_cmp)]
fn particle_at(x: f64, y: f64) -> Particle {
    let (cell_x, cell_y) = (x.floor(), y.floor());
    let fraction_x = (x - cell_x) as f32;
    let fraction_y = (y - cell_y) as f32;
    // The fixture coordinates are dyadic, so the split matches #87's f64 query.
    assert_eq!(f64::from(fraction_x), x - cell_x);
    assert_eq!(f64::from(fraction_y), y - cell_y);
    Particle::new(&ParticleInit {
        cell_x: cell_x as i32,
        cell_y: cell_y as i32,
        pos_x: fraction_x,
        pos_y: fraction_y,
        pos_z: 0.0,
        mass: [1.0, 0.0, 0.0, 0.0],
        release_point: 0,
        class: 0,
        time: 0,
    })
}

/// Lane buffer with flag-inactive lanes and an active, never-read tail;
/// both carry NaN coordinates that must never fail or be sampled.
fn lane_particles(ctx: &GpuContext, positions: &[(f64, f64)]) -> ParticleBuffers {
    let mut garbage = particle_at(0.0, 0.0);
    garbage.pos_x = f32::NAN;
    garbage.cell_y = -7;
    let mut host = vec![garbage; CAPACITY as usize];
    for lane in INACTIVE_LANES {
        host[lane].deactivate();
    }
    for (lane, (x, y)) in POSITION_LANES.iter().zip(positions) {
        host[*lane] = particle_at(*x, *y);
    }
    let mut particles = ParticleBuffers::from_particles(ctx, &host);
    particles.set_dispatch_count(ACTIVE as usize);
    particles
}

fn bits(values: &[f32]) -> Vec<String> {
    values
        .iter()
        .map(|v| format!("{:08x}", v.to_bits()))
        .collect()
}

fn audit(metadata: &ResidentPrecipitationMetadata, stages: &[ResidentPrecipitationStage]) -> Value {
    let bound =
        metadata.plane_bytes + PER_LANE_BYTES * u64::from(metadata.capacity) + CONTROL_BYTES;
    json!({"capacity": metadata.capacity, "active_count": metadata.active_count,
        "plane_bytes": metadata.plane_bytes, "device_bytes": metadata.device_bytes, "bound_bytes": bound,
        "dispatches": stages.iter().filter(|s| s.is_dispatch()).count(),
        "copies": stages.iter().filter(|s| !s.is_dispatch()).count(), "stages": stages})
}

fn time_class(itime: i64, member_times: &[i64]) -> &'static str {
    if Some(&itime) == member_times.last() {
        "final"
    } else if member_times.contains(&itime) {
        "exact_interior_member"
    } else {
        "interior"
    }
}

/// Existing #119 bounded batch API on the same queries, for bitwise identity.
fn batch_api_rates(
    ctx: &GpuContext,
    producer: &mut PreparedPrecipitationMembers<'_, '_>,
    steps: &[Step],
) -> Vec<f32> {
    let queries: Vec<_> = steps
        .iter()
        .flat_map(|step| {
            step.rows.iter().map(|row| PrecipitationTimeQuery {
                xt: row.x,
                yt: row.y,
                itime_epoch_seconds: step.itime,
            })
        })
        .collect();
    let kernels = PrecipitationTemporalKernels::new(ctx).expect("#119 kernels");
    let mut plan =
        prepare_precipitation_time_samples(ctx, producer, &queries, 1).expect("#119 batch");
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    plan.encode(ctx, &kernels, &mut encoder).expect("#119");
    ctx.queue.submit(Some(encoder.finish()));
    pollster::block_on(plan.download(ctx)).expect("#119 batch rates")
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_precipitation_resident_genuine_pair_shared_matches_batch_and_original() {
    let requested_class = if std::env::var("FLEXPART_GPU_SOFTWARE").is_ok_and(|v| v == "1") {
        "software_wgsl"
    } else {
        "hardware_gpu"
    };
    let pending = report_path(requested_class);
    std::fs::create_dir_all(pending.parent().expect("parent")).expect("evidence directory");
    std::fs::write(&pending, "{\"verdict\":\"not_executed\"}\n").expect("invalidate before setup");
    assert_eq!(
        grib202::sha256_hex(ORIGINAL.as_bytes()),
        "8654e238cfe94293bb69e681eb4a4a29000bf83d46fe76a34bcaba62d19c8488",
        "original fixture drifted"
    );
    let original = grib202::exact_json(ORIGINAL);
    let steps = original_steps(&original);
    assert_eq!(steps.len(), 49);
    assert!(steps.iter().all(|s| s.rows.len() == POSITION_LANES.len()));
    let snapshots = grib202::snapshots(&grib202::observations());
    let ctx = pollster::block_on(GpuContext::new()).expect("required #227 GPU, no skip");
    let adapter = GpuAdapterEvidence::from_context(&ctx);
    let class = serde_json::to_value(adapter.adapter_class).expect("class");
    let path = report_path(class.as_str().expect("class"));
    std::fs::write(&path, "{\"verdict\":\"not_executed\"}\n").expect("invalidate");
    let lsp = upload(&ctx, FieldId::LargeScalePrecipitation, &snapshots);
    let cp = upload(&ctx, FieldId::ConvectivePrecipitation, &snapshots);
    let mut producer = prepare_precipitation_members(
        &ctx,
        &lsp,
        &cp,
        PrecipitationMemberPreparation::flex_extract_rrint0(),
    )
    .expect("#207");
    let grid = producer.metadata().horizontal_grid.clone();
    let member_kernels = PrecipitationMemberKernels::new(&ctx).expect("#207 kernels");
    let resident_kernels = ResidentQueryKernels::new(&ctx).expect("#171 kernels");
    let kernels = ResidentPrecipitationKernels::new(&ctx).expect("#227 kernels");
    let mut batches: Vec<_> = steps
        .iter()
        .map(|_| ResidentQueryBatch::new(&ctx, &grid, CAPACITY, ACTIVE).expect("lanes"))
        .collect();
    let particles: Vec<_> = steps
        .iter()
        .map(|s| lane_particles(&ctx, &s.rows.iter().map(|r| (r.x, r.y)).collect::<Vec<_>>()))
        .collect();
    let mut large_batch =
        ResidentQueryBatch::new(&ctx, &grid, LARGE_CAPACITY, LARGE_CAPACITY).expect("large");
    let large_particles = ParticleBuffers::from_particles(
        &ctx,
        &vec![particle_at(1.25, 0.5); LARGE_CAPACITY as usize],
    );

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("genuine_227_single_encoder"),
        });
    ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    // #207 is reconstructed once; every step shares it read-only.
    producer
        .encode(&ctx, &member_kernels, &mut encoder)
        .expect("#90/#207");
    let mut plans: Vec<_> = steps
        .iter()
        .zip(batches.iter_mut())
        .map(|(step, batch)| {
            prepare_resident_precipitation_rates(&ctx, &producer, batch, step.itime, 1)
                .expect("resident step")
        })
        .collect();
    let large_step = 1;
    let mut large_plan = prepare_resident_precipitation_rates(
        &ctx,
        &producer,
        &mut large_batch,
        steps[large_step].itime,
        1,
    )
    .expect("large step");
    let mut stage_lists = Vec::new();
    for (plan, particles) in plans.iter_mut().zip(&particles) {
        let handoff = plan
            .encode_from_particles(&ctx, particles, &resident_kernels, &kernels, &mut encoder)
            .expect("resident encode");
        stage_lists.push(handoff.stages.to_vec());
    }
    let large_stages = large_plan
        .encode_from_particles(
            &ctx,
            &large_particles,
            &resident_kernels,
            &kernels,
            &mut encoder,
        )
        .expect("large encode")
        .stages
        .to_vec();
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    assert!(pollster::block_on(ctx.device.pop_error_scope()).is_none());
    let readbacks: Vec<_> = plans
        .iter()
        .map(|plan| pollster::block_on(plan.download(&ctx)).expect("fatal-checked step"))
        .collect();
    let step_metadata: Vec<_> = plans.iter().map(|p| p.metadata().clone()).collect();
    let large = pollster::block_on(large_plan.download(&ctx)).expect("large step");
    let large_metadata = large_plan.metadata().clone();
    drop(plans);
    drop(large_plan);

    // Scalability: planes once per step, O(capacity) remainder, constant work.
    let cell_count = grid.nx * grid.ny;
    let small_audit = audit(&step_metadata[0], &stage_lists[0]);
    let large_audit = audit(&large_metadata, &large_stages);
    for (metadata, value) in [
        (&step_metadata[0], &small_audit),
        (&large_metadata, &large_audit),
    ] {
        assert_eq!(metadata.plane_bytes, 4 * cell_count as u64 * 4);
        assert!(value["device_bytes"].as_u64() <= value["bound_bytes"].as_u64());
    }
    assert_eq!(
        large_metadata.device_bytes - step_metadata[0].device_bytes,
        PER_LANE_BYTES * u64::from(LARGE_CAPACITY - CAPACITY)
    );
    assert!(stage_lists.iter().all(|s| *s == large_stages));
    assert_eq!(small_audit["dispatches"], 12);
    assert_eq!(small_audit["copies"], 4);
    assert!(large.lane_status.iter().all(|s| *s == 2));
    let reference_lane = POSITION_LANES[steps[large_step]
        .rows
        .iter()
        .position(|r| (r.x, r.y) == (1.25, 0.5))
        .expect("reference position")];
    let reference =
        &readbacks[large_step].rates_mm_per_h[2 * reference_lane..2 * reference_lane + 2];
    assert!(large
        .rates_mm_per_h
        .chunks(2)
        .all(|pair| bits(pair) == bits(reference)));

    // Validation-only member and batch-API evidence after the resident output.
    let members = pollster::block_on(producer.download(&ctx)).expect("validation members");
    let mut member_values = serde_json::Map::new();
    let mut member_bits = serde_json::Map::new();
    for (parameter, actual) in [
        ("lsp", members.large_scale_mm_per_h().expect("lsp")),
        ("cp", members.convective_mm_per_h().expect("cp")),
    ] {
        member_values.insert(
            parameter.to_owned(),
            json!(actual
                .chunks(cell_count)
                .map(|p| p.iter().map(|v| f64::from(*v)).collect::<Vec<_>>())
                .collect::<Vec<_>>()),
        );
        member_bits.insert(
            parameter.to_owned(),
            json!(actual.chunks(cell_count).map(bits).collect::<Vec<_>>()),
        );
    }
    let batch = batch_api_rates(&ctx, &mut producer, &steps);
    let member_times: Vec<_> = producer
        .metadata()
        .members
        .iter()
        .map(|m| m.valid_time_epoch_seconds)
        .collect();

    let mut rows = Vec::new();
    let mut candidate = Vec::new();
    let mut expected = Vec::new();
    let mut classes = serde_json::Map::new();
    let mut step_records = Vec::new();
    for (step_index, ((step, readback), metadata)) in
        steps.iter().zip(&readbacks).zip(&step_metadata).enumerate()
    {
        for lane in INACTIVE_LANES.iter().chain(&TAIL_LANES) {
            assert_eq!(readback.lane_status[*lane], 0, "explicit inactive lane");
            let pair = &readback.rates_mm_per_h[2 * lane..2 * lane + 2];
            assert!(pair.iter().all(|v| v.to_bits() == QUIET_NAN_BITS));
        }
        step_records.push(json!({"itime_epoch_seconds": step.itime,
            "memory_pair_slots": [metadata.memory_pair.left.slot, metadata.memory_pair.right.slot],
            "time_differences_seconds": metadata.time_differences_seconds,
            "capacity": metadata.capacity, "active_count": metadata.active_count,
            "fatal_status": 0, "lane_status": readback.lane_status,
            "inactive_or_tail_bits": INACTIVE_LANES.iter().chain(&TAIL_LANES)
                .map(|l| bits(&readback.rates_mm_per_h[2 * l..2 * l + 2])).collect::<Vec<_>>()}));
        for (position, (row, lane)) in step.rows.iter().zip(POSITION_LANES).enumerate() {
            let index = rows.len();
            let actual = &readback.rates_mm_per_h[2 * lane..2 * lane + 2];
            let reference = &batch[2 * index..2 * index + 2];
            let class = time_class(step.itime, &member_times);
            let identical = bits(actual) == bits(reference);
            let entry = classes
                .entry(class)
                .or_insert_with(|| json!({"rows": 0, "bitwise_equal": true}));
            entry["rows"] = json!(entry["rows"].as_u64().expect("count") + 1);
            entry["bitwise_equal"] = json!(entry["bitwise_equal"] == true && identical);
            let original_pair = [row.lsp, row.cp];
            let passed = actual
                .iter()
                .zip(original_pair)
                .all(|(c, o)| member_matches_oracle(f64::from(*c), o));
            candidate.extend(actual.iter().map(|v| f64::from(*v)));
            expected.extend(original_pair);
            rows.push(json!({"index": index, "case": row.case, "step": step_index, "position": position,
                "lane": lane, "query": [row.x, row.y, step.itime], "time_class": class,
                "candidate": actual.iter().map(|v| f64::from(*v)).collect::<Vec<_>>(),
                "candidate_bits": bits(actual), "batch_bits": bits(reference), "bitwise_equal": identical,
                "original": original_pair,
                "absolute_differences": [(f64::from(actual[0]) - row.lsp).abs(), (f64::from(actual[1]) - row.cp).abs()],
                "verdict": if passed {"passed"} else {"failed"}}));
        }
    }
    assert_eq!(rows.len(), 588);
    assert_eq!(
        classes.len(),
        3,
        "interior, exact-member and final timestamps"
    );
    let bitwise = classes.values().all(|c| c["bitwise_equal"] == true);
    let passed = bitwise && rows.iter().all(|r| r["verdict"] == "passed");
    let input_manifest = serde_json::to_string(&json!({
        "members": producer.metadata(),
        "steps": step_metadata.iter().map(|m| json!({"itime_epoch_seconds": m.itime_epoch_seconds,
            "memory_pair": m.memory_pair, "time_differences_seconds": m.time_differences_seconds})).collect::<Vec<_>>(),
        "lanes": {"capacity": CAPACITY, "active_count": ACTIVE, "position_lanes": POSITION_LANES},
    }))
    .expect("input manifest");
    let evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: "genuine-precipitation-resident-227".to_owned(),
        candidate: GpuCandidateEvidence {
            implementation_id: "precipitation-resident-gpu-227".to_owned(),
            revision: candidate_revision(),
            shader_sha256: precipitation_resident_rates_shader_sha256(),
            input_sha256: grib202::sha256_hex(input_manifest.as_bytes()),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(adapter.clone()),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(PinnedOracleEvidence {
            implementation_id: "FLEXPART-11.1 interpol_rain numpf=1".to_owned(),
            revision: original["flexpart_revision"]
                .as_str()
                .expect("pin")
                .to_owned(),
            executable_sha256: original["executable_sha256"]
                .as_str()
                .expect("executable")
                .to_owned(),
            output_sha256: grib202::sha256_hex(ORIGINAL.as_bytes()),
        }),
        comparison: compare_finite_values(
            &expected,
            &candidate,
            ComparisonPolicy::new(1e-6, 1e-5).expect("#71 policy"),
        )
        .expect("finite comparison"),
    };
    evidence.require_paired_pass().expect("#91 paired evidence");
    let shader = |source: &[u8]| grib202::sha256_hex(source);
    let report = json!({"schema": "flexpart-gpu.precipitation-resident-device.v1",
        "candidate": {"revision": candidate_revision(), "worktree_clean": git(&["status", "--porcelain"]).is_empty()},
        "adapter": adapter, "status": members.status(), "metadata": producer.metadata(),
        "fixture_sha256": grib202::sha256_hex(ORIGINAL.as_bytes()),
        "darain_oracle_sha256": grib202::ORACLE_SHA256, "decision_216_sha256": grib202::DECISION_SHA256,
        "grib_files_sha256": grib202::grib_file_hashes(&grib202::decision()),
        "shaders": {"accumulated_interval": accumulated_shader_sha256(), "darain": darain_shader_sha256(),
            "particle_query": shader(include_bytes!("../src/shaders/particle_query.wgsl")),
            "resident_status_reset": shader(include_bytes!("../src/shaders/resident_status_reset.wgsl")),
            "resident_query_adapter": shader(include_bytes!("../src/shaders/resident_query_adapter.wgsl")),
            "horizontal": horizontal_shader_sha256(),
            "precipitation_resident_rates": precipitation_resident_rates_shader_sha256(),
            "precipitation_resident_publish": precipitation_resident_publish_shader_sha256(),
            "precipitation_temporal": precipitation_temporal_shader_sha256()},
        "source_sha256": {"resident": grib202::sha256_hex(include_bytes!("../src/gpu/precipitation_resident.rs")),
            "test": grib202::sha256_hex(include_bytes!("precipitation_resident_gpu.rs")),
            "lanes": grib202::sha256_hex(include_bytes!("../src/gpu/meteorology/resident.rs")),
            "temporal": grib202::sha256_hex(include_bytes!("../src/gpu/precipitation_temporal.rs")),
            "member": grib202::sha256_hex(include_bytes!("../src/gpu/precipitation_members.rs")),
            "spatial": grib202::sha256_hex(include_bytes!("../src/gpu/horizontal.rs"))},
        "encoders": 1, "submissions": 1, "host_transfers_between_stages": 0, "host_coordinate_uploads": 0,
        "member_reconstructions": 1, "steps_per_submission": steps.len() + 1,
        "lanes": {"capacity": CAPACITY, "active_count": ACTIVE, "position_lanes": POSITION_LANES,
            "inactive_lanes": INACTIVE_LANES, "tail_lanes": TAIL_LANES},
        "allocation_audit": {"cell_count": cell_count, "per_lane_bytes": PER_LANE_BYTES,
            "control_bytes_bound": CONTROL_BYTES, "per_lane_bind_groups": 0,
            "batch_api_plane_bytes_for_same_queries": rows.len() as u64 * 4 * cell_count as u64 * 4,
            "small": small_audit, "large": large_audit},
        "bitwise_equal": classes, "steps": step_records,
        "comparison_policy": {"absolute": 1e-6, "relative": 1e-5}, "unit": "millimeter_per_hour",
        "gpu_evidence": evidence, "input_manifest": input_manifest,
        "member_values_mm_per_h": member_values, "member_bits": member_bits, "rows": rows,
        "verdict": if passed {"passed"} else {"failed"}});
    std::fs::write(&path, serde_json::to_string_pretty(&report).expect("JSON")).expect("evidence");
    assert!(
        bitwise,
        "resident lanes must equal the #119 batch API bitwise; {}",
        path.display()
    );
    assert!(
        passed,
        "resident lanes must match the unchanged pinned original; {}",
        path.display()
    );
}

/// Run one encoded step, returning the raw published rates and the final result.
fn execute_step(
    ctx: &GpuContext,
    producer: &PreparedPrecipitationMembers<'_, '_>,
    grid: &flexpart_gpu::meteorology::HorizontalGrid,
    itime: i64,
    particles: &[Particle],
) -> (Vec<f32>, Vec<u32>, Result<(), PrecipitationTemporalError>) {
    let count = u32::try_from(particles.len()).expect("lanes");
    let resident = ResidentQueryKernels::new(ctx).expect("#171 kernels");
    let kernels = ResidentPrecipitationKernels::new(ctx).expect("#227 kernels");
    let mut batch = ResidentQueryBatch::new(ctx, grid, count, count).expect("lanes");
    let buffers = ParticleBuffers::from_particles(ctx, particles);
    let mut plan =
        prepare_resident_precipitation_rates(ctx, producer, &mut batch, itime, 1).expect("step");
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let handoff = plan
        .encode_from_particles(ctx, &buffers, &resident, &kernels, &mut encoder)
        .expect("encode");
    ctx.queue.submit(Some(encoder.finish()));
    let raw = pollster::block_on(download_buffer_typed::<f32>(
        ctx,
        handoff.rates_mm_per_h,
        particles.len() * 2,
        "raw published rates",
    ))
    .expect("raw");
    let result = pollster::block_on(plan.download(ctx)).map(|_| ());
    drop(plan);
    let status = pollster::block_on(batch.observe_status(ctx)).expect("status");
    let mut words = vec![u32::from(status.fatal)];
    words.extend(status.lanes);
    (raw, words, result)
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_precipitation_resident_fail_closed_matrix() {
    let snapshots = grib202::snapshots(&grib202::observations());
    let ctx = pollster::block_on(GpuContext::new()).expect("required GPU");
    let other = pollster::block_on(GpuContext::new()).expect("independent context");
    let lsp = upload(&ctx, FieldId::LargeScalePrecipitation, &snapshots);
    let cp = upload(&ctx, FieldId::ConvectivePrecipitation, &snapshots);
    for preparation in [
        PrecipitationMemberPreparation {
            numpf: 3,
            ..PrecipitationMemberPreparation::flex_extract_rrint0()
        },
        PrecipitationMemberPreparation {
            rrint: 1,
            ..PrecipitationMemberPreparation::flex_extract_rrint0()
        },
    ] {
        assert!(
            prepare_precipitation_members(&ctx, &lsp, &cp, preparation).is_err(),
            "numpf=3/RRINT=1 never reach resident sampling"
        );
    }
    let mut producer = prepare_precipitation_members(
        &ctx,
        &lsp,
        &cp,
        PrecipitationMemberPreparation::flex_extract_rrint0(),
    )
    .expect("members");
    let grid = producer.metadata().horizontal_grid.clone();
    let first = producer.metadata().members[0].valid_time_epoch_seconds;
    let last = producer
        .metadata()
        .members
        .last()
        .expect("last")
        .valid_time_epoch_seconds;
    let mut batch = ResidentQueryBatch::new(&ctx, &grid, 2, 2).expect("lanes");
    assert!(
        prepare_resident_precipitation_rates(&ctx, &producer, &mut batch, first, 1).is_err(),
        "unencoded #207 owner"
    );
    let member_kernels = PrecipitationMemberKernels::new(&ctx).expect("#207 kernels");
    let valid = [particle_at(0.5, 0.5), particle_at(1.25, 0.5)];
    // An encoded but never executed #207 owner holds zero-filled members; its
    // allocation-time status must reject every lane instead of dry weather.
    let mut discarded = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    producer
        .encode(&ctx, &member_kernels, &mut discarded)
        .expect("#207");
    drop(discarded);
    let (raw, _, result) = execute_step(&ctx, &producer, &grid, first + 2700, &valid);
    let Err(PrecipitationTemporalError::DeviceStatus(word)) = result else {
        panic!("unexecuted #207 work must reject the step: {result:?}");
    };
    assert_ne!(
        (word >> RESIDENT_PRECIPITATION_PRODUCER_SHIFT) & STATUS_NOT_EXECUTED,
        0
    );
    assert!(raw.iter().all(|v| v.to_bits() == QUIET_NAN_BITS));
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    producer
        .encode(&ctx, &member_kernels, &mut encoder)
        .expect("#207");
    ctx.queue.submit(Some(encoder.finish()));
    for direction in [-1, 0, 2] {
        assert!(matches!(
            prepare_resident_precipitation_rates(&ctx, &producer, &mut batch, first, direction),
            Err(PrecipitationTemporalError::BackwardTime)
        ));
    }
    for t in [first - 1, last + 1] {
        assert!(
            prepare_resident_precipitation_rates(&ctx, &producer, &mut batch, t, 1).is_err(),
            "unsupported timestamp"
        );
    }
    assert!(prepare_resident_precipitation_rates(&other, &producer, &mut batch, first, 1).is_err());
    let mut foreign_batch = ResidentQueryBatch::new(&other, &grid, 2, 2).expect("foreign lanes");
    assert!(
        prepare_resident_precipitation_rates(&ctx, &producer, &mut foreign_batch, first, 1)
            .is_err()
    );
    let mut wider = grid.clone();
    wider.nx += 1;
    let mut wider_batch = ResidentQueryBatch::new(&ctx, &wider, 2, 2).expect("other grid");
    assert!(
        prepare_resident_precipitation_rates(&ctx, &producer, &mut wider_batch, first, 1).is_err(),
        "grid/source mismatch"
    );
    let buffers = ParticleBuffers::from_particles(&ctx, &valid);
    let resident = ResidentQueryKernels::new(&ctx).expect("#171");
    let kernels = ResidentPrecipitationKernels::new(&ctx).expect("#227");
    let foreign_kernels = ResidentPrecipitationKernels::new(&other).expect("foreign #227");
    let foreign_resident = ResidentQueryKernels::new(&other).expect("foreign #171");
    {
        let mut plan = prepare_resident_precipitation_rates(&ctx, &producer, &mut batch, first, 1)
            .expect("plan");
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        assert!(plan
            .encode_from_particles(&ctx, &buffers, &resident, &foreign_kernels, &mut encoder)
            .is_err());
        assert!(plan
            .encode_from_particles(&ctx, &buffers, &foreign_resident, &kernels, &mut encoder)
            .is_err());
        plan.encode_from_particles(&ctx, &buffers, &resident, &kernels, &mut encoder)
            .expect("encode");
        assert!(
            plan.encode_from_particles(&ctx, &buffers, &resident, &kernels, &mut encoder)
                .is_err(),
            "one-shot step prevents stale reuse"
        );
        drop(encoder);
        assert!(
            pollster::block_on(plan.download(&ctx)).is_err(),
            "unsubmitted step must never publish precipitation"
        );
    }
    let (raw, status, result) = execute_step(&ctx, &producer, &grid, first + 2700, &valid);
    result.expect("valid control");
    assert_eq!(status, [0, 2, 2]);
    assert!(raw.iter().all(|v| v.is_finite()));

    // #87 domain, non-finite, negative-cell and fraction rejections on-device.
    let mut domain = particle_at(2.0, 0.0);
    domain.pos_x = 0.5;
    let mut nonfinite = particle_at(0.0, 0.0);
    nonfinite.pos_y = f32::NAN;
    let mut negative = particle_at(0.0, 0.0);
    negative.cell_x = -1;
    let mut fraction = particle_at(0.0, 0.0);
    fraction.pos_x = 1.0;
    for (bad, reason) in [(domain, 6), (nonfinite, 5), (negative, 3), (fraction, 4)] {
        let (raw, status, result) =
            execute_step(&ctx, &producer, &grid, first + 2700, &[valid[0], bad]);
        assert!(matches!(
            result,
            Err(PrecipitationTemporalError::DeviceStatus(1))
        ));
        assert_eq!(status, [1, 2, reason]);
        assert!(
            raw.iter().all(|v| v.to_bits() == QUIET_NAN_BITS),
            "no valid prefix and no zero rate after a lane failure"
        );
    }
    drop(producer);

    // A #207 producer failure invalidates every lane of every consuming step.
    let mut overflow = snapshots;
    for snapshot in &mut overflow {
        for field in &mut snapshot.fields {
            if [
                FieldId::LargeScalePrecipitation,
                FieldId::ConvectivePrecipitation,
            ]
            .contains(&field.id)
            {
                field.values.fill(f32::MAX);
                field.source_quantization = None;
                let start = field.time.valid_time_epoch_seconds - 10_800;
                field.time.interval_start_epoch_seconds = Some(start);
                field
                    .time
                    .accumulation
                    .as_mut()
                    .expect("reset")
                    .reset_epoch_seconds = start;
            }
        }
    }
    let lsp = upload(&ctx, FieldId::LargeScalePrecipitation, &overflow);
    let cp = upload(&ctx, FieldId::ConvectivePrecipitation, &overflow);
    let mut failing = prepare_precipitation_members(
        &ctx,
        &lsp,
        &cp,
        PrecipitationMemberPreparation::flex_extract_rrint0(),
    )
    .expect("finite source");
    let t = failing.metadata().members[0].valid_time_epoch_seconds;
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    failing
        .encode(&ctx, &member_kernels, &mut encoder)
        .expect("#207");
    ctx.queue.submit(Some(encoder.finish()));
    let (raw, status, result) = execute_step(&ctx, &failing, &grid, t, &valid);
    let Err(PrecipitationTemporalError::DeviceStatus(word)) = result else {
        panic!("#207 overflow must reject the step: {result:?}");
    };
    assert_ne!(word >> RESIDENT_PRECIPITATION_PRODUCER_SHIFT, 0);
    assert_eq!(status[0], 1, "shared fatal word is set");
    assert!(
        raw.iter().all(|v| v.to_bits() == QUIET_NAN_BITS),
        "no zero rate substituted for a producer failure"
    );
}
