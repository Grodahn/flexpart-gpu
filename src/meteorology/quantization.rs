//! Optional per-field source-quantization provenance (#29 carrier for #216/#219).
//!
//! A canonical [`super::Field`] normally carries only normalized `f32` values,
//! unit, sign and time. Provider-side GRIB packing quantizes every message
//! independently, so a flat accumulation can decode as a small decrease
//! (`docs/research/grib-packing-accumulation-216.md`). Deciding whether such a
//! decrease is compatible with the source representation needs the packing keys
//! of the original message. This record carries exactly the keys frozen by the
//! #216 decision artifact (`fixtures/accumulation/grib-packing-decrease-decision-v1.json`,
//! `negative_controls[*].pair.{previous,current}`), with the same JSON key names.
//!
//! The record is optional and absent for every input that existing providers
//! produce. It is never synthesized from canonical values: a provider (#32) or an
//! explicitly verified test fixture must supply every key, including the MARS
//! class, which comes from provider/control provenance and is never inferred from
//! the packing keys. This module validates only the record's shape (hash format,
//! finite reference value, GRIB format ranges). Whether a record admits a decrease
//! is decided by [`super::accumulation::classify_packing_decrease`], which keeps
//! the frozen #216 predicate and rejection order.

use serde::{Deserialize, Serialize};

use super::{ContractError, FieldId};

/// Encoding identity of GRIB edition 1 grid-point simple packing (ecCodes `grid_simple`).
pub const GRIB1_GRID_SIMPLE_ENCODING: &str = "grib1_grid_simple";
/// Declared canonical conversion of the #216 supported class: IEEE double decode,
/// unit scale in double, one round-to-nearest to `f32`.
pub const F64_DECODE_UNIT_SCALE_THEN_F32_NEAREST: &str = "f64_decode_unit_scale_then_f32_nearest";
/// MARS class of the exercised #202 deaccumulation path (`control.marsclass = "OD"`).
pub const MARS_CLASS_OPERATIONAL: &str = "OD";
/// GRIB1 source unit of ECMWF table 128 parameters 142/143 (metres of water equivalent).
pub const SOURCE_UNIT_METRE: &str = "m";
/// Canonical kg/m2 per metre of water equivalent.
pub const CANONICAL_KG_PER_SQUARE_METER_PER_METRE: u32 = 1000;
/// ECMWF table 128 `paramId` of large-scale precipitation (LSP).
pub const LARGE_SCALE_PRECIPITATION_PARAM_ID: u32 = 142;
/// ECMWF table 128 `paramId` of convective precipitation (CP).
pub const CONVECTIVE_PRECIPITATION_PARAM_ID: u32 = 143;

/// Largest magnitude of a GRIB sign-and-magnitude 16-bit scale factor (E or D).
const GRIB_SCALE_FACTOR_MAGNITUDE_LIMIT: i32 = 32_767;
/// `bitsPerValue` is one octet in GRIB editions 1 and 2.
const GRIB_BITS_PER_VALUE_LIMIT: u32 = 255;
/// Length of a lowercase hexadecimal SHA-256 digest.
const SHA256_HEX_LENGTH: usize = 64;

