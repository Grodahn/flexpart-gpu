//! Per-species parameter mapping for multi-nuclide forcing.
//!
//! Ported from FLEXPART pathway branching:
//! - `readoptions_mod.f90:2306-2394` (`readreleases` species setup: gas vs
//!   aerosol classification, `DRYDEPSPEC`/`WETDEPSPEC` flags, unit conversions)
//! - `readoptions_mod.f90:3087-3091` (gas wet removal requires Henry constant)
//! - `drydepo_mod.f90:337-338` (radioactive decay factor `exp(-|dt|*decay)`)
//!
//! These pure mappings convert [`SpeciesConfig`] into the existing physics
//! input structs without meteo state. Callers combine the returned parameters
//! with per-step meteo (friction velocity, stability, precipitation) using the
//! resistance and scavenging formulas in [`crate::physics::deposition`] and
//! [`crate::physics::wet_scavenging`] to build per-species forcing vectors.

use crate::config::{SpeciesConfig, SpeciesKind};
use crate::particles::MAX_SPECIES;
use crate::physics::deposition::GasSpeciesDepositionInput;
use crate::physics::wet_scavenging::{
    AerosolBelowCloudParams, AerosolInCloudParams, GasBelowCloudParams, GasInCloudParams,
};

/// Convert a validated finite `f64` species parameter to `f32`.
///
/// Species parameters arrive as `f64` from config parsing; physics and GPU
/// buffers use `f32` throughout (Monte Carlo convergence dominates over float
/// precision, see `AGENTS.md`). Non-finite inputs saturate to `0` (disabled).
#[allow(clippy::cast_possible_truncation)]
fn to_f32(value: f64) -> f32 {
    if value.is_finite() {
        value as f32
    } else {
        0.0
    }
}

/// Gas dry-deposition input for one species.
///
/// Returns `Some` when the gas resistance pathway is enabled
/// (`PRELDIFF > 0`). Values `<= 0` mean "not gas-parameterized" downstream.
#[must_use]
pub fn gas_deposition_input(species: &SpeciesConfig) -> Option<GasSpeciesDepositionInput> {
    let reldiff = species.relative_diffusivity?;
    if reldiff <= 0.0 {
        return None;
    }
    Some(GasSpeciesDepositionInput {
        relative_diffusivity_to_h2o: to_f32(reldiff),
        constant_dry_velocity_m_s: species
            .dry_deposition_velocity
            .filter(|v| *v > 0.0)
            .map(to_f32),
    })
}

/// Gas below-cloud scavenging parameters (`weta_gas`, `wetb_gas`).
///
/// Returns `Some` when gas below-cloud removal is enabled (either coefficient
/// positive), mirroring the `WETDEPSPEC` flag
/// (`readoptions_mod.f90:2363-2364`). Missing partners default to `0`, which
/// the scavenging formula sanitizes exactly like the Fortran sentinels.
#[must_use]
pub fn gas_below_cloud_params(species: &SpeciesConfig) -> Option<GasBelowCloudParams> {
    if species.species_kind() != SpeciesKind::Gas {
        return None;
    }
    if !species.below_cloud_wet_active() {
        return None;
    }
    Some(GasBelowCloudParams {
        coefficient_a: species.wet_a_gas.map_or(0.0, to_f32),
        exponent_b: species.wet_b_gas.map_or(0.0, to_f32),
    })
}

/// Aerosol below-cloud scavenging parameters.
///
/// Returns `Some` when aerosol below-cloud removal is enabled
/// (`PCRAIN_AERO > 0` or `PCSNOW_AERO > 0`). A non-positive diameter yields
/// `0` and disables the polynomial branch downstream.
#[must_use]
pub fn aerosol_below_cloud_params(species: &SpeciesConfig) -> Option<AerosolBelowCloudParams> {
    if species.species_kind() != SpeciesKind::Aerosol {
        return None;
    }
    if !species.below_cloud_wet_active() {
        return None;
    }
    Some(AerosolBelowCloudParams {
        particle_diameter_um: species.mean_diameter_um.map_or(0.0, to_f32),
        rain_efficiency: species.crain_aero.map_or(0.0, to_f32),
        snow_efficiency: species.csnow_aero.map_or(0.0, to_f32),
    })
}

/// Aerosol in-cloud scavenging parameters (`ccn_aero`, `in_aero`).
///
/// Returns `Some` when in-cloud removal is enabled for this aerosol species.
#[must_use]
pub fn aerosol_in_cloud_params(species: &SpeciesConfig) -> Option<AerosolInCloudParams> {
    if !species.in_cloud_wet_active() {
        return None;
    }
    Some(AerosolInCloudParams {
        ccn_activation_fraction: species.ccn_aero.map_or(0.0, to_f32),
        ice_activation_fraction: species.in_aero.map_or(0.0, to_f32),
    })
}

