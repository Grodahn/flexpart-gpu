//! Configuration parser for FLEXPART input files.
//!
//! This module implements a practical MVP parser for:
//! - `COMMAND` as a Fortran namelist (`&COMMAND ... /`)
//! - `RELEASES` either as repeated `&RELEASE ... /` blocks or line-based key/value records
//! - `OUTGRID` as `&OUTGRID ... /` or plain key/value content
//! - `SPECIES` as a directory containing one key/value (or `&SPECIES`) file per species
//!
//! The full FLEXPART grammar is significantly larger; this parser intentionally supports a
//! robust subset with clear error messages and typed validation to unblock integration.

// Domain modules stay private so callers share one canonical configuration facade.
mod command;
mod error;
mod output;
mod parsing;
mod release;
mod simulation;
mod species;

#[cfg(test)]
mod test_support;

pub use command::CommandConfig;
pub use error::ConfigError;
pub use output::OutputGridConfig;
pub use release::ReleaseConfig;
pub use simulation::SimulationConfig;
pub use species::{SpeciesConfig, SpeciesKind};
