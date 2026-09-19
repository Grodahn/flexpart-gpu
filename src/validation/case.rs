//! Versioned validation case manifest contract (Issue #51).
//!
//! Defines a machine-readable schema for validation cases that captures all
//! physics-relevant configuration, stochastic identities, execution profile
//! references, and expected artifacts. Supports both `pristine-oracle` and
//! `seedable-validation-oracle` strategies from issue #50.
//!
//! Only `schema_version` 2 is accepted. The legacy v1 shape (`version`,
//! `seeds`, uppercase overrides) is frozen and unsupported: every checked-in
//! case is migrated to v2 and parsers reject v1 fail-closed. See
//! `fixtures/corpus/cases/MIGRATION_NOTES.md` for the field-by-field mapping.

use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Schema version for the validation case manifest.
pub const VALIDATION_CASE_SCHEMA_VERSION: u32 = 2;

/// Oracle kind as defined in issue #50 stochastic identity contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OracleKind {
    /// Unmodified FLEXPART 11.1 at pinned commit, no seed control.
    PristineOracle,
    /// Patched FLEXPART 11.1 with validation-only RNG initialization.
    SeedableValidationOracle,
}

impl FromStr for OracleKind {
    type Err = ValidationCaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pristine-oracle" => Ok(OracleKind::PristineOracle),
            "seedable-validation-oracle" => Ok(OracleKind::SeedableValidationOracle),
            _ => Err(ValidationCaseError::InvalidOracleKind(s.to_string())),
        }
    }
}

/// Stochastic identity specification for a validation case.
///
/// Candidate Philox identities and FLEXPART oracle identities are separate
/// RNG namespaces per issue #50 contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StochasticIdentitySpec {
    /// Candidate RNG namespace: Philox key/counter for the GPU candidate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_philox: Option<CandidatePhiloxIdentity>,
    /// Oracle RNG namespace: validation seed identity for FLEXPART oracle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oracle_seed: Option<OracleSeedIdentity>,
}

/// Candidate-side Philox identity (separate RNG namespace from oracle).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidatePhiloxIdentity {
    /// Base Philox key [key0, key1] for seed derivation.
    pub base_key: [u32; 2],
    /// Base Philox counter for the first timestep.
    pub base_counter: [u32; 4],
    /// Number of independent seeds in the ensemble.
    pub count: u32,
    /// Derivation rule: "seed i uses key [base0 + i, base1] with zeroed counter"
    pub derivation: String,
    /// When true, every seed reuses the base key unchanged to prove
    /// bit-identical reruns (REPEAT-009). Defaults to false (wrapping
    /// derivation). Introduced in the v1 -> v2 migration to make repeat
    /// semantics explicit instead of hard-coding case IDs in tooling.
    #[serde(default)]
    pub identical_repeats: bool,
}

impl CandidatePhiloxIdentity {
    /// Derive the Philox key for seed index `i`.
    ///
    /// Canonical rule: `[base0.wrapping_add(i), base1]`, unless
    /// `identical_repeats` is set, in which case the base key is reused
    /// unchanged for every seed.
    #[must_use]
    pub fn key_for_seed_index(&self, seed_index: u32) -> [u32; 2] {
        if self.identical_repeats {
            return self.base_key;
        }
        [
            self.base_key[0].wrapping_add(seed_index),
            self.base_key[1],
        ]
    }

    /// Counter for seed index `i` (currently the declared base counter).
    #[must_use]
    pub fn counter_for_seed_index(&self, _seed_index: u32) -> [u32; 4] {
        self.base_counter
    }
}

/// Driver deposition forcing carried by the case manifest.
///
/// Mirrors the legacy v1 `deposition` block key-for-key so migrated cases
/// keep byte-identical scientific values. `None` when both deposition
/// switches are off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepositionSpec {
    /// Dry deposition velocity [m/s].
    pub dry_deposition_velocity_m_s: f32,
    /// Dry deposition reference height [m] (FLEXPART `href` layer scale).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_reference_height_m: Option<f32>,
    /// Wet scavenging coefficient [1/s].
    pub wet_scavenging_coefficient_s_inv: f32,
    /// Precipitating fraction [0, 1].
    pub wet_precipitating_fraction: f32,
}

/// Oracle-side seed identity per issue #50 contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OracleSeedIdentity {
    /// Oracle kind: pristine-oracle or seedable-validation-oracle.
    pub kind: OracleKind,
    /// Validation seed value (canonical decimal in [1, 1000000000]).
    /// Omit or set to null for default mode (pristine bit-exact initialization).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    /// Number of repetitions for repeatability characterization.
    #[serde(default = "default_repetitions")]
    pub repetitions: u32,
}

fn default_repetitions() -> u32 {
    5
}

/// Execution profile reference (from frozen #49 contract).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionProfileRef {
    /// Profile ID (e.g., "flexpart-11.1-single-thread").
    pub id: String,
    /// Profile version.
    pub version: u32,
    /// Manifest file path for verification.
    pub manifest_path: String,
}

/// Domain specification with explicit units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainSpec {
    /// Number of grid cells in x.
    pub nx: u32,
    /// Number of grid cells in y.
    pub ny: u32,
    /// Number of vertical levels.
    pub nz: u32,
    /// Grid spacing in x [degrees].
    pub dx_deg: f32,
    /// Grid spacing in y [degrees].
    pub dy_deg: f32,
    /// Origin longitude [degrees].
    pub xlon0_deg: f32,
    /// Origin latitude [degrees].
    pub ylat0_deg: f32,
    /// Vertical level heights [m AGL].
    pub wind_heights_m: Vec<f32>,
}

/// Release specification with explicit units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseSpec {
    /// Release longitude [degrees].
    pub lon_deg: f32,
    /// Release latitude [degrees].
    pub lat_deg: f32,
    /// Release height [m AGL].
    pub z_m: f32,
    /// Total particle count.
    pub particle_count: u32,
    /// Total mass [kg] distributed over all particles.
    pub mass_kg_total: f32,
    /// Per-particle mass [kg] (alternative to `mass_kg_total`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mass_kg_per_particle: Option<f32>,
}

/// Wind field specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "profile", rename_all = "snake_case")]
pub enum WindSpec {
    /// Uniform wind field.
    Uniform {
        u_m_s: f32,
        v_m_s: f32,
        w_m_s: f32,
    },
    /// Linear shear wind profile u(z) = u0 + shear * z.
    LinearShear {
        u0_m_s: f32,
        u_shear_per_s: f32,
        v_m_s: f32,
        w_m_s: f32,
    },
    /// Real-weather wind from native ERA5 model levels (ETEX, etc.).
    NativeEra5 {
        /// Path to the prepared meteorology relative to case fixtures.
        #[serde(skip_serializing_if = "Option::is_none")]
        met_path: Option<String>,
        /// Note describing the source and processing.
        note: String,
    },
}

