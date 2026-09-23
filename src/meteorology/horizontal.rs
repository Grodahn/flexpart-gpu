//! Canonical provider-independent horizontal meteorology interpolation for issue #72.
//!
//! This module owns x/y bilinear sampling over normalized #29 canonical grids.
//! It implements the horizontal semantics frozen by #71 without rediscovering
//! FLEXPART interpolation behavior.
//!
//! Ported from `interpol_mod.f90:128-166` (`find_grid_indices`, mother-grid path),
//! `interpol_mod.f90:168-188` (`find_grid_distances`) and
//! `interpol_mod.f90:481-504` (`hor_interpol_4d`).
//! Geographic mapping follows `point_mod::coordtrafo`:
//! `xt = (lon_deg - xlon0_deg) / dx_deg`, `yt = (lat_deg - ylat0_deg) / dy_deg`.
//!
//! This ticket owns x/y interpolation only. Vertical interpolation (#73),
//! temporal interpolation (#74), accumulated-field handling (#75), 4D composition
//! (#76), provider decoding (#32) and consumer migration (#77) are out of scope.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{HorizontalGrid, HorizontalStaggering, LongitudeDomain};

/// Tolerance for canonical grid-closure checks, matching #29 schema validation.
const GRID_TOLERANCE_DEG: f64 = 1.0e-9;
/// Bilinear horizontal sample with full oracle-traceable evidence.
///
/// `weights` are `[p1, p2, p3, p4]` in FLEXPART order:
/// `(ix,jy)`, `(ixp,jy)`, `(ix,jyp)`, `(ixp,jyp)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HorizontalSample {
    /// Sampled field value, computed in `f64` from `f32` corners then rounded once.
    pub value: f32,
    /// Canonical grid-index x coordinate after edge snapping.
    pub xt: f64,
    /// Canonical grid-index y coordinate after edge snapping.
    pub yt: f64,
    /// Lower x index (`int(xt)` for `xt >= 0`).
    pub ix: usize,
    /// Lower y index (`int(yt)` for `yt >= 0`).
    pub jy: usize,
    /// Upper x index (`ix + 1`, ghost `nx` for the periodic seam, or `ix` on an exact edge).
    pub ixp: usize,
    /// Upper y index (`jy + 1`, or `jy` on the exact last row).
    pub jyp: usize,
    /// Bilinear weights `[p1, p2, p3, p4]`.
    pub weights: [f64; 4],
    /// Whether the source grid was classified as periodic in x.
    pub is_periodic_x: bool,
}

/// Fail-closed errors for canonical horizontal sampling.
#[derive(Debug, Error, PartialEq)]
pub enum HorizontalError {
    /// Zero-sized horizontal dimensions.
    #[error("malformed horizontal dimensions: nx={nx}, ny={ny}")]
    MalformedDimensions { nx: usize, ny: usize },
    /// Grid spacing, origin or coverage inconsistent with #29 schema v1.
    #[error("inconsistent horizontal grid metadata: {reason}")]
    InconsistentGridMetadata { reason: &'static str },
    /// Only cell-center staggering is frozen by #71.
    #[error(
        "unsupported horizontal staggering: {staggering:?}; only cell_center is frozen by #71"
    )]
    UnsupportedStaggering { staggering: HorizontalStaggering },
    /// Non-finite or physically impossible coordinate.
    #[error("impossible horizontal coordinate: {reason}")]
    ImpossibleCoordinate { reason: &'static str },
    /// Valid Earth coordinate outside the supported canonical domain.
    #[error("horizontal coordinate out of supported domain: xt={xt}, yt={yt}")]
    OutOfDomain { xt: f64, yt: f64 },
    /// Longitude is not represented by the grid's declared convention.
    #[error("longitude {lon_deg} is outside declared convention {longitude_domain:?}")]
    LongitudeOutsideConvention {
        lon_deg: f64,
        longitude_domain: LongitudeDomain,
    },
    /// Field value count does not match `nx * ny` for cell-center storage.
    #[error("horizontal field shape mismatch: expected {expected} values, got {actual}")]
    ShapeMismatch { expected: usize, actual: usize },
    /// Non-finite field value at the sampled position or in the input slice.
    #[error("non-finite horizontal field value at flat index {index}")]
    NonFiniteValue { index: usize },
    /// Sampled bilinear combination is non-finite.
    #[error("non-finite horizontal sample result")]
    NonFiniteResult,
}

