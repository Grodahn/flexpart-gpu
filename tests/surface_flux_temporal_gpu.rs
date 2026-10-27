//! #120 device proof for the approved instantaneous surface-flux field set.
//!
//! Compares the existing #89 blend and the existing #76 #87 -> #89 composition
//! against direct pinned FLEXPART 11.1 routines evaluated on the fields' memory
//! slots (`fixtures/interpolation/surface-flux-temporal-v1`). Real device
//! execution is mandatory; absent adapters never pass by skipping.

use flexpart_gpu::gpu::evidence::GpuEvidenceSchema;
use flexpart_gpu::gpu::{
    compare_finite_values, download_buffer_typed,
    horizontal::horizontal_shader_sha256,
    meteorology::{
        CanonicalGpuField, MeteorologyCompositionKernels, MeteorologySampleRequest,
        MeteorologyStage, MeteorologyTimeSelection,
    },
    temporal::{sample_field_gpu, TemporalInterpolationKernel},
    ComparisonPolicy, GpuAdapterEvidence, GpuCalculationEvidence, GpuCalculationPath,
    GpuCandidateEvidence, GpuContext, GpuExecutionEvidence, GpuExecutionStatus,
    PinnedOracleEvidence,
};
use flexpart_gpu::meteorology::temporal::{
    resolve_temporal_bracket, InstantaneousSamplingBasis, RequestedSampleTime, TemporalApplication,
    TemporalError, DEFAULT_ABS_TOL, DEFAULT_REL_TOL, INSTANTANEOUS_SURFACE_FLUX_FIELDS,
};
use flexpart_gpu::meteorology::{
    Calendar, Field, FieldId, SignConvention, Snapshot, TemporalKind, Unit,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const TEMPORAL_SHADER: &str = include_str!("../src/shaders/temporal_interpolation.wgsl");
const EVIDENCE_DIRECTORY: &str = "target/ci-gate/surface-flux-temporal-gpu";

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn oracle() -> Value {
    let path = std::env::var("FLEXPART_GPU_SURFACE_FLUX_ORACLE")
        .unwrap_or_else(|_| "fixtures/interpolation/surface-flux-temporal-v1/report.json".into());
    serde_json::from_slice(&std::fs::read(path).expect("oracle report is required")).unwrap()
}

fn revision() -> String {
    std::env::var("FLEXPART_GPU_CANDIDATE_REVISION").unwrap_or_else(|_| {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    })
}

fn field_id(name: &str) -> FieldId {
    serde_json::from_value(json!(name)).unwrap()
}

/// Canonical 4x3 surface snapshots carrying every approved flux field.
fn snapshots(report: &Value) -> Vec<Snapshot> {
    let base: Snapshot = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-column-v1.json"
    ))
    .unwrap();
    let grid = &report["grid"];
    let (nx, ny) = (
        grid["nx"].as_u64().unwrap() as usize,
        grid["ny"].as_u64().unwrap() as usize,
    );
    let template = base
        .fields
        .iter()
        .find(|f| f.id == FieldId::SurfacePressure)
        .unwrap()
        .clone();
    report["snapshot_times_s"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, time)| {
            let mut snapshot = base.clone();
            snapshot.horizontal_grid.nx = nx;
            snapshot.horizontal_grid.ny = ny;
            // Retain the hybrid-coordinate dependencies, repeated over the grid.
            for field in &mut snapshot.fields {
                field.shape[0] = nx;
                field.shape[1] = ny;
                field.values = field
                    .values
                    .iter()
                    .flat_map(|v| vec![*v; nx * ny])
                    .collect();
                if field.time.kind != TemporalKind::Static {
                    field.time.valid_time_epoch_seconds = time.as_i64().unwrap();
                }
            }
            for spec in report["fields"].as_array().unwrap() {
                let mut field: Field = template.clone();
                field.id = field_id(spec["canonical_field"].as_str().unwrap());
                field.unit = serde_json::from_value(spec["unit"].clone()).unwrap();
                field.sign = serde_json::from_value(spec["canonical_sign"].clone()).unwrap();
                field.shape = vec![nx, ny];
                field.time.valid_time_epoch_seconds = time.as_i64().unwrap();
                field.values = spec["canonical_values_per_snapshot"][index]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap() as f32)
                    .collect();
                snapshot.fields.push(field);
            }
            snapshot
        })
        .collect()
}

