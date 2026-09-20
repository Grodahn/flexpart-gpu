use flexpart_gpu::meteorology::{Requirements, Snapshot, SCHEMA_VERSION};

#[test]
fn checked_in_synthetic_fixture_validates_and_roundtrips() {
    let source = include_str!("../../fixtures/meteorology/synthetic-v1.json");
    let snapshot: Snapshot = serde_json::from_str(source).expect("parse canonical fixture");

    snapshot
        .validate(&Requirements::advection())
        .expect("checked-in canonical fixture must validate");

    assert_eq!(snapshot.provenance().schema_version, SCHEMA_VERSION);

    let encoded = serde_json::to_string(&snapshot).expect("serialize canonical fixture");
    let decoded: Snapshot = serde_json::from_str(&encoded).expect("roundtrip canonical fixture");
    assert_eq!(decoded, snapshot);
}
