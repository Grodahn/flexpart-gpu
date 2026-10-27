//! Genuine #180 controlled shared-wind experiment through both real timestep drivers.
//! The explicit shared forcing matches the Fortran driver, independently of remap tests.
use super::*;
use wgpu::util::DeviceExt;

#[path = "../../../tests/support/shared_height.rs"]
mod shared_fixture;

fn timestamp(seconds: i64) -> String {
    let (date, within_day) = if seconds < 0 {
        ("20231231", 86400 + seconds)
    } else {
        ("20240101", seconds)
    };
    format!(
        "{date}{:02}{:02}{:02}",
        within_day / 3600,
        (within_day % 3600) / 60,
        within_day % 60
    )
}

fn sources(geometry: usize, direction: i64) -> [Snapshot; 2] {
    let initial = shared_fixture::snapshot(1);
    [1, 2].map(|member| {
        let mut s = shared_fixture::snapshot(member);
        s.horizontal_grid.xlon0_deg = 0.0;
        s.horizontal_grid.ylat0_deg = 0.0;
        s.horizontal_grid.dx_deg = 1.0;
        s.horizontal_grid.dy_deg = 1.0;
        for f in &mut s.fields {
            if geometry == 0
                && matches!(
                    f.id,
                    FieldId::SurfacePressure
                        | FieldId::Temperature
                        | FieldId::SpecificHumidity
                        | FieldId::Temperature2m
                        | FieldId::Dewpoint2m
                )
            {
                let original = initial.fields.iter().find(|o| o.id == f.id).unwrap();
                for (i, v) in f.values.iter_mut().enumerate() {
                    *v = original.values[(i / 4) * 4];
                }
            }
            if f.time.kind != TemporalKind::Static {
                f.time.valid_time_epoch_seconds =
                    START_SECONDS + (member as i64 - 1) * direction * 3600;
            }
        }
        s
    })
}

/// Explicit experiment inputs, copied from the immutable #180 input controls, not its outputs.
/// Fortran also assigns already-shared uu/vv/ww directly before calling real advance.
fn controlled_shared_winds(
    ctx: &GpuContext,
    owners: &crate::simulation::CanonicalMeteorologyResources<'_>,
    row: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let direction = row[2].as_i64().unwrap() as f32;
    let epoch = row[3].as_i64().unwrap() as f32;
    let old = row[6].as_f64().unwrap() as f32;
    let end = row[7].as_f64().unwrap() as f32;
    let slope = row[8].as_f64().unwrap() as f32;
    let mut retained = Vec::new();
    for member in &owners.fields()[0].members {
        let uv = member.prepared_uv.as_ref().unwrap();
        let w = member.prepared_w.as_ref().unwrap();
        let relative_time =
            (uv.source_identity().time.valid_time_epoch_seconds - START_SECONDS) as f32;
        let fraction = (relative_time - epoch * 1200.0 * direction) / (600.0 * direction);
        let u = direction * 0.5 / (600.0 * (1.0 / 111_200.0_f32));
        let us = vec![u; 16];
        let vs = vec![0.0_f32; 16];
        let ws: Vec<_> = (0..16)
            .map(|i| direction * (old + (end - old) * fraction + slope * (i % 2) as f32))
            .collect();
        // Explicit test-only meteorological upload; the production sampler and boundary are unchanged.
        upload_control(ctx, uv.u(), &us);
        upload_control(ctx, uv.v(), &vs);
        upload_control(ctx, w.w(), &ws);
        retained.push(json!({"source_time":uv.source_identity().time,"u":us,"v":vs,"w":ws,"prepared_w":w.metadata()}));
    }
    retained
}

fn upload_control(ctx: &GpuContext, output: &wgpu::Buffer, values: &[f32]) {
    let input = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("explicit #180 shared-wind input"),
            contents: bytemuck::cast_slice(values),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let pass = resident::QueryPass::new(
        ctx,
        "test-only shared-wind assignment",
        include_str!("../../shaders/transport_top_fixture_assign.wgsl"),
        &[true, false],
    );
    let params = resident::uniform(ctx, &[values.len() as u32, 0, 0, 0]);
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    pass.encode(
        ctx,
        &[&input, output, &params],
        values.len() as u32,
        &mut encoder,
    );
    ctx.queue.submit(Some(encoder.finish()));
}

