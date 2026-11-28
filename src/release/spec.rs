//! Versioned source-term contract before particle allocation.

use std::collections::BTreeSet;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

use crate::config::{ConfigError, SpeciesConfig};
use crate::particles::MAX_SPECIES;

const VERSION: u32 = 1;

/// A Gregorian UTC instant with second precision, serialized as seconds since 1970-01-01 UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct ReleaseTime(i64);

impl ReleaseTime {
    /// Parse an exact compact UTC timestamp or ISO 8601 UTC timestamp with a trailing Z.
    ///
    /// # Errors
    /// Returns a configuration error if the value lacks defined UTC syntax or is not a Gregorian instant.
    pub fn parse_utc(value: &str) -> Result<Self, ConfigError> {
        let invalid = || invalid_release(format!("invalid Gregorian UTC timestamp: {value}"));
        let compact = if value.len() == 20
            && value.is_ascii()
            && &value[4..5] == "-"
            && &value[7..8] == "-"
            && &value[10..11] == "T"
            && &value[13..14] == ":"
            && &value[16..17] == ":"
            && &value[19..] == "Z"
        {
            format!(
                "{}{}{}{}{}{}",
                &value[..4],
                &value[5..7],
                &value[8..10],
                &value[11..13],
                &value[14..16],
                &value[17..19]
            )
        } else {
            value.to_owned()
        };
        if compact.len() != 14 || !compact.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        let read = |start, end| {
            compact
                .get(start..end)
                .and_then(|part| part.parse::<i64>().ok())
                .ok_or_else(invalid)
        };
        let year = read(0, 4)?;
        let month = read(4, 6)?;
        let day = read(6, 8)?;
        let hour = read(8, 10)?;
        let minute = read(10, 12)?;
        let second = read(12, 14)?;
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let last_day = match month {
            2 => 28 + i64::from(leap),
            4 | 6 | 9 | 11 => 30,
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            _ => return Err(invalid()),
        };
        if year == 0 || !(1..=last_day).contains(&day) || hour > 23 || minute > 59 || second > 59 {
            return Err(invalid());
        }
        let days = super::days_from_civil(year, month, day);
        Ok(Self(days * 86_400 + hour * 3_600 + minute * 60 + second))
    }

    /// Return whole seconds since the Unix epoch for downstream scheduling.
    #[must_use]
    pub fn unix_seconds(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for ReleaseTime {
    type Error = ConfigError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        let first = super::days_from_civil(1, 1, 1) * 86_400;
        let end = super::days_from_civil(10_000, 1, 1) * 86_400;
        if (first..end).contains(&value) {
            Ok(Self(value))
        } else {
            Err(invalid_release(
                "UTC seconds outside Gregorian years 1 through 9999",
            ))
        }
    }
}

impl From<ReleaseTime> for i64 {
    fn from(value: ReleaseTime) -> Self {
        value.0
    }
}

/// Interval-integrated release timing in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseProfile {
    /// Entire inventory is requested at one instant.
    Instant { at: ReleaseTime },
    /// Constant requested rate between the start and end instants.
    UniformWindow {
        start: ReleaseTime,
        end: ReleaseTime,
    },
}

impl ReleaseProfile {
    /// Return inclusive UTC endpoints for downstream scheduling.
    #[must_use]
    pub fn bounds(self) -> (ReleaseTime, ReleaseTime) {
        match self {
            Self::Instant { at } => (at, at),
            Self::UniformWindow { start, end } => (start, end),
        }
    }
}

/// Horizontal coordinate reference for the release geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseHorizontalRef {
    /// WGS84 longitude (east positive) and latitude, both in decimal degrees.
    GeographicWgs84Degrees,
}

/// Horizontal source geometry in geographic WGS84 east-positive longitude and latitude degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseGeometry {
    /// A fixed longitude and latitude in degrees.
    Point { lon_deg: f64, lat_deg: f64 },
    /// Uniform in angular coordinates within a non-wrapping rectangle or line.
    UniformLonLatBounds {
        lon_min_deg: f64,
        lon_max_deg: f64,
        lat_min_deg: f64,
        lat_max_deg: f64,
    },
}

