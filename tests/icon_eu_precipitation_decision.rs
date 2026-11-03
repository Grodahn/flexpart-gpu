//! Regression lock for the #232 ICON-EU GRIB2 precipitation admission decision.
//!
//! The decision admits DWD ICON-EU run accumulations as canonical LSP/CP
//! (`RAIN_GSP + SNOW_GSP`, `RAIN_CON + SNOW_CON`, kg m-2) when the `TOT_PREC`
//! closure holds, and admits same-run CCSDS-quantization decreases only inside
//! the bound `B` of the artifact. This test re-implements the decision
//! predicate independently of `scripts/provider/icon_eu_precipitation_decision.py`
//! and checks every frozen control and genuine row, the evidence invariants the
//! verdicts rest on, and the committed raw GRIB2 octets. It decodes no CCSDS
//! data (the two-decoder proof is the Python self-test) and changes no
//! production behaviour; #75/#90/#219 still reject every ICON-EU input.
//! Reproduction and evidence: docs/research/icon-eu-precipitation-232.md.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const DECISION_PATH: &str = "fixtures/meteorology/icon-eu-precipitation-232/decision-v1.json";
const EVIDENCE_PATH: &str = "fixtures/meteorology/icon-eu-precipitation-232/evidence-v1.json";
const RAW_ROOTS: [&str; 2] = [
    "fixtures/meteorology/icon-eu-precipitation-232/raw/2026103100",
    "fixtures/meteorology/icon-eu-audit-v1/raw/2026103100",
];
/// Pinned SHA-256 of the canonical LF artifacts; update deliberately.
const DECISION_SHA256: &str = "98d6458e2afbb40b4ff871b1f17223eeeb5dabece4aff7a27fc1c38aec17135f";
const EVIDENCE_SHA256: &str = "9efe409f72f98193c64c28d78e4cb20c7f5b4c497ac88878cccffe5f605a75a5";
const COMMITTED_SERIES: [&str; 2] = ["SNOW_CON", "SNOW_GSP"];
const COMMITTED_STEPS: [u64; 6] = [0, 1, 2, 3, 47, 48];
const SAMPLE_MESSAGES: usize = 45;
/// Implementation issue that owns the #75/#90 adoption of this decision.
const IMPLEMENTATION_ISSUE: i64 = 240;
const REJECTED_SAMPLE_STEPS: [&str; 3] = ["78", "81", "84"];
const DWD_CENTRE: u64 = 78;
const PRODUCT: &str = "icon-eu_europe_regular-lat-lon";
const CCSDS_ENCODING: &str = "grib2_template_5_42_ccsds";
const CCSDS_TEMPLATE: u64 = 42;
const SOURCE_UNIT: &str = "kg m-2";
const SECONDS_PER_MINUTE: i64 = 60;
/// Largest `bitsPerValue` for which `X * 2^E` is exact in `f32`.
const F32_EXACT_BITS: u64 = 24;
const LARGE_SCALE: &str = "large_scale_precipitation";
const CONVECTIVE: &str = "convective_precipitation";
/// WMO code table 4.2-0-1 numbers of `RAIN_GSP`, `SNOW_GSP`, `RAIN_CON`, `SNOW_CON`.
const LARGE_SCALE_COMPONENTS: [u64; 2] = [56, 77];
const CONVECTIVE_COMPONENTS: [u64; 2] = [55, 76];

type Verdict = (&'static str, &'static str);

fn read_canonical(path: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .replace("\r\n", "\n")
}

/// Prefix marking a JSON number kept as its source text by [`parse`].
const NUMBER_PREFIX: char = '#';

