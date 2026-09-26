//! Issue #80 checks for the frozen pristine FLEXPART W production oracle.

use serde_json::Value;
use sha2::{Digest, Sha256};

const REPORT: &str = include_str!("../fixtures/interpolation/w-production-oracle-v1.json");
const SNAPSHOT: &[u8] = include_bytes!("../fixtures/vertical/synthetic-column-v1.json");
const MOTION: &[u8] =
    include_bytes!("../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json");
const REAL_FIXTURE: &[u8] = include_bytes!("../fixtures/meteorology/era5-etex-native-v1.json");
const REFERENCE: &str = include_str!("../reference/flexpart-11.1.json");
const VERTICAL_SAMPLING_SOURCE: &str = include_str!("../src/meteorology/vertical_sampling.rs");

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn as_f64(value: &Value, context: &str) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("{context} must be a number"))
}

fn linear_sample(points: &[Value], height: f64) -> f64 {
    let first_height = as_f64(&points[0]["height_m_agl"], "first height");
    let first_value = as_f64(&points[0]["w_m_s"], "first W");
    if height <= first_height {
        return first_value;
    }
    let last = points.last().expect("non-empty vertical profile");
    let last_height = as_f64(&last["height_m_agl"], "last height");
    let last_value = as_f64(&last["w_m_s"], "last W");
    if height >= last_height {
        return last_value;
    }
    for pair in points.windows(2) {
        let lower_height = as_f64(&pair[0]["height_m_agl"], "lower height");
        let upper_height = as_f64(&pair[1]["height_m_agl"], "upper height");
        if height <= upper_height {
            let fraction = (height - lower_height) / (upper_height - lower_height);
            let lower_value = as_f64(&pair[0]["w_m_s"], "lower W");
            let upper_value = as_f64(&pair[1]["w_m_s"], "upper W");
            return lower_value + fraction * (upper_value - lower_value);
        }
    }
    panic!("bounded sample height lacked a bracket")
}

