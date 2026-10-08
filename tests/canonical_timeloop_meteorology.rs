//! #173 production-driver preparation proof; missing device execution is a failure.

use flexpart_gpu::{
    coords::GridDomain,
    gpu::{
        compare_finite_values, download_buffer_typed,
        meteorology::{
            resident::{ResidentFixtureState, ResidentQueryBatch, ResidentQueryKernels},
            MeteorologyCompositionError, MeteorologyCompositionKernels, MeteorologyTimeSelection,
        },
        vertical::{vertical_geometry_identity, vertical_model_comparison_policy},
        GpuAdapterEvidence, NumericalVerdict, ParticleBuffers,
    },
    meteorology::{
        temporal::TemporalError,
        vertical::{
            reconstruct_vertical_geometry, reconstruct_vertical_geometry_with_motion,
            NativeVerticalMotion, NativeVerticalMotionKind, NativeVerticalMotionProvenance,
            NativeVerticalMotionSign, NativeVerticalMotionUnit, VerticalTransformResult,
        },
        ContractError, FieldId, SignConvention, Snapshot, TemporalKind, Unit, VerticalStaggering,
    },
    particles::{Particle, ParticleInit},
    simulation::{
        BackwardReceptorConfig, BackwardTimeLoopConfig, BackwardTimeLoopDriver,
        CanonicalMeteorologyBracket, CanonicalMeteorologySlot, ForwardStepForcing,
        ForwardTimeLoopConfig, ForwardTimeLoopDriver, MetTimeBracket, PreparedCanonicalMeteorology,
        TimeLoopError, CANONICAL_WIND_FIELDS,
    },
    wind::{uniform_wind_field, SurfaceFields, WindFieldGrid},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const START_SECONDS: i64 = 1_704_067_200;

fn snapshot(seconds: i64, offset: f32) -> Snapshot {
    let mut snapshot: Snapshot = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-column-v1.json"
    ))
    .unwrap();
    snapshot.horizontal_grid.nx = 2;
    snapshot.horizontal_grid.ny = 2;
    for field in &mut snapshot.fields {
        field.shape[0] = 2;
        field.shape[1] = 2;
        field.values = field.values.iter().flat_map(|v| [*v; 4]).collect();
        if field.time.kind != TemporalKind::Static {
            field.time.valid_time_epoch_seconds = seconds;
        }
    }
    let template = snapshot
        .fields
        .iter()
        .find(|f| f.id == FieldId::Temperature)
        .unwrap()
        .clone();
    for (id, value) in [
        (FieldId::WindU, 1.0 + offset),
        (FieldId::WindV, 2.0 + offset),
        (FieldId::VerticalVelocity, 0.25 + offset / 4.0),
    ] {
        let mut field = template.clone();
        field.id = id;
        let spec = flexpart_gpu::meteorology::FIELD_SPECS
            .iter()
            .find(|spec| spec.id == id)
            .unwrap();
        field.unit = spec.unit;
        field.sign = spec.sign;
        field.values.fill(value);
        snapshot.fields.push(field);
    }
    snapshot
}

fn native_motion(snapshot: &Snapshot, source_id: &str) -> NativeVerticalMotion {
    let values = snapshot
        .fields
        .iter()
        .find(|f| f.id == FieldId::VerticalVelocity)
        .unwrap()
        .values
        .clone();
    NativeVerticalMotion {
        kind: NativeVerticalMotionKind::GeometricVelocity,
        unit: NativeVerticalMotionUnit::MeterPerSecond,
        sign: NativeVerticalMotionSign::PositiveUpward,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values,
        provenance: NativeVerticalMotionProvenance {
            source_id: source_id.into(),
        },
    }
}

fn runtime(snapshot: &Snapshot, source_id: &str) -> VerticalTransformResult {
    reconstruct_vertical_geometry_with_motion(snapshot, &native_motion(snapshot, source_id))
        .unwrap()
}