/// Parses an artifact with every JSON number kept as its source text.
///
/// The crate's `serde_json` (no `float_roundtrip` feature) may round a decimal
/// one ulp away; bounds and canonical values are compared bit for bit, so
/// numbers are re-read with the correctly rounded `str::parse` (as in the #216 lock).
fn parse(path: &str) -> Value {
    let source = read_canonical(path);
    let mut quoted = String::with_capacity(source.len() * 2);
    let mut chars = source.chars().peekable();
    let (mut in_string, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if in_string {
            in_string = escaped || c != '"';
            escaped = !escaped && c == '\\';
            quoted.push(c);
        } else if c == '-' || c.is_ascii_digit() {
            quoted.push('"');
            quoted.push(NUMBER_PREFIX);
            quoted.push(c);
            while let Some(next) =
                chars.next_if(|n| n.is_ascii_alphanumeric() || "+-.".contains(*n))
            {
                quoted.push(next);
            }
            quoted.push('"');
        } else {
            in_string = c == '"';
            quoted.push(c);
        }
    }
    serde_json::from_str(&quoted).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn number_text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .and_then(|token| token.strip_prefix(NUMBER_PREFIX))
}

fn opt_int(value: &Value) -> Option<i64> {
    number_text(value).map(|token| token.parse().expect("integer"))
}

fn opt_real(value: &Value) -> Option<f64> {
    number_text(value).map(|token| token.parse().expect("number"))
}

fn uint(value: &Value, key: &str) -> u64 {
    number_text(&value[key])
        .and_then(|token| token.parse().ok())
        .unwrap_or_else(|| panic!("missing unsigned integer `{key}` in {value}"))
}

fn int(value: &Value, key: &str) -> i64 {
    opt_int(&value[key]).unwrap_or_else(|| panic!("missing integer `{key}` in {value}"))
}

fn float(value: &Value, key: &str) -> f64 {
    opt_real(&value[key]).unwrap_or_else(|| panic!("missing number `{key}` in {value}"))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    let token = value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string `{key}` in {value}"));
    assert!(!token.starts_with(NUMBER_PREFIX), "`{key}` is a number");
    token
}

/// Exact equality of two optional bounds (`None` for rules without a bound).
fn same_bound(actual: Option<f64>, expected: Option<f64>) -> bool {
    actual.map(f64::to_bits) == expected.map(f64::to_bits)
}

/// Canonical value of an observation; the artifact encodes NaN as the string "NaN".
fn canonical_value(observation: &Value) -> Option<f64> {
    match &observation["canonical_value"] {
        Value::Null => None,
        Value::String(token) if token == "NaN" => Some(f64::NAN),
        other => Some(opt_real(other).expect("canonical value")),
    }
}

/// Level spacing `2^E * 10^-D` in kg m-2; the run-start anchor (0 bits) is exact.
fn quantum(record: &Value) -> f64 {
    if uint(record, "bitsPerValue") == 0 {
        return 0.0;
    }
    let binary = i32::try_from(int(record, "binaryScaleFactor")).expect("E range");
    let decimal = i32::try_from(int(record, "decimalScaleFactor")).expect("D range");
    2.0_f64.powi(binary) / 10.0_f64.powi(decimal)
}

fn packing_keys(record: &Value) -> (u64, i64, i64) {
    (
        float(record, "referenceValue").to_bits(),
        int(record, "binaryScaleFactor"),
        int(record, "decimalScaleFactor"),
    )
}

/// Spacing from a non-negative `f32` value to the next larger `f32`.
fn ulp_f32(value: f64) -> f64 {
    #[allow(clippy::cast_possible_truncation)]
    let level = value as f32;
    assert_eq!(
        f64::from(level).to_bits(),
        value.to_bits(),
        "canonical values are f32"
    );
    f64::from(f32::from_bits(level.to_bits() + 1)) - value
}

fn admitted_packing(record: &Value) -> bool {
    if text(record, "encoding") != CCSDS_ENCODING || record["bitmap_present"] != Value::Bool(false)
    {
        return false;
    }
    if int(record, "decimalScaleFactor") != 0 || float(record, "referenceValue") != 0.0 {
        return false;
    }
    match uint(record, "bitsPerValue") {
        0 => uint(record, "lengthOfTimeRange") == 0,
        bits => bits <= F32_EXACT_BITS,
    }
}

