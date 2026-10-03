//! Canonical schema-v2 document parsing and validated serialization.

use super::{ValidationCaseError, ValidationCaseManifest, VALIDATION_CASE_SCHEMA_VERSION};
use std::path::Path;

impl ValidationCaseManifest {
    /// Load a validation case manifest from a JSON file.
    ///
    /// # Errors
    /// Returns [`ValidationCaseError::ReadFile`] if the file cannot be read,
    /// [`ValidationCaseError::ParseJson`] if the JSON is malformed,
    /// or a validation error if the manifest fails schema validation.
    pub fn load_from_file(path: &Path) -> Result<Self, ValidationCaseError> {
        let content =
            std::fs::read_to_string(path).map_err(|source| ValidationCaseError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;
        Self::parse(&content, path)
    }

    /// Parse a validation case manifest from a JSON string.
    ///
    /// Only `schema_version` 2 documents are accepted. Legacy v1 documents
    /// (key `version`) and any other schema version are rejected with a
    /// field-specific version error; v1 is frozen and unsupported, see
    /// `fixtures/corpus/cases/MIGRATION_NOTES.md`.
    ///
    /// # Errors
    /// Returns [`ValidationCaseError::ParseJson`] if the JSON is malformed,
    /// [`ValidationCaseError::SchemaVersionMismatch`] if the schema version
    /// is missing, ambiguous, or not 2, or a validation error if the
    /// manifest fails schema validation.
    pub fn parse(content: &str, path: &Path) -> Result<Self, ValidationCaseError> {
        let raw: serde_json::Value =
            serde_json::from_str(content).map_err(|source| ValidationCaseError::ParseJson {
                path: path.to_path_buf(),
                source,
            })?;
        if raw.get("schema_version").is_some() && raw.get("version").is_some() {
            return Err(ValidationCaseError::AmbiguousField {
                field: "schema_version/version",
                message: "document contains both v2 `schema_version` and legacy v1 `version`; ambiguous mixed-version document rejected".to_string(),
            });
        }
        match (
            raw.get("schema_version").and_then(|v| v.as_u64()),
            raw.get("version").and_then(|v| v.as_u64()),
        ) {
            (Some(2), None) => {}
            (Some(actual), None) => {
                let actual = u32::try_from(actual).unwrap_or(u32::MAX);
                return Err(ValidationCaseError::SchemaVersionMismatch {
                    expected: VALIDATION_CASE_SCHEMA_VERSION,
                    actual,
                });
            }
            (None, Some(legacy)) => {
                let legacy = u32::try_from(legacy).unwrap_or(u32::MAX);
                return Err(ValidationCaseError::SchemaVersionMismatch {
                    expected: VALIDATION_CASE_SCHEMA_VERSION,
                    actual: legacy,
                });
            }
            (None, None) => {
                return Err(ValidationCaseError::MissingField {
                    field: "schema_version",
                });
            }
            _ => {
                return Err(ValidationCaseError::AmbiguousField {
                    field: "schema_version/version",
                    message: "ambiguous schema version declaration".to_string(),
                });
            }
        }
        // Oracle command overrides: reject ambiguous mixed-spelling documents
        // (legacy uppercase + canonical lowercase) before typed
        // deserialization. Legacy-only uppercase spellings are rejected by
        // `deny_unknown_fields` on `OracleCommandOverrides`, which names the
        // canonical alternative; v1 is frozen, see MIGRATION_NOTES.md.
        if let Some(overrides) = raw
            .get("oracle_command_overrides")
            .and_then(serde_json::Value::as_object)
        {
            const LEGACY_TO_CANONICAL: [(&str, &str); 5] = [
                ("LTURBULENCE", "lturbulence"),
                ("LCONVECTION", "lconvection"),
                ("CTL", "ctl"),
                ("IFINE", "ifine"),
                ("LSYNCTIME", "lsynctime_s"),
            ];
            for (legacy, canonical) in LEGACY_TO_CANONICAL {
                if overrides.contains_key(legacy) && overrides.contains_key(canonical) {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "oracle_command_overrides",
                        message: format!(
                            "document declares both `{legacy}` and `{canonical}`; \
                             ambiguous mixed-spelling oracle override rejected; \
                             keep only the canonical lowercase form"
                        ),
                    });
                }
            }
        }