fn bracket<'a>(
    s: [&'a Snapshot; 2],
    r: [&'a VerticalTransformResult; 2],
) -> CanonicalMeteorologyBracket<'a> {
    CanonicalMeteorologyBracket::new(s, r.map(|r| r.runtime_view().unwrap())).unwrap()
}

fn rejection(s: [&Snapshot; 2], r: [&VerticalTransformResult; 2]) -> MeteorologyCompositionError {
    match CanonicalMeteorologyBracket::new(s, r.map(|r| r.runtime_view().unwrap())) {
        Ok(_) => panic!("invalid canonical bracket accepted"),
        Err(error) => error,
    }
}

#[test]
fn test_canonical_bracket_rejects_missing_components_units_signs_and_staggering() {
    let original = snapshot(START_SECONDS, 0.0);
    let upper = snapshot(START_SECONDS + 4, 4.0);
    let lower_runtime = runtime(&original, "canonical-synthetic");
    let upper_runtime = runtime(&upper, "canonical-synthetic");
    for id in CANONICAL_WIND_FIELDS {
        let mut invalid = original.clone();
        invalid.fields.retain(|f| f.id != id);
        assert!(
            matches!(rejection([&invalid, &upper], [&lower_runtime, &upper_runtime]),
            MeteorologyCompositionError::Contract(ContractError::MissingRequiredField(missing)) if missing == id)
        );
    }
    for change in 0..5 {
        let mut invalid = original.clone();
        let w = invalid
            .fields
            .iter_mut()
            .find(|f| f.id == FieldId::VerticalVelocity)
            .unwrap();
        match change {
            0 => w.unit = Unit::Pascal,
            1 => w.sign = SignConvention::PositiveEastward,
            2 => w.vertical_staggering = VerticalStaggering::NotApplicable,
            3 => w.time.kind = TemporalKind::Static,
            4 => w.values[0] = f32::NAN,
            _ => unreachable!(),
        }
        assert!(matches!(
            rejection([&invalid, &upper], [&lower_runtime, &upper_runtime]),
            MeteorologyCompositionError::Contract(_)
        ));
    }
    let mut interface = original.clone();
    interface.vertical_coordinate.interface_values =
        Some(vec![40_000.0, 60_000.0, 80_000.0, 100_000.0]);
    let w = interface
        .fields
        .iter_mut()
        .find(|f| f.id == FieldId::VerticalVelocity)
        .unwrap();
    w.vertical_staggering = VerticalStaggering::LevelInterface;
    w.shape[2] += 1;
    w.values = vec![0.25; w.shape.iter().product()];
    assert!(matches!(
        rejection([&interface, &upper], [&lower_runtime, &upper_runtime]),
        MeteorologyCompositionError::Unsupported(_)
    ));
    for (id, staggering, axis) in [
        (
            FieldId::WindU,
            flexpart_gpu::meteorology::HorizontalStaggering::XFace,
            0,
        ),
        (
            FieldId::WindV,
            flexpart_gpu::meteorology::HorizontalStaggering::YFace,
            1,
        ),
    ] {
        let mut face = original.clone();
        let field = face.fields.iter_mut().find(|f| f.id == id).unwrap();
        field.horizontal_staggering = staggering;
        field.shape[axis] += 1;
        field.values = vec![1.0; field.shape.iter().product()];
        assert!(matches!(
            rejection([&face, &upper], [&lower_runtime, &upper_runtime]),
            MeteorologyCompositionError::Unsupported(_)
        ));
    }
}