fn expected_components(field: &str) -> Option<[u64; 2]> {
    match field {
        LARGE_SCALE => Some(LARGE_SCALE_COMPONENTS),
        CONVECTIVE => Some(CONVECTIVE_COMPONENTS),
        _ => None,
    }
}

/// Independent re-implementation of the decision's field admission.
fn classify_field(field: &str, observation: &Value) -> Verdict {
    let Some(expected) = expected_components(field) else {
        return ("REJECT", "unsupported_field");
    };
    let Some(records) = observation["components"]
        .as_array()
        .filter(|records| !records.is_empty())
    else {
        return ("UNRESOLVED", "missing_packing_provenance");
    };
    let mut found: Vec<(u64, u64, u64)> = records
        .iter()
        .map(|r| {
            (
                uint(r, "discipline"),
                uint(r, "parameterCategory"),
                uint(r, "parameterNumber"),
            )
        })
        .collect();
    found.sort_unstable();
    if found != expected.map(|number| (0, 1, number)) {
        return ("REJECT", "component_set_mismatch");
    }
    let hashes: BTreeSet<&str> = records.iter().map(|r| text(r, "message_sha256")).collect();
    if hashes.len() != records.len() {
        return ("REJECT", "packing_provenance_mismatch");
    }
    if records
        .iter()
        .any(|r| uint(r, "centre") != DWD_CENTRE || text(r, "product") != PRODUCT)
    {
        return ("UNRESOLVED", "unsupported_source_class");
    }
    if !records.iter().all(admitted_packing) {
        return ("UNRESOLVED", "unsupported_packing");
    }
    if records
        .iter()
        .any(|r| text(r, "source_unit") != SOURCE_UNIT)
    {
        return ("UNRESOLVED", "unsupported_conversion");
    }
    let accumulation_from_run_start = records.iter().all(|r| {
        uint(r, "productDefinitionTemplateNumber") == 8
            && uint(r, "typeOfStatisticalProcessing") == 1
            && uint(r, "numberOfTimeRanges") == 1
            && uint(r, "indicatorOfUnitOfTimeRange") == 0
            && uint(r, "forecastTime") == 0
    });
    if !accumulation_from_run_start {
        return ("REJECT", "statistical_processing_mismatch");
    }
    let runs: BTreeSet<(i64, i64)> = records
        .iter()
        .map(|r| (int(r, "forecast_reference_s"), int(r, "lengthOfTimeRange")))
        .collect();
    let [(reference, length_minutes)] = runs.into_iter().collect::<Vec<_>>()[..] else {
        return ("REJECT", "run_identity_mismatch");
    };
    if opt_int(&observation["reset_s"]) != Some(reference)
        || opt_int(&observation["valid_time_s"])
            != Some(reference + SECONDS_PER_MINUTE * length_minutes)
    {
        return ("REJECT", "run_identity_mismatch");
    }
    match observation["closure_verified"].as_bool() {
        None => ("UNRESOLVED", "missing_closure_reference"),
        Some(false) => ("REJECT", "component_closure_violation"),
        Some(true) => ("ADMIT", "admitted_component_sum"),
    }
}

/// Source part and full bound `B` of a pair (components with identical keys add 0).
fn admission_bound(previous: &Value, current: &Value, values: (f64, f64)) -> (f64, f64) {
    let current_records = current["components"].as_array().expect("components");
    let mut source = 0.0;
    for before in previous["components"].as_array().expect("components") {
        let number = uint(before, "parameterNumber");
        let after = current_records
            .iter()
            .find(|r| uint(r, "parameterNumber") == number)
            .expect("matching component");
        if packing_keys(before) != packing_keys(after) {
            source += quantum(before) + quantum(after);
        }
    }
    (
        source,
        source + ulp_f32(values.0) / 2.0 + ulp_f32(values.1) / 2.0,
    )
}

