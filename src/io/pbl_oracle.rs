//! Oracle-conformant PBL diagnostics (RISK-03.3G-04).
//!
//! This module ports the FLEXPART 11.1 PBL diagnostic chain so identical
//! meteorological input yields the identical PBL state on both sides:
//!
//! - saturation vapor pressure `ew` (Goff-Gratch, `qvsat_mod.f90`),
//! - momentum/heat stability corrections `psim`/`psih`
//!   (`pbl_profile_mod.f90`; Paulson 1970, Beljaars-Holtslag 1991),
//! - profile-method friction velocity and heat flux (Berkovicz and Prahm
//!   1982, `pbl_profile` in `pbl_profile_mod.f90`),
//! - mixing height and convective scale via the bulk Richardson loop
//!   (Vogelezang and Holtslag 1996, `richardson` in `getfields_mod.f90`).
//!
//! Heat-flux sign convention follows the oracle and GRIB input (ECMWF:
//! positive DOWNWARD into the surface). Positive flux is stable, negative
//! (upward, convective) flux is unstable. This intentionally differs from
//! earlier revisions of `pbl_params.rs`, which assumed positive-upward.
//!
//! Documented adaptations (no silent changes):
//! - The oracle integrates over pressure levels (`akz`/`bkz`); this port
//!   takes explicit per-level heights and pressures, which callers derive
//!   identically. Relative humidity diagnostics of the oracle loop are not
//!   used by the Richardson exit and are omitted.
//! - The oracle aborts the whole run when no stable layer is found; the port
//!   returns [`PblParameterError::NoStableLayerFound`] so callers choose an
//!   explicit, logged fallback policy instead.
//! - `ew` returns `0.0` for non-positive temperature (debug-asserted) instead
//!   of aborting like the oracle.

use crate::constants::{CPA, GA, R_AIR, VON_KARMAN};
use crate::io::pbl_params::PblParameterError;

/// Critical bulk Richardson number (Vogelezang and Holtslag 1996).
pub const RICHARDSON_CRITICAL: f32 = 0.25;
/// Shear-term coefficient `b` in the Richardson denominator.
pub const RICHARDSON_SHEAR_COEFFICIENT: f32 = 100.0;
/// Convective kinetic-energy factor for `hmixplus` (matches `CONVKE`).
pub const RICHARDSON_CONVKE: f32 = 2.0;
/// Minimum denominator guard shared by the Richardson loop.
pub const RICHARDSON_DENOMINATOR_FLOOR: f32 = 0.1;
/// Oracle PBL height bounds [m] (`hmixmin`/`hmixmax` in `par_mod.f90`).
pub const ORACLE_HMIX_MIN_M: f32 = 100.0;
/// Oracle PBL height bounds [m] (`hmixmin`/`hmixmax` in `par_mod.f90`).
pub const ORACLE_HMIX_MAX_M: f32 = 4500.0;
/// Iteration cap of the profile method (`maxiter`).
pub const PROFILE_METHOD_MAX_ITER: usize = 10;
/// Iteration cap of the Richardson excess loop (`itmax`).
pub const RICHARDSON_MAX_ITER: usize = 3;
/// Richardson sub-division steps between critical levels.
pub const RICHARDSON_SUBDIVISIONS: usize = 20;

/// Saturation vapor pressure over water [Pa] (Goff-Gratch, `qvsat_mod.f90:ew`).
///
/// Documented deviation: the oracle aborts on `T <= 0`; this port returns
/// `0.0` so diagnostics stay total (callers sanitize downstream).
// clippy::approx_constant: 0.43429 is the oracle's literal (log10(e)), kept
// bit-identical to the Fortran source rather than the library constant.
#[allow(clippy::approx_constant)]
#[must_use]
pub fn saturation_vapor_pressure_pa(temperature_k: f32) -> f32 {
    if temperature_k <= 0.0 {
        return 0.0;
    }
    let y = 373.16 / temperature_k;
    let mut accum = -7.90298 * (y - 1.0);
    accum += 5.02808 * 0.43429 * y.ln();
    let mut c = (1.0 - 1.0 / y) * 11.344;
    c = -1.0 + 10.0_f32.powf(c);
    c = -1.3816 * c / 1.0e7;
    let mut d = (1.0 - y) * 3.49149;
    d = -1.0 + 10.0_f32.powf(d);
    d = 8.1328 * d / 1.0e3;
    101_324.6 * 10.0_f32.powf(accum + c + d)
}

