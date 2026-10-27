//! Regression lock for the #211 GRIB2 convective-precipitation units decision.
//!
//! The decision artifact records why the pinned FLEXPART 11.1 GRIB2 CP divisor
//! of 1000 is correct for `flex_extract` 7.1.2 output converted by ecCodes 2.24.2,
//! and which GRIB2 classes #32 must reject. This test does not rerun the
//! original producer or reader; it fails closed when the committed decision,
//! its decoded evidence, the pinned references or the #71 tolerance policy
//! drift apart, and it re-checks the decoded producer/reader rows offline.
//! Reproduction: docs/research/grib2-cp-units-211.md.

use flexpart_gpu::meteorology::temporal::{DEFAULT_ABS_TOL, DEFAULT_REL_TOL};
use serde_json::Value;
use sha2::{Digest, Sha256};

const DECISION_PATH: &str = "fixtures/accumulation/grib2-cp-units-v1.json";
const EVIDENCE_PATH: &str = "fixtures/accumulation/grib2-cp-units-evidence-v1.json";
/// Pinned SHA-256 of the canonical LF decision artifact; update deliberately.
const DECISION_SHA256: &str = "127be2dec076d3354805c661e31c4d4f63a938dcb0f94ecbbe917fce7c6e6251";
/// Evidence conversion of the accepted GRIB2 class (ecCodes 2.24.2 producer).
const ACCEPTED_GRIB2_CONVERSION: &str = "GRIB2";
/// Evidence conversion of the ecCodes 2.50.0 producer variant.
const VARIANT_CONVERSION: &str = "prepared-grib2-eccodes-2.50.0";
/// ecCodes 2.24.2 `unitsFactor` of the paramId 143 GRIB2 concept, which the
/// pinned reader divisor (`windfields_mod.f90:2147`) compensates.
const ECCODES_CP_UNITS_FACTOR: f64 = 1000.0;
/// Relative parse error bound, in units of `f64::EPSILON`, of `serde_json`'s
/// default float parser, which is not guaranteed to round correctly.
const SERDE_PARSE_ULPS: f64 = 1.0;
/// `gridcheck_ecmwf` stop when no precipitation message is recognised.
const READER_STOP_LOCATION: &str = "windfields_mod.f90:760";

const EXPECTED_CLASSES: [(&str, &str); 7] = [
    ("fe712-grib1", "CORRECT_AS_PREPARED"),
    ("fe712-grib2-eccodes-2.24.2", "CORRECT_AS_PREPARED"),
    (
        "fe712-grib2-eccodes-2.50.0-reader-eccodes-2.24.2",
        "UNRESOLVED",
    ),
    (
        "fe712-grib2-eccodes-ge-2.31-reader-eccodes-ge-2.31",
        "UNRESOLVED",
    ),
    ("fe712-grib2-eccodes-2.25-to-2.30", "UNRESOLVED"),
    ("non-flex-extract-grib2", "UNRESOLVED"),
    ("fe712-grib2-rrint1-numpf3", "UNRESOLVED"),
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

/// Asserts an exactly representable decoded value, such as a zero rate or an
/// unchanged LSP value, without a tolerance.
#[allow(clippy::float_cmp)]
fn assert_exact(actual: f64, expected: f64, what: &str) {
    assert_eq!(actual, expected, "{what}");
}

fn number(value: &Value, key: &str) -> f64 {
    value[key]
        .as_f64()
        .unwrap_or_else(|| panic!("missing number {key}"))
}

fn integer(value: &Value, key: &str) -> i64 {
    value[key]
        .as_i64()
        .unwrap_or_else(|| panic!("missing integer {key}"))
}

fn pinned_commit(manifest: &str) -> String {
    parse(manifest)["pinned_commit"]
        .as_str()
        .expect("manifest pinned_commit")
        .to_owned()
}

fn conversion<'a>(evidence: &'a Value, label: &str) -> &'a Value {
    let found = &evidence["conversions"][label];
    assert!(found.is_object(), "missing evidence conversion {label}");
    found
}

fn rows<'a>(evidence: &'a Value, label: &str, field: &str) -> Vec<&'a Value> {
    let selected: Vec<&Value> = conversion(evidence, label)["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter(|row| row["field"] == field)
        .collect();
    assert!(!selected.is_empty(), "{label}: no {field} rows");
    selected
}

fn cases<'a>(evidence: &'a Value, label: &str) -> Vec<&'a Value> {
    conversion(evidence, label)["cases"]
        .as_object()
        .expect("cases")
        .values()
        .collect()
}