#[test]
fn test_w_production_oracle_contract_is_complete_and_not_equivalent() {
    let report: Value = serde_json::from_str(REPORT).expect("#80 report must parse");
    let reference: Value = serde_json::from_str(REFERENCE).expect("reference manifest");
    assert_eq!(report["schema"]["id"], "flexpart-gpu.w-production-oracle");
    assert_eq!(report["schema"]["version"], 1);
    assert_eq!(report["issue"], 80);
    assert_eq!(report["conclusion"], "not_equivalent");
    assert!(report["conclusion_statement"]
        .as_str()
        .expect("conclusion statement")
        .contains("differs from pristine FLEXPART"));
    assert_eq!(
        report["provenance"]["oracle"]["pinned_commit"],
        reference["pinned_commit"]
    );
    assert_eq!(report["provenance"]["oracle"]["checkout_clean"], true);

    assert_eq!(
        report["oracle_path"],
        serde_json::json!([
            "pressure velocity / interface W",
            "verttransform_mod::verttransform_ecmwf_heights",
            "verttransform_mod::verttransform_ecmwf_windfields",
            "windfields_mod::ww on height[]",
            "interpol_mod::interpol_wind (eta=no)",
            "interpol_mod::interpol_wind_meter",
            "final sampled W"
        ])
    );
    let edges = report["provenance"]["linked_flexpart"]["verified_call_edges"]
        .as_array()
        .expect("verified call edges");
    for (caller, callee) in [
        (
            "MAIN__",
            "__verttransform_mod_MOD_verttransform_ecmwf_heights",
        ),
        (
            "MAIN__",
            "__verttransform_mod_MOD_verttransform_ecmwf_windfields",
        ),
        ("MAIN__", "__interpol_mod_MOD_interpol_wind"),
        (
            "__interpol_mod_MOD_interpol_wind",
            "__interpol_mod_MOD_interpol_wind_meter",
        ),
    ] {
        assert!(edges.iter().any(|edge| {
            edge["caller"] == caller
                && edge["callee"] == callee
                && edge["verified_in_linked_executable"] == true
        }));
    }

    let synthetic = &report["synthetic_case"];
    assert_eq!(synthetic["snapshot_sha256"], sha256_hex(SNAPSHOT));
    assert_eq!(synthetic["motion_sha256"], sha256_hex(MOTION));
    let motion: Value = serde_json::from_slice(MOTION).expect("nonlinear motion fixture");
    assert_eq!(motion["profile_shape"], "deliberately_non_linear");
    let omega = motion["values"].as_array().expect("omega values");
    assert!(omega.windows(3).any(|values| {
        let second_difference = as_f64(&values[2], "omega") - 2.0 * as_f64(&values[1], "omega")
            + as_f64(&values[0], "omega");
        second_difference.abs() > 1.0e-6
    }));

    let model = synthetic["model_grid"].as_array().expect("model grid");
    let interfaces = synthetic["interface_grid"]
        .as_array()
        .expect("interface grid");
    assert!(model.windows(2).all(|pair| {
        as_f64(&pair[0]["height_m_agl"], "model height")
            < as_f64(&pair[1]["height_m_agl"], "model height")
    }));
    assert!(interfaces.windows(2).all(|pair| {
        as_f64(&pair[0]["height_m_agl"], "interface height")
            < as_f64(&pair[1]["height_m_agl"], "interface height")
    }));

    let comparisons = synthetic["comparisons"].as_array().expect("comparisons");
    assert_eq!(comparisons.len(), 5);
    assert_eq!(
        comparisons
            .iter()
            .filter(|query| query["kind"] == 1)
            .count(),
        3
    );
    assert!(comparisons
        .iter()
        .any(|query| query["classification"] == "lower_boundary"));
    assert!(comparisons
        .iter()
        .any(|query| query["classification"] == "upper_flexpart_height_boundary"));
    let mut non_equivalent_count = 0;
    for query in comparisons {
        let height = as_f64(&query["particle_height_m_agl"], "particle height");
        let pristine = as_f64(&query["pristine_w_m_s"], "pristine W");
        let direct = as_f64(&query["direct_interface_w_m_s"], "direct W");
        let tolerance = as_f64(&query["tolerance_m_s"], "tolerance");
        let expected_pristine = linear_sample(model, height);
        let expected_direct = linear_sample(interfaces, height);
        assert!((pristine - expected_pristine).abs() <= 2.0e-7);
        assert!((direct - expected_direct).abs() <= f64::EPSILON * 8.0);
        let equivalent = (pristine - direct).abs() <= tolerance;
        assert_eq!(query["equivalent"], equivalent);
        non_equivalent_count += usize::from(!equivalent);
    }
    assert!(non_equivalent_count >= 1);
    assert!(
        as_f64(
            &report["maximum_absolute_difference_m_s"],
            "maximum difference"
        ) > 0.05
    );
}

#[test]
fn test_w_production_oracle_real_data_and_issue_73_boundaries_are_explicit() {
    let report: Value = serde_json::from_str(REPORT).expect("#80 report must parse");
    let real = &report["real_data_obligation"];
    assert_eq!(real["fixture_sha256"], sha256_hex(REAL_FIXTURE));
    assert_eq!(
        real["vertical_motion_fields_present"],
        serde_json::json!([])
    );
    assert_eq!(
        real["status"],
        "not_available_in_checked_in_canonical_fixture"
    );
    assert!(real["scope_result"]
        .as_str()
        .expect("real-data scope result")
        .contains("does not add provider decoding or eta-dot preprocessing"));
    assert_eq!(
        report["handoff_to_issue_73"]["status"],
        "blocked_pending_review_and_merge_of_issue_80"
    );
    assert_eq!(
        report["handoff_to_issue_73"]["interface_vertical_motion_block_removed"],
        false
    );
    assert!(VERTICAL_SAMPLING_SOURCE.contains("InterfaceVerticalMotionBlocked"));
}