/// Gas in-cloud scavenging parameters (`henry`).
///
/// Returns `Some` when a positive Henry coefficient is set. Gas species with
/// below-cloud removal but no Henry constant are rejected at config
/// validation (`readspecies` error 996); a missing Henry here means in-cloud
/// gas scavenging is disabled.
#[must_use]
pub fn gas_in_cloud_params(species: &SpeciesConfig) -> Option<GasInCloudParams> {
    if species.species_kind() != SpeciesKind::Gas {
        return None;
    }
    let henry = species.henry.filter(|v| *v > 0.0)?;
    Some(GasInCloudParams {
        henry_coefficient: to_f32(henry),
    })
}

/// Radioactive decay constant for one species [1/s].
///
/// Returns `0` when decay is disabled, matching Fortran guards
/// (`decay(ks) > 0`, see `drydepo_mod.f90:337`).
#[must_use]
pub fn decay_constant_s_inv(species: &SpeciesConfig) -> f32 {
    species
        .decay_constant
        .filter(|v| v.is_finite() && *v > 0.0)
        .map_or(0.0, to_f32)
}

/// Decay constants for up to [`MAX_SPECIES`] species [1/s], padded with `0`.
#[must_use]
pub fn species_decay_constants(species: &[SpeciesConfig]) -> [f32; MAX_SPECIES] {
    let mut lambdas = [0.0_f32; MAX_SPECIES];
    for (slot, config) in species.iter().take(MAX_SPECIES).enumerate() {
        lambdas[slot] = decay_constant_s_inv(config);
    }
    lambdas
}

/// Radioactive decay survival factor for one step.
///
/// Uses `exp(-lambda * |dt|)`, the same form as FLEXPART's decay correction
/// (`drydepo_mod.f90:338`, `unc_mod.f90:225`). Commutes with deposition
/// survival factors, so dispatch order relative to deposition is irrelevant.
#[must_use]
pub fn decay_survival_factor(decay_constant_s_inv: f32, dt_seconds: f32) -> f32 {
    let lambda = decay_constant_s_inv.max(0.0);
    if lambda <= 0.0 || !dt_seconds.is_finite() || dt_seconds == 0.0 {
        return 1.0;
    }
    (-lambda * dt_seconds.abs()).exp().clamp(0.0, 1.0)
}

/// Apply one radioactive decay step to a single species mass.
///
/// Returns `(remaining_mass_kg, decayed_mass_kg)`.
#[must_use]
pub fn apply_decay_mass_step(
    initial_mass_kg: f32,
    decay_constant_s_inv: f32,
    dt_seconds: f32,
) -> (f32, f32) {
    let mass = if initial_mass_kg.is_finite() && initial_mass_kg > 0.0 {
        initial_mass_kg
    } else {
        0.0
    };
    let survival = decay_survival_factor(decay_constant_s_inv, dt_seconds);
    let remaining = (mass * survival).max(0.0);
    (remaining, (mass - remaining).max(0.0))
}

