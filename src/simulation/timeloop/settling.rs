//! Run-owned original settling flag and zero-by-carrier selection for #214.

use super::config::TimeDirection;
use crate::config::{ReleaseConfig, SpeciesConfig};
use crate::gpu::meteorology::MeteorologyCompositionError;
use crate::gpu::GpuContext;
use crate::particles::MAX_SPECIES;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Explicit original run options and positionally bound #10 species table.
/// Missing species identity never implies disabled settling.
#[derive(Debug, Clone, Default)]
pub struct TransportSettlingConfig {
    /// Species in particle mass-slot order, including backward receptor carriers.
    pub species: Vec<SpeciesConfig>,
    /// Original MQUASILAG; only zero is currently supported.
    pub mquasilag: i32,
    /// Original MDOMAINFILL; only zero is currently supported.
    pub domain_fill: i32,
    /// Original IPIN; restart modes are not supported.
    pub restart: i32,
}

/// Accepted velocity ownership; neither variant computes a velocity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PetterssenSettlingInput {
    Disabled,
    ZeroByCarrier,
}

/// Immutable run flag, normalized release/carrier provenance and device identity.
#[derive(Debug, Clone)]
pub(crate) struct TransportSettlingRoute {
    context: Arc<()>,
    direction: TimeDirection,
    input: PetterssenSettlingInput,
    // Run provenance is retained for the explicit validation/output consumer.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) provenance_sha256: String,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) provenance_json: String,
}

/// Ported from readoptions_mod.f90:2574-2585; eps2 is an original f32 constant.
fn original_enabled(masses: &[Vec<f64>], species: usize, mquasilag: i32) -> bool {
    const RELEASE_THRESHOLD_KG: f64 = 1.0e-9_f32 as f64;
    if mquasilag != 0 {
        return species <= 1;
    }
    !masses.iter().any(|row| {
        row.iter()
            .filter(|mass| **mass > RELEASE_THRESHOLD_KG)
            .count()
            > 1
    })
}

/// Preserve the original zero-density branch while retaining #10 parameter validation.
fn validate_carriers(species: &[SpeciesConfig]) -> Result<(), MeteorologyCompositionError> {
    let invalid = MeteorologyCompositionError::Incompatible;
    let mut names = std::collections::BTreeSet::new();
    for carrier in species {
        if !names.insert(&carrier.name) || carrier.name.trim().is_empty() {
            return Err(invalid("ambiguous settling carrier identity"));
        }
        // #10 normalizes inactive density sentinels to None. Preserve the explicit
        // original density for this branch; other #10 validation still applies.
        let density = carrier
            .particle_density_kg_m3
            .or_else(|| {
                carrier
                    .raw
                    .get("pdensity")
                    .or_else(|| carrier.raw.get("density"))
                    .and_then(|value| value.replace(['D', 'd'], "e").parse::<f64>().ok())
            })
            .ok_or_else(|| invalid("missing explicit settling carrier density"))?;
        if !density.is_finite() {
            return Err(invalid("nonfinite settling carrier density"));
        }
        if carrier
            .raw
            .get("pshape")
            .is_some_and(|shape| shape.parse::<i32>() != Ok(0))
        {
            return Err(invalid("unsupported settling carrier shape"));
        }
        let mut validated = carrier.clone();
        if density <= 0.0 {
            validated.particle_density_kg_m3 = None;
        }
        validated
            .validate()
            .map_err(|_| invalid("invalid settling carrier parameters"))?;
        if density > 0.0 || carrier.mean_diameter_um.is_some() {
            return Err(MeteorologyCompositionError::Unsupported("nonzero/Produced settling velocity requires #35/#37; mixed gas/aerosol routes unsupported"));
        }
    }
    Ok(())
}

