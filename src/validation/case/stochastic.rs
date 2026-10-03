//! Separate candidate/oracle RNG namespaces and executable identity preparation.

use super::{ValidationCaseError, ValidationCaseManifest};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Stable identity of the completed #50 oracle stochastic-identity contract.
/// Case manifests reference this contract and never duplicate its
/// requested-identity -> FLEXPART RNG-state mapping.
pub const ORACLE_STOCHASTIC_STRATEGY_ID: &str = "flexpart-oracle-validation-seed-offset";

pub const ORACLE_STOCHASTIC_STRATEGY_VERSION: u32 = 1;

pub const ORACLE_STOCHASTIC_CONTRACT_PATH: &str = "reference/oracle-stochastic-identity.json";

/// Oracle kind as defined in issue #50 stochastic identity contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OracleKind {
    /// Unmodified FLEXPART 11.1 at pinned commit, no seed control.
    PristineOracle,
    /// Patched FLEXPART 11.1 with validation-only RNG initialization.
    SeedableValidationOracle,
}

impl FromStr for OracleKind {
    type Err = ValidationCaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pristine-oracle" => Ok(OracleKind::PristineOracle),
            "seedable-validation-oracle" => Ok(OracleKind::SeedableValidationOracle),
            _ => Err(ValidationCaseError::InvalidOracleKind(s.to_string())),
        }
    }
}

/// Stochastic identity specification for a validation case.
///
/// Candidate Philox identities and FLEXPART oracle identities are separate
/// RNG namespaces per issue #50 contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct StochasticIdentitySpec {
    /// Candidate RNG namespace: Philox key/counter for the GPU candidate.
    /// Canonical JSON must carry this field explicitly; deterministic cases use null.
    pub candidate_philox: Option<CandidatePhiloxIdentity>,
    /// Oracle RNG namespace: validation seed identity for FLEXPART oracle.
    /// Canonical JSON must carry this field explicitly; cases with no oracle RNG use null.
    pub oracle_seed: Option<OracleSeedIdentity>,
}

/// Versioned candidate-side Philox identity derivation.
///
/// This enum is executable contract data, not documentation. Adding a new
/// derivation requires a new explicit variant and corresponding runner/audit
/// semantics; unknown strings fail deserialization closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidatePhiloxDerivation {
    /// Seed i uses [base_key[0] + i (wrapping u32), base_key[1]] and the
    /// declared base counter.
    #[serde(rename = "wrapping_add_key0_v1")]
    WrappingAddKey0V1,
    /// Every ensemble member reuses the exact declared base key and counter.
    /// Used by REPEAT-009 to prove bit-identical reruns.
    #[serde(rename = "reuse_base_identity_v1")]
    ReuseBaseIdentityV1,
}

/// Candidate-side Philox identity (separate RNG namespace from oracle).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePhiloxIdentity {
    /// Base Philox key [key0, key1] for seed derivation.
    pub base_key: [u32; 2],
    /// Base Philox counter for the first timestep.
    pub base_counter: [u32; 4],
    /// Number of ensemble identities/repetitions.
    pub count: u32,
    /// Versioned executable derivation policy.
    pub derivation: CandidatePhiloxDerivation,
}

impl CandidatePhiloxIdentity {
    /// Derive the Philox key for seed index `i` from the declared policy.
    #[must_use]
    pub fn key_for_seed_index(&self, seed_index: u32) -> [u32; 2] {
        match self.derivation {
            CandidatePhiloxDerivation::WrappingAddKey0V1 => {
                [self.base_key[0].wrapping_add(seed_index), self.base_key[1]]
            }
            CandidatePhiloxDerivation::ReuseBaseIdentityV1 => self.base_key,
        }
    }

    /// Derive the Philox counter for seed index `i` from the declared policy.
    #[must_use]
    pub fn counter_for_seed_index(&self, _seed_index: u32) -> [u32; 4] {
        match self.derivation {
            CandidatePhiloxDerivation::WrappingAddKey0V1
            | CandidatePhiloxDerivation::ReuseBaseIdentityV1 => self.base_counter,
        }
    }
}

/// Stable reference to the completed #50 seedable-oracle strategy contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleStrategyRef {
    pub strategy: String,
    pub version: u32,
    pub contract_path: String,
}