fn instant(seconds: i64) -> MeteorologyTimeSelection {
    MeteorologyTimeSelection::Instantaneous(RequestedSampleTime::new(Calendar::Gregorian, seconds))
}

fn surface_request(xt: f64, yt: f64, time: MeteorologyTimeSelection) -> MeteorologySampleRequest {
    MeteorologySampleRequest {
        xt,
        yt,
        time,
        height: None,
    }
}

fn application(name: &str) -> TemporalApplication {
    serde_json::from_value(json!(name)).unwrap()
}

/// One field's #89-only and #76-composed values with their oracle targets.
#[derive(Default)]
struct FieldComparison {
    oracle: Vec<f64>,
    candidate: Vec<f64>,
    rows: Vec<Value>,
}

impl FieldComparison {
    fn push(&mut self, path: &str, record: &Value, actual: f32) {
        let expected = record["canonical_value"].as_f64().unwrap();
        let actual = f64::from(actual);
        self.oracle.push(expected);
        self.candidate.push(actual);
        self.rows.push(json!({
            "path": path, "itime_s": record["itime_s"], "xt": record["xt"], "yt": record["yt"],
            "application": record["application"], "oracle_canonical_value": expected,
            "flexpart_value": record["flexpart_value"], "gpu_value": actual,
            "absolute_difference": (actual - expected).abs(),
        }));
    }
}

/// Isolated #89 path: whole-field device blend compared at every grid node.
fn compare_isolated(
    ctx: &GpuContext,
    kernel: &TemporalInterpolationKernel,
    id: FieldId,
    series: &[&Snapshot],
    records: &[&Value],
    comparison: &mut FieldComparison,
) {
    let nx = series[0].horizontal_grid.nx;
    let mut times: Vec<i64> = records
        .iter()
        .filter(|r| r["node"] == true)
        .map(|r| r["itime_s"].as_i64().unwrap())
        .collect();
    times.dedup();
    for time in times {
        let request = RequestedSampleTime::new(Calendar::Gregorian, time);
        let sample = pollster::block_on(sample_field_gpu(ctx, id, series, request, kernel))
            .expect("approved surface flux must execute on the device");
        assert_eq!(
            sample.bracket.sampling_basis,
            InstantaneousSamplingBasis::PointInTimeSurfaceFlux
        );
        for record in records
            .iter()
            .filter(|r| r["node"] == true && r["itime_s"].as_i64() == Some(time))
        {
            assert_eq!(
                sample.application,
                application(record["application"].as_str().unwrap())
            );
            let x = record["xt"].as_f64().unwrap() as usize;
            let y = record["yt"].as_f64().unwrap() as usize;
            let actual = sample.values[x + y * nx];
            if sample.application != TemporalApplication::LinearInterior {
                // #74/#89 endpoints and exact source timestamps return the source unchanged.
                let source = series
                    .iter()
                    .filter_map(|s| s.fields.iter().find(|f| f.id == id))
                    .find(|f| f.time.valid_time_epoch_seconds == time)
                    .unwrap();
                assert_eq!(actual.to_bits(), source.values[x + y * nx].to_bits());
            }
            comparison.push("isolated_89", record, actual);
        }
    }
}

