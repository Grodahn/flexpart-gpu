//! Regression lock for the #233 ICON-EU time-averaged surface-flux decision.
//!
//! The decision records, for the four canonical `SurfaceFluxRate` fields,
//! which DWD ICON-EU run-start average supplies them, the sign
//! transformation, the admissible canonical temporal kind, and the verdict
//! for FLEXPART `calcpar`-equivalent consumers. This test does not decode
//! GRIB2 or download data. It fails closed when the decision drifts from
//! `FIELD_SPECS`, admits an average as `Instantaneous`, loses its evidence
//! binding, or stops superseding the #230 contract rows that #233 owns.
//! Reproduction and evidence: docs/research/icon-eu-surface-fluxes-233.md.

use flexpart_gpu::meteorology::{FieldId, TemporalKind, TemporalPolicy, FIELD_SPECS};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const DECISION_PATH: &str = "fixtures/meteorology/icon-eu-flux-233-v1/decision.json";
const EVIDENCE_PATH: &str = "fixtures/meteorology/icon-eu-flux-233-v1/flux-evidence.json";
const MANIFEST_PATH: &str = "fixtures/meteorology/icon-eu-flux-233-v1/retrieval-manifest.json";
const PROVIDER_CONTRACT_PATH: &str = "fixtures/meteorology/icon-eu-audit-v1/provider-contract.json";
/// Pinned SHA-256 of the canonical LF decision; update deliberately.
const DECISION_SHA256: &str = "faf95b81499dc4c491f3a02c4432dbe035524efb5cd24e3230e62953fa95ef6e";
const OWNING_ISSUE: u64 = 233;
const NOT_ADMISSIBLE: &str = "NOT_ADMISSIBLE";
const INTERVAL_MEAN_ADMISSIBLE: &str = "ADMISSIBLE_WITH_PROVEN_TRANSFORMATION";
const CALCPAR_VERDICT: &str = "REQUIRES_NEW_TEMPORAL_ORACLE";

fn read_canonical(path: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .replace("\r\n", "\n")
}

fn canonical_sha256(path: &str) -> String {
    format!("{:x}", Sha256::digest(read_canonical(path).as_bytes()))
}

fn parse(path: &str) -> Value {
    serde_json::from_str(&read_canonical(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string `{key}` in {value}"))
}

fn snake_case<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .expect("serializable enum")
        .as_str()
        .expect("unit variant")
        .to_owned()
}

fn decision_field<'a>(decision: &'a Value, id: &str) -> &'a Value {
    decision["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|item| item["canonical_id"] == id)
        .unwrap_or_else(|| panic!("{id} missing from the #233 decision"))
}

#[test]
fn test_icon_eu_flux_decision_is_pinned() {
    assert_eq!(
        canonical_sha256(DECISION_PATH),
        DECISION_SHA256,
        "decision changed without updating the pin"
    );
}

#[test]
fn test_icon_eu_flux_decision_binds_its_evidence() {
    let decision = parse(DECISION_PATH);
    let evidence = &decision["evidence"];
    assert_eq!(
        canonical_sha256(EVIDENCE_PATH),
        text(evidence, "evidence_sha256")
    );
    assert_eq!(
        canonical_sha256(MANIFEST_PATH),
        text(evidence, "retrieval_manifest_sha256")
    );
    assert_eq!(
        canonical_sha256(PROVIDER_CONTRACT_PATH),
        text(&decision["supersedes"], "provider_contract_sha256")
    );
    let recorded = parse(EVIDENCE_PATH);
    assert_eq!(recorded["identity_problem_count"], 0);
    assert_eq!(
        text(&recorded["decoder"], "label"),
        text(evidence, "primary_decoder")
    );
}

