//! OUTGRID geographic bounds, grid dimensions and spacing validation.

use serde::{Deserialize, Serialize};
use std::path::Path;

use super::error::ConfigError;
use super::parsing::{
    extract_namelist_sections, parse_assignments, parse_optional_f64, parse_required_f64,
    parse_required_u32, read_text_file, strip_comments, ConfigMap,
};

/// Output geographic bounds, cell counts and horizontal/vertical spacing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputGridConfig {
    pub lon_min: f64,
    pub lon_max: f64,
    pub lat_min: f64,
    pub lat_max: f64,
    pub nx: u32,
    pub ny: u32,
    pub nz: u32,
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    pub raw: ConfigMap,
}

impl OutputGridConfig {
    /// Read output-grid geometry from a required OUTGRID file.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = read_text_file(path)?;
        Self::parse(&content, path)
    }

    /// Parse output bounds and retain the existing inferred horizontal spacing.
    pub fn parse(input: &str, source: &Path) -> Result<Self, ConfigError> {
        let context = format!("OUTGRID ({})", source.display());
        let raw = if let Some(section) = extract_namelist_sections(input, "OUTGRID")
            .map_err(|message| ConfigError::Parse {
                context: context.clone(),
                message,
            })?
            .first()
            .cloned()
        {
            parse_assignments(&section).map_err(|message| ConfigError::Parse {
                context: context.clone(),
                message,
            })?
        } else {
            parse_assignments(&strip_comments(input)).map_err(|message| ConfigError::Parse {
                context: context.clone(),
                message,
            })?
        };
        Self::from_map(raw, &context)
    }

    fn from_map(raw: ConfigMap, context: &str) -> Result<Self, ConfigError> {
        let lon_min = parse_required_f64(&raw, context, &["lon_min", "xlon0", "xmin"])?;
        let lon_max = parse_required_f64(&raw, context, &["lon_max", "xlon1", "xmax"])?;
        let lat_min = parse_required_f64(&raw, context, &["lat_min", "ylat0", "ymin"])?;
        let lat_max = parse_required_f64(&raw, context, &["lat_max", "ylat1", "ymax"])?;
        let nx = parse_required_u32(&raw, context, &["nx"])?;
        let ny = parse_required_u32(&raw, context, &["ny"])?;
        let nz = parse_required_u32(&raw, context, &["nz"])?;
        let dx = parse_optional_f64(&raw, context, &["dx", "xres"])?
            .unwrap_or((lon_max - lon_min) / f64::from(nx));
        let dy = parse_optional_f64(&raw, context, &["dy", "yres"])?
            .unwrap_or((lat_max - lat_min) / f64::from(ny));
        let dz = parse_required_f64(&raw, context, &["dz", "zres"])?;

        let grid = Self {
            lon_min,
            lon_max,
            lat_min,
            lat_max,
            nx,
            ny,
            nz,
            dx,
            dy,
            dz,
            raw,
        };
        grid.validate()?;
        Ok(grid)
    }

    /// Reject invalid output bounds, dimensions and spacing.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.lon_min >= self.lon_max {
            return Err(ConfigError::Validation {
                message: format!(
                    "OUTGRID lon_min ({}) must be < lon_max ({})",
                    self.lon_min, self.lon_max
                ),
            });
        }
        if self.lat_min >= self.lat_max {
            return Err(ConfigError::Validation {
                message: format!(
                    "OUTGRID lat_min ({}) must be < lat_max ({})",
                    self.lat_min, self.lat_max
                ),
            });
        }
        if !(-180.0..=180.0).contains(&self.lon_min) || !(-180.0..=180.0).contains(&self.lon_max) {
            return Err(ConfigError::Validation {
                message: "OUTGRID longitude bounds must be within [-180, 180]".to_string(),
            });
        }
        if !(-90.0..=90.0).contains(&self.lat_min) || !(-90.0..=90.0).contains(&self.lat_max) {
            return Err(ConfigError::Validation {
                message: "OUTGRID latitude bounds must be within [-90, 90]".to_string(),
            });
        }
        if self.nx == 0 || self.ny == 0 || self.nz == 0 {
            return Err(ConfigError::Validation {
                message: "OUTGRID dimensions nx, ny, nz must all be > 0".to_string(),
            });
        }
        if self.dx <= 0.0 || self.dy <= 0.0 || self.dz <= 0.0 {
            return Err(ConfigError::Validation {
                message: "OUTGRID resolution dx, dy, dz must all be > 0".to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::command::CommandConfig;
    use super::*;

    #[test]
    fn validation_failures_reported() {
        let bad_command = CommandConfig {
            start_time: "20240102120000".to_string(),
            end_time: "20240101120000".to_string(),
            output_interval_seconds: Some(3600),
            sync_interval_seconds: None,
            raw: ConfigMap::new(),
        };
        assert!(bad_command.validate().is_err());

        let bad_grid = OutputGridConfig {
            lon_min: 0.0,
            lon_max: 10.0,
            lat_min: 45.0,
            lat_max: 55.0,
            nx: 100,
            ny: 100,
            nz: 10,
            dx: -0.1,
            dy: 0.1,
            dz: 100.0,
            raw: ConfigMap::new(),
        };
        assert!(bad_grid.validate().is_err());
    }
}
