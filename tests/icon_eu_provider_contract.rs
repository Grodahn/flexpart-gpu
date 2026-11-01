//! Regression lock for the #230 ICON-EU provider feasibility decision.
//!
//! The provider contract records, for every canonical #29 field, whether the
//! real DWD ICON-EU regular-lat-lon GRIB2 sample can supply it, and the
//! resulting `NO_GO` for ICON-EU as the first #32 ingestion provider. This
//! test does not decode GRIB2 or download data. It fails closed when the
//! contract drifts from the canonical field set in `FIELD_SPECS`, marks a
//! field supported without evidence, loses a blocker, or when the committed
//! raw GRIB2 bytes stop matching their retrieval manifest.
//! Reproduction and evidence: docs/research/icon-eu-provider-230.md.

use flexpart_gpu::meteorology::{HorizontalGrid, FIELD_SPECS};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const CONTRACT_PATH: &str = "fixtures/meteorology/icon-eu-audit-v1/provider-contract.json";
const MANIFEST_PATH: &str = "fixtures/meteorology/icon-eu-audit-v1/retrieval-manifest.json";
const RAW_DIRECTORY: &str = "fixtures/meteorology/icon-eu-audit-v1/raw";
/// Pinned SHA-256 of the canonical LF contract; update deliberately.
const CONTRACT_SHA256: &str = "699ccf8fe09c5c87366446973f168fbf53f9ff5e829a87f74311626d08213d68";
const EXPECTED_VERDICT: &str = "NO_GO";
const RAW_FIXTURE_COUNT: usize = 10;
const SUPPORTED: [&str; 2] = ["SUPPORTED_AS_IS", "SUPPORTED_WITH_PROVEN_TRANSFORMATION"];
const CLASSIFICATIONS: [&str; 5] = [
    "SUPPORTED_AS_IS",
    "SUPPORTED_WITH_PROVEN_TRANSFORMATION",
    "REQUIRES_SCIENTIFIC_DECISION",
    "MISSING_SOURCE",
    "UNSUPPORTED",
];
/// #230 follow-up blocker issues that must stay referenced by the contract.
const BLOCKER_ISSUES: [u64; 5] = [231, 232, 233, 234, 235];

fn read_canonical(path: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .replace("\r\n", "\n")
}

