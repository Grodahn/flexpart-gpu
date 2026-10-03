//! Root case model and ordered domain, integration, unit and physics validation.

use super::{
    CandidatePhysicsProfileRef, ExecutionProfileRef, ExpectedArtifacts, OracleCommandOverrides,
    OracleExecutionPolicy, OracleMeteorologyProfileRef, OutputGridSpec, OutputSpec, ReleaseSpec,
    RepresentationDifferences, SimulationDirection, StochasticIdentitySpec, SurfaceSpec,
    ValidationDefinitionRefs, WindSpec, CANDIDATE_PHYSICS_PROFILE_ID,
    CANDIDATE_PHYSICS_PROFILE_PATH, CANDIDATE_PHYSICS_PROFILE_VERSION, ORACLE_EXECUTION_PROFILE_ID,
    ORACLE_EXECUTION_PROFILE_PATH, ORACLE_EXECUTION_PROFILE_VERSION,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Schema version for the validation case manifest.
pub const VALIDATION_CASE_SCHEMA_VERSION: u32 = 2;

/// Checked-in machine-readable structural contract for schema v2.
pub const VALIDATION_CASE_SCHEMA_PATH: &str = "schemas/validation-case-v2.schema.json";

/// Driver deposition forcing carried by the case manifest.
///
/// Mirrors the legacy v1 `deposition` block key-for-key so migrated cases
/// keep byte-identical scientific values. `None` when both deposition
/// switches are off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Domain specification with explicit units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainSpec {
    /// Number of grid cells in x.
    pub nx: u32,
    /// Number of grid cells in y.
    pub ny: u32,
    /// Number of vertical levels.
    pub nz: u32,
    /// Grid spacing in x [degrees, must be finite and strictly positive].
    pub dx_deg: f32,
    /// Grid spacing in y [degrees, must be finite and strictly positive].
    pub dy_deg: f32,
    /// Origin longitude [degrees, `horizontal_ref` convention].
    pub xlon0_deg: f32,
    /// Origin latitude [degrees, `horizontal_ref` convention].
    pub ylat0_deg: f32,
    /// Horizontal coordinate convention for origin, spacing, and release
    /// geometry. Required: a domain without convention metadata is rejected.
    pub horizontal_ref: HorizontalCoordRef,
    /// Vertical level heights [m, `wind_heights_ref` reference].
    pub wind_heights_m: Vec<f32>,
    /// Vertical reference for `wind_heights_m` (all checked-in cases AGL).
    pub wind_heights_ref: VerticalRef,
}

/// Horizontal coordinate convention for domain and release geometry.
///
/// All checked-in cases use geographic coordinates. The convention is part of
/// the contract so runners never assume a projection or axis order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HorizontalCoordRef {
    /// Geographic longitude (east-positive, decimal degrees) and latitude
    /// (decimal degrees) on the WGS84 ellipsoid.
    GeographicLonLatDegrees,
}

/// Vertical height reference.
///
/// Distinguishes above-ground-level from above-sea-level heights. Every
/// checked-in case uses AGL: candidate particle heights, PBL diagnostics,
/// and `wind_heights_m` are heights above ground, and FLEXPART `RELEASES`
/// uses `ZKIND=1` (meters above ground; see the source-contract mapping in
/// `fixtures/corpus/cases/MIGRATION_NOTES.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerticalRef {
    /// Height above ground level [m].
    Agl,
    /// Height above sea level [m].
    Asl,
}

/// Integration timestep and window specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct UnitsSpec {
    /// Wind velocity unit (typically "m/s").
    pub wind: String,
    /// Displacement/length unit for analytic expectations (typically "m").
    /// Carries the ADV-ANA-001 `displacement` semantics as a typed field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub displacement: Option<String>,
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

