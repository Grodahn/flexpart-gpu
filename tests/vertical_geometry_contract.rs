//! Pre-decomposition serialization identity through the canonical #30 facade.

use flexpart_gpu::meteorology::{
    vertical::{reconstruct_vertical_geometry_with_motion, NativeVerticalMotion},
    Snapshot, VerticalOrdering,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn fixture(ordering: VerticalOrdering, multi_column: bool) -> (Snapshot, NativeVerticalMotion) {
    let mut snapshot: Snapshot = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-column-v1.json"
    ))
    .expect("canonical fixture");
    let mut motion: NativeVerticalMotion = serde_json::from_str(include_str!(
        "../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"
    ))
    .expect("interface omega fixture");

    if multi_column {
        snapshot.horizontal_grid.nx = 2;
        snapshot.horizontal_grid.ny = 2;
        for field in &mut snapshot.fields {
            field.shape[0] = 2;
            field.shape[1] = 2;
            field.values = field.values.iter().flat_map(|value| [*value; 4]).collect();
        }
        snapshot
            .fields
            .iter_mut()
            .find(|field| field.id == flexpart_gpu::meteorology::FieldId::Orography)
            .expect("terrain")
            .values = vec![250.0, -20.0, 0.0, 1000.0];
        snapshot
            .fields
            .iter_mut()
            .find(|field| field.id == flexpart_gpu::meteorology::FieldId::SurfacePressure)
            .expect("local surface pressure")
            .values = vec![100_000.0, 95_000.0, 90_000.0, 105_000.0];
        motion.values = motion.values.iter().flat_map(|value| [*value; 4]).collect();
    }

    if ordering == VerticalOrdering::Decreasing {
        let vertical = &mut snapshot.vertical_coordinate;
        vertical.ordering = ordering;
        vertical.level_values.reverse();
        vertical
            .interface_values
            .as_mut()
            .expect("interfaces")
            .reverse();
        vertical
            .hybrid_a_interface_pa
            .as_mut()
            .expect("A")
            .reverse();
        vertical.hybrid_b_interface.as_mut().expect("B").reverse();
        let horizontal = snapshot.horizontal_grid.nx * snapshot.horizontal_grid.ny;
        for field in &mut snapshot.fields {
            if field.shape.len() == 3 {
                field.values = field
                    .values
                    .chunks(horizontal)
                    .rev()
                    .flatten()
                    .copied()
                    .collect();
            }
        }
        motion.values = motion
            .values
            .chunks(horizontal)
            .rev()
            .flatten()
            .copied()
            .collect();
    }
    (snapshot, motion)
}

fn serialization_hashes() -> Value {
    let mut cases = Vec::new();
    for multi_column in [false, true] {
        for ordering in [VerticalOrdering::Increasing, VerticalOrdering::Decreasing] {
            let (snapshot, motion) = fixture(ordering, multi_column);
            let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &motion)
                .expect("canonical geometry and motion");
            let runtime = geometry.runtime_view().expect("validated runtime facade");
            let (nx, ny, nz) = runtime.dimensions();
            let mut points = Vec::new();
            for z in 0..=nz {
                for y in 0..ny {
                    for x in 0..nx {
                        let interface = runtime.interface(x, y, z).expect("interface identity");
                        points.push(json!([
                            interface.pressure_pa,
                            interface.height_agl_m,
                            interface.height_asl_m,
                            runtime.terrain_asl_m(x, y).expect("local terrain"),
                        ]));
                        if z < nz {
                            let level = runtime.level(x, y, z).expect("model-level identity");
                            points.push(json!([
                                level.pressure_pa,
                                level.height_agl_m,
                                level.height_asl_m,
                            ]));
                        }
                    }
                }
            }
            cases.push(json!({
                "multi_column": multi_column,
                "ordering": ordering,
                "compact_sha256": format!("{:x}", Sha256::digest(serde_json::to_vec(&geometry).expect("compact geometry"))),
                "pretty_sha256": format!("{:x}", Sha256::digest(serde_json::to_vec_pretty(&geometry).expect("pretty geometry"))),
                "runtime_points_sha256": format!("{:x}", Sha256::digest(serde_json::to_vec(&points).expect("runtime points"))),
            }));
        }
    }
    json!({"base_revision": "375426db534e666e5a95b3b203acdb167a4b7f7d", "cases": cases})
}

#[test]
fn test_vertical_geometry_facade_preserves_pre_decomposition_identity() {
    let actual = serialization_hashes();
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/vertical-geometry-identity-v1.json"))
            .expect("pre-decomposition identities");
    assert_eq!(actual, expected);
}
