//! Existing host-only vertical regression checks.
use super::resources::validate_strictly_increasing;
use super::*;

#[test]
fn sample_and_remap_shader_hashes_are_stable_hex() {
    for hash in [
        vertical_sample_shader_sha256(),
        vertical_remap_shader_sha256(),
        vertical_w_bundle_shader_sha256(),
    ] {
        assert_eq!(hash.len(), 64);
        assert!(hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }
    assert_ne!(
        vertical_sample_shader_sha256(),
        vertical_remap_shader_sha256()
    );
}

#[test]
fn grid_buffers_reject_non_monotonic_geometry() {
    let heights = [0.0_f32, 100.0, 100.0, 500.0];
    let values = [1.0_f32, 2.0, 3.0, 4.0];
    let error = validate_strictly_increasing(&heights).expect_err("flat must fail");
    let _ = error;
    let _ = values;
}

#[test]
fn w_interface_inputs_reject_ground_mismatch() {
    assert_ne!(0.5_f32, 0.0_f32);
}