/// Apply one radioactive decay step to all species mass slots in place.
///
/// `decay_constants_s_inv[s]` holds the decay constant for slot `s`; missing
/// entries (shorter slice) are treated as stable (`0`).
pub fn apply_decay_to_species_masses(
    masses: &mut [f32; MAX_SPECIES],
    decay_constants_s_inv: &[f32],
    dt_seconds: f32,
) {
    for (slot, mass) in masses.iter_mut().enumerate() {
        let lambda = decay_constants_s_inv.get(slot).copied().unwrap_or(0.0);
        let (remaining, _) = apply_decay_mass_step(*mass, lambda, dt_seconds);
        *mass = remaining;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn test_species(
        reldiff: Option<f64>,
        density: Option<f64>,
        diameter_um: Option<f64>,
        sigma: Option<f64>,
        half_life_s: Option<f64>,
        decay_constant: Option<f64>,
    ) -> SpeciesConfig {
        SpeciesConfig {
            name: "test".to_string(),
            molecular_weight: None,
            dry_deposition_velocity: None,
            decay_constant,
            half_life_s,
            wet_a_gas: None,
            wet_b_gas: None,
            crain_aero: None,
            csnow_aero: None,
            ccn_aero: None,
            in_aero: None,
            relative_diffusivity: reldiff,
            henry: None,
            surface_reactivity_f0: None,
            particle_density_kg_m3: density,
            mean_diameter_um: diameter_um,
            diameter_sigma: sigma,
            source_file: None,
            raw: BTreeMap::new(),
        }
    }

    #[test]
    fn gas_species_maps_to_gas_deposition_input() {
        let species = test_species(Some(0.8), None, None, None, None, None);
        let input = gas_deposition_input(&species).expect("gas pathway enabled");
        assert!((input.relative_diffusivity_to_h2o - 0.8).abs() < 1.0e-6);
        assert_eq!(input.constant_dry_velocity_m_s, None);
        assert!(aerosol_below_cloud_params(&species).is_none());
    }

    #[test]
    fn aerosol_species_has_no_gas_pathway() {
        let species = SpeciesConfig {
            crain_aero: Some(1.0),
            csnow_aero: Some(1.0),
            ccn_aero: Some(0.9),
            in_aero: Some(0.1),
            particle_density_kg_m3: Some(1000.0),
            mean_diameter_um: Some(50.0),
            diameter_sigma: Some(3.3),
            ..test_species(None, Some(1000.0), Some(50.0), Some(3.3), None, None)
        };
        assert!(gas_deposition_input(&species).is_none());
        let below = aerosol_below_cloud_params(&species).expect("below-cloud enabled");
        assert!((below.particle_diameter_um - 50.0).abs() < 1.0e-4);
        assert!((below.rain_efficiency - 1.0).abs() < 1.0e-6);
        let in_cloud = aerosol_in_cloud_params(&species).expect("in-cloud enabled");
        assert!((in_cloud.ccn_activation_fraction - 0.9).abs() < 1.0e-6);
        assert!((in_cloud.ice_activation_fraction - 0.1).abs() < 1.0e-6);
    }

    #[test]
    fn passive_tracer_maps_to_no_pathways() {
        let species = test_species(None, None, None, None, None, None);
        assert!(species.is_passive_tracer());
        assert!(gas_deposition_input(&species).is_none());
        assert!(gas_below_cloud_params(&species).is_none());
        assert!(aerosol_below_cloud_params(&species).is_none());
        assert!(aerosol_in_cloud_params(&species).is_none());
        assert!(gas_in_cloud_params(&species).is_none());
        assert_eq!(decay_constant_s_inv(&species).to_bits(), 0.0_f32.to_bits());
    }

    #[test]
    fn gas_below_cloud_params_require_gas_kind() {
        let mut species = test_species(Some(0.5), None, None, None, None, None);
        species.wet_a_gas = Some(1.0e-5);
        species.wet_b_gas = Some(0.7);
        let params = gas_below_cloud_params(&species).expect("gas wet params");
        assert!((params.coefficient_a - 1.0e-5).abs() < 1.0e-12);
        assert!((params.exponent_b - 0.7).abs() < 1.0e-6);
    }

    // Oracle truncation (`0.693147`) is intentional for Fortran parity.
    #[allow(clippy::approx_constant)]
    #[test]
    fn decay_constant_mapping_and_survival() {
        use std::f64::consts::LN_2;

        let half_life = 453_168.0_f64;
        let lambda = 0.693_147_f64 / half_life;
        let species = test_species(None, None, None, None, Some(half_life), Some(lambda));
        let mapped = decay_constant_s_inv(&species);
        assert!((f64::from(mapped) - lambda).abs() < 1.0e-12);
        // Oracle truncation (`0.693147` vs full `LN_2`) differs by ~4e-13 here.
        assert!((lambda - LN_2 / half_life).abs() < 1.0e-12);

        let survival = decay_survival_factor(mapped, 60.0);
        let expected = (-lambda * 60.0).exp();
        assert!((f64::from(survival) - expected).abs() < 1.0e-7);

        assert_eq!(
            decay_survival_factor(0.0, 60.0).to_bits(),
            1.0_f32.to_bits()
        );
        assert_eq!(
            decay_survival_factor(mapped, 0.0).to_bits(),
            1.0_f32.to_bits()
        );
    }

    #[test]
    fn decay_mass_step_conserves_mass() {
        let (remaining, decayed) = apply_decay_mass_step(2.5, 0.01, 60.0);
        assert!((remaining + decayed - 2.5).abs() < 1.0e-6);
        assert!(remaining > 0.0 && decayed > 0.0);

        let (stable, decayed_stable) = apply_decay_mass_step(2.5, 0.0, 60.0);
        assert!((stable - 2.5).abs() < 1.0e-7);
        assert!(decayed_stable.abs() < 1.0e-7);
    }

    #[test]
    fn decay_applies_per_slot_with_padding() {
        let mut masses = [1.0, 2.0, 4.0, 8.0];
        apply_decay_to_species_masses(&mut masses, &[0.01], 60.0);
        let expected_survival = (-0.01_f64 * 60.0).exp();
        assert!((f64::from(masses[0]) - expected_survival).abs() < 1.0e-6);
        assert!((masses[1] - 2.0).abs() < 1.0e-7);
        assert!((masses[2] - 4.0).abs() < 1.0e-7);
        assert!((masses[3] - 8.0).abs() < 1.0e-7);
    }

    #[test]
    fn species_decay_constants_pads_to_max_species() {
        let decaying = test_species(None, None, None, None, Some(100.0), Some(0.006_931_47));
        let stable = test_species(None, None, None, None, None, None);
        let lambdas = species_decay_constants(&[decaying, stable]);
        assert!(lambdas[0] > 0.0);
        for lambda in lambdas.iter().skip(1) {
            assert!(lambda.abs() < f32::EPSILON);
        }
    }
}