        // Both RNG namespaces are explicit contract fields. Missing is not
        // equivalent to null: null means deliberately no identity for that model.
        let stochastic = raw
            .get("stochastic")
            .and_then(serde_json::Value::as_object)
            .ok_or(ValidationCaseError::MissingField {
                field: "stochastic",
            })?;
        for field in ["candidate_philox", "oracle_seed"] {
            if !stochastic.contains_key(field) {
                return Err(ValidationCaseError::MissingField {
                    field: if field == "candidate_philox" {
                        "stochastic.candidate_philox"
                    } else {
                        "stochastic.oracle_seed"
                    },
                });
            }
        }
        if let Some(oracle) = stochastic
            .get("oracle_seed")
            .and_then(serde_json::Value::as_object)
        {
            for field in ["kind", "strategy", "mode", "seed", "repetitions"] {
                if !oracle.contains_key(field) {
                    return Err(ValidationCaseError::AmbiguousField {
                        field: "stochastic.oracle_seed",
                        message: format!(
                            "{field} is required explicitly; null is distinct from omission"
                        ),
                    });
                }
            }
        }
        let manifest: Self =
            serde_json::from_value(raw).map_err(|source| ValidationCaseError::ParseJson {
                path: path.to_path_buf(),
                source,
            })?;

        // Fail-closed validation of all required fields
        manifest.validate()?;