impl OracleStrategyRef {
    #[must_use]
    pub fn canonical() -> Self {
        Self {
            strategy: ORACLE_STOCHASTIC_STRATEGY_ID.to_string(),
            version: ORACLE_STOCHASTIC_STRATEGY_VERSION,
            contract_path: ORACLE_STOCHASTIC_CONTRACT_PATH.to_string(),
        }
    }
}

/// Closed oracle identity mode. The mode is explicit so neither an omitted
/// seed nor a null seed carries hidden semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleSeedMode {
    /// Normative/default oracle mode. `seed` must be explicit null.
    Default,
    /// Validation-only requested identity from the #50 seedable strategy.
    RequestedIdentity,
}

/// Deserialize an explicitly present nullable field.
///
/// Applying this with `deserialize_with` keeps the field required at the wire
/// level while allowing its value to be JSON null.
fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// Oracle-side stochastic identity per issue #50 contract.
///
/// Every field is serialized explicitly. In particular, `strategy` and `seed`
/// serialize as null when absent, while `mode` states why the seed is null.
/// Custom deserialization keeps explicit null distinct from an omitted key even
/// for callers that use `serde_json::from_str::<ValidationCaseManifest>()`
/// directly rather than the canonical `ValidationCaseManifest::parse` helper.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OracleSeedIdentity {
    /// Oracle kind: pristine-oracle or seedable-validation-oracle.
    pub kind: OracleKind,
    /// Exact #50 strategy for seedable-validation-oracle; explicit null for pristine.
    pub strategy: Option<OracleStrategyRef>,
    /// Explicit mode: default or requested_identity.
    pub mode: OracleSeedMode,
    /// Requested identity [1, 1000000000], or explicit null in default mode.
    pub seed: Option<u32>,
    /// Number of repetitions for repeatability characterization.
    /// Required explicitly; no workflow/default repetition count is implied.
    pub repetitions: u32,
}

impl<'de> Deserialize<'de> for OracleSeedIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            kind: OracleKind,
            #[serde(deserialize_with = "deserialize_required_nullable")]
            strategy: Option<OracleStrategyRef>,
            mode: OracleSeedMode,
            #[serde(deserialize_with = "deserialize_required_nullable")]
            seed: Option<u32>,
            repetitions: u32,
        }

        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            kind: wire.kind,
            strategy: wire.strategy,
            mode: wire.mode,
            seed: wire.seed,
            repetitions: wire.repetitions,
        })
    }
}

