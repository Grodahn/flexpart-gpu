//! Regression lock for the #216 GRIB-packing accumulation-decrease decision.
//!
//! The decision artifact classifies the three same-run accumulation decreases
//! of the genuine #202 GRIB example and fixes the bounded admission policy that
//! a later #75/#90 change must implement. This test re-derives that evidence
//! offline from the committed GRIB1 bytes with its own decoder, re-applies the
//! policy independently with exact dyadic arithmetic, and checks that the
//! current #75 validation still rejects the rows. It neither modifies nor
//! bypasses #75/#90 runtime validation and runs no GPU work.
//! Reproduction: docs/research/grib-packing-accumulation-216.md.

use std::collections::{BTreeMap, BTreeSet};

use flexpart_gpu::meteorology::accumulation::{
    validate_interval_sequence, AccumulatedObservation, AccumulationError,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const DECISION_PATH: &str = "fixtures/accumulation/grib-packing-decrease-decision-v1.json";
const ECCODES_KEYS_PATH: &str = "fixtures/accumulation/grib-packing-eccodes-keys-v1.json";
const MAPPING_DECISION_PATH: &str = "fixtures/accumulation/precipitation-member-mapping-v1.json";
const EVIDENCE_PATH: &str = "fixtures/accumulation/precipitation-member-mapping-evidence-v1.json";
const ORACLE_PATH: &str = "fixtures/accumulation/precipitation-darain-oracle-v1.json";
const GRIB_DIR: &str = "fixtures/accumulation/precipitation-202-accumulated-grib";
/// Committed as `<#202 trace name>.grib` (repository binary-fixture policy); the
/// #202 evidence and the decision key the files by the original trace name.
const GRIB_SUFFIX: &str = ".grib";
/// Pinned SHA-256 of the canonical LF decision artifact; update deliberately.
const DECISION_SHA256: &str = "83059dcbce88544aa141ffcb9fb8e2ec96cc3ec2eca9ef76873e9b84bf3ddfa1";
/// `flex_extract` divides GRIB metres by `fak = 1/1000` (`EcFlexpart.py:1123-1131`);
/// the #207 harness builds canonical kg/m2 with the same quotient.
const FLEX_EXTRACT_METRE_FACTOR: f64 = 1.0 / 1000.0;
/// `flex_extract` `DTIME` of the #202 example in hours.
const DTIME_HOURS: f64 = 3.0;
/// Canonical kg/m2 per GRIB metre of water equivalent.
const CANONICAL_PER_SOURCE_UNIT: i128 = 1000;
/// Conversion path of the supported class (decision `supported_class`).
const CANONICAL_CONVERSION: &str = "f64_decode_unit_scale_then_f32_nearest";
/// Relative f64 decode/unit-conversion term of the canonical bound, as `2^-50`.
const F64_PATH_SHIFT: u32 = 50;
/// #202 example grid: 3 x 3 nodes, canonical cell = j * NX + i with j = 0 south.
const NX: usize = 3;
const NY: usize = 3;
/// Bound arithmetic is exact on dyadic rationals `n * 2^-DYADIC_SCALE`.
const DYADIC_SCALE: i32 = 120;
const SUPPORTED_FIELDS: [&str; 2] = ["large_scale_precipitation", "convective_precipitation"];
/// Negative-control classes the #216 decision must cover.
const REQUIRED_CONTROLS: [&str; 18] = [
    "unsupported_source_class_ea",
    "unsupported_source_class_other",
    "missing_source_class_provenance",
    "bound_smallest_rejected",
    "decrease_twice_bound",
    "undeclared_reset_consistent_provenance",
    "identical_packing_parameters",
    "missing_packing_provenance",
    "packing_provenance_for_other_message",
    "provenance_param_mismatch",
    "reversed_timestamps",
    "missing_timestamp",
    "mid_window_reset",
    "mixed_grid_identity",
    "mixed_source_identity",
    "missing_accumulated_observation",
    "wrong_source_unit",
    "non_finite_nan",
];

fn read_canonical(path: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .replace("\r\n", "\n")
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Prefix marking a JSON number kept as its source text by [`parse`].
const NUMBER_PREFIX: char = '#';

/// Parses a fixture with every JSON number kept as its source text.
///
/// The crate's `serde_json` (no `float_roundtrip` feature) may round a decimal
/// one ulp away; this lock compares Python-written f64 values bit for bit, so
/// numbers are re-read with the correctly rounded `str::parse` instead.
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

fn number_text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .and_then(|token| token.strip_prefix(NUMBER_PREFIX))
}

fn opt_integer(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        other => number_text(other).map(|n| n.parse().expect("integer")),
    }
}

fn opt_real(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        other => number_text(other).map(|n| n.parse().expect("number")),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    let token = value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string {key}"));
    assert!(!token.starts_with(NUMBER_PREFIX), "{key} is a number");
    token
}

fn integer(value: &Value, key: &str) -> i64 {
    opt_integer(&value[key]).unwrap_or_else(|| panic!("missing integer {key}"))
}

fn real(value: &Value) -> f64 {
    opt_real(value).expect("number")
}

/// Exact equality for values the artifact records bit-exactly.
fn assert_bits(actual: f64, expected: f64, what: &str) {
    assert_eq!(actual.to_bits(), expected.to_bits(), "{what}");
}

/// IEEE equality without a tolerance (signed zeros compare equal).
#[allow(clippy::float_cmp)]
fn numerically_equal(left: f64, right: f64) -> bool {
    left == right
}

// --- independent GRIB1 decoding ---------------------------------------------

