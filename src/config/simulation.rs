//! Aggregate file loading and ordered cross-domain configuration validation.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::particles::MAX_SPECIES;

use super::command::CommandConfig;
use super::error::ConfigError;
use super::output::OutputGridConfig;
use super::parsing::validate_config_version;
use super::release::ReleaseConfig;
use super::species::SpeciesConfig;

/// Complete run inputs with ordered cross-domain validation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimulationConfig {
    /// Version of the project-owned serialized configuration. `None` denotes legacy input.
    #[serde(default)]
    pub version: Option<u32>,
    pub command: CommandConfig,
    pub releases: Vec<ReleaseConfig>,
    pub outgrid: OutputGridConfig,
    pub species: Vec<SpeciesConfig>,
}

impl SimulationConfig {
    /// Load the four required FLEXPART input surfaces before validating the run.
    pub fn load(base_path: &Path) -> Result<Self, ConfigError> {
        let command = CommandConfig::from_file(&base_path.join("COMMAND"))?;
        let releases = ReleaseConfig::load_many_from_file(&base_path.join("RELEASES"))?;
        let outgrid = OutputGridConfig::from_file(&base_path.join("OUTGRID"))?;
        let species = SpeciesConfig::load_dir(&base_path.join("SPECIES"))?;

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

    /// Reject unsupported versions and inconsistent run, release and species inputs.
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_config_version(self.version, "simulation")?;
        self.command.validate()?;
        self.outgrid.validate()?;

        if self.species.len() > MAX_SPECIES {
            return Err(ConfigError::Validation {
                message: format!(
                    "too many species: found {}, maximum supported is {MAX_SPECIES}",
                    self.species.len()
                ),
            });
        }

        if self.releases.is_empty() {
            return Err(ConfigError::Validation {
                message: "at least one release is required".to_string(),
            });
        }

        for release in &self.releases {
            release.validate()?;
        }
        for specie in &self.species {
            specie.validate()?;
        }

        let species_count = self.species.len();
        for release in &self.releases {
            if let Some(masses) = &release.species_masses_kg {
                if masses.len() > species_count {
                    return Err(ConfigError::Validation {
                        message: format!(
                            "release `{}` species_masses_kg has {} entries but simulation has only {} configured species",
                            release.name,
                            masses.len(),
                            species_count
                        ),
                    });
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::parsing::{ConfigMap, CURRENT_CONFIG_VERSION};
    use super::super::test_support::temp_dir;
    use super::*;
    use std::fs;

    #[test]
    fn validate_rejects_release_with_more_masses_than_species() {
        let config = SimulationConfig {
            version: Some(CURRENT_CONFIG_VERSION),
            command: CommandConfig {
                start_time: "20240101120000".to_string(),
                end_time: "20240101180000".to_string(),
                output_interval_seconds: Some(3600),
                sync_interval_seconds: Some(900),
                raw: ConfigMap::new(),
            },
            releases: vec![ReleaseConfig {
                name: "r1".to_string(),
                start_time: "20240101130000".to_string(),
                end_time: "20240101150000".to_string(),
                lon: 7.5,
                lat: 46.2,
                z_min: 0.0,
                z_max: 100.0,
                mass_kg: 1.0,
                particle_count: 1000,
                species_masses_kg: Some(vec![1.0, 0.5]),
                raw: ConfigMap::new(),
            }],
            outgrid: OutputGridConfig {
                lon_min: 5.0,
                lon_max: 10.0,
                lat_min: 44.0,
                lat_max: 48.0,
                nx: 100,
                ny: 80,
                nz: 10,
                dx: 0.05,
                dy: 0.05,
                dz: 100.0,
                raw: ConfigMap::new(),
            },
            species: vec![SpeciesConfig {
                name: "SO2".to_string(),
                version: Some(CURRENT_CONFIG_VERSION),
                molecular_weight: Some(64.066),
                dry_deposition_velocity: Some(0.01),
                decay_constant: Some(0.0),
                half_life_s: None,
                wet_a_gas: None,
                wet_b_gas: None,
                crain_aero: None,
                csnow_aero: None,
                ccn_aero: None,
                in_aero: None,
                relative_diffusivity: None,
                henry: None,
                surface_reactivity_f0: None,
                particle_density_kg_m3: None,
                mean_diameter_um: None,
                diameter_sigma: None,
                source_file: Some("SO2.spec".to_string()),
                raw: ConfigMap::new(),
            }],
        };

        let err = config.validate().expect_err("excess masses must fail");
        assert!(
            err.to_string()
                .contains("release `r1` species_masses_kg has 2 entries but simulation has only 1 configured species")
        );
    }

    #[test]
    fn validate_accepts_equal_length_masses_and_species() {
        let config = SimulationConfig {
            version: Some(CURRENT_CONFIG_VERSION),
            command: CommandConfig {
                start_time: "20240101120000".to_string(),
                end_time: "20240101180000".to_string(),
                output_interval_seconds: Some(3600),
                sync_interval_seconds: Some(900),
                raw: ConfigMap::new(),
            },
            releases: vec![ReleaseConfig {
                name: "r1".to_string(),
                start_time: "20240101130000".to_string(),
                end_time: "20240101150000".to_string(),
                lon: 7.5,
                lat: 46.2,
                z_min: 0.0,
                z_max: 100.0,
                mass_kg: 1.0,
                particle_count: 1000,
                species_masses_kg: Some(vec![1.0, 0.5]),
                raw: ConfigMap::new(),
            }],
            outgrid: OutputGridConfig {
                lon_min: 5.0,
                lon_max: 10.0,
                lat_min: 44.0,
                lat_max: 48.0,
                nx: 100,
                ny: 80,
                nz: 10,
                dx: 0.05,
                dy: 0.05,
                dz: 100.0,
                raw: ConfigMap::new(),
            },
            species: vec![
                SpeciesConfig {
                    name: "SO2".to_string(),
                    version: Some(CURRENT_CONFIG_VERSION),
                    molecular_weight: Some(64.066),
                    dry_deposition_velocity: Some(0.01),
                    decay_constant: Some(0.0),
                    half_life_s: None,
                    wet_a_gas: None,
                    wet_b_gas: None,
                    crain_aero: None,
                    csnow_aero: None,
                    ccn_aero: None,
                    in_aero: None,
                    relative_diffusivity: None,
                    henry: None,
                    surface_reactivity_f0: None,
                    particle_density_kg_m3: None,
                    mean_diameter_um: None,
                    diameter_sigma: None,
                    source_file: Some("SO2.spec".to_string()),
                    raw: ConfigMap::new(),
                },
                SpeciesConfig {
                    name: "Xe133".to_string(),
                    version: Some(CURRENT_CONFIG_VERSION),
                    molecular_weight: Some(133.0),
                    dry_deposition_velocity: None,
                    decay_constant: Some(0.0),
                    half_life_s: Some(453168.0),
                    wet_a_gas: None,
                    wet_b_gas: None,
                    crain_aero: None,
                    csnow_aero: None,
                    ccn_aero: None,
                    in_aero: None,
                    relative_diffusivity: None,
                    henry: None,
                    surface_reactivity_f0: None,
                    particle_density_kg_m3: None,
                    mean_diameter_um: None,
                    diameter_sigma: None,
                    source_file: Some("Xe133.spec".to_string()),
                    raw: ConfigMap::new(),
                },
            ],
        };

        config.validate().expect("equal lengths must pass");
    }

    #[test]
    fn validate_accepts_shorter_masses_list() {
        let config = SimulationConfig {
            version: Some(CURRENT_CONFIG_VERSION),
            command: CommandConfig {
                start_time: "20240101120000".to_string(),
                end_time: "20240101180000".to_string(),
                output_interval_seconds: Some(3600),
                sync_interval_seconds: Some(900),
                raw: ConfigMap::new(),
            },
            releases: vec![ReleaseConfig {
                name: "r1".to_string(),
                start_time: "20240101130000".to_string(),
                end_time: "20240101150000".to_string(),
                lon: 7.5,
                lat: 46.2,
                z_min: 0.0,
                z_max: 100.0,
                mass_kg: 1.0,
                particle_count: 1000,
                species_masses_kg: Some(vec![1.0]),
                raw: ConfigMap::new(),
            }],
            outgrid: OutputGridConfig {
                lon_min: 5.0,
                lon_max: 10.0,
                lat_min: 44.0,
                lat_max: 48.0,
                nx: 100,
                ny: 80,
                nz: 10,
                dx: 0.05,
                dy: 0.05,
                dz: 100.0,
                raw: ConfigMap::new(),
            },
            species: vec![
                SpeciesConfig {
                    name: "SO2".to_string(),
                    version: Some(CURRENT_CONFIG_VERSION),
                    molecular_weight: Some(64.066),
                    dry_deposition_velocity: Some(0.01),
                    decay_constant: Some(0.0),
                    half_life_s: None,
                    wet_a_gas: None,
                    wet_b_gas: None,
                    crain_aero: None,
                    csnow_aero: None,
                    ccn_aero: None,
                    in_aero: None,
                    relative_diffusivity: None,
                    henry: None,
                    surface_reactivity_f0: None,
                    particle_density_kg_m3: None,
                    mean_diameter_um: None,
                    diameter_sigma: None,
                    source_file: Some("SO2.spec".to_string()),
                    raw: ConfigMap::new(),
                },
                SpeciesConfig {
                    name: "Xe133".to_string(),
                    version: Some(CURRENT_CONFIG_VERSION),
                    molecular_weight: Some(133.0),
                    dry_deposition_velocity: None,
                    decay_constant: Some(0.0),
                    half_life_s: Some(453168.0),
                    wet_a_gas: None,
                    wet_b_gas: None,
                    crain_aero: None,
                    csnow_aero: None,
                    ccn_aero: None,
                    in_aero: None,
                    relative_diffusivity: None,
                    henry: None,
                    surface_reactivity_f0: None,
                    particle_density_kg_m3: None,
                    mean_diameter_um: None,
                    diameter_sigma: None,
                    source_file: Some("Xe133.spec".to_string()),
                    raw: ConfigMap::new(),
                },
            ],
        };

        config.validate().expect("shorter masses list must pass");
    }

    #[test]
    fn validate_accepts_legacy_absent_masses() {
        let config = SimulationConfig {
            version: Some(CURRENT_CONFIG_VERSION),
            command: CommandConfig {
                start_time: "20240101120000".to_string(),
                end_time: "20240101180000".to_string(),
                output_interval_seconds: Some(3600),
                sync_interval_seconds: Some(900),
                raw: ConfigMap::new(),
            },
            releases: vec![ReleaseConfig {
                name: "r1".to_string(),
                start_time: "20240101130000".to_string(),
                end_time: "20240101150000".to_string(),
                lon: 7.5,
                lat: 46.2,
                z_min: 0.0,
                z_max: 100.0,
                mass_kg: 1.0,
                particle_count: 1000,
                species_masses_kg: None,
                raw: ConfigMap::new(),
            }],
            outgrid: OutputGridConfig {
                lon_min: 5.0,
                lon_max: 10.0,
                lat_min: 44.0,
                lat_max: 48.0,
                nx: 100,
                ny: 80,
                nz: 10,
                dx: 0.05,
                dy: 0.05,
                dz: 100.0,
                raw: ConfigMap::new(),
            },
            species: vec![
                SpeciesConfig {
                    name: "SO2".to_string(),
                    version: Some(CURRENT_CONFIG_VERSION),
                    molecular_weight: Some(64.066),
                    dry_deposition_velocity: Some(0.01),
                    decay_constant: Some(0.0),
                    half_life_s: None,
                    wet_a_gas: None,
                    wet_b_gas: None,
                    crain_aero: None,
                    csnow_aero: None,
                    ccn_aero: None,
                    in_aero: None,
                    relative_diffusivity: None,
                    henry: None,
                    surface_reactivity_f0: None,
                    particle_density_kg_m3: None,
                    mean_diameter_um: None,
                    diameter_sigma: None,
                    source_file: Some("SO2.spec".to_string()),
                    raw: ConfigMap::new(),
                },
                SpeciesConfig {
                    name: "Xe133".to_string(),
                    version: Some(CURRENT_CONFIG_VERSION),
                    molecular_weight: Some(133.0),
                    dry_deposition_velocity: None,
                    decay_constant: Some(0.0),
                    half_life_s: Some(453168.0),
                    wet_a_gas: None,
                    wet_b_gas: None,
                    crain_aero: None,
                    csnow_aero: None,
                    ccn_aero: None,
                    in_aero: None,
                    relative_diffusivity: None,
                    henry: None,
                    surface_reactivity_f0: None,
                    particle_density_kg_m3: None,
                    mean_diameter_um: None,
                    diameter_sigma: None,
                    source_file: Some("Xe133.spec".to_string()),
                    raw: ConfigMap::new(),
                },
            ],
        };

        config.validate().expect("absent masses must pass");
    }

    #[test]
    fn serde_round_trip_simulation_config() {
        let config = SimulationConfig {
            version: Some(CURRENT_CONFIG_VERSION),
            command: CommandConfig {
                start_time: "20240101120000".to_string(),
                end_time: "20240101180000".to_string(),
                output_interval_seconds: Some(3600),
                sync_interval_seconds: Some(900),
                raw: ConfigMap::new(),
            },
            releases: vec![ReleaseConfig {
                name: "r1".to_string(),
                start_time: "20240101130000".to_string(),
                end_time: "20240101150000".to_string(),
                lon: 7.5,
                lat: 46.2,
                z_min: 0.0,
                z_max: 100.0,
                mass_kg: 1.0,
                particle_count: 1000,
                species_masses_kg: None,
                raw: ConfigMap::new(),
            }],
            outgrid: OutputGridConfig {
                lon_min: 5.0,
                lon_max: 10.0,
                lat_min: 44.0,
                lat_max: 48.0,
                nx: 100,
                ny: 80,
                nz: 10,
                dx: 0.05,
                dy: 0.05,
                dz: 100.0,
                raw: ConfigMap::new(),
            },
            species: vec![SpeciesConfig {
                name: "SO2".to_string(),
                version: Some(CURRENT_CONFIG_VERSION),
                molecular_weight: Some(64.066),
                dry_deposition_velocity: Some(0.01),
                decay_constant: Some(0.0),
                half_life_s: None,
                wet_a_gas: None,
                wet_b_gas: None,
                crain_aero: None,
                csnow_aero: None,
                ccn_aero: None,
                in_aero: None,
                relative_diffusivity: None,
                henry: None,
                surface_reactivity_f0: None,
                particle_density_kg_m3: None,
                mean_diameter_um: None,
                diameter_sigma: None,
                source_file: Some("SO2.spec".to_string()),
                raw: ConfigMap::new(),
            }],
        };

        let json = serde_json::to_string(&config).expect("serialize");
        let back: SimulationConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config, back);
        assert_eq!(back.version, Some(CURRENT_CONFIG_VERSION));
        assert_eq!(back.species[0].version, Some(CURRENT_CONFIG_VERSION));

        for invalid in [0, CURRENT_CONFIG_VERSION + 1] {
            let mut unsupported = back.clone();
            unsupported.version = Some(invalid);
            assert!(unsupported.validate().is_err());
            unsupported.version = Some(CURRENT_CONFIG_VERSION);
            unsupported.species[0].version = Some(invalid);
            assert!(unsupported.validate().is_err());
        }
    }

    #[test]
    fn load_from_base_path_reads_all_files() {
        let base = temp_dir("io06");
        fs::write(
            base.join("COMMAND"),
            "&COMMAND\nIBDATE=20240101,IBTIME=000000,IEDATE=20240101,IETIME=120000,LOUTSTEP=3600\n/\n",
        )
        .expect("write COMMAND");
        fs::write(
            base.join("RELEASES"),
            "&RELEASE\nNAME='src',START='20240101010000',END='20240101020000',LON=7.0,LAT=46.0,Z1=0,Z2=50,MASS=1,PARTICLES=100\n/\n",
        )
        .expect("write RELEASES");
        fs::write(
            base.join("OUTGRID"),
            "&OUTGRID\nLON_MIN=5,LON_MAX=15,LAT_MIN=40,LAT_MAX=50,NX=100,NY=100,NZ=5,DX=0.1,DY=0.1,DZ=100\n/\n",
        )
        .expect("write OUTGRID");
        let species_dir = base.join("SPECIES");
        fs::create_dir_all(&species_dir).expect("create SPECIES dir");
        fs::write(
            species_dir.join("SO2.spec"),
            "NAME=SO2\nMOLECULAR_WEIGHT=64.066\n",
        )
        .expect("write species");

        let loaded = SimulationConfig::load(&base).expect("load simulation config");
        assert_eq!(loaded.releases.len(), 1);
        assert_eq!(loaded.species.len(), 1);
        assert_eq!(loaded.species[0].name, "SO2");

        fs::remove_dir_all(&base).expect("cleanup temp directory");
    }
}
