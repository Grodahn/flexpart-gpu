//! Genuine #202 accumulated LSP/CP snapshots with #216 source records (#219).
//!
//! Test provider only; #32 owns operational GRIB decoding. Every
//! source-quantization key is read from the committed GRIB1 bytes in
//! `fixtures/accumulation/precipitation-202-accumulated-grib/` and cross-checked
//! against the frozen #216 artifact (`decoding.messages`, `consumed`). The MARS
//! class is the #202 trace's explicit control provenance recorded in
//! `consumed.source_class`; it is never inferred from the GRIB keys. Canonical
//! values follow the declared conversion `f32_nearest(Y / 0.001)` in double and
//! must equal the darain oracle's decoded `values_m`.
#![allow(dead_code)]

use flexpart_gpu::meteorology::{
    Accumulation, Calendar, Field, FieldId, FieldTime, GribPackingProvenance, Snapshot,
    SourceQuantization, TemporalKind, Unit, CANONICAL_KG_PER_SQUARE_METER_PER_METRE,
    F64_DECODE_UNIT_SCALE_THEN_F32_NEAREST, GRIB1_GRID_SIMPLE_ENCODING,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const DECISION: &str =
    include_str!("../../fixtures/accumulation/grib-packing-decrease-decision-v1.json");
/// Pinned digest of the frozen #216 artifact (as `tests/grib_packing_decision.rs`).
pub const DECISION_SHA256: &str =
    "83059dcbce88544aa141ffcb9fb8e2ec96cc3ec2eca9ef76873e9b84bf3ddfa1";
pub const ORACLE: &str =
    include_str!("../../fixtures/accumulation/precipitation-darain-oracle-v1.json");
pub const ORACLE_SHA256: &str = "54830e945c438e1f0eb5e8eabcf7aa9746891952d19cd9b8c44a1e4b16997f90";
pub const GRIB_DIR: &str = "fixtures/accumulation/precipitation-202-accumulated-grib";
/// Unix seconds of the oracle epoch `2023-11-01T00:00:00Z`.
pub const ORACLE_EPOCH_UNIX_S: i64 = 1_698_796_800;
/// `flex_extract` divides GRIB metres by `fak = 1/1000` (`EcFlexpart.py:1123-1131`).
const FLEX_EXTRACT_METRE_FACTOR: f64 = 1.0 / 1000.0;
/// ECMWF originating centre (`ecmf`) and local parameter table of LSP/CP.
const ECMWF_CENTRE: u8 = 98;
const ECMWF_TABLE: u8 = 128;
pub const NX: usize = 3;
pub const NY: usize = 3;

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Marker prefix of a quoted number token during exact parsing.
const NUMBER_MARK: char = '\u{E000}';

/// Parse JSON with correctly rounded numbers.
///
/// The crate's `serde_json` (no `float_roundtrip`) may round a decimal one ulp
/// away; Python-written f64 values must be compared bit for bit, so every
/// number token is re-read with `str::parse` (as `tests/grib_packing_decision.rs`).
pub fn exact_json(source: &str) -> Value {
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
            quoted.push(NUMBER_MARK);
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
    restore_numbers(serde_json::from_str(&quoted).expect("fixture JSON parses"))
}

fn restore_numbers(value: Value) -> Value {
    match value {
        Value::String(text) => match text.strip_prefix(NUMBER_MARK) {
            Some(number) if !number.contains(['.', 'e', 'E']) => {
                Value::Number(number.parse::<i64>().expect("integer").into())
            }
            Some(number) => Value::Number(
                serde_json::Number::from_f64(number.parse().expect("number")).expect("finite"),
            ),
            None => Value::String(text),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(restore_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, item)| (key, restore_numbers(item)))
                .collect(),
        ),
        other => other,
    }
}

pub fn decision() -> Value {
    assert_eq!(
        sha256_hex(DECISION.as_bytes()),
        DECISION_SHA256,
        "#216 artifact drifted"
    );
    exact_json(DECISION)
}

pub fn oracle() -> Value {
    assert_eq!(
        sha256_hex(ORACLE.as_bytes()),
        ORACLE_SHA256,
        "darain oracle drifted"
    );
    exact_json(ORACLE)
}

/// One message decoded from the committed bytes.
#[derive(Debug, Clone)]
pub struct Message {
    pub file: String,
    pub record: SourceQuantization,
    pub decoded_m: Vec<f64>,
}