/// Validate canonical #29 horizontal grid metadata for sampling.
///
/// Returns whether the grid is periodic in x (`nx * dx == 360` within tolerance).
/// Regional grids must fit wholly inside the declared longitude domain and the
/// full latitude cell coverage must remain inside `[-90, 90]`.
///
/// # Errors
/// Returns [`HorizontalError`] for zero dimensions or inconsistent metadata.
pub fn validate_horizontal_grid(grid: &HorizontalGrid) -> Result<bool, HorizontalError> {
    if grid.nx == 0 || grid.ny == 0 {
        return Err(HorizontalError::MalformedDimensions {
            nx: grid.nx,
            ny: grid.ny,
        });
    }
    if !grid.dx_deg.is_finite()
        || !grid.dy_deg.is_finite()
        || grid.dx_deg <= 0.0
        || grid.dy_deg <= 0.0
        || !grid.xlon0_deg.is_finite()
        || !grid.ylat0_deg.is_finite()
    {
        return Err(HorizontalError::InconsistentGridMetadata {
            reason: "spacing and origin must be finite with positive spacing",
        });
    }
    if !(-90.0..=90.0).contains(&grid.ylat0_deg) {
        return Err(HorizontalError::InconsistentGridMetadata {
            reason: "ylat0_deg must lie within [-90, 90]",
        });
    }

    let south_edge = grid.ylat0_deg - 0.5 * grid.dy_deg;
    let north_edge = grid.ylat0_deg + (grid.ny.saturating_sub(1) as f64 + 0.5) * grid.dy_deg;
    if south_edge < -90.0 - GRID_TOLERANCE_DEG || north_edge > 90.0 + GRID_TOLERANCE_DEG {
        return Err(HorizontalError::InconsistentGridMetadata {
            reason: "latitude cell coverage must remain inside [-90, 90]",
        });
    }

    let (lon_min, lon_max, origin_valid) = match grid.longitude_domain {
        LongitudeDomain::Minus180To180 => {
            (-180.0, 180.0, (-180.0..=180.0).contains(&grid.xlon0_deg))
        }
        LongitudeDomain::ZeroTo360 => (0.0, 360.0, (0.0..360.0).contains(&grid.xlon0_deg)),
    };
    if !origin_valid {
        return Err(HorizontalError::InconsistentGridMetadata {
            reason: "xlon0_deg outside declared longitude domain",
        });
    }

    let x_coverage = grid.dx_deg * grid.nx as f64;
    if !x_coverage.is_finite() || x_coverage > 360.0 + GRID_TOLERANCE_DEG {
        return Err(HorizontalError::InconsistentGridMetadata {
            reason: "x coverage exceeds 360 degrees",
        });
    }

    let is_periodic = grid_close(x_coverage, 360.0);
    if !is_periodic {
        let west_edge = grid.xlon0_deg - 0.5 * grid.dx_deg;
        let east_edge = grid.xlon0_deg + (grid.nx.saturating_sub(1) as f64 + 0.5) * grid.dx_deg;
        if west_edge < lon_min - GRID_TOLERANCE_DEG || east_edge > lon_max + GRID_TOLERANCE_DEG {
            return Err(HorizontalError::InconsistentGridMetadata {
                reason:
                    "regional grid must fit inside its longitude domain without crossing the seam",
            });
        }
    }
    Ok(is_periodic)
}

/// Convert canonical grid indices to geographic coordinates.
///
/// Inverse of `point_mod::coordtrafo`: `lon = xlon0 + xt * dx`,
/// `lat = ylat0 + yt * dy`. Used for comparison evidence only.
#[must_use]
pub fn grid_index_to_lonlat(grid: &HorizontalGrid, xt: f64, yt: f64) -> (f64, f64) {
    (
        grid.xlon0_deg + xt * grid.dx_deg,
        grid.ylat0_deg + yt * grid.dy_deg,
    )
}