impl TransportSettlingRoute {
    /// Freeze original run-wide enablement before release, using positional species ownership.
    pub(crate) fn new(
        ctx: &GpuContext,
        config: &TransportSettlingConfig,
        releases: &[ReleaseConfig],
        direction: TimeDirection,
    ) -> Result<Self, MeteorologyCompositionError> {
        let invalid = MeteorologyCompositionError::Incompatible;
        if config.species.is_empty() || config.species.len() > MAX_SPECIES || releases.is_empty() {
            return Err(invalid(
                "settling requires unambiguous release/species ownership",
            ));
        }
        validate_carriers(&config.species)?;
        let mut masses = Vec::new();
        for release in releases {
            release
                .validate()
                .map_err(|_| invalid("invalid settling release/source binding"))?;
            if !release.z_min.is_finite()
                || !release.z_max.is_finite()
                || !release.mass_kg.is_finite()
            {
                return Err(invalid("nonfinite settling source input"));
            }
            let row = release
                .species_masses_kg
                .clone()
                .unwrap_or_else(|| vec![release.mass_kg]);
            if row.len() != config.species.len()
                || row.iter().any(|v| !v.is_finite() || *v < 0.0)
                || !row.iter().any(|v| *v > f64::from(f32::MIN_POSITIVE))
            {
                return Err(invalid("invalid release/carrier mass-slot binding"));
            }
            #[allow(clippy::cast_possible_truncation)]
            // Original xmass and eps2 are default-real f32.
            let original_row: Vec<_> = row.iter().map(|mass| f64::from(*mass as f32)).collect();
            if original_row.iter().any(|v| !v.is_finite())
                || !original_row
                    .iter()
                    .any(|v| *v > f64::from(f32::MIN_POSITIVE))
            {
                return Err(invalid("release mass outside original f32 domain"));
            }
            masses.push(original_row);
        }
        let enabled = original_enabled(&masses, config.species.len(), config.mquasilag);
        if config.mquasilag != 0 || config.domain_fill != 0 || config.restart != 0 {
            return Err(MeteorologyCompositionError::Unsupported(
                "MQUASILAG/domain-fill/restart settling route",
            ));
        }
        let bytes = serde_json::to_vec(&(
            releases,
            &config.species,
            config.mquasilag,
            config.domain_fill,
            config.restart,
            direction == TimeDirection::Backward,
        ))
        .map_err(|_| invalid("settling provenance serialization"))?;
        Ok(Self {
            context: Arc::clone(&ctx.identity),
            direction,
            input: if enabled {
                PetterssenSettlingInput::ZeroByCarrier
            } else {
                PetterssenSettlingInput::Disabled
            },
            provenance_sha256: format!("{:x}", Sha256::digest(&bytes)),
            provenance_json: String::from_utf8(bytes)
                .map_err(|_| invalid("settling provenance UTF-8"))?,
        })
    }

    /// Validate the run/device/direction identity before any timestep command encoding.
    pub(crate) fn stop_route(
        &self,
        ctx: &GpuContext,
        signed_dt: f32,
    ) -> Result<bool, MeteorologyCompositionError> {
        if !Arc::ptr_eq(&self.context, &ctx.identity)
            || !signed_dt.is_finite()
            || signed_dt == 0.0
            || (signed_dt < 0.0) != (self.direction == TimeDirection::Backward)
        {
            return Err(MeteorologyCompositionError::Incompatible(
                "settling route context/direction identity",
            ));
        }
        Ok(self.input == PetterssenSettlingInput::ZeroByCarrier
            && self.direction == TimeDirection::Backward)
    }
}