        Ok(manifest)
    }

    /// Write the manifest to a JSON file (round-trip serialization).
    ///
    /// Validates first so invalid in-memory values can never be serialized
    /// as apparently valid contract documents.
    ///
    /// # Errors
    /// Returns the validation error if the manifest is invalid,
    /// [`ValidationCaseError::ParseJson`] if serialization fails,
    /// or [`ValidationCaseError::WriteFile`] if the file cannot be written.
    pub fn write_to_file(&self, path: &Path) -> Result<(), ValidationCaseError> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self).map_err(|source| {
            ValidationCaseError::ParseJson {
                path: path.to_path_buf(),
                source,
            }
        })?;
        std::fs::write(path, json).map_err(|source| ValidationCaseError::WriteFile {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        load_checked_in_case, load_validation_case_schema, make_minimal_manifest,
        minimal_manifest_json, validate_json_schema_subset, ALL_CHECKED_IN_CASES,
    };

    use super::super::*;
    use std::io::Write;
    use std::path::Path;
    use tempfile::NamedTempFile;

    #[test]
    fn round_trip_serialization() {
        let manifest = make_minimal_manifest();
        let mut file = NamedTempFile::new().expect("temp file");
        manifest.write_to_file(file.path()).expect("write");
        let loaded = ValidationCaseManifest::load_from_file(file.path()).expect("load");
        assert_eq!(manifest, loaded);
    }

    #[test]
    fn checked_in_cases_match_machine_readable_schema_and_rust_contract() {
        let schema = load_validation_case_schema();
        assert_eq!(
            schema.get("$schema").and_then(serde_json::Value::as_str),
            Some("https://json-schema.org/draft/2020-12/schema")
        );
        assert_eq!(
            schema
                .pointer("/properties/schema_version/const")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(VALIDATION_CASE_SCHEMA_VERSION))
        );

        for case_id in [
            "ADV-ANA-001",
            "WIND-UNI-002",
            "WIND-SHEAR-003",
            "PBL-STABLE-004",
            "PBL-NEUTRAL-005",
            "PBL-UNSTABLE-006",
            "DRY-007",
            "WET-008",
            "REPEAT-009",
            "ETEX-MINI-013",
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("corpus")
                .join("cases")
                .join(format!("{case_id}.json"));
            let text =
                std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {case_id}: {e}"));
            let raw: serde_json::Value =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse JSON {case_id}: {e}"));

            validate_json_schema_subset(&schema, &schema, &raw, "$")
                .unwrap_or_else(|e| panic!("JSON Schema rejected {case_id}: {e}"));

            ValidationCaseManifest::parse(&text, &path)
                .unwrap_or_else(|e| panic!("Rust contract rejected {case_id}: {e}"));
        }
    }

    #[test]
    fn machine_readable_schema_fails_closed_on_shape_and_version() {
        let schema = load_validation_case_schema();
        let raw = minimal_manifest_json();
        validate_json_schema_subset(&schema, &schema, &raw, "$")
            .expect("minimal Rust manifest must satisfy JSON Schema");

        let mut wrong_version = raw.clone();
        wrong_version["schema_version"] = serde_json::json!(999);
        assert!(
            validate_json_schema_subset(&schema, &schema, &wrong_version, "$").is_err(),
            "unsupported schema version must fail JSON Schema"
        );

        let mut missing_output = raw.clone();
        missing_output
            .as_object_mut()
            .expect("manifest object")
            .remove("output");
        assert!(
            validate_json_schema_subset(&schema, &schema, &missing_output, "$").is_err(),
            "missing required output semantics must fail JSON Schema"
        );

        let mut unknown_field = raw;
        unknown_field
            .as_object_mut()
            .expect("manifest object")
            .insert("hidden_default".to_string(), serde_json::json!(true));
        assert!(
            validate_json_schema_subset(&schema, &schema, &unknown_field, "$").is_err(),
            "unknown top-level fields must fail JSON Schema"
        );
    }

    #[test]
    fn schema_subset_required_precheck_preserves_one_of_semantics() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["kind"],
            "oneOf": [
                {
                    "required": ["kind", "left_value"],
                    "properties": {
                        "kind": {"const": "left"},
                        "left_value": {"type": "integer"}
                    }
                },
                {
                    "required": ["kind", "right_value"],
                    "properties": {
                        "kind": {"const": "right"},
                        "right_value": {"type": "string"}
                    }
                }
            ]
        });

        let right_branch = serde_json::json!({"kind": "right", "right_value": "ok"});
        validate_json_schema_subset(&schema, &schema, &right_branch, "$")
            .expect("one failing branch and one matching branch must satisfy oneOf");

        let missing_branch_field = serde_json::json!({"kind": "right"});
        let error = validate_json_schema_subset(&schema, &schema, &missing_branch_field, "$")
            .expect_err("branch-local required fields must still be enforced");
        assert!(error.contains("oneOf expected exactly one matching branch, got 0"));

        let overlapping_schema = serde_json::json!({
            "type": "object",
            "required": ["value"],
            "oneOf": [
                {"required": ["value"]},
                {"properties": {"value": {"type": "integer"}}}
            ]
        });
        let ambiguous = serde_json::json!({"value": 1});
        let error =
            validate_json_schema_subset(&overlapping_schema, &overlapping_schema, &ambiguous, "$")
                .expect_err("two matching branches must violate exactly-one semantics");
        assert!(error.contains("oneOf expected exactly one matching branch, got 2"));
    }

    #[test]
    fn write_failure_uses_write_file_error() {
        let manifest = make_minimal_manifest();
        let dir = tempfile::tempdir().expect("temp dir");
        let err = manifest
            .write_to_file(dir.path())
            .expect_err("writing JSON to a directory must fail");
        assert!(matches!(err, ValidationCaseError::WriteFile { .. }));
    }

    #[test]
    fn schema_version_mismatch_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.schema_version = 999;
        let mut file = NamedTempFile::new().expect("temp file");
        file.write_all(serde_json::to_string_pretty(&manifest).unwrap().as_bytes())
            .expect("write");
        let err = ValidationCaseManifest::load_from_file(file.path()).expect_err("should fail");
        assert!(matches!(
            err,
            ValidationCaseError::SchemaVersionMismatch { .. }
        ));
    }

    #[test]
    fn direct_serde_deserialization_requires_nullable_oracle_state_fields() {
        let valid = minimal_manifest_json();
        let manifest = serde_json::from_value::<ValidationCaseManifest>(valid.clone())
            .expect("direct serde must accept valid oracle strategy and seed values");
        let oracle = manifest
            .stochastic
            .oracle_seed
            .as_ref()
            .expect("oracle identity");
        assert!(oracle.strategy.is_some());
        assert_eq!(oracle.seed, Some(1));
        let serialized = serde_json::to_value(&manifest).expect("serialize valid oracle identity");
        assert_eq!(
            serialized["stochastic"]["oracle_seed"]["strategy"],
            valid["stochastic"]["oracle_seed"]["strategy"]
        );
        assert_eq!(serialized["stochastic"]["oracle_seed"]["seed"], 1);

        for field in ["strategy", "seed"] {
            let mut raw = minimal_manifest_json();
            raw["stochastic"]["oracle_seed"]
                .as_object_mut()
                .expect("oracle object")
                .remove(field);
            let err = serde_json::from_value::<ValidationCaseManifest>(raw)
                .expect_err("direct serde must reject omitted nullable oracle fields");
            assert!(
                err.to_string().contains(field),
                "direct serde error must name omitted {field}: {err}"
            );
        }

        for (field, malformed) in [
            ("strategy", serde_json::json!("not-an-object")),
            ("seed", serde_json::json!("not-an-integer")),
        ] {
            let mut raw = minimal_manifest_json();
            raw["stochastic"]["oracle_seed"][field] = malformed;
            serde_json::from_value::<ValidationCaseManifest>(raw)
                .expect_err("direct serde must reject malformed nullable oracle values");
        }

        let mut raw = minimal_manifest_json();
        raw["stochastic"]["oracle_seed"] = serde_json::json!({
            "kind": "pristine-oracle",
            "strategy": null,
            "mode": "default",
            "seed": null,
            "repetitions": 1
        });
        let manifest = serde_json::from_value::<ValidationCaseManifest>(raw)
            .expect("direct serde must accept explicit null oracle fields");
        manifest
            .validate()
            .expect("explicit null oracle state must remain contract-valid");
        let oracle = manifest.stochastic.oracle_seed.expect("oracle identity");
        assert!(oracle.strategy.is_none());
        assert!(oracle.seed.is_none());
        let serialized = serde_json::to_value(&oracle).expect("serialize null oracle identity");
        assert!(serialized.get("strategy").expect("strategy key").is_null());
        assert!(serialized.get("seed").expect("seed key").is_null());
    }

    #[test]
    fn sampling_interval_exceeding_averaging_window_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.output.sampling_interval_s = 1800;
        manifest.output.averaging_window_s = 900;
        let err = manifest.validate().expect_err("sample > average fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::AmbiguousField {
                    field: "output.sampling_interval_s",
                    ..
                }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn load_and_validate_adv_ana_001() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("ADV-ANA-001.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load ADV-ANA-001");
        assert_eq!(manifest.case_id, "ADV-ANA-001");
        assert_eq!(manifest.schema_version, 2);
        assert!(!manifest.physics_switches.turbulence);
        assert!(manifest.stochastic.candidate_philox.is_none());
        assert!(manifest.stochastic.oracle_seed.is_none());
        manifest.validate().expect("ADV-ANA-001 should validate");
    }

    #[test]
    fn load_and_validate_wind_uni_002() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("WIND-UNI-002.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load WIND-UNI-002");
        assert_eq!(manifest.case_id, "WIND-UNI-002");
        assert_eq!(manifest.schema_version, 2);
        assert!(manifest.physics_switches.turbulence);
        assert!(manifest.surface.is_some());
        assert!(manifest.stochastic.candidate_philox.is_some());
        assert!(manifest.stochastic.oracle_seed.is_some());
        let oracle = manifest.stochastic.oracle_seed.as_ref().unwrap();
        assert_eq!(oracle.kind, OracleKind::SeedableValidationOracle);
        assert_eq!(oracle.seed, Some(1));
        manifest.validate().expect("WIND-UNI-002 should validate");
    }

    #[test]
    fn round_trip_all_cases() {
        for case_id in ["ADV-ANA-001", "WIND-UNI-002", "ETEX-MINI-013"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("corpus")
                .join("cases")
                .join(format!("{case_id}.json"));
            let manifest = ValidationCaseManifest::load_from_file(&path)
                .unwrap_or_else(|e| panic!("load {case_id}: {e}"));
            let mut temp = NamedTempFile::new().expect("temp file");
            manifest.write_to_file(temp.path()).expect("write");
            let reloaded = ValidationCaseManifest::load_from_file(temp.path()).expect("reload");
            assert_eq!(manifest, reloaded, "{case_id} round-trip failed");
        }
    }

    #[test]
    fn every_checked_in_case_is_canonical_v2() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases");
        let mut found: Vec<String> = std::fs::read_dir(&dir)
            .expect("cases dir readable")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json"))
            .map(|name| name.trim_end_matches(".json").to_string())
            .collect();
        found.sort();
        let mut expected: Vec<String> =
            ALL_CHECKED_IN_CASES.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(
            found, expected,
            "no checked-in case outside the v2 contract"
        );
        for case_id in ALL_CHECKED_IN_CASES {
            let manifest = load_checked_in_case(case_id);
            assert_eq!(manifest.schema_version, VALIDATION_CASE_SCHEMA_VERSION);
            assert_eq!(manifest.case_id, *case_id);
            manifest.validate().expect("checked-in case validates");
        }
    }

    #[test]
    fn migrated_cases_keep_their_scientific_values() {
        let shear = load_checked_in_case("WIND-SHEAR-003");
        match &shear.wind {
            WindSpec::LinearShear {
                u0_m_s,
                u_shear_per_s,
                v_m_s,
                w_m_s,
            } => {
                assert_eq!(*u0_m_s, 2.0);
                assert_eq!(*u_shear_per_s, 0.004);
                assert_eq!(*v_m_s, 0.0);
                assert_eq!(*w_m_s, 0.0);
            }
            other => panic!("shear wind changed: {other:?}"),
        }
        assert_eq!(shear.release.particle_count, 1000);

        let stable = load_checked_in_case("PBL-STABLE-004");
        let surface = stable.surface.as_ref().expect("stable surface");
        assert_eq!(surface.sensible_heat_flux_w_m2, -20.0);
        assert_eq!(surface.inv_obukhov_length_per_m, 0.02);
        assert_eq!(surface.mixing_height_m, 500.0);

        let neutral = load_checked_in_case("PBL-NEUTRAL-005");
        assert_eq!(neutral.release.particle_count, 500);
        let surface = neutral.surface.as_ref().expect("neutral surface");
        assert_eq!(surface.mixing_height_m, 1500.0);
        assert_eq!(surface.sensible_heat_flux_w_m2, 0.0);

        let unstable = load_checked_in_case("PBL-UNSTABLE-006");
        let surface = unstable.surface.as_ref().expect("unstable surface");
        assert_eq!(surface.sensible_heat_flux_w_m2, 150.0);
        assert_eq!(surface.convective_velocity_scale_m_s, 1.5);
        assert_eq!(surface.mixing_height_m, 2000.0);
        assert_eq!(surface.inv_obukhov_length_per_m, -0.02);

        let dry = load_checked_in_case("DRY-007");
        assert!(dry.physics_switches.dry_deposition);
        assert!(!dry.physics_switches.wet_deposition);
        let deposition = dry.deposition.as_ref().expect("dry deposition");
        assert_eq!(deposition.dry_deposition_velocity_m_s, 0.02);
        assert_eq!(deposition.dry_reference_height_m, Some(15.0));
        assert_eq!(deposition.wet_scavenging_coefficient_s_inv, 0.0);
        assert_eq!(deposition.wet_precipitating_fraction, 0.0);

        let wet = load_checked_in_case("WET-008");
        assert!(wet.physics_switches.wet_deposition);
        assert!(!wet.physics_switches.dry_deposition);
        let deposition = wet.deposition.as_ref().expect("wet deposition");
        assert_eq!(deposition.dry_deposition_velocity_m_s, 0.0);
        assert_eq!(deposition.wet_scavenging_coefficient_s_inv, 0.005);
        assert_eq!(deposition.wet_precipitating_fraction, 1.0);
        let surface = wet.surface.as_ref().expect("wet surface");
        assert_eq!(surface.precip_large_scale_mm_h, 2.0);
        assert_eq!(surface.precip_convective_mm_h, 1.0);

        let repeat = load_checked_in_case("REPEAT-009");
        assert_eq!(repeat.release.particle_count, 500);
        let candidate = repeat
            .stochastic
            .candidate_philox
            .as_ref()
            .expect("repeat identity");
        assert_eq!(
            candidate.derivation,
            CandidatePhiloxDerivation::ReuseBaseIdentityV1
        );
        assert_eq!(candidate.count, 2);
        assert_eq!(
            repeat.candidate_seed_identity(0).expect("repeat seed 0"),
            repeat.candidate_seed_identity(1).expect("repeat seed 1")
        );
        assert!(repeat.stochastic.oracle_seed.is_none());
    }

    #[test]
    fn normalized_round_trip_all_checked_in_cases() {
        for case_id in ALL_CHECKED_IN_CASES {
            let manifest = load_checked_in_case(case_id);
            let json = serde_json::to_string_pretty(&manifest).expect("serialize");
            let reparsed =
                ValidationCaseManifest::parse(&json, Path::new(&format!("{case_id}.json")))
                    .unwrap_or_else(|e| panic!("reparse {case_id}: {e}"));
            assert_eq!(manifest, reparsed, "{case_id} round-trip failed");
        }
    }

    #[test]
    fn legacy_v1_document_is_rejected_with_version_error() {
        let doc = r#"{"version": 1, "case_id": "WIND-UNI-002"}"#;
        let err =
            ValidationCaseManifest::parse(doc, Path::new("legacy.json")).expect_err("v1 fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::SchemaVersionMismatch {
                    expected: 2,
                    actual: 1
                }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.schema_version = 999;
        let json = serde_json::to_string(&manifest).expect("serialize");
        let err =
            ValidationCaseManifest::parse(&json, Path::new("future.json")).expect_err("fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::SchemaVersionMismatch {
                    expected: 2,
                    actual: 999
                }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn mixed_version_document_is_rejected_as_ambiguous() {
        let doc = r#"{"schema_version": 2, "version": 1, "case_id": "X"}"#;
        let err = ValidationCaseManifest::parse(doc, Path::new("mixed.json")).expect_err("fails");
        assert!(
            matches!(err, ValidationCaseError::AmbiguousField { .. }),
            "unexpected: {err}"
        );
    }

    #[test]
    fn missing_version_is_rejected() {
        let doc = r#"{"case_id": "X"}"#;
        let err =
            ValidationCaseManifest::parse(doc, Path::new("noversion.json")).expect_err("fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::MissingField {
                    field: "schema_version"
                }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn unknown_top_level_field_is_rejected() {
        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw.as_object_mut()
            .expect("manifest object")
            .insert("bogus_extension".to_string(), serde_json::json!(1));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err = ValidationCaseManifest::parse(&text, Path::new("bogus.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("bogus_extension"),
            "error must name the unknown field: {rendered}"
        );
    }

    #[test]
    fn unknown_nested_field_is_rejected() {
        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw["domain"]
            .as_object_mut()
            .expect("domain object")
            .insert("nx_typo".to_string(), serde_json::json!(32));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("nested.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("nx_typo"),
            "error must name the unknown nested field: {rendered}"
        );
    }

    #[test]
    fn direct_serde_requires_notes_like_json_schema() {
        let mut raw = minimal_manifest_json();
        raw.as_object_mut()
            .expect("manifest object")
            .remove("notes");
        let err = serde_json::from_value::<ValidationCaseManifest>(raw)
            .expect_err("direct serde must reject omitted required notes");
        assert!(
            err.to_string().contains("notes"),
            "direct serde error must name omitted notes: {err}"
        );
    }

    #[test]
    fn typo_in_physics_fields_is_rejected_with_useful_error() {
        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw["release"]
            .as_object_mut()
            .expect("release object")
            .insert("particle_cout".to_string(), serde_json::json!(10));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err = ValidationCaseManifest::parse(&text, Path::new("typo.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("particle_cout"),
            "error must name the typo: {rendered}"
        );

        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .insert("lturbulance".to_string(), serde_json::json!(1));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err = ValidationCaseManifest::parse(&text, Path::new("typo2.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("lturbulance"),
            "error must name the typo: {rendered}"
        );
    }

    #[test]
    fn typo_inside_wind_variant_is_rejected() {
        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw["wind"]
            .as_object_mut()
            .expect("wind object")
            .insert("u_m_ss".to_string(), serde_json::json!(5.0));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("windtypo.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("u_m_ss"),
            "error must name the wind typo: {rendered}"
        );
    }

    /// Recursively assert the serialized manifest preserves every source key.
    ///
    /// Explicit JSON `null` in the source is equivalent to an absent key in
    /// the output (`skip_serializing_if` on optional non-contract fields);
    /// anything else must match exactly so silently discarded fields fail loudly.
    fn assert_source_keys_preserved(
        source: &serde_json::Value,
        output: &serde_json::Value,
        path: &str,
    ) {
        match (source, output) {
            (serde_json::Value::Object(source_map), serde_json::Value::Object(output_map)) => {
                for (key, source_value) in source_map {
                    let child = format!("{path}.{key}");
                    if source_value.is_null() && !output_map.contains_key(key) {
                        continue;
                    }
                    let output_value = output_map.get(key).unwrap_or_else(|| {
                        panic!("source field {child} lost during parse/serialize")
                    });
                    assert_source_keys_preserved(source_value, output_value, &child);
                }
                for key in output_map.keys() {
                    assert!(
                        source_map.contains_key(key),
                        "serialized field {path}.{key} has no source counterpart"
                    );
                }
            }
            (serde_json::Value::Array(source_items), serde_json::Value::Array(output_items)) => {
                assert_eq!(
                    source_items.len(),
                    output_items.len(),
                    "array length changed at {path}"
                );
                for (index, (source_item, output_item)) in
                    source_items.iter().zip(output_items.iter()).enumerate()
                {
                    assert_source_keys_preserved(
                        source_item,
                        output_item,
                        &format!("{path}[{index}]"),
                    );
                }
            }
            (serde_json::Value::Number(source_num), serde_json::Value::Number(output_num)) => {
                let source_f = source_num.as_f64().expect("numeric source");
                let output_f = output_num.as_f64().expect("numeric output");
                let tolerance = 1e-9 * source_f.abs().max(1.0);
                assert!(
                    (source_f - output_f).abs() <= tolerance,
                    "numeric value changed at {path}: {source_f} vs {output_f}"
                );
            }
            _ => assert_eq!(source, output, "value changed at {path}"),
        }
    }

    #[test]
    fn source_documents_survive_parse_and_serialize_without_loss() {
        for case_id in ALL_CHECKED_IN_CASES {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("corpus")
                .join("cases")
                .join(format!("{case_id}.json"));
            let text = std::fs::read_to_string(&path).expect("read source");
            let source: serde_json::Value = serde_json::from_str(&text).expect("source parses");
            let manifest = ValidationCaseManifest::load_from_file(&path)
                .unwrap_or_else(|e| panic!("load {case_id}: {e}"));
            let serialized = serde_json::to_string_pretty(&manifest).expect("serialize");
            let output: serde_json::Value =
                serde_json::from_str(&serialized).expect("output parses");
            assert_source_keys_preserved(&source, &output, case_id);
        }
    }
}
