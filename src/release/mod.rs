//! Particle release manager (source term injection).
//!
//! MVP port of FLEXPART `releaseparticles.f90` intent:
//! - activate releases only within configured time windows
//! - distribute particle counts over the active window
//! - sample deterministic positions in source lon/lat/z bounds
//! - inject particles into [`crate::particles::ParticleStore`]
//! - optionally upload only changed slots to GPU storage buffers
//!
use thiserror::Error;

mod legacy;
mod spec;

use crate::coords::{geo_to_grid, GeoCoord, GridDomain};
use crate::gpu::{GpuBufferError, GpuContext, ParticleBuffers};
use crate::particles::{Particle, ParticleError, ParticleInit, ParticleStore, MAX_SPECIES};
pub use legacy::LegacyReleaseConfig;
pub(crate) use spec::{invalid_release, validate_release_species, GRAMS_PER_KILOGRAM};
pub use spec::{
    validate_release_bindings, ReleaseConversion, ReleaseGeometry, ReleaseHeight, ReleaseHeightRef,
    ReleaseHorizontalRef, ReleaseInventory, ReleaseProfile, ReleaseQuantity, ReleaseSource,
    ReleaseSpec, ReleaseSpecFields, ReleaseTime,
};

/// Result of one release step at a given simulation timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseStepReport {
    /// Number of particles released during this step.
    pub released_count: usize,
    /// Particle slot indices touched in [`ParticleStore`].
    pub released_slots: Vec<usize>,
}