#[test]
fn test_canonical_bracket_rejects_time_grid_runtime_and_motion_mismatches() {
    let lower = snapshot(START_SECONDS, 0.0);
    let upper = snapshot(START_SECONDS + 4, 4.0);
    let r0 = runtime(&lower, "canonical-synthetic");
    let r1 = runtime(&upper, "canonical-synthetic");
    assert!(matches!(
        rejection([&upper, &lower], [&r1, &r0]),
        MeteorologyCompositionError::Time(TemporalError::NonMonotonicTimestamps { .. })
    ));
    assert!(matches!(
        rejection([&lower, &lower], [&r0, &r0]),
        MeteorologyCompositionError::Time(TemporalError::NonMonotonicTimestamps { .. })
    ));
    assert!(matches!(
        rejection([&lower, &upper], [&r1, &r0]),
        MeteorologyCompositionError::Incompatible(_)
    ));
    let no_motion = reconstruct_vertical_geometry(&lower).unwrap();
    assert!(matches!(
        rejection([&lower, &upper], [&no_motion, &r1]),
        MeteorologyCompositionError::Missing(_)
    ));
    let missing_identity = runtime(&lower, "");
    assert!(matches!(
        rejection([&lower, &upper], [&missing_identity, &r1]),
        MeteorologyCompositionError::Missing(_)
    ));
    let mut changed_w = lower.clone();
    changed_w
        .fields
        .iter_mut()
        .find(|f| f.id == FieldId::VerticalVelocity)
        .unwrap()
        .values[0] += 1.0;
    let original_native = NativeVerticalMotion {
        kind: NativeVerticalMotionKind::GeometricVelocity,
        unit: NativeVerticalMotionUnit::MeterPerSecond,
        sign: NativeVerticalMotionSign::PositiveUpward,
        vertical_staggering: VerticalStaggering::LevelCenter,
        values: vec![0.25; 12],
        provenance: NativeVerticalMotionProvenance {
            source_id: "canonical-synthetic".into(),
        },
    };
    let inconsistent_motion =
        reconstruct_vertical_geometry_with_motion(&changed_w, &original_native).unwrap();
    assert!(matches!(
        rejection([&changed_w, &upper], [&inconsistent_motion, &r1]),
        MeteorologyCompositionError::Incompatible(_)
    ));
    for change in 0..4 {
        let mut invalid = upper.clone();
        match change {
            0 => invalid.horizontal_grid.dx_deg *= 2.0,
            1 => invalid.vertical_coordinate.level_values[1] += 1.0,
            2 => {
                for field in &mut invalid.fields {
                    field.time.calendar = flexpart_gpu::meteorology::Calendar::ProlepticGregorian;
                }
            }
            3 => {
                invalid
                    .fields
                    .iter_mut()
                    .find(|f| f.id == FieldId::WindU)
                    .unwrap()
                    .time
                    .valid_time_epoch_seconds += 1
            }
            _ => unreachable!(),
        }
        assert!(CanonicalMeteorologyBracket::new(
            [&lower, &invalid],
            [r0.runtime_view().unwrap(), r1.runtime_view().unwrap()]
        )
        .is_err());
    }
    let valid = bracket([&lower, &upper], [&r0, &r1]);
    assert!(matches!(
        valid.resolve_step_times(START_SECONDS, -1),
        Err(MeteorologyCompositionError::Time(
            TemporalError::BeforeFirstCoverage { .. }
        ))
    ));
    assert!(matches!(
        valid.resolve_step_times(START_SECONDS + 4, 1),
        Err(MeteorologyCompositionError::Time(
            TemporalError::AfterLastCoverage { .. }
        ))
    ));
    assert!(valid.resolve_step_times(i64::MAX, 1).is_err());
}

