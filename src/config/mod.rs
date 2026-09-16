//! Configuration parser for FLEXPART input files.
//!
//! This module implements a practical MVP parser for:
//! - `COMMAND` as a Fortran namelist (`&COMMAND ... /`)
//! - `RELEASES` either as repeated `&RELEASE ... /` blocks or line-based key/value records
//! - `OUTGRID` as `&OUTGRID ... /` or plain key/value content
//! - `SPECIES` as a directory containing one key/value (or `&SPECIES`) file per species
//!
//! The full FLEXPART grammar is significantly larger; this parser intentionally supports a
//! robust subset with clear error messages and typed validation to unblock integration.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

type ConfigMap = BTreeMap<String, String>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimulationConfig {
    pub command: CommandConfig,
    pub releases: Vec<ReleaseConfig>,
    pub outgrid: OutputGridConfig,
    pub species: Vec<SpeciesConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandConfig {
    pub start_time: String,
    pub end_time: String,
    pub output_interval_seconds: Option<u64>,
    pub sync_interval_seconds: Option<u64>,
    pub raw: ConfigMap,
}

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
    pub raw: ConfigMap,
}

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpeciesKind {
    Gas,
    Aerosol,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeciesConfig {
    pub name: String,
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

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing configuration path: {path}")]
    MissingPath { path: PathBuf },
    #[error("failed to read `{path}`: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error in {context}: {message}")]
    Parse { context: String, message: String },
    #[error("missing key `{key}` in {context}")]
    MissingKey { context: String, key: String },
    #[error("invalid value for `{key}` in {context}: `{value}` ({message})")]
    InvalidValue {
        context: String,
        key: String,
        value: String,
        message: String,
    },
    #[error("validation error: {message}")]
    Validation { message: String },
}

impl SimulationConfig {
    pub fn load(base_path: &Path) -> Result<Self, ConfigError> {
        let command = CommandConfig::from_file(&base_path.join("COMMAND"))?;
        let releases = ReleaseConfig::load_many_from_file(&base_path.join("RELEASES"))?;
        let outgrid = OutputGridConfig::from_file(&base_path.join("OUTGRID"))?;
        let species = SpeciesConfig::load_dir(&base_path.join("SPECIES"))?;

        let config = Self {
            command,
            releases,
            outgrid,
            species,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.command.validate()?;
        self.outgrid.validate()?;

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
        Ok(())
    }
}

impl CommandConfig {
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = read_text_file(path)?;
        Self::parse(&content, path)
    }

    pub fn parse(input: &str, source: &Path) -> Result<Self, ConfigError> {
        let context = format!("COMMAND ({})", source.display());
        let sections =
            extract_namelist_sections(input, "COMMAND").map_err(|message| ConfigError::Parse {
                context: context.clone(),
                message,
            })?;
        let section = sections.first().ok_or_else(|| ConfigError::Parse {
            context: context.clone(),
            message: "expected `&COMMAND ... /` section".to_string(),
        })?;
        let raw = parse_assignments(section).map_err(|message| ConfigError::Parse {
            context: context.clone(),
            message,
        })?;
        Self::from_map(raw, &context)
    }

    fn from_map(raw: ConfigMap, context: &str) -> Result<Self, ConfigError> {
        let start_time = extract_timestamp(
            &raw,
            context,
            &["start_time", "start", "ibdatetime"],
            Some(("ibdate", "ibtime")),
        )?;
        let end_time = extract_timestamp(
            &raw,
            context,
            &["end_time", "end", "iedatetime"],
            Some(("iedate", "ietime")),
        )?;
        let output_interval_seconds = parse_optional_u64(
            &raw,
            context,
            &["loutstep", "outstep", "output_interval_seconds"],
        )?;
        let sync_interval_seconds =
            parse_optional_u64(&raw, context, &["lsynctime", "sync_interval_seconds"])?;

        let config = Self {
            start_time,
            end_time,
            output_interval_seconds,
            sync_interval_seconds,
            raw,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.start_time > self.end_time {
            return Err(ConfigError::Validation {
                message: format!(
                    "COMMAND start_time `{}` is after end_time `{}`",
                    self.start_time, self.end_time
                ),
            });
        }
        if self.output_interval_seconds == Some(0) {
            return Err(ConfigError::Validation {
                message: "COMMAND output interval must be > 0".to_string(),
            });
        }
        if self.sync_interval_seconds == Some(0) {
            return Err(ConfigError::Validation {
                message: "COMMAND sync interval must be > 0".to_string(),
            });
        }
        Ok(())
    }
}

impl ReleaseConfig {
    pub fn load_many_from_file(path: &Path) -> Result<Vec<Self>, ConfigError> {
        let content = read_text_file(path)?;
        Self::parse_many(&content, path)
    }

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
            raw,
        };
        release.validate()?;
        Ok(release)
    }

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
        Ok(())
    }
}