/// Momentum stability correction (Paulson 1970, `pbl_profile_mod.f90:psim`).
// clippy::manual_midpoint: factored form mirrors the Fortran source.
#[allow(clippy::manual_midpoint)]
#[must_use]
pub fn stability_correction_momentum_m(height_m: f32, obukhov_l_m: f32) -> f32 {
    let zeta = height_m / obukhov_l_m;
    if zeta <= 0.0 {
        let x = (1.0 - 15.0 * zeta).powf(0.25);
        let a1 = ((1.0 + x) * 0.5).powi(2);
        let a2 = (1.0 + x * x) * 0.5;
        (a1 * a2).ln() - 2.0 * x.atan() + std::f32::consts::FRAC_PI_2
    } else {
        -4.7 * zeta
    }
}

/// Heat stability correction (Langer/Stohl, Beljaars-Holtslag 1991,
/// `pbl_profile_mod.f90:psih`).
// clippy::similar_names: single-letter oracle variable names (z, l) mirror
// the Fortran source intentionally.
// clippy::manual_midpoint: factored form mirrors the Fortran source.
#[allow(clippy::similar_names, clippy::manual_midpoint)]
#[must_use]
pub fn stability_correction_heat_m(height_m: f32, obukhov_l_m: f32) -> f32 {
    const EPS: f32 = 1.0e-20;
    let mut length = obukhov_l_m;
    if (0.0..EPS).contains(&length) {
        length = EPS;
    } else if (-EPS..0.0).contains(&length) {
        length = -EPS;
    }
    if (height_m.log10() - length.abs().log10()) < EPS.log10() {
        return 0.0;
    }
    let zeta = height_m / length;
    if zeta > 0.0 {
        -((1.0 + 0.667 * zeta).powf(1.5))
            - 0.667 * (zeta - 5.0 / 0.35) * (-0.35 * zeta).exp()
            - 0.667 * 5.0 / 0.35
            + 1.0
    } else {
        let x = (1.0 - 16.0 * zeta).powf(0.25);
        2.0 * ((1.0 + x * x) * 0.5).ln()
    }
}

/// Inputs for the Berkovicz and Prahm (1982) profile method.
#[derive(Debug, Clone, Copy)]
pub struct ProfileMethodInput {
    /// Surface pressure [Pa].
    pub surface_pressure_pa: f32,
    /// 2 m dew point [K].
    pub dewpoint_2m_k: f32,
    /// Height of the first model level [m].
    pub level_height_m: f32,
    /// 2 m temperature [K].
    pub temperature_2m_k: f32,
    /// Temperature at the first model level [K].
    pub level_temperature_k: f32,
    /// 10 m wind speed [m/s].
    pub wind_10m_m_s: f32,
    /// Wind speed at the first model level [m/s].
    pub level_wind_m_s: f32,
}

/// Friction velocity and heat flux from the profile method.
///
/// Mirrors `pbl_profile` exactly, including the neutral branch, the
/// non-converging stable approximation, and the 10-step iteration.
/// Output heat flux uses the GRIB/ECMWF sign present at the input.
// clippy::similar_names: short oracle variable names mirror the source.
// clippy::manual_midpoint: factored form mirrors the Fortran source.
#[allow(clippy::similar_names, clippy::manual_midpoint)]
#[must_use]
pub fn profile_method_ustar_heat_flux(input: ProfileMethodInput) -> (f32, f32) {
    const MAX_ITER: usize = PROFILE_METHOD_MAX_ITER;
    const R1: f32 = 0.74;

    let vapor = saturation_vapor_pressure_pa(input.dewpoint_2m_k);
    let virtual_temp = input.temperature_2m_k * (1.0 + 0.378 * vapor / input.surface_pressure_pa);
    let air_density = input.surface_pressure_pa / (R_AIR * virtual_temp);

    let wind_diff = input.level_wind_m_s - input.wind_10m_m_s;
    if wind_diff <= 0.001 {
        return (0.01, 0.0);
    }
    let temp_diff =
        input.level_temperature_k - input.temperature_2m_k + 0.0098 * (input.level_height_m - 2.0);

    if temp_diff.abs() <= 0.03 {
        let ustar = VON_KARMAN * wind_diff
            / ((input.level_height_m / 10.0).ln()
                - stability_correction_momentum_m(input.level_height_m, 9999.0)
                + stability_correction_momentum_m(10.0, 9999.0));
        return (ustar, 0.0);
    }

    let mean_temp = 0.5 * (input.temperature_2m_k + input.level_temperature_k);
    let crit = (0.0219 * mean_temp * (input.level_height_m - 2.0) * wind_diff * wind_diff)
        / (temp_diff * (input.level_height_m - 10.0).powi(2));
    if temp_diff > 0.0 && crit <= 1.0 {
        let obukhov_l = 50.0;
        let ustar = VON_KARMAN * wind_diff
            / ((input.level_height_m * 0.1).ln()
                - stability_correction_momentum_m(input.level_height_m, obukhov_l)
                + stability_correction_momentum_m(10.0, obukhov_l));
        let theta_star = (VON_KARMAN * temp_diff / R1)
            / ((input.level_height_m * 0.5).ln()
                - stability_correction_heat_m(input.level_height_m, obukhov_l)
                + stability_correction_heat_m(2.0, obukhov_l));
        let heat_flux = air_density * CPA * ustar * theta_star;
        return (ustar, heat_flux);
    }

    let mut obukhov_l = 9999.0;
    let mut ustar = 0.01;
    let mut theta_star = 0.0;
    for _ in 0..MAX_ITER {
        let previous = obukhov_l;
        ustar = VON_KARMAN * wind_diff
            / ((input.level_height_m * 0.1).ln()
                - stability_correction_momentum_m(input.level_height_m, obukhov_l)
                + stability_correction_momentum_m(10.0, obukhov_l));
        theta_star = (VON_KARMAN * temp_diff / R1)
            / ((input.level_height_m * 0.5).ln()
                - stability_correction_heat_m(input.level_height_m, obukhov_l)
                + stability_correction_heat_m(2.0, obukhov_l));
        obukhov_l = (mean_temp * ustar * ustar) / (GA * VON_KARMAN * theta_star);
        if ((obukhov_l - previous) / previous).abs() < 0.01 {
            break;
        }
    }
    let heat_flux = air_density * CPA * ustar * theta_star;
    // NOTE: the oracle clamps a dead local Obukhov copy here; omitted as it
    // has no observable effect (only ustar and heat flux are returned).
    (ustar, heat_flux)
}

