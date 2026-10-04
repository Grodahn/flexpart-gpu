//! Stable provider-independent canonical meteorology facade.
//!
//! Provider/file adapters normalize into these types before validation. Physics
//! consumers use this boundary without provider names or implicit unit/sign rules.
//!
//! Private metadata owners define schema/fields, source grids/coordinates/times
//! and snapshot validation. Public domain modules retain their established
//! sampling, accumulated-interval and derived-runtime-geometry contracts.

mod coordinate;
mod field;
mod grid;
mod schema;
mod snapshot;
mod time;

pub mod accumulation;
pub mod horizontal;
pub mod temporal;
pub mod vertical;
pub mod vertical_sampling;

pub use coordinate::{
    VerticalCoordinate, VerticalCoordinateKind, VerticalOrdering, VerticalReference,
    VerticalStaggering,
};
pub use field::{
    Axis, Field, FieldId, FieldSpec, HorizontalStaggering, RequirementSet, Requirements,
    SignConvention, StorageOrder, TemporalPolicy, Unit, FIELD_SPECS, FLEXPART_LAND_USE_CLASS_COUNT,
};
pub use grid::{HorizontalGrid, LongitudeDomain};
pub use schema::{ContractError, SchemaIdentity, SCHEMA_ID, SCHEMA_VERSION};
pub use snapshot::{Provenance, Snapshot};
pub use time::{Accumulation, Calendar, FieldTime, TemporalKind};