impl OutputGridConfig {
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = read_text_file(path)?;
        Self::parse(&content, path)
    }

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

impl SpeciesConfig {
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

    pub fn validate(&self) -> Result<(), ConfigError> {
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
                    return Ok((None, None));
                }
                return Ok((Some(value), Some(lambda)));
            }
            Some(value) if value.is_finite() && value <= 0.0 => {
                return Ok((None, None));
            }
            Some(_) => return Ok((None, None)),
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

fn read_text_file(path: &Path) -> Result<String, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::MissingPath {
            path: path.to_path_buf(),
        });
    }
    fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.to_path_buf(),
        source,
    })
}

fn extract_namelist_sections(input: &str, section_name: &str) -> Result<Vec<String>, String> {
    let lower = input.to_ascii_lowercase();
    let target = section_name.to_ascii_lowercase();
    let mut sections = Vec::new();
    let mut idx = 0;

    while let Some(found) = lower[idx..].find('&') {
        let start = idx + found;
        let mut name_end = start + 1;

        while let Some(ch) = lower.as_bytes().get(name_end).map(|byte| char::from(*byte)) {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                name_end += 1;
            } else {
                break;
            }
        }

        if name_end == start + 1 {
            idx = start + 1;
            continue;
        }

        let name = &lower[(start + 1)..name_end];
        if name != target {
            idx = name_end;
            continue;
        }

        let mut end = name_end;
        let mut in_single = false;
        let mut in_double = false;
        let mut found_terminator = false;

        while let Some(ch) = input.as_bytes().get(end).map(|byte| char::from(*byte)) {
            if ch == '\'' && !in_double {
                in_single = !in_single;
            } else if ch == '"' && !in_single {
                in_double = !in_double;
            } else if ch == '/' && !in_single && !in_double {
                sections.push(input[name_end..end].to_string());
                end += 1;
                found_terminator = true;
                break;
            }
            end += 1;
        }

        if !found_terminator {
            return Err(format!(
                "section `&{section_name}` is missing a terminating `/`"
            ));
        }

        idx = end;
    }
    Ok(sections)
}

fn strip_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        let mut in_single = false;
        let mut in_double = false;
        for ch in line.chars() {
            if ch == '\'' && !in_double {
                in_single = !in_single;
            } else if ch == '"' && !in_single {
                in_double = !in_double;
            }
            if !in_single && !in_double && (ch == '!' || ch == '#') {
                break;
            }
            out.push(ch);
        }
        out.push('\n');
    }
    out
}