/// Surface fields specification with explicit units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceSpec {
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

/// Integration timestep and window specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntegrationSpec {
    /// Simulation start timestamp (YYYYMMDDHHMMSS).
    pub start: String,
    /// Integration timestep [s].
    pub dt_s: f32,
    /// Number of integration steps.
    pub steps: u32,
    /// Total simulation duration [s] (`dt_s` * steps).
    pub total_s: f32,
}

/// Physics switches controlling model processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicsSwitches {
    /// Turbulence (Hanna/Langevin).
    pub turbulence: bool,
    /// Convection (Emanuel scheme).
    pub convection: bool,
    /// Dry deposition.
    pub dry_deposition: bool,
    /// Wet deposition (scavenging).
    pub wet_deposition: bool,
    /// Radioactive decay.
    pub decay: bool,
}

/// Explicit units for all physical quantities in the case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitsSpec {
    /// Wind velocity unit (typically "m/s").
    pub wind: String,
    /// Pressure unit (typically "Pa").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure: Option<String>,
    /// Temperature unit (typically "K").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<String>,
    /// Heat flux unit (typically "W/m2").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heat_flux: Option<String>,
    /// Height/length unit (typically "m").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<String>,
    /// Mass unit (typically "kg").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mass: Option<String>,
    /// Time unit (typically "s").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    /// Shear unit (typically "1/s").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shear: Option<String>,
    /// Inverse Obukhov length unit (typically "1/m").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inv_obukhov: Option<String>,
    /// Deposition velocity unit (typically "m/s").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposition_velocity: Option<String>,
    /// Scavenging coefficient unit (typically "1/s").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scavenging_coefficient: Option<String>,
    /// Concentration unit (typically "kg/m3" or "pg/m3").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concentration: Option<String>,
}

/// Expected artifacts produced by a validation run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectedArtifacts {
    /// Candidate output directory pattern.
    pub candidate_dir: String,
    /// Oracle output directory pattern.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oracle_dir: Option<String>,
    /// Comparison report path.
    pub comparison_report: String,
    /// Run manifest path (provenance).
    pub run_manifest: String,
}

/// Oracle command overrides (namelist values).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OracleCommandOverrides {
    /// Turbulence flag (0/1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lturbulence: Option<u8>,
    /// Convection flag (0/1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lconvection: Option<u8>,
    /// CTL parameter (Hanna turbulence scaling).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctl: Option<f32>,
    /// IFINE sub-stepping factor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ifine: Option<u32>,
    /// Dry deposition flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ldrydep: Option<u8>,
    /// Wet deposition flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lwetdep: Option<u8>,
    /// Decay flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ldecay: Option<u8>,
}

/// Structured representation differences for #52 input-equivalence verdicts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RepresentationDifferences {
    /// Vertical coordinate differences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertical_coordinate: Option<String>,
    /// Horizontal grid differences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizontal_grid: Option<String>,
    /// Temporal resolution differences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal_resolution: Option<String>,
    /// Wind component differences (e.g., etadot vs omega-derived w).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wind_components: Option<String>,
    /// PBL diagnostic differences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pbl_diagnostics: Option<String>,
    /// Additional notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<String>>,
}

/// Input equivalence status for #52.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEquivalenceStatus {
    /// Input equivalence has been demonstrated.
    Demonstrated,
    /// Input equivalence has not been demonstrated (default for real-weather cases).
    NotDemonstrated,
    /// Input equivalence is not applicable (analytic/synthetic cases).
    NotApplicable,
}

/// Complete validation case manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationCaseManifest {
    /// Schema version (must equal `VALIDATION_CASE_SCHEMA_VERSION`).
    pub schema_version: u32,
    /// Unique case identifier (e.g., "ADV-ANA-001").
    pub case_id: String,
    /// Human-readable description.
    pub description: String,
    /// Domain specification.
    pub domain: DomainSpec,
    /// Release specification.
    pub release: ReleaseSpec,
    /// Wind field specification.
    pub wind: WindSpec,
    /// Surface fields specification (optional for analytic cases).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<SurfaceSpec>,
    /// Integration specification.
    pub integration: IntegrationSpec,
    /// Physics switches.
    pub physics_switches: PhysicsSwitches,
    /// Driver deposition forcing. `None` when deposition is off.
    /// Migrated key-for-key from the legacy v1 `deposition` block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposition: Option<DepositionSpec>,
    /// Explicit units for all quantities.
    pub units: UnitsSpec,
    /// Stochastic identity specification (candidate + oracle).
    pub stochastic: StochasticIdentitySpec,
    /// Execution profile reference (frozen #49).
    pub execution_profile: ExecutionProfileRef,
    /// Oracle COMMAND namelist overrides.
    #[serde(default)]
    pub oracle_command_overrides: OracleCommandOverrides,
    /// Expected output artifacts.
    pub expected_artifacts: ExpectedArtifacts,
    /// Structured representation differences for #52.
    #[serde(default)]
    pub representation_differences: RepresentationDifferences,
    /// Input equivalence status for #52.
    #[serde(default = "default_input_equivalence")]
    pub input_equivalence: InputEquivalenceStatus,
    /// Additional notes.
    #[serde(default)]
    pub notes: Vec<String>,
}

fn default_input_equivalence() -> InputEquivalenceStatus {
    InputEquivalenceStatus::NotApplicable
}