/// One GRIB1 message of the committed #202 input, decoded from its bytes.
struct GribMessage {
    file: String,
    index: usize,
    sha256: String,
    param: u8,
    reference_s: i64,
    step_s: i64,
    decimal_scale: i32,
    binary_scale: i32,
    reference_value: f64,
    bits: u8,
    packed: Vec<u32>,
    decoded_m: Vec<f64>,
}

fn u24(bytes: &[u8], pos: usize) -> usize {
    (usize::from(bytes[pos]) << 16)
        | (usize::from(bytes[pos + 1]) << 8)
        | usize::from(bytes[pos + 2])
}

fn sign_magnitude16(bytes: &[u8], pos: usize) -> i32 {
    let raw = (i32::from(bytes[pos]) << 8) | i32::from(bytes[pos + 1]);
    let magnitude = raw & 0x7FFF;
    if raw & 0x8000 == 0 {
        magnitude
    } else {
        -magnitude
    }
}

/// Exact `2^exponent` for the normal f64 range.
fn pow2(exponent: i32) -> f64 {
    let biased = u64::try_from(exponent + 1023).expect("normal f64 exponent");
    assert!((1..2047).contains(&biased), "normal f64 exponent");
    f64::from_bits(biased << 52)
}

/// GRIB1 IBM System/360 single-precision reference value.
fn ibm_single(word: u32) -> f64 {
    let sign = if word & 0x8000_0000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::try_from((word >> 24) & 0x7F).expect("7-bit exponent");
    sign * f64::from(word & 0x00FF_FFFF) * pow2(4 * (exponent - 64) - 24)
}

fn unpack(data: &[u8], bits: u8, count: usize) -> Vec<u32> {
    let width = usize::from(bits);
    assert!((1..=32).contains(&width), "supported bitsPerValue");
    assert!(
        count * width <= data.len() * 8,
        "BDS shorter than its values"
    );
    (0..count)
        .map(|k| {
            (k * width..(k + 1) * width).fold(0_u64, |value, bit| {
                (value << 1) | u64::from((data[bit / 8] >> (7 - bit % 8)) & 1)
            })
        })
        .map(|value| u32::try_from(value).expect("at most 32 bits"))
        .collect()
}

fn parse_message(file: &str, index: usize, message: &[u8]) -> GribMessage {
    assert_eq!(&message[..4], b"GRIB", "{file}: section 0");
    assert_eq!(message[7], 1, "{file}: GRIB edition 1");
    let mut pos = 8;
    let pds = &message[pos..pos + u24(message, pos)];
    pos += pds.len();
    assert_eq!(pds[7] & 0xC0, 0x80, "{file}: GDS present, no bitmap");
    // Hour steps, P2 = 0, timeRangeIndicator 0, 2023-11, minute 0 (century 21).
    assert_eq!(
        (pds[17], pds[19], pds[20], pds[24], pds[12], pds[13], pds[16]),
        (1, 0, 0, 21, 23, 11, 0),
        "{file}: #202 time encoding"
    );
    let gds = &message[pos..pos + u24(message, pos)];
    pos += gds.len();
    let ni = usize::from(u16::from_be_bytes([gds[6], gds[7]]));
    let nj = usize::from(u16::from_be_bytes([gds[8], gds[9]]));
    assert_eq!((ni, nj), (NX, NY), "{file}: grid shape");
    let bds = &message[pos..pos + u24(message, pos)];
    pos += bds.len();
    assert_eq!(&message[pos..], b"7777", "{file}: section 5");
    assert_eq!(
        bds[3] >> 4,
        0,
        "{file}: grid-point simple packing, float data"
    );
    let binary_scale = sign_magnitude16(bds, 4);
    let reference_value = ibm_single(u32::from_be_bytes([bds[6], bds[7], bds[8], bds[9]]));
    let bits = bds[10];
    let packed = unpack(&bds[11..], bits, ni * nj);
    let decoded_m = packed
        .iter()
        .map(|x| f64::from(*x) * pow2(binary_scale) + reference_value)
        .collect();
    GribMessage {
        file: file.to_owned(),
        index,
        sha256: sha256_hex(message),
        param: pds[8],
        reference_s: (i64::from(pds[14]) - 1) * 86_400 + i64::from(pds[15]) * 3_600,
        step_s: i64::from(pds[18]) * 3_600,
        decimal_scale: sign_magnitude16(pds, 26),
        binary_scale,
        reference_value,
        bits,
        packed,
        decoded_m,
    }
}

fn grib_files() -> Vec<(String, Vec<u8>)> {
    let mut names: Vec<String> = std::fs::read_dir(GRIB_DIR)
        .expect("GRIB fixture directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("name")
        })
        .filter_map(|file| {
            file.strip_prefix("FCOG_acc_SL.")?;
            file.strip_suffix(GRIB_SUFFIX).map(str::to_owned)
        })
        .collect();
    names.sort();
    assert_eq!(names.len(), 4, "the four #202 accumulated GRIB inputs");
    names
        .into_iter()
        .map(|name| {
            let path = format!("{GRIB_DIR}/{name}{GRIB_SUFFIX}");
            let bytes = std::fs::read(&path).expect("GRIB bytes");
            (name, bytes)
        })
        .collect()
}

fn grib_messages() -> Vec<GribMessage> {
    let mut messages = Vec::new();
    for (name, bytes) in grib_files() {
        let mut pos = 0;
        let mut index = 0;
        while pos < bytes.len() {
            let total = u24(&bytes, pos + 4);
            messages.push(parse_message(&name, index, &bytes[pos..pos + total]));
            pos += total;
            index += 1;
        }
    }
    messages
}