/// Complete validation case manifest.
///
/// Closed contract: unknown fields are rejected at every level so typos and
/// stray extension keys fail instead of being silently discarded. Deliberate
/// extensions belong in `notes` or a versioned schema revision, never in
/// ad-hoc top-level keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Simulation direction (required, never defaulted).
    pub simulation_direction: SimulationDirection,
    /// Output timing and scientific semantics (required, never defaulted).
    pub output: OutputSpec,
    /// Explicit concentration/comparison output grid. Required for every
    /// validation case. The optional Rust representation is retained only so
    /// missing documents receive a field-specific fail-closed validation error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_grid: Option<OutputGridSpec>,
    /// Driver deposition forcing. `None` when deposition is off.
    /// Migrated key-for-key from the legacy v1 `deposition` block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposition: Option<DepositionSpec>,
    /// Explicit units for all quantities.
    pub units: UnitsSpec,
    /// External metric/threshold definitions. Numeric gates/formulas remain
    /// owned outside #51 and are referenced rather than duplicated.
    pub validation_definition_refs: ValidationDefinitionRefs,
    /// Stochastic identity specification (candidate + oracle).
    pub stochastic: StochasticIdentitySpec,
    /// Execution profile reference (frozen #49).
    pub execution_profile: ExecutionProfileRef,
    /// Explicitly declares whether this case executes an oracle run.
    pub oracle_execution: OracleExecutionPolicy,
    /// Stable identity/source of the oracle meteorology representation.
    pub oracle_meteorology_profile: OracleMeteorologyProfileRef,
    /// Candidate-side versioned physics/runtime profile. Required so shared
    /// PBL/integration settings never come from executable defaults.
    pub candidate_physics_profile: CandidatePhysicsProfileRef,
    /// Oracle COMMAND namelist overrides. Required; no default block exists.
    pub oracle_command_overrides: OracleCommandOverrides,
    /// Expected output artifacts.
    pub expected_artifacts: ExpectedArtifacts,
    /// Structured representation differences and known limitations for #52.
    /// Required explicitly; this field never contains an input-equivalence verdict.
    pub representation_differences: RepresentationDifferences,
    /// Whether the release geometry must lie inside the domain.
    /// Required explicitly: synthetic corpus cases use true; ETEX-MINI-013
    /// explicitly waives containment pending #52 input-equivalence work.
    pub require_source_containment: bool,
    /// Additional notes. Required explicitly by schema v2; use an empty array
    /// when no notes apply so omission cannot carry hidden compatibility semantics.
    pub notes: Vec<String>,
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
    #[error("failed to write case file `{path}`: {source}")]
    WriteFile {
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
    #[error("invalid candidate physics profile reference: {message}")]
    InvalidCandidatePhysicsProfile { message: String },
    #[error("ambiguous field: {field} - {message}")]
    AmbiguousField {
        field: &'static str,
        message: String,
    },
    #[error("unit mismatch: {field} expected {expected}, got {actual}")]
    UnitMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
}