#[test]
fn test_surface_flux_temporal_gpu_matches_direct_oracle_for_each_approved_field() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(EVIDENCE_DIRECTORY);
    let report_path = directory.join("report.json");
    if report_path.exists() {
        std::fs::remove_file(&report_path).unwrap();
    }
    let ctx = pollster::block_on(GpuContext::new()).expect("#120 device execution cannot skip");
    let kernel = TemporalInterpolationKernel::new(&ctx).unwrap();
    let kernels = MeteorologyCompositionKernels::new(&ctx).unwrap();
    let report = oracle();
    let provenance = &report["provenance"];
    let pin: Value = serde_json::from_str(include_str!("../reference/flexpart-11.1.json")).unwrap();
    assert_eq!(provenance["pinned_commit"], pin["pinned_commit"]);
    assert_eq!(provenance["checkout_clean"], true);
    let snapshots = snapshots(&report);
    let series: Vec<&Snapshot> = snapshots.iter().collect();
    let policy = ComparisonPolicy::new(DEFAULT_ABS_TOL, DEFAULT_REL_TOL).unwrap();
    let shader_sha256 = sha256_hex(
        format!(
            "{}{}",
            horizontal_shader_sha256(),
            sha256_hex(TEMPORAL_SHADER.as_bytes())
        )
        .as_bytes(),
    );
    let mut verdicts = serde_json::Map::new();
    let mut field_reports = Vec::new();
    let mut failures = Vec::new();

    for spec in report["fields"].as_array().unwrap() {
        let name = spec["canonical_field"].as_str().unwrap();
        let id = field_id(name);
        assert!(INSTANTANEOUS_SURFACE_FLUX_FIELDS.contains(&id));
        let records: Vec<&Value> = report["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["canonical_field"] == name)
            .collect();
        let mut comparison = FieldComparison::default();
        compare_isolated(&ctx, &kernel, id, &series, &records, &mut comparison);

        // #76: canonical source -> #87 per bracket member -> #89, one caller encoder.
        let source = CanonicalGpuField::upload(&ctx, id, &series, &[None, None, None]).unwrap();
        let composed: Vec<&Value> = records
            .iter()
            .copied()
            .filter(|r| r["node"] == false)
            .collect();
        let mut plans: Vec<_> = composed
            .iter()
            .map(|r| {
                let request = surface_request(
                    r["xt"].as_f64().unwrap(),
                    r["yt"].as_f64().unwrap(),
                    instant(r["itime_s"].as_i64().unwrap()),
                );
                source.prepare_sample(&ctx, request).unwrap()
            })
            .collect();
        ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("#120 caller encoder"),
            });
        let mut consumers = Vec::new();
        let mut handoffs = Vec::new();
        for (plan, record) in plans.iter_mut().zip(&composed) {
            let handoff = plan.encode(&ctx, &kernels, &mut encoder).unwrap();
            let stages: Vec<_> = handoff.stages.iter().map(|s| s.stage).collect();
            assert_eq!(
                stages,
                [
                    MeteorologyStage::Horizontal,
                    MeteorologyStage::Horizontal,
                    MeteorologyStage::Temporal
                ]
            );
            let metadata = handoff.metadata;
            assert_eq!(metadata.field_id, id);
            assert_eq!(json!(metadata.unit), spec["unit"]);
            assert_eq!(json!(metadata.sign), spec["canonical_sign"]);
            assert_eq!(metadata.value_count, 1);
            let bracket = metadata.temporal_bracket.as_ref().unwrap();
            assert_eq!(
                bracket.sampling_basis,
                InstantaneousSamplingBasis::PointInTimeSurfaceFlux
            );
            assert_eq!(
                bracket.application,
                application(record["application"].as_str().unwrap())
            );
            assert!(metadata
                .source_times
                .iter()
                .all(|t| t.kind == TemporalKind::Instantaneous
                    && t.interval_start_epoch_seconds.is_none()
                    && t.accumulation.is_none()));
            assert_eq!(
                metadata.source_times[0].valid_time_epoch_seconds,
                bracket.source_timestamps.lower_epoch_seconds
            );
            assert_eq!(
                metadata.source_times[1].valid_time_epoch_seconds,
                bracket.source_timestamps.upper_epoch_seconds
            );
            for (hash, index) in metadata
                .source_snapshot_sha256
                .iter()
                .zip([bracket.lower_index, bracket.upper_index])
            {
                assert_eq!(
                    *hash,
                    sha256_hex(&serde_json::to_vec(series[index]).unwrap())
                );
            }
            assert!(!handoff
                .values
                .usage()
                .contains(wgpu::BufferUsages::MAP_READ));
            // Poisoned downstream consumer copied inside the same caller encoder.
            let consumer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("#120 fixture consumer"),
                size: 4,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            ctx.queue
                .write_buffer(&consumer, 0, bytemuck::cast_slice(&[f32::NAN]));
            encoder.copy_buffer_to_buffer(handoff.values, 0, &consumer, 0, 4);
            handoffs.push(json!({
                "metadata": metadata, "device_copies": handoff.device_copies,
                "stages": handoff.stages,
            }));
            consumers.push(consumer);
        }
        // One submission for all producers and consumers; no intermediate host step.
        ctx.queue.submit(Some(encoder.finish()));
        ctx.device.poll(wgpu::Maintain::Wait);
        let errors: Vec<_> = (0..3)
            .filter_map(|_| pollster::block_on(ctx.device.pop_error_scope()))
            .collect();
        assert!(errors.is_empty(), "scoped device errors: {errors:?}");
        for (consumer, record) in consumers.iter().zip(&composed) {
            let actual: Vec<f32> = pollster::block_on(download_buffer_typed(
                &ctx,
                consumer,
                1,
                "#120 final validation consumer",
            ))
            .unwrap();
            comparison.push("composed_76", record, actual[0]);
        }

        let input_bytes = serde_json::to_vec(&snapshots).unwrap();
        let evidence = GpuCalculationEvidence {
            schema: GpuEvidenceSchema::default(),
            case_id: format!("SURFACE-FLUX-TEMPORAL-120-{name}"),
            candidate: GpuCandidateEvidence {
                implementation_id: "#89 encode_temporal_blend and #76 #87->#89 composition".into(),
                revision: revision(),
                shader_sha256: shader_sha256.clone(),
                input_sha256: sha256_hex(&input_bytes),
            },
            execution: GpuExecutionEvidence {
                status: GpuExecutionStatus::Passed,
                calculation_path: GpuCalculationPath::WgslDevice,
                adapter: Some(GpuAdapterEvidence::from_context(&ctx)),
                failure: None,
                skip_reason: None,
            },
            oracle: Some(PinnedOracleEvidence {
                implementation_id:
                    "FLEXPART-11.1 hor_interpol + temporal_interpolation on memory slots".into(),
                revision: provenance["pinned_commit"].as_str().unwrap().into(),
                executable_sha256: provenance["artifacts_sha256"]
                    ["build/surface-flux-temporal-oracle"]
                    .as_str()
                    .unwrap()
                    .into(),
                output_sha256: provenance["artifacts_sha256"]["output.txt"]
                    .as_str()
                    .unwrap()
                    .into(),
            }),
            comparison: compare_finite_values(&comparison.oracle, &comparison.candidate, policy)
                .unwrap(),
        };
        evidence.validate().unwrap();
        let verdict = if evidence.require_paired_pass().is_ok() {
            "PASS"
        } else {
            failures.push(name.to_string());
            "FAIL"
        };
        verdicts.insert(
            name.into(),
            json!({
                "instantaneous": verdict,
                "interval_mean_through_instantaneous_path": "UNSUPPORTED",
                "accumulated": "UNSUPPORTED",
            }),
        );
        field_reports.push(json!({
            "canonical_field": name, "verdict": verdict,
            "compared_values": comparison.oracle.len(), "gpu_evidence": evidence,
            "rows": comparison.rows, "handoffs": handoffs,
        }));
    }

    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("inputs.json"),
        serde_json::to_vec(&snapshots).unwrap(),
    )
    .unwrap();
    let record = json!({
        "schema": {"id": "flexpart-gpu.surface-flux-temporal-gpu-evidence", "version": 1},
        "issue": 120,
        "candidate_revision": revision(),
        "adapter": GpuAdapterEvidence::from_context(&ctx),
        "software_adapter": ctx.is_software_adapter(),
        "validation_level": "paired direct pinned FLEXPART 11.1 routines on memory slots; not consumer parity",
        "comparison_policy": {"owner": "#74/#89", "absolute": DEFAULT_ABS_TOL, "relative": DEFAULT_REL_TOL},
        "shader_sha256": {"horizontal": horizontal_shader_sha256(),
                          "temporal": sha256_hex(TEMPORAL_SHADER.as_bytes()),
                          "combined": shader_sha256},
        "input_sha256": sha256_hex(&serde_json::to_vec(&snapshots).unwrap()),
        "oracle_provenance": provenance,
        // Exact serde preimages let the audit recompute snapshot identities.
        "serialized_sources": snapshots.iter().map(|s| serde_json::to_string(s).unwrap()).collect::<Vec<_>>(),
        "verdicts": verdicts,
        "fields": field_reports,
        "one_encoder_one_submission_per_field": true,
        "intermediate_host_readbacks": 0,
    });
    std::fs::write(&report_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    assert!(failures.is_empty(), "failed fields: {failures:?}");
    println!(
        "SURFACE-FLUX-TEMPORAL-120 fields=4 adapter={} software={}",
        ctx.device_name(),
        ctx.is_software_adapter()
    );
}