fn parse(path: &str) -> Value {
    serde_json::from_str(&read_canonical(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string `{key}` in {value}"))
}

fn issue_numbers(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .expect("issue list")
        .iter()
        .map(|item| item.as_u64().expect("issue number"))
        .collect()
}

#[test]
fn test_icon_eu_contract_is_pinned() {
    let digest = format!(
        "{:x}",
        Sha256::digest(read_canonical(CONTRACT_PATH).as_bytes())
    );
    assert_eq!(
        digest, CONTRACT_SHA256,
        "provider contract changed without updating the pin"
    );
}

#[test]
fn test_icon_eu_matrix_covers_every_canonical_field_once() {
    let contract = parse(CONTRACT_PATH);
    let matrix = contract["field_matrix"].as_array().expect("field matrix");
    let listed: Vec<String> = matrix
        .iter()
        .map(|entry| text(entry, "canonical_id").to_owned())
        .collect();
    let unique: BTreeSet<&String> = listed.iter().collect();
    assert_eq!(unique.len(), listed.len(), "duplicate canonical field rows");
    let canonical: BTreeSet<String> = FIELD_SPECS
        .iter()
        .map(|spec| {
            serde_json::to_value(spec.id)
                .expect("field id")
                .as_str()
                .expect("id")
                .to_owned()
        })
        .collect();
    assert_eq!(
        unique.into_iter().cloned().collect::<BTreeSet<_>>(),
        canonical
    );
}

#[test]
fn test_icon_eu_supported_rows_are_evidenced_and_blocked_rows_owned() {
    let contract = parse(CONTRACT_PATH);
    for entry in contract["field_matrix"].as_array().expect("field matrix") {
        let id = text(entry, "canonical_id");
        let classification = text(entry, "classification");
        assert!(
            CLASSIFICATIONS.contains(&classification),
            "{id}: {classification}"
        );
        assert!(
            CLASSIFICATIONS.contains(&text(entry, "quantity_mapping")),
            "{id}"
        );
        let evidence = entry["evidence"].as_array().expect("evidence list");
        let blockers = issue_numbers(&entry["blocker_issues"]);
        if SUPPORTED.contains(&classification) {
            assert!(!evidence.is_empty(), "{id}: supported without evidence");
            assert!(blockers.is_empty(), "{id}: supported but blocked");
            assert!(SUPPORTED.contains(&text(entry, "quantity_mapping")), "{id}");
        } else {
            assert!(
                !blockers.is_empty(),
                "{id}: {classification} without an owning issue"
            );
        }
    }
}

#[test]
fn test_icon_eu_decision_follows_from_the_matrix() {
    let contract = parse(CONTRACT_PATH);
    let decision = &contract["decision"];
    assert_eq!(text(decision, "verdict"), EXPECTED_VERDICT);
    let supported: BTreeSet<&str> = contract["field_matrix"]
        .as_array()
        .expect("field matrix")
        .iter()
        .filter(|entry| SUPPORTED.contains(&text(entry, "classification")))
        .map(|entry| text(entry, "canonical_id"))
        .collect();
    let subset: BTreeSet<&str> = decision["decodable_subset"]
        .as_array()
        .expect("subset")
        .iter()
        .map(|item| item.as_str().expect("field id"))
        .collect();
    assert_eq!(subset, supported);
    assert!(decision["decodable_subset_consumers"]
        .as_array()
        .expect("consumers")
        .is_empty());
    for (consumer, readiness) in contract["consumer_readiness"]
        .as_object()
        .expect("readiness")
    {
        assert!(
            !issue_numbers(&readiness["blocked_by"]).is_empty(),
            "{consumer} unblocked"
        );
    }
    let prerequisites: BTreeSet<u64> =
        issue_numbers(&decision["icon_eu_later_adapter_prerequisites"])
            .into_iter()
            .collect();
    assert_eq!(prerequisites, BLOCKER_ISSUES.into_iter().collect());
}

#[test]
fn test_icon_eu_canonical_grid_is_regional() {
    let contract = parse(CONTRACT_PATH);
    let grid: HorizontalGrid =
        serde_json::from_value(contract["grid"]["canonical_horizontal_grid"].clone())
            .expect("canonical grid");
    assert_eq!((grid.nx, grid.ny), (1377, 657));
    assert!(!grid.is_periodic_x());
    let octets = &contract["grid"]["octet_identity"];
    let first_lon = octets["first_lon_deg"].as_f64().expect("first longitude");
    assert!((first_lon - 360.0 - grid.xlon0_deg).abs() < 1.0e-9);
}

#[test]
fn test_icon_eu_raw_fixtures_match_retrieval_manifest() {
    let manifest = parse(MANIFEST_PATH);
    let mut count = 0;
    for cycle in std::fs::read_dir(RAW_DIRECTORY).expect("raw directory") {
        let cycle = cycle.expect("cycle entry").path();
        for file in std::fs::read_dir(&cycle).expect("cycle directory") {
            let path = file.expect("raw file").path();
            let relative = format!(
                "{}/{}.bz2",
                cycle.file_name().expect("cycle").to_string_lossy(),
                path.file_name().expect("file").to_string_lossy()
            );
            let expected = manifest["files"]
                .as_array()
                .expect("manifest files")
                .iter()
                .find(|item| item["path"] == relative.as_str())
                .unwrap_or_else(|| panic!("{relative} missing from the retrieval manifest"));
            let bytes = std::fs::read(&path).expect("raw bytes");
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                text(expected, "grib2_sha256"),
                "{relative}"
            );
            count += 1;
        }
    }
    assert_eq!(count, RAW_FIXTURE_COUNT);
}
