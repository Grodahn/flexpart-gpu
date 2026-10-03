//! Wind/surface declarations and meteorology provenance/profile validation.

use super::{ValidationCaseError, ValidationCaseManifest};
use serde::{Deserialize, Serialize};

/// Stable Oracle meteorology identities used by schema-v2 validation cases.
pub const SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_ID: &str = "flexpart-synthetic-grib-v1";

pub const SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_VERSION: u32 = 1;

pub const SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_PATH: &str =
    "reference/oracle-meteorology/synthetic-grib-v1.json";

pub const REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_ID: &str = "real-weather-manifest-v1";

pub const REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_VERSION: u32 = 1;

pub const REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_PATH: &str =
    "reference/oracle-meteorology/real-weather-manifest-v1.json";

/// Stable reference to the meteorology representation/source used by the oracle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleMeteorologyProfileRef {
    pub id: String,
    pub version: u32,
    pub manifest_path: String,
}

/// Candidate-side transformation step in the meteorology processing chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateMetTransformation {
    /// Description of the transformation (e.g., "ERA5 native 137 hybrid levels -> 16 AGL levels, omega to m/s").
    pub description: String,
    /// Script or tool identity that performs the transformation (e.g., "scripts/etex/prepare_native_era5.py", version or git hash).
    pub script: String,
    /// Version or commit of the script.
    pub version: String,
}

/// Oracle-side transformation step in the meteorology processing chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleMetTransformation {
    /// Description of the transformation (e.g., "ERA5 native 137 hybrid levels retained, etadot used").
    pub description: String,
    /// Script or tool identity that performs the transformation.
    pub script: String,
    /// Version or commit of the script.
    pub version: String,
}

/// Meteorology source reference with complete identity and transformation chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeteorologySpec {
    /// Stable dataset or fixture identifier (e.g., "era5-native-mini-19941023-24").
    pub dataset_id: String,
    /// Version of the dataset (e.g., "v1", date-based, or content hash).
    pub version: String,
    /// Repository-relative source path to the native meteorology data (e.g., "fixtures/etex/native-mini/").
    pub source_path: String,
    /// SHA-256 digest of the source data, or reference to a versioned digest manifest.
    pub digest: String,
    /// Temporal coverage of the meteorology data [start, end] in YYYYMMDDHHMMSS.
    pub temporal_coverage: [String; 2],
    /// Horizontal coordinate identity (e.g., "geographic_lon_lat_degrees").
    pub horizontal_coord: String,
    /// Vertical coordinate identity (e.g., "era5_native_hybrid_137_levels" or "agl_16_levels").
    pub vertical_coord: String,
    /// Required meteorological fields present in the source data.
    pub required_fields: Vec<String>,
    /// Candidate-side transformation chain.
    pub candidate_transformation: CandidateMetTransformation,
    /// Oracle-side transformation chain.
    pub oracle_transformation: OracleMetTransformation,
    /// Optional note for human readers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Wind field specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "profile", rename_all = "snake_case", deny_unknown_fields)]
pub enum WindSpec {
    /// Uniform wind field.
    Uniform { u_m_s: f32, v_m_s: f32, w_m_s: f32 },
    /// Linear shear wind profile u(z) = u0 + shear * z.
    LinearShear {
        u0_m_s: f32,
        u_shear_per_s: f32,
        v_m_s: f32,
        w_m_s: f32,
    },
    /// Real-weather wind from a versioned meteorology specification.
    RealWeather {
        /// Complete meteorology specification including identity, provenance, and transformations.
        meteorology: MeteorologySpec,
    },
}

/// Surface fields specification with explicit units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceSpec {
    /// 10-m eastward wind used by candidate PBL diagnostics [m/s].
    pub u10_m_s: f32,
    /// 10-m northward wind used by candidate PBL diagnostics [m/s].
    pub v10_m_s: f32,
    /// Surface pressure [Pa].
    pub surface_pressure_pa: f32,
    /// 2-m temperature [K].
    pub temperature_2m_k: f32,
    /// 2-m dewpoint [K].
    pub dewpoint_2m_k: f32,
    /// Sensible heat flux [W/m^2].
    pub sensible_heat_flux_w_m2: f32,
    /// Solar radiation [W/m^2].
    pub solar_radiation_w_m2: f32,
    /// Surface stress [N/m^2].
    pub surface_stress_n_m2: f32,
    /// Friction velocity [m/s].
    pub friction_velocity_m_s: f32,
    /// Convective velocity scale [m/s].
    pub convective_velocity_scale_m_s: f32,
    /// Mixing height (PBL height) [m].
    pub mixing_height_m: f32,
    /// Tropopause height [m].
    pub tropopause_height_m: f32,
    /// Inverse Obukhov length [1/m].
    pub inv_obukhov_length_per_m: f32,
    /// Large-scale precipitation [mm/h].
    pub precip_large_scale_mm_h: f32,
    /// Convective precipitation [mm/h].
    pub precip_convective_mm_h: f32,
}

