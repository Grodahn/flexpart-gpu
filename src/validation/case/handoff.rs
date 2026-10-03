//! External contract references, representation limitations and evidence declarations.

use super::{ValidationCaseError, ValidationCaseManifest};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// Stable identity of the frozen #49 FLEXPART oracle execution profile.
pub const ORACLE_EXECUTION_PROFILE_ID: &str = "flexpart-11.1-single-thread";

pub const ORACLE_EXECUTION_PROFILE_VERSION: u32 = 1;

pub const ORACLE_EXECUTION_PROFILE_PATH: &str = "reference/flexpart-11.1.json";

/// Stable candidate-side physics/runtime profile used by validation runners.
/// The case manifest references this profile so PBL/integration behavior never
/// comes from Rust `Default` implementations.
pub const CANDIDATE_PHYSICS_PROFILE_ID: &str = "candidate-forward-timeloop-v1";

pub const CANDIDATE_PHYSICS_PROFILE_VERSION: u32 = 1;

pub const CANDIDATE_PHYSICS_PROFILE_PATH: &str =
    "reference/candidate-physics/candidate-forward-timeloop-v1.json";

/// Whether this case actually executes the FLEXPART oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleExecutionPolicy {
    Required,
    NotApplicable,
}

/// Stable reference to a metric/threshold definition owned outside #51.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationDefinitionRef {
    pub id: String,
    pub version: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationDefinitionRefs {
    pub metric_contracts: Vec<ValidationDefinitionRef>,
    pub threshold_contracts: Vec<ValidationDefinitionRef>,
}

/// Execution profile reference (from frozen #49 contract).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProfileRef {
    /// Profile ID (e.g., "flexpart-11.1-single-thread").
    pub id: String,
    /// Profile version.
    pub version: u32,
    /// Manifest file path for verification.
    pub manifest_path: String,
}

/// Stable reference to the candidate-side physics/runtime profile consumed by
/// validation runners. Shared PBL, substep and synthetic thermodynamic-state
/// settings live in the referenced profile rather than executable defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePhysicsProfileRef {
    pub id: String,
    pub version: u32,
    pub manifest_path: String,
}

impl CandidatePhysicsProfileRef {
    #[must_use]
    pub fn canonical() -> Self {
        Self {
            id: CANDIDATE_PHYSICS_PROFILE_ID.to_string(),
            version: CANDIDATE_PHYSICS_PROFILE_VERSION,
            manifest_path: CANDIDATE_PHYSICS_PROFILE_PATH.to_string(),
        }
    }
}

/// Producer namespace for a required validation artifact.
///
/// Paths, hashes, and concrete immutable artifact identities are intentionally
/// not part of the case contract; #53 binds these declarations to actual run
/// artifacts and provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactProducer {
    Candidate,
    Oracle,
    ValidationPipeline,
}

/// Semantic class of a required validation artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactClass {
    RawModelOutput,
    DecodedModelOutput,
    ComparisonReport,
    RunManifest,
}

/// One stable artifact requirement declared by the case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedArtifactRequirement {
    pub id: String,
    pub producer: ArtifactProducer,
    pub class: ArtifactClass,
}

/// Required artifact classes for a validation case.
/// #51 declares what evidence must exist; #53 owns path/hash attribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedArtifacts {
    pub required: Vec<ExpectedArtifactRequirement>,
}

/// A known limitation that #52 must consider when proving input equivalence.
/// This is declarative case context, not an equivalence verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputEquivalenceLimitation {
    pub code: String,
    pub description: String,
}

/// Structured representation differences for #52 input-equivalence evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RepresentationDifferences {
    /// Vertical coordinate differences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertical_coordinate: Option<String>,
    /// Horizontal grid differences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizontal_grid: Option<String>,
    /// Temporal resolution differences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal_resolution: Option<String>,
    /// Wind component differences (e.g., etadot vs omega-derived w).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wind_components: Option<String>,
    /// PBL diagnostic differences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pbl_diagnostics: Option<String>,
    /// Known input-equivalence limitations carried forward for #52.
    /// Absence does not imply demonstrated equivalence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_input_equivalence_limitations: Vec<InputEquivalenceLimitation>,
    /// Additional notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<String>>,
}

