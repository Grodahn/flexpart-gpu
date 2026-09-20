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

use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Schema version for the validation case manifest.
pub const VALIDATION_CASE_SCHEMA_VERSION: u32 = 2;

/// Checked-in machine-readable structural contract for schema v2.
pub const VALIDATION_CASE_SCHEMA_PATH: &str = "schemas/validation-case-v2.schema.json";

/// Stable identity of the completed #50 oracle stochastic-identity contract.
/// Case manifests reference this contract and never duplicate its
/// requested-identity -> FLEXPART RNG-state mapping.
pub const ORACLE_STOCHASTIC_STRATEGY_ID: &str = "flexpart-oracle-validation-seed-offset";
pub const ORACLE_STOCHASTIC_STRATEGY_VERSION: u32 = 1;
pub const ORACLE_STOCHASTIC_CONTRACT_PATH: &str = "reference/oracle-stochastic-identity.json";

pub const SPECIES_024_INERT_CONTRACT_ID: &str = "species-024-inert-v1";
pub const SPECIES_024_INERT_CONTRACT_PATH: &str =
    "reference/species-physics/species-024-inert-v1.json";
pub const SPECIES_024_INERT_CONTRACT_BLOB: &str =
    "21dd5ccd2b616642be9e3989e9b7444774d97fd9";
pub const SPECIES_040_DRY_CONTRACT_ID: &str = "species-040-dry-constant-v1";
pub const SPECIES_040_DRY_CONTRACT_PATH: &str =
    "reference/species-physics/species-040-dry-constant-v1.json";
pub const SPECIES_040_DRY_CONTRACT_BLOB: &str =
    "c050d6351244beaf2b1a57f661f2321101bda6f1";
pub const SPECIES_040_WET_CONTRACT_ID: &str = "species-040-wet-aerosol-v1";
pub const SPECIES_040_WET_CONTRACT_PATH: &str =
    "reference/species-physics/species-040-wet-aerosol-v1.json";
pub const SPECIES_040_WET_CONTRACT_BLOB: &str =
    "4f55d23294f320f0050581cfad14800b22bf141d";

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
#[serde(deny_unknown_fields)]
pub struct StochasticIdentitySpec {
    /// Candidate RNG namespace: Philox key/counter for the GPU candidate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_philox: Option<CandidatePhiloxIdentity>,
    /// Oracle RNG namespace: validation seed identity for FLEXPART oracle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oracle_seed: Option<OracleSeedIdentity>,
}

/// Versioned candidate-side Philox identity derivation.
///
/// This enum is executable contract data, not documentation. Adding a new
/// derivation requires a new explicit variant and corresponding runner/audit
/// semantics; unknown strings fail deserialization closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidatePhiloxDerivation {
    /// Seed i uses [base_key[0] + i (wrapping u32), base_key[1]] and the
    /// declared base counter.
    #[serde(rename = "wrapping_add_key0_v1")]
    WrappingAddKey0V1,
    /// Every ensemble member reuses the exact declared base key and counter.
    /// Used by REPEAT-009 to prove bit-identical reruns.
    #[serde(rename = "reuse_base_identity_v1")]
    ReuseBaseIdentityV1,
}

/// Candidate-side Philox identity (separate RNG namespace from oracle).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePhiloxIdentity {
    /// Base Philox key [key0, key1] for seed derivation.
    pub base_key: [u32; 2],
    /// Base Philox counter for the first timestep.
    pub base_counter: [u32; 4],
    /// Number of ensemble identities/repetitions.
    pub count: u32,
    /// Versioned executable derivation policy.
    pub derivation: CandidatePhiloxDerivation,
}

impl CandidatePhiloxIdentity {
    /// Derive the Philox key for seed index `i` from the declared policy.
    #[must_use]
    pub fn key_for_seed_index(&self, seed_index: u32) -> [u32; 2] {
        match self.derivation {
            CandidatePhiloxDerivation::WrappingAddKey0V1 => [
                self.base_key[0].wrapping_add(seed_index),
                self.base_key[1],
            ],
            CandidatePhiloxDerivation::ReuseBaseIdentityV1 => self.base_key,
        }
    }

    /// Derive the Philox counter for seed index `i` from the declared policy.
    #[must_use]
    pub fn counter_for_seed_index(&self, _seed_index: u32) -> [u32; 4] {
        match self.derivation {
            CandidatePhiloxDerivation::WrappingAddKey0V1
            | CandidatePhiloxDerivation::ReuseBaseIdentityV1 => self.base_counter,
        }
    }
}

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

/// Stable reference to the completed #50 seedable-oracle strategy contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleStrategyRef {
    pub strategy: String,
    pub version: u32,
    pub contract_path: String,
}

impl OracleStrategyRef {
    #[must_use]
    pub fn canonical() -> Self {
        Self {
            strategy: ORACLE_STOCHASTIC_STRATEGY_ID.to_string(),
            version: ORACLE_STOCHASTIC_STRATEGY_VERSION,
            contract_path: ORACLE_STOCHASTIC_CONTRACT_PATH.to_string(),
        }
    }
}

/// Oracle-side stochastic identity per issue #50 contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleSeedIdentity {
    /// Oracle kind: pristine-oracle or seedable-validation-oracle.
    pub kind: OracleKind,
    /// Required for seedable-validation-oracle; forbidden for pristine-oracle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<OracleStrategyRef>,
    /// Requested identity [1, 1000000000]. Null means default mode.
    /// For pristine-oracle it MUST be null; for the seedable oracle null is
    /// the #50 default-equivalent offset-zero mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    /// Number of repetitions for repeatability characterization.
    #[serde(default = "default_repetitions")]
    pub repetitions: u32,
}

fn default_repetitions() -> u32 {
    5
}

/// Stable reference to a metric/threshold definition owned outside #51.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationDefinitionRef {
    pub id: String,
    pub version: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationDefinitionRefs {
    pub metric_contracts: Vec<ValidationDefinitionRef>,
    pub threshold_contracts: Vec<ValidationDefinitionRef>,
}

/// Execution profile reference (from frozen #49 contract).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Producer namespace for a required validation artifact.
///
/// Paths, hashes, and concrete immutable artifact identities are intentionally
/// not part of the case contract; #53 binds these declarations to actual run
/// artifacts and provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactProducer {
    Candidate,
    Oracle,
    ValidationPipeline,
}

/// Semantic class of a required validation artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactClass {
    RawModelOutput,
    DecodedModelOutput,
    ComparisonReport,
    RunManifest,
}

/// One stable artifact requirement declared by the case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedArtifactRequirement {
    pub id: String,
    pub producer: ArtifactProducer,
    pub class: ArtifactClass,
}

/// Required artifact classes for a validation case.
/// #51 declares what evidence must exist; #53 owns path/hash attribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedArtifacts {
    pub required: Vec<ExpectedArtifactRequirement>,
}

/// Oracle turbulence/integration formulation selection (Issue #67).
///
/// FLEXPART derives two coupled behaviours from the COMMAND `ctl` value
/// (pinned oracle, reference/flexpart-11.1.json at commit c70586c):
///
/// - the dispersion method (`readoptions_mod.f90:786-795`): `ctl > 0` selects
///   the adaptive particle-timestep method (`method=1`, `mintime=minstep`);
///   `ctl <= 0` selects the fixed-timestep method (`method=0`,
///   `mintime=lsynctime`);
/// - the Markov-chain formulation (`readoptions_mod.f90:626,645-650`):
///   `ctl >= 0.1` selects the w/sigw formulation (`turbswitch=.true.`);
///   `ctl < 0.1` silently selects the w formulation and forces `ifine=1`.
///
/// Schema v2 supports the two formulations that are actually present in the
/// checked-in validation corpus. The synthetic corpus uses adaptive w/sigw;
/// ETEX-MINI-013 preserves its historical fixed-timestep / w formulation.
/// The typed value is authoritative and must agree with `ctl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleTurbulenceFormulation {
    /// Adaptive integration (`method=1`, readoptions_mod.f90:786-795) with the
    /// w/sigw Markov formulation (`turbswitch=.true.`, readoptions_mod.f90:645-650).
    #[serde(rename = "adaptive_w_sigw")]
    AdaptiveWSigmaW,
    /// Fixed particle timestep (`method=0`, `mintime=lsynctime`) with the
    /// w Markov formulation selected by CTL < 0.1. FLEXPART forces effective
    /// IFINE=1 in this formulation even if the raw COMMAND contains another
    /// IFINE value; ETEX-MINI-013 historically contains IFINE=4.
    #[serde(rename = "fixed_sync_w")]
    FixedSyncW,
}

impl std::fmt::Display for OracleTurbulenceFormulation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OracleTurbulenceFormulation::AdaptiveWSigmaW => "adaptive_w_sigw",
            OracleTurbulenceFormulation::FixedSyncW => "fixed_sync_w",
        })
    }
}

/// `ctl` threshold of the pinned oracle's w/sigw Markov formulation
/// (`turbswitch=.true.`, readoptions_mod.f90:645-650) and lower bound of the
/// adaptive dispersion method's valid `ctl` range. Single documented constant
/// shared with the Python generator contract (step 6 of Issue #67).
pub const CTL_W_SIGW_FORMULATION_THRESHOLD: f32 = 0.1;

/// Oracle command overrides (namelist values).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleCommandOverrides {
    /// Declared turbulence/integration formulation (required, never defaulted
    /// in a document; see `OracleTurbulenceFormulation`).
    pub turbulence_formulation: OracleTurbulenceFormulation,
    /// Turbulence flag (0/1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lturbulence: Option<u8>,
    /// Convection flag (0/1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lconvection: Option<u8>,
    /// CTL parameter (Hanna turbulence scaling).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctl: Option<f32>,
    /// IFINE value written to COMMAND. In fixed_sync_w FLEXPART forces the
    /// effective value to 1 internally (readoptions_mod.f90:645-650).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ifine: Option<u32>,
    /// FLEXPART synchronisation interval (COMMAND LSYNCTIME) [s].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lsynctime_s: Option<u32>,
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

/// A known limitation that #52 must consider when proving input equivalence.
/// This is declarative case context, not an equivalence verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputEquivalenceLimitation {
    pub code: String,
    pub description: String,
}

