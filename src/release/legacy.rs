//! Existing receptor and diagnostic scheduler input.

use crate::{config::ConfigError, particles::MAX_SPECIES};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Transitional receptor and diagnostic scheduler input. Production forward input uses `ReleaseSpec`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LegacyReleaseConfig {
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
    /// Fortran `SPECNUM_REL` number mapping on this transitional path. `None`
    /// retains the diagnostic convention of placing `mass_kg` into slot 0.
    pub species_masses_kg: Option<Vec<f64>>,
    pub raw: BTreeMap<String, String>,
}

impl LegacyReleaseConfig {
    pub(super) fn legacy_horizontal_bounds(&self) -> (f64, f64, f64, f64) {
        let optional = |aliases: &[&str], original| {
            aliases
                .iter()
                .find_map(|key| self.raw.get(*key))
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(original)
        };
        (
            optional(&["lon_min", "xpoint1", "xlon1"], self.lon),
            optional(&["lon_max", "xpoint2", "xlon2"], self.lon),
            optional(&["lat_min", "ypoint1", "ylat1"], self.lat),
            optional(&["lat_max", "ypoint2", "ylat2"], self.lat),
        )
    }

    pub(super) fn legacy_mass_row(&self) -> [f64; MAX_SPECIES] {
        let mut masses = [0.0; MAX_SPECIES];
        if let Some(row) = &self.species_masses_kg {
            for (slot, mass) in masses.iter_mut().zip(row) {
                *slot = *mass;
            }
        } else {
            masses[0] = self.mass_kg;
        }
        masses
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