#[test]
fn test_icon_eu_flux_decision_covers_exactly_the_surface_flux_specs() {
    let decision = parse(DECISION_PATH);
    let flux_specs: Vec<_> = FIELD_SPECS
        .iter()
        .filter(|spec| spec.temporal_policy == TemporalPolicy::SurfaceFluxRate)
        .collect();
    let listed: BTreeSet<&str> = decision["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .map(|item| text(item, "canonical_id"))
        .collect();
    assert_eq!(
        listed.len(),
        decision["fields"].as_array().expect("fields").len()
    );
    let expected: BTreeSet<String> = flux_specs.iter().map(|spec| snake_case(spec.id)).collect();
    assert_eq!(
        listed
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<BTreeSet<_>>(),
        expected
    );
    for spec in flux_specs {
        let item = decision_field(&decision, &snake_case(spec.id));
        assert_eq!(text(item, "canonical_unit"), snake_case(spec.unit));
        assert_eq!(text(item, "canonical_sign"), snake_case(spec.sign));
        assert_eq!(
            text(item, "canonical_temporal_kind"),
            snake_case(TemporalKind::IntervalMean)
        );
    }
}

#[test]
fn test_icon_eu_flux_averages_are_never_instantaneous() {
    let decision = parse(DECISION_PATH);
    for item in decision["fields"].as_array().expect("fields") {
        let id = text(item, "canonical_id");
        assert_eq!(text(item, "instantaneous_verdict"), NOT_ADMISSIBLE, "{id}");
        assert_eq!(
            text(item, "interval_mean_verdict"),
            INTERVAL_MEAN_ADMISSIBLE,
            "{id}"
        );
        assert_eq!(
            text(item, "calcpar_consumer_verdict"),
            CALCPAR_VERDICT,
            "{id}"
        );
        assert_ne!(
            text(item, "canonical_temporal_kind"),
            snake_case(TemporalKind::Instantaneous),
            "{id}"
        );
    }
    assert!(
        !decision["follow_up_issues"]
            .as_array()
            .expect("follow-up issues")
            .is_empty(),
        "calcpar-equivalent use needs an owning oracle issue"
    );
}

#[test]
fn test_icon_eu_flux_signs_match_flexpart_reader_semantics() {
    let decision = parse(DECISION_PATH);
    let sensible = decision_field(&decision, &snake_case(FieldId::SensibleHeatFlux));
    assert_eq!(text(sensible, "source"), "ASHFL_S");
    assert_eq!(text(sensible, "source_sign"), "positive_downward");
    assert_eq!(text(sensible, "sign_transformation"), "negate");
    let solar = decision_field(&decision, &snake_case(FieldId::SurfaceSolarRadiation));
    assert_eq!(text(solar, "source"), "ASOB_S");
    assert_eq!(
        text(solar, "sign_transformation"),
        "identity_then_clip_negative_within_bound_to_zero"
    );
    for id in [
        FieldId::SurfaceStressEastward,
        FieldId::SurfaceStressNorthward,
    ] {
        let stress = decision_field(&decision, &snake_case(id));
        assert_eq!(text(stress, "sign_transformation"), "identity");
    }
}

#[test]
fn test_icon_eu_flux_decision_resolves_every_230_row_it_blocks() {
    let decision = parse(DECISION_PATH);
    let contract = parse(PROVIDER_CONTRACT_PATH);
    let blocked: BTreeSet<&str> = contract["field_matrix"]
        .as_array()
        .expect("field matrix")
        .iter()
        .filter(|row| {
            row["blocker_issues"]
                .as_array()
                .expect("blockers")
                .iter()
                .any(|issue| issue.as_u64() == Some(OWNING_ISSUE))
        })
        .map(|row| text(row, "canonical_id"))
        .collect();
    let follow_ups: BTreeSet<u64> = decision["follow_up_issues"]
        .as_array()
        .expect("follow-up issues")
        .iter()
        .map(|issue| issue.as_u64().expect("issue number"))
        .collect();
    let mut decided = BTreeSet::new();
    for item in decision["fields"].as_array().expect("fields") {
        decided.insert(text(item, "canonical_id"));
    }
    for item in decision["derived_fields"]
        .as_array()
        .expect("derived fields")
    {
        let id = text(item, "canonical_id");
        decided.insert(id);
        assert_eq!(text(item, "verdict"), CALCPAR_VERDICT, "{id}");
        let blockers: BTreeSet<u64> = item["blocker_issues"]
            .as_array()
            .expect("blockers")
            .iter()
            .map(|issue| issue.as_u64().expect("issue number"))
            .collect();
        assert!(
            !blockers.contains(&OWNING_ISSUE),
            "{id} still blocked by #233"
        );
        assert!(
            !blockers.is_disjoint(&follow_ups),
            "{id} not handed to a follow-up"
        );
    }
    assert_eq!(blocked, decided);
}
