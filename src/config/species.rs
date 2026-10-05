//! SPECIES identity, oracle units/sentinels and deposition/decay activation.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::particles::MAX_SPECIES;

use super::error::ConfigError;
use super::parsing::{
    extract_namelist_sections, get_first, parse_assignments, parse_optional_f64,
    parse_optional_string, parse_optional_u32, read_text_file, strip_comments,
    validate_config_version, ConfigMap,
};

/// Gas/aerosol branch selected by the configured positive mean diameter.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpeciesKind {
    Gas,
    Aerosol,
}

/// Species identity and optional deposition, scavenging, decay and size parameters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeciesConfig {
    pub name: String,
    /// Version of the project-owned species schema. `None` denotes legacy input.
    #[serde(default)]
    pub version: Option<u32>,
    pub molecular_weight: Option<f64>,
    pub dry_deposition_velocity: Option<f64>,
    pub decay_constant: Option<f64>,
    /// Half-life from `PDECAY` [s], if positive.
    ///
    /// Ported from `readoptions_mod.f90:2328` (`decay=0.693147/decay`).
    /// `None` means radioactive decay is disabled (sentinel or missing).
    pub half_life_s: Option<f64>,
    /// Below-cloud gas scavenging coefficient `A` (`PWETA_GAS`).
    pub wet_a_gas: Option<f64>,
    /// Below-cloud gas scavenging exponent `B` (`PWETB_GAS`).
    pub wet_b_gas: Option<f64>,
    /// Below-cloud rain efficiency (`PCRAIN_AERO`).
    pub crain_aero: Option<f64>,
    /// Below-cloud snow efficiency (`PCSNOW_AERO`).
    pub csnow_aero: Option<f64>,
    /// In-cloud CCN activation fraction (`PCCN_AERO`).
    pub ccn_aero: Option<f64>,
    /// In-cloud ice activation fraction (`PIN_AERO`).
    pub in_aero: Option<f64>,
    /// Relative diffusivity to water vapor (`PRELDIFF`).
    pub relative_diffusivity: Option<f64>,
    /// Henry coefficient (`PHENRY`).
    pub henry: Option<f64>,
    /// Surface reactivity (`PF0`). Missing maps to `None` (treated as `0` downstream).
    pub surface_reactivity_f0: Option<f64>,
    /// Particle density (`PDENSITY`) [kg/m^3].
    pub particle_density_kg_m3: Option<f64>,
    /// Mean particle diameter (`PDIA`, fallback `PDQUER`) converted to [um].
    ///
    /// Ported from `readoptions_mod.f90:2341` (`dquer=dquer*1000000`, m to um).
    /// `None` means gas (no aerosol diameter).
    pub mean_diameter_um: Option<f64>,
    /// Diameter distribution width (`PDSIGMA`). Aerosols require `> 1`.
    pub diameter_sigma: Option<f64>,
    pub source_file: Option<String>,
    pub raw: ConfigMap,
}

impl SpeciesConfig {
    /// Load species in sorted file order and enforce the supported mass-slot limit.
    pub fn load_dir(dir: &Path) -> Result<Vec<Self>, ConfigError> {
        if !dir.exists() {
            return Err(ConfigError::MissingPath {
                path: dir.to_path_buf(),
            });
        }
        if !dir.is_dir() {
            return Err(ConfigError::Parse {
                context: format!("SPECIES ({})", dir.display()),
                message: "expected SPECIES to be a directory".to_string(),
            });
        }

        let mut files = Vec::new();
        for entry in fs::read_dir(dir).map_err(|source| ConfigError::ReadFile {
            path: dir.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| ConfigError::ReadFile {
                path: dir.to_path_buf(),
                source,
            })?;
            if entry
                .file_type()
                .map_err(|source| ConfigError::ReadFile {
                    path: entry.path(),
                    source,
                })?
                .is_file()
            {
                files.push(entry.path());
            }
        }
        files.sort();

        let mut species = Vec::with_capacity(files.len());
        for path in files {
            species.push(Self::from_file(&path)?);
        }

        if species.is_empty() {
            return Err(ConfigError::Validation {
                message: "SPECIES directory is empty".to_string(),
            });
        }
        if species.len() > MAX_SPECIES {
            return Err(ConfigError::Validation {
                message: format!(
                    "too many species: found {}, maximum supported is {MAX_SPECIES}",
                    species.len()
                ),
            });
        }
        Ok(species)
    }

    fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let file_text = read_text_file(path)?;
        let context = format!("SPECIES ({})", path.display());
        // Oracle files use `&SPECIES_PARAMS ... /` (see `readspecies` in
        // `readoptions_mod.f90:2715`). Accept that first, then the legacy
        // `&SPECIES ... /` subset, then plain key/value content.
        let mut raw: Option<ConfigMap> = None;
        for section_name in ["SPECIES_PARAMS", "SPECIES"] {
            let sections =
                extract_namelist_sections(&file_text, section_name).map_err(|message| {
                    ConfigError::Parse {
                        context: context.clone(),
                        message,
                    }
                })?;
            if let Some(section) = sections.first() {
                raw = Some(
                    parse_assignments(section).map_err(|message| ConfigError::Parse {
                        context: context.clone(),
                        message,
                    })?,
                );
                break;
            }
        }
        let raw = match raw {
            Some(raw) => raw,
            None => parse_assignments(&strip_comments(&file_text)).map_err(|message| {
                ConfigError::Parse {
                    context: context.clone(),
                    message,
                }
            })?,
        };
        Self::from_map(raw, path, &context)
    }

    // Oracle `A`/`B` scavenging pairs intentionally share names (`wet_a_gas`
    // vs `wet_b_gas`, `crain` vs `csnow`); the similarity carries meaning.
    #[allow(clippy::similar_names)]
    fn from_map(raw: ConfigMap, path: &Path, context: &str) -> Result<Self, ConfigError> {
        let version = parse_optional_u32(&raw, context, &["version", "species_version"])?;
        let name_from_file = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .filter(|name| !name.trim().is_empty());
        let name = parse_optional_string(&raw, &["name", "species", "specname", "pspecies"])
            .or(name_from_file)
            .ok_or_else(|| ConfigError::MissingKey {
                context: context.to_string(),
                key: "name".to_string(),
            })?;
        let molecular_weight = parse_positive_or_none(
            &raw,
            context,
            &[
                "molecular_weight",
                "mol_weight",
                "weightmolar",
                "pweightmolar",
            ],
        )?;
        let dry_deposition_velocity = parse_dry_velocity_m_s(&raw, context, &name)?;
        let (half_life_s, decay_constant) = parse_decay(&raw, context)?;
        let wet_a_gas = parse_positive_or_none(&raw, context, &["pweta_gas", "weta_gas"])?;
        let wet_b_gas = parse_positive_or_none(&raw, context, &["pwetb_gas", "wetb_gas"])?;
        let crain_aero = parse_positive_or_none(&raw, context, &["pcrain_aero", "crain_aero"])?;
        let csnow_aero = parse_positive_or_none(&raw, context, &["pcsnow_aero", "csnow_aero"])?;
        let ccn_aero = parse_positive_or_none(&raw, context, &["pccn_aero", "ccn_aero"])?;
        let in_aero = parse_positive_or_none(&raw, context, &["pin_aero", "in_aero"])?;
        let relative_diffusivity = parse_positive_or_none(&raw, context, &["preldiff", "reldiff"])?;
        let henry = parse_positive_or_none(&raw, context, &["phenry", "henry"])?;
        let surface_reactivity_f0 = parse_non_negative_or_none(&raw, context, &["pf0", "f0"])?;
        let particle_density_kg_m3 =
            parse_positive_or_none(&raw, context, &["pdensity", "density"])?;
        let mean_diameter_um = parse_diameter_um(&raw, context)?;
        let diameter_sigma = parse_positive_or_none(&raw, context, &["pdsigma", "dsigma"])?;
        reject_non_spherical_shape(&raw, context, &name)?;

        let species = Self {
            name,
            version,
            molecular_weight,
            dry_deposition_velocity,
            decay_constant,
            half_life_s,
            wet_a_gas,
            wet_b_gas,
            crain_aero,
            csnow_aero,
            ccn_aero,
            in_aero,
            relative_diffusivity,
            henry,
            surface_reactivity_f0,
            particle_density_kg_m3,
            mean_diameter_um,
            diameter_sigma,
            source_file: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            raw,
        };
        species.validate()?;
        Ok(species)
    }

    /// Gas or aerosol classification from mean diameter.
    ///
    /// Ported from `readspecies` gas/aerosol branching (`dquer > 0` means
    /// aerosol, see `readoptions_mod.f90:3010-3013`).
    #[must_use]
    pub fn species_kind(&self) -> SpeciesKind {
        if self.mean_diameter_um.is_some_and(|d| d > 0.0) {
            SpeciesKind::Aerosol
        } else {
            SpeciesKind::Gas
        }
    }

    /// Whether dry deposition is enabled for this species.
    ///
    /// Mirrors `readreleases` (`reldiff > 0`, `density > 0`, or `dryvel > 0`
    /// sets `DRYDEPSPEC`, see `readoptions_mod.f90:2388-2391`).
    #[must_use]
    pub fn dry_deposition_active(&self) -> bool {
        self.relative_diffusivity.is_some_and(|v| v > 0.0)
            || self.particle_density_kg_m3.is_some_and(|v| v > 0.0)
            || self.dry_deposition_velocity.is_some_and(|v| v > 0.0)
    }

    /// Whether below-cloud scavenging is enabled for this species.
    #[must_use]
    pub fn below_cloud_wet_active(&self) -> bool {
        match self.species_kind() {
            SpeciesKind::Gas => {
                self.wet_a_gas.is_some_and(|v| v > 0.0) || self.wet_b_gas.is_some_and(|v| v > 0.0)
            }
            SpeciesKind::Aerosol => {
                self.crain_aero.is_some_and(|v| v > 0.0) || self.csnow_aero.is_some_and(|v| v > 0.0)
            }
        }
    }

    /// Whether in-cloud scavenging is enabled (aerosols only).
    #[must_use]
    pub fn in_cloud_wet_active(&self) -> bool {
        self.species_kind() == SpeciesKind::Aerosol
            && (self.ccn_aero.is_some_and(|v| v > 0.0) || self.in_aero.is_some_and(|v| v > 0.0))
    }

    /// Whether radioactive decay is enabled for this species.
    #[must_use]
    pub fn decay_active(&self) -> bool {
        self.decay_constant.is_some_and(|v| v > 0.0)
    }

    /// Whether this species carries no active deposition or decay pathway.
    #[must_use]
    pub fn is_passive_tracer(&self) -> bool {
        !self.dry_deposition_active()
            && !self.below_cloud_wet_active()
            && !self.in_cloud_wet_active()
            && !self.decay_active()
    }

    /// Reject unsupported versions and inconsistent species process parameters.
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_config_version(self.version, &format!("species `{}`", self.name))?;
        if self.name.trim().is_empty() {
            return Err(ConfigError::Validation {
                message: "species name must not be empty".to_string(),
            });
        }
        validate_positive_option(&self.name, "molecular_weight", self.molecular_weight, false)?;
        if let Some(vdep) = self.dry_deposition_velocity {
            if !vdep.is_finite() || vdep < 0.0 {
                return Err(ConfigError::Validation {
                    message: format!(
                        "species `{}` dry_deposition_velocity must be >= 0",
                        self.name
                    ),
                });
            }
        }
        if let Some(decay) = self.decay_constant {
            if !decay.is_finite() || decay < 0.0 {
                return Err(ConfigError::Validation {
                    message: format!("species `{}` decay_constant must be >= 0", self.name),
                });
            }
        }
        validate_positive_option(&self.name, "half_life_s", self.half_life_s, false)?;
        validate_positive_option(&self.name, "wet_a_gas", self.wet_a_gas, false)?;
        validate_positive_option(&self.name, "wet_b_gas", self.wet_b_gas, false)?;
        validate_positive_option(&self.name, "crain_aero", self.crain_aero, false)?;
        validate_positive_option(&self.name, "csnow_aero", self.csnow_aero, false)?;
        validate_positive_option(&self.name, "ccn_aero", self.ccn_aero, false)?;
        validate_positive_option(&self.name, "in_aero", self.in_aero, false)?;
        validate_positive_option(
            &self.name,
            "relative_diffusivity",
            self.relative_diffusivity,
            false,
        )?;
        validate_positive_option(&self.name, "henry", self.henry, false)?;
        if let Some(f0) = self.surface_reactivity_f0 {
            if !f0.is_finite() || f0 < 0.0 {
                return Err(ConfigError::Validation {
                    message: format!("species `{}` surface_reactivity_f0 must be >= 0", self.name),
                });
            }
        }
        validate_positive_option(
            &self.name,
            "particle_density_kg_m3",
            self.particle_density_kg_m3,
            false,
        )?;
        validate_positive_option(&self.name, "mean_diameter_um", self.mean_diameter_um, false)?;
        if let Some(sigma) = self.diameter_sigma {
            if !sigma.is_finite() || sigma < 0.0 {
                return Err(ConfigError::Validation {
                    message: format!("species `{}` diameter_sigma must be >= 0", self.name),
                });
            }
        }
        self.validate_gas_particle_branching()
    }

    fn validate_gas_particle_branching(&self) -> Result<(), ConfigError> {
        // A species cannot be both gas and particle (see
        // `readoptions_mod.f90:3114-3120`).
        if self.relative_diffusivity.is_some_and(|v| v > 0.0)
            && self.particle_density_kg_m3.is_some_and(|v| v > 0.0)
        {
            return Err(ConfigError::Validation {
                message: format!(
                    "species `{}` cannot be both gas (PRELDIFF > 0) and particle (PDENSITY > 0)",
                    self.name
                ),
            });
        }
        // Gas below-cloud scavenging without a Henry constant is a hard error
        // in FLEXPART (see `readspecies` error 996, `readoptions_mod.f90:3087-3091`).
        if self.species_kind() == SpeciesKind::Gas
            && (self.wet_a_gas.is_some_and(|v| v > 0.0) || self.wet_b_gas.is_some_and(|v| v > 0.0))
            && !self.henry.is_some_and(|v| v > 0.0)
        {
            return Err(ConfigError::Validation {
                message: format!(
                    "species `{}` gas wet removal requires PHENRY > 0",
                    self.name
                ),
            });
        }
        // Particle dry deposition needs a diameter for settling bins.
        if self.particle_density_kg_m3.is_some_and(|v| v > 0.0)
            && !self.mean_diameter_um.is_some_and(|v| v > 0.0)
        {
            return Err(ConfigError::Validation {
                message: format!(
                    "species `{}` has PDENSITY > 0 but no positive PDIA",
                    self.name
                ),
            });
        }
        // Aerosol width convention changed in v10.4: values <= 1 are rejected
        // (see `readoptions_mod.f90:3099-3112`).
        if self.mean_diameter_um.is_some_and(|v| v > 0.0)
            && !self.diameter_sigma.is_some_and(|sigma| sigma > 1.0)
        {
            return Err(ConfigError::Validation {
                message: format!(
                    "species `{}` aerosol diameter requires PDSIGMA > 1",
                    self.name
                ),
            });
        }
        Ok(())
    }
}

