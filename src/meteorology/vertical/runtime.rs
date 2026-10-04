//! Opaque canonical geometry and validated borrowed access for downstream sampling.

use super::super::VerticalStaggering;
use super::layout::{interface_offset, surface_offset, volume_offset};
use super::{NormalizedVerticalMotion, VerticalTransformError, VerticalTransformProvenance};
use serde::Serialize;

/// Final derived vertical runtime state consumed by later sampling/interpolation.
///
/// Heights are column-wise 3-D fields; they must never be collapsed to one
/// horizontally averaged vertical profile.
/// Construction and mutation stay within this vertical boundary so consumers
/// cannot recombine geometry from different Snapshots.
///
/// ```compile_fail
/// use flexpart_gpu::meteorology::vertical::VerticalTransformResult;
/// fn mutate_geometry(result: &mut VerticalTransformResult) {
///     result.height_agl_m.clear();
/// }
/// ```
///
/// ```compile_fail
/// use flexpart_gpu::meteorology::vertical::VerticalTransformResult;
/// let _ = serde_json::from_str::<VerticalTransformResult>("{}");
/// ```
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VerticalTransformResult {
    pub(super) nx: usize,
    pub(super) ny: usize,
    pub(super) nz: usize,
    pub(super) interface_pressure_pa: Vec<f32>,
    pub(super) interface_height_asl_m: Vec<f32>,
    pub(super) interface_height_agl_m: Vec<f32>,
    pub(super) level_pressure_pa: Vec<f32>,
    pub(super) terrain_asl_m: Vec<f32>,
    pub(super) height_asl_m: Vec<f32>,
    pub(super) height_agl_m: Vec<f32>,
    pub(super) vertical_velocity: Option<NormalizedVerticalMotion>,
    pub(super) provenance: VerticalTransformProvenance,
}

/// One model-level point exposed through the immutable #30 runtime boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalLevelPoint {
    pub pressure_pa: f32,
    pub height_agl_m: f32,
    pub height_asl_m: f32,
}

/// One W/interface point exposed through the immutable #30 runtime boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalInterfacePoint {
    pub pressure_pa: f32,
    pub height_agl_m: f32,
    pub height_asl_m: f32,
}

/// Borrowed, validated provider-independent runtime view intended for #31.
///
/// It exposes vertical geometry and motion without performing horizontal,
/// vertical, or temporal interpolation.
#[derive(Debug, Clone, Copy)]
pub struct VerticalRuntimeView<'a> {
    result: &'a VerticalTransformResult,
}

impl VerticalTransformResult {
    /// Validate all derived array shapes before exposing this result to #31.
    pub fn runtime_view(&self) -> Result<VerticalRuntimeView<'_>, VerticalTransformError> {
        let horizontal =
            self.nx
                .checked_mul(self.ny)
                .ok_or(VerticalTransformError::RuntimeShapeMismatch {
                    field: "horizontal",
                    expected: usize::MAX,
                    actual: 0,
                })?;
        let level_count = horizontal.checked_mul(self.nz).ok_or(
            VerticalTransformError::RuntimeShapeMismatch {
                field: "level_geometry",
                expected: usize::MAX,
                actual: 0,
            },
        )?;
        let interface_count = horizontal.checked_mul(self.nz + 1).ok_or(
            VerticalTransformError::RuntimeShapeMismatch {
                field: "interface_pressure_pa",
                expected: usize::MAX,
                actual: 0,
            },
        )?;

        for (field, actual, expected) in [
            ("terrain_asl_m", self.terrain_asl_m.len(), horizontal),
            (
                "level_pressure_pa",
                self.level_pressure_pa.len(),
                level_count,
            ),
            ("height_asl_m", self.height_asl_m.len(), level_count),
            ("height_agl_m", self.height_agl_m.len(), level_count),
            (
                "interface_pressure_pa",
                self.interface_pressure_pa.len(),
                interface_count,
            ),
            (
                "interface_height_asl_m",
                self.interface_height_asl_m.len(),
                interface_count,
            ),
            (
                "interface_height_agl_m",
                self.interface_height_agl_m.len(),
                interface_count,
            ),
        ] {
            if actual != expected {
                return Err(VerticalTransformError::RuntimeShapeMismatch {
                    field,
                    expected,
                    actual,
                });
            }
        }