impl ReleaseGeometry {
    /// Normalize a geographic extent, using a point when both axes are fixed.
    #[must_use]
    #[allow(clippy::float_cmp)] // A fixed axis requires exactly coinciding source bounds.
    pub fn from_bounds(
        lon_min_deg: f64,
        lon_max_deg: f64,
        lat_min_deg: f64,
        lat_max_deg: f64,
    ) -> Self {
        if lon_min_deg == lon_max_deg && lat_min_deg == lat_max_deg {
            Self::Point {
                lon_deg: lon_min_deg,
                lat_deg: lat_min_deg,
            }
        } else {
            Self::UniformLonLatBounds {
                lon_min_deg,
                lon_max_deg,
                lat_min_deg,
                lat_max_deg,
            }
        }
    }

    /// Return inclusive longitude and latitude bounds in degrees.
    #[must_use]
    pub fn bounds(self) -> (f64, f64, f64, f64) {
        match self {
            Self::Point { lon_deg, lat_deg } => (lon_deg, lon_deg, lat_deg, lat_deg),
            Self::UniformLonLatBounds {
                lon_min_deg,
                lon_max_deg,
                lat_min_deg,
                lat_max_deg,
            } => (lon_min_deg, lon_max_deg, lat_min_deg, lat_max_deg),
        }
    }
}

/// Vertical datum for the source heights in metres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseHeightRef {
    /// Above local ground level.
    Agl,
    /// Above mean sea level.
    Asl,
}

/// Fixed height or uniform vertical extent in metres relative to the declared datum.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseHeight {
    /// A single height in metres.
    Fixed {
        reference: ReleaseHeightRef,
        height_m: f64,
    },
    /// Uniform along the metre coordinate between distinct bounds.
    UniformRange {
        reference: ReleaseHeightRef,
        lower_m: f64,
        upper_m: f64,
    },
}

impl ReleaseHeight {
    /// Normalize a metre extent, using a fixed height when its bounds coincide.
    #[must_use]
    #[allow(clippy::float_cmp)] // A fixed height requires exactly coinciding source bounds.
    pub fn from_bounds(reference: ReleaseHeightRef, lower_m: f64, upper_m: f64) -> Self {
        if lower_m == upper_m {
            Self::Fixed {
                reference,
                height_m: lower_m,
            }
        } else {
            Self::UniformRange {
                reference,
                lower_m,
                upper_m,
            }
        }
    }

    /// Return the declared datum and inclusive metre bounds, without a terrain transform.
    #[must_use]
    pub fn bounds(self) -> (ReleaseHeightRef, f64, f64) {
        match self {
            Self::Fixed {
                reference,
                height_m,
            } => (reference, height_m, height_m),
            Self::UniformRange {
                reference,
                lower_m,
                upper_m,
            } => (reference, lower_m, upper_m),
        }
    }
}

/// Physical inventory per release and species. No activity-to-mass conversion is defined in v1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseQuantity {
    /// Total mass in kilograms.
    Mass { kg: f64 },
    /// Total activity in becquerels at the stated UTC reference instant.
    Activity {
        bq: f64,
        reference_time: ReleaseTime,
    },
}

/// Stable #10 identity and the explicitly resolved production particle mass lane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseInventory {
    pub species_id: String,
    pub mass_slot: u8,
    pub quantity: ReleaseQuantity,
    /// Original quantity and conversion method when the adapter changes units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversion: Option<ReleaseConversion>,
}

/// Physics-relevant unit conversion recorded alongside the resulting inventory.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseConversion {
    /// Explicit project RELEASES `MASS_UNIT=g` converted to kilograms.
    DeclaredGramsToKgV1 { input_g: f64 },
}

pub(crate) const GRAMS_PER_KILOGRAM: f64 = 1_000.0;

/// Versioned source identity and normalization route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "adapter", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseSource {
    /// Existing project RELEASES syntax with the v1 alias and UTC interpretation.
    ProjectReleasesV1 {
        path: String,
        sha256: String,
        record: u32,
    },
    /// Pinned FLEXPART 11.1 RELEASES namelists with explicit species numbers.
    Flexpart11_1ReleasesV1 {
        path: String,
        sha256: String,
        record: u32,
    },
    /// #51 declaration v2, explicitly mapped to the production contract.
    ValidationCaseV2 { case_id: String },
    /// A caller supplies an auditable stable source identifier.
    ExplicitV1 { source_id: String },
}

