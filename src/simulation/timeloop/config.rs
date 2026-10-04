//! Run configuration and its existing fail-closed validation.

use crate::coords::GridDomain;
use crate::io::{PblComputationOptions, TimeBoundsBehavior};
use crate::particles::ParticleSpatialSortOptions;
use crate::physics::{PhiloxCounter, PhiloxKey, VelocityToGridScale};
use crate::simulation::timeloop::error::TimeLoopError;
use crate::simulation::timeloop::time::{parse_timestamp_seconds, timestep_seconds_f32};

/// Time-loop configuration for forward integration.
#[derive(Debug, Clone)]
pub struct ForwardTimeLoopConfig {
    /// Inclusive simulation start timestamp (`YYYYMMDDHHMMSS`).
    pub start_timestamp: String,
    /// Inclusive simulation end timestamp (`YYYYMMDDHHMMSS`).
    pub end_timestamp: String,
    /// Integration timestep [s].
    pub timestep_seconds: i64,
    /// Temporal-interpolation behavior outside met bracket.
    pub time_bounds_behavior: TimeBoundsBehavior,
    /// Conversion from wind velocity to grid-coordinate displacement.
    pub velocity_to_grid_scale: VelocityToGridScale,
    /// PBL diagnostic options used by IO-05 computation.
    pub pbl_options: PblComputationOptions,
    /// Dry-deposition reference height [m].
    pub dry_reference_height_m: f32,
    /// Vertical Langevin turbulence substeps per simulation timestep.
    pub langevin_vertical_substeps: u32,
    /// Minimum reflected particle height [m] used by Langevin PBL reflection.
    pub langevin_min_height_m: f32,
    /// Deterministic Philox RNG key for the Langevin update.
    pub philox_key: PhiloxKey,
    /// Initial Philox counter for the first timestep.
    pub initial_philox_counter: PhiloxCounter,
    /// Optional particle spatial sorting for better GPU memory locality (O-01).
    pub spatial_sort: Option<ForwardSpatialSortConfig>,
    /// If true, download the full particle buffer after each timestep to keep
    /// host-side [`ParticleStore`] synchronized in lockstep.
    ///
    /// Set to `false` in performance mode to defer host synchronization.
    pub sync_particle_store_each_step: bool,
    /// If true, download per-particle dry/wet deposition probabilities for each
    /// step report. Set to `false` to avoid per-step probability readbacks.
    pub collect_deposition_probabilities_each_step: bool,
}

/// Runtime settings for optional particle Morton sorting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForwardSpatialSortConfig {
    /// Apply sort every `interval_steps` timesteps (1 = every step).
    pub interval_steps: usize,
    /// Spatial-key and map-generation options.
    pub sort_options: ParticleSpatialSortOptions,
}

impl Default for ForwardTimeLoopConfig {
    fn default() -> Self {
        Self {
            start_timestamp: "20240101000000".to_string(),
            end_timestamp: "20240101000000".to_string(),
            timestep_seconds: 1,
            time_bounds_behavior: TimeBoundsBehavior::Clamp,
            velocity_to_grid_scale: VelocityToGridScale::IDENTITY,
            pbl_options: PblComputationOptions::default(),
            dry_reference_height_m: 15.0,
            langevin_vertical_substeps: 4,
            langevin_min_height_m: 0.01,
            philox_key: [0xDECA_FBAD, 0x1234_5678],
            initial_philox_counter: [0, 0, 0, 0],
            spatial_sort: None,
            sync_particle_store_each_step: true,
            collect_deposition_probabilities_each_step: true,
        }
    }
}

/// Direction of simulation time integration (`ldirect` in FLEXPART terms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeDirection {
    /// Forward integration (`ldirect = +1`).
    Forward,
    /// Backward integration (`ldirect = -1`).
    Backward,
}

impl TimeDirection {
    #[must_use]
    pub(super) fn advection_dt_seconds(self, timestep_seconds: f32) -> f32 {
        match self {
            Self::Forward => timestep_seconds,
            Self::Backward => -timestep_seconds,
        }
    }
}

/// Receptor release specification for backward mode.
///
/// Particles are released at receptor coordinates and then integrated backward
/// in time to estimate source attribution.
#[derive(Debug, Clone)]
pub struct BackwardReceptorConfig {
    /// Receptor identifier.
    pub name: String,
    /// Receptor longitude [degrees].
    pub lon: f64,
    /// Receptor latitude [degrees].
    pub lat: f64,
    /// Receptor release height [m].
    pub z_m: f64,
    /// Number of particles to release from this receptor.
    pub particle_count: u64,
    /// Total emitted mass [kg] distributed over `particle_count`.
    pub mass_kg: f64,
}