#[cfg(test)]
mod tests {
    use super::original_enabled;
    #[test]
    fn test_settling_original_run_global_flag_threshold_and_mquasilag() {
        assert!(original_enabled(&[vec![1.0]], 1, 0));
        let threshold = f64::from(1.0e-9_f32);
        assert!(original_enabled(&[vec![1.0, threshold]], 2, 0));
        assert!(!original_enabled(
            &[vec![1.0, 0.0], vec![1.0, threshold + f64::EPSILON]],
            2,
            0
        ));
        assert!(original_enabled(&[vec![1.0, 0.0], vec![0.0, 1.0]], 2, 0));
        assert!(!original_enabled(&[vec![1.0, 0.0]], 2, 1));
        assert!(original_enabled(&[vec![1.0]], 1, 1));
    }
    #[test]
    fn test_settling_route_carriers_bindings_and_context_fail_closed() {
        use super::*;
        let ctx = pollster::block_on(GpuContext::new()).expect("required route adapter");
        let gas: SpeciesConfig = serde_json::from_value(
            serde_json::json!({"name":"gas","particle_density_kg_m3":-1.0,"raw":{"pdensity":"-1"}}),
        )
        .expect("gas");
        let config = TransportSettlingConfig {
            species: vec![gas],
            ..Default::default()
        };
        let release:ReleaseConfig=serde_json::from_value(serde_json::json!({"name":"source","start_time":"20240101000000","end_time":"20240101000000","lon":0.25,"lat":0.0,"z_min":1.0,"z_max":1.0,"mass_kg":1.0,"particle_count":1,"raw":{}})).expect("release");
        let route = TransportSettlingRoute::new(
            &ctx,
            &config,
            std::slice::from_ref(&release),
            TimeDirection::Backward,
        )
        .expect("explicit gas");
        assert!(route.stop_route(&ctx, -1.0).expect("same context"));
        assert!(route.stop_route(&ctx, 1.0).is_err());
        assert!(route.stop_route(&ctx, f32::NAN).is_err());
        let other = pollster::block_on(GpuContext::new()).expect("second required context");
        assert!(route.stop_route(&other, -1.0).is_err());
        let forward = TransportSettlingRoute::new(
            &ctx,
            &config,
            std::slice::from_ref(&release),
            TimeDirection::Forward,
        )
        .expect("forward");
        assert!(!forward.stop_route(&ctx, 1.0).expect("no forward stop"));
        assert!(TransportSettlingRoute::new(
            &ctx,
            &TransportSettlingConfig::default(),
            std::slice::from_ref(&release),
            TimeDirection::Backward
        )
        .is_err());
        for option in 0..9 {
            let mut invalid = config.clone();
            match option {
                0 => invalid.mquasilag = 1,
                1 => invalid.domain_fill = 1,
                2 => invalid.restart = 3,
                3 => {
                    invalid.species[0].particle_density_kg_m3 = None;
                    invalid.species[0].raw.clear();
                }
                4 => invalid.species[0].particle_density_kg_m3 = Some(f64::NAN),
                5 => invalid.species[0].particle_density_kg_m3 = Some(2500.0),
                6 => invalid.species.push(invalid.species[0].clone()),
                7 => invalid.species[0].mean_diameter_um = Some(f64::INFINITY),
                _ => invalid.species[0].name.clear(),
            }
            assert!(
                TransportSettlingRoute::new(
                    &ctx,
                    &invalid,
                    std::slice::from_ref(&release),
                    TimeDirection::Backward
                )
                .is_err(),
                "mutation {option}"
            );
        }
        for mass in [f64::NAN, f64::INFINITY, -1.0, 0.0, f64::MAX] {
            let mut invalid = release.clone();
            invalid.mass_kg = mass;
            assert!(TransportSettlingRoute::new(
                &ctx,
                &config,
                &[invalid],
                TimeDirection::Backward
            )
            .is_err());
        }
        let mut two = config.clone();
        let mut second = two.species[0].clone();
        second.name = "gas2".into();
        two.species.push(second);
        let mut multi = release.clone();
        multi.species_masses_kg = Some(vec![0.5, 0.5]);
        let disabled = TransportSettlingRoute::new(&ctx, &two, &[multi], TimeDirection::Backward)
            .expect("original run-wide disable");
        assert!(!disabled.stop_route(&ctx, -1.0).expect("disabled"));
    }
}
