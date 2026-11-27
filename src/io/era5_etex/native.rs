use super::DecodeError;
use crate::meteorology::{
    Axis, Calendar, ContractError, Field, FieldId, FieldTime, HorizontalGrid, HorizontalStaggering,
    LongitudeDomain, Requirements, SchemaIdentity, Snapshot, StorageOrder, TemporalKind,
    VerticalCoordinate, VerticalCoordinateKind, VerticalOrdering, VerticalReference,
    VerticalStaggering, FIELD_SPECS,
};
use eccodes::{CodesFile, FallibleIterator, KeyRead, ProductKind, RefMessage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

const PROFILE: &str = "era5-etex-native-model-levels-v1";
const SOURCE_DIR: &str = "fixtures/etex/native-mini";
const FIRST_REQUEST: &[u8] = include_bytes!("../../../fixtures/etex/native-mini/request.json");
const SECOND_REQUEST: &[u8] =
    include_bytes!("../../../fixtures/etex/native-mini/request-next-day.json");
mod surface;
pub use surface::{load_snapshot_with_surface, VerifiedSnapshot};
const NX: usize = 65;
const NY: usize = 41;
const LEVELS: usize = 137;
const INTERFACES: usize = LEVELS + 1;
const REFERENCE_SURFACE_PRESSURE_PA: f32 = 101_325.0;
const PV_SHA256: &str = "0d51f96436ff9872d6e9ea168543e9da1e569bacf0b41c2a2823d33851b3d3a8";
const MESSAGE_BYTES: usize = 6630;
const SOURCE_BYTES: usize = 3 * 5 * LEVELS * MESSAGE_BYTES;
const FIRST_TIMES: [(i64, i64); 3] = [
    (1500, 782_924_400),
    (1800, 782_935_200),
    (2100, 782_946_000),
];
const SECOND_TIMES: [(i64, i64); 3] = [(0, 782_956_800), (300, 782_967_600), (600, 782_978_400)];

const LONG_KEYS: &[(&str, i64)] = &[
    ("edition", 2),
    ("subCentre", 0),
    ("localDefinitionNumber", 1),
    ("generatingProcessIdentifier", 145),
    ("discipline", 0),
    ("tablesVersion", 5),
    ("localTablesVersion", 0),
    ("productionStatusOfProcessedData", 0),
    ("typeOfProcessedData", 0),
    ("significanceOfReferenceTime", 1),
    ("productDefinitionTemplateNumber", 0),
    ("indicatorOfUnitOfTimeRange", 1),
    ("startStep", 0),
    ("endStep", 0),
    ("second", 0),
    ("typeOfFirstFixedSurface", 105),
    ("typeOfSecondFixedSurface", 255),
    ("scaleFactorOfFirstFixedSurface", 0),
    ("NV", 276),
    ("PVPresent", 1),
    ("gridDefinitionTemplateNumber", 0),
    ("sourceOfGridDefinition", 0),
    ("shapeOfTheEarth", 6),
    ("basicAngleOfTheInitialProductionDomain", 0),
    ("subdivisionsOfBasicAngle", 2_147_483_647),
    ("Ni", 65),
    ("Nj", 41),
    ("latitudeOfFirstGridPoint", 53_000_000),
    ("latitudeOfLastGridPoint", 43_000_000),
    ("longitudeOfFirstGridPoint", 352_000_000),
    ("longitudeOfLastGridPoint", 8_000_000),
    ("iDirectionIncrement", 250_000),
    ("jDirectionIncrement", 250_000),
    ("scanningMode", 0),
    ("resolutionAndComponentFlags", 48),
    ("uvRelativeToGrid", 0),
    ("bitmapPresent", 0),
    ("numberOfMissing", 0),
    ("numberOfPoints", 2665),
    ("numberOfValues", 2665),
    ("dataRepresentationTemplateNumber", 0),
    ("bitsPerValue", 16),
    ("decimalScaleFactor", 0),
];

const STRING_KEYS: &[(&str, &str)] = &[
    ("centre", "ecmf"),
    ("marsClass", "ea"),
    ("marsStream", "oper"),
    ("marsType", "an"),
    ("experimentVersionNumber", "0001"),
    ("stepType", "instant"),
    ("typeOfLevel", "hybrid"),
    ("gridType", "regular_ll"),
    ("packingType", "grid_simple"),
];

#[derive(Clone, Copy, Serialize)]
struct Parameter {
    param_id: i64,
    category: i64,
    number: i64,
    units: &'static str,
    canonical: Option<FieldId>,
}

const PARAMETERS: [Parameter; 5] = [
    Parameter {
        param_id: 131,
        category: 2,
        number: 2,
        units: "m s**-1",
        canonical: Some(FieldId::WindU),
    },
    Parameter {
        param_id: 132,
        category: 2,
        number: 3,
        units: "m s**-1",
        canonical: Some(FieldId::WindV),
    },
    Parameter {
        param_id: 130,
        category: 0,
        number: 0,
        units: "K",
        canonical: Some(FieldId::Temperature),
    },
    Parameter {
        param_id: 133,
        category: 1,
        number: 0,
        units: "kg kg**-1",
        canonical: Some(FieldId::SpecificHumidity),
    },
    Parameter {
        param_id: 135,
        category: 2,
        number: 8,
        units: "Pa s**-1",
        canonical: None,
    },
];

#[derive(Deserialize, Serialize)]
struct SourceRequest {
    source: String,
    dataset: String,
    retrieved_utc: String,
    request: BTreeMap<String, String>,
    file: String,
    bytes: usize,
    sha256: String,
}

/// Four validated native fields for diagnostics when surface pressure is unavailable.
/// Its serialization deliberately lacks the top-level canonical Snapshot structure.
#[derive(Debug, Serialize)]
pub struct ModelLevelsDiagnostic {
    kind: &'static str,
    model_levels: Snapshot,
    provenance: Value,
}

impl ModelLevelsDiagnostic {
    /// The four model-level fields in canonical cell order and units.
    #[must_use]
    pub fn fields(&self) -> &[Field] {
        &self.model_levels.fields
    }

    /// Native vertical coefficients and schema-v1 reference pressures.
    #[must_use]
    pub fn vertical_coordinate(&self) -> &VerticalCoordinate {
        &self.model_levels.vertical_coordinate
    }

    /// Full ETEX model grid in canonical longitude and latitude order.
    #[must_use]
    pub fn horizontal_grid(&self) -> &HorizontalGrid {
        &self.model_levels.horizontal_grid
    }
}

/// Decode one of the six pinned ERA5 analyses as a model-level-only diagnostic.
///
/// # Errors
/// Rejects unsupported times, incorrect source bytes, corrupt or unsupported
/// GRIB records, inconsistent metadata, incomplete coverage and invalid values.
pub fn decode_model_levels(
    path: &Path,
    valid_time_epoch_seconds: i64,
) -> Result<ModelLevelsDiagnostic, DecodeError> {
    let mut result = decode_model_analysis(path, &Analysis::select(valid_time_epoch_seconds)?)?;
    result.provenance["surface_pressure"] = json!({
        "status": "unavailable_unverified_cross_source_join",
        "ancillary_inputs_consumed": []
    });
    Ok(result)
}

struct Analysis {
    model_request: &'static [u8],
    surface_request: &'static [u8],
    date: i64,
    times: [i64; 3],
    hhmm: i64,
    epoch: i64,
}

impl Analysis {
    fn select(epoch: i64) -> Result<Self, DecodeError> {
        let (model_request, surface_request, date, times) =
            if FIRST_TIMES.iter().any(|&(_, candidate)| candidate == epoch) {
                (
                    FIRST_REQUEST,
                    surface::FIRST_REQUEST,
                    19_941_023,
                    FIRST_TIMES,
                )
            } else {
                (
                    SECOND_REQUEST,
                    surface::SECOND_REQUEST,
                    19_941_024,
                    SECOND_TIMES,
                )
            };
        let &(hhmm, _) = times
            .iter()
            .find(|&&(_, candidate)| candidate == epoch)
            .ok_or(DecodeError::UnsupportedTime(epoch))?;
        Ok(Self {
            model_request,
            surface_request,
            date,
            times: times.map(|(time, _)| time),
            hhmm,
            epoch,
        })
    }
}

fn decode_model_analysis(
    path: &Path,
    analysis: &Analysis,
) -> Result<ModelLevelsDiagnostic, DecodeError> {
    let Analysis {
        model_request: request_bytes,
        date,
        times,
        hhmm,
        epoch,
        ..
    } = *analysis;
    let request: SourceRequest = serde_json::from_slice(request_bytes)?;
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() != SOURCE_BYTES as u64 {
        return Err(DecodeError::SourceIdentity);
    }
    let mut bytes = Vec::with_capacity(SOURCE_BYTES);
    File::open(path)?
        .take(SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let digest = sha256(&bytes);
    if bytes.len() != SOURCE_BYTES || bytes.len() != request.bytes || digest != request.sha256 {
        return Err(DecodeError::SourceIdentity);
    }
    let model_levels = decode_verified(bytes, date, &times, hhmm, epoch)?;
    let payload = serde_json::to_vec(&model_levels)?;
    // SAFETY: This ecCodes function takes no arguments and returns the linked library version by value.
    let eccodes_api_version = unsafe { eccodes_sys::codes_get_api_version() };
    let provenance = json!({
        "profile": PROFILE,
        "canonical_schema": model_levels.schema,
        "raw_source": {
            "path_display": path.display().to_string(),
            "fixture_directory": SOURCE_DIR,
            "verified_bytes": request.bytes,
            "verified_sha256": digest,
            "request_record": request,
            "request_record_sha256": sha256(request_bytes)
        },
        "toolchain": {
            "crate_version": env!("CARGO_PKG_VERSION"),
            "decoder_source_sha256": sha256(include_bytes!("native.rs")),
            "dependency_lock_sha256": sha256(include_bytes!("../../../Cargo.lock")),
            "eccodes_api_version": eccodes_api_version,
            "target_os": std::env::consts::OS,
            "target_arch": std::env::consts::ARCH
        },
        "grib": {
            "integer_keys": LONG_KEYS.iter().copied().collect::<BTreeMap<_, _>>(),
            "string_keys": STRING_KEYS.iter().copied().collect::<BTreeMap<_, _>>(),
            "parameter_map": PARAMETERS,
            "dataDate": date, "validityDate": date,
            "dataTime": hhmm, "validityTime": hhmm,
            "accepted_file_times_hhmm": times,
            "levels": [1, LEVELS],
            "pv_f64_le_sha256": PV_SHA256,
            "validated_message_count": 3 * PARAMETERS.len() * LEVELS
        },
        "normalization": [
            "Gregorian GRIB calendar dates and UTC hours to Unix epoch seconds",
            "352-degree west edge mapped to -8; rows reversed from north-first to south-first",
            "X-fastest, model level 1 first; earth-relative U/V preserved without sign or unit conversion",
            "ecCodes f64 values and PV narrowed to canonical IEEE-754 f32; no filling, clipping or interpolation",
            "A+B*101325 and adjacent-interface means are schema reference metadata only",
            "pressure velocity (135) checked as a source record and omitted from output"
        ],
        "payload": {
            "encoding": "serde_json::to_vec(model_levels), compact UTF-8 without newline",
            "bytes": payload.len(),
            "sha256": sha256(&payload)
        }
    });
    Ok(ModelLevelsDiagnostic {
        kind: "model_levels_only_diagnostic",
        model_levels,
        provenance,
    })
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_frames(bytes: &[u8]) -> Result<(), DecodeError> {
    for (index, frame) in bytes.chunks(MESSAGE_BYTES).enumerate() {
        if frame.len() != MESSAGE_BYTES
            || &frame[..4] != b"GRIB"
            || frame[7] != 2
            || frame[8..16] != (MESSAGE_BYTES as u64).to_be_bytes()
            || !frame.ends_with(b"7777")
        {
            return Err(DecodeError::Frame(index * MESSAGE_BYTES));
        }
    }
    Ok(())
}

fn check<T: Copy + PartialEq + ToString>(
    ordinal: usize,
    key: &'static str,
    actual: T,
    expected: T,
) -> Result<(), DecodeError> {
    if actual == expected {
        return Ok(());
    }
    Err(DecodeError::Key {
        message: ordinal,
        key,
        expected: expected.to_string(),
        actual: actual.to_string(),
    })
}

fn validate_header(
    message: &RefMessage<'_>,
    ordinal: usize,
    date: i64,
) -> Result<i64, DecodeError> {
    let long = |key| {
        // ecCodes exposes these coded table values with string labels by default.
        if matches!(key, "typeOfProcessedData" | "typeOfFirstFixedSurface") {
            KeyRead::<i64>::read_key_unchecked(message, key)
        } else {
            KeyRead::<i64>::read_key(message, key)
        }
    };
    let string = |key| KeyRead::<String>::read_key(message, key);
    for &(key, expected) in LONG_KEYS {
        check(ordinal, key, long(key)?, expected)?;
    }
    for &(key, expected) in STRING_KEYS {
        check(ordinal, key, string(key)?.as_str(), expected)?;
    }
    let time = long("dataTime")?;
    check(ordinal, "dataDate", long("dataDate")?, date)?;
    check(ordinal, "validityDate", long("validityDate")?, date)?;
    check(ordinal, "validityTime", long("validityTime")?, time)?;
    Ok(time)
}

fn read_record(
    message: &RefMessage<'_>,
    ordinal: usize,
    date: i64,
    times: &[i64; 3],
    seen: &mut BTreeSet<(i64, i64, usize)>,
    pv_anchor: &mut Option<Vec<f64>>,
) -> Result<(i64, Parameter, usize, Vec<f64>), DecodeError> {
    let time = validate_header(message, ordinal, date)?;
    let long = |key| KeyRead::<i64>::read_key(message, key);
    let param_id = long("paramId")?;
    let raw_level = long("level")?;
    let record_error = || DecodeError::Record {
        time,
        parameter: param_id,
        level: raw_level,
    };
    let parameter = PARAMETERS
        .iter()
        .find(|entry| entry.param_id == param_id)
        .ok_or_else(record_error)?;
    let level = usize::try_from(raw_level).map_err(|_| record_error())?;
    if !times.contains(&time)
        || !(1..=LEVELS).contains(&level)
        || !seen.insert((time, param_id, level))
    {
        return Err(record_error());
    }
    validate_parameter(message, ordinal, parameter)?;
    let values = read_hybrid_values(message, ordinal, raw_level, pv_anchor)?;
    check(
        ordinal,
        "valid values",
        values.iter().copied().all(|value| {
            value.is_finite()
                && value.abs() <= f64::from(f32::MAX)
                && (param_id != 133 || (0.0..=1.0).contains(&value))
                && (param_id != 130 || value > 0.0)
        }),
        true,
    )?;
    Ok((time, *parameter, level, values))
}

fn read_hybrid_values(
    message: &RefMessage<'_>,
    ordinal: usize,
    level: i64,
    pv_anchor: &mut Option<Vec<f64>>,
) -> Result<Vec<f64>, DecodeError> {
    check(
        ordinal,
        "scaledValueOfFirstFixedSurface",
        KeyRead::<i64>::read_key(message, "scaledValueOfFirstFixedSurface")?,
        level,
    )?;
    let pv = KeyRead::<Vec<f64>>::read_key(message, "pv")?;
    check(ordinal, "pv length", pv.len(), 2 * INTERFACES)?;
    if let Some(expected) = pv_anchor {
        let identical = pv
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.to_bits() == expected.to_bits());
        check(ordinal, "pv matches first message", identical, true)?;
    } else {
        let bytes: Vec<_> = pv
            .iter()
            .flat_map(|coefficient| coefficient.to_le_bytes())
            .collect();
        check(ordinal, "pv SHA-256", sha256(&bytes).as_str(), PV_SHA256)?;
        *pv_anchor = Some(pv);
    }
    let values = KeyRead::<Vec<f64>>::read_key(message, "values")?;
    check(ordinal, "values length", values.len(), NX * NY)?;
    Ok(values)
}

fn validate_parameter(
    message: &RefMessage<'_>,
    ordinal: usize,
    parameter: &Parameter,
) -> Result<(), DecodeError> {
    validate_parameter_codes(message, ordinal, parameter)?;
    check(
        ordinal,
        "units",
        KeyRead::<String>::read_key(message, "units")?.as_str(),
        parameter.units,
    )
}

fn validate_parameter_codes(
    message: &RefMessage<'_>,
    ordinal: usize,
    parameter: &Parameter,
) -> Result<(), DecodeError> {
    check(
        ordinal,
        "parameterCategory",
        KeyRead::<i64>::read_key(message, "parameterCategory")?,
        parameter.category,
    )?;
    check(
        ordinal,
        "parameterNumber",
        KeyRead::<i64>::read_key(message, "parameterNumber")?,
        parameter.number,
    )
}

fn decode_verified(
    bytes: Vec<u8>,
    date: i64,
    times: &[i64; 3],
    hhmm: i64,
    epoch: i64,
) -> Result<Snapshot, DecodeError> {
    validate_frames(&bytes)?;
    let mut file = CodesFile::new_from_memory(bytes, ProductKind::GRIB)?;
    let mut messages = file.ref_message_iter();
    let mut seen = BTreeSet::new();
    let mut pv = None;
    let mut selected = BTreeMap::new();
    while let Some(message) = messages.next()? {
        let (time, parameter, level, values) =
            read_record(&message, seen.len() + 1, date, times, &mut seen, &mut pv)?;
        if time == hhmm && parameter.canonical.is_some() {
            selected.insert((parameter.param_id, level), values);
        }
    }
    if seen.len() != times.len() * PARAMETERS.len() * LEVELS {
        return Err(DecodeError::Coverage);
    }

    let time = FieldTime {
        calendar: Calendar::Gregorian,
        kind: TemporalKind::Instantaneous,
        valid_time_epoch_seconds: epoch,
        interval_start_epoch_seconds: None,
        interval_end_epoch_seconds: None,
        accumulation: None,
    };
    let mut fields = Vec::new();
    for parameter in PARAMETERS {
        let Some(id) = parameter.canonical else {
            continue;
        };
        let mut values = Vec::with_capacity(NX * NY * LEVELS);
        for level in 1..=LEVELS {
            let source = selected
                .remove(&(parameter.param_id, level))
                .ok_or(DecodeError::Coverage)?;
            for row in source.as_chunks::<NX>().0.iter().rev() {
                values.extend(row.iter().map(|&value| narrow(value)));
            }
        }
        let spec = FIELD_SPECS
            .iter()
            .find(|spec| spec.id == id)
            .ok_or(ContractError::MissingRequiredField(id))?;
        fields.push(Field {
            id,
            shape: vec![NX, NY, LEVELS],
            axis_order: vec![Axis::X, Axis::Y, Axis::Z],
            storage_order: StorageOrder::XFastest,
            unit: spec.unit,
            sign: spec.sign,
            horizontal_staggering: HorizontalStaggering::CellCenter,
            vertical_staggering: VerticalStaggering::LevelCenter,
            time: time.clone(),
            values,
            source_quantization: None,
        });
    }
    let snapshot = Snapshot {
        schema: SchemaIdentity::default(),
        horizontal_grid: HorizontalGrid {
            nx: NX,
            ny: NY,
            xlon0_deg: -8.0,
            ylat0_deg: 43.0,
            dx_deg: 0.25,
            dy_deg: 0.25,
            longitude_domain: LongitudeDomain::Minus180To180,
        },
        vertical_coordinate: vertical(&pv.ok_or(DecodeError::Coverage)?),
        fields,
    };
    // The shared validator checks all present fields and coordinates before
    // reporting the missing hybrid dependency. Never expose this as a Snapshot.
    match snapshot.validate(&Requirements::real_data_native_levels()) {
        Err(ContractError::MissingRequiredField(FieldId::SurfacePressure)) => Ok(snapshot),
        Err(error) => Err(error.into()),
        Ok(()) => Err(DecodeError::SurfacePressureUnavailable),
    }
}

fn vertical(pv: &[f64]) -> VerticalCoordinate {
    let (a, b) = pv.split_at(INTERFACES);
    let interfaces: Vec<_> = a
        .iter()
        .zip(b)
        .map(|(&a, &b)| a + b * f64::from(REFERENCE_SURFACE_PRESSURE_PA))
        .collect();
    VerticalCoordinate {
        kind: VerticalCoordinateKind::HybridSigmaPressure,
        reference: VerticalReference::ModelNative,
        ordering: VerticalOrdering::Increasing,
        level_values: interfaces
            .windows(2)
            .map(|pair| narrow(f64::midpoint(pair[0], pair[1])))
            .collect(),
        interface_values: Some(interfaces.into_iter().map(narrow).collect()),
        hybrid_a_interface_pa: Some(a.iter().copied().map(narrow).collect()),
        hybrid_b_interface: Some(b.iter().copied().map(narrow).collect()),
        reference_surface_pressure_pa: Some(REFERENCE_SURFACE_PRESSURE_PA),
        surface_pressure_dependency: Some(FieldId::SurfacePressure),
    }
}

// Schema v1 stores binary32. The decoder checks field ranges and pins the PV vector.
#[allow(clippy::cast_possible_truncation)]
fn narrow(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests;