/// Packing keys of the original source message (frozen #216 `packing` record).
///
/// Values are copied from the message; none is defaulted. `mars_class` may be
/// absent, which the #216 rule treats as an unsupported source class.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GribPackingProvenance {
    /// Packing identity, e.g. [`GRIB1_GRID_SIMPLE_ENCODING`].
    pub encoding: String,
    /// SHA-256 of the exact source message bytes this record describes.
    pub message_sha256: String,
    /// MARS class from explicit provider/control provenance, never inferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mars_class: Option<String>,
    /// GRIB parameter identity (ECMWF table 128 `paramId`).
    #[serde(rename = "paramId")]
    pub param_id: u32,
    /// Packed bits per value.
    #[serde(rename = "bitsPerValue")]
    pub bits_per_value: u32,
    /// Binary scale factor `E`; the level spacing is `2^E * 10^-D` source units.
    #[serde(rename = "binaryScaleFactor")]
    pub binary_scale_factor: i32,
    /// Decimal scale factor `D`.
    #[serde(rename = "decimalScaleFactor")]
    pub decimal_scale_factor: i32,
    /// GRIB packing reference value `R` in source units (not a reset time).
    #[serde(rename = "referenceValue")]
    pub reference_value: f64,
    /// Whether the message carries a bitmap / missing-value representation.
    pub bitmap_present: bool,
    /// Forecast reference time in epoch seconds.
    #[serde(rename = "forecast_reference_s")]
    pub forecast_reference_epoch_seconds: i64,
    /// Forecast step in seconds; reference + step is the message validity time.
    #[serde(rename = "step_s")]
    pub step_seconds: i64,
    /// Physical unit of the decoded source values, e.g. [`SOURCE_UNIT_METRE`].
    pub source_unit: String,
    /// Canonical units per source unit applied by the declared conversion.
    pub canonical_per_source_unit: u32,
    /// Declared canonical conversion, e.g. [`F64_DECODE_UNIT_SCALE_THEN_F32_NEAREST`].
    pub canonical_conversion: String,
}

/// Source/observation identity and packing provenance of one canonical field.
///
/// Mirrors the observation side of a #216 pair: `source_id` and
/// `source_message_sha256` identify the observation that produced the field's
/// values; `packing.message_sha256` names the message the packing keys belong to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceQuantization {
    /// Provider source identity of the observation, e.g. `ecmf-table128-param142`.
    pub source_id: String,
    /// SHA-256 of the source message that produced this field's values.
    pub source_message_sha256: String,
    /// Packing keys of that message.
    pub packing: GribPackingProvenance,
}

/// Reject a malformed record at the canonical boundary.
///
/// Only the record's shape is checked: identity strings, digest format, finite
/// reference value and GRIB format ranges. Supported-class membership is a #216
/// classification result, not a schema error, so unsupported but well-formed
/// records (for example ERA5 class `EA` or GRIB2 packing) pass here and are
/// rejected later if they are ever needed to admit a decrease.
pub(super) fn validate_source_quantization(
    id: FieldId,
    record: &SourceQuantization,
) -> Result<(), ContractError> {
    let packing = &record.packing;
    let invalid = || ContractError::InvalidSourceQuantization(id);
    let identities_valid = is_identity_token(&record.source_id)
        && is_identity_token(&packing.encoding)
        && is_identity_token(&packing.source_unit)
        && is_identity_token(&packing.canonical_conversion)
        && packing.mars_class.as_deref().is_none_or(is_identity_token);
    if !identities_valid
        || !is_sha256_hex(&record.source_message_sha256)
        || !is_sha256_hex(&packing.message_sha256)
    {
        return Err(invalid());
    }
    let ranges_valid = packing.param_id > 0
        && packing.bits_per_value <= GRIB_BITS_PER_VALUE_LIMIT
        && packing.binary_scale_factor.abs() <= GRIB_SCALE_FACTOR_MAGNITUDE_LIMIT
        && packing.decimal_scale_factor.abs() <= GRIB_SCALE_FACTOR_MAGNITUDE_LIMIT
        && packing.reference_value.is_finite()
        && packing.step_seconds >= 0
        && packing
            .forecast_reference_epoch_seconds
            .checked_add(packing.step_seconds)
            .is_some()
        && packing.canonical_per_source_unit > 0;
    if !ranges_valid {
        return Err(invalid());
    }
    Ok(())
}