fn grib_index(cell: usize) -> usize {
    let (i, j) = (cell % NX, cell / NX);
    (NY - 1 - j) * NX + i
}

fn param_id(name: &str) -> u8 {
    match name {
        "lsp" => 142,
        "cp" => 143,
        other => panic!("unknown parameter {other}"),
    }
}

/// One accumulated observation of one cell with the message it was decoded from.
struct Observation<'a> {
    valid_s: i64,
    reset_s: i64,
    message: &'a GribMessage,
    decoded_m: f64,
    canonical: f32,
}

#[allow(clippy::cast_possible_truncation)]
fn canonical_kg_per_m2(metres: f64) -> f32 {
    (metres / FLEX_EXTRACT_METRE_FACTOR) as f32
}

fn sequence<'a>(
    oracle: &Value,
    messages: &'a [GribMessage],
    name: &str,
    cell: usize,
) -> Vec<Observation<'a>> {
    oracle["genuine"]["accumulated"][name]
        .as_array()
        .expect("accumulated observations")
        .iter()
        .map(|record| {
            let (valid_s, reset_s) = (integer(record, "valid_s"), integer(record, "reset_s"));
            let message = messages
                .iter()
                .find(|m| {
                    m.param == param_id(name)
                        && m.reference_s == reset_s
                        && m.step_s == valid_s - reset_s
                })
                .expect("message of the observation");
            let decoded_m = message.decoded_m[grib_index(cell)];
            Observation {
                valid_s,
                reset_s,
                message,
                decoded_m,
                canonical: canonical_kg_per_m2(decoded_m),
            }
        })
        .collect()
}

fn f32_ulp(value: f32) -> f32 {
    let magnitude = value.abs();
    f32::from_bits(magnitude.to_bits() + 1) - magnitude
}

#[allow(clippy::cast_possible_truncation)]
fn darain(window: [f64; 4], clip: bool) -> f64 {
    let [xa, xb, xc, xd] = if clip {
        window.map(|x| if x < 0.0 { 0.0 } else { x })
    } else {
        window
    };
    let xac = if xa + xc > 0.0 {
        xb * xc / (xa + xc)
    } else {
        0.5 * xb
    };
    let xbd = if xb + xd > 0.0 {
        xb * xc / (xb + xd)
    } else {
        0.5 * xc
    };
    xac + xbd
}

// --- independent policy re-implementation ------------------------------------

/// Exact dyadic value `n * 2^-DYADIC_SCALE` of a finite f64.
fn dyadic(value: f64) -> i128 {
    assert!(value.is_finite(), "finite dyadic input");
    if value == 0.0 {
        return 0;
    }
    let bits = value.abs().to_bits();
    let exponent = i32::try_from((bits >> 52) & 0x7FF).expect("exponent");
    assert!(exponent > 0, "normal f64 dyadic input");
    let mantissa = i128::from((bits & ((1_u64 << 52) - 1)) | (1_u64 << 52));
    let shift = exponent - 1075 + DYADIC_SCALE;
    let magnitude = if shift >= 0 {
        assert!(shift <= 126 - 53, "dyadic overflow");
        mantissa << shift
    } else {
        let drop = u32::try_from(-shift).expect("shift");
        assert_eq!(mantissa & ((1_i128 << drop) - 1), 0, "inexact dyadic");
        mantissa >> drop
    };
    if value < 0.0 {
        -magnitude
    } else {
        magnitude
    }
}