impl ValidationCaseManifest {
    pub(super) fn validate_representation_differences(&self) -> Result<(), ValidationCaseError> {
        for limitation in &self
            .representation_differences
            .known_input_equivalence_limitations
        {
            if limitation.code.trim().is_empty() || limitation.description.trim().is_empty() {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "representation_differences.known_input_equivalence_limitations",
                    message: "limitation code and description must not be empty".to_string(),
                });
            }
        }
        Ok(())
    }

    pub(super) fn validate_expected_artifacts(&self) -> Result<(), ValidationCaseError> {
        if self.expected_artifacts.required.is_empty() {
            return Err(ValidationCaseError::MissingField {
                field: "expected_artifacts.required",
            });
        }
        let mut ids = HashSet::new();
        let mut candidate_raw = false;
        let mut candidate_decoded = false;
        let mut oracle_raw = false;
        let mut oracle_decoded = false;
        let mut comparison_report = false;
        let mut run_manifest = false;
        for artifact in &self.expected_artifacts.required {
            if artifact.id.trim().is_empty() {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "expected_artifacts.required.id",
                    message: "artifact id must not be empty".to_string(),
                });
            }
            if !ids.insert(artifact.id.as_str()) {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "expected_artifacts.required.id",
                    message: format!("duplicate artifact id {}", artifact.id),
                });
            }
            match (artifact.producer, artifact.class) {
                (ArtifactProducer::Candidate, ArtifactClass::RawModelOutput) => {
                    candidate_raw = true
                }
                (ArtifactProducer::Candidate, ArtifactClass::DecodedModelOutput) => {
                    candidate_decoded = true
                }
                (ArtifactProducer::Oracle, ArtifactClass::RawModelOutput) => oracle_raw = true,
                (ArtifactProducer::Oracle, ArtifactClass::DecodedModelOutput) => {
                    oracle_decoded = true
                }
                (ArtifactProducer::ValidationPipeline, ArtifactClass::ComparisonReport) => {
                    comparison_report = true
                }
                (ArtifactProducer::ValidationPipeline, ArtifactClass::RunManifest) => {
                    run_manifest = true
                }
                _ => {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "expected_artifacts.required",
                        message: format!(
                            "artifact {} has invalid producer/class pairing {:?}/{:?}",
                            artifact.id, artifact.producer, artifact.class
                        ),
                    })
                }
            }
        }
        for (present, label) in [
            (candidate_raw, "candidate/raw_model_output"),
            (candidate_decoded, "candidate/decoded_model_output"),
            (comparison_report, "validation_pipeline/comparison_report"),
            (run_manifest, "validation_pipeline/run_manifest"),
        ] {
            if !present {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "expected_artifacts.required",
                    message: format!("missing required artifact class {label}"),
                });
            }
        }
        match self.oracle_execution {
            OracleExecutionPolicy::Required => {
                for (present, label) in [
                    (oracle_raw, "oracle/raw_model_output"),
                    (oracle_decoded, "oracle/decoded_model_output"),
                ] {
                    if !present {
                        return Err(ValidationCaseError::AmbiguousField {
                            field: "expected_artifacts.required",
                            message: format!("missing required artifact class {label}"),
                        });
                    }
                }
            }
            OracleExecutionPolicy::NotApplicable => {
                if oracle_raw || oracle_decoded {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "expected_artifacts.required",
                        message:
                            "oracle_execution=not_applicable forbids oracle raw/decoded artifacts"
                                .to_string(),
                    });
                }
                if self.stochastic.oracle_seed.is_some() {
                    return Err(ValidationCaseError::InvalidStochasticIdentity {
                        message:
                            "oracle_execution=not_applicable requires stochastic.oracle_seed=null"
                                .to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) fn validate_validation_definition_refs(&self) -> Result<(), ValidationCaseError> {
        let validate = |field: &'static str,
                        refs: &[ValidationDefinitionRef]|
         -> Result<(), ValidationCaseError> {
            if refs.is_empty() {
                return Err(ValidationCaseError::MissingField { field });
            }
            for reference in refs {
                if reference.id.is_empty()
                    || reference.version.is_empty()
                    || reference.path.is_empty()
                {
                    return Err(ValidationCaseError::AmbiguousField {
                        field,
                        message: "definition references require non-empty id/version/path"
                            .to_string(),
                    });
                }
                if Path::new(&reference.path).is_absolute()
                    || reference.path.split('/').any(|segment| segment == "..")
                {
                    return Err(ValidationCaseError::AmbiguousField {
                            field,
                            message: format!(
                                "definition reference path must be repository-relative without '..': {}",
                                reference.path
                            ),
                        });
                }
            }
            Ok(())
        };
        validate(
            "validation_definition_refs.metric_contracts",
            &self.validation_definition_refs.metric_contracts,
        )?;
        validate(
            "validation_definition_refs.threshold_contracts",
            &self.validation_definition_refs.threshold_contracts,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        load_validation_case_schema, make_minimal_manifest, minimal_manifest_json,
        parse_json_value, validate_json_schema_subset,
    };

    use super::super::*;
    use std::path::Path;

    #[test]
    fn oracle_execution_policy_controls_oracle_artifact_requirements() {
        let mut required = make_minimal_manifest();
        required.expected_artifacts.required.retain(|artifact| {
            artifact.producer != ArtifactProducer::Oracle
                || artifact.class != ArtifactClass::DecodedModelOutput
        });
        let err = required
            .validate()
            .expect_err("required oracle decoded artifact must not be optional");
        assert!(err.to_string().contains("oracle/decoded_model_output"));

        let repeat_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/corpus/cases/REPEAT-009.json");
        let repeat = ValidationCaseManifest::load_from_file(&repeat_path)
            .expect("REPEAT-009 candidate-only case must validate");
        assert_eq!(
            repeat.oracle_execution,
            OracleExecutionPolicy::NotApplicable
        );
        assert!(repeat.stochastic.oracle_seed.is_none());
        assert!(repeat
            .expected_artifacts
            .required
            .iter()
            .all(|artifact| { artifact.producer != ArtifactProducer::Oracle }));
    }

    #[test]
    fn candidate_physics_profile_reference_is_pinned() {
        let mut manifest = make_minimal_manifest();
        manifest
            .validate()
            .expect("canonical candidate profile validates");
        manifest.candidate_physics_profile.version += 1;
        let err = manifest
            .validate()
            .expect_err("wrong candidate profile must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidCandidatePhysicsProfile { .. }
        ));
    }

    #[test]
    fn validation_definition_refs_fail_closed() {
        let mut manifest = make_minimal_manifest();
        manifest.validation_definition_refs.metric_contracts.clear();
        assert!(manifest
            .validate()
            .expect_err("missing metric refs")
            .to_string()
            .contains("metric_contracts"));

        let mut manifest = make_minimal_manifest();
        manifest.validation_definition_refs.threshold_contracts[0].path = "../bad.json".to_string();
        assert!(manifest
            .validate()
            .expect_err("parent path")
            .to_string()
            .contains("repository-relative"));
    }

    #[test]
    fn case_level_input_equivalence_verdict_is_rejected() {
        let mut raw = minimal_manifest_json();
        raw.as_object_mut().expect("manifest object").insert(
            "input_equivalence".to_string(),
            serde_json::json!("demonstrated"),
        );
        let err = parse_json_value(&raw).expect_err("#52 verdict must not live in #51 manifest");
        assert!(err.to_string().contains("input_equivalence"));
    }

    #[test]
    fn expected_artifact_paths_are_rejected() {
        let mut raw = minimal_manifest_json();
        raw["expected_artifacts"] = serde_json::json!({"candidate_dir":"target/corpus/candidate/TEST-001","comparison_report":"target/corpus/comparison_report.json","run_manifest":"target/corpus/run_manifest.json"});
        let err = parse_json_value(&raw).expect_err("paths belong to #53, not #51");
        assert!(err.to_string().contains("candidate_dir"));
    }

    #[test]
    fn expected_artifacts_require_candidate_raw_and_decoded_classes() {
        let mut manifest = make_minimal_manifest();
        manifest.expected_artifacts.required.retain(|artifact| {
            !(artifact.producer == ArtifactProducer::Candidate
                && artifact.class == ArtifactClass::DecodedModelOutput)
        });
        let err = manifest
            .validate()
            .expect_err("candidate decoded artifact is required");
        assert!(err.to_string().contains("candidate/decoded_model_output"));
    }

    #[test]
    fn expected_artifacts_require_oracle_raw_and_decoded_classes() {
        for (class, label) in [
            (ArtifactClass::RawModelOutput, "oracle/raw_model_output"),
            (
                ArtifactClass::DecodedModelOutput,
                "oracle/decoded_model_output",
            ),
        ] {
            let mut manifest = make_minimal_manifest();
            manifest.expected_artifacts.required.retain(|artifact| {
                !(artifact.producer == ArtifactProducer::Oracle && artifact.class == class)
            });
            let err = manifest
                .validate()
                .expect_err("oracle raw/decoded artifacts are required");
            assert!(err.to_string().contains(label), "unexpected error: {err}");
        }
    }

    #[test]
    fn representation_differences_are_required_in_rust_contract() {
        let mut raw = minimal_manifest_json();
        raw.as_object_mut()
            .expect("manifest object")
            .remove("representation_differences");
        let err = parse_json_value(&raw).expect_err("missing representation_differences must fail");
        assert!(
            err.to_string().contains("representation_differences"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn execution_profile_must_match_frozen_issue49_reference() {
        let schema = load_validation_case_schema();
        for (field, value) in [
            ("id", serde_json::json!("some-other-profile")),
            ("version", serde_json::json!(2)),
            ("manifest_path", serde_json::json!("reference/other.json")),
            ("manifest_path", serde_json::json!("")),
        ] {
            let mut raw = minimal_manifest_json();
            raw["execution_profile"][field] = value;
            assert!(
                validate_json_schema_subset(&schema, &schema, &raw, "$").is_err(),
                "JSON Schema must reject execution-profile drift in {field}"
            );
            let err = parse_json_value(&raw).expect_err("execution profile drift must fail");
            assert!(
                err.to_string().contains("execution_profile"),
                "unexpected: {err}"
            );
        }
    }
}
