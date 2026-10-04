//! The current caller-owned meteorology bracket handoff.

use crate::wind::{SurfaceFields, WindField3D};

/// Per-step meteorological bracket used for temporal interpolation (IO-04).
pub struct MetTimeBracket<'a> {
    /// 3-D met field at lower timestamp bound.
    pub wind_t0: &'a WindField3D,
    /// 3-D met field at upper timestamp bound.
    pub wind_t1: &'a WindField3D,
    /// 2-D surface field at lower timestamp bound.
    pub surface_t0: &'a SurfaceFields,
    /// 2-D surface field at upper timestamp bound.
    pub surface_t1: &'a SurfaceFields,
    /// Lower timestamp bound [s since epoch].
    pub time_t0_seconds: i64,
    /// Upper timestamp bound [s since epoch].
    pub time_t1_seconds: i64,
}