/// Sample one cell-center horizontal slice at canonical grid indices.
///
/// `field_values` holds exactly `nx * ny` `f32` samples in x-fastest order
/// (`offset = x + nx * y`). Only [`HorizontalStaggering::CellCenter`] is
/// supported; face staggering is not frozen by #71 and fails closed.
///
/// Supported domain:
/// non-periodic `0 <= xt <= nx-1`, `0 <= yt <= ny-1`;
/// periodic `0 <= xt < nx`, `0 <= yt <= ny-1`.
/// Anything else fails with [`HorizontalError::OutOfDomain`] rather than
/// inheriting raw primitive wrap/clamp behavior.
///
/// # Errors
/// Returns [`HorizontalError`] for malformed grids, unsupported staggering,
/// shape mismatches, non-finite inputs, impossible coordinates or
/// out-of-domain requests.
pub fn sample_horizontal(
    grid: &HorizontalGrid,
    field_values: &[f32],
    staggering: HorizontalStaggering,
    xt: f64,
    yt: f64,
) -> Result<HorizontalSample, HorizontalError> {
    if staggering != HorizontalStaggering::CellCenter {
        return Err(HorizontalError::UnsupportedStaggering { staggering });
    }
    let is_periodic_x = validate_horizontal_grid(grid)?;
    let (nx, ny) = (grid.nx, grid.ny);

    let expected = nx
        .checked_mul(ny)
        .ok_or(HorizontalError::MalformedDimensions { nx, ny })?;
    if field_values.len() != expected {
        return Err(HorizontalError::ShapeMismatch {
            expected,
            actual: field_values.len(),
        });
    }
    if !xt.is_finite() || !yt.is_finite() {
        return Err(HorizontalError::ImpossibleCoordinate {
            reason: "grid indices must be finite",
        });
    }

    validate_supported_domain(xt, yt, nx, ny, is_periodic_x)?;

    // For xt >= 0 truncation equals floor; negatives are already rejected.
    let ix = xt.floor() as usize;
    let jy = yt.floor() as usize;
    if ix >= nx || jy >= ny {
        return Err(HorizontalError::OutOfDomain { xt, yt });
    }
    let ddx = xt - ix as f64;
    let ddy = yt - jy as f64;
    if !(0.0..=1.0).contains(&ddx) || !(0.0..=1.0).contains(&ddy) {
        return Err(HorizontalError::OutOfDomain { xt, yt });
    }

    // North edge: FLEXPART temporary fix `if (jyp >= nymax) jyp = jyp - 1`.
    // Supported only for the exact last row; overshoot already failed above.
    let jyp = if jy + 1 >= ny { jy } else { jy + 1 };
    // East handling: periodic seam uses the duplicate ghost column `nx`;
    // non-periodic exact last column collapses to `ix` (weights on `ixp` are zero).
    // The regional `ixp >= nxmax` wrap branch is explicitly unsupported and
    // unreachable because out-of-domain `xt` already failed.
    let ixp = if is_periodic_x {
        if ix + 1 > nx {
            return Err(HorizontalError::OutOfDomain { xt, yt });
        }
        ix + 1
    } else if ix + 1 >= nx {
        ix
    } else {
        ix + 1
    };

    let rddx = 1.0 - ddx;
    let rddy = 1.0 - ddy;
    let weights = [rddx * rddy, ddx * rddy, rddx * ddy, ddx * ddy];

    let value_00 = field_at(field_values, nx, ny, ix, jy, is_periodic_x)?;
    let value_10 = field_at(field_values, nx, ny, ixp, jy, is_periodic_x)?;
    let value_01 = field_at(field_values, nx, ny, ix, jyp, is_periodic_x)?;
    let value_11 = field_at(field_values, nx, ny, ixp, jyp, is_periodic_x)?;

    let value_f64 = weights[0] * value_00
        + weights[1] * value_10
        + weights[2] * value_01
        + weights[3] * value_11;
    if !value_f64.is_finite() {
        return Err(HorizontalError::NonFiniteResult);
    }
    // f32 rounding is intentional: canonical fields store f32 samples.
    #[allow(clippy::cast_possible_truncation)]
    let value = value_f64 as f32;
    if !value.is_finite() {
        return Err(HorizontalError::NonFiniteResult);
    }

    Ok(HorizontalSample {
        value,
        xt,
        yt,
        ix,
        jy,
        ixp,
        jyp,
        weights,
        is_periodic_x,
    })
}