/// Errors for validation case manifest handling.
#[derive(Debug, Error)]
pub enum ValidationCaseError {
    #[error("failed to read case file `{path}`: {source}")]
    ReadFile {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse case JSON `{path}`: {source}")]
    ParseJson {
        path: std::path::PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("schema version mismatch: expected {expected}, got {actual}")]
    SchemaVersionMismatch { expected: u32, actual: u32 },
    #[error("missing required field: {field}")]
    MissingField { field: &'static str },
    #[error("invalid oracle kind: {0}")]
    InvalidOracleKind(String),
    #[error("invalid physics switch configuration: {message}")]
    InvalidPhysicsSwitches { message: String },
    #[error("invalid stochastic identity: {message}")]
    InvalidStochasticIdentity { message: String },
    #[error("invalid execution profile reference: {message}")]
    InvalidExecutionProfile { message: String },
    #[error("ambiguous field: {field} - {message}")]
    AmbiguousField { field: &'static str, message: String },
    #[error("unit mismatch: {field} expected {expected}, got {actual}")]
    UnitMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
}

impl ValidationCaseManifest {
    /// Load a validation case manifest from a JSON file.
    ///
    /// # Errors
    /// Returns [`ValidationCaseError::ReadFile`] if the file cannot be read,
    /// [`ValidationCaseError::ParseJson`] if the JSON is malformed,
    /// or a validation error if the manifest fails schema validation.
    pub fn load_from_file(path: &Path) -> Result<Self, ValidationCaseError> {
        let content = std::fs::read_to_string(path).map_err(|source| ValidationCaseError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&content, path)
    }

    /// Parse a validation case manifest from a JSON string.
    ///
    /// Only `schema_version` 2 documents are accepted. Legacy v1 documents
    /// (key `version`) and any other schema version are rejected with a
    /// field-specific version error; v1 is frozen and unsupported, see
    /// `fixtures/corpus/cases/MIGRATION_NOTES.md`.
    ///
    /// # Errors
    /// Returns [`ValidationCaseError::ParseJson`] if the JSON is malformed,
    /// [`ValidationCaseError::SchemaVersionMismatch`] if the schema version
    /// is missing, ambiguous, or not 2, or a validation error if the
    /// manifest fails schema validation.
    pub fn parse(content: &str, path: &Path) -> Result<Self, ValidationCaseError> {
        let raw: serde_json::Value =
            serde_json::from_str(content).map_err(|source| ValidationCaseError::ParseJson {
                path: path.to_path_buf(),
                source,
            })?;
        if raw.get("schema_version").is_some() && raw.get("version").is_some() {
            return Err(ValidationCaseError::AmbiguousField {
                field: "schema_version/version",
                message: "document contains both v2 `schema_version` and legacy v1 `version`; ambiguous mixed-version document rejected".to_string(),
            });
        }
        match (
            raw.get("schema_version").and_then(|v| v.as_u64()),
            raw.get("version").and_then(|v| v.as_u64()),
        ) {
            (Some(2), None) => {}
            (Some(actual), None) => {
                let actual = u32::try_from(actual).unwrap_or(u32::MAX);
                return Err(ValidationCaseError::SchemaVersionMismatch {
                    expected: VALIDATION_CASE_SCHEMA_VERSION,
                    actual,
                });
            }
            (None, Some(legacy)) => {
                let legacy = u32::try_from(legacy).unwrap_or(u32::MAX);
                return Err(ValidationCaseError::SchemaVersionMismatch {
                    expected: VALIDATION_CASE_SCHEMA_VERSION,
                    actual: legacy,
                });
            }
            (None, None) => {
                return Err(ValidationCaseError::MissingField {
                    field: "schema_version",
                });
            }
            _ => {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "schema_version/version",
                    message: "ambiguous schema version declaration".to_string(),
                });
            }
        }
        let manifest: Self =
            serde_json::from_value(raw).map_err(|source| ValidationCaseError::ParseJson {
                path: path.to_path_buf(),
                source,
            })?;

        // Fail-closed validation of all required fields
        manifest.validate()?;

        Ok(manifest)
    }

    /// Validate the manifest (fail-closed).
    ///
    /// # Errors
    /// Returns a [`ValidationCaseError`] variant describing the first validation failure.
    pub fn validate(&self) -> Result<(), ValidationCaseError> {
        // Validate case_id format
        if self.case_id.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "case_id",
            });
        }

        // Validate domain
        if self.domain.nx == 0 || self.domain.ny == 0 || self.domain.nz == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "domain dimensions must be > 0".to_string(),
            });
        }
        if self.domain.wind_heights_m.len() != self.domain.nz as usize {
            return Err(ValidationCaseError::AmbiguousField {
                field: "domain.wind_heights_m",
                message: format!(
                    "wind_heights_m length ({}) must equal domain.nz ({})",
                    self.domain.wind_heights_m.len(),
                    self.domain.nz
                ),
            });
        }
        if self.domain.wind_heights_m.windows(2).any(|w| w[1] <= w[0]) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "wind_heights_m must be strictly increasing".to_string(),
            });
        }

        // Validate release
        if self.release.particle_count == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "release.particle_count must be > 0".to_string(),
            });
        }
        if self.release.mass_kg_total <= 0.0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "release.mass_kg_total must be > 0".to_string(),
            });
        }

        // Validate integration
        if self.integration.dt_s <= 0.0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "integration.dt_s must be > 0".to_string(),
            });
        }
        if self.integration.steps == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "integration.steps must be > 0".to_string(),
            });
        }
        let expected_total = self.integration.dt_s * self.integration.steps as f32;
        if (self.integration.total_s - expected_total).abs() > 1e-6 {
            return Err(ValidationCaseError::AmbiguousField {
                field: "integration.total_s",
                message: format!(
                    "total_s ({}) must equal dt_s * steps ({})",
                    self.integration.total_s, expected_total
                ),
            });
        }

        // Validate physics switches consistency with other fields
        self.validate_physics_consistency()?;

        // Validate stochastic identity
        self.validate_stochastic()?;

        // Validate Oracle command overrides (required, no hidden defaults)
        self.validate_oracle_overrides()?;

        // Validate deposition forcing against deposition switches
        self.validate_deposition()?;

        // Validate execution profile reference
        if self.execution_profile.id.is_empty() {
            return Err(ValidationCaseError::InvalidExecutionProfile {
                message: "execution_profile.id must not be empty".to_string(),
            });
        }
        if self.execution_profile.version == 0 {
            return Err(ValidationCaseError::InvalidExecutionProfile {
                message: "execution_profile.version must be > 0".to_string(),
            });
        }

        // Validate units (check required fields present)
        if self.units.wind.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "units.wind",
            });
        }

        // Validate expected artifacts
        if self.expected_artifacts.candidate_dir.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "expected_artifacts.candidate_dir",
            });
        }
        if self.expected_artifacts.comparison_report.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "expected_artifacts.comparison_report",
            });
        }
        if self.expected_artifacts.run_manifest.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "expected_artifacts.run_manifest",
            });
        }

        Ok(())
    }

    fn validate_physics_consistency(&self) -> Result<(), ValidationCaseError> {
        // Surface fields required when turbulence is enabled
        if self.physics_switches.turbulence && self.surface.is_none() {
            return Err(ValidationCaseError::AmbiguousField {
                field: "surface",
                message: "surface fields required when physics_switches.turbulence=true".to_string(),
            });
        }

        // If turbulence is disabled, surface should be None or empty
        if !self.physics_switches.turbulence && self.surface.is_some() {
            // Allow surface to be present but warn via notes - this is a valid config
            // for cases that define surface for oracle but disable turbulence in candidate
        }

        // Check wind profile consistency with domain heights
        match &self.wind {
            WindSpec::Uniform { .. }
            | WindSpec::LinearShear { .. }
            | WindSpec::NativeEra5 { .. } => {}
        }

        // Deposition switches require surface fields with relevant parameters
        if (self.physics_switches.dry_deposition || self.physics_switches.wet_deposition)
            && self.surface.is_none()
        {
            return Err(ValidationCaseError::AmbiguousField {
                field: "surface",
                message: "surface fields required when deposition is enabled".to_string(),
            });
        }

        // Decay requires species configuration (not in this manifest but flagged)
        if self.physics_switches.decay {
            // This is a flag for downstream - species config lives in SPECIES/ files
        }

        Ok(())
    }

    fn validate_stochastic(&self) -> Result<(), ValidationCaseError> {
        // Candidate Philox identity validation
        if let Some(candidate) = &self.stochastic.candidate_philox {
            if candidate.count == 0 {
                return Err(ValidationCaseError::InvalidStochasticIdentity {
                    message: "candidate_philox.count must be > 0".to_string(),
                });
            }
            if candidate.derivation.is_empty() {
                return Err(ValidationCaseError::InvalidStochasticIdentity {
                    message: "candidate_philox.derivation must not be empty".to_string(),
                });
            }
        }

        // Oracle seed identity validation
        if let Some(oracle) = &self.stochastic.oracle_seed {
            if let Some(seed) = oracle.seed {
                if seed == 0 || seed > 1_000_000_000 {
                    return Err(ValidationCaseError::InvalidStochasticIdentity {
                        message: format!("oracle_seed.seed must be in [1, 1000000000], got {seed}"),
                    });
                }
            }
            if oracle.repetitions == 0 {
                return Err(ValidationCaseError::InvalidStochasticIdentity {
                    message: "oracle_seed.repetitions must be > 0".to_string(),
                });
            }
        }

        // For analytic cases with no RNG consumption, both can be None
        // For stochastic cases, at least one should be specified
        if self.physics_switches.turbulence
            && self.stochastic.candidate_philox.is_none()
            && self.stochastic.oracle_seed.is_none()
        {
            return Err(ValidationCaseError::AmbiguousField {
                field: "stochastic",
                message: "stochastic identity required when turbulence is enabled".to_string(),
            });
        }

        Ok(())
    }

    fn validate_oracle_overrides(&self) -> Result<(), ValidationCaseError> {
        let overrides = &self.oracle_command_overrides;
        let flag = |name: &'static str, value: Option<u8>| -> Result<u8, ValidationCaseError> {
            let value = value.ok_or(ValidationCaseError::MissingField { field: name })?;
            if value != 0 && value != 1 {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{name} must be exactly 0 or 1, got {value}"),
                });
            }
            Ok(value)
        };
        let lturbulence = flag(
            "oracle_command_overrides.lturbulence",
            overrides.lturbulence,
        )?;
        let lconvection = flag(
            "oracle_command_overrides.lconvection",
            overrides.lconvection,
        )?;
        let ctl = overrides.ctl.ok_or(ValidationCaseError::MissingField {
            field: "oracle_command_overrides.ctl",
        })?;
        if !ctl.is_finite() {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!("oracle_command_overrides.ctl must be finite, got {ctl}"),
            });
        }
        let ifine = overrides.ifine.ok_or(ValidationCaseError::MissingField {
            field: "oracle_command_overrides.ifine",
        })?;
        if !(1..=10).contains(&ifine) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!("oracle_command_overrides.ifine must be in 1..=10, got {ifine}"),
            });
        }
        for (name, value) in [
            ("oracle_command_overrides.ldrydep", overrides.ldrydep),
            ("oracle_command_overrides.lwetdep", overrides.lwetdep),
            ("oracle_command_overrides.ldecay", overrides.ldecay),
        ] {
            if let Some(value) = value {
                if value != 0 && value != 1 {
                    return Err(ValidationCaseError::InvalidPhysicsSwitches {
                        message: format!("{name} must be exactly 0 or 1, got {value}"),
                    });
                }
            }
        }
        if self.physics_switches.turbulence != (lturbulence == 1) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "physics_switches.turbulence={} conflicts with oracle lturbulence={lturbulence}",
                    self.physics_switches.turbulence
                ),
            });
        }
        if self.physics_switches.convection != (lconvection == 1) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "physics_switches.convection={} conflicts with oracle lconvection={lconvection}",
                    self.physics_switches.convection
                ),
            });
        }
        Ok(())
    }

    fn validate_deposition(&self) -> Result<(), ValidationCaseError> {
        let dry = self.physics_switches.dry_deposition;
        let wet = self.physics_switches.wet_deposition;
        let Some(spec) = &self.deposition else {
            // Absent block means forcing is defined by the execution pipeline
            // (e.g. ETEX-MINI-013); present blocks are strictly validated.
            return Ok(());
        };
        if !dry && !wet {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "deposition block must be absent when deposition switches are off".to_string(),
            });
        }
        if !spec.dry_deposition_velocity_m_s.is_finite()
            || spec.dry_deposition_velocity_m_s < 0.0
        {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "deposition.dry_deposition_velocity_m_s must be finite and >= 0, got {}",
                    spec.dry_deposition_velocity_m_s
                ),
            });
        }
        if dry && !(spec.dry_deposition_velocity_m_s > 0.0) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "deposition.dry_deposition_velocity_m_s must be > 0 when physics_switches.dry_deposition=true".to_string(),
            });
        }
        if let Some(href) = spec.dry_reference_height_m {
            if !href.is_finite() || href <= 0.0 {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!(
                        "deposition.dry_reference_height_m must be finite and > 0, got {href}"
                    ),
                });
            }
        } else if dry {
            return Err(ValidationCaseError::MissingField {
                field: "deposition.dry_reference_height_m",
            });
        }
        if !spec.wet_scavenging_coefficient_s_inv.is_finite()
            || spec.wet_scavenging_coefficient_s_inv < 0.0
        {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "deposition.wet_scavenging_coefficient_s_inv must be finite and >= 0, got {}",
                    spec.wet_scavenging_coefficient_s_inv
                ),
            });
        }
        if !(0.0..=1.0).contains(&spec.wet_precipitating_fraction)
            || !spec.wet_precipitating_fraction.is_finite()
        {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "deposition.wet_precipitating_fraction must be in [0, 1], got {}",
                    spec.wet_precipitating_fraction
                ),
            });
        }
        if wet
            && !(spec.wet_scavenging_coefficient_s_inv > 0.0
                && spec.wet_precipitating_fraction > 0.0)
        {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "deposition wet scavenging coefficient and precipitating fraction must be > 0 when physics_switches.wet_deposition=true".to_string(),
            });
        }
        Ok(())
    }

    /// Resolve the candidate Philox key/counter for seed index `i`.
    ///
    /// Fail-closed: returns [`ValidationCaseError::InvalidStochasticIdentity`]
    /// when no candidate RNG identity is declared. Deterministic cases
    /// (e.g. `ADV-ANA-001`) must not call this; they declare no identity.
    ///
    /// # Errors
    /// Returns [`ValidationCaseError::InvalidStochasticIdentity`] if
    /// `stochastic.candidate_philox` is missing.
    pub fn candidate_seed_identity(
        &self,
        seed_index: u32,
    ) -> Result<([u32; 2], [u32; 4]), ValidationCaseError> {
        let Some(candidate) = &self.stochastic.candidate_philox else {
            return Err(ValidationCaseError::InvalidStochasticIdentity {
                message: format!(
                    "case {} declares no stochastic.candidate_philox; stochastic cases must declare a Philox identity, deterministic cases must not request one",
                    self.case_id
                ),
            });
        };
        Ok((
            candidate.key_for_seed_index(seed_index),
            candidate.counter_for_seed_index(seed_index),
        ))
    }

    /// Write the manifest to a JSON file (round-trip serialization).
    ///
    /// # Errors
    /// Returns [`ValidationCaseError::ParseJson`] if serialization fails,
    /// or [`ValidationCaseError::ReadFile`] if the file cannot be written.
    pub fn write_to_file(&self, path: &Path) -> Result<(), ValidationCaseError> {
        let json = serde_json::to_string_pretty(self).map_err(|source| ValidationCaseError::ParseJson {
            path: path.to_path_buf(),
            source,
        })?;
        std::fs::write(path, json).map_err(|source| ValidationCaseError::ReadFile {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_minimal_manifest() -> ValidationCaseManifest {
        ValidationCaseManifest {
            schema_version: VALIDATION_CASE_SCHEMA_VERSION,
            case_id: "TEST-001".to_string(),
            description: "Minimal test case".to_string(),
            domain: DomainSpec {
                nx: 32,
                ny: 32,
                nz: 8,
                dx_deg: 0.1,
                dy_deg: 0.1,
                xlon0_deg: 9.5,
                ylat0_deg: 8.5,
                wind_heights_m: vec![50.0, 100.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0],
            },
            release: ReleaseSpec {
                lon_deg: 10.0,
                lat_deg: 10.0,
                z_m: 50.0,
                particle_count: 1000,
                mass_kg_total: 1.0,
                mass_kg_per_particle: None,
            },
            wind: WindSpec::Uniform {
                u_m_s: 5.0,
                v_m_s: -3.0,
                w_m_s: 0.0,
            },
            surface: Some(SurfaceSpec {
                surface_pressure_pa: 101325.0,
                temperature_2m_k: 289.0,
                dewpoint_2m_k: 284.0,
                sensible_heat_flux_w_m2: 0.0,
                solar_radiation_w_m2: 120.0,
                surface_stress_n_m2: 0.2,
                friction_velocity_m_s: 0.35,
                convective_velocity_scale_m_s: 0.0,
                mixing_height_m: 1500.0,
                tropopause_height_m: 10000.0,
                inv_obukhov_length_per_m: 0.0,
                precip_large_scale_mm_h: 0.0,
                precip_convective_mm_h: 0.0,
            }),
            integration: IntegrationSpec {
                start: "20240101000000".to_string(),
                dt_s: 300.0,
                steps: 12,
                total_s: 3600.0,
            },
            physics_switches: PhysicsSwitches {
                turbulence: true,
                convection: false,
                dry_deposition: false,
                wet_deposition: false,
                decay: false,
            },
            deposition: None,
            units: UnitsSpec {
                wind: "m/s".to_string(),
                pressure: Some("Pa".to_string()),
                temperature: Some("K".to_string()),
                heat_flux: Some("W/m2".to_string()),
                height: Some("m".to_string()),
                mass: Some("kg".to_string()),
                time: Some("s".to_string()),
                shear: Some("1/s".to_string()),
                inv_obukhov: Some("1/m".to_string()),
                deposition_velocity: Some("m/s".to_string()),
                scavenging_coefficient: Some("1/s".to_string()),
                concentration: Some("kg/m3".to_string()),
            },
            stochastic: StochasticIdentitySpec {
                candidate_philox: Some(CandidatePhiloxIdentity {
                    base_key: [3737180555, 305419896],
                    base_counter: [0, 0, 0, 0],
                    count: 10,
                    derivation: "seed i uses key [base0 + i, base1] with zeroed counter".to_string(),
                    identical_repeats: false,
                }),
                oracle_seed: Some(OracleSeedIdentity {
                    kind: OracleKind::SeedableValidationOracle,
                    seed: Some(1),
                    repetitions: 5,
                }),
            },
            execution_profile: ExecutionProfileRef {
                id: "flexpart-11.1-single-thread".to_string(),
                version: 1,
                manifest_path: "reference/flexpart-11.1.json".to_string(),
            },
            oracle_command_overrides: OracleCommandOverrides {
                lturbulence: Some(1),
                ctl: Some(5.0),
                ifine: Some(4),
                lconvection: Some(0),
                ..Default::default()
            },
            expected_artifacts: ExpectedArtifacts {
                candidate_dir: "target/corpus/candidate/TEST-001".to_string(),
                oracle_dir: Some("target/corpus/oracle/TEST-001".to_string()),
                comparison_report: "target/corpus/comparison_report.json".to_string(),
                run_manifest: "target/corpus/run_manifest.json".to_string(),
            },
            representation_differences: RepresentationDifferences::default(),
            input_equivalence: InputEquivalenceStatus::NotApplicable,
            notes: vec![],
        }
    }

    #[test]
    fn round_trip_serialization() {
        let manifest = make_minimal_manifest();
        let mut file = NamedTempFile::new().expect("temp file");
        manifest.write_to_file(file.path()).expect("write");
        let loaded = ValidationCaseManifest::load_from_file(file.path()).expect("load");
        assert_eq!(manifest, loaded);
    }

    #[test]
    fn schema_version_mismatch_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.schema_version = 999;
        let mut file = NamedTempFile::new().expect("temp file");
        file.write_all(serde_json::to_string_pretty(&manifest).unwrap().as_bytes())
            .expect("write");
        let err = ValidationCaseManifest::load_from_file(file.path()).expect_err("should fail");
        assert!(matches!(err, ValidationCaseError::SchemaVersionMismatch { .. }));
    }

    #[test]
    fn missing_surface_when_turbulence_enabled_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.surface = None;
        manifest.physics_switches.turbulence = true;
        let err = manifest.validate().expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::AmbiguousField { field: "surface", .. }
        ));
    }

    #[test]
    fn invalid_oracle_seed_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed = Some(OracleSeedIdentity {
            kind: OracleKind::SeedableValidationOracle,
            seed: Some(0), // Invalid: 0 is rejected per #50 contract
            repetitions: 5,
        });
        let err = manifest.validate().expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidStochasticIdentity { .. }
        ));
    }

    #[test]
    fn invalid_oracle_seed_too_large_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed = Some(OracleSeedIdentity {
            kind: OracleKind::SeedableValidationOracle,
            seed: Some(1_000_000_001), // Invalid: > 1e9
            repetitions: 5,
        });
        let err = manifest.validate().expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidStochasticIdentity { .. }
        ));
    }

    #[test]
    fn mismatched_wind_heights_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.domain.nz = 8;
        manifest.domain.wind_heights_m = vec![50.0, 100.0]; // Only 2 heights for 8 levels
        let err = manifest.validate().expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::AmbiguousField { field: "domain.wind_heights_m", .. }
        ));
    }

    #[test]
    fn non_increasing_wind_heights_rejected() {
        let mut manifest = make_minimal_manifest();
        // 8 heights, but not strictly increasing (500 > 200 is false)
        manifest.domain.wind_heights_m = vec![50.0, 100.0, 200.0, 500.0, 200.0, 1500.0, 3000.0, 5000.0];
        let err = manifest.validate().expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
    }

    #[test]
    fn total_s_mismatch_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.integration.total_s = 9999.0; // Wrong
        let err = manifest.validate().expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::AmbiguousField { field: "integration.total_s", .. }
        ));
    }

    #[test]
    fn analytic_case_no_stochastic_allowed() {
        let mut manifest = make_minimal_manifest();
        manifest.physics_switches.turbulence = false;
        manifest.oracle_command_overrides.lturbulence = Some(0);
        manifest.stochastic = StochasticIdentitySpec::default(); // Both None
        manifest.surface = None; // No surface needed
        // Should validate successfully
        manifest.validate().expect("analytic case should validate");
    }

    #[test]
    fn oracle_kind_parsing() {
        assert_eq!(
            "pristine-oracle".parse::<OracleKind>().unwrap(),
            OracleKind::PristineOracle
        );
        assert_eq!(
            "seedable-validation-oracle".parse::<OracleKind>().unwrap(),
            OracleKind::SeedableValidationOracle
        );
        assert!("invalid".parse::<OracleKind>().is_err());
    }

    #[test]
    fn input_equivalence_status_serialization() {
        assert_eq!(
            serde_json::to_string(&InputEquivalenceStatus::Demonstrated).unwrap(),
            "\"demonstrated\""
        );
        assert_eq!(
            serde_json::to_string(&InputEquivalenceStatus::NotDemonstrated).unwrap(),
            "\"not_demonstrated\""
        );
        assert_eq!(
            serde_json::to_string(&InputEquivalenceStatus::NotApplicable).unwrap(),
            "\"not_applicable\""
        );
    }

    #[test]
    fn load_and_validate_adv_ana_001() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("ADV-ANA-001.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load ADV-ANA-001");
        assert_eq!(manifest.case_id, "ADV-ANA-001");
        assert_eq!(manifest.schema_version, 2);
        assert!(!manifest.physics_switches.turbulence);
        assert!(manifest.stochastic.candidate_philox.is_none());
        assert!(manifest.stochastic.oracle_seed.is_none());
        assert_eq!(manifest.input_equivalence, InputEquivalenceStatus::NotApplicable);
        manifest.validate().expect("ADV-ANA-001 should validate");
    }

    #[test]
    fn load_and_validate_wind_uni_002() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("WIND-UNI-002.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load WIND-UNI-002");
        assert_eq!(manifest.case_id, "WIND-UNI-002");
        assert_eq!(manifest.schema_version, 2);
        assert!(manifest.physics_switches.turbulence);
        assert!(manifest.surface.is_some());
        assert!(manifest.stochastic.candidate_philox.is_some());
        assert!(manifest.stochastic.oracle_seed.is_some());
        let oracle = manifest.stochastic.oracle_seed.as_ref().unwrap();
        assert_eq!(oracle.kind, OracleKind::SeedableValidationOracle);
        assert_eq!(oracle.seed, Some(1));
        assert_eq!(manifest.input_equivalence, InputEquivalenceStatus::NotApplicable);
        manifest.validate().expect("WIND-UNI-002 should validate");
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
        assert!(manifest.physics_switches.dry_deposition);
        assert!(manifest.physics_switches.wet_deposition);
        assert!(manifest.surface.is_some());
        assert!(manifest.stochastic.candidate_philox.is_some());
        assert!(manifest.stochastic.oracle_seed.is_some());
        let oracle = manifest.stochastic.oracle_seed.as_ref().unwrap();
        assert_eq!(oracle.kind, OracleKind::SeedableValidationOracle);
        assert_eq!(oracle.seed, Some(1));
        assert_eq!(manifest.input_equivalence, InputEquivalenceStatus::NotDemonstrated);
        assert!(manifest.representation_differences.vertical_coordinate.is_some());
        assert!(manifest.representation_differences.wind_components.is_some());
        manifest.validate().expect("ETEX-MINI-013 should validate");
    }

    #[test]
    fn round_trip_all_cases() {
        for case_id in ["ADV-ANA-001", "WIND-UNI-002", "ETEX-MINI-013"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("corpus")
                .join("cases")
                .join(format!("{case_id}.json"));
            let manifest = ValidationCaseManifest::load_from_file(&path)
                .unwrap_or_else(|e| panic!("load {case_id}: {e}"));
            let mut temp = NamedTempFile::new().expect("temp file");
            manifest.write_to_file(temp.path()).expect("write");
            let reloaded = ValidationCaseManifest::load_from_file(temp.path()).expect("reload");
            assert_eq!(manifest, reloaded, "{case_id} round-trip failed");
        }
    }

    #[test]
    fn wind_uni_002_seed_zero_uses_declared_base_key() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("WIND-UNI-002.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load WIND-UNI-002");
        let candidate = manifest
            .stochastic
            .candidate_philox
            .as_ref()
            .expect("WIND-UNI-002 declares candidate_philox");
        assert_eq!(candidate.base_key, [3737180555, 305419896]);
        assert_eq!(candidate.base_counter, [0, 0, 0, 0]);
        let (key0, counter0) = manifest
            .candidate_seed_identity(0)
            .expect("seed 0 resolves");
        assert_eq!(key0, [3737180555, 305419896]);
        assert_eq!(counter0, [0, 0, 0, 0]);
    }

    #[test]
    fn candidate_key_derivation_wraps_u32() {
        let identity = CandidatePhiloxIdentity {
            base_key: [u32::MAX, 305419896],
            base_counter: [0, 0, 0, 0],
            count: 10,
            derivation: "seed i uses key [base0 + i, base1] with zeroed counter".to_string(),
            identical_repeats: false,
        };
        assert_eq!(identity.key_for_seed_index(0), [u32::MAX, 305419896]);
        assert_eq!(identity.key_for_seed_index(1), [0, 305419896]);
        assert_eq!(identity.key_for_seed_index(2), [1, 305419896]);
    }

    #[test]
    fn stochastic_case_without_candidate_philox_is_rejected() {
        let mut manifest = make_minimal_manifest();
        assert!(manifest.physics_switches.turbulence);
        manifest.stochastic.candidate_philox = None;
        manifest.stochastic.oracle_seed = None;
        // Manifest-level validation rejects turbulence without any identity.
        assert!(manifest.validate().is_err());
        // Seed resolution also fails closed instead of substituting a key.
        assert!(manifest.candidate_seed_identity(0).is_err());
    }

    #[test]
    fn adv_ana_001_needs_no_rng_identity() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("ADV-ANA-001.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load ADV-ANA-001");
        assert!(!manifest.physics_switches.turbulence);
        assert!(manifest.stochastic.candidate_philox.is_none());
        manifest.validate().expect("ADV-ANA-001 stays valid");
        assert!(manifest.candidate_seed_identity(0).is_err());
    }

    #[test]
    fn adv_ana_001_oracle_overrides_disable_turbulence_and_convection() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("ADV-ANA-001.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load ADV-ANA-001");
        assert_eq!(manifest.oracle_command_overrides.lturbulence, Some(0));
        assert_eq!(manifest.oracle_command_overrides.lconvection, Some(0));
        manifest.validate().expect("ADV-ANA-001 overrides stay valid");
    }

    #[test]
    fn wind_uni_002_oracle_overrides_match_manifest() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("WIND-UNI-002.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load WIND-UNI-002");
        assert_eq!(manifest.oracle_command_overrides.lturbulence, Some(1));
        assert_eq!(manifest.oracle_command_overrides.lconvection, Some(0));
        assert_eq!(manifest.oracle_command_overrides.ctl, Some(5.0));
        assert_eq!(manifest.oracle_command_overrides.ifine, Some(4));
        manifest.validate().expect("WIND-UNI-002 overrides stay valid");
    }

    #[test]
    fn missing_oracle_override_is_rejected_without_default() {
        let mut manifest = make_minimal_manifest();
        manifest.oracle_command_overrides.lturbulence = None;
        let err = manifest.validate().expect_err("missing lturbulence must fail");
        assert!(matches!(
            err,
            ValidationCaseError::MissingField { field: "oracle_command_overrides.lturbulence" }
        ));
    }

    #[test]
    fn oracle_flag_other_than_zero_or_one_is_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.oracle_command_overrides.lturbulence = Some(2);
        let err = manifest.validate().expect_err("flag 2 must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
    }

    #[test]
    fn oracle_switch_conflicting_with_physics_is_rejected() {
        let mut manifest = make_minimal_manifest();
        assert!(manifest.physics_switches.turbulence);
        manifest.oracle_command_overrides.lturbulence = Some(0);
        let err = manifest.validate().expect_err("conflict must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
    }

    const ALL_CHECKED_IN_CASES: &[&str] = &[
        "ADV-ANA-001",
        "WIND-UNI-002",
        "WIND-SHEAR-003",
        "PBL-STABLE-004",
        "PBL-NEUTRAL-005",
        "PBL-UNSTABLE-006",
        "DRY-007",
        "WET-008",
        "REPEAT-009",
        "ETEX-MINI-013",
    ];

    fn load_checked_in_case(case_id: &str) -> ValidationCaseManifest {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join(format!("{case_id}.json"));
        ValidationCaseManifest::load_from_file(&path)
            .unwrap_or_else(|e| panic!("load {case_id}: {e}"))
    }

    #[test]
    fn every_checked_in_case_is_canonical_v2() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases");
        let mut found: Vec<String> = std::fs::read_dir(&dir)
            .expect("cases dir readable")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json"))
            .map(|name| name.trim_end_matches(".json").to_string())
            .collect();
        found.sort();
        let mut expected: Vec<String> =
            ALL_CHECKED_IN_CASES.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(found, expected, "no checked-in case outside the v2 contract");
        for case_id in ALL_CHECKED_IN_CASES {
            let manifest = load_checked_in_case(case_id);
            assert_eq!(manifest.schema_version, VALIDATION_CASE_SCHEMA_VERSION);
            assert_eq!(manifest.case_id, *case_id);
            manifest.validate().expect("checked-in case validates");
        }
    }

    #[test]
    fn migrated_cases_keep_their_scientific_values() {
        let shear = load_checked_in_case("WIND-SHEAR-003");
        match &shear.wind {
            WindSpec::LinearShear { u0_m_s, u_shear_per_s, v_m_s, w_m_s } => {
                assert_eq!(*u0_m_s, 2.0);
                assert_eq!(*u_shear_per_s, 0.004);
                assert_eq!(*v_m_s, 0.0);
                assert_eq!(*w_m_s, 0.0);
            }
            other => panic!("shear wind changed: {other:?}"),
        }
        assert_eq!(shear.release.particle_count, 1000);

        let stable = load_checked_in_case("PBL-STABLE-004");
        let surface = stable.surface.as_ref().expect("stable surface");
        assert_eq!(surface.sensible_heat_flux_w_m2, -20.0);
        assert_eq!(surface.inv_obukhov_length_per_m, 0.02);
        assert_eq!(surface.mixing_height_m, 500.0);

        let neutral = load_checked_in_case("PBL-NEUTRAL-005");
        assert_eq!(neutral.release.particle_count, 500);
        let surface = neutral.surface.as_ref().expect("neutral surface");
        assert_eq!(surface.mixing_height_m, 1500.0);
        assert_eq!(surface.sensible_heat_flux_w_m2, 0.0);

        let unstable = load_checked_in_case("PBL-UNSTABLE-006");
        let surface = unstable.surface.as_ref().expect("unstable surface");
        assert_eq!(surface.sensible_heat_flux_w_m2, 150.0);
        assert_eq!(surface.convective_velocity_scale_m_s, 1.5);
        assert_eq!(surface.mixing_height_m, 2000.0);
        assert_eq!(surface.inv_obukhov_length_per_m, -0.02);

        let dry = load_checked_in_case("DRY-007");
        assert!(dry.physics_switches.dry_deposition);
        assert!(!dry.physics_switches.wet_deposition);
        let deposition = dry.deposition.as_ref().expect("dry deposition");
        assert_eq!(deposition.dry_deposition_velocity_m_s, 0.02);
        assert_eq!(deposition.dry_reference_height_m, Some(15.0));
        assert_eq!(deposition.wet_scavenging_coefficient_s_inv, 0.0);
        assert_eq!(deposition.wet_precipitating_fraction, 0.0);

        let wet = load_checked_in_case("WET-008");
        assert!(wet.physics_switches.wet_deposition);
        assert!(!wet.physics_switches.dry_deposition);
        let deposition = wet.deposition.as_ref().expect("wet deposition");
        assert_eq!(deposition.dry_deposition_velocity_m_s, 0.0);
        assert_eq!(deposition.wet_scavenging_coefficient_s_inv, 0.005);
        assert_eq!(deposition.wet_precipitating_fraction, 1.0);
        let surface = wet.surface.as_ref().expect("wet surface");
        assert_eq!(surface.precip_large_scale_mm_h, 2.0);
        assert_eq!(surface.precip_convective_mm_h, 1.0);

        let repeat = load_checked_in_case("REPEAT-009");
        assert_eq!(repeat.release.particle_count, 500);
        let candidate = repeat
            .stochastic
            .candidate_philox
            .as_ref()
            .expect("repeat identity");
        assert!(candidate.identical_repeats);
        assert_eq!(candidate.count, 2);
        assert_eq!(
            repeat.candidate_seed_identity(0).expect("repeat seed 0"),
            repeat.candidate_seed_identity(1).expect("repeat seed 1")
        );
        assert!(repeat.stochastic.oracle_seed.is_none());
    }

    #[test]
    fn normalized_round_trip_all_checked_in_cases() {
        for case_id in ALL_CHECKED_IN_CASES {
            let manifest = load_checked_in_case(case_id);
            let json = serde_json::to_string_pretty(&manifest).expect("serialize");
            let reparsed = ValidationCaseManifest::parse(
                &json,
                Path::new(&format!("{case_id}.json")),
            )
            .unwrap_or_else(|e| panic!("reparse {case_id}: {e}"));
            assert_eq!(manifest, reparsed, "{case_id} round-trip failed");
        }
    }

    #[test]
    fn legacy_v1_document_is_rejected_with_version_error() {
        let doc = r#"{"version": 1, "case_id": "WIND-UNI-002"}"#;
        let err =
            ValidationCaseManifest::parse(doc, Path::new("legacy.json")).expect_err("v1 fails");
        assert!(
            matches!(err, ValidationCaseError::SchemaVersionMismatch { expected: 2, actual: 1 }),
            "unexpected: {err}"
        );
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.schema_version = 999;
        let json = serde_json::to_string(&manifest).expect("serialize");
        let err =
            ValidationCaseManifest::parse(&json, Path::new("future.json")).expect_err("fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::SchemaVersionMismatch { expected: 2, actual: 999 }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn mixed_version_document_is_rejected_as_ambiguous() {
        let doc = r#"{"schema_version": 2, "version": 1, "case_id": "X"}"#;
        let err =
            ValidationCaseManifest::parse(doc, Path::new("mixed.json")).expect_err("fails");
        assert!(
            matches!(err, ValidationCaseError::AmbiguousField { .. }),
            "unexpected: {err}"
        );
    }

    #[test]
    fn missing_version_is_rejected() {
        let doc = r#"{"case_id": "X"}"#;
        let err =
            ValidationCaseManifest::parse(doc, Path::new("noversion.json")).expect_err("fails");
        assert!(
            matches!(err, ValidationCaseError::MissingField { field: "schema_version" }),
            "unexpected: {err}"
        );
    }

    #[test]
    fn deposition_block_forbidden_when_switches_off() {
        let mut manifest = make_minimal_manifest();
        assert!(!manifest.physics_switches.dry_deposition);
        assert!(!manifest.physics_switches.wet_deposition);
        manifest.deposition = Some(DepositionSpec {
            dry_deposition_velocity_m_s: 0.0,
            dry_reference_height_m: None,
            wet_scavenging_coefficient_s_inv: 0.0,
            wet_precipitating_fraction: 0.0,
        });
        let err = manifest.validate().expect_err("stray block fails");
        assert!(
            matches!(err, ValidationCaseError::InvalidPhysicsSwitches { .. }),
            "unexpected: {err}"
        );
    }

    #[test]
    fn dry_deposition_requires_positive_velocity_and_height() {
        let mut manifest = make_minimal_manifest();
        manifest.physics_switches.dry_deposition = true;
        manifest.deposition = Some(DepositionSpec {
            dry_deposition_velocity_m_s: 0.0,
            dry_reference_height_m: Some(15.0),
            wet_scavenging_coefficient_s_inv: 0.0,
            wet_precipitating_fraction: 0.0,
        });
        assert!(manifest.validate().is_err());
        manifest.deposition = Some(DepositionSpec {
            dry_deposition_velocity_m_s: 0.02,
            dry_reference_height_m: None,
            wet_scavenging_coefficient_s_inv: 0.0,
            wet_precipitating_fraction: 0.0,
        });
        let err = manifest.validate().expect_err("missing href fails");
        assert!(
            matches!(err, ValidationCaseError::MissingField { .. }),
            "unexpected: {err}"
        );
    }
}