//! #171 same-encoder resident-query proof. A missing adapter is a failure.
use flexpart_gpu::{
    gpu::{
        compare_finite_values, download_buffer_typed,
        horizontal::default_comparison_policy,
        meteorology::{
            resident::{ResidentFixtureState, ResidentQueryBatch, ResidentQueryKernels},
            CanonicalGpuField, MeteorologyCompositionKernels, MeteorologyTimeSelection,
        },
        vertical::{physical_model_column_from_runtime, vertical_model_comparison_policy},
        GpuAdapterEvidence, GpuContext, NumericalVerdict, ParticleBuffers,
    },
    meteorology::{
        temporal::RequestedSampleTime, vertical::reconstruct_vertical_geometry, Axis, Calendar,
        FieldId, Snapshot, VerticalStaggering,
    },
    particles::{Particle, ParticleInit},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn instant(t: i64) -> MeteorologyTimeSelection {
    MeteorologyTimeSelection::Instantaneous(RequestedSampleTime::new(Calendar::Gregorian, t))
}

fn snapshot() -> Snapshot {
    let mut s: Snapshot = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-column-v1.json"
    ))
    .unwrap();
    s.horizontal_grid.nx = 4;
    s.horizontal_grid.ny = 3;
    s.horizontal_grid.xlon0_deg = 0.0;
    s.horizontal_grid.ylat0_deg = 0.0;
    s.horizontal_grid.dx_deg = 1.0;
    s.horizontal_grid.dy_deg = 1.0;
    for f in &mut s.fields {
        f.shape[0] = 4;
        f.shape[1] = 3;
        f.time.valid_time_epoch_seconds = 0;
        f.values = f.values.iter().flat_map(|v| vec![*v; 12]).collect();
    }
    s
}

fn pinned_horizontal(case: &str) -> (Value, Vec<f32>) {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/interpolation/contract-v1.json")).unwrap();
    let c = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == case)
        .unwrap()
        .clone();
    let cells = c["golden"]["CANONICAL_NX"].as_u64().unwrap()
        * c["golden"]["CANONICAL_NY"].as_u64().unwrap();
    let values = c["input"].as_array().unwrap()[4..4 + cells as usize]
        .iter()
        .map(|v| v.as_str().unwrap().parse().unwrap())
        .collect();
    (c, values)
}

fn particle(x: i32, y: i32, fx: f32, fy: f32, h: f32) -> Particle {
    Particle::new(&ParticleInit {
        cell_x: x,
        cell_y: y,
        pos_x: fx,
        pos_y: fy,
        pos_z: h,
        mass: [1.0, 0.0, 0.0, 0.0],
        release_point: 0,
        class: 0,
        time: 0,
    })
}

struct Harness<'a> {
    ctx: &'a GpuContext,
    kernels: &'a MeteorologyCompositionKernels<'a>,
    resident: &'a ResidentQueryKernels<'a>,
    rows: Vec<Value>,
}