impl ValidationCaseManifest {
    pub(super) fn validate_stochastic(&self) -> Result<(), ValidationCaseError> {
        if let Some(candidate) = &self.stochastic.candidate_philox {
            if candidate.count == 0 {
                return Err(ValidationCaseError::InvalidStochasticIdentity {
                    message: "candidate_philox.count must be > 0".to_string(),
                });
            }
        }

        if let Some(oracle) = &self.stochastic.oracle_seed {
            if oracle.repetitions == 0 {
                return Err(ValidationCaseError::InvalidStochasticIdentity {
                    message: "oracle_seed.repetitions must be > 0".to_string(),
                });
            }
            match oracle.kind {
                OracleKind::PristineOracle => {
                    if oracle.strategy.is_some() {
                        return Err(ValidationCaseError::InvalidStochasticIdentity {
                            message: "pristine-oracle requires strategy=null".to_string(),
                        });
                    }
                    if oracle.mode != OracleSeedMode::Default {
                        return Err(ValidationCaseError::InvalidStochasticIdentity {
                            message: "pristine-oracle requires mode=default".to_string(),
                        });
                    }
                    if oracle.seed.is_some() {
                        return Err(ValidationCaseError::InvalidStochasticIdentity {
                            message: "pristine-oracle default mode requires seed=null".to_string(),
                        });
                    }
                }
                OracleKind::SeedableValidationOracle => {
                    let strategy = oracle.strategy.as_ref().ok_or_else(|| {
                        ValidationCaseError::InvalidStochasticIdentity {
                            message: "seedable-validation-oracle requires the explicit #50 strategy reference".to_string(),
                        }
                    })?;
                    if strategy.strategy != ORACLE_STOCHASTIC_STRATEGY_ID
                        || strategy.version != ORACLE_STOCHASTIC_STRATEGY_VERSION
                        || strategy.contract_path != ORACLE_STOCHASTIC_CONTRACT_PATH
                    {
                        return Err(ValidationCaseError::InvalidStochasticIdentity {
                            message: format!(
                                "unsupported oracle strategy reference: expected {} v{} at {}, got {} v{} at {}",
                                ORACLE_STOCHASTIC_STRATEGY_ID,
                                ORACLE_STOCHASTIC_STRATEGY_VERSION,
                                ORACLE_STOCHASTIC_CONTRACT_PATH,
                                strategy.strategy,
                                strategy.version,
                                strategy.contract_path
                            ),
                        });
                    }
                    match oracle.mode {
                        OracleSeedMode::Default => {
                            if oracle.seed.is_some() {
                                return Err(ValidationCaseError::InvalidStochasticIdentity {
                                    message:
                                        "seedable-validation-oracle mode=default requires seed=null"
                                            .to_string(),
                                });
                            }
                        }
                        OracleSeedMode::RequestedIdentity => {
                            let seed = oracle.seed.ok_or_else(|| {
                                ValidationCaseError::InvalidStochasticIdentity {
                                    message: "seedable-validation-oracle mode=requested_identity requires an explicit seed".to_string(),
                                }
                            })?;
                            if seed == 0 || seed > 1_000_000_000 {
                                return Err(ValidationCaseError::InvalidStochasticIdentity {
                                    message: format!(
                                        "oracle_seed.seed must be in [1, 1000000000], got {seed}"
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }

        if self.physics_switches.turbulence && self.stochastic.candidate_philox.is_none() {
            return Err(ValidationCaseError::InvalidStochasticIdentity {
                message: "physics_switches.turbulence=true requires an explicit candidate Philox identity in stochastic.candidate_philox; an oracle_seed belongs to a separate RNG namespace and cannot satisfy the candidate requirement".to_string(),
            });
        }
        Ok(())
    }

    /// Resolve the candidate Philox key/counter for seed index `i`.
    ///
    /// Fail-closed: returns [`ValidationCaseError::InvalidStochasticIdentity`]
    /// when no candidate RNG identity is declared. Deterministic cases
    /// (e.g. `ADV-ANA-001`) must not call this; they declare no identity.
    ///
    /// # Errors
    /// Returns [`ValidationCaseError::InvalidStochasticIdentity`] if
    /// `stochastic.candidate_philox` is missing or `seed_index` is outside
    /// the declared candidate ensemble.
    pub fn candidate_seed_identity(
        &self,
        seed_index: u32,
    ) -> Result<([u32; 2], [u32; 4]), ValidationCaseError> {
        let Some(candidate) = &self.stochastic.candidate_philox else {
            return Err(ValidationCaseError::InvalidStochasticIdentity {
                message: format!(
                    "case {} declares no stochastic.candidate_philox; stochastic cases must declare a Philox identity, deterministic cases must not request one",
                    self.case_id
                ),
            });
        };
        if seed_index >= candidate.count {
            return Err(ValidationCaseError::InvalidStochasticIdentity {
                message: format!(
                    "case {} candidate seed_index {} outside declared ensemble [0, {})",
                    self.case_id, seed_index, candidate.count
                ),
            });
        }
        Ok((
            candidate.key_for_seed_index(seed_index),
            candidate.counter_for_seed_index(seed_index),
        ))
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
    fn invalid_oracle_seed_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed = Some(OracleSeedIdentity {
            kind: OracleKind::SeedableValidationOracle,
            strategy: Some(OracleStrategyRef::canonical()),
            mode: OracleSeedMode::RequestedIdentity,
            seed: Some(0), // Invalid: 0 is rejected per #50 contract
            repetitions: 5,
        });
        let err = manifest.validate().expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidStochasticIdentity { .. }
        ));
    }

    #[test]
    fn invalid_oracle_seed_too_large_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed = Some(OracleSeedIdentity {
            kind: OracleKind::SeedableValidationOracle,
            strategy: Some(OracleStrategyRef::canonical()),
            mode: OracleSeedMode::RequestedIdentity,
            seed: Some(1_000_000_001), // Invalid: > 1e9
            repetitions: 5,
        });
        let err = manifest.validate().expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidStochasticIdentity { .. }
        ));
    }

    #[test]
    fn pristine_oracle_default_mode_is_seedless() {
        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed = Some(OracleSeedIdentity {
            kind: OracleKind::PristineOracle,
            strategy: None,
            mode: OracleSeedMode::Default,
            seed: None,
            repetitions: 2,
        });
        manifest
            .validate()
            .expect("pristine default mode validates");
        manifest.stochastic.oracle_seed.as_mut().unwrap().seed = Some(1);
        let err = manifest.validate().expect_err("pristine seed must fail");
        assert!(err.to_string().contains("pristine-oracle"));
    }

    #[test]
    fn seedable_oracle_requires_exact_issue50_reference() {
        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed.as_mut().unwrap().strategy = None;
        let err = manifest.validate().expect_err("missing strategy must fail");
        assert!(err.to_string().contains("#50 strategy"));

        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed.as_mut().unwrap().strategy = Some(OracleStrategyRef {
            strategy: "wrong".to_string(),
            version: 1,
            contract_path: ORACLE_STOCHASTIC_CONTRACT_PATH.to_string(),
        });
        let err = manifest.validate().expect_err("wrong strategy must fail");
        assert!(err
            .to_string()
            .contains("unsupported oracle strategy reference"));

        let mut manifest = make_minimal_manifest();
        {
            let oracle = manifest.stochastic.oracle_seed.as_mut().unwrap();
            oracle.mode = OracleSeedMode::Default;
            oracle.seed = None;
        }
        manifest
            .validate()
            .expect("seedable default-equivalent mode validates");
    }

    #[test]
    fn oracle_default_mode_serializes_explicit_null_state() {
        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed = Some(OracleSeedIdentity {
            kind: OracleKind::PristineOracle,
            strategy: None,
            mode: OracleSeedMode::Default,
            seed: None,
            repetitions: 1,
        });
        manifest
            .validate()
            .expect("explicit pristine default mode validates");
        let value = serde_json::to_value(&manifest).expect("serialize manifest");
        let oracle = &value["stochastic"]["oracle_seed"];
        assert_eq!(oracle["mode"], serde_json::json!("default"));
        assert!(oracle.get("strategy").expect("strategy key").is_null());
        assert!(oracle.get("seed").expect("seed key").is_null());
    }

    #[test]
    fn oracle_seed_state_fields_cannot_be_omitted() {
        let schema = load_validation_case_schema();
        for field in ["strategy", "mode", "seed"] {
            let mut raw = minimal_manifest_json();
            raw["stochastic"]["oracle_seed"]
                .as_object_mut()
                .expect("oracle object")
                .remove(field);
            assert!(
                validate_json_schema_subset(&schema, &schema, &raw, "$").is_err(),
                "JSON Schema must reject omitted oracle state field {field}"
            );
            let err = parse_json_value(&raw).expect_err("Rust parser must reject omission");
            assert!(
                err.to_string().contains(field) || err.to_string().contains("oracle_seed"),
                "unexpected error for omitted {field}: {err}"
            );
        }
    }

    #[test]
    fn requested_identity_mode_requires_seed_and_default_mode_forbids_it() {
        let mut manifest = make_minimal_manifest();
        manifest.stochastic.oracle_seed.as_mut().unwrap().seed = None;
        let err = manifest
            .validate()
            .expect_err("requested identity needs seed");
        assert!(err.to_string().contains("requested_identity"));

        let mut manifest = make_minimal_manifest();
        {
            let oracle = manifest.stochastic.oracle_seed.as_mut().unwrap();
            oracle.mode = OracleSeedMode::Default;
            oracle.seed = Some(3);
        }
        let err = manifest.validate().expect_err("default mode forbids seed");
        assert!(err.to_string().contains("mode=default"));
    }

    #[test]
    fn analytic_case_no_stochastic_allowed() {
        let mut manifest = make_minimal_manifest();
        manifest.physics_switches.turbulence = false;
        manifest.oracle_command_overrides.lturbulence = Some(0);
        manifest.stochastic = StochasticIdentitySpec::default(); // Both None
        manifest.surface = None; // No surface needed
                                 // Should validate successfully
        manifest.validate().expect("analytic case should validate");
    }

    #[test]
    fn oracle_kind_parsing() {
        assert_eq!(
            "pristine-oracle".parse::<OracleKind>().unwrap(),
            OracleKind::PristineOracle
        );
        assert_eq!(
            "seedable-validation-oracle".parse::<OracleKind>().unwrap(),
            OracleKind::SeedableValidationOracle
        );
        assert!("invalid".parse::<OracleKind>().is_err());
    }

    #[test]
    fn wind_uni_002_seed_zero_uses_declared_base_key() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("WIND-UNI-002.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load WIND-UNI-002");
        let candidate = manifest
            .stochastic
            .candidate_philox
            .as_ref()
            .expect("WIND-UNI-002 declares candidate_philox");
        assert_eq!(candidate.base_key, [3737180555, 305419896]);
        assert_eq!(candidate.base_counter, [0, 0, 0, 0]);
        let (key0, counter0) = manifest
            .candidate_seed_identity(0)
            .expect("seed 0 resolves");
        assert_eq!(key0, [3737180555, 305419896]);
        assert_eq!(counter0, [0, 0, 0, 0]);
    }

    #[test]
    fn candidate_seed_identity_rejects_index_outside_declared_count() {
        let mut manifest = make_minimal_manifest();
        let candidate = manifest
            .stochastic
            .candidate_philox
            .as_mut()
            .expect("candidate identity");
        candidate.count = 2;

        manifest
            .candidate_seed_identity(1)
            .expect("last declared seed index resolves");
        let err = manifest
            .candidate_seed_identity(2)
            .expect_err("first index outside declared count must fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("seed_index 2") && rendered.contains("[0, 2)"),
            "unexpected: {rendered}"
        );
    }

    #[test]
    fn candidate_key_derivation_wraps_u32() {
        let identity = CandidatePhiloxIdentity {
            base_key: [u32::MAX, 305419896],
            base_counter: [0, 0, 0, 0],
            count: 10,
            derivation: CandidatePhiloxDerivation::WrappingAddKey0V1,
        };
        assert_eq!(identity.key_for_seed_index(0), [u32::MAX, 305419896]);
        assert_eq!(identity.key_for_seed_index(1), [0, 305419896]);
        assert_eq!(identity.key_for_seed_index(2), [1, 305419896]);
    }

    #[test]
    fn candidate_derivation_is_executable_and_unknown_values_fail_closed() {
        let mut manifest = make_minimal_manifest();
        manifest
            .stochastic
            .candidate_philox
            .as_mut()
            .expect("candidate identity")
            .derivation = CandidatePhiloxDerivation::WrappingAddKey0V1;
        assert_ne!(
            manifest.candidate_seed_identity(0).expect("seed 0"),
            manifest.candidate_seed_identity(1).expect("seed 1")
        );

        manifest
            .stochastic
            .candidate_philox
            .as_mut()
            .expect("candidate identity")
            .derivation = CandidatePhiloxDerivation::ReuseBaseIdentityV1;
        assert_eq!(
            manifest.candidate_seed_identity(0).expect("repeat 0"),
            manifest.candidate_seed_identity(1).expect("repeat 1")
        );

        let mut raw = minimal_manifest_json();
        raw["stochastic"]["candidate_philox"]["derivation"] =
            serde_json::json!("some_future_or_misspelled_policy");
        let err = parse_json_value(&raw).expect_err("unknown derivation must fail closed");
        assert!(
            err.to_string().contains("derivation") || err.to_string().contains("unknown variant"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn stochastic_case_without_candidate_philox_is_rejected() {
        let mut manifest = make_minimal_manifest();
        assert!(manifest.physics_switches.turbulence);
        manifest.stochastic.candidate_philox = None;
        manifest.stochastic.oracle_seed = None;
        // Manifest-level validation rejects turbulence without any identity.
        assert!(manifest.validate().is_err());
        // Seed resolution also fails closed instead of substituting a key.
        assert!(manifest.candidate_seed_identity(0).is_err());
    }

    #[test]
    fn candidate_turbulence_requires_candidate_namespace_even_with_oracle_seed() {
        let mut manifest = make_minimal_manifest();
        assert!(manifest.physics_switches.turbulence);
        assert!(manifest.stochastic.oracle_seed.is_some());
        manifest.stochastic.candidate_philox = None;
        let err = manifest
            .validate()
            .expect_err("oracle namespace must not satisfy candidate RNG requirement");
        let rendered = err.to_string();
        assert!(
            rendered.contains("candidate Philox") && rendered.contains("separate RNG namespace"),
            "unexpected: {rendered}"
        );
    }

    #[test]
    fn canonical_json_requires_both_nullable_stochastic_namespace_fields() {
        let schema = load_validation_case_schema();
        for field in ["candidate_philox", "oracle_seed"] {
            let mut raw = minimal_manifest_json();
            raw["stochastic"]
                .as_object_mut()
                .expect("stochastic object")
                .remove(field);
            assert!(
                validate_json_schema_subset(&schema, &schema, &raw, "$").is_err(),
                "JSON Schema must reject missing stochastic.{field}"
            );
            let err = parse_json_value(&raw)
                .expect_err("Rust parser must reject missing namespace field");
            assert!(
                err.to_string().contains(field),
                "Rust error must name missing stochastic.{field}: {err}"
            );
        }
    }

    #[test]
    fn oracle_repetitions_are_required_without_default() {
        let schema = load_validation_case_schema();
        let mut raw = minimal_manifest_json();
        raw["stochastic"]["oracle_seed"]
            .as_object_mut()
            .expect("oracle_seed object")
            .remove("repetitions");
        assert!(
            validate_json_schema_subset(&schema, &schema, &raw, "$").is_err(),
            "JSON Schema must reject missing oracle repetitions"
        );
        let err = parse_json_value(&raw)
            .expect_err("Rust deserialization must reject missing repetitions");
        assert!(err.to_string().contains("repetitions"), "unexpected: {err}");
    }

    #[test]
    fn candidate_philox_words_are_u32_in_schema_and_rust() {
        let schema = load_validation_case_schema();
        for (field, value) in [
            ("base_key", serde_json::json!([4_294_967_296_u64, 0])),
            (
                "base_counter",
                serde_json::json!([0, 0, 0, 4_294_967_296_u64]),
            ),
        ] {
            let mut raw = minimal_manifest_json();
            raw["stochastic"]["candidate_philox"][field] = value;
            assert!(
                validate_json_schema_subset(&schema, &schema, &raw, "$").is_err(),
                "JSON Schema must reject {field} words above u32::MAX"
            );
            let err =
                parse_json_value(&raw).expect_err("Rust must reject Philox words above u32::MAX");
            assert!(
                err.to_string().contains(field) || err.to_string().contains("u32"),
                "unexpected error for {field}: {err}"
            );
        }
    }

    #[test]
    fn deterministic_round_trip_serializes_explicit_null_namespaces() {
        let mut manifest = make_minimal_manifest();
        manifest.physics_switches.turbulence = false;
        manifest.oracle_command_overrides.lturbulence = Some(0);
        manifest.stochastic = StochasticIdentitySpec::default();
        manifest.surface = None;
        manifest.validate().expect("deterministic manifest");
        let raw = serde_json::to_value(&manifest).expect("serialize manifest");
        assert!(raw["stochastic"].get("candidate_philox").is_some());
        assert!(raw["stochastic"]["candidate_philox"].is_null());
        assert!(raw["stochastic"].get("oracle_seed").is_some());
        assert!(raw["stochastic"]["oracle_seed"].is_null());
    }

    #[test]
    fn adv_ana_001_needs_no_rng_identity() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("ADV-ANA-001.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load ADV-ANA-001");
        assert!(!manifest.physics_switches.turbulence);
        assert!(manifest.stochastic.candidate_philox.is_none());
        manifest.validate().expect("ADV-ANA-001 stays valid");
        assert!(manifest.candidate_seed_identity(0).is_err());
    }

    #[test]
    fn generic_shared_seed_field_is_rejected() {
        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw["stochastic"]
            .as_object_mut()
            .expect("stochastic object")
            .insert("seed".to_string(), serde_json::json!(7));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err = ValidationCaseManifest::parse(&text, Path::new("seed.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("seed"),
            "generic shared seed must fail with a useful error: {rendered}"
        );
    }

    #[test]
    fn legacy_top_level_seeds_field_is_rejected() {
        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw.as_object_mut().expect("manifest object").insert(
            "seeds".to_string(),
            serde_json::json!({"base_philox_key": [1, 2]}),
        );
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err = ValidationCaseManifest::parse(&text, Path::new("seeds.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("seeds"),
            "legacy seeds block must fail with a useful error: {rendered}"
        );
    }
}
