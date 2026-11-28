//! Aggregate file loading and ordered cross-domain configuration validation.

use serde::{Deserialize, Serialize};
use std::path::Path;

use super::parsing::validate_config_version;
use super::{CommandConfig, ConfigError, OutputGridConfig, ReleaseConfig, SpeciesConfig};
use crate::release::{validate_release_bindings, ReleaseSource, ReleaseSpec};

/// Complete run inputs with validated and species-bound release specifications.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimulationConfig {
    /// Version of the project-owned serialized configuration. `None` denotes legacy input.
    #[serde(default)]
    pub version: Option<u32>,
    pub command: CommandConfig,
    pub releases: Vec<ReleaseSpec>,
    pub outgrid: OutputGridConfig,
    pub species: Vec<SpeciesConfig>,
}

impl SimulationConfig {
    /// Load production inputs and bind the complete release set before returning.
    ///
    /// # Errors
    /// Returns a configuration error if a required input is absent or inconsistent.
    pub fn load(base_path: &Path) -> Result<Self, ConfigError> {
        let command = CommandConfig::from_file(&base_path.join("COMMAND"))?;
        let declarations = ReleaseConfig::load_many_from_file(&base_path.join("RELEASES"))?;
        let outgrid = OutputGridConfig::from_file(&base_path.join("OUTGRID"))?;
        let species = SpeciesConfig::load_dir(&base_path.join("SPECIES"))?;
        command.validate_forward_release_mass_mode(
            declarations
                .iter()
                .any(ReleaseConfig::uses_native_release_mode),
        )?;
        let releases = ReleaseConfig::normalize_forward_mass(&declarations, &species)?;
        let config = Self {
            version: None,
            command,
            releases,
            outgrid,
            species,
        };
        config.validate()?;
        Ok(config)
    }

    /// Recheck aggregate configuration, including release lanes against the actual species table.
    ///
    /// # Errors
    /// Returns a configuration error if the aggregate inputs or species bindings are invalid.
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_config_version(self.version, "simulation")?;
        self.command.validate()?;
        self.command
            .validate_forward_release_mass_mode(self.releases.iter().any(|release| {
                matches!(release.source, ReleaseSource::Flexpart11_1ReleasesV1 { .. })
            }))?;
        self.outgrid.validate()?;
        validate_release_bindings(&self.releases, &self.species)
    }
}