/// Independent re-implementation of the decision's same-run pair rule.
fn classify_pair(field: &str, previous: &Value, current: &Value) -> (Verdict, Option<f64>) {
    if expected_components(field).is_none() {
        return (("REJECT", "unsupported_field"), None);
    }
    let (Some(before), Some(after)) = (canonical_value(previous), canonical_value(current)) else {
        return (("REJECT", "missing_value"), None);
    };
    if !before.is_finite() || !after.is_finite() {
        return (("REJECT", "non_finite_value"), None);
    }
    if before < 0.0 || after < 0.0 {
        return (("REJECT", "negative_accumulated_amount"), None);
    }
    let times = [
        opt_int(&previous["valid_time_s"]),
        opt_int(&current["valid_time_s"]),
        opt_int(&previous["reset_s"]),
        opt_int(&current["reset_s"]),
    ];
    let [Some(valid_before), Some(valid_after), Some(reset_before), Some(reset_after)] = times
    else {
        return (("REJECT", "missing_time_metadata"), None);
    };
    if valid_after <= valid_before {
        return (("REJECT", "non_increasing_valid_time"), None);
    }
    if reset_before != reset_after {
        return (("REJECT", "reset_mismatch"), None);
    }
    for observation in [previous, current] {
        let verdict = classify_field(field, observation);
        if verdict.0 != "ADMIT" {
            return (verdict, None);
        }
    }
    if after >= before {
        return (("OUTSIDE_RULE", "ordinary_same_run_interval"), None);
    }
    let (source, bound) = admission_bound(previous, current, (before, after));
    if source == 0.0 {
        return (("REJECT", "identical_packing_parameters"), None);
    }
    if before - after < bound {
        (
            ("ADMIT_BOUNDED", "quantization_compatible_decrease"),
            Some(bound),
        )
    } else {
        (
            ("REJECT", "decrease_exceeds_quantization_bound"),
            Some(bound),
        )
    }
}

fn messages(evidence: &Value) -> &Vec<Value> {
    evidence["messages"].as_array().expect("messages")
}

fn evidence_message<'a>(evidence: &'a Value, variable: &str, step: u64) -> &'a Value {
    messages(evidence)
        .iter()
        .find(|m| text(m, "variable") == variable && uint(m, "step") == step)
        .unwrap_or_else(|| panic!("no evidence for {variable} step {step}"))
}

fn committed_bytes(variable: &str, step: u64) -> Vec<u8> {
    let name = format!("{PRODUCT}_single-level_2026103100_{step:03}_{variable}.grib2");
    RAW_ROOTS
        .iter()
        .map(|root| format!("{root}/{name}"))
        .find_map(|path| std::fs::read(path).ok())
        .unwrap_or_else(|| panic!("{name} is not committed"))
}

/// Section octets keyed by section number (WMO octet n is index n-1).
fn grib2_sections(message: &[u8]) -> Vec<(u8, &[u8])> {
    assert_eq!(&message[..4], b"GRIB", "GRIB indicator");
    assert_eq!(message[7], 2, "GRIB edition 2");
    assert_eq!(&message[message.len() - 4..], b"7777", "end section");
    let mut found = Vec::new();
    let mut offset = 16;
    while offset < message.len() - 4 {
        let length = u32::from_be_bytes(message[offset..offset + 4].try_into().expect("length"));
        let length = usize::try_from(length).expect("section length");
        found.push((message[offset + 4], &message[offset..offset + length]));
        offset += length;
    }
    assert_eq!(offset, message.len() - 4, "section lengths end at 7777");
    found
}

fn section(message: &[u8], number: u8) -> &[u8] {
    grib2_sections(message)
        .into_iter()
        .find_map(|(found, octets)| (found == number).then_some(octets))
        .unwrap_or_else(|| panic!("missing section {number}"))
}

/// WMO sign-and-magnitude 16-bit integer.
fn sign_magnitude(octets: [u8; 2]) -> i64 {
    let raw = i64::from(u16::from_be_bytes(octets));
    if raw & 0x8000 == 0 {
        raw
    } else {
        -(raw & 0x7fff)
    }
}