/// Structured representation differences for #52 input-equivalence evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
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
    /// Known input-equivalence limitations carried forward for #52.
    /// Absence does not imply demonstrated equivalence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_input_equivalence_limitations: Vec<InputEquivalenceLimitation>,
    /// Additional notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<String>>,
}


/// Simulation direction, i.e. the FLEXPART `LDIRECT` semantic
/// (`readoptions_mod.f90`): `ldirect` contains the direction of time,
/// 1 for forward, -1 for backward.
///
/// Required and typed so a document can never fall back to an implicit
/// direction (Issue #51 / #57): the raw numeric key stays on the generated
/// namelist, while the manifest carries the canonical semantic value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimulationDirection {
    /// Forward simulation in time (FLEXPART `LDIRECT = 1`).
    Forward,
    /// Backward simulation in time (FLEXPART `LDIRECT = -1`).
    Backward,
}

impl SimulationDirection {
    /// FLEXPART COMMAND `LDIRECT` value this direction maps to.
    #[must_use]
    pub const fn flexpart_ldirect(self) -> i32 {
        match self {
            SimulationDirection::Forward => 1,
            SimulationDirection::Backward => -1,
        }
    }
}

/// Scientific quantity represented by each produced output field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputQuantity {
    /// Time-averaged mass concentration [kg/m3] over the averaging window.
    TimeAveragedMassConcentrationKgM3,
}

/// Output timing and scientific semantics (Issue #51 / #57).
///
/// Maps one-to-one onto the FLEXPART COMMAND keys `LOUTSTEP` / `LOUTAVER` /
/// `LOUTSAMPLE` (`readoptions_mod.f90`): an output field is written every
/// `interval_s` seconds, its values averaging particle samples taken every
/// `sampling_interval_s` seconds across the `averaging_window_s` window.
/// The FLEXPART binary header writes the triplet verbatim
/// (`binary_output_mod.f90`). No workflow default cushions a missing field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSpec {
    /// Output interval (FLEXPART `LOUTSTEP`) [s].
    pub interval_s: u32,
    /// Averaging window (FLEXPART `LOUTAVER`) [s].
    pub averaging_window_s: u32,
    /// Sampling interval (FLEXPART `LOUTSAMPLE`) [s].
    pub sampling_interval_s: u32,
    /// Scientific quantity each output field represents.
    pub quantity: OutputQuantity,
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
    /// Oracle COMMAND namelist overrides. Required; no default block exists.
    pub oracle_command_overrides: OracleCommandOverrides,
    /// Expected output artifacts.
    pub expected_artifacts: ExpectedArtifacts,
    /// Structured representation differences and known limitations for #52.
    /// This field never contains an input-equivalence verdict.
    #[serde(default)]
    pub representation_differences: RepresentationDifferences,
    /// Whether the release geometry must lie inside the domain.
    /// Synthetic corpus cases require containment; ETEX-MINI-013 waives it
    /// (placeholder domain/release pending #52 input-equivalence work) with
    /// rationale in `notes`. Defaults to true.
    #[serde(default = "default_containment_required")]
    pub require_source_containment: bool,
    /// Additional notes.
    #[serde(default)]
    pub notes: Vec<String>,
}