/// Half-life to decay-constant factor used by FLEXPART (`readoptions_mod.f90:2328`).
///
/// Uses the oracle's truncated `0.693147` instead of full-precision `LN_2` for
/// bit-level parity with Fortran validation comparisons.
#[allow(clippy::approx_constant)]
const HALF_LIFE_TO_DECAY_FACTOR: f64 = 0.693_147;
/// Oracle dry-velocity unit conversion (`readoptions_mod.f90:2357`, cm/s to m/s).
const CM_S_TO_M_S: f64 = 0.01;
/// Oracle diameter unit conversion (`readoptions_mod.f90:2341`, m to um).
const M_TO_UM: f64 = 1_000_000.0;

/// Parse an oracle parameter where only positive finite values enable the pathway.
///
/// Missing keys, sentinel negatives, zero, and non-finite values all map to
/// `None` (disabled), mirroring Fortran `> 0` checks in `readspecies`.
fn parse_positive_or_none(
    raw: &ConfigMap,
    context: &str,
    keys: &[&str],
) -> Result<Option<f64>, ConfigError> {
    let value = parse_optional_f64(raw, context, keys)?;
    Ok(value.filter(|v| v.is_finite() && *v > 0.0))
}

/// Parse a parameter where zero is meaningful (e.g. `PF0`, default `0`).
///
/// Negative sentinels and missing keys map to `None`; explicit finite values
/// `>= 0` are preserved.
fn parse_non_negative_or_none(
    raw: &ConfigMap,
    context: &str,
    keys: &[&str],
) -> Result<Option<f64>, ConfigError> {
    let value = parse_optional_f64(raw, context, keys)?;
    Ok(value.filter(|v| v.is_finite() && *v >= 0.0))
}

