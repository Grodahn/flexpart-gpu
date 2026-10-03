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

//! Responsibility-specific implementations are private; this facade preserves
//! the established public case contract. See the validation navigation map.

mod document;
mod handoff;
mod manifest;
mod meteorology;
mod oracle;
mod output;
mod release;
mod stochastic;

#[cfg(test)]
mod test_support;

pub use handoff::{
    ArtifactClass, ArtifactProducer, CandidatePhysicsProfileRef, ExecutionProfileRef,
    ExpectedArtifactRequirement, ExpectedArtifacts, InputEquivalenceLimitation,
    OracleExecutionPolicy, RepresentationDifferences, ValidationDefinitionRef,
    ValidationDefinitionRefs, CANDIDATE_PHYSICS_PROFILE_ID, CANDIDATE_PHYSICS_PROFILE_PATH,
    CANDIDATE_PHYSICS_PROFILE_VERSION, ORACLE_EXECUTION_PROFILE_ID, ORACLE_EXECUTION_PROFILE_PATH,
    ORACLE_EXECUTION_PROFILE_VERSION,
};
pub use manifest::{
    DepositionSpec, DomainSpec, HorizontalCoordRef, IntegrationSpec, PhysicsSwitches, UnitsSpec,
    ValidationCaseError, ValidationCaseManifest, VerticalRef, VALIDATION_CASE_SCHEMA_PATH,
    VALIDATION_CASE_SCHEMA_VERSION,
};
pub use meteorology::{
    CandidateMetTransformation, MeteorologySpec, OracleMetTransformation,
    OracleMeteorologyProfileRef, SurfaceSpec, WindSpec, REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_ID,
    REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_PATH, REAL_WEATHER_ORACLE_METEOROLOGY_PROFILE_VERSION,
    SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_ID, SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_PATH,
    SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_VERSION,
};
pub use oracle::{
    OracleCommandOverrides, OracleTurbulenceFormulation, CTL_W_SIGW_FORMULATION_THRESHOLD,
};
pub use output::{OutputGridSpec, OutputQuantity, OutputSpec, SimulationDirection};
pub use release::{
    MassUnit, ReleaseInventory, ReleaseSpec, ReleaseTiming, SourceGeometry,
    SpeciesPhysicsContractRef, SpeciesPhysicsProfile, SpeciesRef, MASS_CONSISTENCY_TOLERANCE_REL,
    SPECIES_024_INERT_CONTRACT_BLOB, SPECIES_024_INERT_CONTRACT_ID,
    SPECIES_024_INERT_CONTRACT_PATH, SPECIES_040_DRY_CONTRACT_BLOB, SPECIES_040_DRY_CONTRACT_ID,
    SPECIES_040_DRY_CONTRACT_PATH, SPECIES_040_WET_CONTRACT_BLOB, SPECIES_040_WET_CONTRACT_ID,
    SPECIES_040_WET_CONTRACT_PATH,
};
pub use stochastic::{
    CandidatePhiloxDerivation, CandidatePhiloxIdentity, OracleKind, OracleSeedIdentity,
    OracleSeedMode, OracleStrategyRef, StochasticIdentitySpec, ORACLE_STOCHASTIC_CONTRACT_PATH,
    ORACLE_STOCHASTIC_STRATEGY_ID, ORACLE_STOCHASTIC_STRATEGY_VERSION,
};