fn default_containment_required() -> bool {
    true
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
        // Oracle command overrides: reject ambiguous mixed-spelling documents
        // (legacy uppercase + canonical lowercase) before typed
        // deserialization. Legacy-only uppercase spellings are rejected by
        // `deny_unknown_fields` on `OracleCommandOverrides`, which names the
        // canonical alternative; v1 is frozen, see MIGRATION_NOTES.md.
        if let Some(overrides) = raw
            .get("oracle_command_overrides")
            .and_then(serde_json::Value::as_object)
        {
            const LEGACY_TO_CANONICAL: [(&str, &str); 8] = [
                ("LTURBULENCE", "lturbulence"),
                ("LCONVECTION", "lconvection"),
                ("CTL", "ctl"),
                ("IFINE", "ifine"),
                ("LSYNCTIME", "lsynctime_s"),
                ("LDRYDEP", "ldrydep"),
                ("LWETDEP", "lwetdep"),
                ("LDECAY", "ldecay"),
            ];
            for (legacy, canonical) in LEGACY_TO_CANONICAL {
                if overrides.contains_key(legacy) && overrides.contains_key(canonical) {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "oracle_command_overrides",
                        message: format!(
                            "document declares both `{legacy}` and `{canonical}`; \
                             ambiguous mixed-spelling oracle override rejected; \
                             keep only the canonical lowercase form"
                        ),
                    });
                }
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
        if !( -180.0..=360.0).contains(&self.domain.xlon0_deg) {
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
        if self.domain.wind_heights_m.iter().any(|h| !h.is_finite() || *h < 0.0) {
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
        let expected_total = self.integration.dt_s * self.integration.steps as f32;
        if !expected_total.is_finite()
            || (self.integration.total_s - expected_total).abs() > 1e-6
        {
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

        // Validate output timing semantics (positive, internally consistent)
        self.validate_output()?;

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

        self.validate_units()?;
        self.validate_validation_definition_refs()?;
        self.validate_representation_differences()?;
        self.validate_expected_artifacts()?;

        Ok(())
    }

    fn validate_representation_differences(&self) -> Result<(), ValidationCaseError> {
        for limitation in &self.representation_differences.known_input_equivalence_limitations {
            if limitation.code.trim().is_empty() || limitation.description.trim().is_empty() {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "representation_differences.known_input_equivalence_limitations",
                    message: "limitation code and description must not be empty".to_string(),
                });
            }
        }
        Ok(())
    }

    fn validate_expected_artifacts(&self) -> Result<(), ValidationCaseError> {
        if self.expected_artifacts.required.is_empty() { return Err(ValidationCaseError::MissingField { field: "expected_artifacts.required" }); }
        let mut ids = HashSet::new();
        let mut candidate_raw = false;
        let mut candidate_decoded = false;
        let mut comparison_report = false;
        let mut run_manifest = false;
        for artifact in &self.expected_artifacts.required {
            if artifact.id.trim().is_empty() { return Err(ValidationCaseError::AmbiguousField { field: "expected_artifacts.required.id", message: "artifact id must not be empty".to_string() }); }
            if !ids.insert(artifact.id.as_str()) { return Err(ValidationCaseError::AmbiguousField { field: "expected_artifacts.required.id", message: format!("duplicate artifact id {}", artifact.id) }); }
            match (artifact.producer, artifact.class) {
                (ArtifactProducer::Candidate, ArtifactClass::RawModelOutput) => candidate_raw = true,
                (ArtifactProducer::Candidate, ArtifactClass::DecodedModelOutput) => candidate_decoded = true,
                (ArtifactProducer::Oracle, ArtifactClass::RawModelOutput) | (ArtifactProducer::Oracle, ArtifactClass::DecodedModelOutput) => {},
                (ArtifactProducer::ValidationPipeline, ArtifactClass::ComparisonReport) => comparison_report = true,
                (ArtifactProducer::ValidationPipeline, ArtifactClass::RunManifest) => run_manifest = true,
                _ => return Err(ValidationCaseError::AmbiguousField { field: "expected_artifacts.required", message: format!("artifact {} has invalid producer/class pairing {:?}/{:?}", artifact.id, artifact.producer, artifact.class) }),
            }
        }
        for (present, label) in [(candidate_raw, "candidate/raw_model_output"), (candidate_decoded, "candidate/decoded_model_output"), (comparison_report, "validation_pipeline/comparison_report"), (run_manifest, "validation_pipeline/run_manifest")] {
            if !present { return Err(ValidationCaseError::AmbiguousField { field: "expected_artifacts.required", message: format!("missing required artifact class {label}") }); }
        }
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

    fn validate_validation_definition_refs(&self) -> Result<(), ValidationCaseError> {
        let validate =
            |field: &'static str,
             refs: &[ValidationDefinitionRef]|
             -> Result<(), ValidationCaseError> {
                if refs.is_empty() {
                    return Err(ValidationCaseError::MissingField { field });
                }
                for reference in refs {
                    if reference.id.is_empty()
                        || reference.version.is_empty()
                        || reference.path.is_empty()
                    {
                        return Err(ValidationCaseError::AmbiguousField {
                            field,
                            message: "definition references require non-empty id/version/path"
                                .to_string(),
                        });
                    }
                    if Path::new(&reference.path).is_absolute()
                        || reference.path.split('/').any(|segment| segment == "..")
                    {
                        return Err(ValidationCaseError::AmbiguousField {
                            field,
                            message: format!(
                                "definition reference path must be repository-relative without '..': {}",
                                reference.path
                            ),
                        });
                    }
                }
                Ok(())
            };
        validate(
            "validation_definition_refs.metric_contracts",
            &self.validation_definition_refs.metric_contracts,
        )?;
        validate(
            "validation_definition_refs.threshold_contracts",
            &self.validation_definition_refs.threshold_contracts,
        )?;
        Ok(())
    }

    fn validate_output(&self) -> Result<(), ValidationCaseError> {
        if self.simulation_direction != SimulationDirection::Forward {
            return Err(ValidationCaseError::AmbiguousField {
                field: "simulation_direction",
                message: "backward is a valid FLEXPART mode but is deliberately unsupported by schema v2: the current OutputQuantity models forward time-averaged mass concentration, while backward IOUT=1 uses source-receptor/residence-time semantics".to_string(),
            });
        }

        let output = &self.output;
        for (name, magnitude) in [
            ("output.interval_s", output.interval_s),
            ("output.averaging_window_s", output.averaging_window_s),
            ("output.sampling_interval_s", output.sampling_interval_s),
        ] {
            if magnitude == 0 {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{name} must be > 0, got {magnitude}"),
                });
            }
        }
        if output.sampling_interval_s > output.averaging_window_s {
            return Err(ValidationCaseError::AmbiguousField {
                field: "output.sampling_interval_s",
                message: format!(
                    "sampling interval {} s must not exceed averaging window {} s (FLEXPART LOUTSAMPLE <= LOUTAVER)",
                    output.sampling_interval_s, output.averaging_window_s
                ),
            });
        }
        if output.averaging_window_s > output.interval_s {
            return Err(ValidationCaseError::AmbiguousField {
                field: "output.averaging_window_s",
                message: format!(
                    "averaging window {} s must not exceed output interval {} s (FLEXPART LOUTAVER <= LOUTSTEP)",
                    output.averaging_window_s, output.interval_s
                ),
            });
        }

        let sync = self.oracle_command_overrides.lsynctime_s.ok_or(
            ValidationCaseError::MissingField {
                field: "oracle_command_overrides.lsynctime_s",
            },
        )?;
        if sync == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "oracle_command_overrides.lsynctime_s must be > 0".to_string(),
            });
        }
        for (name, magnitude) in [
            ("output.interval_s", output.interval_s),
            ("output.averaging_window_s", output.averaging_window_s),
            ("output.sampling_interval_s", output.sampling_interval_s),
        ] {
            if magnitude % sync != 0 {
                return Err(ValidationCaseError::AmbiguousField {
                    field: name,
                    message: format!(
                        "{name}={magnitude} s must be a multiple of the declared FLEXPART LSYNCTIME={sync} s"
                    ),
                });
            }
        }
        if output.averaging_window_s < 2 * sync {
            return Err(ValidationCaseError::AmbiguousField {
                field: "output.averaging_window_s",
                message: format!(
                    "averaging window {} s must be at least 2*LSYNCTIME={} s",
                    output.averaging_window_s,
                    2 * sync
                ),
            });
        }
        if output.interval_s < 2 * sync {
            return Err(ValidationCaseError::AmbiguousField {
                field: "output.interval_s",
                message: format!(
                    "output interval {} s must be at least 2*LSYNCTIME={} s",
                    output.interval_s,
                    2 * sync
                ),
            });
        }
        Ok(())
    }

    fn parse_timestamp_seconds(
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

        Ok(days * 86_400
            + i64::from(hour) * 3_600
            + i64::from(minute) * 60
            + i64::from(second))
    }

    fn validate_timestamp(value: &str, field: &'static str) -> Result<(), ValidationCaseError> {
        Self::parse_timestamp_seconds(value, field).map(|_| ())
    }

    fn simulation_bounds_seconds(&self) -> Result<(f64, f64), ValidationCaseError> {
        let start = Self::parse_timestamp_seconds(&self.integration.start, "integration.start")?
            as f64;
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
        let max = Self::parse_timestamp_seconds(
            "99991231235959",
            "integration.start",
        )? as f64;
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

    fn validate_chronology(&self) -> Result<(), ValidationCaseError> {
        let (sim_start, sim_end) = self.simulation_bounds_seconds()?;
        let check_inside = |
            value: &str,
            field: &'static str,
        | -> Result<f64, ValidationCaseError> {
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

    fn validate_release(&self) -> Result<(), ValidationCaseError> {
        let release = &self.release;
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
            let implied =
                f64::from(per_particle) * f64::from(release.particle_count);
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
                message: format!(
                    "{} must match SPECIES_<NNN>",
                    release.species.id
                ),
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
            SourceGeometry::Point { lon_deg, lat_deg, z_m } => {
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
                        message: format!(
                            "lon_max {lon_max_deg} must be >= lon_min {lon_min_deg}"
                        ),
                    });
                }
                if lat_max_deg < lat_min_deg {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.geometry.lat_max_deg",
                        message: format!(
                            "lat_max {lat_max_deg} must be >= lat_min {lat_min_deg}"
                        ),
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
        let lon_max = lon_min + f64::from(domain.nx) * f64::from(domain.dx_deg);
        let lat_min = f64::from(domain.ylat0_deg);
        let lat_max = lat_min + f64::from(domain.ny) * f64::from(domain.dy_deg);
        let height_min = f64::from(*domain.wind_heights_m.first().unwrap_or(&0.0));
        let height_max = f64::from(*domain.wind_heights_m.last().unwrap_or(&0.0));
        let epsilon = 1e-6;
        let mut check_point =
            |name: &'static str, lon: f64, lat: f64, z: f64| -> Result<(), ValidationCaseError> {
                if lon < lon_min - epsilon || lon > lon_max + epsilon {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.geometry",
                        message: format!(
                            "{name} lon {lon} outside domain [{lon_min}, {lon_max}]"
                        ),
                    });
                }
                if lat < lat_min - epsilon || lat > lat_max + epsilon {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "release.geometry",
                        message: format!(
                            "{name} lat {lat} outside domain [{lat_min}, {lat_max}]"
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
            SourceGeometry::Point { lon_deg, lat_deg, z_m } => {
                check_point("point", f64::from(*lon_deg), f64::from(*lat_deg), f64::from(*z_m))?;
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
            | WindSpec::RealWeather { .. } => {}
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

        // Validate meteorology specification for real-weather cases
        if let WindSpec::RealWeather { meteorology } = &self.wind {
            self.validate_meteorology(meteorology)?;
        }

        Ok(())
    }

    fn validate_meteorology(&self, met: &MeteorologySpec) -> Result<(), ValidationCaseError> {
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
        if met.source_path.starts_with('/') || met.source_path.starts_with('.') {
            return Err(ValidationCaseError::AmbiguousField {
                field: "meteorology.source_path",
                message: "source_path must be a normalized repository-relative path (no leading '/' or '.')".to_string(),
            });
        }

        // Validate digest format (sha256 or manifest reference)
        if met.digest.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "meteorology.digest",
            });
        }
        // Accept either 64-char hex (sha256) or "manifest:<path>"
        if met.digest.len() != 64 && !met.digest.starts_with("manifest:") {
            return Err(ValidationCaseError::AmbiguousField {
                field: "meteorology.digest",
                message: "digest must be 64-char hex sha256 or 'manifest:<path>'".to_string(),
            });
        }
        if met.digest.len() == 64 && !met.digest.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ValidationCaseError::AmbiguousField {
                field: "meteorology.digest",
                message: "sha256 digest must be hexadecimal".to_string(),
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

    fn validate_species_physics_contract(&self) -> Result<(), ValidationCaseError> {
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

    fn validate_stochastic(&self) -> Result<(), ValidationCaseError> {
        if let Some(candidate) = &self.stochastic.candidate_philox {
            if candidate.count == 0 {
                return Err(ValidationCaseError::InvalidStochasticIdentity {
                    message: "candidate_philox.count must be > 0".to_string(),
                });
            }
        }

        if let Some(oracle) = &self.stochastic.oracle_seed {
            if oracle.repetitions == 0 {
                return Err(ValidationCaseError::InvalidStochasticIdentity {
                    message: "oracle_seed.repetitions must be > 0".to_string(),
                });
            }
            match oracle.kind {
                OracleKind::PristineOracle => {
                    if oracle.seed.is_some() {
                        return Err(ValidationCaseError::InvalidStochasticIdentity {
                            message: "pristine-oracle cannot carry a requested seed; use seed=null for pristine/default mode".to_string(),
                        });
                    }
                    if oracle.strategy.is_some() {
                        return Err(ValidationCaseError::InvalidStochasticIdentity {
                            message: "pristine-oracle cannot declare the seedable #50 strategy".to_string(),
                        });
                    }
                }
                OracleKind::SeedableValidationOracle => {
                    let strategy = oracle.strategy.as_ref().ok_or_else(|| {
                        ValidationCaseError::InvalidStochasticIdentity {
                            message: "seedable-validation-oracle requires a stable #50 strategy reference".to_string(),
                        }
                    })?;
                    if strategy.strategy != ORACLE_STOCHASTIC_STRATEGY_ID
                        || strategy.version != ORACLE_STOCHASTIC_STRATEGY_VERSION
                        || strategy.contract_path != ORACLE_STOCHASTIC_CONTRACT_PATH
                    {
                        return Err(ValidationCaseError::InvalidStochasticIdentity {
                            message: format!(
                                "unsupported oracle strategy reference: expected {} v{} at {}, got {} v{} at {}",
                                ORACLE_STOCHASTIC_STRATEGY_ID,
                                ORACLE_STOCHASTIC_STRATEGY_VERSION,
                                ORACLE_STOCHASTIC_CONTRACT_PATH,
                                strategy.strategy,
                                strategy.version,
                                strategy.contract_path
                            ),
                        });
                    }
                    if let Some(seed) = oracle.seed {
                        if seed == 0 || seed > 1_000_000_000 {
                            return Err(ValidationCaseError::InvalidStochasticIdentity {
                                message: format!(
                                    "oracle_seed.seed must be in [1, 1000000000], got {seed}"
                                ),
                            });
                        }
                    }
                }
            }
        }

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
        Self::validate_oracle_ctl_formulation(ctl, overrides.turbulence_formulation)?;
        let ifine = overrides.ifine.ok_or(ValidationCaseError::MissingField {
            field: "oracle_command_overrides.ifine",
        })?;
        let lsynctime_s = overrides.lsynctime_s.ok_or(ValidationCaseError::MissingField {
            field: "oracle_command_overrides.lsynctime_s",
        })?;
        if lsynctime_s == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "oracle_command_overrides.lsynctime_s must be > 0".to_string(),
            });
        }
        if !(1..=10).contains(&ifine) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "oracle_command_overrides.ifine must be in 1..=10 (the oracle silently \
                     clamps IFINE to >= 1 via max(ifine,1), readoptions_mod.f90:624; the \
                     corpus contract caps the vertical sub-stepping at 10), got {ifine}"
                ),
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
        self.validate_oracle_physics_agreement(lturbulence, lconvection)?;
        Ok(())
    }

    /// Enforces the Issue #67 `ctl` contract for the pinned oracle
    /// (reference/flexpart-11.1.json):
    ///
    /// - `ctl` must be finite;
    /// - `ctl = 0` is a division by zero: the oracle computes `ctl = 1./ctl`
    ///   unconditionally (`readoptions_mod.f90:653`) and sizes particle steps
    ///   from it (`advance_mod.f90:557-568`);
    /// - `ctl < 0` selects the fixed-timestep dispersion mode (`method=0`,
    ///   `mintime=lsynctime`) and the w formulation; this is represented
    ///   explicitly as `fixed_sync_w` for ETEX-MINI-013;
    /// - `0 < ctl < CTL_W_SIGW_FORMULATION_THRESHOLD` selects adaptive timing
    ///   but silently switches the Markov chain to w and forces `ifine=1`;
    ///   that mixed mode is not present in the corpus and remains unsupported.
    fn validate_oracle_ctl_formulation(
        ctl: f32,
        formulation: OracleTurbulenceFormulation,
    ) -> Result<(), ValidationCaseError> {
        if !ctl.is_finite() {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!("oracle_command_overrides.ctl must be finite, got {ctl}"),
            });
        }
        if ctl == 0.0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "oracle_command_overrides.ctl must be non-zero: the oracle computes \
                     ctl = 1./ctl unconditionally (readoptions_mod.f90:653) and sizes \
                     particle time steps from it (advance_mod.f90:557-568); CTL=0 is a \
                     division by zero producing a divergent step, got {ctl}"
                ),
            });
        }

        match formulation {
            OracleTurbulenceFormulation::AdaptiveWSigmaW => {
                if ctl < CTL_W_SIGW_FORMULATION_THRESHOLD {
                    return Err(ValidationCaseError::InvalidPhysicsSwitches {
                        message: format!(
                            "oracle_command_overrides.ctl={ctl} contradicts \
                             turbulence_formulation={formulation}: adaptive_w_sigw requires \
                             CTL >= {CTL_W_SIGW_FORMULATION_THRESHOLD}; CTL <= 0 selects \
                             fixed-timestep method=0/mintime=lsynctime and values below the \
                             threshold select the w formulation and force effective IFINE=1 \
                             (readoptions_mod.f90:645-650,786-795)"
                        ),
                    });
                }
            }
            OracleTurbulenceFormulation::FixedSyncW => {
                if ctl >= 0.0 {
                    return Err(ValidationCaseError::InvalidPhysicsSwitches {
                        message: format!(
                            "oracle_command_overrides.ctl={ctl} contradicts \
                             turbulence_formulation={formulation}: fixed_sync_w requires \
                             CTL < 0 so FLEXPART selects method=0 with mintime=lsynctime; \
                             CTL < 0.1 also selects the w formulation and forces effective \
                             IFINE=1 (readoptions_mod.f90:645-650,786-795)"
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Cross-checks that oracle COMMAND switches agree with the declared
    /// `physics_switches`, so a case cannot silently run different physics than
    /// it claims. Departures from Fortran module state are rejected as
    /// `InvalidPhysicsSwitches`.
    fn validate_oracle_physics_agreement(
        &self,
        lturbulence: u8,
        lconvection: u8,
    ) -> Result<(), ValidationCaseError> {
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
        for (name, value, physics_key, physics_value) in [
            (
                "oracle_command_overrides.ldrydep",
                self.oracle_command_overrides.ldrydep,
                "physics_switches.dry_deposition",
                self.physics_switches.dry_deposition,
            ),
            (
                "oracle_command_overrides.lwetdep",
                self.oracle_command_overrides.lwetdep,
                "physics_switches.wet_deposition",
                self.physics_switches.wet_deposition,
            ),
            (
                "oracle_command_overrides.ldecay",
                self.oracle_command_overrides.ldecay,
                "physics_switches.decay",
                self.physics_switches.decay,
            ),
        ] {
            if let Some(value) = value {
                if value != u8::from(physics_value) {
                    return Err(ValidationCaseError::InvalidPhysicsSwitches {
                        message: format!("{physics_key}={physics_value} conflicts with oracle {name}={value}"),
                    });
                }
            }
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
    /// Validates first so invalid in-memory values can never be serialized
    /// as apparently valid contract documents.
    ///
    /// # Errors
    /// Returns the validation error if the manifest is invalid,
    /// [`ValidationCaseError::ParseJson`] if serialization fails,
    /// or [`ValidationCaseError::WriteFile`] if the file cannot be written.
    pub fn write_to_file(&self, path: &Path) -> Result<(), ValidationCaseError> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self).map_err(|source| ValidationCaseError::ParseJson {
            path: path.to_path_buf(),
            source,
        })?;
        std::fs::write(path, json).map_err(|source| ValidationCaseError::WriteFile {
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

    fn json_schema_type_matches(expected: &str, value: &serde_json::Value) -> bool {
        match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => false,
        }
    }

    /// Minimal Draft 2020-12 evaluator for the keyword subset used by the
    /// checked-in validation-case schema. This keeps the schema test
    /// dependency-free while validating the actual schema document, not a
    /// second hand-written fixture shape.
    fn validate_json_schema_subset(
        root: &serde_json::Value,
        schema: &serde_json::Value,
        value: &serde_json::Value,
        path: &str,
    ) -> Result<(), String> {
        let node = schema
            .as_object()
            .ok_or_else(|| format!("{path}: schema node is not an object"))?;

        if let Some(reference) = node.get("$ref").and_then(serde_json::Value::as_str) {
            let pointer = reference
                .strip_prefix('#')
                .ok_or_else(|| format!("{path}: only local schema refs are supported: {reference}"))?;
            let target = root
                .pointer(pointer)
                .ok_or_else(|| format!("{path}: unresolved schema ref {reference}"))?;
            return validate_json_schema_subset(root, target, value, path);
        }

        if let Some(branches) = node.get("oneOf").and_then(serde_json::Value::as_array) {
            let matches = branches
                .iter()
                .filter(|branch| validate_json_schema_subset(root, branch, value, path).is_ok())
                .count();
            if matches != 1 {
                return Err(format!(
                    "{path}: oneOf expected exactly one matching branch, got {matches}"
                ));
            }
        }

        if let Some(expected) = node.get("const") {
            if expected != value {
                return Err(format!("{path}: expected const {expected}, got {value}"));
            }
        }

        if let Some(allowed) = node.get("enum").and_then(serde_json::Value::as_array) {
            if !allowed.iter().any(|candidate| candidate == value) {
                return Err(format!("{path}: value {value} is not in enum {allowed:?}"));
            }
        }

        if let Some(expected_type) = node.get("type").and_then(serde_json::Value::as_str) {
            if !json_schema_type_matches(expected_type, value) {
                return Err(format!(
                    "{path}: expected JSON type {expected_type}, got {value}"
                ));
            }
        }

        if let Some(object) = value.as_object() {
            if let Some(required) = node.get("required").and_then(serde_json::Value::as_array) {
                for key in required.iter().filter_map(serde_json::Value::as_str) {
                    if !object.contains_key(key) {
                        return Err(format!("{path}: missing required property {key}"));
                    }
                }
            }

            let properties = node
                .get("properties")
                .and_then(serde_json::Value::as_object);
            for (key, child) in object {
                if let Some(child_schema) = properties.and_then(|props| props.get(key)) {
                    validate_json_schema_subset(
                        root,
                        child_schema,
                        child,
                        &format!("{path}.{key}"),
                    )?;
                } else if node
                    .get("additionalProperties")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                {
                    return Err(format!("{path}: unknown property {key}"));
                }
            }
        }

        if let Some(array) = value.as_array() {
            if let Some(min_items) = node.get("minItems").and_then(serde_json::Value::as_u64) {
                if array.len() < min_items as usize {
                    return Err(format!("{path}: fewer than {min_items} items"));
                }
            }
            if let Some(max_items) = node.get("maxItems").and_then(serde_json::Value::as_u64) {
                if array.len() > max_items as usize {
                    return Err(format!("{path}: more than {max_items} items"));
                }
            }
            if let Some(item_schema) = node.get("items") {
                for (index, child) in array.iter().enumerate() {
                    validate_json_schema_subset(
                        root,
                        item_schema,
                        child,
                        &format!("{path}[{index}]"),
                    )?;
                }
            }
        }

        if let Some(text) = value.as_str() {
            if let Some(min_length) = node.get("minLength").and_then(serde_json::Value::as_u64) {
                if text.chars().count() < min_length as usize {
                    return Err(format!("{path}: string shorter than {min_length}"));
                }
            }
            if let Some(max_length) = node.get("maxLength").and_then(serde_json::Value::as_u64) {
                if text.chars().count() > max_length as usize {
                    return Err(format!("{path}: string longer than {max_length}"));
                }
            }
            if let Some(pattern) = node.get("pattern").and_then(serde_json::Value::as_str) {
                match pattern {
                    "^[0-9]{14}$" => {
                        if text.len() != 14 || !text.bytes().all(|b| b.is_ascii_digit()) {
                            return Err(format!("{path}: string does not match {pattern}"));
                        }
                    }
                    other => {
                        return Err(format!(
                            "{path}: schema test evaluator does not support pattern {other}"
                        ));
                    }
                }
            }
        }

        if let Some(number) = value.as_f64() {
            if let Some(minimum) = node.get("minimum").and_then(serde_json::Value::as_f64) {
                if number < minimum {
                    return Err(format!("{path}: {number} is below minimum {minimum}"));
                }
            }
            if let Some(minimum) = node
                .get("exclusiveMinimum")
                .and_then(serde_json::Value::as_f64)
            {
                if number <= minimum {
                    return Err(format!(
                        "{path}: {number} is not greater than exclusiveMinimum {minimum}"
                    ));
                }
            }
            if let Some(maximum) = node.get("maximum").and_then(serde_json::Value::as_f64) {
                if number > maximum {
                    return Err(format!("{path}: {number} exceeds maximum {maximum}"));
                }
            }
            if let Some(maximum) = node
                .get("exclusiveMaximum")
                .and_then(serde_json::Value::as_f64)
            {
                if number >= maximum {
                    return Err(format!(
                        "{path}: {number} is not less than exclusiveMaximum {maximum}"
                    ));
                }
            }
        }

        Ok(())
    }

    fn load_validation_case_schema() -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(VALIDATION_CASE_SCHEMA_PATH);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
    }

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
                horizontal_ref: HorizontalCoordRef::GeographicLonLatDegrees,
                wind_heights_m: vec![50.0, 100.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0],
                wind_heights_ref: VerticalRef::Agl,
            },
            release: ReleaseSpec {
                geometry: SourceGeometry::Point {
                    lon_deg: 10.0,
                    lat_deg: 10.0,
                    z_m: 50.0,
                },
                vertical_ref: VerticalRef::Agl,
                timing: ReleaseTiming::Instant {
                    at: "20240101000000".to_string(),
                },
                species: SpeciesRef {
                    id: "SPECIES_024".to_string(),
                    physics_contract: SpeciesPhysicsContractRef {
                        profile: SpeciesPhysicsProfile::Species024InertV1,
                        id: SPECIES_024_INERT_CONTRACT_ID.to_string(),
                        version: 1,
                        path: SPECIES_024_INERT_CONTRACT_PATH.to_string(),
                        git_blob_sha: SPECIES_024_INERT_CONTRACT_BLOB.to_string(),
                    },
                },
                inventory: ReleaseInventory {
                    quantity_kg: 1.0,
                    unit: MassUnit::Kg,
                },
                particle_count: 1000,
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
            simulation_direction: SimulationDirection::Forward,
            output: OutputSpec {
                interval_s: 1800,
                averaging_window_s: 1800,
                sampling_interval_s: 300,
                quantity: OutputQuantity::TimeAveragedMassConcentrationKgM3,
            },
            deposition: None,
            units: UnitsSpec {
                wind: "m/s".to_string(),
                displacement: None,
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
                    derivation: CandidatePhiloxDerivation::WrappingAddKey0V1,
                }),
                oracle_seed: Some(OracleSeedIdentity {
                    kind: OracleKind::SeedableValidationOracle,
                    strategy: Some(OracleStrategyRef::canonical()),
                    seed: Some(1),
                    repetitions: 5,
                }),
            },
            validation_definition_refs: ValidationDefinitionRefs {
                metric_contracts: vec![ValidationDefinitionRef {
                    id: "evaluation-metrics".to_string(),
                    version: "report-schema-1.0.0".to_string(),
                    path: "scripts/evaluate/metrics.py".to_string(),
                }],
                threshold_contracts: vec![
                    ValidationDefinitionRef {
                        id: "scientific-thresholds".to_string(),
                        version: "v1".to_string(),
                        path: "evaluation/thresholds/scientific-thresholds-v1.json".to_string(),
                    },
                    ValidationDefinitionRef {
                        id: "corpus-thresholds".to_string(),
                        version: "1".to_string(),
                        path: "fixtures/corpus/thresholds.json".to_string(),
                    },
                ],
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
                lsynctime_s: Some(300),
                lconvection: Some(0),
                ldrydep: None,
                lwetdep: None,
                ldecay: None,
            },
            expected_artifacts: ExpectedArtifacts {
                required: vec![
                    ExpectedArtifactRequirement { id: "candidate.raw".to_string(), producer: ArtifactProducer::Candidate, class: ArtifactClass::RawModelOutput },
                    ExpectedArtifactRequirement { id: "candidate.decoded".to_string(), producer: ArtifactProducer::Candidate, class: ArtifactClass::DecodedModelOutput },
                    ExpectedArtifactRequirement { id: "oracle.raw".to_string(), producer: ArtifactProducer::Oracle, class: ArtifactClass::RawModelOutput },
                    ExpectedArtifactRequirement { id: "oracle.decoded".to_string(), producer: ArtifactProducer::Oracle, class: ArtifactClass::DecodedModelOutput },
                    ExpectedArtifactRequirement { id: "comparison.report".to_string(), producer: ArtifactProducer::ValidationPipeline, class: ArtifactClass::ComparisonReport },
                    ExpectedArtifactRequirement { id: "run.manifest".to_string(), producer: ArtifactProducer::ValidationPipeline, class: ArtifactClass::RunManifest },
                ],
            },
            representation_differences: RepresentationDifferences::default(),
            require_source_containment: true,
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
    fn checked_in_cases_match_machine_readable_schema_and_rust_contract() {
        let schema = load_validation_case_schema();
        assert_eq!(
            schema.get("$schema").and_then(serde_json::Value::as_str),
            Some("https://json-schema.org/draft/2020-12/schema")
        );
        assert_eq!(
            schema
                .pointer("/properties/schema_version/const")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(VALIDATION_CASE_SCHEMA_VERSION))
        );

        for case_id in [
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
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("corpus")
                .join("cases")
                .join(format!("{case_id}.json"));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {case_id}: {e}"));
            let raw: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("parse JSON {case_id}: {e}"));

            validate_json_schema_subset(&schema, &schema, &raw, "$")
                .unwrap_or_else(|e| panic!("JSON Schema rejected {case_id}: {e}"));

            ValidationCaseManifest::parse(&text, &path)
                .unwrap_or_else(|e| panic!("Rust contract rejected {case_id}: {e}"));
        }
    }

    #[test]
    fn machine_readable_schema_fails_closed_on_shape_and_version() {
        let schema = load_validation_case_schema();
        let raw = minimal_manifest_json();
        validate_json_schema_subset(&schema, &schema, &raw, "$")
            .expect("minimal Rust manifest must satisfy JSON Schema");

        let mut wrong_version = raw.clone();
        wrong_version["schema_version"] = serde_json::json!(999);
        assert!(
            validate_json_schema_subset(&schema, &schema, &wrong_version, "$").is_err(),
            "unsupported schema version must fail JSON Schema"
        );

        let mut missing_output = raw.clone();
        missing_output
            .as_object_mut()
            .expect("manifest object")
            .remove("output");
        assert!(
            validate_json_schema_subset(&schema, &schema, &missing_output, "$").is_err(),
            "missing required output semantics must fail JSON Schema"
        );

        let mut unknown_field = raw;
        unknown_field
            .as_object_mut()
            .expect("manifest object")
            .insert("hidden_default".to_string(), serde_json::json!(true));
        assert!(
            validate_json_schema_subset(&schema, &schema, &unknown_field, "$").is_err(),
            "unknown top-level fields must fail JSON Schema"
        );
    }


    #[test]
    fn write_failure_uses_write_file_error() {
        let manifest = make_minimal_manifest();
        let dir = tempfile::tempdir().expect("temp dir");
        let err = manifest
            .write_to_file(dir.path())
            .expect_err("writing JSON to a directory must fail");
        assert!(matches!(err, ValidationCaseError::WriteFile { .. }));
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
            strategy: Some(OracleStrategyRef::canonical()),
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
            strategy: Some(OracleStrategyRef::canonical()),
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
    fn pristine_oracle_default_mode_is_seedless() {
        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed = Some(OracleSeedIdentity {
            kind: OracleKind::PristineOracle,
            strategy: None,
            seed: None,
            repetitions: 2,
        });
        manifest.validate().expect("pristine default mode validates");
        manifest.stochastic.oracle_seed.as_mut().unwrap().seed = Some(1);
        let err = manifest.validate().expect_err("pristine seed must fail");
        assert!(err.to_string().contains("pristine-oracle"));
    }

    #[test]
    fn seedable_oracle_requires_exact_issue50_reference() {
        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed.as_mut().unwrap().strategy = None;
        let err = manifest.validate().expect_err("missing strategy must fail");
        assert!(err.to_string().contains("#50 strategy"));

        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed.as_mut().unwrap().strategy =
            Some(OracleStrategyRef {
                strategy: "wrong".to_string(),
                version: 1,
                contract_path: ORACLE_STOCHASTIC_CONTRACT_PATH.to_string(),
            });
        let err = manifest.validate().expect_err("wrong strategy must fail");
        assert!(err.to_string().contains("unsupported oracle strategy reference"));

        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed.as_mut().unwrap().seed = None;
        manifest.validate().expect("seedable default-equivalent mode validates");
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
        assert!(manifest.validate().expect_err("missing mass unit").to_string().contains("units.mass"));

        let mut manifest = make_minimal_manifest();
        manifest.units.concentration = Some("pg/m3".to_string());
        assert!(manifest.validate().expect_err("wrong concentration unit").to_string().contains("units.concentration"));

        let mut manifest = make_minimal_manifest();
        manifest.units.pressure = None;
        assert!(manifest.validate().expect_err("missing pressure unit").to_string().contains("units.pressure"));
    }

    #[test]
    fn validation_definition_refs_fail_closed() {
        let mut manifest = make_minimal_manifest();
        manifest.validation_definition_refs.metric_contracts.clear();
        assert!(manifest.validate().expect_err("missing metric refs").to_string().contains("metric_contracts"));

        let mut manifest = make_minimal_manifest();
        manifest.validation_definition_refs.threshold_contracts[0].path = "../bad.json".to_string();
        assert!(manifest.validate().expect_err("parent path").to_string().contains("repository-relative"));
    }

    #[test]
    fn simulation_direction_round_trips_and_maps_to_flexpart_ldirect() {
        let forward: SimulationDirection =
            serde_json::from_str("\"forward\"").expect("forward variant");
        let backward: SimulationDirection =
            serde_json::from_str("\"backward\"").expect("backward variant");
        assert_eq!(forward, SimulationDirection::Forward);
        assert_eq!(
            serde_json::to_string(&forward).expect("serialize forward"),
            "\"forward\""
        );
        assert_eq!(
            serde_json::to_string(&backward).expect("serialize backward"),
            "\"backward\""
        );
        assert!(
            serde_json::from_str::<SimulationDirection>("\"Forward\"").is_err(),
            "unknown variant must not silently coerce"
        );
        assert_eq!(forward.flexpart_ldirect(), 1);
        assert_eq!(backward.flexpart_ldirect(), -1);
    }

    #[test]
    fn case_level_input_equivalence_verdict_is_rejected() {
        let mut raw = minimal_manifest_json();
        raw.as_object_mut().expect("manifest object").insert("input_equivalence".to_string(), serde_json::json!("demonstrated"));
        let err = parse_json_value(&raw).expect_err("#52 verdict must not live in #51 manifest");
        assert!(err.to_string().contains("input_equivalence"));
    }

    #[test]
    fn expected_artifact_paths_are_rejected() {
        let mut raw = minimal_manifest_json();
        raw["expected_artifacts"] = serde_json::json!({"candidate_dir":"target/corpus/candidate/TEST-001","comparison_report":"target/corpus/comparison_report.json","run_manifest":"target/corpus/run_manifest.json"});
        let err = parse_json_value(&raw).expect_err("paths belong to #53, not #51");
        assert!(err.to_string().contains("candidate_dir"));
    }

    #[test]
    fn expected_artifacts_require_candidate_raw_and_decoded_classes() {
        let mut manifest = make_minimal_manifest();
        manifest.expected_artifacts.required.retain(|a| a.class != ArtifactClass::DecodedModelOutput);
        let err = manifest.validate().expect_err("candidate decoded artifact is required");
        assert!(err.to_string().contains("candidate/decoded_model_output"));
    }
    #[test]
    fn missing_simulation_direction_is_rejected() {
        let mut raw = minimal_manifest_json();
        raw.as_object_mut()
            .expect("manifest object")
            .remove("simulation_direction");
        let err = parse_json_value(&raw).expect_err("missing direction fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("simulation_direction"),
            "error must name the missing field: {rendered}"
        );
    }

    #[test]
    fn missing_output_spec_is_rejected() {
        let mut raw = minimal_manifest_json();
        raw.as_object_mut()
            .expect("manifest object")
            .remove("output");
        let err = parse_json_value(&raw).expect_err("missing output fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("output"),
            "error must name the missing field: {rendered}"
        );
    }

    #[test]
    fn missing_individual_output_timing_field_is_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.output.interval_s = 0;
        let err = manifest.validate().expect_err("zero interval fails");
        assert!(matches!(err, ValidationCaseError::InvalidPhysicsSwitches { .. }));

        let mut raw = minimal_manifest_json();
        raw["output"]
            .as_object_mut()
            .expect("output object")
            .remove("interval_s");
        let err = parse_json_value(&raw).expect_err("missing interval_s fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("interval_s"),
            "error must name the missing key: {rendered}"
        );
    }

    #[test]
    fn zero_output_timing_rejected() {
        for (field, magnitude) in [
            ("interval_s", 0_u32),
            ("averaging_window_s", 0_u32),
            ("sampling_interval_s", 0_u32),
        ] {
            let mut manifest = make_minimal_manifest();
            match field {
                "interval_s" => manifest.output.interval_s = magnitude,
                "averaging_window_s" => manifest.output.averaging_window_s = magnitude,
                "sampling_interval_s" => manifest.output.sampling_interval_s = magnitude,
                _ => unreachable!(),
            }
            let err = manifest.validate().expect_err("zero timing fails");
            assert!(
                err.to_string().contains(field),
                "error must name {field}: {err}"
            );
        }
    }

    #[test]
    fn sampling_interval_exceeding_averaging_window_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.output.sampling_interval_s = 1800;
        manifest.output.averaging_window_s = 900;
        let err = manifest.validate().expect_err("sample > average fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::AmbiguousField { field: "output.sampling_interval_s", .. }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn averaging_window_exceeding_output_interval_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.output.averaging_window_s = 3600;
        manifest.output.interval_s = 1800;
        let err = manifest.validate().expect_err("average > output fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::AmbiguousField { field: "output.averaging_window_s", .. }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn gregorian_timestamp_validation_rejects_impossible_dates_and_accepts_leap_day() {
        assert!(ValidationCaseManifest::validate_timestamp(
            "20240229010203",
            "integration.start"
        )
        .is_ok());
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
        let err = manifest.validate().expect_err("release before simulation must fail");
        assert!(err.to_string().contains("outside simulation window"));

        let mut manifest = make_minimal_manifest();
        manifest.release.timing = ReleaseTiming::Window {
            start: "20240101003000".to_string(),
            end: "20240101010001".to_string(),
        };
        let err = manifest.validate().expect_err("release after simulation must fail");
        assert!(err.to_string().contains("outside simulation window"));
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
    fn backward_direction_is_rejected_until_output_semantics_are_modeled() {
        let mut manifest = make_minimal_manifest();
        manifest.simulation_direction = SimulationDirection::Backward;
        let err = manifest.validate().expect_err("backward must fail closed in schema v2");
        assert!(matches!(
            err,
            ValidationCaseError::AmbiguousField {
                field: "simulation_direction",
                ..
            }
        ));
        assert!(err.to_string().contains("source-receptor"));
    }

    #[test]
    fn output_timings_must_match_declared_sync_interval() {
        let mut manifest = make_minimal_manifest();
        manifest.output.sampling_interval_s = 301;
        let err = manifest.validate().expect_err("non-multiple sample must fail");
        assert!(err.to_string().contains("LSYNCTIME"));

        let mut manifest = make_minimal_manifest();
        manifest.output.interval_s = manifest.oracle_command_overrides.lsynctime_s.expect("lsynctime");
        manifest.output.averaging_window_s = manifest.oracle_command_overrides.lsynctime_s.expect("lsynctime");
        manifest.output.sampling_interval_s = manifest.oracle_command_overrides.lsynctime_s.expect("lsynctime");
        let err = manifest.validate().expect_err("interval/average below 2*sync must fail");
        assert!(err.to_string().contains("2*LSYNCTIME"));
    }

    #[test]
    fn migrated_cases_keep_their_output_direction_semantics() {
        // Synthetic cases run forward with LOUTSTEP=1800 / LOUTAVER=1800 /
        // LOUTSAMPLE=300; ETEX-MINI-013 uses the real ETEX window
        // (LOUTSTEP=10800 / LOUTAVER=10800 / LOUTSAMPLE=900).
        for case_id in ALL_CHECKED_IN_CASES {
            let manifest = load_checked_in_case(case_id);
            assert_eq!(
                manifest.simulation_direction,
                SimulationDirection::Forward,
                "{case_id} direction changed"
            );
            assert_eq!(
                manifest.simulation_direction.flexpart_ldirect(),
                1,
                "{case_id} ldirect mapping"
            );
            if *case_id == "ETEX-MINI-013" {
                assert_eq!(manifest.output.interval_s, 10800, "{case_id}");
                assert_eq!(manifest.output.averaging_window_s, 10800, "{case_id}");
                assert_eq!(manifest.output.sampling_interval_s, 900, "{case_id}");
            } else {
                assert_eq!(manifest.output.interval_s, 1800, "{case_id}");
                assert_eq!(manifest.output.averaging_window_s, 1800, "{case_id}");
                assert_eq!(manifest.output.sampling_interval_s, 300, "{case_id}");
            }
            assert_eq!(
                manifest.output.quantity,
                OutputQuantity::TimeAveragedMassConcentrationKgM3,
                "{case_id} output quantity"
            );
        }
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
        assert!(!manifest.physics_switches.dry_deposition);
        assert!(!manifest.physics_switches.wet_deposition);
        assert_eq!(
            manifest.release.species.physics_contract.profile,
            SpeciesPhysicsProfile::Species024InertV1
        );
        assert!(manifest.surface.is_some());
        assert!(manifest.stochastic.candidate_philox.is_some());
        assert!(manifest.stochastic.oracle_seed.is_some());
        let oracle = manifest.stochastic.oracle_seed.as_ref().unwrap();
        assert_eq!(oracle.kind, OracleKind::SeedableValidationOracle);
        assert_eq!(oracle.seed, Some(1));
        assert_eq!(
            manifest.oracle_command_overrides.turbulence_formulation,
            OracleTurbulenceFormulation::FixedSyncW
        );
        assert_eq!(manifest.oracle_command_overrides.ctl, Some(-5.0));
        assert_eq!(manifest.oracle_command_overrides.ifine, Some(4));
        assert_eq!(manifest.oracle_command_overrides.lsynctime_s, Some(900));
        assert!(manifest.representation_differences.vertical_coordinate.is_some());
        assert!(manifest.representation_differences.wind_components.is_some());
        assert!(manifest.representation_differences.known_input_equivalence_limitations.iter().any(|l| l.code == "INPUT_EQUIVALENCE_NOT_DEMONSTRATED"));
        let serialized = serde_json::to_value(&manifest).expect("serialize ETEX manifest");
        assert!(serialized.get("input_equivalence").is_none());
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
            derivation: CandidatePhiloxDerivation::WrappingAddKey0V1,
        };
        assert_eq!(identity.key_for_seed_index(0), [u32::MAX, 305419896]);
        assert_eq!(identity.key_for_seed_index(1), [0, 305419896]);
        assert_eq!(identity.key_for_seed_index(2), [1, 305419896]);
    }

    #[test]
    fn candidate_derivation_is_executable_and_unknown_values_fail_closed() {
        let mut manifest = make_minimal_manifest();
        manifest
            .stochastic
            .candidate_philox
            .as_mut()
            .expect("candidate identity")
            .derivation = CandidatePhiloxDerivation::WrappingAddKey0V1;
        assert_ne!(
            manifest.candidate_seed_identity(0).expect("seed 0"),
            manifest.candidate_seed_identity(1).expect("seed 1")
        );

        manifest
            .stochastic
            .candidate_philox
            .as_mut()
            .expect("candidate identity")
            .derivation = CandidatePhiloxDerivation::ReuseBaseIdentityV1;
        assert_eq!(
            manifest.candidate_seed_identity(0).expect("repeat 0"),
            manifest.candidate_seed_identity(1).expect("repeat 1")
        );

        let mut raw = minimal_manifest_json();
        raw["stochastic"]["candidate_philox"]["derivation"] =
            serde_json::json!("some_future_or_misspelled_policy");
        let err = parse_json_value(&raw).expect_err("unknown derivation must fail closed");
        assert!(
            err.to_string().contains("derivation")
                || err.to_string().contains("unknown variant"),
            "unexpected: {err}"
        );
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
        assert_eq!(
            manifest.oracle_command_overrides.turbulence_formulation,
            OracleTurbulenceFormulation::AdaptiveWSigmaW
        );
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

    #[test]
    fn optional_oracle_flag_conflicting_with_physics_is_rejected() {
        let mut manifest = make_minimal_manifest();
        assert!(!manifest.physics_switches.dry_deposition);
        manifest.oracle_command_overrides.ldrydep = Some(1);
        let err = manifest.validate().expect_err("conflict must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
        assert!(err.to_string().contains("dry_deposition"));
    }

    #[test]
    fn oracle_ctl_five_point_zero_is_accepted() {
        let manifest = make_minimal_manifest();
        assert_eq!(manifest.oracle_command_overrides.ctl, Some(5.0));
        assert_eq!(
            manifest.oracle_command_overrides.turbulence_formulation,
            OracleTurbulenceFormulation::AdaptiveWSigmaW
        );
        manifest.validate().expect("CTL=5.0 corpus config stays valid");
    }

    #[test]
    fn zero_ctl_is_rejected_for_nonzero_timestep_division() {
        let mut manifest = make_minimal_manifest();
        manifest.oracle_command_overrides.ctl = Some(0.0);
        let err = manifest.validate().expect_err("ctl=0 must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
        assert!(err.to_string().contains("non-zero"));
        assert!(err.to_string().contains("readoptions_mod.f90:653"));
    }

    #[test]
    fn fixed_sync_ctl_requires_explicit_fixed_formulation() {
        let mut manifest = make_minimal_manifest();
        manifest.oracle_command_overrides.ctl = Some(-5.0);
        let err = manifest
            .validate()
            .expect_err("negative CTL with adaptive formulation must fail");
        assert!(err.to_string().contains("adaptive_w_sigw"));

        manifest.oracle_command_overrides.turbulence_formulation =
            OracleTurbulenceFormulation::FixedSyncW;
        manifest.validate().expect("explicit fixed_sync_w mode validates");
    }

    #[test]
    fn small_positive_ctl_is_rejected_for_turbulence_formulation() {
        let mut manifest = make_minimal_manifest();
        assert_eq!(manifest.oracle_command_overrides.lturbulence, Some(1));
        manifest.oracle_command_overrides.ctl = Some(0.05);
        let err = manifest.validate().expect_err("ctl below w-sigw threshold");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
        let rendered = err.to_string();
        assert!(
            rendered.contains("readoptions_mod.f90:645-650"),
            "error must cite the silent reformulation: {rendered}"
        );
    }

    #[test]
    fn small_positive_ctl_is_rejected_even_with_turbulence_disabled() {
        let mut manifest = make_minimal_manifest();
        manifest.physics_switches.turbulence = false;
        manifest.oracle_command_overrides.lturbulence = Some(0);
        manifest.oracle_command_overrides.ctl = Some(0.05);
        // The declared turbulence_formulation pins the w/sigw formulation, so a
        // sub-threshold CTL is inconsistent regardless of the LTURBULENCE flag.
        let err = manifest.validate().expect_err("formulation inconsistency must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
    }

    #[test]
    fn ifine_zero_is_rejected_for_silent_clamp() {
        let mut manifest = make_minimal_manifest();
        manifest.oracle_command_overrides.ifine = Some(0);
        let err = manifest.validate().expect_err("ifine=0 must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
        assert!(err.to_string().contains("readoptions_mod.f90:624"));
    }

    #[test]
    fn unknown_turbulence_formulation_variant_is_rejected() {
        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .insert("turbulence_formulation".to_string(), serde_json::json!("fixed_sync_unknown"));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err = ValidationCaseManifest::parse(&text, Path::new("badform.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("adaptive_w_sigw") && rendered.contains("fixed_sync_w"),
            "error must enumerate the supported variants: {rendered}"
        );
    }

    #[test]
    fn missing_turbulence_formulation_is_rejected() {
        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .remove("turbulence_formulation");
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err = ValidationCaseManifest::parse(&text, Path::new("noform.json")).expect_err("fails");
        assert!(
            err.to_string().contains("turbulence_formulation"),
            "error must name the missing field: {err}"
        );
    }

    #[test]
    fn both_spellings_of_oracle_override_rejected_as_ambiguous() {
        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .insert("LTURBULENCE".to_string(), serde_json::json!(1));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("both.json")).expect_err("fails");
        assert!(matches!(
            err,
            ValidationCaseError::AmbiguousField {
                field: "oracle_command_overrides",
                ..
            }
        ));
        let rendered = err.to_string();
        assert!(
            rendered.contains("LTURBULENCE") && rendered.contains("lturbulence"),
            "error must name both spellings: {rendered}"
        );
    }

    #[test]
    fn legacy_only_uppercase_oracle_override_is_rejected() {
        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .remove("lturbulence");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .insert("LTURBULENCE".to_string(), serde_json::json!(1));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("legacy.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("LTURBULENCE"),
            "legacy spelling must fail naming the key: {rendered}"
        );
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
        assert_eq!(
            candidate.derivation,
            CandidatePhiloxDerivation::ReuseBaseIdentityV1
        );
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
    fn unknown_top_level_field_is_rejected() {
        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw.as_object_mut()
            .expect("manifest object")
            .insert("bogus_extension".to_string(), serde_json::json!(1));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("bogus.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("bogus_extension"),
            "error must name the unknown field: {rendered}"
        );
    }

    #[test]
    fn unknown_nested_field_is_rejected() {
        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw["domain"]
            .as_object_mut()
            .expect("domain object")
            .insert("nx_typo".to_string(), serde_json::json!(32));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("nested.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("nx_typo"),
            "error must name the unknown nested field: {rendered}"
        );
    }

    #[test]
    fn generic_shared_seed_field_is_rejected() {
        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw["stochastic"]
            .as_object_mut()
            .expect("stochastic object")
            .insert("seed".to_string(), serde_json::json!(7));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("seed.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("seed"),
            "generic shared seed must fail with a useful error: {rendered}"
        );
    }

    #[test]
    fn legacy_top_level_seeds_field_is_rejected() {
        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw.as_object_mut()
            .expect("manifest object")
            .insert(
                "seeds".to_string(),
                serde_json::json!({"base_philox_key": [1, 2]}),
            );
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("seeds.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("seeds"),
            "legacy seeds block must fail with a useful error: {rendered}"
        );
    }

    #[test]
    fn typo_in_physics_fields_is_rejected_with_useful_error() {
        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw["release"]
            .as_object_mut()
            .expect("release object")
            .insert("particle_cout".to_string(), serde_json::json!(10));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("typo.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("particle_cout"),
            "error must name the typo: {rendered}"
        );

        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .insert("lturbulance".to_string(), serde_json::json!(1));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("typo2.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("lturbulance"),
            "error must name the typo: {rendered}"
        );
    }

    #[test]
    fn typo_inside_wind_variant_is_rejected() {
        let mut raw: serde_json::Value = serde_json::to_value(make_minimal_manifest())
            .expect("serialize minimal");
        raw["wind"]
            .as_object_mut()
            .expect("wind object")
            .insert("u_m_ss".to_string(), serde_json::json!(5.0));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("windtypo.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("u_m_ss"),
            "error must name the wind typo: {rendered}"
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

    /// Recursively assert the serialized manifest preserves every source key.
    ///
    /// Explicit JSON `null` in the source is equivalent to an absent key in
    /// the output (`skip_serializing_if` on `Option` fields); anything else
    /// must match exactly so silently discarded fields fail loudly.
    /// Fields with defaults that are serialized even when absent from source
    /// (e.g. `require_source_containment` defaulting to true) are exempted.
    fn assert_source_keys_preserved(source: &serde_json::Value, output: &serde_json::Value, path: &str) {
        match (source, output) {
            (serde_json::Value::Object(source_map), serde_json::Value::Object(output_map)) => {
                for (key, source_value) in source_map {
                    let child = format!("{path}.{key}");
                    if source_value.is_null() && !output_map.contains_key(key) {
                        continue;
                    }
                    let output_value = output_map.get(key).unwrap_or_else(|| {
                        panic!("source field {child} lost during parse/serialize")
                    });
                    assert_source_keys_preserved(source_value, output_value, &child);
                }
                for key in output_map.keys() {
                    if key == "require_source_containment" {
                        // This field has a default (true) that serializes even
                        // when absent from source; exempt from the round-trip check.
                        continue;
                    }
                    assert!(
                        source_map.contains_key(key),
                        "serialized field {path}.{key} has no source counterpart"
                    );
                }
            }
            (serde_json::Value::Array(source_items), serde_json::Value::Array(output_items)) => {
                assert_eq!(
                    source_items.len(),
                    output_items.len(),
                    "array length changed at {path}"
                );
                for (index, (source_item, output_item)) in
                    source_items.iter().zip(output_items.iter()).enumerate()
                {
                    assert_source_keys_preserved(
                        source_item,
                        output_item,
                        &format!("{path}[{index}]"),
                    );
                }
            }
            (serde_json::Value::Number(source_num), serde_json::Value::Number(output_num)) => {
                let source_f = source_num.as_f64().expect("numeric source");
                let output_f = output_num.as_f64().expect("numeric output");
                let tolerance = 1e-9 * source_f.abs().max(1.0);
                assert!(
                    (source_f - output_f).abs() <= tolerance,
                    "numeric value changed at {path}: {source_f} vs {output_f}"
                );
            }
            _ => assert_eq!(source, output, "value changed at {path}"),
        }
    }

    #[test]
    fn source_documents_survive_parse_and_serialize_without_loss() {
        for case_id in ALL_CHECKED_IN_CASES {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("corpus")
                .join("cases")
                .join(format!("{case_id}.json"));
            let text = std::fs::read_to_string(&path).expect("read source");
            let source: serde_json::Value =
                serde_json::from_str(&text).expect("source parses");
            let manifest = ValidationCaseManifest::load_from_file(&path)
                .unwrap_or_else(|e| panic!("load {case_id}: {e}"));
            let serialized = serde_json::to_string_pretty(&manifest).expect("serialize");
            let output: serde_json::Value =
                serde_json::from_str(&serialized).expect("output parses");
            assert_source_keys_preserved(&source, &output, case_id);
        }
    }

    fn minimal_manifest_json() -> serde_json::Value {
        serde_json::to_value(make_minimal_manifest()).expect("serialize minimal")
    }

    fn parse_json_value(value: &serde_json::Value) -> Result<ValidationCaseManifest, ValidationCaseError> {
        let text = serde_json::to_string(value).expect("re-serialize");
        ValidationCaseManifest::parse(&text, Path::new("test.json"))
    }

    #[test]
    fn oracle_override_schema_and_rust_required_fields_are_in_parity() {
        let schema = load_validation_case_schema();
        for field in [
            "turbulence_formulation",
            "lturbulence",
            "lconvection",
            "ctl",
            "ifine",
            "lsynctime_s",
        ] {
            let mut raw = minimal_manifest_json();
            raw["oracle_command_overrides"]
                .as_object_mut()
                .expect("oracle overrides object")
                .remove(field);

            let schema_error =
                validate_json_schema_subset(&schema, &schema, &raw, "$")
                    .expect_err("JSON Schema must reject missing oracle field");
            assert!(
                schema_error.contains(field),
                "schema error must name missing {field}: {schema_error}"
            );

            let rust_error = parse_json_value(&raw)
                .expect_err("Rust contract must reject the same missing oracle field");
            assert!(
                rust_error.to_string().contains(field),
                "Rust error must name missing {field}: {rust_error}"
            );
        }

        let mut missing_block = minimal_manifest_json();
        missing_block
            .as_object_mut()
            .expect("manifest object")
            .remove("oracle_command_overrides");
        assert!(
            validate_json_schema_subset(&schema, &schema, &missing_block, "$").is_err(),
            "JSON Schema must reject a missing oracle override block"
        );
        let err = parse_json_value(&missing_block)
            .expect_err("Rust deserialization must reject a missing oracle override block");
        assert!(
            err.to_string().contains("oracle_command_overrides"),
            "Rust error must name missing oracle block: {err}"
        );
    }

    #[test]
    fn oracle_override_schema_and_rust_numeric_semantics_are_in_parity() {
        let schema = load_validation_case_schema();

        let mut too_large_ifine = minimal_manifest_json();
        too_large_ifine["oracle_command_overrides"]["ifine"] = serde_json::json!(11);
        assert!(
            validate_json_schema_subset(&schema, &schema, &too_large_ifine, "$").is_err(),
            "JSON Schema must enforce IFINE <= 10"
        );
        assert!(
            parse_json_value(&too_large_ifine).is_err(),
            "Rust contract must enforce IFINE <= 10"
        );

        let mut adaptive_negative = minimal_manifest_json();
        adaptive_negative["oracle_command_overrides"]["ctl"] = serde_json::json!(-5.0);
        assert!(
            validate_json_schema_subset(&schema, &schema, &adaptive_negative, "$").is_err(),
            "adaptive_w_sigw with negative CTL must fail JSON Schema"
        );
        assert!(
            parse_json_value(&adaptive_negative).is_err(),
            "adaptive_w_sigw with negative CTL must fail Rust"
        );

        let mut fixed_positive = minimal_manifest_json();
        fixed_positive["oracle_command_overrides"]["turbulence_formulation"] =
            serde_json::json!("fixed_sync_w");
        fixed_positive["oracle_command_overrides"]["ctl"] = serde_json::json!(5.0);
        assert!(
            validate_json_schema_subset(&schema, &schema, &fixed_positive, "$").is_err(),
            "fixed_sync_w with positive CTL must fail JSON Schema"
        );
        assert!(
            parse_json_value(&fixed_positive).is_err(),
            "fixed_sync_w with positive CTL must fail Rust"
        );

        let mut fixed_valid = minimal_manifest_json();
        fixed_valid["oracle_command_overrides"]["turbulence_formulation"] =
            serde_json::json!("fixed_sync_w");
        fixed_valid["oracle_command_overrides"]["ctl"] = serde_json::json!(-5.0);
        validate_json_schema_subset(&schema, &schema, &fixed_valid, "$")
            .expect("JSON Schema accepts explicit fixed_sync_w semantics");
        parse_json_value(&fixed_valid)
            .expect("Rust accepts the same explicit fixed_sync_w semantics");
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
    fn inconsistent_total_and_per_particle_mass_is_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.release.inventory.quantity_kg = 1.0;
        manifest.release.particle_count = 1000;
        manifest.release.mass_kg_per_particle = Some(0.5);
        let err = manifest.validate().expect_err("inconsistent mass fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::AmbiguousField { field: "release.mass_kg_per_particle", .. }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn invalid_coordinates_spacing_and_placement_are_rejected() {
        let mut manifest = make_minimal_manifest();
        // Longitude out of range.
        if let SourceGeometry::Point { ref mut lon_deg, .. } = manifest.release.geometry {
            *lon_deg = 500.0;
        }
        assert!(manifest.validate().is_err());
        let mut manifest = make_minimal_manifest();
        if let SourceGeometry::Point { ref mut lat_deg, .. } = manifest.release.geometry {
            *lat_deg = f32::INFINITY;
        }
        assert!(manifest.validate().is_err());
        // Non-positive grid spacing.
        let mut manifest = make_minimal_manifest();
        manifest.domain.dx_deg = 0.0;
        assert!(manifest.validate().is_err());
        // Out-of-domain release with containment required.
        let mut manifest = make_minimal_manifest();
        if let SourceGeometry::Point { ref mut lon_deg, .. } = manifest.release.geometry {
            *lon_deg = 100.0;
        }
        let err = manifest.validate().expect_err("out-of-domain fails");
        assert!(
            matches!(err, ValidationCaseError::AmbiguousField { field: "release.geometry", .. }),
            "unexpected: {err}"
        );
        // Same placement validates when containment is waived.
        let mut manifest = make_minimal_manifest();
        if let SourceGeometry::Point { ref mut lon_deg, .. } = manifest.release.geometry {
            *lon_deg = 100.0;
        }
        manifest.require_source_containment = false;
        manifest.validate().expect("waived containment passes");
    }

    #[test]
    fn adv_and_wind_retain_their_original_source_meaning() {
        let adv = load_checked_in_case("ADV-ANA-001");
        match &adv.release.geometry {
            SourceGeometry::Point { lon_deg, lat_deg, z_m } => {
                assert_eq!((*lon_deg, *lat_deg, *z_m), (10.0, 50.0, 100.0));
            }
            other => panic!("ADV geometry changed: {other:?}"),
        }
        assert_eq!(adv.release.vertical_ref, VerticalRef::Agl);
        assert_eq!(
            adv.release.timing,
            ReleaseTiming::Instant { at: "20240101000000".to_string() }
        );
        assert_eq!(adv.release.species.id, "SPECIES_024");
        assert_eq!(adv.release.inventory.quantity_kg, 1024.0);
        assert_eq!(adv.release.inventory.unit, MassUnit::Kg);
        assert_eq!(adv.release.particle_count, 1024);
        assert_eq!(adv.domain.horizontal_ref, HorizontalCoordRef::GeographicLonLatDegrees);
        assert_eq!(adv.domain.wind_heights_ref, VerticalRef::Agl);

        let wind = load_checked_in_case("WIND-UNI-002");
        match &wind.release.geometry {
            SourceGeometry::Point { lon_deg, lat_deg, z_m } => {
                assert_eq!((*lon_deg, *lat_deg, *z_m), (10.0, 10.0, 50.0));
            }
            other => panic!("WIND geometry changed: {other:?}"),
        }
        assert_eq!(wind.release.vertical_ref, VerticalRef::Agl);
        assert_eq!(
            wind.release.timing,
            ReleaseTiming::Instant { at: "20240101000000".to_string() }
        );
        assert_eq!(wind.release.species.id, "SPECIES_024");
        assert_eq!(wind.release.inventory.quantity_kg, 1.0);
        assert_eq!(wind.release.particle_count, 1000);
    }

    #[test]
    fn active_deposition_requires_explicit_forcing_block() {
        let mut manifest = make_minimal_manifest();
        manifest.physics_switches.dry_deposition = true;
        manifest.release.species.id = "SPECIES_040".to_string();
        manifest.release.species.physics_contract = SpeciesPhysicsContractRef {
            profile: SpeciesPhysicsProfile::Species040DryConstantV1,
            id: SPECIES_040_DRY_CONTRACT_ID.to_string(),
            version: 1,
            path: SPECIES_040_DRY_CONTRACT_PATH.to_string(),
            git_blob_sha: SPECIES_040_DRY_CONTRACT_BLOB.to_string(),
        };
        manifest.oracle_command_overrides.ldrydep = Some(1);
        manifest.deposition = None;
        let err = manifest.validate().expect_err("active dry deposition needs forcing");
        assert!(matches!(
            err,
            ValidationCaseError::MissingField { field: "deposition" }
        ));
    }

    #[test]
    fn species_physics_contract_is_exact_and_controls_switches() {
        let mut manifest = make_minimal_manifest();
        manifest.release.species.physics_contract.git_blob_sha = "deadbeef".to_string();
        let err = manifest.validate().expect_err("wrong contract hash must fail");
        assert!(err.to_string().contains("canonical"));

        let mut manifest = make_minimal_manifest();
        manifest.physics_switches.decay = true;
        manifest.oracle_command_overrides.ldecay = Some(1);
        let err = manifest
            .validate()
            .expect_err("decay requires a dedicated species physics profile");
        assert!(err.to_string().contains("species physics profile"));
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
                "candidate_transformation": {"description": "d", "script": "s", "version": "v"},
                "oracle_transformation": {"description": "d", "script": "s", "version": "v"}
            }
        });
        // Should pass
        parse_json_value(&raw).expect("complete meteorology passes");

        // Missing dataset_id
        let mut raw2 = minimal_manifest_json();
        raw2["wind"] = serde_json::json!({
            "profile": "real_weather",
            "meteorology": {
                "version": "1",
                "source_path": "path",
                "digest": "manifest:path",
                "temporal_coverage": ["20240101000000", "20240101010000"],
                "horizontal_coord": "test",
                "vertical_coord": "test",
                "required_fields": ["u"],
                "candidate_transformation": {"description": "d", "script": "s", "version": "v"},
                "oracle_transformation": {"description": "d", "script": "s", "version": "v"}
            }
        });
        let err = parse_json_value(&raw2).expect_err("missing dataset_id fails");
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
        assert!(err.to_string().contains("candidate_transformation.description"));
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
        adv.validate().expect("ADV-ANA-001 validates without meteorology");

        let wind = load_checked_in_case("WIND-UNI-002");
        match &wind.wind {
            WindSpec::Uniform { .. } => {}
            other => panic!("WIND-UNI-002 wind changed: {other:?}"),
        }
        wind.validate().expect("WIND-UNI-002 validates without meteorology");
    }

    #[test]
    fn etex_mini_013_meteorology_complete() {
        let etex = load_checked_in_case("ETEX-MINI-013");
        match &etex.wind {
            WindSpec::RealWeather { meteorology } => {
                assert_eq!(meteorology.dataset_id, "era5-native-mini-19941023-24");
                assert_eq!(meteorology.version, "2024-09-19");
                assert_eq!(meteorology.source_path, "fixtures/etex/native-mini/");
                assert_eq!(meteorology.digest, "manifest:fixtures/etex/native-mini/DIGESTS.json");
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
        etex.validate().expect("ETEX-MINI-013 validates with complete meteorology");
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