impl Harness<'_> {
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        id: &str,
        source: &CanonicalGpuField<'_>,
        batch: &mut ResidentQueryBatch<'_>,
        particles: &ParticleBuffers,
        time: MeteorologyTimeSelection,
        expected: Option<&[f32]>,
        reasons: &[u32],
        model: bool,
    ) {
        eprintln!("#171 device case {id}");
        let capacity = batch.capacity();
        let active_count = batch.active_count();
        let mut plan = source.prepare_resident(self.ctx, batch, time).unwrap();
        let sentinel: Vec<f32> = (0..capacity)
            .map(|i| f32::from_bits(0x80000000 + i))
            .collect();
        let state = ResidentFixtureState::upload(self.ctx, &sentinel).unwrap();
        self.ctx
            .device
            .push_error_scope(wgpu::ErrorFilter::Internal);
        self.ctx
            .device
            .push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.ctx
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("#171 one encoder producer -> samplers -> guarded consumer"),
            });
        let sample = plan
            .encode_from_particles(
                self.ctx,
                particles,
                self.kernels,
                self.resident,
                &mut encoder,
            )
            .unwrap();
        assert_eq!(sample.metadata.capacity, capacity);
        assert_eq!(sample.metadata.active_count, active_count);
        assert_eq!(sample.metadata.component_count, 1);
        assert_eq!(sample.metadata.lane_stride, 1);
        assert_eq!(
            sample.metadata.horizontal_staggering,
            flexpart_gpu::meteorology::HorizontalStaggering::CellCenter
        );
        assert_eq!(
            sample.metadata.geometry_identity.len(),
            if model { 2 } else { 0 }
        );
        sample
            .encode_fixture_consumer(self.ctx, self.resident, &state, &mut encoder)
            .unwrap();
        let stages = sample.stages.to_vec();
        let metadata = sample.metadata.clone();
        self.ctx.queue.submit(Some(encoder.finish()));
        self.ctx.device.poll(wgpu::Maintain::Wait);
        let errors: Vec<_> = (0..3)
            .filter_map(|_| pollster::block_on(self.ctx.device.pop_error_scope()))
            .collect();
        assert!(errors.is_empty(), "{id}: {errors:?}");
        drop(plan);
        // All observations occur after the guarded consumer and single submission.
        let status = pollster::block_on(batch.observe_status(self.ctx)).unwrap();
        assert_eq!(status.lanes, reasons, "{id}");
        let output: Vec<f32> = pollster::block_on(download_buffer_typed(
            self.ctx,
            &state.values,
            sentinel.len(),
            "final fixture state",
        ))
        .unwrap();
        let mut comparison = None;
        if let Some(expected) = expected {
            status.require_success().unwrap();
            for (i, reason) in reasons.iter().enumerate() {
                if *reason == 0 {
                    assert_eq!(output[i].to_bits(), sentinel[i].to_bits());
                }
            }
            let policy = if model {
                vertical_model_comparison_policy().unwrap()
            } else {
                default_comparison_policy().unwrap()
            };
            let actual: Vec<_> = output
                .iter()
                .zip(reasons)
                .filter(|(_, r)| **r == 2)
                .map(|(v, _)| f64::from(*v))
                .collect();
            let expected: Vec<_> = expected.iter().map(|v| f64::from(*v)).collect();
            if !expected.is_empty() {
                let result = compare_finite_values(&expected, &actual, policy).unwrap();
                assert_eq!(result.verdict, NumericalVerdict::Passed, "{id}: {output:?}");
                comparison = Some(result);
            } else {
                assert!(actual.is_empty());
            }
        } else {
            assert!(status.fatal);
            assert!(status.require_success().is_err());
            assert_eq!(
                bytemuck::cast_slice::<f32, u8>(&output),
                bytemuck::cast_slice::<f32, u8>(&sentinel),
                "{id}: fatal batch must preserve every byte"
            );
        }
        let final_particles = pollster::block_on(particles.download_particles(self.ctx)).unwrap();
        let query_inputs: Vec<_> = final_particles.iter().map(|p| json!({"cell_x":p.cell_x, "cell_y":p.cell_y,
            "fraction_x_bits":p.pos_x.to_bits(), "fraction_y_bits":p.pos_y.to_bits(), "height_agl_bits":p.pos_z.to_bits(), "flags":p.flags})).collect();
        let input_hash = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!({"metadata":metadata,"queries":query_inputs})).unwrap()
            )
        );
        self.rows.push(json!({ "id": id, "metadata": metadata, "canonical_stages": stages,
            "query_inputs": query_inputs, "input_sha256": input_hash,
            "comparison": comparison,
            "order": ["device_status_reset", "particle_producer", "resident_adapter", "canonical_sampling", "device_result_status", "status_guarded_fixture_consumer"],
            "submission_count": 1, "intermediate_d2h_count": 0, "host_prepare_sample": false,
            "status": status, "fixture_output_bits": output.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), "passed": true }));
    }
}