/// Serializable v1 fields shared by the normalizers and the canonical wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSpecFields {
    pub id: String,
    pub profile: ReleaseProfile,
    pub horizontal_ref: ReleaseHorizontalRef,
    pub geometry: ReleaseGeometry,
    pub height: ReleaseHeight,
    pub inventories: Vec<ReleaseInventory>,
    /// Total requested logical particles for this release.
    pub particle_count: u64,
    /// Optional release seed supplied by the source. Allocation and RNG addressing are downstream.
    pub sampling_seed: Option<u64>,
    pub source: ReleaseSource,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseSpecDocument {
    schema_version: u32,
    release: ReleaseSpecFields,
}

/// Validated, versioned source term. Deserialization checks the structure; run binding checks the #10 table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ReleaseSpecDocument", into = "ReleaseSpecDocument")]
pub struct ReleaseSpec(ReleaseSpecFields);

impl Deref for ReleaseSpec {
    type Target = ReleaseSpecFields;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<ReleaseSpec> for ReleaseSpecDocument {
    fn from(value: ReleaseSpec) -> Self {
        Self {
            schema_version: VERSION,
            release: value.0,
        }
    }
}

impl TryFrom<ReleaseSpecDocument> for ReleaseSpec {
    type Error = ConfigError;

    fn try_from(document: ReleaseSpecDocument) -> Result<Self, Self::Error> {
        if document.schema_version != VERSION {
            return Err(invalid_release(format!(
                "unsupported ReleaseSpec schema version {}",
                document.schema_version
            )));
        }
        Self::new(document.release)
    }
}

impl ReleaseSpec {
    /// Validate and canonicalize a source term without allocating any particles.
    ///
    /// # Errors
    /// Returns a configuration error for invalid or ambiguous source fields or provenance.
    #[allow(clippy::float_cmp)] // Fixed bounds and recorded unit conversions require exact equality.
    pub fn new(mut fields: ReleaseSpecFields) -> Result<Self, ConfigError> {
        let error = |message| invalid_release(format!("release `{}`: {message}", fields.id));
        let (start, end) = fields.profile.bounds();
        if matches!(fields.profile, ReleaseProfile::UniformWindow { .. }) && start >= end {
            return Err(error("uniform window requires start < end"));
        }
        let (west, east, south, north) = fields.geometry.bounds();
        if !(-180.0..=180.0).contains(&west)
            || !(-180.0..=180.0).contains(&east)
            || !(-90.0..=90.0).contains(&south)
            || !(-90.0..=90.0).contains(&north)
            || west > east
            || south > north
            || (matches!(fields.geometry, ReleaseGeometry::UniformLonLatBounds { .. })
                && west == east
                && south == north)
        {
            return Err(error("invalid geographic longitude/latitude bounds"));
        }
        let (datum, lower, upper) = fields.height.bounds();
        if !lower.is_finite()
            || !upper.is_finite()
            || lower > upper
            || (datum == ReleaseHeightRef::Agl && lower < 0.0)
            || (matches!(fields.height, ReleaseHeight::UniformRange { .. }) && lower == upper)
        {
            return Err(error("invalid metre height bounds"));
        }
        if fields.particle_count == 0 || !is_identity(&fields.id) || !valid_source(&fields.source) {
            return Err(error(
                "explicit particle count, release ID and source provenance are required",
            ));
        }
        validate_inventory(&fields.inventories).map_err(error)?;
        let project = matches!(fields.source, ReleaseSource::ProjectReleasesV1 { .. });
        for item in &mut fields.inventories {
            match (&mut item.conversion, item.quantity) {
                (
                    Some(ReleaseConversion::DeclaredGramsToKgV1 { input_g }),
                    ReleaseQuantity::Mass { kg },
                ) if project
                    && input_g.is_finite()
                    && *input_g >= 0.0
                    && *input_g / GRAMS_PER_KILOGRAM == kg
                    && (kg > 0.0 || *input_g == 0.0) =>
                {
                    *input_g = zero(*input_g);
                }
                (None, _) => {}
                _ => {
                    return Err(error(
                        "missing or inconsistent quantity conversion provenance",
                    ))
                }
            }
        }
        fields.geometry =
            ReleaseGeometry::from_bounds(zero(west), zero(east), zero(south), zero(north));
        fields.height = ReleaseHeight::from_bounds(datum, zero(lower), zero(upper));
        for inventory in &mut fields.inventories {
            match &mut inventory.quantity {
                ReleaseQuantity::Mass { kg } => *kg = zero(*kg),
                ReleaseQuantity::Activity { bq, .. } => *bq = zero(*bq),
            }
        }
        fields
            .inventories
            .sort_unstable_by_key(|inventory| inventory.mass_slot);
        Ok(Self(fields))
    }
}

fn is_identity(value: &str) -> bool {
    !value.is_empty() && value == value.trim() && !value.chars().any(char::is_control)
}

fn valid_source(source: &ReleaseSource) -> bool {
    match source {
        ReleaseSource::ProjectReleasesV1 {
            path,
            sha256,
            record,
        }
        | ReleaseSource::Flexpart11_1ReleasesV1 {
            path,
            sha256,
            record,
        } => {
            is_identity(path)
                && *record > 0
                && sha256.len() == 64
                && sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
        ReleaseSource::ValidationCaseV2 { case_id } => is_identity(case_id),
        ReleaseSource::ExplicitV1 { source_id } => is_identity(source_id),
    }
}

fn zero(value: f64) -> f64 {
    if value == 0.0 {
        0.0
    } else {
        value
    }
}

fn validate_inventory(inventories: &[ReleaseInventory]) -> Result<(), &'static str> {
    if inventories.is_empty() || inventories.len() > MAX_SPECIES {
        return Err("inventory must contain between one and four species");
    }
    let mut names = BTreeSet::new();
    let mut slots = BTreeSet::new();
    let mut positive = false;
    for item in inventories {
        if !is_identity(&item.species_id)
            || !names.insert(item.species_id.to_ascii_lowercase())
            || usize::from(item.mass_slot) >= MAX_SPECIES
            || !slots.insert(item.mass_slot)
        {
            return Err("unknown, duplicate or unrepresentable species identity/slot");
        }
        let amount = match item.quantity {
            ReleaseQuantity::Mass { kg } => kg,
            ReleaseQuantity::Activity { bq, .. } => bq,
        };
        if !amount.is_finite() || amount < 0.0 {
            return Err("inventory quantities must be finite and nonnegative");
        }
        positive |= amount > 0.0;
    }
    positive
        .then_some(())
        .ok_or("at least one inventory quantity must be positive")
}

/// Check the production species table and the identity of every release inventory lane.
///
/// # Errors
/// Returns a configuration error for an invalid table, duplicate release ID, or inconsistent lane.
pub fn validate_release_bindings(
    releases: &[ReleaseSpec],
    species: &[SpeciesConfig],
) -> Result<(), ConfigError> {
    validate_release_species(species)?;
    if releases.is_empty() {
        return Err(invalid_release("at least one release is required"));
    }
    let mut ids = BTreeSet::new();
    for release in releases {
        if !ids.insert(release.id.as_str()) {
            return Err(invalid_release(format!(
                "duplicate release ID: {}",
                release.id
            )));
        }
        for inventory in &release.inventories {
            if species
                .get(usize::from(inventory.mass_slot))
                .map(|spec| spec.name.as_str())
                != Some(inventory.species_id.as_str())
            {
                return Err(invalid_release(format!(
                    "release `{}`: species `{}` does not bind to run slot {}",
                    release.id, inventory.species_id, inventory.mass_slot
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_release_species(species: &[SpeciesConfig]) -> Result<(), ConfigError> {
    if species.is_empty() || species.len() > MAX_SPECIES {
        return Err(invalid_release(format!(
            "release requires 1..={MAX_SPECIES} configured species, got {}",
            species.len()
        )));
    }
    let mut names = BTreeSet::new();
    for item in species {
        item.validate()?;
        if !is_identity(&item.name) || !names.insert(item.name.to_ascii_lowercase()) {
            return Err(invalid_release(format!(
                "ambiguous configured species identity: {}",
                item.name
            )));
        }
    }
    Ok(())
}

pub(crate) fn invalid_release(message: impl Into<String>) -> ConfigError {
    ConfigError::Validation {
        message: message.into(),
    }
}