/// Rebuilds the decoded value from the simple-packing integer and scale keys,
/// `(R + X * 2^E) / 10^D`. Positive powers keep the scaling exact; `powi`
/// with a negative exponent is not correctly rounded.
fn unpacked(packed: i64, metadata: &Value) -> f64 {
    let binary = i32::try_from(integer(metadata, "binaryScaleFactor")).expect("binary scale");
    let decimal = i32::try_from(integer(metadata, "decimalScaleFactor")).expect("decimal scale");
    #[allow(clippy::cast_precision_loss)]
    let packed = packed as f64;
    let scaled = if binary < 0 {
        packed / 2f64.powi(-binary)
    } else {
        packed * 2f64.powi(binary)
    };
    let reference_plus_scaled = number(metadata, "referenceValue") + scaled;
    if decimal < 0 {
        reference_plus_scaled * 10f64.powi(-decimal)
    } else {
        reference_plus_scaled / 10f64.powi(decimal)
    }
}

#[test]
fn test_grib2_cp_units_decision_digest_is_pinned() {
    assert_eq!(
        sha256_hex(&read_canonical(DECISION_PATH)),
        DECISION_SHA256,
        "decision artifact changed; review and update DECISION_SHA256 deliberately"
    );
}

#[test]
fn test_grib2_cp_units_evidence_digest_matches_decision() {
    let decision = parse(DECISION_PATH);
    assert_eq!(decision["evidence"], EVIDENCE_PATH);
    assert_eq!(
        decision["evidence_sha256"]
            .as_str()
            .expect("evidence_sha256"),
        sha256_hex(&read_canonical(EVIDENCE_PATH))
    );
}

#[test]
fn test_grib2_cp_units_pins_match_reference_manifests() {
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
        evidence["producer"]["flex_extract"]["pinned_commit"],
        flex_extract.as_str()
    );
    for checkout in evidence["reader_checkout"]
        .as_array()
        .expect("reader_checkout")
    {
        assert_eq!(
            checkout[0],
            flexpart.as_str(),
            "reader checkout before/after"
        );
    }
    assert_eq!(evidence["producer"]["runtime"]["eccodes"], "2.24.2");
}

#[test]
fn test_grib2_cp_units_tolerance_matches_71_policy() {
    let evidence = parse(EVIDENCE_PATH);
    let policy = &evidence["tolerance_policy"];
    assert_exact(
        number(policy, "absolute"),
        DEFAULT_ABS_TOL,
        "absolute tolerance",
    );
    assert_exact(
        number(policy, "relative"),
        DEFAULT_REL_TOL,
        "relative tolerance",
    );
}

#[test]
fn test_grib2_cp_units_input_classes_are_enumerated() {
    let decision = parse(DECISION_PATH);
    let classes = decision["input_classes"].as_array().expect("input_classes");
    let found: Vec<(&str, &str)> = classes
        .iter()
        .map(|class| {
            (
                class["id"].as_str().expect("class id"),
                class["verdict"].as_str().expect("class verdict"),
            )
        })
        .collect();
    assert_eq!(found, EXPECTED_CLASSES);
    let accepted = decision["handoff_32"]["accepted_provider_identities"]
        .as_array()
        .expect("accepted identities");
    let correct = EXPECTED_CLASSES
        .iter()
        .filter(|(_, verdict)| *verdict == "CORRECT_AS_PREPARED");
    assert_eq!(accepted.len(), correct.clone().count());
    for ((id, _), identity) in correct.zip(accepted) {
        assert!(identity.as_str().expect("identity").starts_with(id));
    }
}

#[test]
fn test_grib2_cp_units_accepted_message_identity() {
    let decision = parse(DECISION_PATH);
    let evidence = parse(EVIDENCE_PATH);
    let required = &decision["handoff_32"]["required_grib2_keys"];
    for case in cases(&evidence, ACCEPTED_GRIB2_CONVERSION) {
        assert_eq!(case["reader_grib2_outcome"], "STORED");
        for (param, field) in [("143", "cp"), ("142", "lsp")] {
            let metadata = &case["grib2_metadata"][param];
            for key in [
                "discipline",
                "parameterCategory",
                "parameterNumber",
                "productDefinitionTemplateNumber",
                "typeOfStatisticalProcessing",
                "typeOfFirstFixedSurface",
                "stepRange",
            ] {
                assert_eq!(metadata[key], required[field][key], "{field} {key}");
            }
            assert_eq!(
                metadata["paramId"],
                required[field]["paramId_decoded_by_ecCodes_2.24.2"]
            );
            assert_eq!(
                integer(metadata, "decimalScaleFactor"),
                0,
                "{field} decimal scale"
            );
        }
        assert_eq!(case["grib2_metadata"]["143"]["units"], "kg m**-2");
        assert_eq!(case["grib2_metadata"]["142"]["units"], "m");
        let diagnostics = case["reader_grib2_log_diagnostics"].to_string();
        assert!(
            diagnostics.contains("parId 3063 is6 143"),
            "reader saw CP as 3063"
        );
    }
}