impl ValidationCaseManifest {
    /// Validate the manifest (fail-closed).
    ///
    /// # Errors
    /// Returns a [`ValidationCaseError`] variant describing the first validation failure.
    pub fn validate(&self) -> Result<(), ValidationCaseError> {
        // Validate case_id format
        if self.case_id.is_empty() {
            return Err(ValidationCaseError::MissingField { field: "case_id" });
        }

        // Validate domain
        if self.domain.nx < 2 || self.domain.ny < 2 || self.domain.nz == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "domain nx/ny must be >= 2 and nz must be > 0".to_string(),
            });
        }
        for (name, spacing) in [
            ("domain.dx_deg", self.domain.dx_deg),
            ("domain.dy_deg", self.domain.dy_deg),
        ] {
            if !spacing.is_finite() || spacing <= 0.0 {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{name} must be finite and strictly positive, got {spacing}"),
                });
            }
        }
        for (name, value) in [
            ("domain.xlon0_deg", self.domain.xlon0_deg),
            ("domain.ylat0_deg", self.domain.ylat0_deg),
        ] {
            if !value.is_finite() {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{name} must be finite, got {value}"),
                });
            }
        }
        if !(-180.0..=360.0).contains(&self.domain.xlon0_deg) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "domain.xlon0_deg must be in [-180, 360], got {}",
                    self.domain.xlon0_deg
                ),
            });
        }
        if !(-90.0..=90.0).contains(&self.domain.ylat0_deg) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "domain.ylat0_deg must be in [-90, 90], got {}",
                    self.domain.ylat0_deg
                ),
            });
        }
        if self.domain.wind_heights_ref != VerticalRef::Agl {
            return Err(ValidationCaseError::AmbiguousField {
                field: "domain.wind_heights_ref",
                message: "schema v2 currently supports only AGL domain wind heights; ASL conversion semantics are not implemented".to_string(),
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
        if self
            .domain
            .wind_heights_m
            .iter()
            .any(|h| !h.is_finite() || *h < 0.0)
        {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "wind_heights_m must be finite and >= 0".to_string(),
            });
        }
        if self.domain.wind_heights_m.windows(2).any(|w| w[1] <= w[0]) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "wind_heights_m must be strictly increasing".to_string(),
            });
        }

        // Validate integration before comparing release/met chronology.
        Self::validate_timestamp(&self.integration.start, "integration.start")?;
        if !self.integration.dt_s.is_finite() || self.integration.dt_s <= 0.0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "integration.dt_s must be finite and > 0, got {}",
                    self.integration.dt_s
                ),
            });
        }
        if self.integration.dt_s.fract() != 0.0 {
            return Err(ValidationCaseError::AmbiguousField {
                field: "integration.dt_s",
                message: format!(
                    "dt_s ({}) must be whole seconds because ForwardTimeLoopConfig uses integer-second timesteps",
                    self.integration.dt_s
                ),
            });
        }
        if self.integration.steps == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "integration.steps must be > 0".to_string(),
            });
        }
        if !self.integration.total_s.is_finite() || self.integration.total_s <= 0.0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "integration.total_s must be finite and > 0, got {}",
                    self.integration.total_s
                ),
            });
        }
        if self.integration.total_s.fract() != 0.0 {
            return Err(ValidationCaseError::AmbiguousField {
                field: "integration.total_s",
                message: format!(
                    "total_s ({}) must be whole seconds because manifest/FLEXPART timestamps are second-resolution",
                    self.integration.total_s
                ),
            });
        }
        let expected_total = self.integration.dt_s * self.integration.steps as f32;
        if !expected_total.is_finite() || (self.integration.total_s - expected_total).abs() > 1e-6 {
            return Err(ValidationCaseError::AmbiguousField {
                field: "integration.total_s",
                message: format!(
                    "total_s ({}) must equal dt_s * steps ({})",
                    self.integration.total_s, expected_total
                ),
            });
        }

        // Validate normalized release and its chronology against the simulation.
        self.validate_release()?;
        self.validate_species_physics_contract()?;
        self.validate_chronology()?;

        // Validate physics switches consistency with other fields
        self.validate_physics_consistency()?;

        // Validate output timing and the explicit comparison grid.
        self.validate_output()?;
        self.validate_output_grid()?;

        // Validate stochastic identity
        self.validate_stochastic()?;

        // Validate Oracle command overrides (required, no hidden defaults)
        self.validate_oracle_overrides()?;

        // Validate deposition forcing against deposition switches
        self.validate_deposition()?;

        // Validate the frozen #49 execution profile reference exactly.
        if self.execution_profile.id != ORACLE_EXECUTION_PROFILE_ID
            || self.execution_profile.version != ORACLE_EXECUTION_PROFILE_VERSION
            || self.execution_profile.manifest_path != ORACLE_EXECUTION_PROFILE_PATH
        {
            return Err(ValidationCaseError::InvalidExecutionProfile {
                message: format!(
                    "execution_profile must reference {} v{} at {}, got {} v{} at {}",
                    ORACLE_EXECUTION_PROFILE_ID,
                    ORACLE_EXECUTION_PROFILE_VERSION,
                    ORACLE_EXECUTION_PROFILE_PATH,
                    self.execution_profile.id,
                    self.execution_profile.version,
                    self.execution_profile.manifest_path
                ),
            });
        }

        self.validate_oracle_meteorology_profile()?;

        if self.candidate_physics_profile.id != CANDIDATE_PHYSICS_PROFILE_ID
            || self.candidate_physics_profile.version != CANDIDATE_PHYSICS_PROFILE_VERSION
            || self.candidate_physics_profile.manifest_path != CANDIDATE_PHYSICS_PROFILE_PATH
        {
            return Err(ValidationCaseError::InvalidCandidatePhysicsProfile {
                message: format!(
                    "candidate_physics_profile must reference {} v{} at {}, got {} v{} at {}",
                    CANDIDATE_PHYSICS_PROFILE_ID,
                    CANDIDATE_PHYSICS_PROFILE_VERSION,
                    CANDIDATE_PHYSICS_PROFILE_PATH,
                    self.candidate_physics_profile.id,
                    self.candidate_physics_profile.version,
                    self.candidate_physics_profile.manifest_path
                ),
            });
        }

        self.validate_units()?;
        self.validate_validation_definition_refs()?;
        self.validate_representation_differences()?;
        self.validate_expected_artifacts()?;

        Ok(())
    }

    fn validate_units(&self) -> Result<(), ValidationCaseError> {
        let require = |field: &'static str,
                       actual: Option<&str>,
                       expected: &'static str|
         -> Result<(), ValidationCaseError> {
            let Some(actual) = actual else {
                return Err(ValidationCaseError::MissingField { field });
            };
            if actual != expected {
                return Err(ValidationCaseError::UnitMismatch {
                    field,
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                });
            }
            Ok(())
        };

        if self.units.wind != "m/s" {
            return Err(ValidationCaseError::UnitMismatch {
                field: "units.wind",
                expected: "m/s".to_string(),
                actual: self.units.wind.clone(),
            });
        }
        require("units.height", self.units.height.as_deref(), "m")?;
        require("units.mass", self.units.mass.as_deref(), "kg")?;
        require("units.time", self.units.time.as_deref(), "s")?;
        require(
            "units.concentration",
            self.units.concentration.as_deref(),
            "kg/m3",
        )?;

        if self.surface.is_some() {
            require("units.pressure", self.units.pressure.as_deref(), "Pa")?;
            require("units.temperature", self.units.temperature.as_deref(), "K")?;
            require("units.heat_flux", self.units.heat_flux.as_deref(), "W/m2")?;
            require(
                "units.inv_obukhov",
                self.units.inv_obukhov.as_deref(),
                "1/m",
            )?;
        }
        if matches!(self.wind, WindSpec::LinearShear { .. }) {
            require("units.shear", self.units.shear.as_deref(), "1/s")?;
        }
        if self.physics_switches.dry_deposition {
            require(
                "units.deposition_velocity",
                self.units.deposition_velocity.as_deref(),
                "m/s",
            )?;
        }
        if self.physics_switches.wet_deposition {
            require(
                "units.scavenging_coefficient",
                self.units.scavenging_coefficient.as_deref(),
                "1/s",
            )?;
        }
        if let Some(unit) = self.units.displacement.as_deref() {
            if unit != "m" {
                return Err(ValidationCaseError::UnitMismatch {
                    field: "units.displacement",
                    expected: "m".to_string(),
                    actual: unit.to_string(),
                });
            }
        }
        Ok(())
    }

    fn validate_physics_consistency(&self) -> Result<(), ValidationCaseError> {
        // Analytic/synthetic turbulence cases require an explicit static surface
        // block. Real-weather cases source time-varying surface fields from the
        // meteorology contract instead and must not invent representative values.
        let real_weather = matches!(self.wind, WindSpec::RealWeather { .. });
        if self.physics_switches.turbulence && self.surface.is_none() && !real_weather {
            return Err(ValidationCaseError::AmbiguousField {
                field: "surface",
                message: "surface fields required when physics_switches.turbulence=true unless wind.profile=real_weather supplies them through meteorology".to_string(),
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
            | WindSpec::RealWeather { .. } => {}
        }

        // Deposition switches require surface fields with relevant parameters
        if (self.physics_switches.dry_deposition || self.physics_switches.wet_deposition)
            && self.surface.is_none()
            && !real_weather
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

        // Validate meteorology specification for real-weather cases
        if let WindSpec::RealWeather { meteorology } = &self.wind {
            self.validate_meteorology(meteorology)?;
        }

        Ok(())
    }

    fn validate_deposition(&self) -> Result<(), ValidationCaseError> {
        let dry = self.physics_switches.dry_deposition;
        let wet = self.physics_switches.wet_deposition;
        let Some(spec) = &self.deposition else {
            if dry || wet {
                return Err(ValidationCaseError::MissingField {
                    field: "deposition",
                });
            }
            return Ok(());
        };
        if !dry && !wet {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "deposition block must be absent when deposition switches are off"
                    .to_string(),
            });
        }
        if !spec.dry_deposition_velocity_m_s.is_finite() || spec.dry_deposition_velocity_m_s < 0.0 {
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
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        load_validation_case_schema, make_minimal_manifest, make_valid_dry_deposition_manifest,
        minimal_manifest_json, parse_json_value, validate_json_schema_subset,
    };

    use super::super::*;
    use std::path::Path;

    #[test]
    fn missing_surface_when_turbulence_enabled_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.surface = None;
        manifest.physics_switches.turbulence = true;
        let err = manifest.validate().expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::AmbiguousField {
                field: "surface",
                ..
            }
        ));
    }

    #[test]
    fn fractional_candidate_timestep_is_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.integration.dt_s = 12.5;
        manifest.integration.steps = 288;
        manifest.integration.total_s = 3600.0;
        let err = manifest.validate().expect_err("fractional dt must fail");
        assert!(err.to_string().contains("whole seconds"));
    }

    #[test]
    fn unit_mismatch_uses_dedicated_error_variant() {
        let mut manifest = make_minimal_manifest();
        manifest.units.wind = "km/h".to_string();
        let err = manifest.validate().expect_err("wrong wind unit must fail");
        assert!(matches!(
            err,
            ValidationCaseError::UnitMismatch {
                field: "units.wind",
                ..
            }
        ));
    }

    #[test]
    fn context_required_units_fail_closed() {
        let mut manifest = make_minimal_manifest();
        manifest.units.mass = None;
        assert!(manifest
            .validate()
            .expect_err("missing mass unit")
            .to_string()
            .contains("units.mass"));

        let mut manifest = make_minimal_manifest();
        manifest.units.concentration = Some("pg/m3".to_string());
        assert!(manifest
            .validate()
            .expect_err("wrong concentration unit")
            .to_string()
            .contains("units.concentration"));

        let mut manifest = make_minimal_manifest();
        manifest.units.pressure = None;
        assert!(manifest
            .validate()
            .expect_err("missing pressure unit")
            .to_string()
            .contains("units.pressure"));
    }

    #[test]
    fn mismatched_wind_heights_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.domain.nz = 8;
        manifest.domain.wind_heights_m = vec![50.0, 100.0]; // Only 2 heights for 8 levels
        let err = manifest.validate().expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::AmbiguousField {
                field: "domain.wind_heights_m",
                ..
            }
        ));
    }

    #[test]
    fn non_increasing_wind_heights_rejected() {
        let mut manifest = make_minimal_manifest();
        // 8 heights, but not strictly increasing (500 > 200 is false)
        manifest.domain.wind_heights_m =
            vec![50.0, 100.0, 200.0, 500.0, 200.0, 1500.0, 3000.0, 5000.0];
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
            ValidationCaseError::AmbiguousField {
                field: "integration.total_s",
                ..
            }
        ));
    }

    #[test]
    fn domain_requires_two_horizontal_grid_points() {
        let mut manifest = make_minimal_manifest();
        manifest.domain.nx = 1;
        let err = manifest.validate().expect_err("nx=1 must fail");
        assert!(
            err.to_string().contains("nx/ny must be >= 2"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn adv_ana_001_retains_displacement_unit_semantics() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("ADV-ANA-001.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load ADV-ANA-001");
        assert_eq!(manifest.units.displacement.as_deref(), Some("m"));
        let json = serde_json::to_string_pretty(&manifest).expect("serialize");
        assert!(
            json.contains("\"displacement\""),
            "serialized manifest must retain the displacement unit"
        );
        let reparsed =
            ValidationCaseManifest::parse(&json, Path::new("adv.json")).expect("reparse");
        assert_eq!(reparsed.units.displacement.as_deref(), Some("m"));
    }

    #[test]
    fn integration_total_seconds_are_whole_in_schema_and_rust() {
        let schema = load_validation_case_schema();
        let mut raw = minimal_manifest_json();
        raw["integration"]["dt_s"] = serde_json::json!(0.5);
        raw["integration"]["steps"] = serde_json::json!(1);
        raw["integration"]["total_s"] = serde_json::json!(0.5);
        assert!(
            validate_json_schema_subset(&schema, &schema, &raw, "$").is_err(),
            "JSON Schema must reject fractional total_s"
        );
        let err = parse_json_value(&raw).expect_err("Rust must reject fractional total_s");
        assert!(
            err.to_string().contains("whole seconds"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn domain_without_convention_metadata_is_rejected() {
        let mut raw = minimal_manifest_json();
        raw["domain"]
            .as_object_mut()
            .expect("domain object")
            .remove("horizontal_ref");
        let err = parse_json_value(&raw).expect_err("missing horizontal_ref fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("horizontal_ref"),
            "error must name the missing convention: {rendered}"
        );
        let mut raw = minimal_manifest_json();
        raw["domain"]
            .as_object_mut()
            .expect("domain object")
            .remove("wind_heights_ref");
        let err = parse_json_value(&raw).expect_err("missing wind_heights_ref fails");
        assert!(
            err.to_string().contains("wind_heights_ref"),
            "error must name the missing reference: {err}"
        );
    }

    #[test]
    fn active_deposition_requires_explicit_forcing_block() {
        let mut manifest = make_valid_dry_deposition_manifest();
        manifest.deposition = None;
        let err = manifest
            .validate()
            .expect_err("active dry deposition needs forcing");
        assert!(matches!(
            err,
            ValidationCaseError::MissingField {
                field: "deposition"
            }
        ));
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
        let mut manifest = make_valid_dry_deposition_manifest();
        manifest
            .deposition
            .as_mut()
            .expect("deposition")
            .dry_deposition_velocity_m_s = 0.0;
        let err = manifest
            .validate()
            .expect_err("zero dry-deposition velocity fails");
        assert!(
            matches!(err, ValidationCaseError::InvalidPhysicsSwitches { .. })
                && err.to_string().contains("dry_deposition_velocity_m_s"),
            "unexpected: {err}"
        );

        let mut manifest = make_valid_dry_deposition_manifest();
        manifest
            .deposition
            .as_mut()
            .expect("deposition")
            .dry_reference_height_m = None;
        let err = manifest.validate().expect_err("missing href fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::MissingField {
                    field: "deposition.dry_reference_height_m"
                }
            ),
            "unexpected: {err}"
        );
    }
}