impl ValidationCaseManifest {
    pub(super) fn validate_oracle_meteorology_profile(&self) -> Result<(), ValidationCaseError> {
        let (expected_id, expected_version, expected_path) = match &self.wind {
            WindSpec::RealWeather { .. } => (
                REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_ID,
                REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_VERSION,
                REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_PATH,
            ),
            _ => (
                SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_ID,
                SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_VERSION,
                SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_PATH,
            ),
        };
        let actual = &self.oracle_meteorology_profile;
        if actual.id != expected_id
            || actual.version != expected_version
            || actual.manifest_path != expected_path
        {
            return Err(ValidationCaseError::InvalidExecutionProfile {
                message: format!(
                    "oracle_meteorology_profile must reference {expected_id} v{expected_version} at {expected_path}, got {} v{} at {}",
                    actual.id, actual.version, actual.manifest_path
                ),
            });
        }
        Ok(())
    }

    pub(super) fn validate_meteorology(
        &self,
        met: &MeteorologySpec,
    ) -> Result<(), ValidationCaseError> {
        // Required fields present
        if met.dataset_id.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.dataset_id",
            });
        }
        if met.version.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.version",
            });
        }
        if met.source_path.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.source_path",
            });
        }
        if met.digest.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.digest",
            });
        }
        if met.temporal_coverage[0].is_empty() || met.temporal_coverage[1].is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.temporal_coverage",
            });
        }
        if met.horizontal_coord.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.horizontal_coord",
            });
        }
        if met.vertical_coord.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.vertical_coord",
            });
        }
        if met.required_fields.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.required_fields",
            });
        }

        let coverage_start = Self::parse_timestamp_seconds(
            &met.temporal_coverage[0],
            "meteorology.temporal_coverage",
        )? as f64;
        let coverage_end = Self::parse_timestamp_seconds(
            &met.temporal_coverage[1],
            "meteorology.temporal_coverage",
        )? as f64;
        if coverage_end < coverage_start {
            return Err(ValidationCaseError::AmbiguousField {
                field: "meteorology.temporal_coverage",
                message: "temporal_coverage end must be >= start".to_string(),
            });
        }
        let (sim_start, sim_end) = self.simulation_bounds_seconds()?;
        if coverage_start > sim_start || coverage_end < sim_end {
            return Err(ValidationCaseError::AmbiguousField {
                field: "meteorology.temporal_coverage",
                message: format!(
                    "meteorology coverage {}..{} must cover the full simulation starting {} for {} s",
                    met.temporal_coverage[0],
                    met.temporal_coverage[1],
                    self.integration.start,
                    self.integration.total_s
                ),
            });
        }

        // Validate candidate transformation
        if met.candidate_transformation.description.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.candidate_transformation.description",
            });
        }
        if met.candidate_transformation.script.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.candidate_transformation.script",
            });
        }
        if met.candidate_transformation.version.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.candidate_transformation.version",
            });
        }

        // Validate oracle transformation
        if met.oracle_transformation.description.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.oracle_transformation.description",
            });
        }
        if met.oracle_transformation.script.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.oracle_transformation.script",
            });
        }
        if met.oracle_transformation.version.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.oracle_transformation.version",
            });
        }

        // Validate source_path is a non-empty, normalized repository-relative path
        if met.source_path.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.source_path",
            });
        }
        if met.source_path.starts_with('/')
            || met.source_path.starts_with('.')
            || met.source_path.contains('\\')
            || met
                .source_path
                .split('/')
                .any(|segment| segment == "." || segment == "..")
            || met.source_path.contains("//")
        {
            return Err(ValidationCaseError::AmbiguousField {
                field: "meteorology.source_path",
                message: "source_path must be a normalized repository-relative path (no absolute/dot-parent/backslash/double-slash form)".to_string(),
            });
        }

        // Validate digest format (sha256 or a normalized repository-relative manifest reference).
        if met.digest.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.digest",
            });
        }
        if met.digest.len() == 64 {
            if !met.digest.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "meteorology.digest",
                    message: "sha256 digest must be hexadecimal".to_string(),
                });
            }
        } else if let Some(manifest_path) = met.digest.strip_prefix("manifest:") {
            if manifest_path.is_empty()
                || manifest_path.starts_with('/')
                || manifest_path.starts_with('.')
                || manifest_path.ends_with('/')
                || manifest_path.contains('\\')
                || manifest_path.contains("//")
                || manifest_path
                    .split('/')
                    .any(|segment| segment == "." || segment == ".." || segment.is_empty())
            {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "meteorology.digest",
                    message: "manifest digest must be manifest:<normalized repository-relative file path>".to_string(),
                });
            }
        } else {
            return Err(ValidationCaseError::AmbiguousField {
                field: "meteorology.digest",
                message: "digest must be 64-char hex sha256 or 'manifest:<normalized repository-relative file path>'".to_string(),
            });
        }

        // Candidate transformation required fields
        if met.candidate_transformation.description.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.candidate_transformation.description",
            });
        }
        if met.candidate_transformation.script.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.candidate_transformation.script",
            });
        }
        if met.candidate_transformation.version.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.candidate_transformation.version",
            });
        }

        // Oracle transformation required fields
        if met.oracle_transformation.description.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.oracle_transformation.description",
            });
        }
        if met.oracle_transformation.script.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.oracle_transformation.script",
            });
        }
        if met.oracle_transformation.version.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.oracle_transformation.version",
            });
        }

        // Required fields non-empty
        for (i, field) in met.required_fields.iter().enumerate() {
            if field.is_empty() {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "meteorology.required_fields",
                    message: format!("required_fields[{}] must not be empty", i),
                });
            }
        }

        // Horizontal/vertical coordinate must be non-empty
        if met.horizontal_coord.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.horizontal_coord",
            });
        }
        if met.vertical_coord.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.vertical_coord",
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        load_checked_in_case, make_minimal_manifest, minimal_manifest_json, parse_json_value,
        valid_real_weather_manifest_json,
    };

    use super::super::*;
    use std::path::Path;

    #[test]
    fn oracle_meteorology_profile_is_required_and_matches_wind_kind() {
        let mut raw = minimal_manifest_json();
        raw.as_object_mut()
            .expect("manifest")
            .remove("oracle_meteorology_profile");
        assert!(parse_json_value(&raw).is_err());

        let mut manifest = make_minimal_manifest();
        manifest.oracle_meteorology_profile.id =
            REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_ID.to_string();
        manifest.oracle_meteorology_profile.manifest_path =
            REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_PATH.to_string();
        let err = manifest
            .validate()
            .expect_err("synthetic wind must use synthetic oracle meteo profile");
        assert!(err.to_string().contains("oracle_meteorology_profile"));
    }

    #[test]
    fn real_weather_meteorology_must_cover_full_simulation_window() {
        let mut raw = minimal_manifest_json();
        raw["wind"] = serde_json::json!({
            "profile": "real_weather",
            "meteorology": {
                "dataset_id": "test",
                "version": "1",
                "source_path": "path",
                "digest": "manifest:path",
                "temporal_coverage": ["20240101000001", "20240101020000"],
                "horizontal_coord": "test",
                "vertical_coord": "test",
                "required_fields": ["u"],
                "candidate_transformation": {"description": "d", "script": "s", "version": "v"},
                "oracle_transformation": {"description": "d", "script": "s", "version": "v"}
            }
        });
        let err = parse_json_value(&raw).expect_err("coverage starting late must fail");
        assert!(err.to_string().contains("full simulation"));

        raw["wind"]["meteorology"]["temporal_coverage"] =
            serde_json::json!(["20231231230000", "20240101005959"]);
        let err = parse_json_value(&raw).expect_err("coverage ending early must fail");
        assert!(err.to_string().contains("full simulation"));
    }

    #[test]
    fn load_and_validate_etex_mini_013() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("ETEX-MINI-013.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load ETEX-MINI-013");
        assert_eq!(manifest.case_id, "ETEX-MINI-013");
        assert_eq!(manifest.schema_version, 2);
        assert!(manifest.physics_switches.turbulence);
        assert!(!manifest.physics_switches.dry_deposition);
        assert!(!manifest.physics_switches.wet_deposition);
        assert_eq!(
            manifest.release.species.physics_contract.profile,
            SpeciesPhysicsProfile::Species024InertV1
        );
        assert!(manifest.surface.is_none());
        assert_eq!(
            (manifest.domain.nx, manifest.domain.ny, manifest.domain.nz),
            (65, 41, 16)
        );
        assert_eq!(manifest.integration.dt_s, 900.0);
        assert_eq!(manifest.integration.steps, 48);
        assert_eq!(manifest.integration.total_s, 43200.0);

        match &manifest.release.geometry {
            SourceGeometry::Box {
                lon_min_deg,
                lon_max_deg,
                lat_min_deg,
                lat_max_deg,
                z_min_m,
                z_max_m,
            } => {
                assert_eq!((*lon_min_deg, *lon_max_deg), (-2.0, -2.0));
                assert_eq!((*lat_min_deg, *lat_max_deg), (48.058, 48.058));
                assert_eq!((*z_min_m, *z_max_m), (5.0, 15.0));
            }
            other => panic!("ETEX release must be a vertical box at one lon/lat, got {other:?}"),
        }
        assert_eq!(
            manifest.release.timing,
            ReleaseTiming::Window {
                start: "19941023160000".to_string(),
                end: "19941024034000".to_string(),
            }
        );
        assert_eq!(manifest.release.particle_count, 10_000);
        assert_eq!(manifest.release.inventory.quantity_kg, 340.0);

        let output_grid = manifest.output_grid.as_ref().expect("ETEX output grid");
        assert_eq!(
            (output_grid.nx, output_grid.ny, output_grid.nz),
            (64, 40, 5)
        );
        assert_eq!(
            output_grid.heights_m,
            vec![100.0, 500.0, 1000.0, 2000.0, 5000.0]
        );

        let candidate = manifest
            .stochastic
            .candidate_philox
            .as_ref()
            .expect("ETEX candidate Philox");
        assert_eq!(candidate.base_key, [0xDECA_FBAD, 0x1234_5678]);
        assert_eq!(candidate.base_counter, [0, 0, 0, 0]);
        assert_eq!(candidate.count, 1);

        let oracle = manifest
            .stochastic
            .oracle_seed
            .as_ref()
            .expect("ETEX oracle identity");
        assert_eq!(oracle.kind, OracleKind::PristineOracle);
        assert_eq!(oracle.seed, None);
        assert_eq!(oracle.repetitions, 1);
        assert!(oracle.strategy.is_none());

        assert_eq!(
            manifest.oracle_command_overrides.turbulence_formulation,
            OracleTurbulenceFormulation::FixedSyncW
        );
        assert_eq!(manifest.oracle_command_overrides.ctl, Some(-5.0));
        assert_eq!(manifest.oracle_command_overrides.ifine, Some(4));
        assert_eq!(manifest.oracle_command_overrides.lsynctime_s, Some(900));
        assert!(manifest
            .representation_differences
            .vertical_coordinate
            .is_some());
        assert!(manifest
            .representation_differences
            .wind_components
            .is_some());
        assert!(manifest
            .representation_differences
            .known_input_equivalence_limitations
            .iter()
            .any(|l| l.code == "INPUT_EQUIVALENCE_NOT_DEMONSTRATED"));
        let serialized = serde_json::to_value(&manifest).expect("serialize ETEX manifest");
        assert!(serialized.get("input_equivalence").is_none());
        manifest.validate().expect("ETEX-MINI-013 should validate");
    }

    #[test]
    fn real_weather_case_requires_complete_meteorology() {
        let mut raw = minimal_manifest_json();
        raw["wind"] = serde_json::json!({
            "profile": "real_weather",
            "meteorology": {}
        });
        let err = parse_json_value(&raw).expect_err("empty meteorology fails");
        let rendered = err.to_string();
        // serde reports "missing field `dataset_id`" for missing required fields
        assert!(
            rendered.contains("dataset_id") || rendered.contains("missing field"),
            "error must name missing dataset_id: {rendered}"
        );
    }

    #[test]
    fn real_weather_meteorology_missing_fields_rejected() {
        let raw = valid_real_weather_manifest_json();
        parse_json_value(&raw).expect("complete meteorology passes");

        let mut missing_dataset_id = raw;
        missing_dataset_id["wind"]["meteorology"]
            .as_object_mut()
            .expect("meteorology object")
            .remove("dataset_id");
        let err = parse_json_value(&missing_dataset_id).expect_err("missing dataset_id fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("dataset_id") || rendered.contains("missing field"),
            "error must name missing dataset_id: {rendered}"
        );
    }

    #[test]
    fn real_weather_meteorology_bad_digest_rejected() {
        let mut raw = minimal_manifest_json();
        raw["wind"] = serde_json::json!({
            "profile": "real_weather",
            "meteorology": {
                "dataset_id": "test",
                "version": "1",
                "source_path": "path",
                "digest": "not-a-valid-digest",
                "temporal_coverage": ["20240101000000", "20240101010000"],
                "horizontal_coord": "test",
                "vertical_coord": "test",
                "required_fields": ["u"],
                "candidate_transformation": {"description": "d", "script": "s", "version": "v"},
                "oracle_transformation": {"description": "d", "script": "s", "version": "v"}
            }
        });
        let err = parse_json_value(&raw).expect_err("bad digest fails");
        assert!(err.to_string().contains("meteorology.digest"));
    }

    #[test]
    fn real_weather_meteorology_temporal_coverage_invalid() {
        let mut raw = minimal_manifest_json();
        raw["wind"] = serde_json::json!({
            "profile": "real_weather",
            "meteorology": {
                "dataset_id": "test",
                "version": "1",
                "source_path": "path",
                "digest": "manifest:path",
                "temporal_coverage": ["not-a-date", "20240101010000"],
                "horizontal_coord": "test",
                "vertical_coord": "test",
                "required_fields": ["u"],
                "candidate_transformation": {"description": "d", "script": "s", "version": "v"},
                "oracle_transformation": {"description": "d", "script": "s", "version": "v"}
            }
        });
        let err = parse_json_value(&raw).expect_err("bad temporal_coverage fails");
        assert!(err.to_string().contains("temporal_coverage"));
    }

    #[test]
    fn real_weather_meteorology_temporal_coverage_end_before_start_rejected() {
        let mut raw = minimal_manifest_json();
        raw["wind"] = serde_json::json!({
            "profile": "real_weather",
            "meteorology": {
                "dataset_id": "test",
                "version": "1",
                "source_path": "path",
                "digest": "manifest:path",
                "temporal_coverage": ["20240101010000", "20240101000000"],
                "horizontal_coord": "test",
                "vertical_coord": "test",
                "required_fields": ["u"],
                "candidate_transformation": {"description": "d", "script": "s", "version": "v"},
                "oracle_transformation": {"description": "d", "script": "s", "version": "v"}
            }
        });
        let err = parse_json_value(&raw).expect_err("end before start fails");
        assert!(err.to_string().contains("temporal_coverage"));
    }

    #[test]
    fn real_weather_meteorology_bad_transformation_rejected() {
        let mut raw = minimal_manifest_json();
        raw["wind"] = serde_json::json!({
            "profile": "real_weather",
            "meteorology": {
                "dataset_id": "test",
                "version": "1",
                "source_path": "path",
                "digest": "manifest:path",
                "temporal_coverage": ["20240101000000", "20240101010000"],
                "horizontal_coord": "test",
                "vertical_coord": "test",
                "required_fields": ["u"],
                "candidate_transformation": {"description": "", "script": "s", "version": "v"},
                "oracle_transformation": {"description": "d", "script": "s", "version": "v"}
            }
        });
        let err = parse_json_value(&raw).expect_err("empty candidate description fails");
        assert!(err
            .to_string()
            .contains("candidate_transformation.description"));
    }

    #[test]
    fn real_weather_meteorology_bad_manifest_digest_path_rejected() {
        for digest in [
            "manifest:",
            "manifest:/absolute/path.json",
            "manifest:../outside.json",
            "manifest:fixtures//DIGESTS.json",
            "manifest:fixtures\\DIGESTS.json",
        ] {
            let mut raw = minimal_manifest_json();
            raw["wind"] = serde_json::json!({
                "profile": "real_weather",
                "meteorology": {
                    "dataset_id": "test",
                    "version": "1",
                    "source_path": "fixtures/weather/",
                    "digest": digest,
                    "temporal_coverage": ["20240101000000", "20240101010000"],
                    "horizontal_coord": "test",
                    "vertical_coord": "test",
                    "required_fields": ["u"],
                    "candidate_transformation": {"description": "d", "script": "s", "version": "v"},
                    "oracle_transformation": {"description": "d", "script": "s", "version": "v"}
                }
            });
            let err = parse_json_value(&raw).expect_err("bad manifest digest path must fail");
            assert!(
                err.to_string().contains("meteorology.digest"),
                "unexpected for {digest}: {err}"
            );
        }
    }

    #[test]
    fn real_weather_meteorology_bad_source_path_rejected() {
        let mut raw = minimal_manifest_json();
        raw["wind"] = serde_json::json!({
            "profile": "real_weather",
            "meteorology": {
                "dataset_id": "test",
                "version": "1",
                "source_path": "/absolute/path",
                "digest": "manifest:path",
                "temporal_coverage": ["20240101000000", "20240101010000"],
                "horizontal_coord": "test",
                "vertical_coord": "test",
                "required_fields": ["u"],
                "candidate_transformation": {"description": "d", "script": "s", "version": "v"},
                "oracle_transformation": {"description": "d", "script": "s", "version": "v"}
            }
        });
        let err = parse_json_value(&raw).expect_err("absolute source_path fails");
        assert!(err.to_string().contains("source_path"));
    }

    #[test]
    fn real_weather_meteorology_with_only_note_fails() {
        // The old NativeEra5 variant had only a note; this must fail with the new schema
        let mut raw = minimal_manifest_json();
        raw["wind"] = serde_json::json!({
            "profile": "real_weather",
            "meteorology": {
                "note": "some note"
            }
        });
        let err = parse_json_value(&raw).expect_err("note-only meteorology fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("dataset_id") || rendered.contains("missing field"),
            "error must name missing dataset_id: {rendered}"
        );
    }

    #[test]
    fn synthetic_cases_do_not_require_meteorology() {
        let adv = load_checked_in_case("ADV-ANA-001");
        match &adv.wind {
            WindSpec::Uniform { .. } => {}
            other => panic!("ADV-ANA-001 wind changed: {other:?}"),
        }
        // Should validate without meteorology
        adv.validate()
            .expect("ADV-ANA-001 validates without meteorology");

        let wind = load_checked_in_case("WIND-UNI-002");
        match &wind.wind {
            WindSpec::Uniform { .. } => {}
            other => panic!("WIND-UNI-002 wind changed: {other:?}"),
        }
        wind.validate()
            .expect("WIND-UNI-002 validates without meteorology");
    }

    #[test]
    fn etex_mini_013_meteorology_complete() {
        let etex = load_checked_in_case("ETEX-MINI-013");
        match &etex.wind {
            WindSpec::RealWeather { meteorology } => {
                assert_eq!(meteorology.dataset_id, "era5-native-mini-19941023-24");
                assert_eq!(meteorology.version, "2024-09-19");
                assert_eq!(meteorology.source_path, "fixtures/etex/native-mini/");
                assert_eq!(
                    meteorology.digest,
                    "manifest:fixtures/etex/native-mini/DIGESTS.json"
                );
                assert_eq!(meteorology.temporal_coverage[0], "19941023150000");
                assert_eq!(meteorology.temporal_coverage[1], "19941024060000");
                assert_eq!(meteorology.horizontal_coord, "geographic_lon_lat_degrees");
                assert_eq!(meteorology.vertical_coord, "era5_native_hybrid_137_levels");
                assert!(!meteorology.required_fields.is_empty());
                assert!(!meteorology.candidate_transformation.description.is_empty());
                assert!(!meteorology.candidate_transformation.script.is_empty());
                assert!(!meteorology.candidate_transformation.version.is_empty());
                assert!(!meteorology.oracle_transformation.description.is_empty());
                assert!(!meteorology.oracle_transformation.script.is_empty());
                assert!(!meteorology.oracle_transformation.version.is_empty());
            }
            other => panic!("ETEX-MINI-013 wind changed: {other:?}"),
        }
        etex.validate()
            .expect("ETEX-MINI-013 validates with complete meteorology");
    }

    #[test]
    fn synthetic_cases_reject_real_weather_profile() {
        let mut raw = minimal_manifest_json();
        raw["wind"] = serde_json::json!({
            "profile": "real_weather",
            "meteorology": {}
        });
        let err = parse_json_value(&raw).expect_err("synthetic case with real_weather fails");
        let rendered = err.to_string();
        // serde reports missing field for missing required fields
        assert!(
            rendered.contains("dataset_id") || rendered.contains("missing field"),
            "error must name missing dataset_id: {rendered}"
        );
    }
}