#[test]
fn test_grib2_cp_units_divisor_compensates_eccodes_scaling() {
    let evidence = parse(EVIDENCE_PATH);
    let cp = rows(&evidence, ACCEPTED_GRIB2_CONVERSION, "cp");
    let mut zero = 0;
    let mut scaled = 0;
    for row in &cp {
        let declared = number(row, "declared_mm_per_h");
        let stored = number(row, "reader_grib2_mm_per_h");
        if row["zero"].as_bool().expect("zero flag") {
            zero += 1;
            assert_exact(number(row, "grib2_decoded"), 0.0, "zero CP decoded");
            assert_exact(stored, 0.0, "zero CP stored");
            continue;
        }
        assert!(within_policy(
            number(row, "grib2_decoded"),
            ECCODES_CP_UNITS_FACTOR * declared
        ));
        assert!(within_policy(stored, declared), "GRIB2 CP stored in mm/h");
        assert!(within_policy(stored, number(row, "reader_grib1_mm_per_h")));
        if row.get("grib2_decoded_power_of_1000").is_some() {
            scaled += 1;
            assert_eq!(integer(row, "grib2_decoded_power_of_1000"), 1);
            assert_eq!(integer(row, "reader_grib2_power_of_1000"), 0);
        }
    }
    assert!(
        zero > 0 && scaled > 0,
        "zero and factor-exposing CP rows required"
    );
}

#[test]
fn test_grib2_cp_units_lsp_control_is_unscaled() {
    let evidence = parse(EVIDENCE_PATH);
    let lsp = rows(&evidence, ACCEPTED_GRIB2_CONVERSION, "lsp");
    assert!(lsp.iter().any(|row| number(row, "declared_mm_per_h") > 0.0));
    for row in lsp {
        assert_exact(
            number(row, "grib2_decoded"),
            number(row, "grib1_decoded"),
            "LSP GRIB2 decode",
        );
        assert_exact(
            number(row, "reader_grib2_mm_per_h"),
            number(row, "reader_grib1_mm_per_h"),
            "LSP reader store",
        );
        assert!(within_policy(
            number(row, "reader_grib1_mm_per_h"),
            number(row, "declared_mm_per_h")
        ));
    }
}

#[test]
fn test_grib2_cp_units_packed_integers_rebuild_decoded_values() {
    let evidence = parse(EVIDENCE_PATH);
    for case in cases(&evidence, ACCEPTED_GRIB2_CONVERSION) {
        let stamp = case["member_stamp"].as_str().expect("stamp");
        for (param, field) in [("143", "cp"), ("142", "lsp")] {
            for row in rows(&evidence, ACCEPTED_GRIB2_CONVERSION, field) {
                if !row["case"].as_str().expect("case").ends_with(stamp) {
                    continue;
                }
                for (edition, prefix) in [("grib1_metadata", "grib1"), ("grib2_metadata", "grib2")]
                {
                    let packed = integer(row, &format!("{prefix}_packed"));
                    let rebuilt = unpacked(packed, &case[edition][param]);
                    let decoded = number(row, &format!("{prefix}_decoded"));
                    // The trace already proved exact equality; this allows only the
                    // one-ulp error of serde_json's default (not correctly rounded) parser.
                    assert!(
                        (rebuilt - decoded).abs()
                            <= SERDE_PARSE_ULPS * f64::EPSILON * decoded.abs(),
                        "simple-packing reconstruction: {rebuilt} vs {decoded}"
                    );
                }
            }
        }
    }
}

#[test]
fn test_grib2_cp_units_newer_eccodes_is_rejected_by_reader() {
    let evidence = parse(EVIDENCE_PATH);
    let variant = conversion(&evidence, VARIANT_CONVERSION);
    assert_eq!(variant["producer_eccodes"], "2.50.0");
    assert_eq!(variant["reader_outcome"], "STOP");
    assert!(variant["rows"].as_array().expect("rows").is_empty());
    for case in cases(&evidence, VARIANT_CONVERSION) {
        assert_eq!(integer(case, "reader_grib2_exit_status"), 1);
        assert!(case["reader_grib2_log_diagnostics"]
            .to_string()
            .contains(READER_STOP_LOCATION));
        for (param, number_expected) in [("143", 195), ("142", 196)] {
            let metadata = &case["grib2_metadata"][param];
            assert_eq!(integer(metadata, "discipline"), 0);
            assert_eq!(integer(metadata, "parameterCategory"), 1);
            assert_eq!(integer(metadata, "parameterNumber"), number_expected);
            assert_eq!(metadata["units"], "m");
        }
    }
}