fn parse_assignments(input: &str) -> Result<ConfigMap, String> {
    let cleaned = strip_comments(input);
    let mut map = BTreeMap::new();
    let mut chars = cleaned.chars().peekable();

    while chars.peek().is_some() {
        while let Some(ch) = chars.peek() {
            if ch.is_whitespace() || *ch == ',' {
                chars.next();
            } else {
                break;
            }
        }
        if chars.peek().is_none() {
            break;
        }

        let mut key = String::new();
        while let Some(ch) = chars.peek() {
            if *ch == '=' {
                break;
            }
            if *ch == ',' || *ch == '\n' || *ch == '\r' {
                return Err(format!("expected `=` after key `{}`", key.trim()));
            }
            key.push(*ch);
            chars.next();
        }
        if chars.next() != Some('=') {
            return Err(format!("missing `=` for key `{}`", key.trim()));
        }
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            return Err("empty key before `=`".to_string());
        }

        while let Some(ch) = chars.peek() {
            if ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }

        let mut value = String::new();
        let mut in_single = false;
        let mut in_double = false;
        while let Some(ch) = chars.peek() {
            let c = *ch;
            if c == '\'' && !in_double {
                in_single = !in_single;
                value.push(c);
                chars.next();
                continue;
            }
            if c == '"' && !in_single {
                in_double = !in_double;
                value.push(c);
                chars.next();
                continue;
            }
            if !in_single && !in_double && (c == ',' || c == '\n' || c == '\r') {
                break;
            }
            value.push(c);
            chars.next();
        }

        let value = trim_wrapping_quotes(value.trim());
        if value.is_empty() {
            return Err(format!("empty value for key `{key}`"));
        }
        map.insert(key, value.to_string());

        while let Some(ch) = chars.peek() {
            if *ch == ',' || *ch == '\n' || *ch == '\r' || ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
    }

    Ok(map)
}

fn trim_wrapping_quotes(input: &str) -> &str {
    if input.len() >= 2 {
        let starts_single = input.starts_with('\'') && input.ends_with('\'');
        let starts_double = input.starts_with('"') && input.ends_with('"');
        if starts_single || starts_double {
            return &input[1..(input.len() - 1)];
        }
    }
    input
}

fn get_first<'a>(map: &'a ConfigMap, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| map.get(*key).map(String::as_str))
}

fn parse_optional_string(map: &ConfigMap, keys: &[&str]) -> Option<String> {
    get_first(map, keys).map(str::trim).map(ToString::to_string)
}

fn parse_required_f64(map: &ConfigMap, context: &str, keys: &[&str]) -> Result<f64, ConfigError> {
    parse_optional_f64(map, context, keys)?.ok_or_else(|| ConfigError::MissingKey {
        context: context.to_string(),
        key: keys[0].to_string(),
    })
}

fn parse_optional_f64(
    map: &ConfigMap,
    context: &str,
    keys: &[&str],
) -> Result<Option<f64>, ConfigError> {
    let Some(raw) = get_first(map, keys) else {
        return Ok(None);
    };
    raw.parse::<f64>()
        .map(Some)
        .map_err(|_| ConfigError::InvalidValue {
            context: context.to_string(),
            key: keys[0].to_string(),
            value: raw.to_string(),
            message: "expected floating-point number".to_string(),
        })
}

fn parse_required_u32(map: &ConfigMap, context: &str, keys: &[&str]) -> Result<u32, ConfigError> {
    parse_optional_u32(map, context, keys)?.ok_or_else(|| ConfigError::MissingKey {
        context: context.to_string(),
        key: keys[0].to_string(),
    })
}

fn parse_optional_u32(
    map: &ConfigMap,
    context: &str,
    keys: &[&str],
) -> Result<Option<u32>, ConfigError> {
    let Some(raw) = get_first(map, keys) else {
        return Ok(None);
    };
    raw.parse::<u32>()
        .map(Some)
        .map_err(|_| ConfigError::InvalidValue {
            context: context.to_string(),
            key: keys[0].to_string(),
            value: raw.to_string(),
            message: "expected unsigned integer".to_string(),
        })
}

