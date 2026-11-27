//! Admit the checksum-pinned ERA5 Complete lnsp companion to #237.

use super::{
    check, decode_model_analysis, narrow, read_hybrid_values, sha256, validate_frames,
    validate_header, validate_parameter_codes, Analysis, Parameter, SourceRequest, MESSAGE_BYTES,
    NX, NY,
};
use crate::io::era5_etex::DecodeError;
use crate::meteorology::{
    Axis, Field, FieldId, HorizontalStaggering, Requirements, SignConvention, Snapshot,
    StorageOrder, Unit, VerticalStaggering,
};
use eccodes::{CodesFile, FallibleIterator, KeyRead, ProductKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub(super) const FIRST_REQUEST: &[u8] =
    include_bytes!("../../../../fixtures/etex/native-mini/era5-19941023-151821-lnsp.request.json");
pub(super) const SECOND_REQUEST: &[u8] =
    include_bytes!("../../../../fixtures/etex/native-mini/era5-19941024-000306-lnsp.request.json");
const LN_SP: Parameter = Parameter {
    param_id: 152,
    category: 3,
    number: 25,
    units: "Numeric",
    canonical: Some(FieldId::SurfacePressure),
};
// Older ecCodes definitions label the same logarithmic parameter as "~".
const LN_SP_UNIT_LABELS: [&str; 2] = [LN_SP.units, "~"];
// A deliberately broad sanity envelope for this ETEX region, including the Alps.
const MIN_PRESSURE_PA: f64 = 50_000.0;
const MAX_PRESSURE_PA: f64 = 110_000.0;

#[derive(Deserialize, Serialize)]
struct SurfaceRequest {
    #[serde(flatten)]
    source: SourceRequest,
    model_request_sha256: String,
    model_sha256: String,
}

impl SurfaceRequest {
    fn verify_lineage(&self, model_request: &[u8]) -> Result<(), DecodeError> {
        let model: SourceRequest = serde_json::from_slice(model_request)?;
        let mut expected = model.request;
        expected.insert("levelist".into(), "1".into());
        expected.insert("param".into(), LN_SP.param_id.to_string());
        if self.model_request_sha256 != sha256(model_request)
            || self.model_sha256 != model.sha256
            || self.source.source != model.source
            || self.source.dataset != model.dataset
            || self.source.request != expected
        {
            return Err(DecodeError::SurfaceLineage);
        }
        Ok(())
    }
}

/// One validated ERA5 ETEX native-level Snapshot and its source-pair evidence.
#[derive(Debug, Serialize)]
pub struct VerifiedSnapshot {
    snapshot: Snapshot,
    provenance: Value,
}

impl VerifiedSnapshot {
    /// Access canonical U, V, T, q and surface pressure with all native levels.
    #[must_use]
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Take ownership of the validated canonical meteorology snapshot.
    #[must_use]
    pub fn into_snapshot(self) -> Snapshot {
        self.snapshot
    }

    /// Inspect the verified source-pair and serialized snapshot identities.
    #[must_use]
    pub fn provenance(&self) -> &Value {
        &self.provenance
    }
}

/// Decode the pinned model and lnsp files for one ETEX analysis.
///
/// # Errors
/// Rejects unsupported times, unverified file identities or source lineage,
/// invalid GRIB structure, metadata, coverage or pressure, and invalid snapshots.
pub fn load_snapshot_with_surface(
    model_path: &Path,
    surface_path: &Path,
    valid_time_epoch_seconds: i64,
) -> Result<VerifiedSnapshot, DecodeError> {
    let analysis = Analysis::select(valid_time_epoch_seconds)?;
    let request: SurfaceRequest = serde_json::from_slice(analysis.surface_request)?;
    request.verify_lineage(analysis.model_request)?;
    let source = &request.source;
    let expected_bytes = analysis.times.len() * MESSAGE_BYTES;
    let metadata = surface_path.metadata().map_err(DecodeError::SurfaceIo)?;
    if !metadata.is_file() || metadata.len() != expected_bytes as u64 {
        return Err(DecodeError::SurfaceSourceIdentity);
    }
    let mut bytes = Vec::with_capacity(expected_bytes);
    File::open(surface_path)
        .and_then(|file| file.take(expected_bytes as u64 + 1).read_to_end(&mut bytes))
        .map_err(DecodeError::SurfaceIo)?;
    if bytes.len() != expected_bytes
        || bytes.len() != source.bytes
        || sha256(&bytes) != source.sha256
    {
        return Err(DecodeError::SurfaceSourceIdentity);
    }

    let (surface_pressure, units) = decode_pressure(bytes, &analysis)?;
    let diagnostic = decode_model_analysis(model_path, &analysis)?;
    let mut snapshot = diagnostic.model_levels;
    let time = snapshot
        .fields
        .first()
        .ok_or(DecodeError::Coverage)?
        .time
        .clone();
    let id = FieldId::SurfacePressure;
    snapshot.fields.push(Field {
        id,
        shape: vec![NX, NY],
        axis_order: vec![Axis::X, Axis::Y],
        storage_order: StorageOrder::XFastest,
        unit: Unit::Pascal,
        sign: SignConvention::NonNegative,
        horizontal_staggering: HorizontalStaggering::CellCenter,
        vertical_staggering: VerticalStaggering::NotApplicable,
        time,
        values: surface_pressure,
        source_quantization: None,
    });
    snapshot.validate(&Requirements::real_data_native_levels())?;
    let payload = serde_json::to_vec(&snapshot)?;
    let provenance = json!({
        "profile": "era5-etex-native-lnsp-pair-v1",
        "canonical_schema": snapshot.schema,
        "valid_time_epoch_seconds": valid_time_epoch_seconds,
        "model_levels": diagnostic.provenance,
        "surface_pressure": {
            "path_display": surface_path.display().to_string(),
            "request_record": request,
            "request_record_sha256": sha256(analysis.surface_request),
            "shared_grib_keys": "model_levels.grib.integer_keys and string_keys",
            "parameter": LN_SP,
            "accepted_eccodes_unit_labels": LN_SP_UNIT_LABELS,
            "eccodes_unit_label": units,
            "encoded_level": 1,
            "validated_message_count": analysis.times.len(),
            "normalization": [
                "Same Gregorian UTC analysis and canonical grid order as model levels.",
                "Natural logarithm of pressure in Pa: exp in binary64, then narrow to binary32.",
                "No interpolation, clipping or filling."
            ],
            "pressure_range_pa_inclusive": [MIN_PRESSURE_PA, MAX_PRESSURE_PA],
            "decoder_source_sha256": sha256(include_bytes!("surface.rs"))
        },
        "payload": {
            "encoding": "serde_json::to_vec(snapshot), compact UTF-8 without newline",
            "bytes": payload.len(),
            "sha256": sha256(&payload)
        }
    });
    Ok(VerifiedSnapshot {
        snapshot,
        provenance,
    })
}

fn decode_pressure(bytes: Vec<u8>, analysis: &Analysis) -> Result<(Vec<f32>, String), DecodeError> {
    validate_frames(&bytes)?;
    if bytes.is_empty() {
        return Err(DecodeError::SurfaceCoverage);
    }
    let mut file = CodesFile::new_from_memory(bytes, ProductKind::GRIB)?;
    let mut messages = file.ref_message_iter();
    let mut pressures = BTreeMap::new();
    let mut pv = None;
    while let Some(message) = messages.next()? {
        let ordinal = pressures.len() + 1;
        let time = validate_header(&message, ordinal, analysis.date)?;
        if !analysis.times.contains(&time) || pressures.contains_key(&time) {
            return Err(DecodeError::SurfaceRecord(time));
        }
        check(
            ordinal,
            "paramId",
            KeyRead::<i64>::read_key(&message, "paramId")?,
            LN_SP.param_id,
        )?;
        check(
            ordinal,
            "level",
            KeyRead::<i64>::read_key(&message, "level")?,
            1,
        )?;
        validate_parameter_codes(&message, ordinal, &LN_SP)?;
        let units = KeyRead::<String>::read_key(&message, "units")?;
        validate_lnsp_units(ordinal, &units)?;
        let source = read_hybrid_values(&message, ordinal, 1, &mut pv)?;
        let pa = source
            .into_iter()
            .enumerate()
            .map(|(cell, lnsp)| pressure_pa(lnsp, ordinal, cell))
            .collect::<Result<Vec<_>, _>>()?;
        let canonical = pa
            .as_chunks::<NX>()
            .0
            .iter()
            .rev()
            .flatten()
            .copied()
            .collect();
        pressures.insert(time, (canonical, units));
    }
    if pressures.len() != analysis.times.len() {
        return Err(DecodeError::SurfaceCoverage);
    }
    pressures
        .remove(&analysis.hhmm)
        .ok_or(DecodeError::SurfaceCoverage)
}

fn validate_lnsp_units(ordinal: usize, actual: &str) -> Result<(), DecodeError> {
    if LN_SP_UNIT_LABELS.contains(&actual) {
        return Ok(());
    }
    Err(DecodeError::Key {
        message: ordinal,
        key: "units",
        expected: LN_SP_UNIT_LABELS.join(" or "),
        actual: actual.to_owned(),
    })
}

fn pressure_pa(lnsp: f64, message: usize, cell: usize) -> Result<f32, DecodeError> {
    let pa = lnsp.exp();
    if !(MIN_PRESSURE_PA..=MAX_PRESSURE_PA).contains(&pa) {
        return Err(DecodeError::SurfacePressureValue { message, cell });
    }
    Ok(narrow(pa))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::era5_etex::native::tests::section_offset;

    const RAW: &[u8] =
        include_bytes!("../../../../fixtures/etex/native-mini/era5-19941023-151821-lnsp.grib");

    #[test]
    fn test_era5_etex_lnsp_rejects_unverified_lineage() {
        let analysis = Analysis::select(782_924_400).unwrap();
        let load = || serde_json::from_slice::<SurfaceRequest>(analysis.surface_request).unwrap();
        load().verify_lineage(analysis.model_request).unwrap();

        for field in ["model_request_sha256", "model_sha256", "dataset", "time"] {
            let mut request = load();
            match field {
                "model_request_sha256" => request.model_request_sha256.clear(),
                "model_sha256" => request.model_sha256.clear(),
                "dataset" => request.source.dataset = "reanalysis-era5-single-levels".into(),
                "time" => {
                    request.source.request.insert("time".into(), "15/18".into());
                }
                _ => unreachable!(),
            }
            assert!(matches!(
                request.verify_lineage(analysis.model_request),
                Err(DecodeError::SurfaceLineage)
            ));
        }
    }

    #[test]
    fn test_era5_etex_lnsp_rejects_invalid_message_metadata() {
        let analysis = Analysis::select(782_924_400).unwrap();
        assert_eq!(
            decode_pressure(RAW.to_vec(), &analysis).unwrap().0.len(),
            NX * NY
        );
        for (section, byte, value, key) in [
            (1, 14, 13, "dataDate"),
            (1, 15, 32, "dataDate"),
            (1, 20, 1, "typeOfProcessedData"),
            (2, 8, 1, "marsClass"),
            (2, 10, 9, "marsType"),
            (2, 12, 2, "marsStream"),
            (2, 16, b'5', "experimentVersionNumber"),
            (3, 71, 64, "scanningMode"),
            (3, 69, 0, "jDirectionIncrement"),
            (4, 10, 0, "paramId"),
            (4, 21, 1, "startStep"),
            (4, 27, 2, "level"),
            (4, 37, 1, "pv SHA-256"),
        ] {
            let mut bytes = RAW.to_vec();
            let offset = section_offset(&bytes, section);
            bytes[offset + byte] = value;
            let error = decode_pressure(bytes, &analysis).unwrap_err();
            assert!(
                matches!(error, DecodeError::Key { key: actual, .. } if actual == key),
                "{key}: {error}"
            );
        }
        let mut wrong_hour = RAW.to_vec();
        let section = section_offset(&wrong_hour, 1);
        wrong_hour[section + 16] = 14;
        assert!(matches!(
            decode_pressure(wrong_hour, &analysis),
            Err(DecodeError::SurfaceRecord(1400))
        ));
    }

    #[test]
    fn test_era5_etex_lnsp_rejects_incompatible_unit() {
        let mut file =
            CodesFile::new_from_memory(RAW[..MESSAGE_BYTES].to_vec(), ProductKind::GRIB).unwrap();
        let message = file.ref_message_iter().next().unwrap().unwrap();
        let units = KeyRead::<String>::read_key(&message, "units").unwrap();
        validate_lnsp_units(1, &units).unwrap();
        assert!(matches!(
            validate_lnsp_units(1, "Pa"),
            Err(DecodeError::Key { key: "units", .. })
        ));
    }

    #[test]
    fn test_era5_etex_lnsp_rejects_corruption_coverage_and_invalid_values() {
        let analysis = Analysis::select(782_924_400).unwrap();
        let decode = |bytes| decode_pressure(bytes, &analysis);
        assert!(matches!(
            decode(RAW[..RAW.len() - 1].to_vec()),
            Err(DecodeError::Frame(_))
        ));
        assert!(matches!(
            decode(RAW[..2 * MESSAGE_BYTES].to_vec()),
            Err(DecodeError::SurfaceCoverage)
        ));
        let mut extra = RAW.to_vec();
        extra.extend_from_slice(&RAW[..MESSAGE_BYTES]);
        assert!(matches!(
            decode(extra),
            Err(DecodeError::SurfaceRecord(1500))
        ));

        // Lower the GRIB simple-packing reference value without changing the grid.
        let mut invalid = RAW.to_vec();
        let section = section_offset(&invalid, 5);
        invalid[section + 11..section + 15].copy_from_slice(&0_f32.to_be_bytes());
        assert!(matches!(
            decode(invalid),
            Err(DecodeError::SurfacePressureValue { message: 1, .. })
        ));
        for lnsp in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            1000.0,
            12.0,
        ] {
            assert!(matches!(
                pressure_pa(lnsp, 1, 0),
                Err(DecodeError::SurfacePressureValue { .. })
            ));
        }
    }
}
