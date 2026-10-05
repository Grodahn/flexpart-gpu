//! Canonical horizontal grid metadata and schema-v1 extent validation.
//!
//! Sampling and its error contract remain in `horizontal`; this module only
//! validates the serialized source grid and classifies canonical periodicity.

use super::ContractError;
use serde::{Deserialize, Serialize};

/// Describes scalar cell-center longitude/latitude samples in degrees.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HorizontalGrid {
    pub nx: usize,
    pub ny: usize,
    /// Longitude of the X=0 scalar/cell-center sample.
    ///
    /// Schema v1 fixes the horizontal origin at a cell center. An X-face is
    /// therefore located half a grid step west of the corresponding cell
    /// center, while a Y-face is half a grid step south of it.
    pub xlon0_deg: f64,
    /// Latitude of the Y=0 scalar/cell-center sample.
    pub ylat0_deg: f64,
    pub dx_deg: f64,
    pub dy_deg: f64,
    pub longitude_domain: LongitudeDomain,
}

impl HorizontalGrid {
    /// Whether schema-v1 X coordinates wrap periodically.
    ///
    /// Periodicity is canonical rather than provider metadata: an X grid is
    /// periodic iff its cell coverage nx * dx is exactly 360 degrees within
    /// the schema tolerance. All other grids are non-periodic and must fit
    /// wholly inside the declared longitude domain.
    #[must_use]
    pub fn is_periodic_x(&self) -> bool {
        grid_close(self.dx_deg * self.nx as f64, 360.0)
    }
}

/// Declares the canonical longitude convention for a source grid.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LongitudeDomain {
    Minus180To180,
    ZeroTo360,
}

pub(super) fn validate_grid(grid: &HorizontalGrid) -> Result<(), ContractError> {
    if grid.nx == 0
        || grid.ny == 0
        || !grid.dx_deg.is_finite()
        || !grid.dy_deg.is_finite()
        || grid.dx_deg <= 0.0
        || grid.dy_deg <= 0.0
        || !grid.xlon0_deg.is_finite()
        || !grid.ylat0_deg.is_finite()
        || !(-90.0..=90.0).contains(&grid.ylat0_deg)
    {
        return Err(ContractError::InvalidHorizontalGrid);
    }

    // Schema v1 anchors xlon0/ylat0 at scalar cell centers. Validate the
    // complete Y cell coverage rather than only the origin so #31 never has to
    // guess how an apparently valid grid behaves beyond a pole.
    let south_edge = grid.ylat0_deg - 0.5 * grid.dy_deg;
    let north_edge = grid.ylat0_deg + (grid.ny.saturating_sub(1) as f64 + 0.5) * grid.dy_deg;
    if south_edge < -90.0 - GRID_TOLERANCE_DEG || north_edge > 90.0 + GRID_TOLERANCE_DEG {
        return Err(ContractError::InvalidHorizontalGrid);
    }

    let (lon_min, lon_max, origin_valid) = match grid.longitude_domain {
        LongitudeDomain::Minus180To180 => {
            (-180.0, 180.0, (-180.0..=180.0).contains(&grid.xlon0_deg))
        }
        LongitudeDomain::ZeroTo360 => (0.0, 360.0, (0.0..360.0).contains(&grid.xlon0_deg)),
    };
    if !origin_valid {
        return Err(ContractError::InvalidHorizontalGrid);
    }

    let x_coverage = grid.dx_deg * grid.nx as f64;
    if x_coverage > 360.0 + GRID_TOLERANCE_DEG {
        return Err(ContractError::InvalidHorizontalGrid);
    }

    // Exactly-global cell coverage is the one supported periodic topology.
    // Regional grids are explicitly non-periodic and may not cross the
    // longitude-domain seam.
    if !grid.is_periodic_x() {
        let west_edge = grid.xlon0_deg - 0.5 * grid.dx_deg;
        let east_edge = grid.xlon0_deg + (grid.nx.saturating_sub(1) as f64 + 0.5) * grid.dx_deg;
        if west_edge < lon_min - GRID_TOLERANCE_DEG || east_edge > lon_max + GRID_TOLERANCE_DEG {
            return Err(ContractError::InvalidHorizontalGrid);
        }
    }
    Ok(())
}

const GRID_TOLERANCE_DEG: f64 = 1.0e-9;

fn grid_close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() <= GRID_TOLERANCE_DEG
}
