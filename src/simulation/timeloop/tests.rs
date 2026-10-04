//! Existing driver helper regressions.

use super::backward::collect_source_collections;
use crate::coords::GridDomain;
use crate::io::TimeBoundsBehavior;
use crate::particles::MAX_SPECIES;

use super::config::validate_backward_config;
use super::time::{format_timestamp_seconds, interpolation_alpha, parse_timestamp_seconds};
use super::{
    BackwardReceptorConfig, BackwardSourceRegionConfig, BackwardTimeLoopConfig, TimeLoopError,
};

#[test]
fn timestamp_parse_and_format_roundtrip() {
    let ts = "20240101123456";
    let seconds = parse_timestamp_seconds(ts).expect("timestamp parses");
    let roundtrip = format_timestamp_seconds(seconds).expect("timestamp formats");
    assert_eq!(roundtrip, ts);
}

#[test]
fn interpolation_alpha_matches_expected_midpoint() {
    let alpha = interpolation_alpha(100, 200, 150, TimeBoundsBehavior::Strict)
        .expect("midpoint alpha computes");
    assert!((alpha - 0.5).abs() < 1.0e-6);
}

#[test]
fn backward_config_rejects_inverted_range() {
    let config = BackwardTimeLoopConfig {
        start_timestamp: "20240101000000".to_string(),
        end_timestamp: "20240101000001".to_string(),
        receptors: vec![BackwardReceptorConfig {
            name: "r1".to_string(),
            lon: 0.0,
            lat: 0.0,
            z_m: 10.0,
            particle_count: 1,
            mass_kg: 1.0,
        }],
        ..BackwardTimeLoopConfig::default()
    };
    let err = validate_backward_config(&config).expect_err("range should be rejected");
    assert!(matches!(
        err,
        TimeLoopError::InvalidBackwardTimeRange { .. }
    ));
}

#[test]
fn source_collection_counts_active_hits() {
    use crate::particles::{Particle, ParticleInit, ParticleStore};

    let mut mass = [0.0_f32; MAX_SPECIES];
    mass[0] = 1.5;
    let mut store = ParticleStore::with_capacity(2);
    let slot = store
        .add(Particle::new(&ParticleInit {
            cell_x: 9,
            cell_y: 4,
            pos_x: 0.0,
            pos_y: 0.0,
            pos_z: 2.0,
            mass,
            release_point: 0,
            class: 0,
            time: 0,
        }))
        .expect("store has free slot");
    assert_eq!(slot, 0);

    let grid = GridDomain {
        xlon0: 0.0,
        ylat0: 0.0,
        dx: 1.0,
        dy: 1.0,
        nx: 64,
        ny: 64,
    };
    let source = BackwardSourceRegionConfig {
        name: "src".to_string(),
        lon_min: 8.5,
        lon_max: 9.5,
        lat_min: 3.5,
        lat_max: 4.5,
        z_min_m: 0.0,
        z_max_m: 10.0,
    };
    let collections = collect_source_collections(&store, &grid, &[source]);
    let result = collections.get("src").expect("source collection exists");
    assert_eq!(result.hit_count, 1);
    assert!((result.total_mass_kg - 1.5).abs() < 1.0e-6);
}