fn with_field(snapshot: &Snapshot, id: FieldId, edit: impl Fn(&mut Field)) -> Snapshot {
    let mut copy = snapshot.clone();
    edit(copy.fields.iter_mut().find(|f| f.id == id).unwrap());
    copy
}

#[test]
fn test_surface_flux_temporal_gpu_rejects_unsupported_representations() {
    let ctx = pollster::block_on(GpuContext::new()).expect("#120 device execution cannot skip");
    let report = oracle();
    let snapshots = snapshots(&report);
    let [s0, s1, s2] = [&snapshots[0], &snapshots[1], &snapshots[2]];
    for id in INSTANTANEOUS_SURFACE_FLUX_FIELDS {
        let none = [None, None, None];
        // Interval means stay outside the instantaneous resolver and blend.
        let interval = |s: &Snapshot, start: i64| {
            with_field(s, id, |f| {
                f.time.kind = TemporalKind::IntervalMean;
                f.time.interval_start_epoch_seconds = Some(start);
                f.time.interval_end_epoch_seconds = Some(f.time.valid_time_epoch_seconds);
            })
        };
        let (m1, m2) = (interval(s1, 0), interval(s2, 10800));
        assert!(matches!(
            resolve_temporal_bracket(
                id,
                &[&m1, &m2],
                RequestedSampleTime::new(Calendar::Gregorian, 14400)
            ),
            Err(TemporalError::NonInstantaneousSnapshot { .. })
        ));
        let means = CanonicalGpuField::upload(&ctx, id, &[&m1, &m2], &none[..2]).unwrap();
        assert!(means
            .prepare_sample(&ctx, surface_request(0.0, 0.0, instant(14400)))
            .is_err());
        assert!(
            CanonicalGpuField::upload(&ctx, id, &[s0, &m1], &none[..2]).is_err(),
            "mixed instantaneous/interval-mean series"
        );
        let source = CanonicalGpuField::upload(&ctx, id, &[s0, s1, s2], &none).unwrap();
        for selection in [
            MeteorologyTimeSelection::IntervalMean {
                start_epoch_seconds: 0,
                end_epoch_seconds: 10800,
            },
            MeteorologyTimeSelection::Static,
            instant(-1),
            instant(21601),
            MeteorologyTimeSelection::Instantaneous(RequestedSampleTime::new(
                Calendar::ProlepticGregorian,
                3600,
            )),
        ] {
            assert!(
                source
                    .prepare_sample(&ctx, surface_request(0.5, 0.5, selection))
                    .is_err(),
                "{id:?} must reject {selection:?}"
            );
        }
        // Incorrect metadata, values or chronology fail at the canonical boundary.
        for (case, edited) in [
            (
                "unit",
                with_field(s1, id, |f| {
                    f.unit = if f.unit == Unit::WattPerSquareMeter {
                        Unit::NewtonPerSquareMeter
                    } else {
                        Unit::WattPerSquareMeter
                    };
                }),
            ),
            (
                "sign",
                with_field(s1, id, |f| f.sign = SignConvention::SignedScalar),
            ),
            ("non-finite", with_field(s1, id, |f| f.values[3] = f32::NAN)),
            (
                "interval metadata",
                with_field(s1, id, |f| f.time.interval_end_epoch_seconds = Some(10800)),
            ),
        ] {
            assert!(
                CanonicalGpuField::upload(&ctx, id, &[s0, &edited, s2], &none).is_err(),
                "{id:?} must reject {case}"
            );
        }
        assert!(
            CanonicalGpuField::upload(&ctx, id, &[s1, s0], &none[..2]).is_err(),
            "reversed timestamps"
        );
        assert!(
            CanonicalGpuField::upload(&ctx, id, &[s0, s0, s2], &none).is_err(),
            "duplicate timestamps"
        );
        let mut missing = s1.clone();
        missing.fields.retain(|f| f.id != id);
        assert!(CanonicalGpuField::upload(&ctx, id, &[s0, &missing], &none[..2]).is_err());
        assert!(CanonicalGpuField::upload(&ctx, id, &[s0], &none[..1]).is_err());

        // Resources cannot cross independently created device contexts.
        let other = pollster::block_on(GpuContext::new()).unwrap();
        assert!(source
            .prepare_sample(&other, surface_request(0.5, 0.5, instant(3600)))
            .is_err());
        let foreign_kernels = MeteorologyCompositionKernels::new(&other).unwrap();
        let mut plan = source
            .prepare_sample(&ctx, surface_request(0.5, 0.5, instant(3600)))
            .unwrap();
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        assert!(plan.encode(&ctx, &foreign_kernels, &mut encoder).is_err());
    }
}
