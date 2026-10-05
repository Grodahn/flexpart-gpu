//! RELEASES source geometry, scalar/per-species inventory and validation.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::particles::MAX_SPECIES;

use super::error::ConfigError;
use super::parsing::{
    extract_namelist_sections, extract_timestamp, get_first, parse_assignments, parse_optional_f64,
    parse_optional_string, parse_optional_u64, parse_required_f64, read_text_file, strip_comments,
    ConfigMap,
};

/// Release chronology, geographic position, heights and injected mass in kilograms.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReleaseConfig {
    pub name: String,
    pub start_time: String,
    pub end_time: String,
    pub lon: f64,
    pub lat: f64,
    pub z_min: f64,
    pub z_max: f64,
    pub mass_kg: f64,
    pub particle_count: u64,
    /// Per-species release masses [kg], slot `s` ↔ simulation species `s`.
    ///
    /// Mirrors FLEXPART's `xmass(numpoint, nspec)` release matrix
    /// (`readoptions_mod.f90:2446-2448`); positional mapping replaces the
    /// Fortran `SPECNUM_REL` number mapping for now (see follow-up note in
    /// [`crate::release`]). `None` keeps the legacy single-species behavior
    /// (all mass into slot 0). When present, slot masses replace `mass_kg`
    /// for particle injection.
    pub species_masses_kg: Option<Vec<f64>>,
    pub raw: ConfigMap,
}

impl ReleaseConfig {
    /// Read all release records from a required RELEASES file.
    pub fn load_many_from_file(path: &Path) -> Result<Vec<Self>, ConfigError> {
        let content = read_text_file(path)?;
        Self::parse_many(&content, path)
    }

    /// Parse release records while retaining their source order and raw assignments.
    pub fn parse_many(input: &str, source: &Path) -> Result<Vec<Self>, ConfigError> {
        let context = format!("RELEASES ({})", source.display());
        let mut release_maps = Vec::new();
        let sections =
            extract_namelist_sections(input, "RELEASE").map_err(|message| ConfigError::Parse {
                context: context.clone(),
                message,
            })?;

        if !sections.is_empty() {
            for section in sections {
                let parsed = parse_assignments(&section).map_err(|message| ConfigError::Parse {
                    context: context.clone(),
                    message,
                })?;
                release_maps.push(parsed);
            }
        } else {
            for (line_idx, line) in strip_comments(input).lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let parsed = parse_assignments(line).map_err(|message| ConfigError::Parse {
                    context: format!("{context}, line {}", line_idx + 1),
                    message,
                })?;
                release_maps.push(parsed);
            }
        }

        if release_maps.is_empty() {
            return Err(ConfigError::Parse {
                context,
                message: "no release entries found".to_string(),
            });
        }

        let mut releases = Vec::with_capacity(release_maps.len());
        for (idx, raw) in release_maps.into_iter().enumerate() {
            releases.push(Self::from_map(
                raw,
                &format!("RELEASES entry #{}", idx + 1),
                idx + 1,
            )?);
        }
        Ok(releases)
    }

    fn from_map(raw: ConfigMap, context: &str, ordinal: usize) -> Result<Self, ConfigError> {
        let name = parse_optional_string(&raw, &["name", "release_name", "id"])
            .unwrap_or_else(|| format!("release_{ordinal}"));
        let start_time = extract_timestamp(&raw, context, &["start_time", "start"], None)?;
        let end_time = extract_timestamp(&raw, context, &["end_time", "end"], None)?;
        let lon = parse_required_f64(&raw, context, &["lon", "longitude"])?;
        let lat = parse_required_f64(&raw, context, &["lat", "latitude"])?;
        let z_min = parse_optional_f64(&raw, context, &["z_min", "z1", "z_bottom"])?.unwrap_or(0.0);
        let z_max = parse_optional_f64(&raw, context, &["z_max", "z2", "z_top"])?.unwrap_or(z_min);
        let mass_kg =
            parse_optional_f64(&raw, context, &["mass_kg", "mass", "xmass"])?.unwrap_or(1.0);
        let particle_count =
            parse_optional_u64(&raw, context, &["particle_count", "particles", "npart"])?
                .unwrap_or(1);
        let species_masses_kg = parse_species_masses_kg(&raw, context)?;

        let release = Self {
            name,
            start_time,
            end_time,
            lon,
            lat,
            z_min,
            z_max,
            mass_kg,
            particle_count,
            species_masses_kg,
            raw,
        };
        release.validate()?;
        Ok(release)
    }

    /// Reject invalid release geometry, chronology or scalar/per-species inventory.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.start_time > self.end_time {
            return Err(ConfigError::Validation {
                message: format!(
                    "release `{}` start_time `{}` is after end_time `{}`",
                    self.name, self.start_time, self.end_time
                ),
            });
        }
        if !(-180.0..=180.0).contains(&self.lon) {
            return Err(ConfigError::Validation {
                message: format!(
                    "release `{}` lon {} out of range [-180, 180]",
                    self.name, self.lon
                ),
            });
        }
        if !(-90.0..=90.0).contains(&self.lat) {
            return Err(ConfigError::Validation {
                message: format!(
                    "release `{}` lat {} out of range [-90, 90]",
                    self.name, self.lat
                ),
            });
        }
        if self.z_max < self.z_min {
            return Err(ConfigError::Validation {
                message: format!(
                    "release `{}` z_max {} must be >= z_min {}",
                    self.name, self.z_max, self.z_min
                ),
            });
        }
        if self.mass_kg <= 0.0 {
            return Err(ConfigError::Validation {
                message: format!("release `{}` mass_kg must be > 0", self.name),
            });
        }
        if self.particle_count == 0 {
            return Err(ConfigError::Validation {
                message: format!("release `{}` particle_count must be > 0", self.name),
            });
        }
        if let Some(masses) = &self.species_masses_kg {
            if masses.is_empty() || masses.len() > MAX_SPECIES {
                return Err(ConfigError::Validation {
                    message: format!(
                        "release `{}` species_masses_kg needs 1..={MAX_SPECIES} entries, got {}",
                        self.name,
                        masses.len()
                    ),
                });
            }
            if masses.iter().any(|m| !m.is_finite() || *m < 0.0) {
                return Err(ConfigError::Validation {
                    message: format!(
                        "release `{}` species_masses_kg must be finite and >= 0",
                        self.name
                    ),
                });
            }
            if masses.iter().sum::<f64>() <= 0.0 {
                return Err(ConfigError::Validation {
                    message: format!(
                        "release `{}` species_masses_kg total must be > 0",
                        self.name
                    ),
                });
            }
        }
        Ok(())
    }
}

