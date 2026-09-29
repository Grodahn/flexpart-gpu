//! Canonical candidate/oracle input-equivalence verdict (Issue #52).
//!
//! Fail-closed gate deciding whether paired scientific scoring is allowed.
//! The authoritative machine-readable report is produced by
//! `scripts/corpus/input_equivalence.py` (schema
//! `schemas/input-equivalence-v1.schema.json`) from the #51 validation-case
//! contract plus resolved candidate/oracle inputs. This module only enforces
//! the verdict for downstream Rust scoring paths; it never re-derives
//! inputs, never hashes executables/outputs (owned by #53), and never
//! defines scientific metrics or thresholds.

use thiserror::Error;

/// Canonical overall input-equivalence states required by #52.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEquivalenceVerdict {
    /// Every required field is demonstrated equivalent.
    InputEquivalent,
    /// A declared representation difference, missing independent evidence,
    /// or unsupported transformation prevents an equivalence claim.
    InputEquivalenceNotDemonstrated,
    /// Resolved candidate/oracle inputs contradict the declared mapping.
    InputMismatch,
    /// Required manifest/artifact metadata is malformed or missing.
    IntegrityError,
}

impl InputEquivalenceVerdict {
    /// Parse the wire string emitted by the canonical Python report.
    ///
    /// # Errors
    /// Returns [`InputEquivalenceError::UnknownVerdict`] for any other string.
    pub fn parse(value: &str) -> Result<Self, InputEquivalenceError> {
        match value {
            "INPUT_EQUIVALENT" => Ok(Self::InputEquivalent),
            "INPUT_EQUIVALENCE_NOT_DEMONSTRATED" => {
                Ok(Self::InputEquivalenceNotDemonstrated)
            }
            "INPUT_MISMATCH" => Ok(Self::InputMismatch),
            "INTEGRITY_ERROR" => Ok(Self::IntegrityError),
            other => Err(InputEquivalenceError::UnknownVerdict(other.to_string())),
        }
    }

    /// Wire representation used in the machine-readable report.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InputEquivalent => "INPUT_EQUIVALENT",
            Self::InputEquivalenceNotDemonstrated => "INPUT_EQUIVALENCE_NOT_DEMONSTRATED",
            Self::InputMismatch => "INPUT_MISMATCH",
            Self::IntegrityError => "INTEGRITY_ERROR",
        }
    }
}

/// Errors for the input-equivalence gate.
#[derive(Debug, Error)]
pub enum InputEquivalenceError {
    /// The report carries an unknown verdict string.
    #[error("unknown input-equivalence verdict: {0}")]
    UnknownVerdict(String),
    /// Paired scoring was refused because the verdict is not equivalent.
    #[error("paired scientific scoring refused: verdict is {verdict} (INPUT_EQUIVALENT required)")]
    ScoringRefused {
        /// The non-equivalent verdict that caused the refusal.
        verdict: &'static str,
    },
}

/// Require `INPUT_EQUIVALENT` before paired scientific scoring.
///
/// Returns `Ok(())` only for [`InputEquivalenceVerdict::InputEquivalent`];
/// every other state is refused fail-closed. Provenance/build/executable
/// attribution owned by #53 must be verified separately.
///
/// # Errors
/// Returns [`InputEquivalenceError::ScoringRefused`] for every non-equivalent
/// verdict.
pub fn require_input_equivalent(
    verdict: InputEquivalenceVerdict,
) -> Result<(), InputEquivalenceError> {
    match verdict {
        InputEquivalenceVerdict::InputEquivalent => Ok(()),
        other => Err(InputEquivalenceError::ScoringRefused {
            verdict: other.as_str(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_equivalent_allows_scoring() {
        require_input_equivalent(InputEquivalenceVerdict::InputEquivalent)
            .expect("equivalent must allow scoring");
    }

    #[test]
    fn test_non_equivalent_states_refuse_scoring() {
        for verdict in [
            InputEquivalenceVerdict::InputEquivalenceNotDemonstrated,
            InputEquivalenceVerdict::InputMismatch,
            InputEquivalenceVerdict::IntegrityError,
        ] {
            let err = require_input_equivalent(verdict).expect_err("must refuse scoring");
            let rendered = err.to_string();
            assert!(
                rendered.contains(verdict.as_str()),
                "unexpected message: {rendered}"
            );
            assert!(rendered.contains("INPUT_EQUIVALENT required"));
        }
    }

    #[test]
    fn test_parse_round_trips_all_verdicts() {
        for verdict in [
            InputEquivalenceVerdict::InputEquivalent,
            InputEquivalenceVerdict::InputEquivalenceNotDemonstrated,
            InputEquivalenceVerdict::InputMismatch,
            InputEquivalenceVerdict::IntegrityError,
        ] {
            assert_eq!(
                InputEquivalenceVerdict::parse(verdict.as_str())
                    .expect("parse must succeed"),
                verdict
            );
        }
    }

    #[test]
    fn test_parse_rejects_unknown_verdict() {
        assert!(InputEquivalenceError::UnknownVerdict("PASS".to_string())
            .to_string()
            .contains("PASS"));
        assert!(InputEquivalenceVerdict::parse("PASS").is_err());
        assert!(InputEquivalenceVerdict::parse("").is_err());
    }
}