        if let Some(motion) = &self.vertical_velocity {
            let expected = match motion.vertical_staggering {
                VerticalStaggering::LevelCenter => level_count,
                VerticalStaggering::LevelInterface => interface_count,
                VerticalStaggering::NotApplicable => {
                    return Err(VerticalTransformError::InvalidNativeVerticalMotion {
                        reason: "normalized vertical motion cannot use not_applicable staggering",
                    });
                }
            };
            if motion.values_ms.len() != expected {
                return Err(VerticalTransformError::RuntimeShapeMismatch {
                    field: "vertical_velocity.values_ms",
                    expected,
                    actual: motion.values_ms.len(),
                });
            }
        }

        Ok(VerticalRuntimeView { result: self })
    }
}

impl VerticalRuntimeView<'_> {
    /// Return the canonical horizontal and model-level counts for consumers.
    #[must_use]
    pub const fn dimensions(&self) -> (usize, usize, usize) {
        (self.result.nx, self.result.ny, self.result.nz)
    }

    /// Read local terrain in metres ASL without horizontal sampling.
    pub fn terrain_asl_m(&self, x: usize, y: usize) -> Result<f32, VerticalTransformError> {
        validate_xy(x, y, self.result.nx, self.result.ny)?;
        Ok(self.result.terrain_asl_m[surface_offset(x, y, self.result.nx)])
    }

    /// Read one model-level pressure and AGL/ASL height in canonical storage order.
    pub fn level(
        &self,
        x: usize,
        y: usize,
        z: usize,
    ) -> Result<VerticalLevelPoint, VerticalTransformError> {
        validate_xyz(x, y, z, self.result.nx, self.result.ny, self.result.nz)?;
        let index = volume_offset(x, y, z, self.result.nx, self.result.ny);
        Ok(VerticalLevelPoint {
            pressure_pa: self.result.level_pressure_pa[index],
            height_agl_m: self.result.height_agl_m[index],
            height_asl_m: self.result.height_asl_m[index],
        })
    }

    /// Read the distinct W/interface pressure and AGL/ASL height at its own index.
    pub fn interface(
        &self,
        x: usize,
        y: usize,
        interface: usize,
    ) -> Result<VerticalInterfacePoint, VerticalTransformError> {
        validate_xyz(
            x,
            y,
            interface,
            self.result.nx,
            self.result.ny,
            self.result.nz + 1,
        )?;
        let index = interface_offset(x, y, interface, self.result.nx, self.result.ny);
        Ok(VerticalInterfacePoint {
            pressure_pa: self.result.interface_pressure_pa[index],
            height_agl_m: self.result.interface_height_agl_m[index],
            height_asl_m: self.result.interface_height_asl_m[index],
        })
    }

    /// Read interface pressure in pascals while retaining interface identity.
    pub fn interface_pressure_pa(
        &self,
        x: usize,
        y: usize,
        interface: usize,
    ) -> Result<f32, VerticalTransformError> {
        Ok(self.interface(x, y, interface)?.pressure_pa)
    }

    /// Borrow upward-positive geometric motion with its retained staggering.
    #[must_use]
    pub fn vertical_velocity(&self) -> Option<&NormalizedVerticalMotion> {
        self.result.vertical_velocity.as_ref()
    }

    /// Borrow lineage binding the runtime geometry to its canonical source.
    #[must_use]
    pub const fn provenance(&self) -> &VerticalTransformProvenance {
        &self.result.provenance
    }
}

fn validate_xy(x: usize, y: usize, nx: usize, ny: usize) -> Result<(), VerticalTransformError> {
    if x >= nx || y >= ny {
        return Err(VerticalTransformError::RuntimeIndexOutOfBounds {
            x,
            y,
            z: None,
            nx,
            ny,
            nz: None,
        });
    }
    Ok(())
}

fn validate_xyz(
    x: usize,
    y: usize,
    z: usize,
    nx: usize,
    ny: usize,
    nz: usize,
) -> Result<(), VerticalTransformError> {
    if x >= nx || y >= ny || z >= nz {
        return Err(VerticalTransformError::RuntimeIndexOutOfBounds {
            x,
            y,
            z: Some(z),
            nx,
            ny,
            nz: Some(nz),
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/runtime.rs"]
mod tests;