fn be_u32(octets: &[u8]) -> u64 {
    u64::from(u32::from_be_bytes(octets.try_into().expect("four octets")))
}

#[test]
fn test_icon_precip_artifacts_are_pinned() {
    let decision = sha256_hex(read_canonical(DECISION_PATH).as_bytes());
    let evidence = sha256_hex(read_canonical(EVIDENCE_PATH).as_bytes());
    assert_eq!(
        decision, DECISION_SHA256,
        "decision changed without updating the pin"
    );
    assert_eq!(
        evidence, EVIDENCE_SHA256,
        "evidence changed without updating the pin"
    );
    assert_eq!(text(&parse(DECISION_PATH), "evidence_sha256"), evidence);
}

#[test]
fn test_icon_precip_verdicts_answer_every_question() {
    let decision = parse(DECISION_PATH);
    let questions = &decision["questions"];
    assert_eq!(
        text(&questions["units"], "verdict"),
        "AMOUNT_KG_PER_SQUARE_METRE"
    );
    assert_eq!(
        text(&questions["composition"], "verdict"),
        "ADMIT_COMPONENT_SUM_WITH_CLOSURE"
    );
    assert_eq!(text(&questions["quantization"], "verdict"), "ADMIT_BOUNDED");
    let classes = questions["fail_closed_classes"]
        .as_array()
        .expect("classes");
    for verdict in ["ADMIT", "REJECT", "UNRESOLVED"] {
        assert!(
            classes
                .iter()
                .any(|class| text(class, "verdict").starts_with(verdict)),
            "no {verdict} class"
        );
    }
    assert_eq!(
        opt_int(&decision["handoff"]["implementation_issue"]),
        Some(IMPLEMENTATION_ISSUE),
        "the #75/#90 adoption must name its implementation issue"
    );
}

#[test]
fn test_icon_precip_independent_predicate_reproduces_controls() {
    let decision = parse(DECISION_PATH);
    let controls = decision["controls"].as_array().expect("controls");
    let mut codes = BTreeSet::new();
    for control in controls {
        let (verdict, bound) = classify_pair(
            text(control, "field"),
            &control["previous"],
            &control["current"],
        );
        assert_eq!(
            verdict,
            (
                text(control, "expected_verdict"),
                text(control, "expected_code")
            ),
            "control `{}`",
            text(control, "name")
        );
        assert!(
            same_bound(bound, opt_real(&control["bound_kg_m2"])),
            "bound of `{}`",
            text(control, "name")
        );
        codes.insert(verdict.1);
    }
    assert_eq!(codes.len(), 21, "every rule outcome has a control");
}

#[test]
fn test_icon_precip_genuine_decreases_are_admitted_bounded() {
    let decision = parse(DECISION_PATH);
    let rows = decision["genuine_rows"].as_array().expect("rows");
    let fields: BTreeSet<&str> = rows.iter().map(|row| text(row, "field")).collect();
    assert_eq!(fields, BTreeSet::from([LARGE_SCALE, CONVECTIVE]));
    for row in rows {
        let (verdict, bound) = classify_pair(text(row, "field"), &row["previous"], &row["current"]);
        assert_eq!(
            verdict,
            ("ADMIT_BOUNDED", "quantization_compatible_decrease"),
            "{}",
            text(row, "name")
        );
        assert!(same_bound(bound, opt_real(&row["bound_kg_m2"])));
    }
}