fn canonical_number(value: &Value) -> f64 {
    if let Some(number) = opt_real(value) {
        return number;
    }
    match value.as_str().expect("canonical value") {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        other => panic!("unknown non-finite token {other}"),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn canonical_f32(side: &Value) -> f32 {
    let value = canonical_number(&side["canonical_value"]);
    let narrowed = value as f32;
    assert_bits(f64::from(narrowed), value, "canonical values are f32");
    narrowed
}

/// `B = 1000 (q_prev + q_cur) + r_prev + r_cur` in exact dyadic kg/m2.
fn canonical_bound(pair: &Value) -> i128 {
    ["previous", "current"]
        .iter()
        .map(|side| {
            let packing = &pair[side]["packing"];
            assert_eq!(integer(packing, "decimalScaleFactor"), 0, "D = 0 class");
            let exponent = i32::try_from(integer(packing, "binaryScaleFactor")).expect("E");
            let scale = i128::from(integer(packing, "canonical_per_source_unit"));
            let quantum = scale << u32::try_from(exponent + DYADIC_SCALE).expect("quantum");
            let value = canonical_f32(&pair[side]);
            let ulp = dyadic(f64::from(f32_ulp(value)));
            let path = dyadic(f64::from(value)) + ulp;
            assert_eq!(
                path & ((1 << F64_PATH_SHIFT) - 1),
                0,
                "inexact f64 path term"
            );
            quantum + ulp / 2 + (path >> F64_PATH_SHIFT)
        })
        .sum()
}

enum Structure {
    Final(&'static str, &'static str),
    Decrease(i128),
}

fn structure(pair: &Value) -> Structure {
    let (prev, cur) = (&pair["previous"], &pair["current"]);
    if !SUPPORTED_FIELDS.contains(&text(pair, "field")) {
        return Structure::Final("REJECT", "unsupported_field");
    }
    if text(pair, "canonical_unit") != "kilogram_per_square_meter" {
        return Structure::Final("REJECT", "unit_mismatch");
    }
    if prev["grid_id"] != cur["grid_id"] || prev["source_id"] != cur["source_id"] {
        return Structure::Final("REJECT", "identity_mismatch");
    }
    for side in [prev, cur] {
        if side["canonical_value"].is_null() {
            return Structure::Final("REJECT", "missing_value");
        }
        let value = canonical_number(&side["canonical_value"]);
        if !value.is_finite() {
            return Structure::Final("REJECT", "non_finite_value");
        }
        if value < 0.0 {
            return Structure::Final("REJECT", "negative_accumulated_amount");
        }
    }
    let (Some(prev_valid), Some(cur_valid)) = (
        opt_integer(&prev["valid_time_s"]),
        opt_integer(&cur["valid_time_s"]),
    ) else {
        return Structure::Final("REJECT", "missing_time_metadata");
    };
    if cur_valid <= prev_valid {
        return Structure::Final("REJECT", "non_increasing_valid_time");
    }
    let (Some(prev_reset), Some(cur_reset)) =
        (opt_integer(&prev["reset_s"]), opt_integer(&cur["reset_s"]))
    else {
        return Structure::Final("REJECT", "missing_time_metadata");
    };
    if cur_reset == prev_valid && cur_reset > prev_reset {
        return Structure::Final("OUTSIDE_RULE", "declared_contiguous_reset");
    }
    if cur_reset != prev_reset || cur_reset >= prev_valid {
        return Structure::Final("REJECT", "reset_mismatch");
    }
    let difference = dyadic(f64::from(canonical_f32(cur))) - dyadic(f64::from(canonical_f32(prev)));
    if difference >= 0 {
        return Structure::Final("OUTSIDE_RULE", "ordinary_same_run_interval");
    }
    Structure::Decrease(difference)
}

/// Policy B (selected): bounded admission with packing provenance.
fn classify(pair: &Value) -> (&'static str, &'static str) {
    let difference = match structure(pair) {
        Structure::Final(verdict, code) => return (verdict, code),
        Structure::Decrease(difference) => difference,
    };
    for side in ["previous", "current"] {
        let observation = &pair[side];
        let packing = &observation["packing"];
        if packing.is_null() {
            return ("UNRESOLVED", "missing_packing_provenance");
        }
        if packing["mars_class"].as_str() != Some("OD") {
            return ("UNRESOLVED", "unsupported_source_class");
        }
        if text(packing, "encoding") != "grib1_grid_simple"
            || integer(packing, "bitsPerValue") < 1
            || packing["bitmap_present"].as_bool().expect("bitmap flag")
            || integer(packing, "decimalScaleFactor") != 0
        {
            return ("UNRESOLVED", "unsupported_packing");
        }
        if text(packing, "canonical_conversion") != CANONICAL_CONVERSION {
            return ("UNRESOLVED", "unsupported_conversion");
        }
        let expected_param = if text(pair, "field") == "large_scale_precipitation" {
            142
        } else {
            143
        };
        if text(packing, "message_sha256") != text(observation, "source_message_sha256")
            || integer(packing, "paramId") != expected_param
        {
            return ("REJECT", "packing_provenance_mismatch");
        }
        let reference = integer(packing, "forecast_reference_s");
        if reference != integer(observation, "reset_s")
            || reference + integer(packing, "step_s") != integer(observation, "valid_time_s")
        {
            return ("REJECT", "run_identity_mismatch");
        }
        if text(packing, "source_unit") != "m"
            || i128::from(integer(packing, "canonical_per_source_unit"))
                != CANONICAL_PER_SOURCE_UNIT
        {
            return ("REJECT", "unit_mismatch");
        }
    }
    let identical = ["referenceValue", "binaryScaleFactor", "decimalScaleFactor"]
        .iter()
        .all(|key| pair["previous"]["packing"][key] == pair["current"]["packing"][key]);
    if identical {
        return ("REJECT", "identical_packing_parameters");
    }
    if -difference < canonical_bound(pair) {
        ("ADMIT_BOUNDED", "quantization_compatible_decrease")
    } else {
        ("REJECT", "decrease_exceeds_quantization_bound")
    }
}

/// Policy A (current #75) and policy C (`flex_extract` compatibility).
fn classify_alternative(pair: &Value, strict: bool) -> (&'static str, &'static str) {
    match structure(pair) {
        Structure::Final(verdict, code) => (verdict, code),
        Structure::Decrease(_) if strict => ("REJECT", "same_run_negative_delta"),
        Structure::Decrease(_) => ("ADMIT_UNBOUNDED", "clipped_by_darain"),
    }
}

/// Pair record of a genuine row, built from the Rust-decoded messages only.
fn genuine_pair(name: &str, prev: &Observation<'_>, cur: &Observation<'_>) -> Value {
    let side = |ob: &Observation<'_>| {
        serde_json::json!({
            "grid_id": "regular_ll_3x3_lat52_lon10_step1",
            "source_id": format!("ecmf-table128-param{}", param_id(name)),
            "valid_time_s": ob.valid_s,
            "reset_s": ob.reset_s,
            "canonical_value": f64::from(ob.canonical),
            "source_message_sha256": ob.message.sha256,
            "packing": {
                "encoding": "grib1_grid_simple",
                "mars_class": "OD",
                "message_sha256": ob.message.sha256,
                "paramId": ob.message.param,
                "bitsPerValue": ob.message.bits,
                "binaryScaleFactor": ob.message.binary_scale,
                "decimalScaleFactor": ob.message.decimal_scale,
                "referenceValue": ob.message.reference_value,
                "bitmap_present": false,
                "forecast_reference_s": ob.message.reference_s,
                "step_s": ob.message.step_s,
                "source_unit": "m",
                "canonical_per_source_unit": CANONICAL_PER_SOURCE_UNIT,
                "canonical_conversion": CANONICAL_CONVERSION,
            },
        })
    };
    let field = if name == "lsp" {
        "large_scale_precipitation"
    } else {
        "convective_precipitation"
    };
    serde_json::json!({
        "field": field,
        "canonical_unit": "kilogram_per_square_meter",
        "previous": side(prev),
        "current": side(cur),
    })
}

/// `num/den` with a power-of-two denominator as an exact dyadic value.
fn dyadic_from_exact(exact: &str) -> i128 {
    let (numerator, denominator) = exact.split_once('/').expect("num/den");
    let numerator: i128 = numerator.parse().expect("numerator");
    let denominator: u128 = denominator.parse().expect("denominator");
    assert!(denominator.is_power_of_two(), "dyadic denominator");
    let power = i32::try_from(denominator.trailing_zeros()).expect("power");
    numerator << u32::try_from(DYADIC_SCALE - power).expect("scale")
}

/// `(parameter, cell, observation index)` of every decoded same-run decrease,
/// in the decision artifact's row order.
fn decreases(oracle: &Value, messages: &[GribMessage]) -> Vec<(&'static str, usize, usize)> {
    let mut rows = Vec::new();
    for name in ["lsp", "cp"] {
        for cell in 0..NX * NY {
            let obs = sequence(oracle, messages, name, cell);
            for k in 1..obs.len() {
                if obs[k].reset_s == obs[k - 1].reset_s && obs[k].decoded_m < obs[k - 1].decoded_m {
                    rows.push((name, cell, k));
                }
            }
        }
    }
    rows
}

// --- tests -----------------------------------------------------------------------

#[test]
fn test_grib_packing_decision_digest_and_inputs_are_pinned() {
    let decision = parse(DECISION_PATH);
    assert_eq!(
        sha256_hex(read_canonical(DECISION_PATH).as_bytes()),
        DECISION_SHA256
    );
    let consumed = &decision["consumed"];
    for (key, path) in [
        ("mapping_decision_sha256", MAPPING_DECISION_PATH),
        ("mapping_evidence_sha256", EVIDENCE_PATH),
        ("darain_oracle_sha256", ORACLE_PATH),
        ("eccodes_keys_sha256", ECCODES_KEYS_PATH),
    ] {
        assert_eq!(
            text(consumed, key),
            sha256_hex(read_canonical(path).as_bytes()),
            "{key}"
        );
    }
    let evidence = parse(EVIDENCE_PATH);
    assert_eq!(
        text(consumed, "raw_trace_sha256"),
        text(&evidence, "trace_sha256")
    );
    assert_eq!(text(consumed, "eccodes_version"), "2.24.2");
    assert_eq!(text(&evidence["runtime"], "eccodes"), "2.24.2");
    for (name, bytes) in grib_files() {
        let digest = sha256_hex(&bytes);
        assert_eq!(
            digest,
            text(&evidence["accumulated_input_sha256"], &name),
            "{name}"
        );
        assert_eq!(
            digest,
            text(&consumed["grib_files"][&name], "sha256"),
            "{name}"
        );
    }
}

#[test]
fn test_grib_packing_pins_match_reference_manifests() {
    let decision = parse(DECISION_PATH);
    let references = &decision["references"];
    let flex_extract = parse("reference/flex-extract.json");
    let flexpart = parse("reference/flexpart-11.1.json");
    assert_eq!(
        text(&references["flex_extract"], "pinned_commit"),
        text(&flex_extract, "pinned_commit")
    );
    assert_eq!(
        text(&references["flexpart"], "pinned_commit"),
        text(&flexpart, "pinned_commit")
    );
    let oracle = parse(ORACLE_PATH);
    assert_eq!(
        references["flex_extract"]["blobs"]["Source/Python/Mods/disaggregation.py"],
        oracle["flex_extract"]["blob"]
    );
}

#[test]
fn test_grib_packing_rust_decoder_matches_eccodes_and_artifact() {
    let messages = grib_messages();
    let keys = parse(ECCODES_KEYS_PATH);
    let decision = parse(DECISION_PATH);
    let recorded = decision["decoding"]["messages"]
        .as_array()
        .expect("messages");
    assert_eq!(messages.len(), 32);
    assert_eq!(recorded.len(), messages.len());
    for (message, artifact) in messages.iter().zip(recorded) {
        let ec = &keys["files"][&message.file]["messages"][message.index];
        let what = format!("{} #{}", message.file, message.index);
        assert_eq!(text(ec, "packingType"), "grid_simple", "{what}");
        assert_eq!(integer(ec, "paramId"), i64::from(message.param), "{what}");
        assert_eq!(
            integer(ec, "bitsPerValue"),
            i64::from(message.bits),
            "{what}"
        );
        assert_eq!(
            integer(ec, "binaryScaleFactor"),
            i64::from(message.binary_scale),
            "{what}"
        );
        assert_eq!(
            integer(ec, "decimalScaleFactor"),
            i64::from(message.decimal_scale),
            "{what}"
        );
        assert_eq!(integer(ec, "bitmapPresent"), 0, "{what}");
        assert_eq!(text(ec, "units"), "m", "{what}");
        assert_eq!(text(ec, "stepType"), "instant", "{what}");
        assert_bits(message.reference_value, real(&ec["referenceValue"]), &what);
        assert_eq!(text(artifact, "message_sha256"), message.sha256, "{what}");
        assert_eq!(
            integer(artifact, "binaryScaleFactor"),
            i64::from(message.binary_scale)
        );
        for (n, decoded) in message.decoded_m.iter().enumerate() {
            assert_bits(*decoded, real(&ec["values"][n]), &what);
            assert_bits(*decoded, real(&artifact["decoded_m"][n]), &what);
            assert_eq!(
                opt_integer(&artifact["packed_integers"][n]),
                Some(i64::from(message.packed[n])),
                "{what}"
            );
        }
    }
}

#[test]
fn test_grib_packing_encoder_inputs_reproduce_every_packed_integer() {
    let messages = grib_messages();
    let evidence = parse(EVIDENCE_PATH);
    let mut bases: Vec<i64> = messages.iter().map(|m| m.reference_s).collect();
    bases.sort_unstable();
    bases.dedup();
    let mut reproduced = 0;
    for message in &messages {
        let name = if message.param == 142 { "lsp" } else { "cp" };
        let first = 4 * bases
            .iter()
            .position(|b| *b == message.reference_s)
            .expect("base");
        let last = first + usize::try_from(message.step_s / 10_800 - 1).expect("step");
        for cell in 0..NX * NY {
            let node = format!("{},{}", cell % NX, cell / NX);
            let series = &evidence["interval_amounts_mm"][name][&node];
            // Left-to-right float64 sum as Python 3.10 `sum` in the #202 trace.
            let total = (first..=last).fold(0.0, |sum, k| sum + real(&series[k]));
            let true_m = total / 1000.0;
            let scaled = (true_m - message.reference_value) * pow2(-message.binary_scale);
            let expected = (scaled + 0.5).floor();
            assert_bits(
                f64::from(message.packed[grib_index(cell)]),
                expected,
                &message.file,
            );
            reproduced += 1;
        }
    }
    let decision = parse(DECISION_PATH);
    assert_eq!(
        opt_integer(&decision["encoder_reconstruction"]["values_reproduced"]),
        Some(reproduced)
    );
}

#[test]
fn test_grib_packing_three_decreases_reproduced_exactly() {
    let messages = grib_messages();
    let oracle = parse(ORACLE_PATH);
    let decision = parse(DECISION_PATH);
    let rows = decreases(&oracle, &messages);
    let expected: BTreeSet<(String, i64, i64)> = oracle["genuine"]["negative_interval_means"]
        .as_array()
        .expect("negatives")
        .iter()
        .map(|n| {
            (
                text(n, "parameter").to_owned(),
                integer(n, "cell"),
                integer(n, "interval_end_s"),
            )
        })
        .collect();
    let found: BTreeSet<(String, i64, i64)> = rows
        .iter()
        .map(|(name, cell, k)| {
            let obs = sequence(&oracle, &messages, name, *cell);
            (
                (*name).to_owned(),
                i64::try_from(*cell).expect("cell"),
                obs[*k].valid_s,
            )
        })
        .collect();
    assert_eq!(found, expected);
    let offending = decision["offending_rows"].as_array().expect("rows");
    assert_eq!(offending.len(), 3);
    for (row, (name, cell, k)) in offending.iter().zip(&rows) {
        let obs = sequence(&oracle, &messages, name, *cell);
        let (prev, cur) = (&obs[k - 1], &obs[*k]);
        assert_eq!(text(row, "parameter"), *name);
        assert_eq!(
            integer(row, "canonical_cell"),
            i64::try_from(*cell).expect("cell")
        );
        assert_eq!(integer(row, "interval_duration_s"), 10_800);
        // ecCodes-equal decoded values decrease by exactly one previous quantum.
        assert_bits(
            cur.decoded_m - prev.decoded_m,
            -pow2(prev.message.binary_scale),
            name,
        );
        assert_ne!(prev.message.binary_scale, cur.message.binary_scale);
        assert_bits(real(&row["previous"]["decoded_m"]), prev.decoded_m, name);
        assert_bits(real(&row["current"]["decoded_m"]), cur.decoded_m, name);
        // Original flex_extract arithmetic (EcFlexpart.py:1130-1143).
        let deac = (cur.decoded_m / FLEX_EXTRACT_METRE_FACTOR
            - prev.decoded_m / FLEX_EXTRACT_METRE_FACTOR)
            / DTIME_HOURS;
        let oracle_value = oracle["genuine"]["negative_interval_means"]
            .as_array()
            .expect("negatives")
            .iter()
            .find(|n| {
                text(n, "parameter") == *name
                    && integer(n, "cell") == i64::try_from(*cell).expect("cell")
            })
            .expect("oracle row");
        assert_bits(deac, real(&oracle_value["value_mm_per_h"]), name);
        assert_bits(
            real(&row["signed_interval_mean_rate"]["flex_extract_deac_vals_mm_per_h"]),
            deac,
            name,
        );
        // Canonical f32 keeps the decrease; f32 rounding is monotone.
        assert!(cur.canonical < prev.canonical, "{name}: canonical decrease");
        let ulps = (f64::from(cur.canonical) - f64::from(prev.canonical))
            / f64::from(f32_ulp(prev.canonical));
        assert_bits(
            real(&row["signed_difference"]["canonical_f32_ulps_of_previous"]),
            ulps,
            name,
        );
        assert_bits(real(&row["darain_clipped_mm_per_h"]), 0.0, name);
    }
    let stats = &decision["pair_statistics"];
    assert_eq!(opt_integer(&stats["same_run_pairs"]), Some(216));
    assert_eq!(opt_integer(&stats["decoded_decreases"]), Some(3));
    assert_eq!(opt_integer(&stats["canonical_decreases"]), Some(3));
    assert_eq!(
        opt_integer(&stats["decreases_on_unchanged_packing"]),
        Some(0)
    );
}

#[test]
fn test_grib_packing_current_75_validation_still_rejects_rows() {
    let messages = grib_messages();
    let oracle = parse(ORACLE_PATH);
    let decision = parse(DECISION_PATH);
    assert_eq!(
        text(&decision["decision"], "implementation_status"),
        "not_implemented"
    );
    let rejected: BTreeMap<(String, usize), i64> = decision["offending_rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            (
                (
                    text(row, "parameter").to_owned(),
                    usize::try_from(integer(row, "canonical_cell")).expect("cell"),
                ),
                integer(&row["rejection_75_90"], "observation_index"),
            )
        })
        .collect();
    for name in ["lsp", "cp"] {
        for cell in 0..NX * NY {
            let observations: Vec<AccumulatedObservation> =
                sequence(&oracle, &messages, name, cell)
                    .iter()
                    .map(|ob| AccumulatedObservation {
                        valid_time_epoch_seconds: ob.valid_s,
                        reset_epoch_seconds: ob.reset_s,
                        accumulated_amount_kg_per_square_meter: f64::from(ob.canonical),
                    })
                    .collect();
            let result = validate_interval_sequence(&observations);
            match rejected.get(&(name.to_owned(), cell)) {
                Some(index) => match result {
                    Err(AccumulationError::NegativeDelta {
                        index: actual,
                        previous,
                        current,
                    }) => {
                        assert_eq!(i64::try_from(actual).expect("index"), *index);
                        assert!(current < previous, "{name} cell {cell}");
                    }
                    other => panic!("{name} cell {cell}: expected NegativeDelta, got {other:?}"),
                },
                None => assert!(result.is_ok(), "{name} cell {cell}: {result:?}"),
            }
        }
    }
}

