use flexpart_gpu::io::{self, pbl_params};
use flexpart_gpu::pbl;
use flexpart_gpu::simulation::{BackwardTimeLoopConfig, ForwardTimeLoopConfig};
use flexpart_gpu::validation::candidate_physics::CandidatePhysicsProfile;
use flexpart_gpu::validation::case::CandidatePhysicsProfileRef;
use flexpart_gpu::wind::SurfaceFields;

// Frozen from the pre-move Rust Default at b430aa5, in public field order.
const DEFAULT_BITS: [u32; 8] = [
    0x3dcc_cccd,
    0x4120_0000,
    0x3f80_0000,
    0x3e80_0000,
    0x3e80_0000,
    0x4448_0000,
    0x42c8_0000,
    0x458c_a000,
];

fn option_bits(options: pbl::PblComputationOptions) -> [u32; 8] {
    [
        options.roughness_length_m.to_bits(),
        options.wind_reference_height_m.to_bits(),
        options.heat_flux_neutral_threshold_w_m2.to_bits(),
        options.bulk_richardson_critical.to_bits(),
        options.min_shear_squared_m2_s2.to_bits(),
        options.fallback_mixing_height_m.to_bits(),
        options.hmix_min_m.to_bits(),
        options.hmix_max_m.to_bits(),
    ]
}

#[test]
fn test_pbl_options_defaults_preserve_all_eight_bits() {
    assert_eq!(
        option_bits(pbl::PblComputationOptions::default()),
        DEFAULT_BITS
    );
    assert_eq!(
        option_bits(io::PblComputationOptions::default()),
        DEFAULT_BITS
    );
    assert_eq!(
        option_bits(pbl_params::PblComputationOptions::default()),
        DEFAULT_BITS
    );
}

#[test]
fn test_pbl_options_public_paths_are_interchangeable_with_existing_consumers() {
    assert_eq!(
        std::any::TypeId::of::<pbl::PblComputationOptions>(),
        std::any::TypeId::of::<io::PblComputationOptions>()
    );
    assert_eq!(
        std::any::TypeId::of::<pbl::PblComputationOptions>(),
        std::any::TypeId::of::<pbl_params::PblComputationOptions>()
    );
    let options = [
        pbl::PblComputationOptions {
            roughness_length_m: 0.2,
            ..Default::default()
        },
        io::PblComputationOptions {
            roughness_length_m: 0.2,
            ..Default::default()
        },
        pbl_params::PblComputationOptions {
            roughness_length_m: 0.2,
            ..Default::default()
        },
    ];
    let surface = SurfaceFields::zeros(1, 1);
    for options in options {
        let forward = ForwardTimeLoopConfig {
            pbl_options: options,
            ..Default::default()
        };
        let backward = BackwardTimeLoopConfig {
            pbl_options: forward.pbl_options,
            ..Default::default()
        };
        let prepared = pbl_params::compute_pbl_parameters_from_met(
            pbl_params::PblMetInputGrids {
                surface: &surface,
                profile: None,
            },
            backward.pbl_options,
        )
        .expect("same nominal options accepted by CPU preparation");
        assert_eq!(prepared.pbl_state.hmix[[0, 0]].to_bits(), DEFAULT_BITS[5]);
        assert_eq!(option_bits(options)[0], 0.2_f32.to_bits());
    }
}

#[test]
fn test_pbl_options_profile_preserves_all_eight_bits_and_field_mapping() {
    let mut profile = CandidatePhysicsProfile::load(&CandidatePhysicsProfileRef::canonical())
        .expect("frozen canonical profile");
    assert_eq!(option_bits(profile.pbl_options()), DEFAULT_BITS);

    let p = &mut profile.pbl_computation_options;
    p.roughness_length_m = 0.125;
    p.wind_reference_height_m = 12.0;
    p.heat_flux_neutral_threshold_w_m2 = 2.0;
    p.bulk_richardson_critical = 0.375;
    p.min_shear_squared_m2_s2 = 0.5;
    p.fallback_mixing_height_m = 900.0;
    p.hmix_min_m = 120.0;
    p.hmix_max_m = 4000.0;
    assert_eq!(
        option_bits(profile.pbl_options()),
        [0.125_f32, 12.0, 2.0, 0.375, 0.5, 900.0, 120.0, 4000.0].map(f32::to_bits)
    );
}

#[test]
fn test_pbl_options_public_paths_preserve_required_device_results() {
    use flexpart_gpu::gpu::{
        dispatch_pbl_diagnostics_gpu, GpuContext, PblBuffers, SurfaceFieldBuffer,
    };

    let ctx = pollster::block_on(GpuContext::new()).expect("required WGSL device execution");
    if flexpart_gpu::gpu::is_software_adapter_requested_from_env() {
        assert!(ctx.is_software_adapter(), "required software WGSL adapter");
    }
    let surface = SurfaceFields::zeros(1, 1);
    let uploaded = SurfaceFieldBuffer::from_surface_fields(&ctx, &surface);
    let mut results = Vec::new();
    for options in [
        pbl::PblComputationOptions::default(),
        io::PblComputationOptions::default(),
        pbl_params::PblComputationOptions::default(),
    ] {
        let output =
            PblBuffers::from_state(&ctx, &pbl::PblState::new(1, 1)).expect("PBL output buffer");
        dispatch_pbl_diagnostics_gpu(&ctx, &uploaded, &output, &options)
            .expect("existing GPU consumer accepts each public path");
        let state = pollster::block_on(output.download_state(&ctx)).expect("PBL device result");
        results.push(
            [
                state.ustar[[0, 0]],
                state.wstar[[0, 0]],
                state.hmix[[0, 0]],
                state.oli[[0, 0]],
                state.sshf[[0, 0]],
                state.ssr[[0, 0]],
                state.surfstr[[0, 0]],
            ]
            .map(f32::to_bits),
        );
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(results[0], results[2]);
    assert_eq!(results[0][2], DEFAULT_BITS[5]);
    eprintln!(
        "PBL-OPTIONS-147: adapter={:?} software_adapter={} public_paths=3 device_results=identical",
        ctx.adapter_info(),
        ctx.is_software_adapter()
    );
}
