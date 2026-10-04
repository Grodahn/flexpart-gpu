//! Release geometry, mass/species identity and Gregorian chronology checks.

use super::{ValidationCaseError, ValidationCaseManifest, VerticalRef};
use serde::{Deserialize, Serialize};

pub const SPECIES_024_INERT_CONTRACT_ID: &str = "species-024-inert-v1";

pub const SPECIES_024_INERT_CONTRACT_PATH: &str =
    "reference/species-physics/species-024-inert-v1.json";

pub const SPECIES_024_INERT_CONTRACT_BLOB: &str = "21dd5ccd2b616642be9e3989e9b7444774d97fd9";

pub const SPECIES_040_DRY_CONTRACT_ID: &str = "species-040-dry-constant-v1";

pub const SPECIES_040_DRY_CONTRACT_PATH: &str =
    "reference/species-physics/species-040-dry-constant-v1.json";

pub const SPECIES_040_DRY_CONTRACT_BLOB: &str = "c050d6351244beaf2b1a57f661f2321101bda6f1";

pub const SPECIES_040_WET_CONTRACT_ID: &str = "species-040-wet-aerosol-v1";

pub const SPECIES_040_WET_CONTRACT_PATH: &str =
    "reference/species-physics/species-040-wet-aerosol-v1.json";

pub const SPECIES_040_WET_CONTRACT_BLOB: &str = "4f55d23294f320f0050581cfad14800b22bf141d";

/// Mass unit for the released inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MassUnit {
    /// Kilograms (all checked-in cases).
    Kg,
}

/// Normalized source geometry.
///
/// Point releases carry a single position; box releases carry inclusive
/// lon/lat/height ranges. All checked-in cases are points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceGeometry {
    /// Single release position.
    Point {
        /// Longitude [decimal degrees, `horizontal_ref` convention].
        lon_deg: f32,
        /// Latitude [decimal degrees, `horizontal_ref` convention].
        lat_deg: f32,
        /// Height [m, `vertical_ref` reference].
        z_m: f32,
    },
    /// Inclusive release box (lon/lat/height ranges).
    Box {
        /// Minimum longitude [decimal degrees].
        lon_min_deg: f64,
        /// Maximum longitude [decimal degrees].
        lon_max_deg: f64,
        /// Minimum latitude [decimal degrees].
        lat_min_deg: f64,
        /// Maximum latitude [decimal degrees].
        lat_max_deg: f64,
        /// Minimum height [m, `vertical_ref` reference].
        z_min_m: f32,
        /// Maximum height [m, `vertical_ref` reference].
        z_max_m: f32,
    },
}

/// Release timing semantics.
///
/// Timestamps are `YYYYMMDDHHMMSS`. Instant releases inject all particles at
/// one timestamp; window releases span start to end inclusive. All
/// checked-in cases are instants at the integration start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseTiming {
    /// Single release timestamp.
    Instant {
        /// Release timestamp (`YYYYMMDDHHMMSS`).
        at: String,
    },
    /// Release window from start to end inclusive.
    Window {
        /// Window start (`YYYYMMDDHHMMSS`).
        start: String,
        /// Window end (`YYYYMMDDHHMMSS`, must be >= start).
        end: String,
    },
}

/// Released species identifier.
///
/// FLEXPART-native `SPECIES_<NNN>` file identifier (e.g. `SPECIES_024` for
/// the inert tracer, `SPECIES_040` for the depositing aerosol). The trailing
/// number maps to `SPECNUM_REL` and the `SPECIES/SPECIES_<NNN>` oracle file;
/// see the mapping notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeciesPhysicsProfile {
    #[serde(rename = "species_024_inert_v1")]
    Species024InertV1,
    #[serde(rename = "species_040_dry_constant_v1")]
    Species040DryConstantV1,
    #[serde(rename = "species_040_wet_aerosol_v1")]
    Species040WetAerosolV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeciesPhysicsContractRef {
    pub profile: SpeciesPhysicsProfile,
    pub id: String,
    pub version: u32,
    pub path: String,
    /// Content-addressed Git blob identity of the referenced contract file.
    pub git_blob_sha: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeciesRef {
    /// Species identifier (`SPECIES_<NNN>`).
    pub id: String,
    /// Versioned, content-addressed physics contract. Species-dependent
    /// deposition/decay semantics may never come from an unreferenced file.
    pub physics_contract: SpeciesPhysicsContractRef,
}

/// Released inventory (physical mass, distinct from particle sampling).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseInventory {
    /// Released quantity in `unit`.
    pub quantity_kg: f32,
    /// Inventory unit.
    pub unit: MassUnit,
}