/// One ascending column level for Richardson diagnosis.
#[derive(Debug, Clone, Copy)]
pub struct RichardsonColumnLevel {
    /// Height above ground [m].
    pub height_m: f32,
    /// Air temperature [K].
    pub temperature_k: f32,
    /// Specific humidity [kg/kg].
    pub specific_humidity_kg_kg: f32,
    /// U wind [m/s].
    pub wind_u_m_s: f32,
    /// V wind [m/s].
    pub wind_v_m_s: f32,
    /// Pressure [Pa].
    pub pressure_pa: f32,
}

/// Inputs for Vogelezang and Holtslag (1996) mixing-height diagnosis.
#[derive(Debug, Clone)]
pub struct RichardsonInput<'a> {
    /// Surface pressure [Pa].
    pub surface_pressure_pa: f32,
    /// Friction velocity [m/s].
    pub ustar_m_s: f32,
    /// Sensible heat flux, ECMWF sign (positive DOWNWARD) [W/m²].
    pub heat_flux_w_m2: f32,
    /// 2 m temperature [K].
    pub temperature_2m_k: f32,
    /// 2 m dew point [K].
    pub dewpoint_2m_k: f32,
    /// Ascending column levels (index 0 is the lowest model level).
    pub levels: &'a [RichardsonColumnLevel],
}

/// One bracketing point of the Richardson subdivision.
#[derive(Debug, Clone, Copy)]
struct BracketPoint {
    height_m: f32,
    theta_k: f32,
    wind_u_m_s: f32,
    wind_v_m_s: f32,
}