#[test]
fn test_transport_top_pinned_production_driver_matrix() {
    let root = std::env::var("FLEXPART_GPU_TRANSPORT_TOP_EVIDENCE").unwrap_or_else(|_| {
        format!(
            "target/ci-gate/transport-top/validation-{}-compaction-{}",
            std::env::var("FLEXPART_GPU_VALIDATION").unwrap_or_else(|_| "0".into()),
            std::env::var("FLEXPART_GPU_COMPACTION").unwrap_or_else(|_| "0".into())
        )
    });
    let root = std::path::Path::new(&root);
    std::fs::create_dir_all(root).unwrap();
    let _ = std::fs::remove_file(root.join("report.json"));
    let oracle_bytes = std::fs::read("fixtures/transport/top-boundary-v1/report.json").unwrap();
    let oracle: serde_json::Value = serde_json::from_slice(&oracle_bytes).unwrap();
    let grid = GridDomain {
        xlon0: 0.0,
        ylat0: 0.0,
        dx: 1.0,
        dy: 1.0,
        nx: 2,
        ny: 2,
    };
    let wind = uniform_wind_field(
        &WindFieldGrid::new(
            2,
            2,
            3,
            3,
            3,
            1.0,
            1.0,
            0.0,
            0.0,
            ndarray::Array1::from_vec(vec![0.0, 1.0, 2.0]),
        ),
        1000.0,
        1000.0,
        1000.0,
    );
    let mut surface = SurfaceFields::zeros(2, 2);
    surface.surface_pressure_pa.fill(101_325.0);
    surface.temperature_2m_k.fill(280.0);
    surface.dewpoint_2m_k.fill(275.0);
    surface.mixing_height_m.fill(100.0);
    let mut rows = Vec::new();
    for raw in oracle["science"]["rows"]["ADVANCE"].as_array().unwrap() {
        let row = raw.as_array().unwrap();
        let id = row[0].as_u64().unwrap();
        let geometry = row[1].as_u64().unwrap() as usize;
        let direction = row[2].as_i64().unwrap();
        let epoch = row[3].as_i64().unwrap();
        let scenario = row[4].as_u64().unwrap();
        let start_seconds = epoch * 1200 * direction;
        let start = timestamp(start_seconds);
        let end = timestamp(start_seconds + 600 * direction);
        let height = row[5].as_f64().unwrap();
        let snapshots = sources(geometry, direction);
        let runtimes = snapshots
            .each_ref()
            .map(|s| runtime(s, "#198 explicit normalized zero-omega preparation"));
        let releases = [ReleaseConfig {
            name: "transport-top".into(),
            start_time: start.clone(),
            end_time: start.clone(),
            lon: 0.25,
            lat: 0.0,
            z_min: height,
            z_max: height,
            mass_kg: 1.0,
            particle_count: 130,
            species_masses_kg: None,
            raw: BTreeMap::new(),
        }];
        let scale = crate::physics::VelocityToGridScale {
            x_grid_per_meter: 1.0 / 111_200.0,
            y_grid_per_meter: 1.0 / 111_200.0,
            z_grid_per_meter: 1.0,
            ..crate::physics::VelocityToGridScale::IDENTITY
        };
        let mut forward = (direction == 1).then(|| {
            pollster::block_on(ForwardTimeLoopDriver::new(
                ForwardTimeLoopConfig {
                    start_timestamp: start.clone(),
                    end_timestamp: end.clone(),
                    timestep_seconds: 600,
                    velocity_to_grid_scale: scale,
                    ..Default::default()
                },
                &releases,
                grid.clone(),
                131,
            ))
            .expect("required forward adapter")
        });
        let mut backward = (direction == -1).then(|| {
            pollster::block_on(BackwardTimeLoopDriver::new(
                BackwardTimeLoopConfig {
                    start_timestamp: start,
                    end_timestamp: end,
                    timestep_seconds: 600,
                    velocity_to_grid_scale: scale,
                    receptors: vec![BackwardReceptorConfig {
                        name: "transport-top".into(),
                        lon: 0.25,
                        lat: 0.0,
                        z_m: height,
                        particle_count: 130,
                        mass_kg: 1.0,
                    }],
                    ..Default::default()
                },
                grid.clone(),
                131,
            ))
            .expect("required backward adapter")
        });
        if let Some(d) = &mut forward {
            d.test_set_active_prefix(130);
        }
        if let Some(d) = &mut backward {
            d.test_set_active_prefix(130);
        }
        let ctx = forward.as_ref().map_or_else(
            || backward.as_ref().unwrap().meteorology_context(),
            |d| d.meteorology_context(),
        );
        let order = if direction == 1 { [0, 1] } else { [1, 0] };
        let source = bracket_with_initial(
            ctx,
            order.map(|i| &snapshots[i]),
            order.map(|i| &runtimes[i]),
            usize::from(direction == -1),
        );
        let mut slot = CanonicalMeteorologySlot::default();
        let prepared = if let Some(d) = &forward {
            d.prepare_canonical_meteorology(source, &mut slot)
        } else {
            backward
                .as_ref()
                .unwrap()
                .prepare_canonical_meteorology(source, &mut slot)
        }
        .unwrap();
        let owners = Arc::clone(prepared.resources());
        let ctx = prepared.context();
        let adapter = GpuAdapterEvidence::from_context(ctx);
        let uv = owners.fields()[0].members[0].prepared_uv.as_ref().unwrap();
        let target = uv.target().identity().clone();
        let device_target = pollster::block_on(crate::gpu::download_buffer_typed::<f32>(
            ctx,
            uv.target().heights(),
            4,
            "test device target",
        ))
        .unwrap();
        let target_status = pollster::block_on(crate::gpu::download_buffer_typed::<u32>(
            ctx,
            uv.target().status(),
            1,
            "test target status",
        ))
        .unwrap();
        assert_eq!(device_target[3], 9521.84375);
        assert_eq!(target_status, [2]);
        let controlled = controlled_shared_winds(ctx, &owners, row);
        let mut preimage = target.clone();
        preimage.identity_sha256.clear();
        let target_identity_json = serde_json::to_string(&preimage).unwrap();
        let input = json!({"snapshot_json":snapshots.iter().map(|s|serde_json::to_string(s).unwrap()).collect::<Vec<_>>(),
            "native_motion_json":snapshots.iter().map(|s|serde_json::to_string(&native_motion(s,"#198 explicit normalized zero-omega preparation")).unwrap()).collect::<Vec<_>>(),
            "target_identity_json":target_identity_json,"runtimes":runtimes,"controlled_shared_winds":controlled,"target":target,"device_target":device_target,
            "target_status":target_status,"initial_z":height,"nxmax_from_nxfield":2});
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let input_file = format!("input-{id}.json");
        std::fs::write(root.join(&input_file), &input_bytes).unwrap();
        drop(prepared);
        let met = MetTimeBracket {
            canonical: Some(&owners),
            wind_t0: &wind,
            wind_t1: &wind,
            surface_t0: &surface,
            surface_t1: &surface,
            time_t0_seconds: START_SECONDS + (3600 * direction).min(0),
            time_t1_seconds: START_SECONDS + (3600 * direction).max(0),
        };
        let forcing = ForwardStepForcing {
            settling_enabled: scenario == 9,
            ..Default::default()
        };
        if scenario == 9 {
            let before = if let Some(d) = &forward {
                pollster::block_on(d.test_raw_particles())
            } else {
                pollster::block_on(backward.as_ref().unwrap().test_raw_particles())
            };
            let error = if let Some(d) = &mut forward {
                pollster::block_on(d.run_timestep(&met, &forcing))
                    .unwrap_err()
                    .to_string()
            } else {
                pollster::block_on(backward.as_mut().unwrap().run_timestep(&met, &forcing))
                    .unwrap_err()
                    .to_string()
            };
            assert!(error.contains("sticky particle-stop state"));
            let after = if let Some(d) = &forward {
                pollster::block_on(d.test_raw_particles())
            } else {
                pollster::block_on(backward.as_ref().unwrap().test_raw_particles())
            };
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&before),
                bytemuck::cast_slice::<_, u8>(&after)
            );
            rows.push(json!({"oracle_id":id,"settling_rejected":true,"error":error,"initial_bytes":bytemuck::cast_slice::<_,u8>(&before),"final_bytes":bytemuck::cast_slice::<_,u8>(&after),"adapter":adapter,"input_file":input_file,"input_sha256":format!("{:x}",Sha256::digest(&input_bytes))}));
            continue;
        }
        let (published, status, metadata, values, queries, corrected) =
            if let Some(d) = &mut forward {
                pollster::block_on(d.run_timestep(&met, &forcing)).unwrap();
                let (p, s, m, v, q) = pollster::block_on(d.test_advection_state());
                (
                    p,
                    s,
                    m,
                    v,
                    q,
                    pollster::block_on(d.test_corrected_advection()),
                )
            } else {
                let d = backward.as_mut().unwrap();
                pollster::block_on(d.run_timestep(&met, &forcing)).unwrap();
                let (p, s, m, v, q) = pollster::block_on(d.test_advection_state());
                (
                    p,
                    s,
                    m,
                    v,
                    q,
                    pollster::block_on(d.test_corrected_advection()),
                )
            };
        for region in status[..792].chunks_exact(132) {
            assert_eq!(region[0], 0);
            assert!(region[1..131].iter().all(|s| *s == 2));
            assert_eq!(region[131], 0);
        }
        assert_eq!(status[792], 0);
        for lane in 0..130 {
            assert_eq!(
                queries[lane].height_agl_m.to_bits(),
                (height as f32).to_bits()
            );
            if scenario != 8 {
                assert_eq!(
                    corrected[lane].pos_z.to_bits(),
                    (row[11].as_f64().unwrap() as f32).to_bits(),
                    "oracle row {id}, lane {lane}"
                );
            }
        }
        assert_eq!(corrected[130].time, -123);
        assert_eq!(corrected[130].vel_u, 42.0);
        assert_eq!(
            bytemuck::bytes_of(&corrected[130]),
            bytemuck::bytes_of(&published[130])
        );
        let device_file = format!("device-{id}.json");
        let bytes = serde_json::to_vec(&json!({"queries":queries,"sampled_values":values,"status":status,"metadata":metadata,
            "corrected_bytes":bytemuck::cast_slice::<_,u8>(&corrected),"published_bytes":bytemuck::cast_slice::<_,u8>(&published)})).unwrap();
        std::fs::write(root.join(&device_file), &bytes).unwrap();
        rows.push(json!({"oracle_id":id,"settling_rejected":false,"adapter":adapter,"device_file":device_file,"device_sha256":format!("{:x}",Sha256::digest(&bytes)),
            "input_file":input_file,"input_sha256":format!("{:x}",Sha256::digest(&input_bytes)),"comparison":if scenario == 8 {"no_trajectory_policy_204"} else {"exact_f32_projection_of_original_z"}}));
    }
    let sources = [
        "src/gpu/advection_resident.rs",
        "src/gpu/meteorology/transport_boundary.rs",
        "src/gpu/meteorology/transport_top_tests.rs",
        "src/simulation/timeloop/forward/timestep.rs",
        "src/simulation/timeloop/forward/operators.rs",
        "src/simulation/timeloop/backward.rs",
        "src/simulation/timeloop/forward.rs",
        "src/gpu/meteorology/advection_production_tests.rs",
        "tests/support/shared_height.rs",
        "src/gpu/meteorology.rs",
        "src/simulation/timeloop/meteorology.rs",
        "src/simulation/timeloop/forcing.rs",
    ];
    let hashes: BTreeMap<_, _> = sources
        .map(|p| {
            (
                p,
                format!("{:x}", Sha256::digest(std::fs::read(p).unwrap())),
            )
        })
        .into();
    let shaders: BTreeMap<_, _> = [
        "transport_top_fixture_assign",
        "advection_predictor_query",
        "advection_corrector",
        "advection_step_guard",
        "advection_step_commit",
        "particle_query",
        "horizontal_interpolation",
        "vertical_sample",
        "temporal_interpolation",
        "shared_height_query_status",
        "resident_sample_status",
        "resident_status_reset",
        "resident_query_adapter",
        "shared_height_uv",
        "shared_height_w_remap",
        "interior_w_slope",
    ]
    .map(|p| {
        (
            p,
            format!(
                "{:x}",
                Sha256::digest(std::fs::read(format!("src/shaders/{p}.wgsl")).unwrap())
            ),
        )
    })
    .into();
    let report = json!({"schema":"flexpart-gpu.transport-top.v1","execution":"wgsl_device","candidate_revision":shared_fixture::revision(),
        "candidate_executable_sha256":format!("{:x}",Sha256::digest(std::fs::read(std::env::current_exe().unwrap()).unwrap())),
        "source_hashes":hashes,"shader_hashes":shaders,"oracle_fixture_sha256":format!("{:x}",Sha256::digest(&oracle_bytes)),"rows":rows,
        "precision_scope":"f32 state; exact f32 projection is not f64 trajectory equivalence; f64-only predicate probes excluded",
        "scientific_verdict":"AWAITING_FRESH_PINNED_AUDIT"});
    std::fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("TRANSPORT-TOP-198: both real drivers, 56 exact projections, 8 trajectory diagnostics (#204), 8 settling requests rejected (#203)");
}