/// Source envelope used to collect backward particles.
///
/// A particle is counted as a "hit" if its position lies within all lon/lat/z
/// bounds after a backward timestep.
#[derive(Debug, Clone)]
pub struct BackwardSourceRegionConfig {
    /// Source-region identifier.
    pub name: String,
    /// Minimum longitude [degrees].
    pub lon_min: f64,
    /// Maximum longitude [degrees].
    pub lon_max: f64,
    /// Minimum latitude [degrees].
    pub lat_min: f64,
    /// Maximum latitude [degrees].
    pub lat_max: f64,
    /// Minimum height [m].
    pub z_min_m: f64,
    /// Maximum height [m].
    pub z_max_m: f64,
}

impl BackwardSourceRegionConfig {
    #[must_use]
    pub(super) fn contains_particle(
        &self,
        particle: &crate::particles::Particle,
        grid: &GridDomain,
    ) -> bool {
        let lon = grid.xlon0 + particle.grid_x() * grid.dx;
        let lat = grid.ylat0 + particle.grid_y() * grid.dy;
        let z = f64::from(particle.pos_z);
        (self.lon_min..=self.lon_max).contains(&lon)
            && (self.lat_min..=self.lat_max).contains(&lat)
            && (self.z_min_m..=self.z_max_m).contains(&z)
    }
}

/// Per-source backward collection summary at one timestep.
#[derive(Debug, Clone, PartialEq)]
pub struct BackwardSourceCollection {
    /// Number of active particles inside the source region.
    pub hit_count: usize,
    /// Total species-0 mass represented by those hits [kg].
    pub total_mass_kg: f32,
}

impl Default for BackwardSourceCollection {
    fn default() -> Self {
        Self {
            hit_count: 0,
            total_mass_kg: 0.0,
        }
    }
}

/// Time-loop configuration for backward (time-reversed) integration.
///
/// ## MVP simplifications
/// - Receptors are emitted once at `start_timestamp` (not continuously).
/// - Advection uses negative `dt`, but turbulence and deposition dispatch are
///   still executed with positive `dt` magnitude for numerical stability and
///   kernel compatibility. Callers can set zero forcing to effectively disable
///   deposition during backward attribution runs.
#[derive(Debug, Clone)]
pub struct BackwardTimeLoopConfig {
    /// Inclusive simulation start timestamp (`YYYYMMDDHHMMSS`), typically the
    /// receptor sampling time and therefore later than `end_timestamp`.
    pub start_timestamp: String,
    /// Inclusive simulation end timestamp (`YYYYMMDDHHMMSS`), typically earlier
    /// than `start_timestamp`.
    pub end_timestamp: String,
    /// Positive timestep magnitude [s].
    pub timestep_seconds: i64,
    /// Temporal-interpolation behavior outside met bracket.
    pub time_bounds_behavior: TimeBoundsBehavior,
    /// Conversion from wind velocity to grid-coordinate displacement.
    pub velocity_to_grid_scale: VelocityToGridScale,
    /// PBL diagnostic options used by IO-05 computation.
    pub pbl_options: PblComputationOptions,
    /// Dry-deposition reference height [m].
    pub dry_reference_height_m: f32,
    /// Deterministic Philox RNG key for the Langevin update.
    pub philox_key: PhiloxKey,
    /// Initial Philox counter for the first timestep.
    pub initial_philox_counter: PhiloxCounter,
    /// Receptors that emit backward particles at `start_timestamp`.
    pub receptors: Vec<BackwardReceptorConfig>,
    /// Source regions where backward particles are collected.
    pub source_regions: Vec<BackwardSourceRegionConfig>,
}

impl Default for BackwardTimeLoopConfig {
    fn default() -> Self {
        Self {
            start_timestamp: "20240101000000".to_string(),
            end_timestamp: "20240101000000".to_string(),
            timestep_seconds: 1,
            time_bounds_behavior: TimeBoundsBehavior::Clamp,
            velocity_to_grid_scale: VelocityToGridScale::IDENTITY,
            pbl_options: PblComputationOptions::default(),
            dry_reference_height_m: 15.0,
            philox_key: [0xDECA_FBAD, 0x1234_5678],
            initial_philox_counter: [0, 0, 0, 0],
            receptors: Vec::new(),
            source_regions: Vec::new(),
        }
    }
}