#[test]
fn test_icon_precip_evidence_supports_bound_and_closure() {
    let evidence = parse(EVIDENCE_PATH);
    assert_eq!(messages(&evidence).len(), SAMPLE_MESSAGES);
    for message in messages(&evidence) {
        let packing = &message["packing"];
        assert_eq!(message["decoders_bit_identical"], Value::Bool(true));
        assert_eq!(
            message["values_sha256_matches_230_evidence"],
            Value::Bool(true)
        );
        assert!(
            admitted_packing(packing),
            "packing outside class: {packing}"
        );
        let anchor = uint(message, "step") == 0;
        assert_eq!(uint(packing, "bitsPerValue") == 0, anchor);
        assert_eq!(
            uint(packing, "lengthOfTimeRange"),
            uint(message, "step") * 60
        );
    }
    for pair in evidence["component_pairs"].as_array().expect("pairs") {
        let decreases = uint(pair, "decreasing_cells");
        if pair["identical_packing"] == Value::Bool(true) {
            assert_eq!(decreases, 0, "identical keys cannot decrease: {pair}");
        } else if decreases > 0 {
            let most_negative = -float(pair, "most_negative_delta_kg_m2");
            let (previous, current) = (
                float(pair, "previous_quantum_kg_m2"),
                float(pair, "current_quantum_kg_m2"),
            );
            assert!(most_negative < previous + current, "{pair}");
            assert_eq!(
                most_negative.to_bits(),
                (previous.max(current) / 2.0).to_bits(),
                "{pair}"
            );
        }
    }
    for pair in evidence["canonical_pairs"].as_array().expect("pairs") {
        assert_eq!(uint(pair, "violating_cells"), 0, "{pair}");
        assert!(float(pair, "max_decrease_to_bound_ratio") < 1.0, "{pair}");
    }
    let decision = parse(DECISION_PATH);
    let admission = &decision["sample_step_admission"];
    let mut rejected = Vec::new();
    for closure in evidence["closure"].as_array().expect("closure") {
        let beyond = uint(closure, "cells_beyond_bound");
        assert_eq!(uint(closure, "cells_beyond_bound_positive"), 0);
        let step = uint(closure, "step").to_string();
        let expected = if beyond == 0 { "ADMIT" } else { "REJECT" };
        assert_eq!(text(admission, &step), expected, "step {step}");
        if beyond > 0 {
            rejected.push(step);
        }
    }
    assert_eq!(rejected, REJECTED_SAMPLE_STEPS);
}

#[test]
fn test_icon_precip_committed_raw_octets_match_evidence() {
    let evidence = parse(EVIDENCE_PATH);
    for variable in COMMITTED_SERIES {
        for step in COMMITTED_STEPS {
            let message = committed_bytes(variable, step);
            let packing = &evidence_message(&evidence, variable, step)["packing"];
            assert_eq!(sha256_hex(&message), text(packing, "message_sha256"));
            assert_eq!(u64::from(message[6]), uint(packing, "discipline"));
            let product = section(&message, 4);
            assert_eq!(
                u64::from(u16::from_be_bytes([product[7], product[8]])),
                uint(packing, "productDefinitionTemplateNumber")
            );
            assert_eq!(u64::from(product[9]), uint(packing, "parameterCategory"));
            assert_eq!(u64::from(product[10]), uint(packing, "parameterNumber"));
            assert_eq!(be_u32(&product[18..22]), uint(packing, "forecastTime"));
            assert_eq!(
                u64::from(product[46]),
                uint(packing, "typeOfStatisticalProcessing")
            );
            assert_eq!(be_u32(&product[49..53]), uint(packing, "lengthOfTimeRange"));
            let representation = section(&message, 5);
            assert_eq!(
                u64::from(u16::from_be_bytes([representation[9], representation[10]])),
                CCSDS_TEMPLATE
            );
            let reference = f32::from_be_bytes(representation[11..15].try_into().expect("R"));
            assert_eq!(
                f64::from(reference).to_bits(),
                float(packing, "referenceValue").to_bits()
            );
            assert_eq!(
                sign_magnitude([representation[15], representation[16]]),
                int(packing, "binaryScaleFactor")
            );
            assert_eq!(
                sign_magnitude([representation[17], representation[18]]),
                int(packing, "decimalScaleFactor")
            );
            assert_eq!(u64::from(representation[19]), uint(packing, "bitsPerValue"));
            assert_eq!(section(&message, 6)[5], 255, "no bitmap");
        }
    }
}