#[test]
fn test_grib_packing_darain_clip_is_independently_established() {
    let oracle = parse(ORACLE_PATH);
    let decision = parse(DECISION_PATH);
    let mut values = 0;
    for name in ["lsp", "cp"] {
        let means = &oracle["genuine"]["interval_means"][name]["values_mm_per_h"];
        let ends: Vec<i64> = oracle["genuine"]["interval_means"][name]["interval_end_s"]
            .as_array()
            .expect("ends")
            .iter()
            .map(|e| opt_integer(e).expect("end"))
            .collect();
        let negatives: BTreeSet<(usize, usize)> = oracle["genuine"]["negative_interval_means"]
            .as_array()
            .expect("negatives")
            .iter()
            .filter(|n| text(n, "parameter") == name)
            .map(|n| {
                let end = integer(n, "interval_end_s");
                (
                    ends.iter().position(|e| *e == end).expect("interval"),
                    usize::try_from(integer(n, "cell")).expect("cell"),
                )
            })
            .collect();
        let mut changed = BTreeSet::new();
        let members = oracle["genuine"]["darain_members"][name]
            .as_array()
            .expect("members");
        for (slot, member) in members.iter().enumerate() {
            for cell in 0..NX * NY {
                let window = [0, 1, 2, 3].map(|n| real(&means[slot + n][cell]));
                let original = real(&member["result_mm_per_h"][cell]);
                assert_bits(darain(window, true), original, name);
                let projected = [0, 1, 2, 3].map(|n| {
                    if negatives.contains(&(slot + n, cell)) {
                        0.0
                    } else {
                        window[n]
                    }
                });
                assert_bits(darain(projected, true), original, name);
                // Numeric inequality: an unclipped -0.0 equals the original +0.0.
                if !numerically_equal(darain(window, false), original) {
                    changed.insert((integer(member, "member_time_s"), cell));
                }
                values += 1;
            }
        }
        let recorded: BTreeSet<(i64, usize)> = decision["darain_clip"]["recomputation"][name]
            ["values_changed_without_clip"]
            .as_array()
            .expect("changed")
            .iter()
            .map(|c| {
                (
                    integer(c, "member_time_s"),
                    usize::try_from(integer(c, "cell")).expect("cell"),
                )
            })
            .collect();
        assert!(!changed.is_empty(), "{name}: clipping must matter");
        assert_eq!(changed, recorded, "{name}");
    }
    assert_eq!(values, 234);
}