/// Sample one cell-center horizontal slice at geographic coordinates.
///
/// Mapping follows pinned `point_mod::coordtrafo` with `nxshift = 0` equivalent
/// convention (`xlon0_deg` is the first stored cell center):
/// `xt = (lon_deg - xlon0_deg) / dx_deg`, `yt = (lat_deg - ylat0_deg) / dy_deg`.
/// No longitude wrapping or `nxshift` rotation is applied here; that
/// normalization belongs to ingestion (#32). Out-of-domain results fail closed.
///
/// # Errors
/// Returns [`HorizontalError`] under the same conditions as
/// [`sample_horizontal`], plus [`HorizontalError::ImpossibleCoordinate`] for
/// non-finite or pole-impossible latitude.
pub fn sample_horizontal_geographic(
    grid: &HorizontalGrid,
    field_values: &[f32],
    staggering: HorizontalStaggering,
    lon_deg: f64,
    lat_deg: f64,
) -> Result<HorizontalSample, HorizontalError> {
    if !lon_deg.is_finite() || !lat_deg.is_finite() {
        return Err(HorizontalError::ImpossibleCoordinate {
            reason: "longitude and latitude must be finite",
        });
    }
    if !(-90.0..=90.0).contains(&lat_deg) {
        return Err(HorizontalError::ImpossibleCoordinate {
            reason: "latitude outside [-90, 90] is impossible",
        });
    }
    if !grid.dx_deg.is_finite()
        || !grid.dy_deg.is_finite()
        || grid.dx_deg <= 0.0
        || grid.dy_deg <= 0.0
    {
        return Err(HorizontalError::InconsistentGridMetadata {
            reason: "spacing must be finite and positive",
        });
    }
    let xt = (lon_deg - grid.xlon0_deg) / grid.dx_deg;
    let yt = (lat_deg - grid.ylat0_deg) / grid.dy_deg;
    if !xt.is_finite() || !yt.is_finite() {
        return Err(HorizontalError::ImpossibleCoordinate {
            reason: "geographic to grid mapping overflowed",
        });
    }
    let longitude_in_declared_convention = match grid.longitude_domain {
        LongitudeDomain::Minus180To180 => (-180.0..=180.0).contains(&lon_deg),
        LongitudeDomain::ZeroTo360 => (0.0..360.0).contains(&lon_deg),
    };
    if !longitude_in_declared_convention {
        return Err(HorizontalError::LongitudeOutsideConvention {
            lon_deg,
            longitude_domain: grid.longitude_domain,
        });
    }
    sample_horizontal(grid, field_values, staggering, xt, yt)
}

fn grid_close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() <= GRID_TOLERANCE_DEG
}

fn validate_supported_domain(
    xt: f64,
    yt: f64,
    nx: usize,
    ny: usize,
    is_periodic_x: bool,
) -> Result<(), HorizontalError> {
    let nx_f = nx as f64;
    let ny_f = ny as f64;

    let x_is_supported =
        xt >= 0.0 && (is_periodic_x && xt < nx_f || !is_periodic_x && xt <= nx_f - 1.0);
    if !x_is_supported {
        return Err(HorizontalError::OutOfDomain { xt, yt });
    }

    if yt < 0.0 || yt > ny_f - 1.0 {
        return Err(HorizontalError::OutOfDomain { xt, yt });
    }

    Ok(())
}