/// Errors produced by the particle release manager.
#[derive(Debug, Error)]
pub enum ReleaseError {
    #[error("invalid timestamp `{value}`: expected 14 digits YYYYMMDDHHMMSS")]
    InvalidTimestamp { value: String },
    #[error("invalid bounds for release `{release}`: {field} min {min} > max {max}")]
    InvalidBounds {
        release: String,
        field: &'static str,
        min: f64,
        max: f64,
    },
    #[error("release `{release}` sampled outside grid domain at lon={lon}, lat={lat}")]
    OutOfGridDomain { release: String, lon: f64, lat: f64 },
    #[error("timestamp `{value}` cannot be represented as i32 simulation seconds")]
    TimeOutOfRange { value: String },
    #[error("release configuration: {0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("particle release failed: {0}")]
    Particle(#[from] ParticleError),
    #[error("gpu upload failed: {0}")]
    Gpu(#[from] GpuBufferError),
}

/// Manages source-term activation and particle injection over time.
///
/// Each [`LegacyReleaseConfig`] is tracked independently with a cumulative emitted
/// count. Calls to [`inject_for_time`](Self::inject_for_time) are deterministic:
/// the same sequence of timestamps always yields the same particle positions.
#[derive(Clone)]
pub struct ReleaseManager {
    grid: GridDomain,
    states: Vec<ReleaseState>,
}

impl ReleaseManager {
    /// Build a release manager from parsed release configs.
    ///
    /// `grid` is used to convert lon/lat source coordinates into particle
    /// relative grid coordinates (`cell_x/cell_y + frac`).
    pub(crate) fn new_legacy(
        releases: &[LegacyReleaseConfig],
        grid: GridDomain,
    ) -> Result<Self, ReleaseError> {
        let mut states = Vec::with_capacity(releases.len());
        for (release_idx, release) in releases.iter().enumerate() {
            let bounds = ReleaseBounds::from_config(release)?;
            let start_seconds = parse_timestamp_seconds(&release.start_time)?;
            let end_seconds = parse_timestamp_seconds(&release.end_time)?;
            states.push(ReleaseState {
                name: release.name.clone(),
                particle_count: release.particle_count,
                masses_kg: release.legacy_mass_row(),
                bounds,
                start_seconds,
                end_seconds,
                released_count: 0,
                release_point: i32::try_from(release_idx).map_err(|_| {
                    ReleaseError::TimeOutOfRange {
                        value: format!("release index {release_idx}"),
                    }
                })?,
            });
        }
        Ok(Self { grid, states })
    }

    /// Construct the existing schedule from canonical inputs before allocating a particle store.
    pub(crate) fn from_specs(
        releases: &[ReleaseSpec],
        grid: GridDomain,
    ) -> Result<Self, ReleaseError> {
        if grid.nx < 2
            || grid.ny < 2
            || !grid.xlon0.is_finite()
            || !grid.ylat0.is_finite()
            || !grid.dx.is_finite()
            || !grid.dy.is_finite()
            || grid.dx <= 0.0
            || grid.dy <= 0.0
        {
            return Err(invalid_release("invalid forward release grid").into());
        }
        let states = releases
            .iter()
            .enumerate()
            .map(|(index, source)| ReleaseState::from_spec(source, &grid, index))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { grid, states })
    }

    /// Emit particles at `timestamp` and inject them into `store`.
    ///
    /// The timestamp must be normalized as `YYYYMMDDHHMMSS`.
    pub fn inject_for_time(
        &mut self,
        timestamp: &str,
        store: &mut ParticleStore,
    ) -> Result<ReleaseStepReport, ReleaseError> {
        let now_seconds = parse_timestamp_seconds(timestamp)?;
        let particle_time =
            i32::try_from(now_seconds).map_err(|_| ReleaseError::TimeOutOfRange {
                value: timestamp.to_string(),
            })?;

        let mut released_slots = Vec::new();

        for state in &mut self.states {
            let target_count = state.target_emitted_count(now_seconds);
            if target_count <= state.released_count {
                continue;
            }

            let to_emit = target_count - state.released_count;
            let mass_per_slot = state.mass_per_species_slot(state.particle_count);

            for offset in 0..to_emit {
                let sample_index = state.released_count + offset;
                let (lon, lat, z_m) = state
                    .bounds
                    .sample_stratified(sample_index, state.particle_count);
                let grid_coord = geo_to_grid(GeoCoord { lat, lon }, &self.grid);
                if !self.grid.contains(&grid_coord) {
                    return Err(ReleaseError::OutOfGridDomain {
                        release: state.name.clone(),
                        lon,
                        lat,
                    });
                }
                let rel = grid_coord.to_relative();

                let particle = Particle::new(&ParticleInit {
                    cell_x: rel.cell_x,
                    cell_y: rel.cell_y,
                    pos_x: rel.frac_x,
                    pos_y: rel.frac_y,
                    pos_z: z_m as f32,
                    mass: mass_per_slot,
                    release_point: state.release_point,
                    class: 0,
                    time: particle_time,
                });
                let slot = store.add(particle)?;
                released_slots.push(slot);
            }

            state.released_count = target_count;
        }

        Ok(ReleaseStepReport {
            released_count: released_slots.len(),
            released_slots,
        })
    }

    /// Prepare a forward source batch without publishing slots or advancing the schedule.
    pub(crate) fn prepare_batch(
        &self,
        timestamp: &str,
        available: usize,
    ) -> Result<Option<(Self, ParticleStore)>, ReleaseError> {
        let now_seconds = parse_timestamp_seconds(timestamp)?;
        let count = self.states.iter().try_fold(0_usize, |count, state| {
            let due = state.target_emitted_count(now_seconds) - state.released_count;
            usize::try_from(due)
                .ok()
                .and_then(|due| count.checked_add(due))
        });
        let count = count
            .filter(|count| *count <= available)
            .ok_or(ParticleError::StoreFull {
                capacity: available,
            })?;
        if count == 0 {
            return Ok(None);
        }
        let mut schedule = self.clone();
        let mut batch = ParticleStore::with_capacity(count);
        schedule.inject_for_time(timestamp, &mut batch)?;
        Ok(Some((schedule, batch)))
    }

    /// Emit particles and upload only newly touched slots to the GPU particle buffer.
    pub fn inject_and_upload_for_time(
        &mut self,
        timestamp: &str,
        store: &mut ParticleStore,
        buffers: &ParticleBuffers,
        ctx: &GpuContext,
    ) -> Result<ReleaseStepReport, ReleaseError> {
        let report = self.inject_for_time(timestamp, store)?;
        if !report.released_slots.is_empty() {
            buffers.upload_particle_slots(ctx, store, &report.released_slots)?;
        }
        Ok(report)
    }
}

#[derive(Clone)]
struct ReleaseState {
    name: String,
    particle_count: u64,
    masses_kg: [f64; MAX_SPECIES],
    bounds: ReleaseBounds,
    start_seconds: i64,
    end_seconds: i64,
    released_count: u64,
    release_point: i32,
}

impl ReleaseState {
    fn from_spec(
        release: &ReleaseSpec,
        grid: &GridDomain,
        index: usize,
    ) -> Result<Self, ReleaseError> {
        let invalid = |message| invalid_release(format!("release `{}`: {message}", release.id));
        let (datum, z_min, z_max) = release.height.bounds();
        if datum != ReleaseHeightRef::Agl || z_max > f64::from(f32::MAX) {
            return Err(
                invalid("existing scheduler requires AGL heights representable in f32").into(),
            );
        }
        if release.sampling_seed.is_some() {
            return Err(invalid("explicit sampling seeds require #27").into());
        }
        let (lon_min, lon_max, lat_min, lat_max) = release.geometry.bounds();
        for (lon, lat) in [(lon_min, lat_min), (lon_max, lat_max)] {
            if !grid.contains(&geo_to_grid(GeoCoord { lat, lon }, grid)) {
                return Err(ReleaseError::OutOfGridDomain {
                    release: release.id.clone(),
                    lon,
                    lat,
                });
            }
        }
        let (start, end) = release.profile.bounds();
        let start_seconds = start.unix_seconds();
        let end_seconds = end.unix_seconds();
        if i32::try_from(start_seconds).is_err() || i32::try_from(end_seconds).is_err() {
            return Err(ReleaseError::TimeOutOfRange {
                value: release.id.clone(),
            });
        }
        let state = Self {
            name: release.id.clone(),
            particle_count: release.particle_count,
            masses_kg: release_mass_row(release)?,
            bounds: ReleaseBounds {
                lon_min,
                lon_max,
                lat_min,
                lat_max,
                z_min,
                z_max,
            },
            start_seconds,
            end_seconds,
            released_count: 0,
            release_point: i32::try_from(index)
                .map_err(|_| invalid("index exceeds the particle layout"))?,
        };
        if state
            .mass_per_species_slot(state.particle_count)
            .iter()
            .zip(state.masses_kg)
            .any(|(&mass, total)| !mass.is_finite() || (total > 0.0 && mass == 0.0))
        {
            return Err(invalid("inventory outside existing f32 particle mass range").into());
        }
        Ok(state)
    }

