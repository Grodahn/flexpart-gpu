//! Offline lock of the #238 real-source provider decision; no production decoder.

use flexpart_gpu::meteorology::FIELD_SPECS;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::Path;

const ROOT: &str = "fixtures/meteorology/ifs-open-audit-v1";
const LOCK_SHA256: &str = "807418cc011b8438a5f64514870fc7075dd2315f66592f4811ceb202d00b6db2";

fn read(name: &str) -> Value {
    let bytes = std::fs::read(Path::new(ROOT).join(name)).expect("research artifact");
    serde_json::from_slice(&bytes).expect("research JSON")
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().expect(key)
}

#[test]
fn test_ifs_open_complete_field_matrix_and_bounded_verdict() {
    let contract = read("provider-contract.json");
    let matrix = contract["field_matrix"].as_array().expect("field matrix");
    let expected: BTreeSet<_> = FIELD_SPECS
        .iter()
        .map(|spec| {
            serde_json::to_value(spec.id)
                .expect("field id")
                .as_str()
                .expect("id")
                .to_owned()
        })
        .collect();
    let actual: BTreeSet<_> = matrix
        .iter()
        .map(|row| text(row, "canonical_id").to_owned())
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), matrix.len());
    for row in matrix {
        let supported = matches!(
            text(row, "classification"),
            "SUPPORTED_AS_IS" | "SUPPORTED_WITH_PROVEN_TRANSFORMATION"
        );
        assert!(!row["evidence"].as_array().expect("evidence").is_empty());
        assert_eq!(
            row["blocker_issues"]
                .as_array()
                .expect("blockers")
                .is_empty(),
            supported
        );
        for key in [
            "flexpart_consumer",
            "source_units_sign",
            "vertical",
            "temporal",
            "grid",
            "availability",
            "transformation",
            "fatal_behavior",
        ] {
            assert!(!text(row, key).is_empty());
        }
    }
    for key in [
        "verdict",
        "full_europe_48h_p0_verdict",
        "5_to_8_km_target_verdict",
    ] {
        assert_eq!(text(&contract["decision"], key), "NO_GO");
    }
    assert!(contract["decision"]["supported_production_consumer_subset"]
        .as_array()
        .expect("subset")
        .is_empty());
    assert_eq!(
        contract["vertical"]["runtime_30_geometry_sufficient"],
        false
    );
    assert_eq!(contract["vertical"]["hybrid_interface_ab_observed"], false);
}

#[test]
fn test_ifs_open_all_frozen_artifacts_and_raw_messages_have_expected_hashes() {
    let lock_bytes = std::fs::read(Path::new(ROOT).join("artifact-lock.json")).expect("lock bytes");
    assert_eq!(format!("{:x}", Sha256::digest(&lock_bytes)), LOCK_SHA256);
    let lock = read("artifact-lock.json");
    for (name, expected) in lock.as_object().expect("artifact lock") {
        let bytes = std::fs::read(Path::new(ROOT).join(name)).expect("locked bytes");
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            expected.as_str().expect("SHA-256"),
            "{name}"
        );
    }
    let manifest = read("retrieval-manifest.json");
    for section in ["indexes", "messages"] {
        let entries = manifest[section].as_array().expect("manifest section");
        assert_eq!(entries.len(), if section == "indexes" { 51 } else { 34 });
        for entry in entries {
            let name = text(entry, "path");
            let bytes = std::fs::read(Path::new(ROOT).join(name)).expect("preserved source bytes");
            assert_eq!(bytes.len() as u64, entry["bytes"].as_u64().expect("size"));
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                text(entry, "sha256"),
                "{name}"
            );
            if section == "messages" {
                assert_eq!(&bytes[..4], b"GRIB");
                assert_eq!(bytes[7], 2);
                assert_eq!(&bytes[bytes.len() - 4..], b"7777");
            }
        }
    }
}

#[test]
fn test_ifs_open_independent_decoder_proof_covers_every_raw_message() {
    let modern = read("decoded-2.48.0.json");
    let legacy = read("decoded-2.24.2.json");
    let messages = modern["messages"].as_array().expect("modern messages");
    let old = legacy["messages"].as_array().expect("old messages");
    assert_eq!(messages.len(), 34);
    assert_eq!(old.len(), 34);
    for message in messages {
        let other = old
            .iter()
            .find(|row| row["path"] == message["path"])
            .expect("same source");
        assert_eq!(message["sha256"], other["sha256"]);
        assert_eq!(message["values"], other["values"]);
        assert_eq!(message["octets"], other["octets"]);
        assert_eq!(
            message["independent_values"]["max_absolute_difference"],
            0.0
        );
        assert_eq!(other["independent_values"]["max_absolute_difference"], 0.0);
    }
}