pub(super) fn validate_config(config: &ForwardTimeLoopConfig) -> Result<(i64, i64), TimeLoopError> {
    let start_seconds = parse_timestamp_seconds(&config.start_timestamp)?;
    let end_seconds = parse_timestamp_seconds(&config.end_timestamp)?;
    if start_seconds > end_seconds {
        return Err(TimeLoopError::InvalidTimeRange {
            start: config.start_timestamp.to_string(),
            end: config.end_timestamp.to_string(),
        });
    }
    if config.timestep_seconds <= 0 {
        return Err(TimeLoopError::InvalidTimestep {
            value: config.timestep_seconds,
        });
    }
    if !config.dry_reference_height_m.is_finite() || config.dry_reference_height_m <= 0.0 {
        return Err(TimeLoopError::InvalidDryReferenceHeight {
            value: config.dry_reference_height_m,
        });
    }
    if !(1..=4).contains(&config.langevin_vertical_substeps) {
        return Err(TimeLoopError::InvalidLangevinSubsteps {
            value: config.langevin_vertical_substeps,
        });
    }
    if !config.langevin_min_height_m.is_finite() || config.langevin_min_height_m <= 0.0 {
        return Err(TimeLoopError::InvalidLangevinMinHeight {
            value: config.langevin_min_height_m,
        });
    }
    if let Some(spatial_sort) = config.spatial_sort {
        if spatial_sort.interval_steps == 0 {
            return Err(TimeLoopError::InvalidSpatialSortInterval { value: 0 });
        }
        let _ = crate::particles::morton_key_from_position(
            0.0,
            0.0,
            0.0,
            crate::particles::SpatialSortBounds {
                x_min: 0.0,
                x_max: 1.0,
                y_min: 0.0,
                y_max: 1.0,
                z_min: 0.0,
                z_max: 1.0,
            },
            spatial_sort.sort_options.bits_per_axis,
        )?;
    }
    let _ = timestep_seconds_f32(config.timestep_seconds)?;
    Ok((start_seconds, end_seconds))
}

pub(super) fn validate_backward_config(
    config: &BackwardTimeLoopConfig,
) -> Result<(i64, i64), TimeLoopError> {
    let start_seconds = parse_timestamp_seconds(&config.start_timestamp)?;
    let end_seconds = parse_timestamp_seconds(&config.end_timestamp)?;
    if start_seconds < end_seconds {
        return Err(TimeLoopError::InvalidBackwardTimeRange {
            start: config.start_timestamp.to_string(),
            end: config.end_timestamp.to_string(),
        });
    }
    if config.timestep_seconds <= 0 {
        return Err(TimeLoopError::InvalidTimestep {
            value: config.timestep_seconds,
        });
    }
    if !config.dry_reference_height_m.is_finite() || config.dry_reference_height_m <= 0.0 {
        return Err(TimeLoopError::InvalidDryReferenceHeight {
            value: config.dry_reference_height_m,
        });
    }
    if config.receptors.is_empty() {
        return Err(TimeLoopError::MissingReceptors);
    }
    for receptor in &config.receptors {
        validate_receptor(receptor)?;
    }
    for source in &config.source_regions {
        validate_source_region(source)?;
    }
    let _ = timestep_seconds_f32(config.timestep_seconds)?;
    Ok((start_seconds, end_seconds))
}

fn validate_receptor(receptor: &BackwardReceptorConfig) -> Result<(), TimeLoopError> {
    if !(-180.0..=180.0).contains(&receptor.lon) {
        return Err(TimeLoopError::InvalidReceptor {
            name: receptor.name.clone(),
            field: "lon",
            value: receptor.lon,
            reason: "must be in [-180, 180]",
        });
    }
    if !(-90.0..=90.0).contains(&receptor.lat) {
        return Err(TimeLoopError::InvalidReceptor {
            name: receptor.name.clone(),
            field: "lat",
            value: receptor.lat,
            reason: "must be in [-90, 90]",
        });
    }
    if !receptor.z_m.is_finite() || receptor.z_m < 0.0 {
        return Err(TimeLoopError::InvalidReceptor {
            name: receptor.name.clone(),
            field: "z_m",
            value: receptor.z_m,
            reason: "must be finite and >= 0",
        });
    }
    if receptor.particle_count == 0 {
        return Err(TimeLoopError::InvalidReceptor {
            name: receptor.name.clone(),
            field: "particle_count",
            value: 0.0,
            reason: "must be > 0",
        });
    }
    if !receptor.mass_kg.is_finite() || receptor.mass_kg <= 0.0 {
        return Err(TimeLoopError::InvalidReceptor {
            name: receptor.name.clone(),
            field: "mass_kg",
            value: receptor.mass_kg,
            reason: "must be finite and > 0",
        });
    }
    Ok(())
}

fn validate_source_region(source: &BackwardSourceRegionConfig) -> Result<(), TimeLoopError> {
    if source.lon_min > source.lon_max {
        return Err(TimeLoopError::InvalidSourceBounds {
            name: source.name.clone(),
            field: "longitude",
            min: source.lon_min,
            max: source.lon_max,
        });
    }
    if source.lat_min > source.lat_max {
        return Err(TimeLoopError::InvalidSourceBounds {
            name: source.name.clone(),
            field: "latitude",
            min: source.lat_min,
            max: source.lat_max,
        });
    }
    if source.z_min_m > source.z_max_m {
        return Err(TimeLoopError::InvalidSourceBounds {
            name: source.name.clone(),
            field: "height",
            min: source.z_min_m,
            max: source.z_max_m,
        });
    }
    Ok(())
}