    fn target_emitted_count(&self, now_seconds: i64) -> u64 {
        if now_seconds < self.start_seconds || now_seconds > self.end_seconds {
            return self.released_count;
        }
        if self.start_seconds == self.end_seconds {
            return self.particle_count;
        }
        if now_seconds == self.end_seconds {
            return self.particle_count;
        }

        let elapsed = (now_seconds - self.start_seconds) as u64;
        let duration = (self.end_seconds - self.start_seconds) as u64;
        ((u128::from(self.particle_count) * u128::from(elapsed)) / u128::from(duration)) as u64
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn mass_per_species_slot(&self, particle_count: u64) -> [f32; MAX_SPECIES] {
        let count = particle_count.max(1) as f64;
        self.masses_kg.map(|mass| (mass / count) as f32)
    }
}

pub(crate) fn release_mass_row(
    spec: &ReleaseSpec,
) -> Result<[f64; MAX_SPECIES], crate::config::ConfigError> {
    let mut masses = [0.0; MAX_SPECIES];
    for item in &spec.inventories {
        let ReleaseQuantity::Mass { kg } = item.quantity else {
            return Err(invalid_release(format!(
                "release `{}`: activity cannot be injected as mass",
                spec.id
            )));
        };
        masses[usize::from(item.mass_slot)] = kg;
    }
    Ok(masses)
}

#[derive(Clone)]
struct ReleaseBounds {
    lon_min: f64,
    lon_max: f64,
    lat_min: f64,
    lat_max: f64,
    z_min: f64,
    z_max: f64,
}

impl ReleaseBounds {
    fn from_config(config: &LegacyReleaseConfig) -> Result<Self, ReleaseError> {
        let (lon_min, lon_max, lat_min, lat_max) = config.legacy_horizontal_bounds();
        let z_min = config.z_min;
        let z_max = config.z_max;

        if lon_min > lon_max {
            return Err(ReleaseError::InvalidBounds {
                release: config.name.clone(),
                field: "longitude",
                min: lon_min,
                max: lon_max,
            });
        }
        if lat_min > lat_max {
            return Err(ReleaseError::InvalidBounds {
                release: config.name.clone(),
                field: "latitude",
                min: lat_min,
                max: lat_max,
            });
        }
        if z_min > z_max {
            return Err(ReleaseError::InvalidBounds {
                release: config.name.clone(),
                field: "height",
                min: z_min,
                max: z_max,
            });
        }

        Ok(Self {
            lon_min,
            lon_max,
            lat_min,
            lat_max,
            z_min,
            z_max,
        })
    }

