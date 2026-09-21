//! Versioned candidate-side physics/runtime profile for validation runners.
//!
//! Issue #51 case manifests reference this profile so shared candidate settings
//! cannot drift with Rust `Default` implementations.

use std::path::{Path, PathBuf};
use serde::Deserialize;
use thiserror::Error;
use crate::io::{PblComputationOptions, TimeBoundsBehavior};
use super::case::{
    CandidatePhysicsProfileRef, CANDIDATE_PHYSICS_PROFILE_ID,
    CANDIDATE_PHYSICS_PROFILE_PATH, CANDIDATE_PHYSICS_PROFILE_VERSION,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePhysicsProfile {
    pub id: String,
    pub version: u32,
    pub description: String,
    pub time_bounds_behavior: CandidateTimeBoundsBehavior,
    pub integration: CandidateIntegrationPolicy,
    pub langevin: CandidateLangevinPolicy,
    pub pbl_computation_options: CandidatePblComputationOptions,
    pub synthetic_meteorology: CandidateSyntheticMeteorology,
    pub inactive_dry_reference_height_m: f32,
}
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateTimeBoundsBehavior { Strict, Clamp }
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateIntegrationPolicy { pub dispatches_per_manifest_step: u32 }
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateLangevinPolicy {
    pub vertical_substeps_per_timestep: u32,
    pub min_height_m: f32,
    pub rho_grad_over_rho: f32,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePblComputationOptions {
    pub roughness_length_m: f32,
    pub wind_reference_height_m: f32,
    pub heat_flux_neutral_threshold_w_m2: f32,
    pub bulk_richardson_critical: f32,
    pub min_shear_squared_m2_s2: f32,
    pub fallback_mixing_height_m: f32,
    pub hmix_min_m: f32,
    pub hmix_max_m: f32,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSyntheticMeteorology {
    pub temperature_k: f32,
    pub specific_humidity: f32,
    pub pressure_pa: f32,
    pub air_density_kg_m3: f32,
    pub density_gradient_kg_m2: f32,
}
#[derive(Debug, Error)]
pub enum CandidatePhysicsProfileError {
    #[error("unsupported candidate physics profile reference: {message}")]
    InvalidReference { message: String },
    #[error("failed to read candidate physics profile `{path}`: {source}")]
    Read { path: PathBuf, #[source] source: std::io::Error },
    #[error("failed to parse candidate physics profile `{path}`: {source}")]
    Parse { path: PathBuf, #[source] source: serde_json::Error },
    #[error("invalid candidate physics profile: {message}")]
    Invalid { message: String },
}
impl CandidatePhysicsProfile {
    pub fn load(reference: &CandidatePhysicsProfileRef) -> Result<Self, CandidatePhysicsProfileError> {
        if reference.id != CANDIDATE_PHYSICS_PROFILE_ID
            || reference.version != CANDIDATE_PHYSICS_PROFILE_VERSION
            || reference.manifest_path != CANDIDATE_PHYSICS_PROFILE_PATH {
            return Err(CandidatePhysicsProfileError::InvalidReference { message: format!(
                "expected {} v{} at {}, got {} v{} at {}",
                CANDIDATE_PHYSICS_PROFILE_ID, CANDIDATE_PHYSICS_PROFILE_VERSION,
                CANDIDATE_PHYSICS_PROFILE_PATH, reference.id, reference.version,
                reference.manifest_path) });
        }
        let path=Path::new(env!("CARGO_MANIFEST_DIR")).join(&reference.manifest_path);
        let text=std::fs::read_to_string(&path).map_err(|source|CandidatePhysicsProfileError::Read{path:path.clone(),source})?;
        let profile:Self=serde_json::from_str(&text).map_err(|source|CandidatePhysicsProfileError::Parse{path:path.clone(),source})?;
        profile.validate(reference)?;
        Ok(profile)
    }
    fn validate(&self, reference:&CandidatePhysicsProfileRef)->Result<(),CandidatePhysicsProfileError>{
        if self.id!=reference.id || self.version!=reference.version {
            return Err(CandidatePhysicsProfileError::Invalid{message:format!(
                "profile identity {} v{} does not match reference {} v{}",
                self.id,self.version,reference.id,reference.version)});
        }
        if self.description.trim().is_empty() {
            return Err(CandidatePhysicsProfileError::Invalid{message:"description must not be empty".to_string()});
        }
        if self.integration.dispatches_per_manifest_step!=1 {
            return Err(CandidatePhysicsProfileError::Invalid{message:format!(
                "dispatches_per_manifest_step must be 1 for the current ForwardTimeLoopDriver, got {}",
                self.integration.dispatches_per_manifest_step)});
        }
        let l=self.langevin;
        if !(1..=4).contains(&l.vertical_substeps_per_timestep) {
            return Err(CandidatePhysicsProfileError::Invalid{message:format!(
                "langevin.vertical_substeps_per_timestep must be in 1..=4, got {}",
                l.vertical_substeps_per_timestep)});
        }
        if !l.min_height_m.is_finite() || l.min_height_m<=0.0 {
            return Err(CandidatePhysicsProfileError::Invalid{message:format!(
                "langevin.min_height_m must be finite and > 0, got {}", l.min_height_m)});
        }
        if !l.rho_grad_over_rho.is_finite() {
            return Err(CandidatePhysicsProfileError::Invalid{message:
                "langevin.rho_grad_over_rho must be finite".to_string()});
        }
        let p=self.pbl_computation_options;
        for (field,value) in [
            ("roughness_length_m",p.roughness_length_m),
            ("wind_reference_height_m",p.wind_reference_height_m),
            ("bulk_richardson_critical",p.bulk_richardson_critical),
            ("min_shear_squared_m2_s2",p.min_shear_squared_m2_s2),
            ("fallback_mixing_height_m",p.fallback_mixing_height_m),
            ("hmix_min_m",p.hmix_min_m),("hmix_max_m",p.hmix_max_m)] {
            if !value.is_finite() || value<=0.0 { return Err(CandidatePhysicsProfileError::Invalid{
                message:format!("{field} must be finite and > 0, got {value}")}); }
        }
        if !p.heat_flux_neutral_threshold_w_m2.is_finite() || p.heat_flux_neutral_threshold_w_m2<0.0 {
            return Err(CandidatePhysicsProfileError::Invalid{message:"heat_flux_neutral_threshold_w_m2 must be finite and >= 0".to_string()});
        }
        if p.hmix_min_m>p.hmix_max_m { return Err(CandidatePhysicsProfileError::Invalid{message:"hmix_min_m must not exceed hmix_max_m".to_string()}); }
        let m=self.synthetic_meteorology;
        for (field,value) in [("temperature_k",m.temperature_k),("specific_humidity",m.specific_humidity),
            ("pressure_pa",m.pressure_pa),("air_density_kg_m3",m.air_density_kg_m3),
            ("density_gradient_kg_m2",m.density_gradient_kg_m2)] {
            if !value.is_finite(){return Err(CandidatePhysicsProfileError::Invalid{message:format!("{field} must be finite")});}
        }
        if m.temperature_k<=0.0 || m.pressure_pa<=0.0 || m.air_density_kg_m3<=0.0 {
            return Err(CandidatePhysicsProfileError::Invalid{message:"temperature, pressure and air density must be > 0".to_string()});
        }
        if !self.inactive_dry_reference_height_m.is_finite() || self.inactive_dry_reference_height_m<=0.0 {
            return Err(CandidatePhysicsProfileError::Invalid{message:"inactive_dry_reference_height_m must be finite and > 0".to_string()});
        }
        Ok(())
    }
    #[must_use]
    pub fn pbl_options(&self)->PblComputationOptions {
        let p=self.pbl_computation_options;
        PblComputationOptions{roughness_length_m:p.roughness_length_m,
            wind_reference_height_m:p.wind_reference_height_m,
            heat_flux_neutral_threshold_w_m2:p.heat_flux_neutral_threshold_w_m2,
            bulk_richardson_critical:p.bulk_richardson_critical,
            min_shear_squared_m2_s2:p.min_shear_squared_m2_s2,
            fallback_mixing_height_m:p.fallback_mixing_height_m,
            hmix_min_m:p.hmix_min_m,hmix_max_m:p.hmix_max_m}
    }
    #[must_use]
    pub fn time_bounds_behavior(&self)->TimeBoundsBehavior {
        match self.time_bounds_behavior {
            CandidateTimeBoundsBehavior::Strict => TimeBoundsBehavior::Strict,
            CandidateTimeBoundsBehavior::Clamp => TimeBoundsBehavior::Clamp,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_profile_loads_and_matches_previous_effective_defaults() {
        let profile=CandidatePhysicsProfile::load(&CandidatePhysicsProfileRef::canonical()).expect("canonical profile");
        let pbl=profile.pbl_options();
        assert_eq!(pbl.roughness_length_m,0.1);
        assert_eq!(pbl.wind_reference_height_m,10.0);
        assert_eq!(pbl.fallback_mixing_height_m,800.0);
        assert_eq!(pbl.hmix_min_m,100.0);
        assert_eq!(pbl.hmix_max_m,4500.0);
        assert_eq!(profile.synthetic_meteorology.temperature_k,285.0);
        assert_eq!(profile.integration.dispatches_per_manifest_step,1);
        assert_eq!(profile.langevin.vertical_substeps_per_timestep,4);
        assert_eq!(profile.langevin.min_height_m,0.01);
        assert_eq!(profile.langevin.rho_grad_over_rho,0.0);
        assert_eq!(profile.inactive_dry_reference_height_m,15.0);
        assert_eq!(profile.time_bounds_behavior(),TimeBoundsBehavior::Clamp);
    }
}