fn field_at(
    field_values: &[f32],
    nx: usize,
    ny: usize,
    ix: usize,
    jy: usize,
    is_periodic_x: bool,
) -> Result<f64, HorizontalError> {
    if jy >= ny {
        return Err(HorizontalError::OutOfDomain {
            xt: ix as f64,
            yt: jy as f64,
        });
    }
    // Periodic duplicate column `nx` mirrors column 0; it is not stored.
    let stored_x = if is_periodic_x && ix == nx {
        0
    } else if ix < nx {
        ix
    } else {
        return Err(HorizontalError::OutOfDomain {
            xt: ix as f64,
            yt: jy as f64,
        });
    };
    let offset = stored_x + nx * jy;
    let raw = *field_values
        .get(offset)
        .ok_or(HorizontalError::ShapeMismatch {
            expected: nx * ny,
            actual: field_values.len(),
        })?;
    if !raw.is_finite() {
        return Err(HorizontalError::NonFiniteValue { index: offset });
    }
    Ok(f64::from(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regional_grid() -> HorizontalGrid {
        HorizontalGrid {
            nx: 4,
            ny: 3,
            xlon0_deg: 0.0,
            ylat0_deg: 0.0,
            dx_deg: 1.0,
            dy_deg: 1.0,
            longitude_domain: LongitudeDomain::Minus180To180,
        }
    }

    fn periodic_grid() -> HorizontalGrid {
        HorizontalGrid {
            nx: 4,
            ny: 3,
            xlon0_deg: -180.0,
            ylat0_deg: 0.0,
            dx_deg: 90.0,
            dy_deg: 1.0,
            longitude_domain: LongitudeDomain::Minus180To180,
        }
    }

    /// Oracle fixture field: `f(x,y) = 100 + 100*x + 10*y`, x-fastest.
    fn oracle_field() -> Vec<f32> {
        vec![
            100.0, 200.0, 300.0, 400.0, 110.0, 210.0, 310.0, 410.0, 120.0, 220.0, 320.0, 420.0,
        ]
    }

    fn assert_relative(actual: f32, expected: f64) {
        let diff = (f64::from(actual) - expected).abs();
        let tolerance = 1.0e-6 + 1.0e-5 * expected.abs();
        assert!(
            diff <= tolerance,
            "actual {actual} != expected {expected} (diff {diff}, tolerance {tolerance})"
        );
    }

    #[test]
    fn test_horizontal_interior_matches_hand_computation() {
        let sample = sample_horizontal(
            &regional_grid(),
            &oracle_field(),
            HorizontalStaggering::CellCenter,
            1.25,
            0.5,
        )
        .expect("interior query must succeed");
        assert_eq!((sample.ix, sample.jy, sample.ixp, sample.jyp), (1, 0, 2, 1));
        assert!((sample.weights[0] - 0.375).abs() < 1.0e-12);
        assert!((sample.weights[1] - 0.125).abs() < 1.0e-12);
        assert!((sample.weights[2] - 0.375).abs() < 1.0e-12);
        assert!((sample.weights[3] - 0.125).abs() < 1.0e-12);
        assert_relative(sample.value, 230.0);
    }

    #[test]
    fn test_horizontal_linear_field_is_exact() {
        // f(x,y) = 7 + 2x + 3y is reproduced exactly by bilinear interpolation.
        let grid = regional_grid();
        let field: Vec<f32> = (0..3)
            .flat_map(|y| (0..4).map(move |x| 7.0 + 2.0 * x as f32 + 3.0 * y as f32))
            .collect();
        for (xt, yt) in [(0.0, 0.0), (1.25, 0.5), (2.5, 1.5), (2.9, 1.0), (0.4, 2.0)] {
            let sample = sample_horizontal(&grid, &field, HorizontalStaggering::CellCenter, xt, yt)
                .expect("linear interior query must succeed");
            assert_relative(sample.value, 7.0 + 2.0 * xt + 3.0 * yt);
        }
    }

    #[test]
    fn test_horizontal_exact_grid_point_selects_single_corner() {
        let sample = sample_horizontal(
            &regional_grid(),
            &oracle_field(),
            HorizontalStaggering::CellCenter,
            2.0,
            1.0,
        )
        .expect("exact grid point must succeed");
        assert_eq!((sample.ix, sample.jy, sample.ixp, sample.jyp), (2, 1, 3, 2));
        assert_eq!(sample.weights, [1.0, 0.0, 0.0, 0.0]);
        assert_relative(sample.value, 310.0);
    }

    #[test]
    fn test_horizontal_north_edge_collapses_to_last_row() {
        let sample = sample_horizontal(
            &regional_grid(),
            &oracle_field(),
            HorizontalStaggering::CellCenter,
            2.9,
            2.0,
        )
        .expect("exact last row must succeed");
        assert_eq!((sample.ix, sample.jy, sample.ixp, sample.jyp), (2, 2, 3, 2));
        assert_relative(sample.value, 410.0);
    }

    #[test]
    fn test_horizontal_periodic_seam_uses_duplicate_column() {
        let grid = periodic_grid();
        let sample = sample_horizontal(
            &grid,
            &oracle_field(),
            HorizontalStaggering::CellCenter,
            3.2,
            1.5,
        )
        .expect("periodic seam query must succeed");
        assert_eq!((sample.ix, sample.jy, sample.ixp, sample.jyp), (3, 1, 4, 2));
        assert!(sample.is_periodic_x);
        assert_relative(sample.value, 355.0);
    }

    #[test]
    fn test_horizontal_geographic_mapping_matches_grid_path() {
        let grid = HorizontalGrid {
            nx: 4,
            ny: 3,
            xlon0_deg: -2.0,
            ylat0_deg: 48.0,
            dx_deg: 0.25,
            dy_deg: 0.25,
            longitude_domain: LongitudeDomain::Minus180To180,
        };
        let direct = sample_horizontal(
            &grid,
            &oracle_field(),
            HorizontalStaggering::CellCenter,
            1.25,
            0.5,
        )
        .expect("direct grid query must succeed");
        let geographic = sample_horizontal_geographic(
            &grid,
            &oracle_field(),
            HorizontalStaggering::CellCenter,
            -1.6875,
            48.125,
        )
        .expect("geographic query must succeed");
        assert_eq!(direct, geographic);
        assert_relative(geographic.value, 230.0);
    }

    #[test]
    fn test_horizontal_out_of_domain_fails_closed() {
        let grid = regional_grid();
        let field = oracle_field();
        for (xt, yt) in [
            (-0.1, 0.5),
            (0.5, -0.1),
            (3.5, 1.0),
            (1.0, 2.5),
            (4.0, 1.0),
            (f64::NAN, 0.5),
            (f64::INFINITY, 0.5),
        ] {
            assert!(
                matches!(
                    sample_horizontal(&grid, &field, HorizontalStaggering::CellCenter, xt, yt),
                    Err(HorizontalError::OutOfDomain { .. })
                        | Err(HorizontalError::ImpossibleCoordinate { .. })
                ),
                "query ({xt}, {yt}) must fail closed"
            );
        }
        // Periodic duplicate endpoint is explicitly out of the supported domain.
        assert!(matches!(
            sample_horizontal(
                &periodic_grid(),
                &field,
                HorizontalStaggering::CellCenter,
                4.0,
                1.0
            ),
            Err(HorizontalError::OutOfDomain { .. })
        ));
    }

    #[test]
    fn test_horizontal_unsupported_staggering_fails_closed() {
        let grid = regional_grid();
        let mut face_field = vec![0.0_f32; 5 * 3];
        face_field[0] = 1.0;
        assert_eq!(
            sample_horizontal(&grid, &face_field, HorizontalStaggering::XFace, 1.0, 1.0),
            Err(HorizontalError::UnsupportedStaggering {
                staggering: HorizontalStaggering::XFace
            })
        );
        assert_eq!(
            sample_horizontal(&grid, &face_field, HorizontalStaggering::YFace, 1.0, 1.0),
            Err(HorizontalError::UnsupportedStaggering {
                staggering: HorizontalStaggering::YFace
            })
        );
    }

    #[test]
    fn test_horizontal_malformed_grid_and_shape_fail_closed() {
        let mut grid = regional_grid();
        grid.nx = 0;
        assert!(matches!(
            sample_horizontal(&grid, &[], HorizontalStaggering::CellCenter, 0.0, 0.0),
            Err(HorizontalError::MalformedDimensions { .. })
        ));

        let grid = regional_grid();
        assert!(matches!(
            sample_horizontal(
                &grid,
                &vec![0.0; 11],
                HorizontalStaggering::CellCenter,
                1.0,
                1.0
            ),
            Err(HorizontalError::ShapeMismatch { .. })
        ));

        let mut bad_spacing = regional_grid();
        bad_spacing.dx_deg = 0.0;
        assert!(matches!(
            sample_horizontal(
                &bad_spacing,
                &oracle_field(),
                HorizontalStaggering::CellCenter,
                1.0,
                1.0
            ),
            Err(HorizontalError::InconsistentGridMetadata { .. })
        ));
    }

    #[test]
    fn test_horizontal_impossible_geographic_coordinate_fails_closed() {
        let grid = regional_grid();
        let field = oracle_field();
        assert_eq!(
            sample_horizontal_geographic(
                &grid,
                &field,
                HorizontalStaggering::CellCenter,
                0.5,
                91.0
            ),
            Err(HorizontalError::ImpossibleCoordinate {
                reason: "latitude outside [-90, 90] is impossible"
            })
        );
        assert!(matches!(
            sample_horizontal_geographic(
                &grid,
                &field,
                HorizontalStaggering::CellCenter,
                f64::NAN,
                0.5
            ),
            Err(HorizontalError::ImpossibleCoordinate { .. })
        ));
    }
}