fn prove_resident(
    prepared: &PreparedCanonicalMeteorology<'_, '_>,
    direction: &str,
    source_bracket: &str,
    snapshots: [&Snapshot; 2],
) -> Vec<Value> {
    let ctx = prepared.context();
    let kernels = MeteorologyCompositionKernels::new(ctx).unwrap();
    let resident = ResidentQueryKernels::new(ctx).unwrap();
    let resources = prepared.resources();
    let source = resources.bracket();
    let particles = ParticleBuffers::from_particles(
        ctx,
        &[Particle::new(&ParticleInit {
            cell_x: 0,
            cell_y: 0,
            pos_x: 0.25,
            pos_y: 0.5,
            pos_z: 500.0,
            mass: [1.0, 0.0, 0.0, 0.0],
            release_point: 0,
            class: 0,
            time: 0,
        })],
    );
    let mut rows = Vec::new();
    for (field_index, field) in resources.fields().iter().enumerate() {
        for (selection, time) in [
            ("current", prepared.times.current),
            ("predicted", prepared.times.predicted),
        ] {
            let mut batch = ResidentQueryBatch::new(ctx, source.horizontal_grid(), 1, 1).unwrap();
            let state = ResidentFixtureState::upload(ctx, &[-999.0]).unwrap();
            let mut plan = field.prepare_resident(ctx, &mut batch, time).unwrap();
            ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
            ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
            ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut encoder = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            let sample = plan
                .encode_from_particles(ctx, &particles, &kernels, &resident, &mut encoder)
                .unwrap();
            assert_eq!(sample.metadata.field_id, CANONICAL_WIND_FIELDS[field_index]);
            assert_eq!(sample.metadata.horizontal_grid, *source.horizontal_grid());
            assert_eq!(
                sample.metadata.vertical_staggering,
                VerticalStaggering::LevelCenter
            );
            assert_eq!(
                sample.metadata.geometry_identity,
                source.identity().geometry_identity
            );
            assert_eq!(
                sample.metadata.geometry_provenance,
                source.identity().geometry_provenance
            );
            assert_eq!(
                sample.metadata.source_snapshot_sha256,
                source.identity().source_snapshot_sha256
            );
            let MeteorologyTimeSelection::Instantaneous(request) = time else {
                unreachable!()
            };
            let inputs = snapshots.map(|s| {
                s.fields
                    .iter()
                    .find(|f| f.id == CANONICAL_WIND_FIELDS[field_index])
                    .unwrap()
            });
            assert_eq!(sample.metadata.source_times, inputs.map(|f| f.time.clone()));
            assert_eq!(sample.metadata.unit, "meter_per_second");
            assert_eq!(sample.metadata.sign, inputs[0].sign);
            let lower_time = inputs[0].time.valid_time_epoch_seconds;
            let upper_time = inputs[1].time.valid_time_epoch_seconds;
            let alpha =
                (request.epoch_seconds - lower_time) as f64 / (upper_time - lower_time) as f64;
            let expected = f64::from(inputs[0].values[0]) * (1.0 - alpha)
                + f64::from(inputs[1].values[0]) * alpha;
            let metadata = sample.metadata.clone();
            let stages = sample.stages.to_vec();
            sample
                .encode_fixture_consumer(ctx, &resident, &state, &mut encoder)
                .unwrap();
            ctx.queue.submit(Some(encoder.finish()));
            ctx.device.poll(wgpu::Maintain::Wait);
            for _ in 0..3 {
                assert!(pollster::block_on(ctx.device.pop_error_scope()).is_none());
            }
            drop(plan);
            let status = pollster::block_on(batch.observe_status(ctx)).unwrap();
            status.require_success().unwrap();
            assert_eq!(status.lanes, [2]);
            let output: Vec<f32> = pollster::block_on(download_buffer_typed(
                ctx,
                &state.values,
                1,
                "#173 final fixture output",
            ))
            .unwrap();
            let comparison = compare_finite_values(
                &[expected],
                &[f64::from(output[0])],
                vertical_model_comparison_policy().unwrap(),
            )
            .unwrap();
            assert_eq!(comparison.verdict, NumericalVerdict::Passed);
            rows.push(json!({"direction":direction, "source_bracket":source_bracket, "selection":selection, "metadata":metadata, "stages":stages,
                "status":status, "output":output, "expected":expected, "comparison":comparison,
                "submission_count":1, "intermediate_d2h_count":0, "passed":true}));
        }
    }
    rows
}