/// Normalized release (source) definition.
///
/// Position, timing, species, and inventory are explicit: no workflow
/// defaults. `particle_count` stays a separate execution/sampling parameter
/// (number of computational particles representing the inventory).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSpec {
    /// Source geometry (point or box).
    pub geometry: SourceGeometry,
    /// Vertical reference for geometry heights.
    pub vertical_ref: VerticalRef,
    /// Release timing.
    pub timing: ReleaseTiming,
    /// Released species.
    pub species: SpeciesRef,
    /// Released inventory.
    pub inventory: ReleaseInventory,
    /// Number of computational particles (execution/sampling parameter).
    pub particle_count: u32,
    /// Per-particle mass [kg]. When present it must agree with
    /// `inventory.quantity_kg / particle_count` within 1e-6 relative
    /// (f32 rounding); see `MASS_CONSISTENCY_TOLERANCE_REL`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mass_kg_per_particle: Option<f32>,
}

/// Relative tolerance for total vs per-particle mass consistency (f32
/// rounding of short decimal representations such as 0.001 kg).
pub const MASS_CONSISTENCY_TOLERANCE_REL: f64 = 1e-6;

impl ValidationCaseManifest {
    pub(super) fn parse_timestamp_seconds(
        value: &str,
        field: &'static str,
    ) -> Result<i64, ValidationCaseError> {
        if value.len() != 14 || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ValidationCaseError::AmbiguousField {
                field,
                message: format!("{value} must be YYYYMMDDHHMMSS (14 digits)"),
            });
        }

        let part = |start: usize, end: usize| -> u32 {
            value[start..end]
                .parse::<u32>()
                .expect("timestamp digits were validated")
        };
        let year = part(0, 4);
        let month = part(4, 6);
        let day = part(6, 8);
        let hour = part(8, 10);
        let minute = part(10, 12);
        let second = part(12, 14);

        let leap = |y: u32| -> bool { y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) };
        if year == 0 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
            return Err(ValidationCaseError::AmbiguousField {
                field,
                message: format!("{value} is not a valid Gregorian YYYYMMDDHHMMSS timestamp"),
            });
        }
        let month_days = [
            31_u32,
            if leap(year) { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let max_day = month_days[(month - 1) as usize];
        if day == 0 || day > max_day {
            return Err(ValidationCaseError::AmbiguousField {
                field,
                message: format!(
                    "{value} is not a valid Gregorian timestamp: day {day} is invalid for {year:04}-{month:02}"
                ),
            });
        }

        let y = i64::from(year) - 1;
        let mut days = 365 * y + y / 4 - y / 100 + y / 400;
        days += month_days[..(month - 1) as usize]
            .iter()
            .map(|d| i64::from(*d))
            .sum::<i64>();
        days += i64::from(day - 1);

        Ok(days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))
    }

    pub(super) fn validate_timestamp(
        value: &str,
        field: &'static str,
    ) -> Result<(), ValidationCaseError> {
        Self::parse_timestamp_seconds(value, field).map(|_| ())
    }

    pub(super) fn simulation_bounds_seconds(&self) -> Result<(f64, f64), ValidationCaseError> {
        let start =
            Self::parse_timestamp_seconds(&self.integration.start, "integration.start")? as f64;
        let total = f64::from(self.integration.total_s);
        if !total.is_finite() || total <= 0.0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "integration.total_s must be finite and > 0, got {}",
                    self.integration.total_s
                ),
            });
        }
        let end = start + total;
        let max = Self::parse_timestamp_seconds("99991231235959", "integration.start")? as f64;
        if end > max {
            return Err(ValidationCaseError::AmbiguousField {
                field: "integration.total_s",
                message: format!(
                    "simulation end exceeds the representable Gregorian timestamp range: start={} total_s={}",
                    self.integration.start, self.integration.total_s
                ),
            });
        }
        Ok((start, end))
    }

    pub(super) fn validate_chronology(&self) -> Result<(), ValidationCaseError> {
        let (sim_start, sim_end) = self.simulation_bounds_seconds()?;
        let check_inside = |value: &str, field: &'static str| -> Result<f64, ValidationCaseError> {
            let instant = Self::parse_timestamp_seconds(value, field)? as f64;
            if instant < sim_start || instant > sim_end {
                return Err(ValidationCaseError::AmbiguousField {
                    field,
                    message: format!(
                        "{value} lies outside simulation window starting {} with total_s={}",
                        self.integration.start, self.integration.total_s
                    ),
                });
            }
            Ok(instant)
        };

        match &self.release.timing {
            ReleaseTiming::Instant { at } => {
                check_inside(at, "release.timing.at")?;
            }
            ReleaseTiming::Window { start, end } => {
                let release_start = check_inside(start, "release.timing.start")?;
                let release_end = check_inside(end, "release.timing.end")?;
                if release_end < release_start {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.timing.end",
                        message: format!("window end {end} must be >= start {start}"),
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) fn validate_release(&self) -> Result<(), ValidationCaseError> {
        let release = &self.release;
        if release.vertical_ref != VerticalRef::Agl {
            return Err(ValidationCaseError::AmbiguousField {
                field: "release.vertical_ref",
                message: "schema v2 currently supports only AGL releases; ASL-to-AGL conversion semantics are not implemented".to_string(),
            });
        }
        if release.particle_count == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "release.particle_count must be > 0".to_string(),
            });
        }
        if !release.inventory.quantity_kg.is_finite() || release.inventory.quantity_kg <= 0.0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "release.inventory.quantity_kg must be finite and > 0, got {}",
                    release.inventory.quantity_kg
                ),
            });
        }
        if let Some(per_particle) = release.mass_kg_per_particle {
            if !per_particle.is_finite() || per_particle <= 0.0 {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!(
                        "release.mass_kg_per_particle must be finite and > 0, got {per_particle}"
                    ),
                });
            }
            let implied = f64::from(per_particle) * f64::from(release.particle_count);
            let total = f64::from(release.inventory.quantity_kg);
            let rel = ((implied - total) / total).abs();
            if rel > MASS_CONSISTENCY_TOLERANCE_REL {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "release.mass_kg_per_particle",
                    message: format!(
                        "per-particle mass {per_particle} x count {} implies {implied}, inconsistent with inventory {total}"
                        ,
                        release.particle_count
                    ),
                });
            }
        }
        if release.species.id.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "release.species.id",
            });
        }
        if !release.species.id.starts_with("SPECIES_")
            || release.species.id.len() != "SPECIES_".len() + 3
            || !release.species.id["SPECIES_".len()..]
                .bytes()
                .all(|b| b.is_ascii_digit())
        {
            return Err(ValidationCaseError::AmbiguousField {
                field: "release.species.id",
                message: format!("{} must match SPECIES_<NNN>", release.species.id),
            });
        }
        match &release.timing {
            ReleaseTiming::Instant { at } => {
                Self::validate_timestamp(at, "release.timing.at")?;
            }
            ReleaseTiming::Window { start, end } => {
                Self::validate_timestamp(start, "release.timing.start")?;
                Self::validate_timestamp(end, "release.timing.end")?;
                if end < start {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.timing.end",
                        message: format!("window end {end} must be >= start {start}"),
                    });
                }
            }
        }
        let check_lon = |name: &'static str, lon: f64| -> Result<(), ValidationCaseError> {
            if !lon.is_finite() || !(-180.0..=360.0).contains(&lon) {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{name} must be finite and in [-180, 360], got {lon}"),
                });
            }
            Ok(())
        };
        let check_lat = |name: &'static str, lat: f64| -> Result<(), ValidationCaseError> {
            if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{name} must be finite and in [-90, 90], got {lat}"),
                });
            }
            Ok(())
        };
        let check_height = |name: &'static str, z: f32| -> Result<(), ValidationCaseError> {
            if !z.is_finite() {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{name} must be finite, got {z}"),
                });
            }
            if release.vertical_ref == VerticalRef::Agl && z < 0.0 {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{name} must be >= 0 for AGL releases, got {z}"),
                });
            }
            Ok(())
        };
        match &release.geometry {
            SourceGeometry::Point {
                lon_deg,
                lat_deg,
                z_m,
            } => {
                check_lon("release.geometry.lon_deg", f64::from(*lon_deg))?;
                check_lat("release.geometry.lat_deg", f64::from(*lat_deg))?;
                check_height("release.geometry.z_m", *z_m)?;
            }
            SourceGeometry::Box {
                lon_min_deg,
                lon_max_deg,
                lat_min_deg,
                lat_max_deg,
                z_min_m,
                z_max_m,
            } => {
                check_lon("release.geometry.lon_min_deg", *lon_min_deg)?;
                check_lon("release.geometry.lon_max_deg", *lon_max_deg)?;
                check_lat("release.geometry.lat_min_deg", *lat_min_deg)?;
                check_lat("release.geometry.lat_max_deg", *lat_max_deg)?;
                check_height("release.geometry.z_min_m", *z_min_m)?;
                check_height("release.geometry.z_max_m", *z_max_m)?;
                if lon_max_deg < lon_min_deg {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.geometry.lon_max_deg",
                        message: format!("lon_max {lon_max_deg} must be >= lon_min {lon_min_deg}"),
                    });
                }
                if lat_max_deg < lat_min_deg {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.geometry.lat_max_deg",
                        message: format!("lat_max {lat_max_deg} must be >= lat_min {lat_min_deg}"),
                    });
                }
                if z_max_m < z_min_m {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.geometry.z_max_m",
                        message: format!("z_max {z_max_m} must be >= z_min {z_min_m}"),
                    });
                }
            }
        }
        if self.require_source_containment {
            self.validate_source_containment()?;
        }
        Ok(())
    }

    fn validate_source_containment(&self) -> Result<(), ValidationCaseError> {
        let domain = &self.domain;
        let lon_min = f64::from(domain.xlon0_deg);
        let lat_min = f64::from(domain.ylat0_deg);
        let dx = f64::from(domain.dx_deg);
        let dy = f64::from(domain.dy_deg);
        let max_grid_x_exclusive = f64::from(domain.nx - 1);
        let max_grid_y_exclusive = f64::from(domain.ny - 1);
        let lon_max_exclusive = lon_min + max_grid_x_exclusive * dx;
        let lat_max_exclusive = lat_min + max_grid_y_exclusive * dy;
        let height_min = f64::from(*domain.wind_heights_m.first().unwrap_or(&0.0));
        let height_max = f64::from(*domain.wind_heights_m.last().unwrap_or(&0.0));
        let epsilon = 1e-6;
        let mut check_point =
            |name: &'static str, lon: f64, lat: f64, z: f64| -> Result<(), ValidationCaseError> {
                let grid_x = (lon - lon_min) / dx;
                if grid_x < 0.0 || grid_x >= max_grid_x_exclusive {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.geometry",
                        message: format!(
                        "{name} lon {lon} outside runtime domain [{lon_min}, {lon_max_exclusive})"
                    ),
                    });
                }
                let grid_y = (lat - lat_min) / dy;
                if grid_y < 0.0 || grid_y >= max_grid_y_exclusive {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.geometry",
                        message: format!(
                        "{name} lat {lat} outside runtime domain [{lat_min}, {lat_max_exclusive})"
                    ),
                    });
                }
                if z < height_min - epsilon || z > height_max + epsilon {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.geometry",
                        message: format!(
                            "{name} height {z} outside domain levels [{height_min}, {height_max}]"
                        ),
                    });
                }
                Ok(())
            };
        match &self.release.geometry {
            SourceGeometry::Point {
                lon_deg,
                lat_deg,
                z_m,
            } => {
                check_point(
                    "point",
                    f64::from(*lon_deg),
                    f64::from(*lat_deg),
                    f64::from(*z_m),
                )?;
            }
            SourceGeometry::Box {
                lon_min_deg,
                lon_max_deg,
                lat_min_deg,
                lat_max_deg,
                z_min_m,
                z_max_m,
            } => {
                check_point("box-min", *lon_min_deg, *lat_min_deg, f64::from(*z_min_m))?;
                check_point("box-max", *lon_max_deg, *lat_max_deg, f64::from(*z_max_m))?;
            }
        }
        Ok(())
    }

    pub(super) fn validate_species_physics_contract(&self) -> Result<(), ValidationCaseError> {
        let species = &self.release.species;
        let contract = &species.physics_contract;
        let (
            expected_species,
            expected_id,
            expected_path,
            expected_blob,
            expected_dry,
            expected_wet,
            expected_decay,
        ) = match contract.profile {
            SpeciesPhysicsProfile::Species024InertV1 => (
                "SPECIES_024",
                SPECIES_024_INERT_CONTRACT_ID,
                SPECIES_024_INERT_CONTRACT_PATH,
                SPECIES_024_INERT_CONTRACT_BLOB,
                false,
                false,
                false,
            ),
            SpeciesPhysicsProfile::Species040DryConstantV1 => (
                "SPECIES_040",
                SPECIES_040_DRY_CONTRACT_ID,
                SPECIES_040_DRY_CONTRACT_PATH,
                SPECIES_040_DRY_CONTRACT_BLOB,
                true,
                false,
                false,
            ),
            SpeciesPhysicsProfile::Species040WetAerosolV1 => (
                "SPECIES_040",
                SPECIES_040_WET_CONTRACT_ID,
                SPECIES_040_WET_CONTRACT_PATH,
                SPECIES_040_WET_CONTRACT_BLOB,
                false,
                true,
                false,
            ),
        };

        if species.id != expected_species {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "release.species.id={} conflicts with physics profile {:?}, expected {expected_species}",
                    species.id, contract.profile
                ),
            });
        }
        if contract.id != expected_id
            || contract.version != 1
            || contract.path != expected_path
            || contract.git_blob_sha != expected_blob
        {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "release.species.physics_contract does not match canonical {:?} reference                      (expected id={expected_id}, version=1, path={expected_path}, git_blob_sha={expected_blob})",
                    contract.profile
                ),
            });
        }
        if self.physics_switches.dry_deposition != expected_dry
            || self.physics_switches.wet_deposition != expected_wet
            || self.physics_switches.decay != expected_decay
        {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "physics switches dry/wet/decay={}/{}/{} conflict with species physics profile {:?}, expected {expected_dry}/{expected_wet}/{expected_decay}",
                    self.physics_switches.dry_deposition,
                    self.physics_switches.wet_deposition,
                    self.physics_switches.decay,
                    contract.profile
                ),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        load_checked_in_case, load_validation_case_schema, make_minimal_manifest,
        minimal_manifest_json, parse_json_value, validate_json_schema_subset,
    };

    use super::super::*;

    #[test]
    fn gregorian_timestamp_validation_rejects_impossible_dates_and_accepts_leap_day() {
        assert!(
            ValidationCaseManifest::validate_timestamp("20240229010203", "integration.start")
                .is_ok()
        );
        for bad in [
            "20230229010203",
            "20241301000000",
            "20240431000000",
            "20240101240000",
            "20240101006000",
            "00000101000000",
        ] {
            assert!(
                ValidationCaseManifest::validate_timestamp(bad, "integration.start").is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn release_window_must_stay_inside_simulation_window() {
        let mut manifest = make_minimal_manifest();
        manifest.release.timing = ReleaseTiming::Window {
            start: "20231231235959".to_string(),
            end: "20240101000000".to_string(),
        };
        let err = manifest
            .validate()
            .expect_err("release before simulation must fail");
        assert!(err.to_string().contains("outside simulation window"));

        let mut manifest = make_minimal_manifest();
        manifest.release.timing = ReleaseTiming::Window {
            start: "20240101003000".to_string(),
            end: "20240101010001".to_string(),
        };
        let err = manifest
            .validate()
            .expect_err("release after simulation must fail");
        assert!(err.to_string().contains("outside simulation window"));
    }

    #[test]
    fn source_containment_matches_runtime_half_open_horizontal_domain() {
        let mut manifest = make_minimal_manifest();
        manifest.domain.xlon0_deg = 0.0;
        manifest.domain.ylat0_deg = 0.0;
        manifest.domain.dx_deg = 1.0;
        manifest.domain.dy_deg = 1.0;
        manifest.domain.nx = 3;
        manifest.domain.ny = 3;
        manifest.release.geometry = SourceGeometry::Point {
            lon_deg: 1.999,
            lat_deg: 1.0,
            z_m: 50.0,
        };
        manifest
            .validate()
            .expect("point just inside runtime domain");

        manifest.release.geometry = SourceGeometry::Point {
            lon_deg: 2.0,
            lat_deg: 1.0,
            z_m: 50.0,
        };
        let err = manifest
            .validate()
            .expect_err("runtime upper grid boundary is exclusive");
        assert!(
            err.to_string().contains("runtime domain"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn source_containment_policy_is_required_explicitly() {
        let schema = load_validation_case_schema();
        let mut raw = minimal_manifest_json();
        raw.as_object_mut()
            .expect("manifest object")
            .remove("require_source_containment");
        assert!(
            validate_json_schema_subset(&schema, &schema, &raw, "$").is_err(),
            "JSON Schema must reject an omitted containment policy"
        );
        let err =
            parse_json_value(&raw).expect_err("Rust must reject an omitted containment policy");
        assert!(
            err.to_string().contains("require_source_containment"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn release_height_without_vertical_ref_is_rejected() {
        let mut raw = minimal_manifest_json();
        raw["release"]
            .as_object_mut()
            .expect("release object")
            .remove("vertical_ref");
        let err = parse_json_value(&raw).expect_err("missing vertical_ref fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("vertical_ref"),
            "error must name the missing reference: {rendered}"
        );
    }

    #[test]
    fn schema_v2_rejects_asl_vertical_references_fail_closed() {
        let schema = load_validation_case_schema();

        let mut raw = minimal_manifest_json();
        raw["release"]["vertical_ref"] = serde_json::json!("asl");
        let schema_err = validate_json_schema_subset(&schema, &schema, &raw, "$")
            .expect_err("schema must reject ASL release");
        assert!(
            schema_err.contains("release") || schema_err.contains("const"),
            "{schema_err}"
        );
        let err = parse_json_value(&raw).expect_err("Rust must reject ASL release");
        assert!(
            err.to_string().contains("release.vertical_ref"),
            "unexpected: {err}"
        );

        let mut raw = minimal_manifest_json();
        raw["domain"]["wind_heights_ref"] = serde_json::json!("asl");
        let schema_err = validate_json_schema_subset(&schema, &schema, &raw, "$")
            .expect_err("schema must reject ASL domain heights");
        assert!(
            schema_err.contains("domain") || schema_err.contains("const"),
            "{schema_err}"
        );
        let err = parse_json_value(&raw).expect_err("Rust must reject ASL domain heights");
        assert!(
            err.to_string().contains("domain.wind_heights_ref"),
            "unexpected: {err}"
        );

        let mut raw = minimal_manifest_json();
        raw["output_grid"]["heights_ref"] = serde_json::json!("asl");
        let schema_err = validate_json_schema_subset(&schema, &schema, &raw, "$")
            .expect_err("schema must reject ASL output heights");
        assert!(
            schema_err.contains("output_grid") || schema_err.contains("const"),
            "{schema_err}"
        );
        let err = parse_json_value(&raw).expect_err("Rust must reject ASL output heights");
        assert!(
            err.to_string().contains("output_grid.heights_ref"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn inconsistent_total_and_per_particle_mass_is_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.release.inventory.quantity_kg = 1.0;
        manifest.release.particle_count = 1000;
        manifest.release.mass_kg_per_particle = Some(0.5);
        let err = manifest.validate().expect_err("inconsistent mass fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::AmbiguousField {
                    field: "release.mass_kg_per_particle",
                    ..
                }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn invalid_coordinates_spacing_and_placement_are_rejected() {
        let mut manifest = make_minimal_manifest();
        // Longitude out of range.
        if let SourceGeometry::Point {
            ref mut lon_deg, ..
        } = manifest.release.geometry
        {
            *lon_deg = 500.0;
        }
        assert!(manifest.validate().is_err());
        let mut manifest = make_minimal_manifest();
        if let SourceGeometry::Point {
            ref mut lat_deg, ..
        } = manifest.release.geometry
        {
            *lat_deg = f32::INFINITY;
        }
        assert!(manifest.validate().is_err());
        // Non-positive grid spacing.
        let mut manifest = make_minimal_manifest();
        manifest.domain.dx_deg = 0.0;
        assert!(manifest.validate().is_err());
        // Out-of-domain release with containment required.
        let mut manifest = make_minimal_manifest();
        if let SourceGeometry::Point {
            ref mut lon_deg, ..
        } = manifest.release.geometry
        {
            *lon_deg = 100.0;
        }
        let err = manifest.validate().expect_err("out-of-domain fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::AmbiguousField {
                    field: "release.geometry",
                    ..
                }
            ),
            "unexpected: {err}"
        );
        // Same placement validates when containment is waived.
        let mut manifest = make_minimal_manifest();
        if let SourceGeometry::Point {
            ref mut lon_deg, ..
        } = manifest.release.geometry
        {
            *lon_deg = 100.0;
        }
        manifest.require_source_containment = false;
        manifest.validate().expect("waived containment passes");
    }

    #[test]
    fn adv_and_wind_retain_their_original_source_meaning() {
        let adv = load_checked_in_case("ADV-ANA-001");
        match &adv.release.geometry {
            SourceGeometry::Point {
                lon_deg,
                lat_deg,
                z_m,
            } => {
                assert_eq!((*lon_deg, *lat_deg, *z_m), (10.0, 50.0, 100.0));
            }
            other => panic!("ADV geometry changed: {other:?}"),
        }
        assert_eq!(adv.release.vertical_ref, VerticalRef::Agl);
        assert_eq!(
            adv.release.timing,
            ReleaseTiming::Instant {
                at: "20240101000000".to_string()
            }
        );
        assert_eq!(adv.release.species.id, "SPECIES_024");
        assert_eq!(adv.release.inventory.quantity_kg, 1024.0);
        assert_eq!(adv.release.inventory.unit, MassUnit::Kg);
        assert_eq!(adv.release.particle_count, 1024);
        assert_eq!(
            adv.domain.horizontal_ref,
            HorizontalCoordRef::GeographicLonLatDegrees
        );
        assert_eq!(adv.domain.wind_heights_ref, VerticalRef::Agl);

        let wind = load_checked_in_case("WIND-UNI-002");
        match &wind.release.geometry {
            SourceGeometry::Point {
                lon_deg,
                lat_deg,
                z_m,
            } => {
                assert_eq!((*lon_deg, *lat_deg, *z_m), (10.0, 10.0, 50.0));
            }
            other => panic!("WIND geometry changed: {other:?}"),
        }
        assert_eq!(wind.release.vertical_ref, VerticalRef::Agl);
        assert_eq!(
            wind.release.timing,
            ReleaseTiming::Instant {
                at: "20240101000000".to_string()
            }
        );
        assert_eq!(wind.release.species.id, "SPECIES_024");
        assert_eq!(wind.release.inventory.quantity_kg, 1.0);
        assert_eq!(wind.release.particle_count, 1000);
    }

    #[test]
    fn species_physics_contract_is_exact_and_controls_switches() {
        let mut manifest = make_minimal_manifest();
        manifest.release.species.physics_contract.git_blob_sha = "deadbeef".to_string();
        let err = manifest
            .validate()
            .expect_err("wrong contract hash must fail");
        assert!(err.to_string().contains("canonical"));

        let mut manifest = make_minimal_manifest();
        manifest.physics_switches.decay = true;
        let err = manifest
            .validate()
            .expect_err("decay requires a dedicated species physics profile");
        assert!(err.to_string().contains("species physics profile"));
    }
}