/// Diagnosed `(mixing_height_m, convective_scale_m_s, hmixplus_m)`.
///
/// Mirrors `richardson` in `getfields_mod.f90`: 2 m reference, shear against
/// the second column level, `ric = 0.25` exit with 20-fold subdivision,
/// up-to-3 excess-temperature iterations for upward (negative) heat flux,
/// and `hmixmin`/`hmixmax` clamping.
///
/// # Errors
///
/// Returns [`PblParameterError::NoStableLayerFound`] when no level exceeds
/// the critical Richardson number (the oracle aborts the run in this case).
// clippy::similar_names, clippy::too_many_lines, clippy::cast_precision_loss,
// clippy::manual_midpoint: the loop mirrors Fortran names, structure, and
// factored forms for auditability; subdivision indices (<= 20) are exact.
#[allow(
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::manual_midpoint
)]
pub fn richardson_mixing_height(
    input: &RichardsonInput<'_>,
) -> Result<(f32, f32, f32), PblParameterError> {
    if input.levels.len() < 3 {
        return Err(PblParameterError::NoStableLayerFound);
    }
    let vapor =
        saturation_vapor_pressure_pa(input.dewpoint_2m_k) / input.surface_pressure_pa.max(1.0);
    let virtual_temp_ref = input.temperature_2m_k * (1.0 + 0.378 * vapor);
    let theta_ref_base =
        virtual_temp_ref * (100_000.0 / input.surface_pressure_pa).powf(R_AIR / CPA);

    let mut excess = 0.0_f32;
    let mut diagnosed_h = ORACLE_HMIX_MAX_M;
    let mut diagnosed_wstar = 0.0_f32;
    let mut diagnosed_hmixplus = 0.0_f32;

    for _ in 0..RICHARDSON_MAX_ITER {
        let theta_ref = theta_ref_base + excess;
        // Integrate upward from the 2 m reference; the shear reference is the
        // second column level, mirroring ulev(2)/vlev(2).
        let shear_u_ref = input.levels[1].wind_u_m_s;
        let shear_v_ref = input.levels[1].wind_v_m_s;
        let mut z_old = 2.0_f32;
        let mut theta_old = theta_ref;
        let mut u_old = input.levels[1].wind_u_m_s;
        let mut v_old = input.levels[1].wind_v_m_s;
        let mut found: Option<(BracketPoint, BracketPoint)> = None;

        for level in input.levels.iter().skip(1) {
            let virtual_temp = level.temperature_k * (1.0 + 0.608 * level.specific_humidity_kg_kg);
            let theta = virtual_temp * (100_000.0 / level.pressure_pa.max(1.0)).powf(R_AIR / CPA);
            let shear_sq = (level.wind_u_m_s - shear_u_ref).powi(2)
                + (level.wind_v_m_s - shear_v_ref).powi(2)
                + RICHARDSON_SHEAR_COEFFICIENT * input.ustar_m_s * input.ustar_m_s;
            let richardson = GA / theta_ref * (theta - theta_ref) * (level.height_m - 2.0)
                / shear_sq.max(RICHARDSON_DENOMINATOR_FLOOR);
            if richardson > RICHARDSON_CRITICAL && theta_old < theta {
                found = Some((
                    BracketPoint {
                        height_m: z_old,
                        theta_k: theta_old,
                        wind_u_m_s: u_old,
                        wind_v_m_s: v_old,
                    },
                    BracketPoint {
                        height_m: level.height_m,
                        theta_k: theta,
                        wind_u_m_s: level.wind_u_m_s,
                        wind_v_m_s: level.wind_v_m_s,
                    },
                ));
                break;
            }
            z_old = level.height_m;
            theta_old = theta;
            u_old = level.wind_u_m_s;
            v_old = level.wind_v_m_s;
        }

        let Some((low, high)) = found else {
            return Err(PblParameterError::NoStableLayerFound);
        };

        // 20-fold subdivision between the bracketing levels; (zl1, theta1)
        // tracks the last sub-critical point, (zl, thetal) the first
        // super-critical one.
        let mut zl1 = low.height_m;
        let mut theta1 = low.theta_k;
        let mut zl = high.height_m;
        let mut thetal = high.theta_k;
        let mut ul = high.wind_u_m_s;
        let mut vl = high.wind_v_m_s;
        for i in 1..=RICHARDSON_SUBDIVISIONS {
            let frac = i as f32 / RICHARDSON_SUBDIVISIONS as f32;
            let z = low.height_m + frac * (high.height_m - low.height_m);
            let th = low.theta_k + frac * (high.theta_k - low.theta_k);
            let uu = low.wind_u_m_s + frac * (high.wind_u_m_s - low.wind_u_m_s);
            let vv = low.wind_v_m_s + frac * (high.wind_v_m_s - low.wind_v_m_s);
            let shear_sq = (uu - shear_u_ref).powi(2)
                + (vv - shear_v_ref).powi(2)
                + RICHARDSON_SHEAR_COEFFICIENT * input.ustar_m_s * input.ustar_m_s;
            let ri = GA / theta_ref * (th - theta_ref) * (z - 2.0)
                / shear_sq.max(RICHARDSON_DENOMINATOR_FLOOR);
            zl = z;
            thetal = th;
            ul = uu;
            vl = vv;
            if ri > RICHARDSON_CRITICAL {
                break;
            }
            zl1 = z;
            theta1 = th;
        }

        diagnosed_h = zl;
        let theta_mean = 0.5 * (theta1 + thetal);
        let wind_at_h = (ul * ul + vl * vl).sqrt();
        // Brunt-Vaisala response for subgrid-topography excess (convke = 2).
        // Height span uses the bracketing points of the exit iteration.
        let span = (zl - zl1).max(1.0e-6);
        let slope = (thetal - theta1) / span;
        let bvfsq = GA / theta_mean * slope;
        diagnosed_hmixplus = if bvfsq <= 0.0 {
            9999.0
        } else {
            wind_at_h / bvfsq.sqrt() * RICHARDSON_CONVKE
        };

        if input.heat_flux_w_m2 < 0.0 {
            diagnosed_wstar =
                (-diagnosed_h * GA / theta_ref * input.heat_flux_w_m2 / CPA).powf(1.0 / 3.0);
            excess = -8.5 * input.heat_flux_w_m2 / CPA / diagnosed_wstar.max(1.0e-6);
        } else {
            diagnosed_wstar = 0.0;
            break;
        }
    }

    diagnosed_h = diagnosed_h.clamp(ORACLE_HMIX_MIN_M, ORACLE_HMIX_MAX_M);
    Ok((diagnosed_h, diagnosed_wstar, diagnosed_hmixplus))
}