fn u24(bytes: &[u8], pos: usize) -> usize {
    (usize::from(bytes[pos]) << 16)
        | (usize::from(bytes[pos + 1]) << 8)
        | usize::from(bytes[pos + 2])
}

fn sign_magnitude16(bytes: &[u8], pos: usize) -> i32 {
    let raw = (i32::from(bytes[pos]) << 8) | i32::from(bytes[pos + 1]);
    if raw & 0x8000 == 0 {
        raw & 0x7FFF
    } else {
        -(raw & 0x7FFF)
    }
}

fn ibm_single(word: u32) -> f64 {
    let sign = if word & 0x8000_0000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::try_from((word >> 24) & 0x7F).expect("exponent");
    sign * f64::from(word & 0x00FF_FFFF) * 2_f64.powi(4 * (exponent - 64) - 24)
}

fn unpack(data: &[u8], bits: usize, count: usize) -> Vec<u64> {
    (0..count)
        .map(|k| {
            (k * bits..(k + 1) * bits).fold(0_u64, |value, bit| {
                (value << 1) | u64::from((data[bit / 8] >> (7 - bit % 8)) & 1)
            })
        })
        .collect()
}

/// Days from 1970-01-01 of a proleptic Gregorian date (Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn parse_message(file: &str, message: &[u8], mars_class: &str) -> Message {
    assert_eq!(
        (&message[..4], message[7]),
        (&b"GRIB"[..], 1),
        "{file}: GRIB1"
    );
    let mut pos = 8;
    let pds = &message[pos..pos + u24(message, pos)];
    pos += pds.len();
    assert_eq!(
        (pds[3], pds[4]),
        (ECMWF_TABLE, ECMWF_CENTRE),
        "{file}: ECMWF table 128"
    );
    assert_eq!(pds[7] & 0x80, 0x80, "{file}: GDS present");
    assert_eq!(
        (pds[17], pds[19], pds[20]),
        (1, 0, 0),
        "{file}: hour P1, P2 = 0, TRI 0"
    );
    let gds = &message[pos..pos + u24(message, pos)];
    pos += gds.len();
    assert_eq!(
        (
            u16::from_be_bytes([gds[6], gds[7]]),
            u16::from_be_bytes([gds[8], gds[9]])
        ),
        (3, 3),
        "{file}: 3x3 grid"
    );
    let bitmap_present = pds[7] & 0x40 != 0;
    assert!(
        !bitmap_present,
        "{file}: the #202 input has no bitmap section"
    );
    let bds = &message[pos..pos + u24(message, pos)];
    pos += bds.len();
    assert_eq!(&message[pos..], b"7777", "{file}: section 5");
    assert_eq!(
        bds[3] >> 4,
        0,
        "{file}: grid-point simple packing, float data"
    );
    let binary_scale_factor = sign_magnitude16(bds, 4);
    let reference_value = ibm_single(u32::from_be_bytes([bds[6], bds[7], bds[8], bds[9]]));
    let bits = usize::from(bds[10]);
    let decoded_m = unpack(&bds[11..], bits, NX * NY)
        .into_iter()
        .map(|x| {
            #[allow(clippy::cast_precision_loss)]
            let packed = x as f64;
            packed * 2_f64.powi(binary_scale_factor) + reference_value
        })
        .collect();
    let year = (i64::from(pds[24]) - 1) * 100 + i64::from(pds[12]);
    let reference = days_from_civil(year, i64::from(pds[13]), i64::from(pds[14])) * 86_400
        + i64::from(pds[15]) * 3_600
        + i64::from(pds[16]) * 60;
    let sha = sha256_hex(message);
    Message {
        file: file.to_owned(),
        record: SourceQuantization {
            source_id: format!("ecmf-table{ECMWF_TABLE}-param{}", pds[8]),
            source_message_sha256: sha.clone(),
            packing: GribPackingProvenance {
                encoding: GRIB1_GRID_SIMPLE_ENCODING.to_owned(),
                message_sha256: sha,
                mars_class: Some(mars_class.to_owned()),
                param_id: u32::from(pds[8]),
                bits_per_value: u32::from(bds[10]),
                binary_scale_factor,
                decimal_scale_factor: sign_magnitude16(pds, 26),
                reference_value,
                bitmap_present,
                forecast_reference_epoch_seconds: reference,
                step_seconds: i64::from(pds[18]) * 3_600,
                source_unit: "m".to_owned(),
                canonical_per_source_unit: CANONICAL_KG_PER_SQUARE_METER_PER_METRE,
                canonical_conversion: F64_DECODE_UNIT_SCALE_THEN_F32_NEAREST.to_owned(),
            },
        },
        decoded_m,
    }
}