fn parse_optional_u64(
    map: &ConfigMap,
    context: &str,
    keys: &[&str],
) -> Result<Option<u64>, ConfigError> {
    let Some(raw) = get_first(map, keys) else {
        return Ok(None);
    };
    raw.parse::<u64>()
        .map(Some)
        .map_err(|_| ConfigError::InvalidValue {
            context: context.to_string(),
            key: keys[0].to_string(),
            value: raw.to_string(),
            message: "expected unsigned integer".to_string(),
        })
}

fn extract_timestamp(
    map: &ConfigMap,
    context: &str,
    direct_keys: &[&str],
    date_time_keys: Option<(&str, &str)>,
) -> Result<String, ConfigError> {
    if let Some(raw) = get_first(map, direct_keys) {
        return normalize_timestamp(raw).map_err(|message| ConfigError::InvalidValue {
            context: context.to_string(),
            key: direct_keys[0].to_string(),
            value: raw.to_string(),
            message,
        });
    }

    if let Some((date_key, time_key)) = date_time_keys {
        if let Some(date_raw) = map.get(date_key) {
            let time_raw = map.get(time_key).map_or("000000", String::as_str);
            let combined = format!("{date_raw}{time_raw}");
            return normalize_timestamp(&combined).map_err(|message| ConfigError::InvalidValue {
                context: context.to_string(),
                key: format!("{date_key}/{time_key}"),
                value: combined,
                message,
            });
        }
    }

    Err(ConfigError::MissingKey {
        context: context.to_string(),
        key: direct_keys[0].to_string(),
    })
}

fn normalize_timestamp(raw: &str) -> Result<String, String> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    let normalized = match digits.len() {
        8 => format!("{digits}000000"),
        10 => format!("{digits}0000"),
        12 => format!("{digits}00"),
        14 => digits,
        _ => {
            return Err(
                "expected a date/time with 8, 10, 12, or 14 digits (YYYYMMDD[HH[MM[SS]]])"
                    .to_string(),
            );
        }
    };

    let year = normalized[0..4]
        .parse::<u32>()
        .map_err(|_| "invalid year in timestamp".to_string())?;
    let month = normalized[4..6]
        .parse::<u32>()
        .map_err(|_| "invalid month in timestamp".to_string())?;
    let day = normalized[6..8]
        .parse::<u32>()
        .map_err(|_| "invalid day in timestamp".to_string())?;
    let hour = normalized[8..10]
        .parse::<u32>()
        .map_err(|_| "invalid hour in timestamp".to_string())?;
    let minute = normalized[10..12]
        .parse::<u32>()
        .map_err(|_| "invalid minute in timestamp".to_string())?;
    let second = normalized[12..14]
        .parse::<u32>()
        .map_err(|_| "invalid second in timestamp".to_string())?;

    if year == 0 {
        return Err("year must be > 0".to_string());
    }
    if !(1..=12).contains(&month) {
        return Err("month must be in [1, 12]".to_string());
    }
    if !(1..=31).contains(&day) {
        return Err("day must be in [1, 31]".to_string());
    }
    if hour > 23 {
        return Err("hour must be in [0, 23]".to_string());
    }
    if minute > 59 {
        return Err("minute must be in [0, 59]".to_string());
    }
    if second > 59 {
        return Err("second must be in [0, 59]".to_string());
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        path.push(format!(
            "flexpart_gpu_{prefix}_{nanos}_{}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp directory");
        path
    }

    #[test]
    fn parse_minimal_command_namelist() {
        let input = r#"
            &COMMAND
              IBDATE = 20240101,
              IBTIME = 120000,
              IEDATE = 20240101,
              IETIME = 180000,
              LOUTSTEP = 3600,
            /
        "#;
        let command = CommandConfig::parse(input, Path::new("<inline>")).expect("parse command");
        assert_eq!(command.start_time, "20240101120000");
        assert_eq!(command.end_time, "20240101180000");
        assert_eq!(command.output_interval_seconds, Some(3600));
    }

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
    }

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

    #[test]
    fn serde_round_trip_simulation_config() {
        let config = SimulationConfig {
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
}
