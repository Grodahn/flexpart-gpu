//! Shared PBL diagnostic options for GPU encoding and CPU preparation.

use crate::constants::{HMIX_MAX, HMIX_MIN};

pub(crate) const DEFAULT_ROUGHNESS_LENGTH_M: f32 = 0.1;
pub(crate) const DEFAULT_WIND_REFERENCE_HEIGHT_M: f32 = 10.0;
const DEFAULT_HEAT_FLUX_NEUTRAL_THRESHOLD_W_M2: f32 = 1.0;
const DEFAULT_BULK_RI_CRITICAL: f32 = 0.25;
const DEFAULT_MIN_SHEAR_SQUARED_M2_S2: f32 = 0.25;
const DEFAULT_FALLBACK_HMIX_M: f32 = 800.0;

/// Tunable options for PBL parameter computation.
#[derive(Clone, Copy, Debug)]
pub struct PblComputationOptions {
    /// Roughness length `z0` used when deriving `u*` from 10 m wind [m].
    pub roughness_length_m: f32,
    /// Wind reference height used with the log-law [m].
    pub wind_reference_height_m: f32,
    /// |H| threshold below which conditions are treated as neutral [W/m²].
    pub heat_flux_neutral_threshold_w_m2: f32,
    /// Critical bulk Richardson number for stable/neutral transition [-].
    pub bulk_richardson_critical: f32,
    /// Floor for shear term in Richardson denominator [(m/s)²].
    pub min_shear_squared_m2_s2: f32,
    /// Fallback mixing height if no valid `hmix` and no profile cue [m].
    pub fallback_mixing_height_m: f32,
    /// Minimum allowed mixing height [m].
    pub hmix_min_m: f32,
    /// Maximum allowed mixing height [m].
    pub hmix_max_m: f32,
}

impl Default for PblComputationOptions {
    fn default() -> Self {
        Self {
            roughness_length_m: DEFAULT_ROUGHNESS_LENGTH_M,
            wind_reference_height_m: DEFAULT_WIND_REFERENCE_HEIGHT_M,
            heat_flux_neutral_threshold_w_m2: DEFAULT_HEAT_FLUX_NEUTRAL_THRESHOLD_W_M2,
            bulk_richardson_critical: DEFAULT_BULK_RI_CRITICAL,
            min_shear_squared_m2_s2: DEFAULT_MIN_SHEAR_SQUARED_M2_S2,
            fallback_mixing_height_m: DEFAULT_FALLBACK_HMIX_M,
            hmix_min_m: HMIX_MIN,
            hmix_max_m: HMIX_MAX,
        }
    }
}