/// SHA-256 of each committed GRIB file, verified against the #216 artifact.
pub fn grib_file_hashes(decision: &Value) -> Vec<(String, String)> {
    let files = decision["consumed"]["grib_files"]
        .as_object()
        .expect("files");
    assert_eq!(files.len(), 4, "four #202 GRIB files");
    files
        .iter()
        .map(|(name, entry)| {
            let bytes = std::fs::read(format!("{GRIB_DIR}/{name}.grib")).expect("GRIB bytes");
            let digest = sha256_hex(&bytes);
            assert_eq!(digest, entry["sha256"].as_str().expect("sha"), "{name}");
            (name.clone(), digest)
        })
        .collect()
}

/// All 32 messages, decoded and cross-checked against `decoding.messages`.
pub fn messages() -> Vec<Message> {
    let decision = decision();
    let class = &decision["consumed"]["source_class"];
    let mars_class = class["mars_class"]
        .as_str()
        .expect("explicit control MARS class");
    assert!(
        class["basis"]
            .as_str()
            .expect("basis")
            .contains("control.marsclass=OD"),
        "MARS class must come from control provenance"
    );
    let mut messages = Vec::new();
    for (name, _) in grib_file_hashes(&decision) {
        let bytes = std::fs::read(format!("{GRIB_DIR}/{name}.grib")).expect("GRIB bytes");
        let mut pos = 0;
        while pos < bytes.len() {
            let total = u24(&bytes, pos + 4);
            messages.push(parse_message(&name, &bytes[pos..pos + total], mars_class));
            pos += total;
        }
    }
    let frozen = decision["decoding"]["messages"]
        .as_array()
        .expect("messages");
    assert_eq!(messages.len(), frozen.len(), "message count");
    for message in &messages {
        let p = &message.record.packing;
        let entry = frozen
            .iter()
            .find(|m| m["message_sha256"] == p.message_sha256.as_str())
            .expect("message listed by the #216 artifact");
        assert_eq!(entry["file"], message.file.as_str());
        assert_eq!(entry["packingType"], "grid_simple");
        assert_eq!(entry["eccodes_units"], p.source_unit.as_str());
        assert_eq!(entry["paramId"].as_u64(), Some(u64::from(p.param_id)));
        assert_eq!(
            entry["bitsPerValue"].as_u64(),
            Some(u64::from(p.bits_per_value))
        );
        assert_eq!(
            entry["binaryScaleFactor"].as_i64(),
            Some(i64::from(p.binary_scale_factor))
        );
        assert_eq!(
            entry["decimalScaleFactor"].as_i64(),
            Some(i64::from(p.decimal_scale_factor))
        );
        assert_eq!(entry["referenceValue"].as_f64(), Some(p.reference_value));
        assert_eq!(entry["bitmap_present"].as_bool(), Some(p.bitmap_present));
        assert_eq!(
            entry["forecast_reference_s"].as_i64(),
            Some(p.forecast_reference_epoch_seconds - ORACLE_EPOCH_UNIX_S)
        );
        assert_eq!(entry["step_hours"].as_i64(), Some(p.step_seconds / 3_600));
        let frozen_decoded: Vec<f64> = entry["decoded_m"]
            .as_array()
            .expect("decoded")
            .iter()
            .map(|v| v.as_f64().expect("number"))
            .collect();
        assert_eq!(frozen_decoded, message.decoded_m, "decoded values");
    }
    messages
}

/// Canonical cell `j * NX + i` (j = 0 south) to the GRIB north-first index.
pub const fn grib_index(cell: usize) -> usize {
    (NY - 1 - cell / NX) * NX + cell % NX
}

#[allow(clippy::cast_possible_truncation)]
pub fn canonical_kg_per_m2(metres: f64) -> f32 {
    (metres / FLEX_EXTRACT_METRE_FACTOR) as f32
}

/// One genuine observation time with both parameters' fields.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Declared reset, Unix seconds.
    pub reset_s: i64,
    /// Valid time, Unix seconds.
    pub valid_s: i64,
    /// `(values, record)` for LSP then CP.
    pub parameters: [(Vec<f32>, SourceQuantization); 2],
}

