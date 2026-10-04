//! Forward time-loop orchestration (I-01).
//!
//! This module provides an MVP integration manager equivalent in role to
//! FLEXPART's `timemanager.f90` forward loop:
//! 1. release/update particles (A-06),
//! 2. interpolate met fields to current time (IO-04),
//! 3. derive PBL diagnostics from interpolated surface fields (IO-05),
//! 4. dispatch advection (A-04),
//! 5. dispatch Langevin turbulence (H-04),
//! 6. dispatch dry and wet deposition (D-02, D-04).
//!
//! It also provides a backward-mode MVP (`ldirect = -1` equivalent) used for
//! receptor-driven source attribution (B-01).
//!
//! ## MVP assumptions
//! - CBL-specific turbulence branch is not orchestrated yet.
//! - PBL profile cues are not temporally interpolated in this baseline; IO-05 is
//!   driven from interpolated surface fields only.
//! - Deposition forcing vectors are caller-provided per timestep.
//! - Backward mode currently reverses deterministic advection time direction,
//!   while keeping turbulence and deposition dispatch in forward-sign `dt`.
//!   This simplification is intentional for B-01 MVP and documented in the
//!   backward API docs below.

mod backward;
mod config;
mod error;
mod forcing;
mod forward;
mod meteorology;
mod options;
mod reports;
mod time;

pub use backward::BackwardTimeLoopDriver;
pub use config::{
    BackwardReceptorConfig, BackwardSourceCollection, BackwardSourceRegionConfig,
    BackwardTimeLoopConfig, ForwardSpatialSortConfig, ForwardTimeLoopConfig, TimeDirection,
};
pub use error::TimeLoopError;
pub use forcing::{ForwardStepForcing, ParticleForcingField};
pub use forward::ForwardTimeLoopDriver;
pub use meteorology::MetTimeBracket;
pub use reports::{BackwardStepReport, ForwardStepReport, StepTimingReport};

#[cfg(test)]
mod tests;
