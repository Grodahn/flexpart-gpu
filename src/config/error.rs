//! Stable configuration failure variants and error-source reporting.

use std::path::PathBuf;
use thiserror::Error;

/// Typed failures that keep missing, malformed and invalid configuration fail closed.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing configuration path: {path}")]
    MissingPath { path: PathBuf },
    #[error("failed to read `{path}`: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error in {context}: {message}")]
    Parse { context: String, message: String },
    #[error("missing key `{key}` in {context}")]
    MissingKey { context: String, key: String },
    #[error("invalid value for `{key}` in {context}: `{value}` ({message})")]
    InvalidValue {
        context: String,
        key: String,
        value: String,
        message: String,
    },
    #[error("validation error: {message}")]
    Validation { message: String },
}