/// Parse constant dry deposition velocity [m/s].
///
/// Oracle `PDRYVEL` is in cm/s and takes precedence; legacy
/// `dry_deposition_velocity`/`vdep` keys are already in m/s.
fn parse_dry_velocity_m_s(
    raw: &ConfigMap,
    context: &str,
    species_name: &str,
) -> Result<Option<f64>, ConfigError> {
    if let Some(input) = parse_optional_f64(raw, context, &["pdryvel"])? {
        if input.is_finite() && input > 0.0 {
            return Ok(Some(input * CM_S_TO_M_S));
        }
        if input.is_finite() && input <= 0.0 {
            return Ok(None);
        }
        return Err(ConfigError::Validation {
            message: format!("species `{species_name}` PDRYVEL must be finite"),
        });
    }
    let legacy = parse_optional_f64(raw, context, &["dry_deposition_velocity", "vdep"])?;
    if let Some(value) = legacy {
        if !value.is_finite() || value < 0.0 {
            return Err(ConfigError::Validation {
                message: format!("species `{species_name}` dry_deposition_velocity must be >= 0"),
            });
        }
    }
    Ok(legacy)
}

/// Parse radioactive decay from half-life or legacy decay-constant keys.
///
/// Returns `(half_life_s, decay_constant_s_inv)`. Oracle `PDECAY` holds the
/// half-life [s] and takes precedence; legacy `decay`/`decay_constant` keys
/// already hold the decay constant [1/s].
fn parse_decay(raw: &ConfigMap, context: &str) -> Result<(Option<f64>, Option<f64>), ConfigError> {
    if get_first(raw, &["pdecay"]).is_some() {
        let half_life = parse_optional_f64(raw, context, &["pdecay"])?;
        match half_life {
            Some(value) if value.is_finite() && value > 0.0 => {
                let lambda = HALF_LIFE_TO_DECAY_FACTOR / value;
                if !lambda.is_finite() || lambda <= 0.0 {
                    return Err(ConfigError::InvalidValue {
                        context: context.to_string(),
                        key: "pdecay".to_string(),
                        value: value.to_string(),
                        message: "positive half-life produces non-finite or invalid decay constant"
                            .to_string(),
                    });
                }
                return Ok((Some(value), Some(lambda)));
            }
            Some(value) if value.is_finite() && value <= 0.0 => {
                return Ok((None, None));
            }
            Some(value) => {
                return Err(ConfigError::InvalidValue {
                    context: context.to_string(),
                    key: "pdecay".to_string(),
                    value: value.to_string(),
                    message: "PDECAY must be a finite number".to_string(),
                });
            }
            None => {}
        }
    }
    let legacy = parse_optional_f64(raw, context, &["decay_constant", "decay"])?;
    if let Some(value) = legacy {
        if !value.is_finite() || value < 0.0 {
            return Err(ConfigError::InvalidValue {
                context: context.to_string(),
                key: "decay_constant".to_string(),
                value: value.to_string(),
                message: "expected finite value >= 0".to_string(),
            });
        }
    }
    Ok((None, legacy))
}

