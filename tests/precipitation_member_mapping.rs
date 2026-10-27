//! Regression lock for the #202 precipitation member-mapping decision.
//!
//! The decision artifact records why #90 interval-mean rates cannot directly
//! populate FLEXPART 11.1 `numpf=1` `interpol_rain` members, and what #119 and
//! #207 receive instead. This test does not rerun the original preparation or
//! oracle; it fails closed when the committed decision, its decoded evidence,
//! the pinned references or the #71 tolerance policy drift apart, and it
//! re-checks the decoded original-routine identities offline.
//! Reproduction: docs/research/precipitation-interval-mapping-202.md.

use flexpart_gpu::meteorology::temporal::{DEFAULT_ABS_TOL, DEFAULT_REL_TOL};
use serde_json::Value;
use sha2::{Digest, Sha256};

const DECISION_PATH: &str = "fixtures/accumulation/precipitation-member-mapping-v1.json";
const EVIDENCE_PATH: &str = "fixtures/accumulation/precipitation-member-mapping-evidence-v1.json";
/// Pinned SHA-256 of the canonical LF decision artifact; update deliberately.
const DECISION_SHA256: &str = "7851210604855a7a7e39094603ad906cf654c103ca09a58a23c8eb812c8e78d5";
/// Interval length of the declared `flex_extract` example (DTIME = 3 h).
const EXAMPLE_DELTA_SECONDS: i64 = 10_800;
/// First and last member times with complete four-interval support in the
/// 16-interval example starting at the trace epoch.
const EXAMPLE_FIRST_MEMBER_SECONDS: i64 = 2 * EXAMPLE_DELTA_SECONDS;
const EXAMPLE_LAST_MEMBER_SECONDS: i64 = 14 * EXAMPLE_DELTA_SECONDS;
/// Ratio of pinned `numpf=1` output to the linear blend of the prepared members.
const PINNED_NUMPF1_SCALE: f64 = 3.0;
/// Pinned `interpol_rain` divisor of the member-pair length, `dtt = dt/3`
/// (`interpol_mod.f90:1309`).
const PINNED_DTT_DIVISOR: f64 = 3.0;
/// Linear blends below this value are excluded from the ratio check because
/// f32 member rounding dominates the quotient.
const RATIO_MIN_LINEAR_MM_PER_HOUR: f64 = 1.0e-3;

const REQUIRED_REJECTIONS: [&str; 12] = [
    "interval_mean_as_member",
    "insufficient_support",
    "outside_coverage",
    "gap_or_overlap",
    "unequal_or_non_integer_hour_delta",
    "member_spacing_mismatch",
    "missing_member",
    "identity_mismatch",
    "non_finite",
    "unsupported_preparation",
    "backward_time",
    "duplicate_member_time",
];

fn read_canonical(path: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .replace("\r\n", "\n")
}