#[test]
fn test_resident_query_chain_device_status_and_per_lane_geometry() {
    let report = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/ci-gate/meteorology-resident/report.json");
    if report.exists() {
        std::fs::remove_file(&report).unwrap();
    }
    let ctx = pollster::block_on(GpuContext::new()).expect("#171 requires actual WGSL execution");
    eprintln!("#171 adapter {:?}", GpuAdapterEvidence::from_context(&ctx));
    let kernels = MeteorologyCompositionKernels::new(&ctx).unwrap();
    let resident = ResidentQueryKernels::new(&ctx).unwrap();
    let mut h = Harness {
        ctx: &ctx,
        kernels: &kernels,
        resident: &resident,
        rows: Vec::new(),
    };
    let mut s0 = snapshot();
    let (_, values) = pinned_horizontal("horizontal-interior");
    let mut field = s0.fields[0].clone();
    field.id = FieldId::MixingHeight;
    field.unit = flexpart_gpu::meteorology::Unit::Meter;
    field.values = values;
    s0.fields.push(field);
    let mut s1 = s0.clone();
    for f in &mut s1.fields {
        f.time.valid_time_epoch_seconds = 3600;
    }
    for v in &mut s1.fields.last_mut().unwrap().values {
        *v += 10.0;
    }
    let surface =
        CanonicalGpuField::upload(&ctx, FieldId::MixingHeight, &[&s0, &s1], &[None, None]).unwrap();
    let valid = particle(1, 0, 0.25, 0.5, 0.0);
    let mut host = vec![valid; 4];
    let mut particles = ParticleBuffers::from_particles(&ctx, &host);
    let mut batch = ResidentQueryBatch::new(&ctx, &s0.horizontal_grid, 4, 1).unwrap();
    for count in [1, 3, 4] {
        batch.set_active_count(count).unwrap();
        particles.set_dispatch_count(count as usize);
        let mut reasons = vec![0; 4];
        reasons[..count as usize].fill(2);
        h.run(
            &format!("surface-active-{count}"),
            &surface,
            &mut batch,
            &particles,
            instant(1800),
            Some(&vec![235.0; count as usize]),
            &reasons,
            false,
        );
    }
    host[2].flags = 0;
    host[2].pos_x = f32::NAN;
    particles.upload_particles(&ctx, &host).unwrap();
    h.run(
        "inactive-nonfinite-lane",
        &surface,
        &mut batch,
        &particles,
        instant(1800),
        Some(&[235.0; 3]),
        &[2, 2, 0, 2],
        false,
    );
    host.fill(valid);
    host[1].cell_x = i32::MIN;
    particles.upload_particles(&ctx, &host).unwrap();
    h.run(
        "negative-signed-cell-memory-safe",
        &surface,
        &mut batch,
        &particles,
        instant(1800),
        None,
        &[2, 3, 2, 2],
        false,
    );
    host.fill(valid);
    particles.upload_particles(&ctx, &host).unwrap();
    h.run(
        "reuse-after-fatal-status-reset",
        &surface,
        &mut batch,
        &particles,
        instant(1800),
        Some(&[235.0; 4]),
        &[2; 4],
        false,
    );
    for (id, bad, reason) in [
        ("nonfinite-fraction", particle(1, 0, f32::NAN, 0.5, 0.0), 5),
        (
            "nonfinite-height",
            particle(1, 0, 0.25, 0.5, f32::INFINITY),
            5,
        ),
        ("invalid-fraction", particle(1, 0, 1.0, 0.5, 0.0), 4),
        (
            "nonperiodic-east-overshoot",
            particle(3, 0, 0.5, 0.0, 0.0),
            6,
        ),
        ("north-overshoot", particle(1, 2, 0.0, 0.1, 0.0), 6),
        ("outside-cell", particle(i32::MAX, 0, 0.0, 0.0, 0.0), 6),
    ] {
        host.fill(valid);
        host[1] = bad;
        particles.upload_particles(&ctx, &host).unwrap();
        h.run(
            id,
            &surface,
            &mut batch,
            &particles,
            instant(1800),
            None,
            &[2, reason, 2, 2],
            false,
        );
    }
    host.fill(particle(3, 2, 0.0, 0.0, 0.0));
    particles.upload_particles(&ctx, &host).unwrap();
    h.run(
        "nonperiodic-exact-east-north",
        &surface,
        &mut batch,
        &particles,
        instant(0),
        Some(&[420.0; 4]),
        &[2; 4],
        false,
    );

    let mut precise0 = s0.clone();
    precise0.fields.last_mut().unwrap().values = (0..12)
        .map(|i| if i % 4 == 1 { 1.0e20 } else { 1.0 })
        .collect();
    let mut precise1 = precise0.clone();
    for f in &mut precise1.fields {
        f.time.valid_time_epoch_seconds = 3600;
    }
    let precision_source = CanonicalGpuField::upload(
        &ctx,
        FieldId::MixingHeight,
        &[&precise0, &precise1],
        &[None, None],
    )
    .unwrap();
    let fraction = f32::from_bits(1.0_f32.to_bits() - 1);
    assert_eq!(1.0 + fraction, 2.0); // Whole-coordinate f32 reconstruction loses this fraction.
    host.fill(particle(1, 0, fraction, 0.0, 0.0));
    particles.upload_particles(&ctx, &host).unwrap();
    h.run(
        "split-cell-fraction-survives-endpoint-rounding",
        &precision_source,
        &mut batch,
        &particles,
        instant(0),
        Some(&[(1.0 - fraction) * 1.0e20 + fraction; 4]),
        &[2; 4],
        false,
    );

    // Fatal status is reduced before the consumer across workgroups, too.
    let mut many_host = vec![valid; 130];
    many_host[65].cell_y = -1;
    let mut many_particles = ParticleBuffers::from_particles(&ctx, &many_host);
    let mut many_batch = ResidentQueryBatch::new(&ctx, &s0.horizontal_grid, 130, 130).unwrap();
    let mut many_reasons = vec![2; 130];
    many_reasons[65] = 3;
    h.run(
        "fatal-across-workgroups",
        &surface,
        &mut many_batch,
        &many_particles,
        instant(1800),
        None,
        &many_reasons,
        false,
    );
    many_batch.set_active_count(0).unwrap();
    many_particles.set_dispatch_count(0);
    h.run(
        "reuse-with-empty-active-prefix",
        &surface,
        &mut many_batch,
        &many_particles,
        instant(1800),
        Some(&[]),
        &vec![0; 130],
        false,
    );

    // Different valid lanes may select different #30 profiles. Temperature is
    // changed by column to produce independently reconstructed runtime heights.
    let mut m0 = snapshot();
    let temperature = m0
        .fields
        .iter_mut()
        .find(|f| f.id == FieldId::Temperature)
        .unwrap();
    for (i, value) in temperature.values.iter_mut().enumerate() {
        *value += (i % 4) as f32;
    }
    let mut wind = m0.fields[0].clone();
    wind.id = FieldId::WindU;
    wind.unit = flexpart_gpu::meteorology::Unit::MeterPerSecond;
    wind.sign = flexpart_gpu::meteorology::SignConvention::PositiveEastward;
    wind.shape.push(3);
    wind.axis_order.push(Axis::Z);
    wind.vertical_staggering = VerticalStaggering::LevelCenter;
    wind.values = [30.0, 20.0, 10.0]
        .iter()
        .flat_map(|v| (0..12).map(move |i| v + (i % 4) as f32 * 100.0))
        .collect();
    m0.fields.push(wind);
    let mut m1 = m0.clone();
    for f in &mut m1.fields {
        f.time.valid_time_epoch_seconds = 3600;
    }
    let r0 = reconstruct_vertical_geometry(&m0).unwrap();
    let r1 = reconstruct_vertical_geometry(&m1).unwrap();
    let v0 = r0.runtime_view().unwrap();
    let v1 = r1.runtime_view().unwrap();
    let field = &m0.fields.last().unwrap().values;
    let (z0, _) = physical_model_column_from_runtime(v0, FieldId::WindU, field, 0, 0).unwrap();
    let (z3, _) = physical_model_column_from_runtime(v0, FieldId::WindU, field, 3, 0).unwrap();
    assert_ne!(z0, z3);
    let model = CanonicalGpuField::upload(&ctx, FieldId::WindU, &[&m0, &m1], &[Some(v0), Some(v1)])
        .unwrap();
    host = vec![
        particle(0, 0, 0.0, 0.0, -100.0),
        particle(3, 0, 0.0, 0.0, (z3[0] + z3[1]) * 0.5),
        particle(1, 0, 0.0, 0.0, 100000.0),
        particle(0, 0, 0.0, 0.0, z0[1]),
    ];
    particles.upload_particles(&ctx, &host).unwrap();
    h.run(
        "model-distinct-compatible-profiles-lower-interior-upper",
        &model,
        &mut batch,
        &particles,
        instant(1800),
        Some(&[10.0, 315.0, 130.0, 20.0]),
        &[2; 4],
        true,
    );
    host[1] = particle(0, 0, 0.5, 0.0, z0[1]);
    particles.upload_particles(&ctx, &host).unwrap();
    h.run(
        "within-stencil-differing-geometry-owned-by-118",
        &model,
        &mut batch,
        &particles,
        instant(1800),
        None,
        &[2, 7, 2, 2],
        true,
    );

    // Pinned #87 periodic seam cases, retaining their exact query/value evidence.
    let (periodic, values) = pinned_horizontal("horizontal-periodic-wrap");
    let mut p0 = s0.clone();
    p0.horizontal_grid.dx_deg = 90.0;
    p0.horizontal_grid.xlon0_deg = 0.0;
    p0.horizontal_grid.longitude_domain = flexpart_gpu::meteorology::LongitudeDomain::ZeroTo360;
    p0.fields.last_mut().unwrap().values = values;
    let mut p1 = p0.clone();
    for f in &mut p1.fields {
        f.time.valid_time_epoch_seconds = 3600;
    }
    let periodic_source =
        CanonicalGpuField::upload(&ctx, FieldId::MixingHeight, &[&p0, &p1], &[None, None]).unwrap();
    let mut periodic_batch = ResidentQueryBatch::new(&ctx, &p0.horizontal_grid, 4, 4).unwrap();
    for (i, q) in periodic["golden"]["queries"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let x = q["XY"][0].as_f64().unwrap();
        let y = q["XY"][1].as_f64().unwrap();
        host.fill(particle(
            x.floor() as i32,
            y.floor() as i32,
            (x - x.floor()) as f32,
            (y - y.floor()) as f32,
            0.0,
        ));
        particles.upload_particles(&ctx, &host).unwrap();
        h.run(
            &format!("pinned-periodic-edge-{i}"),
            &periodic_source,
            &mut periodic_batch,
            &particles,
            instant(0),
            Some(&[q["VALUE"][0].as_f64().unwrap() as f32; 4]),
            &[2; 4],
            false,
        );
    }
    assert!(surface
        .prepare_resident(&ctx, &mut periodic_batch, instant(0))
        .is_err());
    assert!(surface
        .prepare_resident(&ctx, &mut batch, MeteorologyTimeSelection::Static)
        .is_err());
    assert!(ResidentQueryBatch::new(&ctx, &s0.horizontal_grid, 1, 2).is_err());
    let other = pollster::block_on(GpuContext::new()).unwrap();
    assert!(surface
        .prepare_resident(&other, &mut batch, instant(0))
        .is_err());
    let foreign_particles = ParticleBuffers::from_particles(&other, &host);
    let mut rejected_encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    assert!(batch
        .encode_particles(&ctx, &foreign_particles, &resident, &mut rejected_encoder)
        .is_err());
    assert!(batch
        .encode_particles(&other, &particles, &resident, &mut rejected_encoder)
        .is_err());
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(revision.status.success());
    let shaders = [
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
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(report, serde_json::to_vec_pretty(&json!({
        "schema": {"id": "flexpart-gpu.meteorology-resident", "version": 1},
        "revision": String::from_utf8(revision.stdout).unwrap().trim(), "adapter": GpuAdapterEvidence::from_context(&ctx),
        "shader_bundle_sha256": format!("{:x}", Sha256::digest(shaders.as_bytes())),
        "scientific_policy": "reuse #87/#88/#89; composition proof only", "cases": h.rows,
        "static_grid_context_time_rejection": true, "passed": true,
    })).unwrap()).unwrap();
}