fn is_identity_token(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == SHA256_HEX_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> SourceQuantization {
        SourceQuantization {
            source_id: "ecmf-table128-param142".to_string(),
            source_message_sha256: "a".repeat(64),
            packing: GribPackingProvenance {
                encoding: GRIB1_GRID_SIMPLE_ENCODING.to_string(),
                message_sha256: "a".repeat(64),
                mars_class: Some(MARS_CLASS_OPERATIONAL.to_string()),
                param_id: LARGE_SCALE_PRECIPITATION_PARAM_ID,
                bits_per_value: 24,
                binary_scale_factor: -32,
                decimal_scale_factor: 0,
                reference_value: 0.0,
                bitmap_present: false,
                forecast_reference_epoch_seconds: 86_400,
                step_seconds: 10_800,
                source_unit: SOURCE_UNIT_METRE.to_string(),
                canonical_per_source_unit: CANONICAL_KG_PER_SQUARE_METER_PER_METRE,
                canonical_conversion: F64_DECODE_UNIT_SCALE_THEN_F32_NEAREST.to_string(),
            },
        }
    }

    fn validate(record: &SourceQuantization) -> Result<(), ContractError> {
        validate_source_quantization(FieldId::LargeScalePrecipitation, record)
    }

    #[test]
    fn test_quantization_well_formed_record_validates() {
        assert_eq!(validate(&record()), Ok(()));
        let mut era5 = record();
        era5.packing.mars_class = Some("EA".to_string());
        assert_eq!(
            validate(&era5),
            Ok(()),
            "class membership is not a schema error"
        );
        let mut absent = record();
        absent.packing.mars_class = None;
        absent.packing.encoding = "grib2_grid_simple".to_string();
        assert_eq!(
            validate(&absent),
            Ok(()),
            "encoding support is not a schema error"
        );
    }

    #[test]
    #[allow(clippy::type_complexity, clippy::semicolon_if_nothing_returned)]
    fn test_quantization_malformed_records_fail_closed() {
        let mutations: [(&str, fn(&mut SourceQuantization)); 14] = [
            ("short hash", |r| r.source_message_sha256.truncate(63)),
            ("uppercase hash", |r| {
                r.packing.message_sha256 = "A".repeat(64)
            }),
            ("non-hex hash", |r| {
                r.packing.message_sha256 = "g".repeat(64)
            }),
            ("empty source", |r| r.source_id.clear()),
            ("padded encoding", |r| {
                r.packing.encoding = " grib1_grid_simple".into()
            }),
            ("empty class", |r| {
                r.packing.mars_class = Some(String::new())
            }),
            ("zero param", |r| r.packing.param_id = 0),
            ("bits octet", |r| r.packing.bits_per_value = 256),
            ("E range", |r| r.packing.binary_scale_factor = -32_768),
            ("D range", |r| r.packing.decimal_scale_factor = 32_768),
            ("NaN R", |r| r.packing.reference_value = f64::NAN),
            ("negative step", |r| r.packing.step_seconds = -1),
            ("step overflow", |r| {
                r.packing.forecast_reference_epoch_seconds = i64::MAX;
            }),
            ("zero scale", |r| r.packing.canonical_per_source_unit = 0),
        ];
        for (what, mutate) in mutations {
            let mut malformed = record();
            mutate(&mut malformed);
            assert_eq!(
                validate(&malformed),
                Err(ContractError::InvalidSourceQuantization(
                    FieldId::LargeScalePrecipitation
                )),
                "{what}"
            );
        }
    }

    #[test]
    fn test_quantization_record_uses_frozen_216_key_names() {
        let value = serde_json::to_value(record()).expect("serialize");
        let mut keys: Vec<_> = value["packing"]
            .as_object()
            .expect("packing object")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "binaryScaleFactor",
                "bitmap_present",
                "bitsPerValue",
                "canonical_conversion",
                "canonical_per_source_unit",
                "decimalScaleFactor",
                "encoding",
                "forecast_reference_s",
                "mars_class",
                "message_sha256",
                "paramId",
                "referenceValue",
                "source_unit",
                "step_s",
            ]
        );
        let unknown = r#"{"source_id":"x","source_message_sha256":"x","packing":{},"extra":1}"#;
        assert!(serde_json::from_str::<SourceQuantization>(unknown).is_err());
    }
}