    fn sample_stratified(&self, sample_index: u64, total_samples: u64) -> (f64, f64, f64) {
        let nx = (total_samples as f64).cbrt().ceil().max(1.0) as u64;
        let ny = nx;
        let nxy = nx.saturating_mul(ny).max(1);
        let nz = total_samples.div_ceil(nxy).max(1);

        let ix = sample_index % nx;
        let iy = (sample_index / nx) % ny;
        let iz = (sample_index / nxy) % nz;

        let fx = (ix as f64 + 0.5) / nx as f64;
        let fy = (iy as f64 + 0.5) / ny as f64;
        let fz = (iz as f64 + 0.5) / nz as f64;

        let lon = self.lon_min + fx * (self.lon_max - self.lon_min);
        let lat = self.lat_min + fy * (self.lat_max - self.lat_min);
        let z_m = self.z_min + fz * (self.z_max - self.z_min);
        (lon, lat, z_m)
    }
}

fn parse_timestamp_seconds(value: &str) -> Result<i64, ReleaseError> {
    if value.len() != 14 {
        return Err(ReleaseError::InvalidTimestamp {
            value: value.to_owned(),
        });
    }
    ReleaseTime::parse_utc(value)
        .map(ReleaseTime::unix_seconds)
        .map_err(|_| ReleaseError::InvalidTimestamp {
            value: value.to_owned(),
        })
}

/// Gregorian civil date to days since Unix epoch (1970-01-01).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let mut y = year;
    let m = month;
    let d = day;

    if m <= 2 {
        y -= 1;
    }
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn sample_release() -> LegacyReleaseConfig {
        let mut raw = BTreeMap::new();
        raw.insert("lon_min".to_string(), "10.0".to_string());
        raw.insert("lon_max".to_string(), "12.0".to_string());
        raw.insert("lat_min".to_string(), "45.0".to_string());
        raw.insert("lat_max".to_string(), "46.0".to_string());

        LegacyReleaseConfig {
            name: "r1".to_string(),
            start_time: "20240101010000".to_string(),
            end_time: "20240101020000".to_string(),
            lon: 11.0,
            lat: 45.5,
            z_min: 100.0,
            z_max: 200.0,
            mass_kg: 1.0,
            particle_count: 12,
            species_masses_kg: None,
            raw,
        }
    }

    fn sample_grid() -> GridDomain {
        GridDomain {
            xlon0: 0.0,
            ylat0: 0.0,
            dx: 1.0,
            dy: 1.0,
            nx: 40,
            ny: 80,
        }
    }