/// Parse mean particle diameter and convert to [um].
///
/// Oracle `PDIA` (fallback legacy `PDQUER`) is in [m]; direct `*_um` keys are
/// already in [um]. Positive values mark aerosols, otherwise the species is a
/// gas (`dquer > 0` branching in `readspecies`).
fn parse_diameter_um(raw: &ConfigMap, context: &str) -> Result<Option<f64>, ConfigError> {
    if get_first(raw, &["pdia", "pdquer"]).is_some() {
        let input = parse_optional_f64(raw, context, &["pdia", "pdquer"])?;
        return Ok(input
            .filter(|v| v.is_finite() && *v > 0.0)
            .map(|v| v * M_TO_UM));
    }
    parse_positive_or_none(
        raw,
        context,
        &["mean_diameter_um", "diameter_um", "dquer_um"],
    )
}

/// Reject non-spherical particle shapes for now.
///
/// FLEXPART supports `PSHAPE 0-6` with Bagheri & Bonadonna settling, but #126
/// only models spheres. Failing loudly avoids silently wrong settling.
fn reject_non_spherical_shape(
    raw: &ConfigMap,
    context: &str,
    species_name: &str,
) -> Result<(), ConfigError> {
    let shape = parse_optional_f64(raw, context, &["pshape", "shape"])?;
    if shape.is_some_and(|v| v != 0.0) {
        return Err(ConfigError::Validation {
            message: format!(
                "species `{species_name}` uses unsupported non-spherical PSHAPE (only PSHAPE=0 spheres in #126 scope)"
            ),
        });
    }
    Ok(())
}

