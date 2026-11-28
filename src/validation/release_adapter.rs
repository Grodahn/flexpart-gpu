//! Narrow #51 declaration adapter into the production release contract.

use crate::config::{resolve_flexpart_species, ConfigError, SpeciesConfig};
use crate::release::{
    invalid_release, validate_release_bindings, ReleaseGeometry, ReleaseHeight, ReleaseHeightRef,
    ReleaseHorizontalRef, ReleaseInventory, ReleaseProfile, ReleaseQuantity, ReleaseSource,
    ReleaseSpec, ReleaseSpecFields, ReleaseTime,
};

use super::case::{
    HorizontalCoordRef, MassUnit, ReleaseTiming, SimulationDirection, SourceGeometry,
    ValidationCaseManifest, VerticalRef, VALIDATION_CASE_SCHEMA_VERSION,
};

impl ValidationCaseManifest {
    /// Bind a validated v2 forward source declaration to the production species table.
    ///
    /// # Errors
    /// Returns a configuration error if the case or its production release binding is invalid.
    pub fn normalize_release(&self, species: &[SpeciesConfig]) -> Result<ReleaseSpec, ConfigError> {
        self.validate()
            .map_err(|error| invalid_release(error.to_string()))?;
        if self.schema_version != VALIDATION_CASE_SCHEMA_VERSION
            || !matches!(self.simulation_direction, SimulationDirection::Forward)
        {
            return Err(invalid_release(
                "ReleaseSpec requires a v2 forward validation case",
            ));
        }
        let horizontal_ref = match self.domain.horizontal_ref {
            HorizontalCoordRef::GeographicLonLatDegrees => {
                ReleaseHorizontalRef::GeographicWgs84Degrees
            }
        };
        let MassUnit::Kg = self.release.inventory.unit;
        let release = &self.release;
        let datum = match release.vertical_ref {
            VerticalRef::Agl => ReleaseHeightRef::Agl,
            VerticalRef::Asl => ReleaseHeightRef::Asl,
        };
        let (geometry, height) = match release.geometry {
            SourceGeometry::Point {
                lon_deg,
                lat_deg,
                z_m,
            } => (
                ReleaseGeometry::Point {
                    lon_deg: f64::from(lon_deg),
                    lat_deg: f64::from(lat_deg),
                },
                ReleaseHeight::Fixed {
                    reference: datum,
                    height_m: f64::from(z_m),
                },
            ),
            SourceGeometry::Box {
                lon_min_deg,
                lon_max_deg,
                lat_min_deg,
                lat_max_deg,
                z_min_m,
                z_max_m,
            } => (
                ReleaseGeometry::from_bounds(lon_min_deg, lon_max_deg, lat_min_deg, lat_max_deg),
                ReleaseHeight::from_bounds(datum, f64::from(z_min_m), f64::from(z_max_m)),
            ),
        };
        let profile = match &release.timing {
            ReleaseTiming::Instant { at } => ReleaseProfile::Instant {
                at: ReleaseTime::parse_utc(at)?,
            },
            ReleaseTiming::Window { start, end } => ReleaseProfile::UniformWindow {
                start: ReleaseTime::parse_utc(start)?,
                end: ReleaseTime::parse_utc(end)?,
            },
        };
        let number = release
            .species
            .id
            .strip_prefix("SPECIES_")
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| invalid_release("invalid validation release species ID"))?;
        let slot = resolve_flexpart_species(number, species)?;
        let spec = ReleaseSpec::new(ReleaseSpecFields {
            id: self.case_id.clone(),
            profile,
            horizontal_ref,
            geometry,
            height,
            inventories: vec![ReleaseInventory {
                species_id: species[slot].name.clone(),
                mass_slot: u8::try_from(slot)
                    .map_err(|_| invalid_release("invalid validation species slot"))?,
                quantity: ReleaseQuantity::Mass {
                    kg: f64::from(release.inventory.quantity_kg),
                },
                conversion: None,
            }],
            particle_count: u64::from(release.particle_count),
            sampling_seed: None,
            source: ReleaseSource::ValidationCaseV2 {
                case_id: self.case_id.clone(),
            },
        })?;
        validate_release_bindings(std::slice::from_ref(&spec), species)?;
        Ok(spec)
    }
}