    #[test]
    fn no_release_outside_time_window() {
        let release = sample_release();
        let mut manager = ReleaseManager::new_legacy(&[release], sample_grid()).expect("manager");
        let mut store = ParticleStore::with_capacity(32);

        let before = manager
            .inject_for_time("20240101005959", &mut store)
            .expect("before window");
        assert_eq!(before.released_count, 0);

        let after = manager
            .inject_for_time("20240101020001", &mut store)
            .expect("after window");
        assert_eq!(after.released_count, 0);
        assert_eq!(store.active_count(), 0);
    }

    #[test]
    fn correct_count_released_inside_window() {
        let release = sample_release();
        let mut manager = ReleaseManager::new_legacy(&[release], sample_grid()).expect("manager");
        let mut store = ParticleStore::with_capacity(32);

        let quarter = manager
            .inject_for_time("20240101011500", &mut store)
            .expect("quarter point");
        assert_eq!(quarter.released_count, 3);

        let three_quarter = manager
            .inject_for_time("20240101014500", &mut store)
            .expect("three-quarter point");
        assert_eq!(three_quarter.released_count, 6);

        let end = manager
            .inject_for_time("20240101020000", &mut store)
            .expect("end point");
        assert_eq!(end.released_count, 3);
        assert_eq!(store.active_count(), 12);
    }

    #[test]
    fn released_positions_stay_within_bounds() {
        let release = sample_release();
        let mut manager = ReleaseManager::new_legacy(&[release], sample_grid()).expect("manager");
        let mut store = ParticleStore::with_capacity(32);

        let report = manager
            .inject_for_time("20240101020000", &mut store)
            .expect("emit full release");
        assert_eq!(report.released_count, 12);

        for slot in report.released_slots {
            let particle = store.get(slot).expect("valid slot");
            let lon = f64::from(particle.cell_x) + f64::from(particle.pos_x);
            let lat = f64::from(particle.cell_y) + f64::from(particle.pos_y);
            assert!((10.0..=12.0).contains(&lon), "lon out of bounds: {lon}");
            assert!((45.0..=46.0).contains(&lat), "lat out of bounds: {lat}");
            assert!(
                (100.0..=200.0).contains(&f64::from(particle.pos_z)),
                "z out of bounds: {}",
                particle.pos_z
            );
        }
    }

    #[test]
    fn legacy_release_puts_all_mass_into_slot_zero() {
        let release = sample_release();
        let mut manager = ReleaseManager::new_legacy(&[release], sample_grid()).expect("manager");
        let mut store = ParticleStore::with_capacity(32);

        let report = manager
            .inject_for_time("20240101020000", &mut store)
            .expect("emit full release");
        assert_eq!(report.released_count, 12);

        // mass_kg=1 over 12 particles; unused slots stay zero.
        for slot in report.released_slots {
            let particle = store.get(slot).expect("valid slot");
            assert!((particle.mass[0] - 1.0 / 12.0).abs() < 1.0e-7);
            for mass in particle.mass.iter().skip(1) {
                assert!(mass.abs() < f32::EPSILON);
            }
        }
    }

    #[test]
    fn multi_species_release_distributes_mass_per_slot() {
        let mut release = sample_release();
        release.species_masses_kg = Some(vec![1.0, 0.5, 0.25]);
        let mut manager = ReleaseManager::new_legacy(&[release], sample_grid()).expect("manager");
        let mut store = ParticleStore::with_capacity(32);

        let report = manager
            .inject_for_time("20240101020000", &mut store)
            .expect("emit full release");
        assert_eq!(report.released_count, 12);

        for slot in report.released_slots {
            let particle = store.get(slot).expect("valid slot");
            assert!((particle.mass[0] - 1.0 / 12.0).abs() < 1.0e-7);
            assert!((particle.mass[1] - 0.5 / 12.0).abs() < 1.0e-7);
            assert!((particle.mass[2] - 0.25 / 12.0).abs() < 1.0e-7);
            assert!(particle.mass[3].abs() < f32::EPSILON);
        }
    }
}