fn validate_positive_option(
    species_name: &str,
    field: &str,
    value: Option<f64>,
    allow_zero: bool,
) -> Result<(), ConfigError> {
    if let Some(v) = value {
        let valid = v.is_finite() && (v > 0.0 || (allow_zero && v == 0.0));
        if !valid {
            return Err(ConfigError::Validation {
                message: format!("species `{species_name}` {field} must be > 0"),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::parsing::CURRENT_CONFIG_VERSION;
    use super::super::test_support::temp_dir;
    use super::*;

    fn species_from_assignments(pairs: &[(&str, &str)], filename: &str) -> SpeciesConfig {
        let raw: ConfigMap = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        SpeciesConfig::from_map(raw, Path::new(filename), "test").expect("parse species")
    }

    fn species_from_file_content(content: &str, filename: &str) -> SpeciesConfig {
        let base = temp_dir("species_inline");
        let path = base.join(filename);
        fs::write(&path, content).expect("write species file");
        let species = SpeciesConfig::from_file(&path).expect("parse species file");
        fs::remove_dir_all(&base).expect("cleanup temp directory");
        species
    }

    #[test]
    fn parse_oracle_tracer_species_024_is_passive() {
        let species = species_from_file_content(
            "&SPECIES_PARAMS\nPSPECIES=\"AIRTRACER\",PDECAY=-9.9,PWETA_GAS=-0.9E-9,PWETB_GAS=-9.9,\
             PCRAIN_AERO=-9.9,PCSNOW_AERO=-9.9,PCCN_AERO=-9.9,PIN_AERO=-9.9,PDENSITY=-0.9E+9,\
             PDIA=0.0,PDSIGMA=0.0,PDRYVEL=-9.99,PRELDIFF=-9.9,PHENRY=-0.9E-9,PF0=-9,\
             PWEIGHTMOLAR=29.0,/\n",
            "SPECIES_024",
        );
        assert_eq!(species.name, "AIRTRACER");
        assert_eq!(species.species_kind(), SpeciesKind::Gas);
        assert!(species.is_passive_tracer());
        assert!(!species.decay_active());
        assert!(!species.dry_deposition_active());
        assert_eq!(species.molecular_weight, Some(29.0));
        assert_eq!(species.half_life_s, None);
        assert_eq!(species.decay_constant, None);
        assert_eq!(species.mean_diameter_um, None);
    }

    #[test]
    fn parse_oracle_aerosol_species_040_maps_size_and_efficiencies() {
        let species = species_from_file_content(
            "&SPECIES_PARAMS\nPSPECIES=\"EXAMPLE\",PDECAY=-9.9,PCRAIN_AERO=1.0,PCSNOW_AERO=1.0,\
             PCCN_AERO=0.9,PIN_AERO=0.1,PDENSITY=1000.0,PDIA=50.0E-06,PDSIGMA=3.3,PDRYVEL=-9.9,\
             PRELDIFF=-9.9,PHENRY=-0.9E-9,PF0=-9,/\n",
            "SPECIES_040",
        );
        assert_eq!(species.name, "EXAMPLE");
        assert_eq!(species.species_kind(), SpeciesKind::Aerosol);
        assert_eq!(species.particle_density_kg_m3, Some(1000.0));
        let diameter = species.mean_diameter_um.expect("diameter parses");
        assert!((diameter - 50.0).abs() < 1.0e-9, "diameter um: {diameter}");
        assert_eq!(species.diameter_sigma, Some(3.3));
        assert_eq!(species.crain_aero, Some(1.0));
        assert_eq!(species.csnow_aero, Some(1.0));
        assert_eq!(species.ccn_aero, Some(0.9));
        assert_eq!(species.in_aero, Some(0.1));
        assert!(species.dry_deposition_active());
        assert!(species.below_cloud_wet_active());
        assert!(species.in_cloud_wet_active());
        assert!(!species.decay_active());
    }

    #[test]
    fn parse_oracle_xe133_half_life_converts_to_decay_constant() {
        let species = species_from_file_content(
            "&SPECIES_PARAMS\nPSPECIES=\"Xe-133\",PDECAY=453168.0,PWETA_GAS=-0.9E-9,PWETB_GAS=-9.9,\
             PCRAIN_AERO=-9.9,PCSNOW_AERO=-9.9,PCCN_AERO=-9.9,PIN_AERO=-9.9,PDENSITY=-0.9E+9,\
             PDIA=0.0,PDSIGMA=0.0,PDRYVEL=-9.99,PRELDIFF=-9.9,/\n",
            "SPECIES_021",
        );
        assert_eq!(species.name, "Xe-133");
        assert_eq!(species.half_life_s, Some(453_168.0));
        let lambda = species.decay_constant.expect("decay constant derived");
        let expected = HALF_LIFE_TO_DECAY_FACTOR / 453_168.0;
        assert!(
            (lambda - expected).abs() < 1.0e-12,
            "lambda: {lambda}, expected: {expected}"
        );
        assert!(species.decay_active());
        assert!(!species.dry_deposition_active());
    }

    #[test]
    fn oracle_sentinels_map_to_none() {
        let species = species_from_assignments(
            &[
                ("pspecies", "SENTINEL"),
                ("pdecay", "-999.9"),
                ("pweta_gas", "-9.9E-09"),
                ("pwetb_gas", "-9.9"),
                ("pcrain_aero", "-9.9E-09"),
                ("preldiff", "-9.9"),
                ("phenry", "-9.9"),
                ("pf0", "-9"),
                ("pdensity", "-9.9E09"),
                ("pdia", "-9.9"),
                ("pdsigma", "0.0"),
                ("pdryvel", "-9.99"),
                ("pweightmolar", "-999.9"),
            ],
            "SPECIES_999",
        );
        assert_eq!(species.half_life_s, None);
        assert_eq!(species.decay_constant, None);
        assert_eq!(species.wet_a_gas, None);
        assert_eq!(species.relative_diffusivity, None);
        assert_eq!(species.henry, None);
        assert_eq!(species.surface_reactivity_f0, None);
        assert_eq!(species.particle_density_kg_m3, None);
        assert_eq!(species.mean_diameter_um, None);
        assert_eq!(species.molecular_weight, None);
        assert!(species.is_passive_tracer());
    }

    #[test]
    fn pdryvel_cm_s_converts_to_m_s() {
        let species =
            species_from_assignments(&[("pspecies", "V"), ("pdryvel", "2.5")], "SPECIES_001");
        let vdep = species
            .dry_deposition_velocity
            .expect("dry velocity parses");
        assert!((vdep - 0.025).abs() < 1.0e-12, "vdep m/s: {vdep}");
    }

    #[test]
    fn legacy_dry_velocity_stays_in_m_s() {
        let species = species_from_assignments(
            &[("name", "V"), ("dry_deposition_velocity", "0.025")],
            "V.spec",
        );
        assert_eq!(species.dry_deposition_velocity, Some(0.025));
    }

    #[test]
    fn gas_particle_mutual_exclusion_rejected() {
        let raw: ConfigMap = [
            ("pspecies".to_string(), "MIX".to_string()),
            ("preldiff".to_string(), "0.5".to_string()),
            ("pdensity".to_string(), "1000.0".to_string()),
            ("pdia".to_string(), "1.0E-06".to_string()),
            ("pdsigma".to_string(), "2.0".to_string()),
        ]
        .into_iter()
        .collect();
        let result = SpeciesConfig::from_map(raw, Path::new("SPECIES_001"), "test");
        assert!(result.is_err(), "gas+particle mix must fail");
    }

    #[test]
    fn aerosol_requires_sigma_above_one() {
        let raw: ConfigMap = [
            ("pspecies".to_string(), "AERO".to_string()),
            ("pdensity".to_string(), "1000.0".to_string()),
            ("pdia".to_string(), "1.0E-06".to_string()),
            ("pdsigma".to_string(), "0.5".to_string()),
        ]
        .into_iter()
        .collect();
        let result = SpeciesConfig::from_map(raw, Path::new("SPECIES_002"), "test");
        assert!(result.is_err(), "dsigma <= 1 must fail for aerosols");
    }

    #[test]
    fn non_spherical_shape_rejected() {
        let raw: ConfigMap = [
            ("pspecies".to_string(), "FIBER".to_string()),
            ("pshape".to_string(), "2".to_string()),
        ]
        .into_iter()
        .collect();
        let result = SpeciesConfig::from_map(raw, Path::new("SPECIES_003"), "test");
        assert!(
            result.is_err(),
            "non-spherical shape must fail in #126 scope"
        );
    }

    #[test]
    fn parse_decay_rejects_nan_pdecay() {
        let raw: ConfigMap = [
            ("pspecies".to_string(), "NAN_DECAY".to_string()),
            ("pdecay".to_string(), "NaN".to_string()),
        ]
        .into_iter()
        .collect();
        let result = SpeciesConfig::from_map(raw, Path::new("SPECIES_NAN"), "test");
        assert!(result.is_err(), "NaN PDECAY must fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("pdecay") && err.contains("NaN"),
            "error must identify pdecay and value: {err}"
        );
    }

    #[test]
    fn parse_decay_rejects_overflow_half_life() {
        // Half-life small enough to overflow the decay constant calculation
        // HALF_LIFE_TO_DECAY_FACTOR = 0.693147, f64::MAX ≈ 1.8e308
        // Overflow when 0.693147 / value > f64::MAX => value < 0.693147 / 1.8e308 ≈ 3.8e-309
        // 1e-309 is subnormal but non-zero and causes overflow
        let raw: ConfigMap = [
            ("pspecies".to_string(), "OVERFLOW_DECAY".to_string()),
            ("pdecay".to_string(), "1e-309".to_string()),
        ]
        .into_iter()
        .collect();
        let result = SpeciesConfig::from_map(raw, Path::new("SPECIES_OVERFLOW"), "test");
        assert!(result.is_err(), "overflow half-life must fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("pdecay") && err.contains("non-finite"),
            "error must identify pdecay and overflow: {err}"
        );
    }

    #[test]
    fn parse_decay_sentinel_maps_to_none() {
        // Negative sentinel values (FLEXPART convention) map to decay disabled
        for sentinel in ["-999.9", "-9.9", "-1.0", "0.0"] {
            let raw: ConfigMap = [
                ("pspecies".to_string(), "SENTINEL".to_string()),
                ("pdecay".to_string(), sentinel.to_string()),
            ]
            .into_iter()
            .collect();
            let species = SpeciesConfig::from_map(raw, Path::new("SPECIES_SENTINEL"), "test")
                .expect("sentinel should parse");
            assert_eq!(
                species.half_life_s, None,
                "sentinel {sentinel} should disable decay"
            );
            assert_eq!(
                species.decay_constant, None,
                "sentinel {sentinel} should disable decay"
            );
            assert!(
                !species.decay_active(),
                "sentinel {sentinel} should not be decay active"
            );
        }
    }

    #[test]
    fn parse_decay_valid_positive_half_life() {
        let raw: ConfigMap = [
            ("pspecies".to_string(), "VALID_DECAY".to_string()),
            ("pdecay".to_string(), "453168.0".to_string()),
        ]
        .into_iter()
        .collect();
        let species = SpeciesConfig::from_map(raw, Path::new("SPECIES_VALID"), "test")
            .expect("valid half-life should parse");
        assert_eq!(species.half_life_s, Some(453_168.0));
        let lambda = species.decay_constant.expect("decay constant derived");
        let expected = HALF_LIFE_TO_DECAY_FACTOR / 453_168.0;
        assert!(
            (lambda - expected).abs() < 1.0e-12,
            "lambda: {lambda}, expected: {expected}"
        );
        assert!(species.decay_active());
    }

    #[test]
    fn species_directory_enforces_four_slot_limit() {
        let directory = temp_dir("species_cardinality");
        for index in 0..MAX_SPECIES {
            fs::write(
                directory.join(format!("SPECIES_{index:03}")),
                "&SPECIES_PARAMS PSPECIES='stable' /",
            )
            .expect("write species");
        }
        assert_eq!(
            SpeciesConfig::load_dir(&directory)
                .expect("four species")
                .len(),
            MAX_SPECIES
        );
        fs::write(
            directory.join(format!("SPECIES_{MAX_SPECIES:03}")),
            "&SPECIES_PARAMS PSPECIES='extra' /",
        )
        .expect("write fifth species");
        let error = SpeciesConfig::load_dir(&directory).expect_err("five species must fail");
        assert!(error
            .to_string()
            .contains("found 5, maximum supported is 4"));
        fs::remove_dir_all(directory).expect("remove species fixture");
    }

    #[test]
    fn species_versioned_and_legacy_files_have_explicit_handling() {
        let current = species_from_assignments(&[("name", "gas"), ("version", "1")], "gas.spec");
        assert_eq!(current.version, Some(CURRENT_CONFIG_VERSION));
        let legacy =
            species_from_file_content("&SPECIES_PARAMS PSPECIES='legacy' /", "SPECIES_001");
        assert_eq!(legacy.version, None);
        let raw = ConfigMap::from([
            ("name".to_string(), "future".to_string()),
            ("version".to_string(), "2".to_string()),
        ]);
        let error = SpeciesConfig::from_map(raw, Path::new("future.spec"), "test")
            .expect_err("future schema must fail");
        assert!(error.to_string().contains("unsupported version 2"));
    }
}