fn source_bracket_evidence(
    prepared: &PreparedCanonicalMeteorology<'_, '_>,
    snapshots: [&Snapshot; 2],
    runtimes: [&VerticalTransformResult; 2],
) -> Value {
    // Preserve the exact encoding hashed by #30, including transitioned sources.
    let snapshot_json = snapshots.map(|snapshot| serde_json::to_string(snapshot).unwrap());
    let identity = prepared.resources().bracket().identity();
    for (encoded, hash) in snapshot_json.iter().zip(&identity.source_snapshot_sha256) {
        assert_eq!(format!("{:x}", Sha256::digest(encoded.as_bytes())), *hash);
    }
    let native_motion_json = std::array::from_fn::<_, 2, _>(|index| {
        let motion = &identity.motion_provenance[index];
        let encoded =
            serde_json::to_string(&native_motion(snapshots[index], &motion.source_id)).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(encoded.as_bytes())),
            motion.source_native_motion_sha256
        );
        encoded
    });
    json!({"snapshot_json":snapshot_json, "native_motion_json":native_motion_json,
        "runtimes":runtimes, "identity":identity})
}

#[test]
fn test_canonical_forward_backward_preparation_reuse_transition_and_resident_execution() {
    let report = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/ci-gate/canonical-timeloop-meteorology/report.json");
    if report.exists() {
        std::fs::remove_file(&report).unwrap();
    }
    let lower = snapshot(START_SECONDS, 0.0);
    let upper = snapshot(START_SECONDS + 4, 4.0);
    let r0 = runtime(&lower, "canonical-synthetic");
    let r1 = runtime(&upper, "canonical-synthetic");
    let grid = GridDomain {
        xlon0: 6.0,
        ylat0: 50.0,
        dx: 0.25,
        dy: 0.25,
        nx: 2,
        ny: 2,
    };
    let mut forward = pollster::block_on(ForwardTimeLoopDriver::new(
        ForwardTimeLoopConfig {
            start_timestamp: "20240101000002".into(),
            end_timestamp: "20240101000003".into(),
            ..ForwardTimeLoopConfig::default()
        },
        &[],
        grid.clone(),
        1,
    ))
    .expect("#173 requires actual driver device preparation");
    let mut backward = pollster::block_on(BackwardTimeLoopDriver::new(
        BackwardTimeLoopConfig {
            start_timestamp: "20240101000002".into(),
            end_timestamp: "20240101000001".into(),
            receptors: vec![BackwardReceptorConfig {
                name: "canonical-receptor".into(),
                lon: 6.0,
                lat: 50.0,
                z_m: 500.0,
                particle_count: 1,
                mass_kg: 1.0,
            }],
            ..BackwardTimeLoopConfig::default()
        },
        grid,
        1,
    ))
    .expect("#173 requires actual backward device preparation");
    let mut forward_slot = CanonicalMeteorologySlot::default();
    let first = forward
        .prepare_canonical_meteorology(bracket([&lower, &upper], [&r0, &r1]), &mut forward_slot)
        .unwrap();
    assert!(!first.reused);
    let retained = Arc::clone(first.resources());
    let second = forward
        .prepare_canonical_meteorology(bracket([&lower, &upper], [&r0, &r1]), &mut forward_slot)
        .unwrap();
    assert!(second.reused);
    assert!(Arc::ptr_eq(first.resources(), second.resources()));
    let mut backward_slot = CanonicalMeteorologySlot::default();
    let reverse = backward
        .prepare_canonical_meteorology(bracket([&lower, &upper], [&r0, &r1]), &mut backward_slot)
        .unwrap();
    assert_eq!(
        first.resources().bracket().identity(),
        reverse.resources().bracket().identity()
    );
    assert_eq!(first.times.current, reverse.times.current);
    assert_ne!(first.times.predicted, reverse.times.predicted);
    assert!(matches!(
        backward.prepare_canonical_meteorology(
            bracket([&lower, &upper], [&r0, &r1]),
            &mut forward_slot
        ),
        Err(TimeLoopError::CanonicalMeteorology(
            MeteorologyCompositionError::Incompatible(_)
        ))
    ));
    let mut rows = prove_resident(&first, "forward", "original", [&lower, &upper]);
    rows.extend(prove_resident(
        &reverse,
        "backward",
        "original",
        [&lower, &upper],
    ));
    let changed = snapshot(START_SECONDS, 2.0);
    let changed_runtime = runtime(&changed, "canonical-synthetic");
    let transition = forward
        .prepare_canonical_meteorology(
            bracket([&changed, &upper], [&changed_runtime, &r1]),
            &mut forward_slot,
        )
        .unwrap();
    assert!(!transition.reused);
    assert!(!Arc::ptr_eq(&retained, transition.resources()));
    assert_ne!(
        retained.bracket().identity().source_snapshot_sha256,
        transition
            .resources()
            .bracket()
            .identity()
            .source_snapshot_sha256
    );
    // The old owner still executes after replacement, as required for forward overlap.
    rows.extend(prove_resident(
        &first,
        "retained-forward",
        "original",
        [&lower, &upper],
    ));
    rows.extend(prove_resident(
        &transition,
        "changed-source",
        "changed",
        [&changed, &upper],
    ));
    let lineage_runtime = runtime(&changed, "different-native-source");
    let lineage = forward
        .prepare_canonical_meteorology(
            bracket([&changed, &upper], [&lineage_runtime, &r1]),
            &mut forward_slot,
        )
        .unwrap();
    assert!(!lineage.reused);
    assert_ne!(
        transition
            .resources()
            .bracket()
            .identity()
            .motion_provenance,
        lineage.resources().bracket().identity().motion_provenance
    );
    assert_eq!(
        retained.bracket().identity().geometry_identity[0],
        vertical_geometry_identity(r0.runtime_view().unwrap())
    );
    let later = snapshot(START_SECONDS + 5, 5.0);
    let later_runtime = runtime(&later, "canonical-synthetic");
    assert!(matches!(
        forward.prepare_canonical_meteorology(
            bracket([&upper, &later], [&r1, &later_runtime]),
            &mut forward_slot
        ),
        Err(TimeLoopError::CanonicalMeteorology(
            MeteorologyCompositionError::Time(TemporalError::BeforeFirstCoverage { .. })
        ))
    ));
    let after_failure = forward
        .prepare_canonical_meteorology(
            bracket([&changed, &upper], [&lineage_runtime, &r1]),
            &mut forward_slot,
        )
        .unwrap();
    assert!(after_failure.reused);
    assert!(Arc::ptr_eq(lineage.resources(), after_failure.resources()));
    assert_eq!(forward.current_time_seconds(), START_SECONDS + 2);
    assert_eq!(backward.current_time_seconds(), START_SECONDS + 2);
    assert_eq!(forward.particle_store().active_count(), 0);
    assert_eq!(backward.particle_store().active_count(), 0);
    let adapters = [
        GpuAdapterEvidence::from_context(first.context()),
        GpuAdapterEvidence::from_context(reverse.context()),
    ];
    let identity = first.resources().bracket().identity().clone();
    let forward_times = first.times;
    let backward_times = reverse.times;
    let reverse_retained = Arc::clone(reverse.resources());
    let lineage_retained = Arc::clone(lineage.resources());
    let source_brackets = json!({
        "original":source_bracket_evidence(&first, [&lower, &upper], [&r0, &r1]),
        "changed":source_bracket_evidence(&transition, [&changed, &upper], [&changed_runtime, &r1]),
        "lineage":source_bracket_evidence(&lineage, [&changed, &upper], [&lineage_runtime, &r1]),
    });
    drop(first);
    drop(second);
    drop(reverse);
    drop(transition);
    drop(lineage);
    drop(after_failure);
    // Explicitly advance the still-legacy drivers. This is not canonical advection:
    // it proves caller-held canonical owners survive actual mutable driver steps.
    let legacy_grid = WindFieldGrid::new(
        2,
        2,
        3,
        3,
        3,
        0.25,
        0.25,
        6.0,
        50.0,
        ndarray::Array1::from_vec(vec![0.0, 1_000.0, 2_000.0]),
    );
    let wind = uniform_wind_field(&legacy_grid, 0.0, 0.0, 0.0);
    let mut surface = SurfaceFields::zeros(2, 2);
    surface.surface_pressure_pa.fill(101_325.0);
    surface.temperature_2m_k.fill(290.0);
    surface.dewpoint_2m_k.fill(285.0);
    surface.mixing_height_m.fill(1_000.0);
    surface.tropopause_height_m.fill(10_000.0);
    let legacy = MetTimeBracket {
        wind_t0: &wind,
        wind_t1: &wind,
        surface_t0: &surface,
        surface_t1: &surface,
        time_t0_seconds: START_SECONDS,
        time_t1_seconds: START_SECONDS + 4,
    };
    let forcing = ForwardStepForcing::default();
    pollster::block_on(forward.run_timestep(&legacy, &forcing)).unwrap();
    pollster::block_on(backward.run_timestep(&legacy, &forcing)).unwrap();
    let next_forward = forward
        .prepare_canonical_meteorology(
            bracket([&changed, &upper], [&lineage_runtime, &r1]),
            &mut forward_slot,
        )
        .unwrap();
    let next_backward = backward
        .prepare_canonical_meteorology(bracket([&lower, &upper], [&r0, &r1]), &mut backward_slot)
        .unwrap();
    assert!(next_forward.reused && next_backward.reused);
    assert!(Arc::ptr_eq(&lineage_retained, next_forward.resources()));
    assert!(Arc::ptr_eq(&reverse_retained, next_backward.resources()));
    assert_ne!(next_forward.times, forward_times);
    assert_ne!(next_backward.times, backward_times);
    rows.extend(prove_resident(
        &next_forward,
        "next-forward",
        "lineage",
        [&changed, &upper],
    ));
    rows.extend(prove_resident(
        &next_backward,
        "next-backward",
        "original",
        [&lower, &upper],
    ));
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(revision.status.success());
    let normalized_inputs = json!({"snapshots":[lower, upper], "runtimes":[r0, r1]});
    let input_json = serde_json::to_string(&normalized_inputs).unwrap();
    let shader_bundle = [
        include_str!("../src/shaders/particle_query.wgsl"),
        include_str!("../src/shaders/resident_status_reset.wgsl"),
        include_str!("../src/shaders/resident_query_adapter.wgsl"),
        include_str!("../src/shaders/horizontal_interpolation.wgsl"),
        include_str!("../src/shaders/vertical_sample.wgsl"),
        include_str!("../src/shaders/temporal_interpolation.wgsl"),
        include_str!("../src/shaders/resident_sample_status.wgsl"),
        include_str!("../src/shaders/resident_fixture_consumer.wgsl"),
    ]
    .join("\n")
    .replace("\r\n", "\n");
    let evidence = json!({"schema":{"id":"flexpart-gpu.canonical-timeloop-meteorology", "version":1},
        "revision":String::from_utf8(revision.stdout).unwrap().trim(),
        "adapters":adapters,
        "inputs":normalized_inputs, "input_json":input_json, "input_sha256":format!("{:x}",Sha256::digest(input_json.as_bytes())),
        "source_brackets":source_brackets,
        "shader_bundle_sha256":format!("{:x}",Sha256::digest(shader_bundle.as_bytes())),
        "identity":identity, "times":{"forward":forward_times,"backward":backward_times},
        "rows":rows,"same_bracket_reused":true,"source_change_rebuilt":true,"motion_lineage_change_rebuilt":true,
        "retained_owner_executed_after_transition":true,"failed_preflight_preserved_slot":true,
        "reused_across_mutable_driver_steps":true,
        "calculation_path":"wgsl_device","validation_level":"production_driver_preparation_and_resource_readiness", "passed":true});
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(report, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    eprintln!("CANONICAL-MET-173: forward backward U/V/center-W resident execution, reuse and transitions passed");
}