#[test]
fn test_grib_packing_bound_uses_actual_message_metadata() {
    let messages = grib_messages();
    let oracle = parse(ORACLE_PATH);
    let decision = parse(DECISION_PATH);
    let rows = decreases(&oracle, &messages);
    for (row, (name, cell, k)) in decision["offending_rows"]
        .as_array()
        .expect("rows")
        .iter()
        .zip(&rows)
    {
        let obs = sequence(&oracle, &messages, name, *cell);
        let pair = genuine_pair(name, &obs[k - 1], &obs[*k]);
        let bound = canonical_bound(&pair);
        assert_eq!(
            bound,
            dyadic_from_exact(text(&row["bounds"]["canonical_kg_per_m2"], "exact")),
            "{name}: canonical bound"
        );
        // Source-level bound q_prev + q_cur from the two messages' own E (D = 0).
        let source = pow2(obs[k - 1].message.binary_scale) + pow2(obs[*k].message.binary_scale);
        assert_bits(
            real(&row["bounds"]["source_adjacent_level_m"]["float"]),
            source,
            name,
        );
        assert_eq!(
            text(&row["previous"], "message_sha256"),
            obs[k - 1].message.sha256
        );
        assert_eq!(
            text(&row["current"], "message_sha256"),
            obs[*k].message.sha256
        );
        assert_eq!(
            classify(&pair),
            ("ADMIT_BOUNDED", "quantization_compatible_decrease")
        );
        assert_eq!(text(&row["classification"], "verdict"), "ADMIT_BOUNDED");
        assert_eq!(classify_alternative(&pair, true).0, "REJECT");
        assert_eq!(classify_alternative(&pair, false).0, "ADMIT_UNBOUNDED");
    }
}