/// Outcome of filling unavailable mixing heights from profile columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MixingHeightDiagnosisOutcome {
    /// Cells filled with a diagnosed mixing height.
    pub diagnosed: usize,
    /// Cells left untouched (already provided).
    pub provided: usize,
    /// Cells where diagnosis failed (fallback applies downstream).
    pub failed: usize,
}

/// Fill unavailable (`<= 0` or non-finite) mixing heights from profile columns.
///
/// For each cell without an operator-provided height, friction velocity comes
/// from surface stress (`ustar = sqrt(tau/rho)`, `scalev`) and the column
/// (`level_heights_m` with per-level temperature, humidity, winds, and
/// pressure) is diagnosed with [`richardson_mixing_height`], mirroring the
/// oracle order (ustar before Richardson). Cells that already carry a height,
/// columns with fewer than three usable levels, non-positive stress, and
/// failed diagnoses are left untouched so existing fallback behavior applies.
///
/// Level heights must contain at least three usable (strictly positive,
/// ascending) entries; leading surface placeholders (`<= 0`) are skipped.
/// Otherwise the whole grid is skipped (legacy grid-index vertical mode) and
/// everything counts as `provided` to keep the call a no-op.
// clippy::similar_names: grid/column array parameters share met-naming roots.
// clippy::too_many_arguments: one array per met field keeps call sites explicit.
#[allow(clippy::similar_names, clippy::too_many_arguments)]
pub fn diagnose_missing_mixing_heights(
    mixing_height_m: &mut ndarray::Array2<f32>,
    surface_pressure_pa: &ndarray::Array2<f32>,
    temperature_2m_k: &ndarray::Array2<f32>,
    dewpoint_2m_k: &ndarray::Array2<f32>,
    surface_stress_n_m2: &ndarray::Array2<f32>,
    temperature_k: &ndarray::Array3<f32>,
    specific_humidity_kg_kg: &ndarray::Array3<f32>,
    wind_u_m_s: &ndarray::Array3<f32>,
    wind_v_m_s: &ndarray::Array3<f32>,
    pressure_pa: &ndarray::Array3<f32>,
    level_heights_m: &[f32; 16],
    heat_flux_w_m2: &ndarray::Array2<f32>,
) -> MixingHeightDiagnosisOutcome {
    let mut outcome = MixingHeightDiagnosisOutcome {
        diagnosed: 0,
        provided: 0,
        failed: 0,
    };
    let shape = mixing_height_m.shape();
    if shape.len() != 2 {
        return outcome;
    }
    let (nx, ny) = (shape[0], shape[1]);
    // Usable levels: strictly positive ascending heights within the field.
    // Leading surface placeholders are skipped; a non-positive or
    // non-ascending entry afterwards ends the usable run.
    let field_depth = temperature_k.shape()[2]
        .min(specific_humidity_kg_kg.shape()[2])
        .min(wind_u_m_s.shape()[2])
        .min(wind_v_m_s.shape()[2])
        .min(pressure_pa.shape()[2])
        .min(level_heights_m.len());
    let mut level_indices: Vec<usize> = Vec::new();
    for k in 0..field_depth {
        if level_heights_m[k] <= 0.0 {
            if level_indices.is_empty() {
                continue;
            }
            break;
        }
        if let Some(&last) = level_indices.last() {
            if level_heights_m[k] <= level_heights_m[last] {
                break;
            }
        }
        level_indices.push(k);
    }
    if level_indices.len() < 3 {
        // No usable column (e.g. legacy grid-index mode): no-op.
        outcome.provided = nx.saturating_mul(ny);
        return outcome;
    }

    for i in 0..nx {
        for j in 0..ny {
            let current = mixing_height_m[[i, j]];
            if current.is_finite() && current > 0.0 {
                outcome.provided += 1;
                continue;
            }
            let levels: Vec<RichardsonColumnLevel> = level_indices
                .iter()
                .map(|&k| RichardsonColumnLevel {
                    height_m: level_heights_m[k],
                    temperature_k: temperature_k[[i, j, k]],
                    specific_humidity_kg_kg: specific_humidity_kg_kg[[i, j, k]],
                    wind_u_m_s: wind_u_m_s[[i, j, k]],
                    wind_v_m_s: wind_v_m_s[[i, j, k]],
                    pressure_pa: pressure_pa[[i, j, k]],
                })
                .collect();
            let vapor = saturation_vapor_pressure_pa(dewpoint_2m_k[[i, j]])
                / surface_pressure_pa[[i, j]].max(1.0);
            let virtual_temp = temperature_2m_k[[i, j]] * (1.0 + 0.378 * vapor);
            let air_density = surface_pressure_pa[[i, j]] / (R_AIR * virtual_temp.max(1.0));
            let stress = surface_stress_n_m2[[i, j]];
            if !stress.is_finite() || stress <= 0.0 || !air_density.is_finite() {
                outcome.failed += 1;
                continue;
            }
            let ustar = (stress / air_density).sqrt();
            let input = RichardsonInput {
                surface_pressure_pa: surface_pressure_pa[[i, j]],
                ustar_m_s: ustar,
                heat_flux_w_m2: heat_flux_w_m2[[i, j]],
                temperature_2m_k: temperature_2m_k[[i, j]],
                dewpoint_2m_k: dewpoint_2m_k[[i, j]],
                levels: &levels,
            };
            match richardson_mixing_height(&input) {
                Ok((hmix, _, _)) => {
                    mixing_height_m[[i, j]] = hmix;
                    outcome.diagnosed += 1;
                }
                Err(_) => outcome.failed += 1,
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    #[test]
    fn test_saturation_vapor_pressure_matches_buck_anchor() {
        // Independent anchor: Buck (1981) gives 611.21 Pa at 0 C; Goff-Gratch
        // evaluates to 610.33 Pa (f64 reference), well within a percent.
        let es = saturation_vapor_pressure_pa(273.15);
        assert!(
            (es - 610.33).abs() < 1.0,
            "es(0 C) = {es}, expected ~610.33 Pa"
        );
        assert!(saturation_vapor_pressure_pa(280.0) > es);
        assert!(saturation_vapor_pressure_pa(260.0) < es);
        assert_abs_diff_eq!(saturation_vapor_pressure_pa(0.0), 0.0, epsilon = 0.0);
    }

    #[test]
    fn test_stability_corrections_match_neutral_limits() {
        // Neutral limit (L -> +/-inf): both corrections vanish.
        assert_abs_diff_eq!(
            stability_correction_momentum_m(100.0, 1.0e30),
            0.0,
            epsilon = 1.0e-6
        );
        assert_abs_diff_eq!(
            stability_correction_heat_m(100.0, 1.0e30),
            0.0,
            epsilon = 1.0e-6
        );
        // Stable momentum branch is exactly -4.7*zeta.
        assert_abs_diff_eq!(
            stability_correction_momentum_m(50.0, 100.0),
            -2.35,
            epsilon = 1.0e-6
        );
        // Stable heat branch at zeta = 1 (f64 reference: -4.43623).
        assert_abs_diff_eq!(
            stability_correction_heat_m(100.0, 100.0),
            -4.43623,
            epsilon = 1.0e-4
        );
    }

    #[test]
    fn test_profile_method_neutral_matches_log_law() {
        // |deltat| <= 0.03 selects the neutral branch:
        // ustar = kappa*deltau / (ln(zml1/10) - psim + psim).
        // f64 reference value: 0.34116749.
        let (ustar, heat_flux) = profile_method_ustar_heat_flux(ProfileMethodInput {
            surface_pressure_pa: 101_325.0,
            dewpoint_2m_k: 284.0,
            level_height_m: 100.0,
            temperature_2m_k: 289.0,
            level_temperature_k: 288.0396,
            wind_10m_m_s: 4.0,
            level_wind_m_s: 6.0,
        });
        assert_abs_diff_eq!(ustar, 0.341_167_5, epsilon = 1.0e-6);
        assert_abs_diff_eq!(heat_flux, 0.0, epsilon = 1.0e-6);
    }

    #[test]
    fn test_profile_method_without_shear_returns_floor() {
        let (ustar, heat_flux) = profile_method_ustar_heat_flux(ProfileMethodInput {
            surface_pressure_pa: 101_325.0,
            dewpoint_2m_k: 284.0,
            level_height_m: 100.0,
            temperature_2m_k: 289.0,
            level_temperature_k: 280.0,
            wind_10m_m_s: 5.0,
            level_wind_m_s: 5.0,
        });
        assert_abs_diff_eq!(ustar, 0.01, epsilon = 1.0e-9);
        assert_abs_diff_eq!(heat_flux, 0.0, epsilon = 1.0e-9);
    }

    fn stable_inversion_column() -> Vec<RichardsonColumnLevel> {
        vec![
            RichardsonColumnLevel {
                height_m: 10.0,
                temperature_k: 300.0,
                specific_humidity_kg_kg: 0.005,
                wind_u_m_s: 2.0,
                wind_v_m_s: 1.0,
                pressure_pa: 100_000.0,
            },
            RichardsonColumnLevel {
                height_m: 100.0,
                temperature_k: 312.0,
                specific_humidity_kg_kg: 0.004,
                wind_u_m_s: 2.1,
                wind_v_m_s: 1.0,
                pressure_pa: 99_000.0,
            },
            RichardsonColumnLevel {
                height_m: 300.0,
                temperature_k: 320.0,
                specific_humidity_kg_kg: 0.003,
                wind_u_m_s: 2.2,
                wind_v_m_s: 1.1,
                pressure_pa: 96_500.0,
            },
        ]
    }

    #[test]
    fn test_richardson_strong_inversion_clamps_to_minimum() {
        // Ri exceeds 0.25 already at the first integrated level, so the
        // crossing sits near the ground and clamps to hmixmin.
        let input = RichardsonInput {
            surface_pressure_pa: 101_325.0,
            ustar_m_s: 0.3,
            heat_flux_w_m2: 50.0,
            temperature_2m_k: 300.0,
            dewpoint_2m_k: 295.0,
            levels: &stable_inversion_column(),
        };
        let (hmix, wstar, hmixplus) =
            richardson_mixing_height(&input).expect("stable column exits");
        assert_abs_diff_eq!(hmix, ORACLE_HMIX_MIN_M, epsilon = 1.0e-6);
        assert_abs_diff_eq!(wstar, 0.0, epsilon = 1.0e-9);
        assert!(hmixplus.is_finite());
    }

    #[test]
    fn test_richardson_convective_column_yields_positive_wstar() {
        let levels = vec![
            RichardsonColumnLevel {
                height_m: 10.0,
                temperature_k: 299.0,
                specific_humidity_kg_kg: 0.005,
                wind_u_m_s: 2.0,
                wind_v_m_s: 1.0,
                pressure_pa: 100_000.0,
            },
            RichardsonColumnLevel {
                height_m: 200.0,
                temperature_k: 290.0,
                specific_humidity_kg_kg: 0.004,
                wind_u_m_s: 2.0,
                wind_v_m_s: 1.0,
                pressure_pa: 97_500.0,
            },
            RichardsonColumnLevel {
                height_m: 1000.0,
                temperature_k: 295.0,
                specific_humidity_kg_kg: 0.003,
                wind_u_m_s: 2.0,
                wind_v_m_s: 1.0,
                pressure_pa: 90_000.0,
            },
        ];
        let input = RichardsonInput {
            surface_pressure_pa: 101_325.0,
            ustar_m_s: 0.3,
            heat_flux_w_m2: -50.0,
            temperature_2m_k: 300.0,
            dewpoint_2m_k: 295.0,
            levels: &levels,
        };
        let (hmix, wstar, _) =
            richardson_mixing_height(&input).expect("convective column exits aloft");
        assert!(hmix > 200.0 && hmix <= 1000.0, "hmix = {hmix}");
        assert!(wstar > 0.0, "convective column must produce wstar > 0");
    }

    #[test]
    fn test_richardson_neutral_column_reports_no_stable_layer() {
        // Degenerate synthetic column (near-dry, isentropic by construction):
        // Ri stays ~0, mirroring the oracle abort. With dewpoint = 200 K the
        // vapor term vanishes, so level temperatures following the dry
        // adiabat keep theta exactly at the reference value.
        let levels = vec![
            RichardsonColumnLevel {
                height_m: 10.0,
                temperature_k: 298.873,
                specific_humidity_kg_kg: 0.0,
                wind_u_m_s: 2.0,
                wind_v_m_s: 1.0,
                pressure_pa: 100_000.0,
            },
            RichardsonColumnLevel {
                height_m: 100.0,
                temperature_k: 298.014,
                specific_humidity_kg_kg: 0.0,
                wind_u_m_s: 2.0,
                wind_v_m_s: 1.0,
                pressure_pa: 99_000.0,
            },
            RichardsonColumnLevel {
                height_m: 300.0,
                temperature_k: 295.837,
                specific_humidity_kg_kg: 0.0,
                wind_u_m_s: 2.0,
                wind_v_m_s: 1.0,
                pressure_pa: 96_500.0,
            },
        ];
        let input = RichardsonInput {
            surface_pressure_pa: 101_325.0,
            ustar_m_s: 0.3,
            heat_flux_w_m2: 0.0,
            temperature_2m_k: 300.0,
            dewpoint_2m_k: 200.0,
            levels: &levels,
        };
        let error = richardson_mixing_height(&input).expect_err("isentropic column must not exit");
        assert!(matches!(error, PblParameterError::NoStableLayerFound));
    }

    /// 2x1 harness grid: cell (0,0) unset, cell (1,0) operator-provided.
    type HarnessGrid = (
        ndarray::Array2<f32>,
        ndarray::Array2<f32>,
        ndarray::Array2<f32>,
        ndarray::Array2<f32>,
        ndarray::Array2<f32>,
        ndarray::Array3<f32>,
        ndarray::Array3<f32>,
        ndarray::Array3<f32>,
        ndarray::Array3<f32>,
        ndarray::Array3<f32>,
        ndarray::Array2<f32>,
    );

    fn diagnosis_harness_grid() -> HarnessGrid {
        use ndarray::{Array2, Array3};
        // 2x1 grid: cell (0,0) unset, cell (1,0) operator-provided.
        let mut hmix = Array2::from_elem((2, 1), 0.0_f32);
        hmix[[1, 0]] = 2500.0;
        let surface = Array2::from_elem((2, 1), 101_325.0);
        let surf_temp = Array2::from_elem((2, 1), 300.0);
        let dewpoint = Array2::from_elem((2, 1), 295.0);
        let stress = Array2::from_elem((2, 1), 0.2);
        let heat_flux = Array2::from_elem((2, 1), 50.0);
        // Stable column (same shape as the inversion unit case).
        let mut column_temp = Array3::zeros((2, 1, 3));
        let mut column_hum = Array3::zeros((2, 1, 3));
        let mut column_u = Array3::zeros((2, 1, 3));
        let mut column_v = Array3::zeros((2, 1, 3));
        let mut column_p = Array3::zeros((2, 1, 3));
        for i in 0..2 {
            column_temp[[i, 0, 0]] = 300.0;
            column_temp[[i, 0, 1]] = 312.0;
            column_temp[[i, 0, 2]] = 320.0;
            column_hum[[i, 0, 0]] = 0.005;
            column_hum[[i, 0, 1]] = 0.004;
            column_hum[[i, 0, 2]] = 0.003;
            column_u[[i, 0, 0]] = 2.0;
            column_u[[i, 0, 1]] = 2.1;
            column_u[[i, 0, 2]] = 2.2;
            column_v[[i, 0, 0]] = 1.0;
            column_v[[i, 0, 1]] = 1.0;
            column_v[[i, 0, 2]] = 1.1;
            column_p[[i, 0, 0]] = 100_000.0;
            column_p[[i, 0, 1]] = 99_000.0;
            column_p[[i, 0, 2]] = 96_500.0;
        }
        (
            hmix,
            surface,
            surf_temp,
            dewpoint,
            stress,
            column_temp,
            column_hum,
            column_u,
            column_v,
            column_p,
            heat_flux,
        )
    }

    #[test]
    fn test_diagnose_fills_only_unset_cells() {
        let (
            mut hmix,
            surface,
            surf_temp,
            dewpoint,
            stress,
            column_temp,
            column_hum,
            column_u,
            column_v,
            column_p,
            heat_flux,
        ) = diagnosis_harness_grid();
        let heights = [
            10.0, 100.0, 300.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ];
        let outcome = diagnose_missing_mixing_heights(
            &mut hmix,
            &surface,
            &surf_temp,
            &dewpoint,
            &stress,
            &column_temp,
            &column_hum,
            &column_u,
            &column_v,
            &column_p,
            &heights,
            &heat_flux,
        );
        assert_eq!(outcome.diagnosed, 1);
        assert_eq!(outcome.provided, 1);
        assert_eq!(outcome.failed, 0);
        assert_abs_diff_eq!(hmix[[0, 0]], ORACLE_HMIX_MIN_M, epsilon = 1.0e-6);
        assert_abs_diff_eq!(hmix[[1, 0]], 2500.0, epsilon = 1.0e-6);
    }

    #[test]
    fn test_diagnose_skips_legacy_grid_index_heights() {
        let (
            mut hmix,
            surface,
            surf_temp,
            dewpoint,
            stress,
            column_temp,
            column_hum,
            column_u,
            column_v,
            column_p,
            heat_flux,
        ) = diagnosis_harness_grid();
        let outcome = diagnose_missing_mixing_heights(
            &mut hmix,
            &surface,
            &surf_temp,
            &dewpoint,
            &stress,
            &column_temp,
            &column_hum,
            &column_u,
            &column_v,
            &column_p,
            &[0.0; 16],
            &heat_flux,
        );
        assert_eq!(outcome.diagnosed, 0);
        assert_eq!(outcome.failed, 0);
        assert_abs_diff_eq!(hmix[[0, 0]], 0.0, epsilon = 1.0e-9);
    }
}