fn sha256_hex(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn parse(path: &str) -> Value {
    serde_json::from_str(&read_canonical(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn within_policy(candidate: f64, oracle: f64) -> bool {
    (candidate - oracle).abs() <= DEFAULT_ABS_TOL + DEFAULT_REL_TOL * oracle.abs()
}

/// Asserts an exactly representable decoded value, such as a zero weight or a
/// zero reproduction difference, without a tolerance.
#[allow(clippy::float_cmp)]
fn assert_exact(actual: f64, expected: f64, what: &str) {
    assert_eq!(actual, expected, "{what}");
}

fn number(value: &Value, key: &str) -> f64 {
    value[key]
        .as_f64()
        .unwrap_or_else(|| panic!("missing number {key}"))
}

fn pinned_commit(manifest: &str) -> String {
    parse(manifest)["pinned_commit"]
        .as_str()
        .expect("manifest pinned_commit")
        .to_owned()
}

/// Recomputes the pinned `interpol_rain` `numpf=1` result from the decoded
/// members and the oracle-reported weights instead of trusting the decoder.
fn recomputed_pinned(row: &Value, key: &str) -> f64 {
    let members = &row[format!("{key}_members_mm_per_h")];
    let (r0, r1) = (
        members[0].as_f64().expect("member r0"),
        members[1].as_f64().expect("member r1"),
    );
    (r0 * number(row, "dt2") + r1 * number(row, "dt1")) / number(row, "dtt")
}

/// Checks that the oracle-reported weights are the pinned `interpol_rain`
/// definitions for an ordinary member pair, including `dtt = dt/3`.
fn assert_pinned_weights(case: &str, row: &Value) {
    let itime = number(row, "itime");
    let memtime_start = row["memtime_s"][0].as_f64().expect("memtime start");
    let memtime_end = row["memtime_s"][1].as_f64().expect("memtime end");
    let pair_seconds = memtime_end - memtime_start;
    assert!(pair_seconds > 0.0, "{case}: not an ordinary member pair");
    assert!(within_policy(number(row, "dt1"), itime - memtime_start));
    assert!(within_policy(number(row, "dt2"), memtime_end - itime));
    assert!(
        within_policy(number(row, "dtt"), pair_seconds / PINNED_DTT_DIVISOR),
        "{case}: dtt is not dt/3"
    );
}

fn node_rows(evidence: &Value, case: &str) -> Vec<Value> {
    evidence["interpol_rain_cases"][case]["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("missing oracle case {case}"))
        .iter()
        .filter(|row| row.get("node").is_some())
        .cloned()
        .collect()
}

#[test]
fn test_precipitation_mapping_decision_digest_is_pinned() {
    assert_eq!(
        sha256_hex(&read_canonical(DECISION_PATH)),
        DECISION_SHA256,
        "decision artifact changed; review the scientific decision and update the pinned digest"
    );
}

#[test]
fn test_precipitation_mapping_evidence_digest_matches_decision() {
    let decision = parse(DECISION_PATH);
    assert_eq!(decision["evidence"]["decoded"], EVIDENCE_PATH);
    assert_eq!(
        sha256_hex(&read_canonical(EVIDENCE_PATH)),
        decision["evidence"]["decoded_sha256"].as_str().unwrap()
    );
}

#[test]
fn test_precipitation_mapping_decision_is_incompatible_with_follow_up() {
    let decision = parse(DECISION_PATH);
    assert_eq!(
        decision["schema"],
        "flexpart-gpu/precipitation-member-mapping-decision/v1"
    );
    assert_eq!(decision["issue"], 202);
    assert_eq!(decision["consumer_issue"], 119);
    assert_eq!(decision["verdict"], "INCOMPATIBLE");
    assert_eq!(decision["compatibility_with_90"]["verdict"], "INCOMPATIBLE");
    assert_eq!(decision["follow_up_issue"], 207);
    assert_eq!(decision["handoff_119"]["blocked_by"][0], 207);
    assert_eq!(
        decision["normative_references"]["flexpart"]["configuration"]["numpf"],
        1
    );
}

#[test]
fn test_precipitation_mapping_pins_match_reference_manifests() {
    let decision = parse(DECISION_PATH);
    let evidence = parse(EVIDENCE_PATH);
    let flexpart = pinned_commit("reference/flexpart-11.1.json");
    let flex_extract = pinned_commit("reference/flex-extract.json");
    assert_eq!(
        decision["normative_references"]["flexpart"]["pinned_commit"],
        flexpart.as_str()
    );
    assert_eq!(
        decision["normative_references"]["flex_extract"]["pinned_commit"],
        flex_extract.as_str()
    );
    assert_eq!(
        evidence["flex_extract"]["pinned_commit"],
        flex_extract.as_str()
    );
}

#[test]
fn test_precipitation_mapping_tolerance_matches_71_policy() {
    for path in [DECISION_PATH, EVIDENCE_PATH] {
        let policy = &parse(path)["tolerance_policy"];
        assert!((number(policy, "absolute") - DEFAULT_ABS_TOL).abs() <= f64::EPSILON);
        assert!((number(policy, "relative") - DEFAULT_REL_TOL).abs() <= f64::EPSILON);
    }
}

#[test]
fn test_precipitation_mapping_rejections_are_enumerated() {
    let decision = parse(DECISION_PATH);
    let codes: Vec<&str> = decision["rejections"]
        .as_array()
        .expect("rejections")
        .iter()
        .map(|rejection| rejection["code"].as_str().expect("rejection code"))
        .collect();
    for required in REQUIRED_REJECTIONS {
        assert!(codes.contains(&required), "missing rejection {required}");
    }
}

#[test]
fn test_precipitation_mapping_audited_fixture_cases_exist() {
    let accumulation = read_canonical("fixtures/accumulation/contract-v1.json");
    let interpolation = read_canonical("fixtures/interpolation/contract-v1.json");
    assert!(accumulation.contains("\"rain-layer-fields-bilinear-rates\""));
    assert!(interpolation.contains("\"rain-layer-fields\""));
}

#[test]
fn test_precipitation_mapping_rrint_representations_match_reader() {
    let evidence = parse(EVIDENCE_PATH);
    let counts = &evidence["precip_messages_per_flux_file"];
    // numpf=1 readwind_ecmwf accepts at most two precipitation fields per file.
    assert_eq!(counts["rrint0"], serde_json::json!([2]));
    assert_eq!(counts["rrint1"], serde_json::json!([6]));
    assert_eq!(evidence["basetime_trailing_duplicate"], true);
    assert_exact(
        number(&evidence, "max_abs_member_minus_darain_result"),
        0.0,
        "darain reproduction",
    );
}

#[test]
fn test_precipitation_mapping_members_are_not_interval_means() {
    let evidence = parse(EVIDENCE_PATH);
    for param in ["142", "143"] {
        let distinct = evidence["member_vs_interval_means"][param]
            .as_array()
            .expect("member audit rows")
            .iter()
            .filter(|row| {
                number(row, "member_mm_per_h") > 0.0
                    && row["equals_ending_mean"] == false
                    && row["equals_starting_mean"] == false
            })
            .count();
        assert!(
            distinct > 0,
            "param {param}: no member distinguishes from interval means"
        );
    }
}

#[test]
fn test_precipitation_mapping_interpol_rain_rows_match_pinned_arithmetic() {
    let evidence = parse(EVIDENCE_PATH);
    let cases = [
        "first_interior",
        "interior_left",
        "interior",
        "last_interior",
        "leading_boundary",
        "final_duplicate_time",
    ];
    for case in cases {
        let rows = node_rows(&evidence, case);
        assert!(!rows.is_empty(), "{case}: no node rows");
        for row in rows {
            if case != "final_duplicate_time" {
                assert_pinned_weights(case, &row);
            }
            for key in ["lsp", "cp"] {
                let oracle = number(&row, key);
                let expected = recomputed_pinned(&row, key);
                assert!(
                    within_policy(expected, oracle),
                    "{case} {key}: {expected} vs {oracle}"
                );
                assert!(within_policy(
                    number(&row, &format!("{key}_pinned_expected")),
                    expected
                ));
                let linear = number(&row, &format!("{key}_linear_point_blend"));
                if linear > RATIO_MIN_LINEAR_MM_PER_HOUR {
                    assert!(
                        within_policy(PINNED_NUMPF1_SCALE * linear, oracle),
                        "{case} {key}: oracle {oracle} is not 3 x linear {linear}"
                    );
                }
            }
        }
    }
}

#[test]
fn test_precipitation_mapping_coverage_endpoints_match_example() {
    let evidence = parse(EVIDENCE_PATH);
    let first = &evidence["interpol_rain_cases"]["first_interior"]["memtime_s"];
    let last = &evidence["interpol_rain_cases"]["last_interior"]["memtime_s"];
    assert_eq!(first[0], EXAMPLE_FIRST_MEMBER_SECONDS);
    assert_eq!(last[1], EXAMPLE_LAST_MEMBER_SECONDS);
    let duplicate = node_rows(&evidence, "final_duplicate_time");
    for row in duplicate {
        assert_eq!(row["memtime_s"][0], EXAMPLE_LAST_MEMBER_SECONDS);
        assert_eq!(row["memtime_s"][1], EXAMPLE_LAST_MEMBER_SECONDS);
        // Real-caller state at T_last: dt = 1, dt2 = 1, dtt = 1/3.
        assert_exact(number(&row, "dt1"), 0.0, "duplicate-time dt1");
        assert_exact(number(&row, "dt2"), 1.0, "duplicate-time dt2");
        assert!(within_policy(number(&row, "dtt"), 1.0 / 3.0));
    }
}

#[test]
fn test_precipitation_mapping_exact_interior_boundary_is_continuous() {
    let evidence = parse(EVIDENCE_PATH);
    let shared_member = evidence["interpol_rain_cases"]["interior"]["memtime_s"][0].clone();
    assert_eq!(
        evidence["interpol_rain_cases"]["interior_left"]["memtime_s"][1],
        shared_member
    );
    let at_member = |case: &str| -> Vec<Value> {
        node_rows(&evidence, case)
            .into_iter()
            .filter(|row| row["itime"] == shared_member)
            .collect()
    };
    let left = at_member("interior_left");
    let right = at_member("interior");
    assert!(!left.is_empty() && left.len() == right.len());
    for (left_row, right_row) in left.iter().zip(&right) {
        assert_eq!(left_row["node"], right_row["node"]);
        assert_exact(number(left_row, "dt2"), 0.0, "left pair dt2");
        assert_exact(number(right_row, "dt1"), 0.0, "right pair dt1");
        for key in ["lsp", "cp"] {
            assert!(within_policy(number(left_row, key), number(right_row, key)));
        }
    }
}