/// Parse the optional per-species release mass list (`mass_species`).
///
/// Accepts comma- and/or whitespace-separated values, e.g.
/// `mass_species="1.0, 0.5"` (quotes keep the tokenizer from splitting the
/// list). Mirrors one row of FLEXPART's `xmass(numpoint, nspec)` matrix.
fn parse_species_masses_kg(
    raw: &ConfigMap,
    context: &str,
) -> Result<Option<Vec<f64>>, ConfigError> {
    let Some(text) = get_first(raw, &["mass_species", "species_mass", "species_masses"]) else {
        return Ok(None);
    };
    let mut masses = Vec::new();
    for token in text.split([',', ' ', '\t']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        masses.push(
            token
                .parse::<f64>()
                .map_err(|_| ConfigError::InvalidValue {
                    context: context.to_string(),
                    key: "mass_species".to_string(),
                    value: text.to_string(),
                    message: "expected comma-separated floating-point masses".to_string(),
                })?,
        );
    }
    if masses.is_empty() {
        return Err(ConfigError::InvalidValue {
            context: context.to_string(),
            key: "mass_species".to_string(),
            value: text.to_string(),
            message: "expected at least one species mass".to_string(),
        });
    }
    Ok(Some(masses))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_release_block() {
        let input = r#"
            &RELEASE
              NAME='stack',
              START='20240101120000',
              END='20240101150000',
              LON=7.25,
              LAT=46.1,
              Z1=10,
              Z2=120,
              MASS=2.5,
              PARTICLES=1000
            /
        "#;
        let releases =
            ReleaseConfig::parse_many(input, Path::new("<inline>")).expect("parse release");
        assert_eq!(releases.len(), 1);
        let release = &releases[0];
        assert_eq!(release.name, "stack");
        assert_eq!(release.particle_count, 1000);
        assert_eq!(release.lon, 7.25);
        assert_eq!(release.species_masses_kg, None);
    }

    #[test]
    fn parse_release_with_species_masses() {
        let input = r#"
            &RELEASE
              NAME='multi',
              START='20240101120000',
              END='20240101150000',
              LON=7.25,
              LAT=46.1,
              Z1=10,
              Z2=120,
              MASS_SPECIES="1.0, 0.5, 0.25",
              PARTICLES=1000
            /
        "#;
        let releases =
            ReleaseConfig::parse_many(input, Path::new("<inline>")).expect("parse release");
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].species_masses_kg, Some(vec![1.0, 0.5, 0.25]));
    }

    #[test]
    fn release_with_too_many_species_masses_rejected() {
        let input = r#"
            &RELEASE
              NAME='too-many',
              START='20240101120000',
              END='20240101150000',
              LON=7.25,
              LAT=46.1,
              Z1=10,
              Z2=120,
              MASS_SPECIES="1,1,1,1,1",
              PARTICLES=1000
            /
        "#;
        let result = ReleaseConfig::parse_many(input, Path::new("<inline>"));
        assert!(result.is_err(), "more than MAX_SPECIES masses must fail");
    }
}