#[test]
fn test_grib_packing_negative_controls_match_independent_policy() {
    let decision = parse(DECISION_PATH);
    let controls = decision["negative_controls"].as_array().expect("controls");
    let ids: BTreeSet<&str> = controls.iter().map(|c| text(c, "id")).collect();
    for required in REQUIRED_CONTROLS {
        assert!(ids.contains(required), "missing control {required}");
    }
    let mut misclassified_by_c = Vec::new();
    for control in controls {
        let id = text(control, "id");
        let pair = &control["pair"];
        let expected = (
            text(control, "expected_verdict"),
            text(control, "expected_code"),
        );
        assert_eq!(classify(pair), expected, "{id}");
        let strict = classify_alternative(pair, true);
        let unconditional = classify_alternative(pair, false);
        assert_eq!(
            control["policy_a"],
            serde_json::json!([strict.0, strict.1]),
            "{id}: A"
        );
        assert_eq!(
            control["policy_c"],
            serde_json::json!([unconditional.0, unconditional.1]),
            "{id}: C"
        );
        if unconditional.0 == "ADMIT_UNBOUNDED" && expected.0 != "ADMIT_BOUNDED" {
            misclassified_by_c.push(id);
        }
    }
    let recorded: Vec<&str> = decision["decision"]["policies"]["C_unconditional_flex_extract"]
        ["misclassified_controls"]
        .as_array()
        .expect("list")
        .iter()
        .map(|v| v.as_str().expect("id"))
        .collect();
    assert_eq!(misclassified_by_c, recorded);
    for id in [
        "bound_smallest_rejected",
        "undeclared_reset_consistent_provenance",
    ] {
        assert!(misclassified_by_c.contains(&id), "{id}");
    }
}

