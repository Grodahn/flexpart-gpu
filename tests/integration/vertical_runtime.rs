use flexpart_gpu::meteorology::{
    vertical::{
        reconstruct_vertical_geometry, reconstruct_vertical_geometry_with_motion,
        resolve_release_height_at_column,
        resolve_release_height_range_at_column, NativeVerticalMotion, VerticalTransformError,
    },
    Snapshot, VerticalReference,
};

fn snapshot() -> Snapshot {
    serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-column-v1.json"
    ))
    .expect("synthetic #30 vertical fixture must parse")
}

#[test]
fn release_height_agl_and_asl_share_the_same_nonzero_terrain_transform() {
    let snapshot = snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("vertical geometry");
    let runtime = geometry.runtime_view().expect("valid runtime view");
    assert_eq!(runtime.dimensions(), (1, 1, 3));
    assert_eq!(runtime.terrain_asl_m(0, 0).expect("terrain"), 250.0);

    let from_agl = resolve_release_height_at_column(
        runtime,
        0,
        0,
        100.0,
        VerticalReference::AboveGroundLevel,
    )
    .expect("100 m AGL over 250 m terrain");
    assert_eq!(from_agl.height_agl_m, 100.0);
    assert_eq!(from_agl.height_asl_m, 350.0);

    let from_asl = resolve_release_height_at_column(
        runtime,
        0,
        0,
        350.0,
        VerticalReference::AboveMeanSeaLevel,
    )
    .expect("350 m ASL over 250 m terrain");
    assert_eq!(from_asl.height_agl_m, 100.0);
    assert_eq!(from_asl.height_asl_m, 350.0);

    assert_eq!(from_agl.height_agl_m, from_asl.height_agl_m);
    assert_eq!(from_agl.height_asl_m, from_asl.height_asl_m);
}

#[test]
fn release_height_ranges_preserve_declared_reference_and_ordering() {
    let snapshot = snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("vertical geometry");
    let runtime = geometry.runtime_view().expect("valid runtime view");

    let range = resolve_release_height_range_at_column(
        runtime,
        0,
        0,
        50.0,
        150.0,
        VerticalReference::AboveGroundLevel,
    )
    .expect("ordered AGL range");
    assert_eq!(range.lower.height_asl_m, 300.0);
    assert_eq!(range.upper.height_asl_m, 400.0);

    let error = resolve_release_height_range_at_column(
        runtime,
        0,
        0,
        150.0,
        50.0,
        VerticalReference::AboveGroundLevel,
    )
    .expect_err("inverted vertical bounds must fail");
    assert!(matches!(
        error,
        VerticalTransformError::InvalidReleaseHeightRange { .. }
    ));
}

#[test]
fn release_height_reference_is_never_inferred() {
    let snapshot = snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("vertical geometry");
    let runtime = geometry.runtime_view().expect("valid runtime view");

    let error = resolve_release_height_at_column(
        runtime,
        0,
        0,
        100.0,
        VerticalReference::ModelNative,
    )
    .expect_err("model-native release height must be rejected");
    assert!(matches!(
        error,
        VerticalTransformError::UnsupportedReleaseHeightReference {
            reference: VerticalReference::ModelNative
        }
    ));
}

#[test]
fn release_asl_below_local_terrain_fails_before_injection() {
    let snapshot = snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("vertical geometry");
    let runtime = geometry.runtime_view().expect("valid runtime view");

    let error = resolve_release_height_at_column(
        runtime,
        0,
        0,
        249.0,
        VerticalReference::AboveMeanSeaLevel,
    )
    .expect_err("release below local terrain must fail");
    assert!(matches!(
        error,
        VerticalTransformError::InvalidReleaseHeight { .. }
    ));
}

#[test]
fn runtime_view_exposes_column_local_geometry_without_interpolation() {
    let snapshot = snapshot();
    let geometry = reconstruct_vertical_geometry(&snapshot).expect("vertical geometry");
    let runtime = geometry.runtime_view().expect("valid runtime view");

    let top = runtime.level(0, 0, 0).expect("top level");
    let middle = runtime.level(0, 0, 1).expect("middle level");
    let bottom = runtime.level(0, 0, 2).expect("bottom level");

    assert!(top.pressure_pa < middle.pressure_pa);
    assert!(middle.pressure_pa < bottom.pressure_pa);
    assert!(top.height_agl_m > middle.height_agl_m);
    assert!(middle.height_agl_m > bottom.height_agl_m);
    assert_eq!(top.height_asl_m - top.height_agl_m, 250.0);

    assert!(runtime.interface_pressure_pa(0, 0, 0).expect("top interface")
        < runtime.interface_pressure_pa(0, 0, 3).expect("surface interface"));
}

#[test]
fn runtime_view_preserves_normalized_motion_staggering_for_interpolation() {
    let snapshot = snapshot();
    let motion: NativeVerticalMotion = serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-omega-interface-v1.json"
    ))
    .expect("synthetic omega fixture must parse");

    let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &motion)
        .expect("geometry plus normalized motion");
    let runtime = geometry.runtime_view().expect("valid runtime view");
    let normalized = runtime
        .vertical_velocity()
        .expect("normalized vertical motion");

    assert_eq!(
        normalized.vertical_staggering,
        flexpart_gpu::meteorology::VerticalStaggering::LevelInterface
    );
    assert_eq!(normalized.values_ms.len(), 4);
    assert_eq!(
        normalized.provenance.algorithm_id,
        "omega_interface_flexpart11_pinmconv_v1"
    );
    assert!(normalized.values_ms.iter().all(|value| value.is_finite()));
}

#[test]
fn runtime_view_fails_closed_on_corrupt_derived_shapes_and_bounds() {
    let snapshot = snapshot();
    let mut geometry = reconstruct_vertical_geometry(&snapshot).expect("vertical geometry");
    geometry.height_agl_m.pop();

    let error = geometry
        .runtime_view()
        .expect_err("corrupt runtime shape must fail");
    assert!(matches!(
        error,
        VerticalTransformError::RuntimeShapeMismatch {
            field: "height_agl_m",
            ..
        }
    ));

    let geometry = reconstruct_vertical_geometry(&snapshot).expect("vertical geometry");
    let runtime = geometry.runtime_view().expect("valid runtime view");
    let error = runtime
        .level(1, 0, 0)
        .expect_err("out-of-bounds x must fail");
    assert!(matches!(
        error,
        VerticalTransformError::RuntimeIndexOutOfBounds { .. }
    ));
}
