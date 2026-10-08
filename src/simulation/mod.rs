//! Simulation orchestration modules.
//!
//! The integration layer wires already-implemented kernels and physics helpers
//! into end-to-end timestep workflows.

pub mod timeloop;

pub use timeloop::{
    BackwardReceptorConfig, BackwardSourceCollection, BackwardSourceRegionConfig,
    BackwardStepReport, BackwardTimeLoopConfig, BackwardTimeLoopDriver,
    CanonicalMeteorologyBracket, CanonicalMeteorologyIdentity, CanonicalMeteorologyResources,
    CanonicalMeteorologySlot, CanonicalMeteorologyStepTimes, ForwardSpatialSortConfig,
    ForwardStepForcing, ForwardStepReport, ForwardTimeLoopConfig, ForwardTimeLoopDriver,
    MetTimeBracket, ParticleForcingField, PreparedCanonicalMeteorology, StepTimingReport,
    TimeDirection, TimeLoopError, CANONICAL_WIND_FIELDS,
};