#[test]
fn test_grib_packing_source_class_requires_declared_od_provenance() {
    let messages = grib_messages();
    let oracle = parse(ORACLE_PATH);
    let obs = sequence(&oracle, &messages, "lsp", 1);
    let pair = genuine_pair("lsp", &obs[8], &obs[9]);
    for side in ["previous", "current"] {
        for source_class in [
            serde_json::json!("EA"),
            serde_json::json!("RD"),
            Value::Null,
        ] {
            let mut unsupported = pair.clone();
            unsupported[side]["packing"]["mars_class"] = source_class;
            assert_eq!(
                classify(&unsupported),
                ("UNRESOLVED", "unsupported_source_class"),
                "{side}: only the exercised OD deaccumulation path is resolved"
            );
        }
    }
    let decision = parse(DECISION_PATH);
    assert_eq!(
        text(&decision["consumed"]["source_class"], "mars_class"),
        "OD"
    );
    for id in [
        "unsupported_source_class_ea",
        "unsupported_source_class_other",
        "missing_source_class_provenance",
    ] {
        let control = decision["negative_controls"]
            .as_array()
            .expect("controls")
            .iter()
            .find(|control| text(control, "id") == id)
            .expect("source-class control");
        assert_eq!(text(control, "expected_verdict"), "UNRESOLVED", "{id}");
        assert_eq!(
            text(control, "expected_code"),
            "unsupported_source_class",
            "{id}"
        );
    }
}

#[test]
fn test_grib_packing_decision_is_internally_consistent() {
    let decision = parse(DECISION_PATH);
    let verdict = &decision["decision"];
    assert_eq!(text(verdict, "verdict"), "ADMIT_BOUNDED");
    let policies = &verdict["policies"];
    assert_eq!(text(&policies["B_evidence_bounded"], "verdict"), "SELECTED");
    assert_eq!(
        text(&policies["C_unconditional_flex_extract"], "verdict"),
        "REJECTED"
    );
    assert_eq!(
        text(&policies["A_strict_rejection"], "verdict"),
        "REJECTED_AS_PERMANENT_POLICY"
    );
    let classes = verdict["classes"].as_array().expect("classes");
    let kinds: BTreeSet<&str> = classes.iter().map(|c| text(c, "verdict")).collect();
    assert_eq!(
        kinds,
        BTreeSet::from(["ADMIT_BOUNDED", "REJECT", "UNRESOLVED"])
    );
    for key in [
        "fields",
        "temporal_kind",
        "canonical_unit",
        "sign",
        "source",
        "packing",
        "decode",
        "canonical_conversion",
        "run_identity",
        "pair",
    ] {
        assert!(
            verdict["supported_class"][key].is_string()
                || verdict["supported_class"][key].is_array(),
            "{key}"
        );
    }
    assert!(
        text(&verdict["supported_class"], "canonical_conversion").starts_with(CANONICAL_CONVERSION)
    );
    assert!(text(&verdict["prerequisite"], "required_record").contains("message sha256"));
    assert_eq!(
        text(&decision["attribution"], "fixture"),
        "PROVEN_BY_CONSTRUCTION"
    );
    assert_eq!(
        text(&decision["attribution"], "operational"),
        "CONSISTENT_ONLY"
    );
    for row in decision["offending_rows"].as_array().expect("rows") {
        assert_eq!(row["encoder_inputs_identical"].as_bool(), Some(true));
        assert_eq!(row["packing_parameters_differ"].as_bool(), Some(true));
        assert_eq!(
            row["run_identity"]["same_forecast_reference"].as_bool(),
            Some(true)
        );
        let ratio = real(&row["bounds"]["decrease_over_canonical_bound"]);
        assert!(
            ratio > 0.0 && ratio < 1.0,
            "admitted rows lie inside the bound"
        );
    }
    assert!(text(&verdict["representation"], "canonical_interval_product").contains("+0.0"));
}