/// The 16 genuine observations in oracle order, values bound to their messages.
pub fn observations() -> Vec<Observation> {
    let oracle = oracle();
    let messages = messages();
    let records = |name: &str| {
        oracle["genuine"]["accumulated"][name]
            .as_array()
            .expect("obs")
            .clone()
    };
    let (lsp, cp) = (records("lsp"), records("cp"));
    assert_eq!(lsp.len(), 16);
    lsp.iter()
        .zip(&cp)
        .map(|(l, c)| {
            let reset_s = l["reset_s"].as_i64().expect("reset") + ORACLE_EPOCH_UNIX_S;
            let valid_s = l["valid_s"].as_i64().expect("valid") + ORACLE_EPOCH_UNIX_S;
            let parameter = |record: &Value, param: u32| {
                assert_eq!(
                    record["reset_s"].as_i64().map(|r| r + ORACLE_EPOCH_UNIX_S),
                    Some(reset_s)
                );
                let message = messages
                    .iter()
                    .find(|m| {
                        let p = &m.record.packing;
                        p.param_id == param
                            && p.forecast_reference_epoch_seconds == reset_s
                            && p.forecast_reference_epoch_seconds + p.step_seconds == valid_s
                    })
                    .expect("message of the observation");
                let values_m: Vec<f64> = record["values_m"]
                    .as_array()
                    .expect("values")
                    .iter()
                    .map(|v| v.as_f64().expect("number"))
                    .collect();
                let values = (0..NX * NY)
                    .map(|cell| {
                        let decoded = message.decoded_m[grib_index(cell)];
                        assert_eq!(decoded, values_m[cell], "oracle value bound to message");
                        canonical_kg_per_m2(decoded)
                    })
                    .collect();
                (values, message.record.clone())
            };
            Observation {
                reset_s,
                valid_s,
                parameters: [parameter(l, 142), parameter(c, 143)],
            }
        })
        .collect()
}

fn base_snapshot() -> (Snapshot, Field) {
    let mut snapshot: Snapshot = serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-column-v1.json"
    ))
    .expect("base snapshot");
    let mut template = snapshot
        .fields
        .iter()
        .find(|f| f.id == FieldId::SurfacePressure)
        .expect("surface template")
        .clone();
    // The hybrid coordinate requires surface pressure; #90/#207 never read it.
    let mut pressure = template.clone();
    pressure.shape = vec![NX, NY];
    pressure.values = vec![pressure.values[0]; NX * NY];
    snapshot.fields = vec![pressure];
    let grid = &mut snapshot.horizontal_grid;
    // GRIB GDS: La1 52N, Lo1 10E, 1 degree, scanning north to south; south row 50N.
    (
        grid.nx,
        grid.ny,
        grid.xlon0_deg,
        grid.ylat0_deg,
        grid.dx_deg,
        grid.dy_deg,
    ) = (NX, NY, 10.0, 50.0, 1.0, 1.0);
    template.shape = vec![NX, NY];
    template.unit = Unit::KilogramPerSquareMeter;
    (snapshot, template)
}

/// Canonical snapshots, one per observation, each with LSP and CP fields.
pub fn snapshots(observations: &[Observation]) -> Vec<Snapshot> {
    let (base, template) = base_snapshot();
    observations
        .iter()
        .map(|observation| {
            let mut snapshot = base.clone();
            snapshot.fields[0].time.valid_time_epoch_seconds = observation.valid_s;
            for (id, (values, record)) in [
                FieldId::LargeScalePrecipitation,
                FieldId::ConvectivePrecipitation,
            ]
            .into_iter()
            .zip(&observation.parameters)
            {
                let mut field = template.clone();
                field.id = id;
                field.time = FieldTime {
                    calendar: Calendar::Gregorian,
                    kind: TemporalKind::AccumulatedSinceReset,
                    valid_time_epoch_seconds: observation.valid_s,
                    interval_start_epoch_seconds: Some(observation.reset_s),
                    interval_end_epoch_seconds: Some(observation.valid_s),
                    accumulation: Some(Accumulation {
                        reset_epoch_seconds: observation.reset_s,
                    }),
                };
                field.values.clone_from(values);
                field.source_quantization = Some(record.clone());
                snapshot.fields.push(field);
            }
            snapshot
        })
        .collect()
}

/// `(parameter index, cell, interval index)` of the three #216 admitted rows.
pub fn admitted_rows(decision: &Value) -> Vec<(usize, usize, usize)> {
    decision["offending_rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            (
                usize::from(row["parameter"] == "cp"),
                usize::try_from(row["canonical_cell"].as_u64().expect("cell")).expect("cell"),
                usize::try_from(row["current"]["observation_index"].as_u64().expect("index"))
                    .expect("index"),
            )
        })
        .collect()
}
